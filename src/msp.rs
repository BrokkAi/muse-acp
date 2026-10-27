//! The adapter's Muse hosts: one `muse serve` each, driven through the
//! `muse-msp-client` crate, plus the adapter's MSP diagnostics and error
//! mapping.
//!
//! Model (per the Muse Code developer docs + the schema the host ships):
//! commands carry caller-minted `commandId`s; the ack is not the outcome —
//! `item/*` and `turn/*` notifications report what happened. Server-initiated
//! `approval/request` gets an immediate `{}` ("handling it"); the verdict
//! travels separately via `approval/decide`.
//!
//! The crate owns the transport, request correlation, exit classification,
//! and bounded shutdown. This module keeps what belongs to the adapter: Muse
//! discovery and its environment overrides, the settings view, log lines,
//! the shutdown deadline, compatibility classification, and the error shapes
//! the editor sees.

use std::fmt;
use std::sync::{Arc, Mutex, mpsc::Receiver};

use muse_msp_client::{
    Capabilities, ClientInfo, ClientOptions, Diagnostic, Handshake, HandshakeError, LaunchConfig,
    MspClient, MspError, Params, ServerRequestMode,
};
pub use muse_msp_client::{ExitClassification, ExitKind, HostFlags, MspEvent};

use crate::compat;
use crate::json::{J, j_to_string, parse_json};

/// Host identification captured from the MSP initialize handshake.
#[derive(Debug, Clone, Default)]
pub struct HandshakeInfo {
    pub server_name: String,
    pub server_version: String,
    pub schema_version: Option<u64>,
    pub fingerprint: String,
    pub status: &'static str,
    pub detail: &'static str,
    /// `sessionDurability` from the handshake. Absent means durable (the
    /// schema's compatibility rule); unknown values are treated as ephemeral.
    pub durability: Option<String>,
    /// Whether this connection was granted the opt-in live session listing
    /// stream. An absent grant deliberately means the legacy poll fallback.
    pub session_list_stream: bool,
    pub user_shell: bool,
    /// Whether `session/start` and `session/resume` may carry
    /// `config.mcpServers`. Without the grant a non-empty map is rejected
    /// with `capabilityRequired`, so client MCP servers are dropped instead.
    pub session_mcp: bool,
    /// Whether `feedback/submit` is granted (Muse 1.4.2+). The adapter only
    /// advertises `/feedback` on such a host.
    pub feedback: bool,
    /// Whether the `sessionRename` capability was requested and granted
    /// (Muse 1.4.8+). `session/rename` backs the editor's `/rename <name>`
    /// command; a host that withholds the grant gets the typed
    /// `capabilityRequired` refusal instead of the call.
    pub session_rename: bool,
    /// Whether the `computerUseSetup` capability was requested and granted.
    /// The adapter's computer-use surface stays dark without it.
    pub computer_use_setup: bool,
    /// Whether the host-wide `computerUseHostSetup` capability was requested
    /// and granted. Host setup reads and checks stay dark without it.
    pub computer_use_host_setup: bool,
    /// `sandboxAvailability` from the handshake (Muse 1.4.7+), verbatim. A
    /// property of the host process, not a capability: nothing is requested
    /// or granted, and absent means unknown (the host predates the field).
    /// Read it through [`HandshakeInfo::sandbox_posture`].
    pub sandbox_availability: Option<String>,
    /// `sandboxUnavailableReason` from the handshake, verbatim. Only
    /// meaningful beside `unavailable`; read it through
    /// [`HandshakeInfo::sandbox_reason`].
    pub sandbox_unavailable_reason: Option<String>,
}

impl HandshakeInfo {
    /// Whether a dead host of this profile may be restarted and its sessions
    /// re-attached. Only `durable` (including the absent-means-durable arm)
    /// carries the recovery guarantee.
    pub fn restartable(&self) -> bool {
        matches!(self.durability.as_deref(), None | Some("durable"))
    }

    /// The leading `MAJOR.MINOR.PATCH` of `serverInfo.version`, ignoring any
    /// suffix such as the `-R4684.1` build label. `None` when the version
    /// does not start with three dot-separated decimal numbers.
    fn release(&self) -> Option<(u64, u64, u64)> {
        muse_msp_client::parse_release(&self.server_version)
    }

    /// Whether the host reports at least this release. An unparseable
    /// version answers false, so a feature gated on it stays off. Features
    /// are gated on the reported version rather than the fingerprint table,
    /// which only knows released builds.
    pub fn at_least(&self, major: u64, minor: u64, patch: u64) -> bool {
        self.release()
            .is_some_and(|release| release >= (major, minor, patch))
    }

    /// MSP `session/delete` (Muse 1.4.1 and 1.4.2). A memory-only host has
    /// nothing durable to delete and does not offer the method, so this also
    /// needs the durable profile. Muse 1.4.3 dropped the method from its
    /// schema and answers it with `methodNotFound`, so later releases stay
    /// off until one is shown to serve it again.
    pub fn supports_session_delete(&self) -> bool {
        self.restartable() && self.at_least(1, 4, 1) && !self.at_least(1, 4, 3)
    }

    /// `workspaceRoots` on `session/start` and `turn/start` (Muse 1.4.1+).
    pub fn supports_workspace_roots(&self) -> bool {
        self.at_least(1, 4, 1)
    }

    /// Host-computed `cost` and cache totals on cumulative token usage
    /// (Muse 1.4.2+).
    pub fn reports_session_cost(&self) -> bool {
        self.at_least(1, 4, 2)
    }

    /// One machine-readable line naming the version-gated MSP features this
    /// host offers.
    pub fn features_line(&self) -> String {
        format!(
            "host-features server={} session_delete={} workspace_roots={} session_cost={}",
            self.host_label(),
            self.supports_session_delete(),
            self.supports_workspace_roots(),
            self.reports_session_cost()
        )
    }

    /// Whether `/rename` may call `session/rename` on this connection.
    ///
    /// Muse 1.4.8 added the `sessionRename` capability to its grantable
    /// registry. A host that predates it ignores the unknown name in
    /// `requestedCapabilities` and withholds it from `grantedCapabilities`,
    /// yet still serves the method: the adapter has driven `/rename` that way
    /// since before the registry entry existed, so an absent grant there is
    /// not a refusal. From 1.4.8 on the name is registered, so a withheld
    /// grant is the host withholding the capability: the adapter answers
    /// `/rename` with the typed `capabilityRequired` instead of sending a
    /// call the host may reject.
    pub fn allows_session_rename(&self) -> bool {
        self.session_rename || !self.at_least(1, 4, 8)
    }

    /// Whether this host can enforce its sandbox: `available`,
    /// `unavailable`, or `unknown`. The vocabulary is open, so an absent
    /// field (a host before 1.4.7) and a value this adapter does not know
    /// both read as `unknown`; neither is guessed into an answer.
    pub fn sandbox_posture(&self) -> &'static str {
        match self.sandbox_availability.as_deref() {
            Some("available") => "available",
            Some("unavailable") => "unavailable",
            _ => "unknown",
        }
    }

    /// Why the host cannot enforce its sandbox, only when it reported
    /// `unavailable` and sent a reason. A reason this adapter does not know
    /// reads as `unknown`; a reason beside any other posture is ignored, and
    /// none is ever inferred.
    pub fn sandbox_reason(&self) -> Option<&'static str> {
        if self.sandbox_posture() != "unavailable" {
            return None;
        }
        match self.sandbox_unavailable_reason.as_deref()? {
            "userNamespacesDenied" => Some("userNamespacesDenied"),
            "bubblewrapUnavailable" => Some("bubblewrapUnavailable"),
            _ => Some("unknown"),
        }
    }

    /// One machine-readable line with the host's sandbox facts, beside
    /// `schema-compat` and `host-features`. `reason=none` means the host
    /// sent no reason (or the posture is not `unavailable`).
    pub fn sandbox_line(&self) -> String {
        format!(
            "host-sandbox server={} availability={} reason={}",
            self.host_label(),
            self.sandbox_posture(),
            self.sandbox_reason().unwrap_or("none")
        )
    }

    /// A human warning when the host reports it cannot enforce its sandbox.
    /// The adapter gates nothing on this; it only makes the posture visible.
    pub fn sandbox_warning(&self) -> Option<String> {
        if self.sandbox_posture() != "unavailable" {
            return None;
        }
        Some(format!(
            "warning: this Muse host reports it cannot enforce its sandbox{}; tool runs are not isolated by it",
            self.sandbox_reason()
                .map(|reason| format!(" (reason: {reason})"))
                .unwrap_or_default()
        ))
    }
}

impl HandshakeInfo {
    pub fn host_label(&self) -> String {
        format!("{}/{}", self.server_name, self.server_version)
    }
}

/// Diagnostic level from `MUSE_LOG`: `normal` (default) or `debug`.
/// Debug adds per-method tracing for protocol drift investigations; there is
/// deliberately no payload logging, so tracing cannot leak file contents or
/// credentials.
pub fn debug_enabled() -> bool {
    std::env::var("MUSE_LOG")
        .map(|v| v.eq_ignore_ascii_case("debug"))
        .unwrap_or(false)
}

pub fn log(msg: &str) {
    eprintln!("[muse-acp] {msg}");
}

/// Opt-in method tracing: names and ids only, never payloads.
pub fn trace_method(direction: &str, method: &str) {
    if debug_enabled() {
        eprintln!("[muse-acp] trace {direction} method={method}");
    }
}

/// Resolve a command timeout from an optional environment override
/// (`MUSE_COMMAND_TIMEOUT_MS`, milliseconds) and the client's method table.
pub fn command_timeout(env_override: Option<&str>, method: &str) -> std::time::Duration {
    timeout_override(env_override).unwrap_or_else(|| muse_msp_client::default_timeout(method))
}

/// A positive `MUSE_COMMAND_TIMEOUT_MS`. Invalid or non-positive values
/// must not silently disable the timeout, so they fall back to the table.
fn timeout_override(env_override: Option<&str>) -> Option<std::time::Duration> {
    let ms = env_override?.trim().parse::<u64>().ok()?;
    (ms > 0).then(|| std::time::Duration::from_millis(ms))
}

/// Guidance for a host that refuses to open a session over its permission
/// profile (seen live on 1.2.1: `compose session permission profile:
/// ... ':auto-review' ... reviewer is unavailable`). Profiles compose
/// host-side from Muse settings — nothing on the MSP wire selects one.
/// The launch configuration handles the built-in auto-review profile;
/// other profile refusals still need an actionable diagnostic. Returns
/// None for unrelated errors; the caller keeps the host text either way.
pub fn session_profile_hint(host_message: &str) -> Option<String> {
    // A retained-session refusal also *composes* a permission profile, but its
    // cause is the lost durable record, not the settings: the profile was
    // never consulted. Pointing at `permissions.default_profile` would send
    // the user to the wrong place, so those refusals get no hint.
    if host_message.contains("retained session refused")
        || host_message.contains("retained durable session")
    {
        return None;
    }
    if !host_message.contains("permission profile") {
        return None;
    }
    let profile = host_message
        .split("permission profile '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .filter(|name| !name.is_empty())
        .map(|name| format!(" ({name})"))
        .unwrap_or_default();
    let view = crate::host_config::view_failure()
        .map(|error| {
            format!(
                " It could not do so for this host: preparing its settings view failed ({error})."
            )
        })
        .unwrap_or_default();
    Some(format!(
        " Hint: muse serve refused its permission profile{profile}. Check Muse settings (`permissions.default_profile`) and managed policy. muse-acp substitutes :ask-me for the saved built-in :auto-review profile at host launch; other profiles must be usable by muse serve.{view}"
    ))
}

/// The Muse executable: `MUSE_CLI`, else `muse` from PATH, else the Muse
/// installer's default location. On Windows the installer ships `muse.cmd`,
/// which `Command::new("muse")` never finds (it only appends `.exe`), so
/// search for the launcher scripts too. Editors often start agents without
/// the installer's directory on PATH, which the last fallback covers,
/// including right after `muse-acp login` installed Muse.
pub fn muse_cli() -> String {
    if let Ok(bin) = std::env::var("MUSE_CLI") {
        return bin;
    }
    let path_dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let install_dir = default_install_dir(
        std::env::var_os("MUSE_INSTALL_DIR"),
        std::env::var_os(if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "HOME"
        }),
    );
    if cfg!(windows) {
        return find_windows_launcher(path_dirs.into_iter().chain(install_dir), "muse")
            .unwrap_or_else(|| "muse".to_string());
    }
    // A bare name keeps symlinked installs (Homebrew, cargo) working across
    // upgrades, so the installer path is only used when PATH has no `muse`.
    let on_path = path_dirs
        .iter()
        .any(|dir| dir.is_absolute() && dir.join("muse").is_file());
    if !on_path
        && let Some(bin) = install_dir
            .map(|dir| dir.join("muse"))
            .filter(|bin| bin.is_file())
            .and_then(|bin| bin.into_os_string().into_string().ok())
    {
        return bin;
    }
    "muse".to_string()
}

/// Where Muse's official installer puts `muse`: `MUSE_INSTALL_DIR` when set,
/// else `<HOME>/.local/bin`, or `<LOCALAPPDATA>\Programs\muse` on Windows.
fn default_install_dir(
    install_dir: Option<std::ffi::OsString>,
    base: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    if let Some(dir) = install_dir.filter(|dir| !dir.is_empty()) {
        return Some(dir.into());
    }
    let base = std::path::PathBuf::from(base.filter(|base| !base.is_empty())?);
    Some(if cfg!(windows) {
        base.join("Programs").join("muse")
    } else {
        base.join(".local").join("bin")
    })
}

/// First `<stem>.exe`, `.cmd`, or `.bat` in PATH order, as a full path.
/// Empty and relative entries (a trailing `;`, `.`) are skipped: they resolve
/// against the working directory, often an untrusted project checkout, which
/// Rust's own Windows search also refuses to consult.
fn find_windows_launcher(
    dirs: impl Iterator<Item = std::path::PathBuf>,
    stem: &str,
) -> Option<String> {
    dirs.filter(|dir| dir.is_absolute())
        .flat_map(|dir| ["exe", "cmd", "bat"].map(|ext| dir.join(format!("{stem}.{ext}"))))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.into_os_string().into_string().ok())
}

/// Turn a host spawn failure into the next user action. The readiness
/// diagnosis starts at "can we even run the CLI"; session errors carry host
/// text from there.
pub fn describe_spawn_error(bin: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "Muse CLI not found: '{bin}'. Install Muse Code \
             (https://dev.meta.ai/docs/muse-code), ensure it is on PATH, or set \
             MUSE_CLI=/absolute/path/to/muse"
        ),
        std::io::ErrorKind::PermissionDenied => format!(
            "Muse CLI is not executable: '{bin}'. Fix permissions or set \
             MUSE_CLI=/absolute/path/to/muse"
        ),
        _ => format!("failed to spawn '{bin} serve': {e}"),
    }
}

/// Match explicit login diagnostics in free-text host errors; a bare 401/403
/// or permission denial may belong to a tool. Never echo raw authentication
/// errors: they can contain credentials.
pub fn auth_failure(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    if [
        "session expired",
        "session has expired",
        "token expired",
        "token has expired",
        "credentials expired",
        "credentials have expired",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        Some("Muse session expired")
    } else if [
        "not authenticated",
        "unauthenticated",
        "not logged in",
        "not signed in",
        "authentication required",
        "please log in",
        "please login",
        "run `muse login`",
        "run muse login",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        Some("Muse is not authenticated")
    } else {
        None
    }
}

/// Read an `account/read` result (MSP `AccountState`). `credentialRequired`
/// is `false` on keyless deployments, which never need a login.
pub fn account_logged_out(state: &J) -> bool {
    state.get("state").and_then(J::as_str) == Some("loggedOut")
        && !matches!(state.get("credentialRequired"), Some(J::Bool(false)))
}

pub fn auth_diagnostic(message: &str, host: &HandshakeInfo) -> Option<String> {
    let failure = auth_failure(message)?;
    let bin = muse_cli();
    let host_label = if host.server_version.is_empty() {
        "unreported (initialize did not complete)".to_string()
    } else {
        host.host_label()
    };
    Some(format!(
        "{failure}. Run `muse login` using the configured Muse executable ({bin}) on the machine and OS account running muse-acp, then restart the editor agent and retry. Host: {}. For browserless/remote login options, run `muse login --help` in that environment.",
        host_label
    ))
}

/// Turn failures can describe tools and other services used by the model.
/// The stable MSP 1.3.0 `authRequired` kind is authoritative; older hosts
/// still need explicit Muse wording (or the model-error default) so service
/// authentication failures keep their own diagnostic.
pub fn turn_auth_diagnostic(
    message: &str,
    error_kind: &str,
    host: &HandshakeInfo,
) -> Option<String> {
    if error_kind == "authRequired" {
        // The kind itself is the authentication signal, so do not depend on
        // the host's free-text detail containing a recognizable phrase. Use
        // the same redacted guidance as other Muse auth failures.
        return auth_diagnostic("authentication required", host);
    }
    if error_kind != "modelError" && !message.to_ascii_lowercase().contains("muse") {
        return None;
    }
    auth_diagnostic(message, host)
}

/// ACP reserves -32000 for authentication required. MSP's stable
/// protocol-surface errors (command rejections, a rejected skill lookup, and
/// the plugin and marketplace write refusals) keep their registry codes;
/// other MSP errors retain the caller's existing ACP mapping.
pub fn acp_error_code(error: &J, fallback: i64) -> i64 {
    if auth_failure(&err_message(error)).is_some() {
        return -32000;
    }
    let kind = error
        .get("data")
        .and_then(|data| data.get("kind"))
        .and_then(|kind| kind.as_str())
        .unwrap_or("");
    match kind {
        "commandRejected" => -32030,
        "skillNotFound" => -32032,
        "marketplaceNotFound" => -32034,
        "pluginNotFound" => -32035,
        "marketplaceAlreadyExists" => -32036,
        "pluginMutationsDisabled" => -32037,
        "computerUseUnavailable" => -32033,
        "capabilityRequired" => -32010,
        _ if err_code(error) == -32032 => -32032,
        _ => fallback,
    }
}

/// Editor-facing wording for a host exit. It names `muse-acp` and its
/// launch settings, so it belongs to the adapter rather than the client
/// crate, which classifies the exit. Show editors `editor_message()`, never
/// the classification's `Display` text, which is the crate's own wording.
pub trait ExitMessage {
    /// Message suitable for an editor-facing terminal or launch diagnostic.
    fn editor_message(&self) -> String;
}

impl ExitMessage for ExitClassification {
    fn editor_message(&self) -> String {
        match self.kind {
            ExitKind::CleanShutdown => {
                "Muse serve shut down cleanly after stdin closed; the session was durably closed."
                    .to_string()
            }
            ExitKind::ConfigError => {
                "Muse serve rejected its configuration (exit code 3). Fix the Muse configuration, then restart muse-acp; retrying without that fix will fail."
                    .to_string()
            }
            ExitKind::SdkSurfaceUnavailable => {
                "Muse serve is unavailable because the Muse SDK surface is switched off (exit code 5). Enable the Muse serve/SDK surface in Muse, then restart muse-acp; changing serve arguments will not help."
                    .to_string()
            }
            ExitKind::UsageError => {
                "Muse serve rejected its arguments (exit code 2). Fix MUSE_SERVE_ARGS or the configured launch arguments, then restart muse-acp."
                    .to_string()
            }
            ExitKind::LeaseUnavailable => {
                "Muse serve could not start because another client holds its session lease (exit code 4). Close that client, then restart muse-acp."
                    .to_string()
            }
            ExitKind::UnhandledError => {
                "Muse serve stopped with an unhandled error (exit code 1). Check the host diagnostics and restart muse-acp."
                    .to_string()
            }
            ExitKind::Crash => match (self.exit_code, self.signal) {
                (Some(code), _) => format!(
                    "Muse serve crashed with exit code {code}. Check the host diagnostics and restart muse-acp."
                ),
                (_, Some(signal)) => format!(
                    "Muse serve was terminated by signal {signal}. Check the host diagnostics and restart muse-acp."
                ),
                _ => "Muse serve stopped unexpectedly. Check the host diagnostics and restart muse-acp."
                    .to_string(),
            },
        }
    }
}

#[derive(Debug)]
pub enum LaunchError {
    /// No Muse executable exists at the resolved path.
    NotInstalled(String),
    Spawn(String),
    Startup(String),
    HostExit(ExitClassification),
}

impl LaunchError {
    pub fn retryable(&self) -> bool {
        match self {
            LaunchError::NotInstalled(_) | LaunchError::Spawn(_) => false,
            LaunchError::Startup(_) => true,
            LaunchError::HostExit(exit) => exit.retryable(),
        }
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::NotInstalled(message)
            | LaunchError::Spawn(message)
            | LaunchError::Startup(message) => f.write_str(message),
            LaunchError::HostExit(exit) => f.write_str(&exit.editor_message()),
        }
    }
}

/// Structured data for the MSP request errors ACP clients need to branch on:
/// rejected commands and skill selectors, plugin/marketplace write refusals,
/// and capability-gated methods. The host payload stays visible so a client
/// sees which command, selector, marketplace, plugin, or capability was
/// refused.
pub fn typed_error_data(error: &J) -> Option<String> {
    let data = error.get("data")?;
    let kind = data
        .get("kind")
        .and_then(|kind| kind.as_str())
        .unwrap_or("");
    let typed = matches!(
        kind,
        "commandRejected"
            | "skillNotFound"
            | "marketplaceNotFound"
            | "pluginNotFound"
            | "marketplaceAlreadyExists"
            | "pluginMutationsDisabled"
            | "computerUseUnavailable"
            | "capabilityRequired"
    );
    typed.then(|| j_to_string(data))
}

/// The host's error shape for one capability-gated method on a connection
/// that was not granted the capability (MSP §1.6): code `-32010`, with
/// `data.kind` `capabilityRequired` and the missing `capability` named. The
/// adapter returns this same shape itself when it knows the grant was
/// withheld, so an editor sees the typed refusal rather than an unexplained
/// failure or a call the host would reject.
pub fn capability_required_error(method: &str, capability: &str) -> J {
    let message = format!("{method} requires the {capability} capability");
    parse_json(&format!(
        r#"{{"code":-32010,"message":{},"data":{{"capability":{},"kind":"capabilityRequired","retryable":false}}}}"#,
        crate::json::esc(&message),
        crate::json::esc(capability)
    ))
    .expect("capability_required_error builds valid JSON")
}

/// One running `muse serve`: the crate's client plus the adapter's view of
/// its handshake and the private settings view it was launched with.
pub struct MspHost {
    /// Shared so a host dropped without a close can finish closing in the
    /// background before its settings view is removed.
    client: Arc<MspClient>,
    handshake: Mutex<HandshakeInfo>,
    host_config: Mutex<Option<crate::host_config::HostConfig>>,
}

impl MspHost {
    /// Last-resort kill without waiting for a lock or child progress.
    pub fn force_stop(&self) {
        if !self.client.force_stop() {
            return;
        }
        // The deadline exits the whole adapter without running Drop, so clean
        // up the settings view here even if the main loop is blocked writing
        // to the editor. The client re-arms its reap budget after `taskkill`
        // and reports success whenever it reached the child, so this cleanup
        // is never skipped; skipping it would leak the temporary view.
        let config = match self.host_config.try_lock() {
            Ok(mut config) => config.take(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        drop(config);
    }

    /// Brings changes made to the real Muse config while the host runs, such
    /// as a login in a terminal, into its private settings view, if any.
    pub fn refresh_config(&self) {
        if let Some(config) = self
            .host_config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            config.refresh();
        }
    }

    /// Handshake facts captured at launch, for diagnostics and support output.
    pub fn handshake(&self) -> HandshakeInfo {
        self.handshake
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Best-effort reap of an exited host child and classification of its
    /// status. EOF has already been observed when the normal caller invokes
    /// this, so reaping also gives the stderr reader a short bounded drain
    /// window before the evidence is exposed. The client arms the shutdown
    /// deadline around it.
    pub fn reap(&self) -> Option<ExitClassification> {
        let exit = self.client.reap()?;
        // Removing (or, on Windows, restoring) the view touches the disk.
        let _deadline = crate::shutdown::deadline("settings view cleanup");
        self.host_config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        Some(exit)
    }

    /// Close the host input, give it a short chance to exit cleanly, then kill
    /// and reap it. Lock acquisition and child progress are both bounded so a
    /// broken host cannot strand adapter shutdown.
    pub fn shutdown(&self) {
        if self.client.close().is_some() {
            let _deadline = crate::shutdown::deadline("settings view cleanup");
            self.host_config
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
        }
    }
}

impl Drop for MspHost {
    /// Every adapter path closes a host before dropping it. Should one ever
    /// be dropped while running, the settings view must outlive the child,
    /// which may still write `auth.json` or `trust.json` there: close it in
    /// the background first, then remove the view.
    fn drop(&mut self) {
        let config = self
            .host_config
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some(config) = config else {
            return;
        };
        if self.client.is_closing() {
            return;
        }
        let client = self.client.clone();
        let _ = std::thread::Builder::new()
            .name("muse-host-drop".into())
            .spawn(move || {
                client.close();
                drop(config);
            });
    }
}

impl MspHost {
    /// Launches `muse serve` with `MUSE_SERVE_ARGS`, `extra_args`, then the
    /// host flags.
    pub fn launch(
        user_input_dialogs: bool,
        user_shell: bool,
        computer_use: bool,
        flags: HostFlags,
        extra_args: &[&str],
    ) -> Result<(Arc<MspHost>, Receiver<MspEvent>), LaunchError> {
        let bin = muse_cli();
        let (mut config, host_config) = serve_config(&bin, flags, extra_args);
        config.options(client_options(user_input_dialogs, user_shell, computer_use));
        let (client, events) = MspClient::spawn(config).map_err(|error| match error {
            muse_msp_client::LaunchError::Spawn { error, .. } => {
                let message = describe_spawn_error(&bin, &error);
                if error.kind() == std::io::ErrorKind::NotFound {
                    LaunchError::NotInstalled(message)
                } else {
                    LaunchError::Spawn(message)
                }
            }
            muse_msp_client::LaunchError::HostExit(exit) => LaunchError::HostExit(exit),
            other => LaunchError::Startup(other.to_string()),
        })?;
        let host = Arc::new(MspHost {
            client: Arc::new(client),
            handshake: Mutex::new(HandshakeInfo::default()),
            host_config: Mutex::new(host_config),
        });
        crate::shutdown::register_host(&host);
        match host.client.initialize() {
            Ok(handshake) => {
                *host.handshake.lock().unwrap_or_else(|p| p.into_inner()) =
                    handshake_info(&handshake, user_shell, computer_use);
                Ok((host, events))
            }
            Err(HandshakeError::Request(error)) => {
                let error = host.command_error(error, "");
                let message = format!("serve initialize failed: {}", err_message(&error));
                if let Some(exit) = host.reap() {
                    return Err(LaunchError::HostExit(exit));
                }
                host.shutdown();
                Err(LaunchError::Startup(message))
            }
            // `compatibility_check` refused the host and logged why.
            Err(HandshakeError::Rejected(message)) => {
                host.shutdown();
                Err(LaunchError::Startup(message))
            }
            Err(HandshakeError::Initialized(error)) => {
                if let Some(exit) = host.reap() {
                    return Err(LaunchError::HostExit(exit));
                }
                host.shutdown();
                Err(LaunchError::Startup(match error {
                    MspError::Write(e) => format!("serve notify failed: {e}"),
                    other => format!("serve notify failed: {other}"),
                }))
            }
        }
    }

    /// UUIDv7 command ids: the host rejects anything else.
    pub fn mint_cmd(&self, _prefix: &str) -> String {
        self.client.mint_command_id()
    }

    /// True only when the experimental `account/read` positively reports no
    /// credential on a deployment that needs one. Any error or unexpected
    /// shape (an older host, a changed experimental surface) reads as "not
    /// known to be logged out", so the first turn's auth error still applies.
    /// The result names a credential lane, never key material.
    pub fn logged_out(&self) -> bool {
        match self.command("account/read", "{}") {
            Ok(state) => account_logged_out(&state),
            Err(error) => {
                log(&format!(
                    "account/read unavailable: {}",
                    err_message(&error)
                ));
                false
            }
        }
    }

    /// Send a command; Ok(result) / Err(error object).
    pub fn command(&self, method: &str, params_json: &str) -> Result<J, J> {
        let timeout = command_timeout(
            std::env::var("MUSE_COMMAND_TIMEOUT_MS").ok().as_deref(),
            method,
        );
        self.client
            .request_with_timeout(method, Params::raw(params_json), timeout)
            .map_err(|error| self.command_error(error, params_json))
    }

    /// The error object a failed command has always produced: host errors
    /// verbatim (or as the redacted login guidance), and local failures as
    /// `-32603` with the reaped exit message when the host is gone.
    fn command_error(&self, error: MspError, params_json: &str) -> J {
        let auth = |error: J| {
            auth_diagnostic(&err_message(&error), &self.handshake())
                .map(|message| mk_err(-32000, &message))
                .unwrap_or(error)
        };
        match error {
            MspError::Rpc(error) => auth(error),
            MspError::Closed(reason) => auth(mk_err(-32603, &reason)),
            MspError::Timeout {
                method,
                id,
                timeout,
            } => mk_err(
                -32603,
                &format!(
                    "serve command timed out after {}ms method={method} id={id}{}",
                    timeout.as_millis(),
                    session_suffix(params_json)
                ),
            ),
            MspError::HostClosed => mk_err(
                -32603,
                &self.host_closed_message("serve host closed the connection"),
            ),
            MspError::Write(e) => {
                let fallback = format!("serve write failed: {e}");
                if e.kind() == std::io::ErrorKind::TimedOut {
                    mk_err(-32603, &fallback)
                } else {
                    mk_err(-32603, &self.host_closed_message(&fallback))
                }
            }
            // The adapter never cancels a command it is waiting for.
            other => mk_err(-32603, &other.to_string()),
        }
    }

    fn host_closed_message(&self, fallback: &str) -> String {
        self.reap()
            .map(|exit| exit.editor_message())
            .unwrap_or_else(|| fallback.to_string())
    }
}

/// The client configuration every adapter host shares.
fn client_options(user_input_dialogs: bool, user_shell: bool, computer_use: bool) -> ClientOptions {
    // `sessionRename` is requested unconditionally and last among the
    // always-on registry names: `/rename` maps onto `session/rename`, and
    // Muse 1.4.8 registers the capability as grantable. Requesting it on
    // an older host is harmless (unknown names are ignored).
    let mut requested = vec![
        "sessionListStream",
        "sessionMcp",
        "feedback",
        "sessionRename",
    ];
    if user_shell {
        requested.push("userShell");
    }
    if computer_use {
        requested.extend(["computerUseSetup", "computerUseHostSetup"]);
    }
    // `experimentalApi` is the only way to reach `account/read`, which
    // lets session/new report a logged-out host before the first turn
    // (see `MspHost::logged_out`). On Muse 1.4.0 it gates nothing else.
    let mut options = ClientOptions::default()
        .client_info(ClientInfo {
            name: "muse_acp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            title: None,
        })
        .capabilities(Capabilities {
            experimental_api: Some(true),
            user_input_dialogs: Some(user_input_dialogs),
            requested: requested.into_iter().map(str::to_string).collect(),
            opt_out_notification_methods: Vec::new(),
        })
        .diagnostics(|diagnostic| match diagnostic {
            Diagnostic::Notification { method } => trace_method("msp<-host", method),
            other => log(&other.to_string()),
        })
        .watchdog(crate::shutdown::deadline)
        .handshake_check(compatibility_check);
    // The handshake honors the same override as every later command.
    if let Some(timeout) =
        timeout_override(std::env::var("MUSE_COMMAND_TIMEOUT_MS").ok().as_deref())
    {
        options = options.request_timeout(timeout);
    }
    // Known server requests are acked `{}` before they are forwarded:
    // reissued multi-stage/resumed requests carry their own choices.
    // Everything else gets the typed methodNotFound error from the client.
    for method in HANDLED_SERVER_REQUESTS {
        options = options.server_request(method, ServerRequestMode::Acknowledge);
    }
    options
}

/// The adapter's compatibility verdict for a handshake.
fn compatibility(handshake: &Handshake) -> compat::Compatibility {
    compat::classify(
        handshake.schema_version,
        handshake.fingerprint.as_deref().unwrap_or("?"),
    )
}

/// Log the `schema-compat` line and refuse an incompatible host before the
/// handshake closes.
fn compatibility_check(handshake: &Handshake) -> Result<(), String> {
    let verdict = compatibility(handshake);
    let host_label = format!(
        "{}/{}",
        handshake.server_name.as_deref().unwrap_or("unknown"),
        handshake.server_version.as_deref().unwrap_or("unknown")
    );
    log(&verdict.log_line(env!("CARGO_PKG_VERSION"), &host_label));
    if verdict.is_fatal() {
        return Err(format!(
            "incompatible host schema: version={} fingerprint={}; upgrade muse-acp",
            verdict
                .schema_version
                .map(|v| v.to_string())
                .unwrap_or_else(|| "absent".into()),
            verdict.fingerprint
        ));
    }
    Ok(())
}

/// The adapter's handshake facts. Shell and computer-use grants count only
/// when this launch requested them.
fn handshake_info(handshake: &Handshake, user_shell: bool, computer_use: bool) -> HandshakeInfo {
    let verdict = compatibility(handshake);
    HandshakeInfo {
        server_name: handshake
            .server_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        server_version: handshake
            .server_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        schema_version: handshake.schema_version,
        fingerprint: verdict.fingerprint.clone(),
        status: verdict.status.as_str(),
        detail: verdict.detail,
        durability: handshake.durability.clone(),
        session_list_stream: handshake.granted("sessionListStream"),
        user_shell: user_shell && handshake.granted("userShell"),
        session_mcp: handshake.granted("sessionMcp"),
        feedback: handshake.granted("feedback"),
        session_rename: handshake.granted("sessionRename"),
        computer_use_setup: computer_use && handshake.granted("computerUseSetup"),
        computer_use_host_setup: computer_use && handshake.granted("computerUseHostSetup"),
        sandbox_availability: handshake.sandbox_availability.clone(),
        sandbox_unavailable_reason: handshake.sandbox_unavailable_reason.clone(),
    }
}

/// All host launches, including restarts and support probes, use the same
/// process-local settings and host-lifetime flags.
fn serve_config(
    bin: &str,
    flags: HostFlags,
    extra_args: &[&str],
) -> (LaunchConfig, Option<crate::host_config::HostConfig>) {
    let mut config = LaunchConfig::new(bin);
    let env_args: Vec<String> = std::env::var("MUSE_SERVE_ARGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    // A setting may appear once: `muse serve` refuses a repeated key
    // ("one setting, one occurrence") and exits 2, so a caller's own
    // `MUSE_SERVE_ARGS` value wins over a pinned default instead of colliding
    // with it.
    let extra = without_duplicate_config_args(&env_args, extra_args);
    config.args(env_args);
    config.args(extra);
    config.host_flags(flags);
    let host_config = crate::host_config::configure(&mut config);
    (config, host_config)
}

/// Drop each `-c KEY=…` / `--config=KEY=…` pair from `later` whose key
/// `earlier` already sets, so concatenating two argument lists never repeats a
/// setting.
fn without_duplicate_config_args<'a>(earlier: &[String], later: &[&'a str]) -> Vec<&'a str> {
    let keys = config_keys(earlier.iter().map(String::as_str));
    let mut kept = Vec::new();
    let mut index = 0;
    while index < later.len() {
        if let Some(key) = config_arg(later, index).map(|(_, key)| key)
            && keys.contains(&key)
        {
            index += config_arg_width(later[index]);
            continue;
        }
        let width = config_arg_width(later[index]);
        kept.extend(later[index..index + width].iter().copied());
        index += width;
    }
    kept
}

/// The settings an argument list sets, in either spelling.
fn config_keys<'a>(args: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let args: Vec<&str> = args.collect();
    let mut keys = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if let Some(key) = config_arg(&args, index).map(|(_, key)| key) {
            keys.push(key);
        }
        index += config_arg_width(args[index]);
    }
    keys
}

/// How many arguments the config spelling at `arg` occupies: two for the
/// separate `-c KEY=…` form, one for the combined `--config=KEY=…` form that
/// Muse also accepts.
fn config_arg_width(arg: &str) -> usize {
    if is_config_flag(arg) { 2 } else { 1 }
}

/// The setting named by the config argument at `index`, and the index of the
/// argument carrying its `KEY=VALUE`.
fn config_arg<'a>(args: &[&'a str], index: usize) -> Option<(usize, &'a str)> {
    let arg = *args.get(index)?;
    if is_config_flag(arg) {
        return config_key(args.get(index + 1)?).map(|key| (index + 1, key));
    }
    let (flag, value) = arg.split_once('=')?;
    if flag != "-c" && flag != "--config" {
        return None;
    }
    config_key(value).map(|key| (index, key))
}

fn is_config_flag(arg: &str) -> bool {
    arg == "-c" || arg == "--config"
}

/// The setting name in a `KEY=VALUE` argument.
fn config_key(value: &str) -> Option<&str> {
    value
        .split_once('=')
        .map(|(key, _)| key.trim())
        .filter(|key| !key.is_empty())
}

/// Run a bounded, stdin-EOF-only serve probe for `--support`. The probe does
/// not send protocol frames; it records the observed process status and
/// bounded stderr evidence for a human diagnostic.
pub fn probe_serve_exit(
    timeout: std::time::Duration,
) -> Result<Option<ExitClassification>, String> {
    let bin = muse_cli();
    let (config, _host_config) = serve_config(&bin, HostFlags::default(), &[]);
    muse_msp_client::probe_exit(&config, timeout).map_err(|error| match error {
        muse_msp_client::LaunchError::Spawn { error, .. } => describe_spawn_error(&bin, &error),
        other => other.to_string(),
    })
}

/// Server-initiated request methods this adapter deliberately handles.
/// Everything else must receive `methodNotFound`, never a synthetic `{}`.
const HANDLED_SERVER_REQUESTS: [&str; 2] = ["approval/request", "userInput/request"];

pub fn mk_err(code: i64, message: &str) -> J {
    J::Obj(vec![
        ("code".to_string(), J::Num(code.to_string())),
        ("message".to_string(), J::Str(message.to_string())),
    ])
}

/// Extract `" session=<id>"` for timeout diagnostics when params carry one.
fn session_suffix(params_json: &str) -> String {
    parse_json(params_json)
        .ok()
        .and_then(|p| {
            p.get("sessionId")
                .and_then(|v| v.as_str())
                .map(|s| format!(" session={s}"))
        })
        .unwrap_or_default()
}

pub fn err_message(e: &J) -> String {
    e.get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error")
        .to_string()
}

/// A `session/resume` whose `config.mcpServers` differs from the MCP set of
/// the session already loaded on this host. MSP fixes that set when the
/// session runtime is built and has no unload command.
pub fn is_session_configuration_conflict(e: &J) -> bool {
    rejection_reason(e) == Some("session_configuration_conflict")
}

/// The `reason` of a `commandRejected` error, if the error is one.
pub fn rejection_reason(e: &J) -> Option<&str> {
    let data = e.get("data")?;
    if data.get("kind")?.as_str()? != "commandRejected" {
        return None;
    }
    data.get("reason")?.as_str()
}

/// Older Muse hosts do not know the 1.3.0 session-default method. Keep the
/// existing per-turn reasoning path usable on those hosts.
pub fn is_method_not_found(e: &J) -> bool {
    matches!(e.get("code"), Some(J::Num(code)) if code == "-32601")
        || e.get("data")
            .and_then(|data| data.get("kind"))
            .and_then(|kind| kind.as_str())
            == Some("methodNotFound")
        || err_message(e)
            .to_ascii_lowercase()
            .contains("method not found")
}

pub fn err_code(e: &J) -> i64 {
    e.get("code")
        .and_then(|v| match v {
            J::Num(n) => n.parse::<i64>().ok(),
            _ => None,
        })
        .unwrap_or(-32603)
}

/// Muse 1.3's typed error for a durable output reference that cannot be read.
pub fn is_output_unavailable(error: &J) -> bool {
    err_code(error) == -32041
        || error
            .get("data")
            .and_then(|data| data.get("kind"))
            .and_then(|kind| kind.as_str())
            == Some("outputUnavailable")
}

/// Keep the host's typed availability facts intact when forwarding the error
/// through ACP. Older hosts may omit the `kind`, so add it only in that case.
pub fn output_unavailable_data(error: &J) -> J {
    let mut fields = match error.get("data") {
        Some(J::Obj(values)) => values.clone(),
        _ => Vec::new(),
    };
    if !fields.iter().any(|(key, _)| key == "kind") {
        fields.push(("kind".to_string(), J::Str("outputUnavailable".to_string())));
    }
    J::Obj(fields)
}

#[cfg(test)]
mod tests {
    use super::{
        ExitKind, ExitMessage, HANDLED_SERVER_REQUESTS, command_timeout,
        is_session_configuration_conflict, rejection_reason, session_profile_hint, session_suffix,
        without_duplicate_config_args,
    };
    use crate::json::parse_json;
    use muse_msp_client::classify_exit_parts;
    use std::time::Duration;

    #[test]
    fn a_setting_already_in_the_environment_arguments_is_not_repeated() {
        // muse serve refuses a repeated setting ("one setting, one occurrence")
        // and exits 2, so a caller's pin must replace a pinned default.
        let env =
            |raw: &str| -> Vec<String> { raw.split_whitespace().map(str::to_string).collect() };
        let reviewer = ["-c", "reasoning_effort=\"low\""];
        assert_eq!(
            without_duplicate_config_args(&env("-c reasoning_effort=\"high\""), &reviewer),
            Vec::<&str>::new()
        );
        assert_eq!(
            without_duplicate_config_args(&env("--trust-workspace"), &reviewer),
            reviewer.to_vec()
        );
        // A different key is not a duplicate, and other arguments survive.
        assert_eq!(
            without_duplicate_config_args(&env("--config model=\"x\""), &reviewer),
            reviewer.to_vec()
        );
        let mixed = ["--disable-write", "-c", "reasoning_effort=\"low\""];
        assert_eq!(
            without_duplicate_config_args(&env("-c reasoning_effort=\"high\""), &mixed),
            vec!["--disable-write"]
        );
        // The long spelling of the flag counts too.
        assert_eq!(
            without_duplicate_config_args(&env("--config reasoning_effort=\"high\""), &reviewer),
            Vec::<&str>::new()
        );
        // Muse also accepts the combined spelling, so it must be recognized
        // in both lists: otherwise the pin survives and the host exits 2.
        for combined in [
            "--config=reasoning_effort=\"high\"",
            "-c=reasoning_effort=\"high\"",
        ] {
            assert_eq!(
                without_duplicate_config_args(&env(combined), &reviewer),
                Vec::<&str>::new(),
                "{combined} sets the same key"
            );
        }
        assert_eq!(
            without_duplicate_config_args(
                &env("-c reasoning_effort=\"high\""),
                &["--config=reasoning_effort=\"low\""]
            ),
            Vec::<&str>::new(),
            "the combined spelling in the later list is dropped too"
        );
        assert_eq!(
            without_duplicate_config_args(
                &env("--config=model=\"x\""),
                &["--config=reasoning_effort=\"low\""]
            ),
            vec!["--config=reasoning_effort=\"low\""],
            "a different key is not a duplicate"
        );
        // A flag-looking value is not mistaken for the flag itself.
        assert_eq!(
            without_duplicate_config_args(
                &env("-c reasoning_effort=\"high\""),
                &["--trust-workspace"]
            ),
            vec!["--trust-workspace"]
        );
    }

    #[test]
    fn serve_exit_codes_are_classified_without_consulting_stderr() {
        let clean = classify_exit_parts(Some(0), None, "exit code 5 in a log line".into());
        assert_eq!(clean.kind, ExitKind::CleanShutdown);
        assert!(!clean.retryable());

        let config = classify_exit_parts(Some(3), None, "sdk surface unavailable".into());
        assert_eq!(config.kind, ExitKind::ConfigError);
        assert!(!config.retryable());
        assert!(
            config
                .editor_message()
                .contains("Fix the Muse configuration")
        );

        let sdk = classify_exit_parts(Some(5), None, "configuration is invalid".into());
        assert_eq!(sdk.kind, ExitKind::SdkSurfaceUnavailable);
        assert!(!sdk.retryable());
        assert!(
            sdk.editor_message()
                .contains("changing serve arguments will not help")
        );

        let signal = classify_exit_parts(None, Some(9), "clean shutdown".into());
        assert_eq!(signal.kind, ExitKind::Crash);
        assert!(signal.retryable());
    }

    #[test]
    fn support_lines_include_exit_code_and_stderr_tail() {
        let exit = classify_exit_parts(Some(5), None, "serve gate is off".into());
        let lines = exit.support_lines("support");
        assert!(lines[0].contains("exit-code=5"));
        assert!(lines.iter().any(|line| line.contains("serve gate is off")));
    }

    #[test]
    fn rejection_reason_reads_only_command_rejections() {
        let error =
            |data: &str| parse_json(&format!(r#"{{"code":-32030,"data":{data}}}"#)).unwrap();
        assert_eq!(
            rejection_reason(&error(
                r#"{"kind":"commandRejected","reason":"compaction_unavailable"}"#
            )),
            Some("compaction_unavailable")
        );
        assert_eq!(
            rejection_reason(&error(
                r#"{"kind":"internal","reason":"compaction_unavailable"}"#
            )),
            None
        );
        assert_eq!(
            rejection_reason(&error(r#"{"kind":"commandRejected"}"#)),
            None
        );
        assert_eq!(
            rejection_reason(&parse_json(r#"{"code":-32603}"#).unwrap()),
            None
        );
        assert!(is_session_configuration_conflict(&error(
            r#"{"kind":"commandRejected","reason":"session_configuration_conflict"}"#
        )));
    }

    #[test]
    fn profile_refusal_names_the_settings_key_and_profile() {
        let host = "internal error: compose session permission profile: permission profile ':auto-review' cannot be used: the automated reviewer is unavailable on this host";
        let hint = session_profile_hint(host).expect("profile refusal must hint");
        assert!(hint.contains("permissions.default_profile"), "{hint}");
        assert!(hint.contains(":auto-review"), "{hint}");
        assert!(hint.contains("muse serve"), "{hint}");
    }

    #[test]
    fn profile_refusal_without_a_quoted_name_still_hints() {
        let hint = session_profile_hint("cannot compose permission profile")
            .expect("unquoted refusal must hint");
        assert!(hint.contains("permissions.default_profile"), "{hint}");
    }

    #[test]
    fn unrelated_session_errors_get_no_hint() {
        for msg in [
            "session not found",
            "approval mode exceeds or is incomparable",
            "",
        ] {
            assert!(session_profile_hint(msg).is_none(), "{msg:?} must not hint");
        }
    }

    #[test]
    fn a_lost_record_refusal_does_not_blame_the_settings() {
        // Muse 1.4.13, captured verbatim from a live host whose durable
        // record held no committed permission authority. The refusal merely
        // composes a permission profile while restoring, so a settings hint
        // would misdirect: `permissions.default_profile` is not the problem
        // and the profile was never consulted.
        let host = "retained session refused (class c): compose session permission profile: \
                    retained durable session has no committed permission authority; start a new \
                    session [/state/sessions/2026/10/27/01a17cba/session.jsonl]; remedy: retry \
                    with a compatible host that can read the retained permission frame, or start \
                    a new session";
        assert!(session_profile_hint(host).is_none(), "{host}");
        // Either half of the marker is enough on its own.
        assert!(session_profile_hint("retained session refused (class c)").is_none());
        assert!(
            session_profile_hint("the retained durable session has no permission profile")
                .is_none()
        );
    }

    #[test]
    fn only_deliberately_handled_server_requests_are_forwarded() {
        // The client answers every other server request with methodNotFound.
        assert_eq!(
            HANDLED_SERVER_REQUESTS,
            ["approval/request", "userInput/request"]
        );
        for method in ["future/request", "approval/decide", "userInput/answer", ""] {
            assert!(
                !HANDLED_SERVER_REQUESTS.contains(&method),
                "{method:?} must get methodNotFound, not a synthetic result"
            );
        }
    }

    #[test]
    fn command_timeouts_are_method_aware() {
        let t = |m: &str| command_timeout(None, m);
        assert_eq!(t("initialize"), Duration::from_millis(30_000));
        assert_eq!(t("session/resume"), Duration::from_millis(180_000));
        assert_eq!(t("view/page"), Duration::from_millis(180_000));
        assert_eq!(t("view/subscribe"), Duration::from_millis(30_000));
        assert_eq!(t("model/list"), Duration::from_millis(30_000));
        assert_eq!(t("approval/decide"), Duration::from_millis(30_000));
        assert_eq!(t("item/readOutput"), Duration::from_millis(30_000));
        assert_eq!(t("task/stop"), Duration::from_millis(30_000));
        assert_eq!(t("task/stopAll"), Duration::from_millis(30_000));
        assert_eq!(t("turn/start"), Duration::from_millis(60_000));
        assert_eq!(t("future/method"), Duration::from_millis(60_000));
    }

    #[test]
    fn command_timeout_environment_override_wins() {
        assert_eq!(
            command_timeout(Some("250"), "session/resume"),
            Duration::from_millis(250)
        );
        // Invalid or non-positive overrides must not silently disable the
        // timeout: fall back to the method table.
        assert_eq!(
            command_timeout(Some("bogus"), "initialize"),
            Duration::from_millis(30_000)
        );
        assert_eq!(
            command_timeout(Some("0"), "initialize"),
            Duration::from_millis(30_000)
        );
        assert_eq!(
            command_timeout(Some(" -5 "), "initialize"),
            Duration::from_millis(30_000)
        );
    }

    #[test]
    fn timeout_diagnostics_carry_the_session_when_present() {
        assert_eq!(
            session_suffix(r#"{"commandId":"c","sessionId":"s-1"}"#),
            " session=s-1"
        );
        assert_eq!(session_suffix(r#"{"commandId":"c"}"#), "");
        assert_eq!(session_suffix("not json"), "");
    }

    #[test]
    fn output_unavailable_preserves_negative_code_and_data() {
        let error = crate::json::parse_json(
            r#"{"code":-32041,"message":"missing","data":{"kind":"outputUnavailable","availability":"missing","itemId":"item-1","outputRef":"out-1"}}"#,
        )
        .expect("error JSON");
        assert_eq!(super::err_code(&error), -32041);
        assert!(super::is_output_unavailable(&error));
        assert_eq!(
            super::output_unavailable_data(&error)
                .get("outputRef")
                .and_then(|value| value.as_str()),
            Some("out-1")
        );
    }

    #[test]
    fn capability_required_keeps_the_host_refusal_shape() {
        let error = super::capability_required_error("session/rename", "sessionRename");
        assert_eq!(super::err_code(&error), -32010);
        assert_eq!(super::acp_error_code(&error, -32603), -32010);
        assert_eq!(
            error
                .get("data")
                .and_then(|data| data.get("capability"))
                .and_then(|capability| capability.as_str()),
            Some("sessionRename")
        );
        assert_eq!(
            super::typed_error_data(&error).as_deref(),
            Some(r#"{"capability":"sessionRename","kind":"capabilityRequired","retryable":false}"#)
        );
        assert_eq!(
            super::err_message(&error),
            "session/rename requires the sessionRename capability"
        );
    }

    #[test]
    fn command_rejected_keeps_the_host_refusal_shape() {
        let error = crate::json::parse_json(
            r#"{"code":-32030,"message":"computerUseSetup/check rejected: runtime_busy","data":{"kind":"commandRejected","reason":"runtime_busy","retryable":false}}"#,
        )
        .expect("error JSON");
        assert_eq!(super::acp_error_code(&error, -32603), -32030);
        assert_eq!(
            super::typed_error_data(&error).as_deref(),
            Some(r#"{"kind":"commandRejected","reason":"runtime_busy","retryable":false}"#)
        );
    }
}

#[cfg(test)]
mod durability_tests {
    use super::HandshakeInfo;

    fn info(durability: Option<&str>) -> HandshakeInfo {
        HandshakeInfo {
            server_name: "s".into(),
            server_version: "0".into(),
            schema_version: Some(1),
            fingerprint: "fp".into(),
            status: "tested",
            detail: "",
            durability: durability.map(str::to_string),
            session_list_stream: false,
            user_shell: false,
            session_mcp: false,
            feedback: false,
            session_rename: false,
            computer_use_setup: false,
            computer_use_host_setup: false,
            sandbox_availability: None,
            sandbox_unavailable_reason: None,
        }
    }

    #[test]
    fn only_durable_profiles_may_restart() {
        // Absent means durable per the schema's compatibility rule.
        assert!(info(None).restartable());
        assert!(info(Some("durable")).restartable());
        // Unknown values carry no recovery guarantee: fail closed.
        assert!(!info(Some("ephemeral")).restartable());
        assert!(!info(Some("future-profile")).restartable());
    }

    fn version(server_version: &str, durability: Option<&str>) -> HandshakeInfo {
        HandshakeInfo {
            server_version: server_version.into(),
            ..info(durability)
        }
    }

    #[test]
    fn session_rename_gate_ignores_pre_registry_hosts() {
        // A host that predates the registry entry ignores the unknown
        // requested name and withholds the grant, yet still serves
        // `session/rename`: refusing there would regress `/rename`.
        for older in ["1.3.0", "1.4.3", "1.4.6", "1.4.7", "0.0.0-fixture"] {
            assert!(
                version(older, None).allows_session_rename(),
                "{older} must keep serving session/rename without the grant"
            );
        }
        // From 1.4.8 on the name is registered, so a withheld grant is a
        // refusal and a granted one is required.
        assert!(!version("1.4.8", None).allows_session_rename());
        assert!(!version("1.5.0", None).allows_session_rename());
        let mut granted = version("1.4.8", None);
        granted.session_rename = true;
        assert!(granted.allows_session_rename());
    }

    #[test]
    fn release_comparison_reads_the_leading_version_only() {
        let at_least_141 = |v: &str| version(v, None).at_least(1, 4, 1);
        assert!(at_least_141("1.4.2"));
        assert!(at_least_141("1.4.1"));
        assert!(!at_least_141("1.3.0"));
        // Numeric, not lexical: 1.10 is newer than 1.4.
        assert!(at_least_141("1.10.0"));
        assert!(at_least_141("2.0.0"));
        // The build label is a suffix, not part of the release.
        assert!(at_least_141("1.4.2-R4684.1"));
        assert!(at_least_141("1.4.1-R4503.1"));
        assert!(!at_least_141("1.3.0-R3401.1"));
        assert!(version("1.4.2-R4684.1", None).at_least(1, 4, 2));
        assert!(!version("1.4.2-R4684.1", None).at_least(1, 4, 3));
        // Anything that does not start with three numbers fails closed.
        for unknown in [
            "",
            "garbage",
            "unknown",
            "0.0.0-fixture",
            "1.4",
            "v1.4.2",
            "+1.4.2",
            "1.+4.2",
            "1..4.2",
            "1.4.-2",
            "99999999999999999999.0.0",
        ] {
            assert!(!at_least_141(unknown), "{unknown:?} must not pass the gate");
        }
    }

    #[test]
    fn session_features_follow_the_release_that_added_them() {
        let v130 = version("1.3.0", None);
        assert!(!v130.supports_session_delete());
        assert!(!v130.supports_workspace_roots());
        assert!(!v130.reports_session_cost());

        let v141 = version("1.4.1", Some("durable"));
        assert!(v141.supports_session_delete());
        assert!(v141.supports_workspace_roots());
        assert!(!v141.reports_session_cost());

        let v142 = version("1.4.2-R4684.1", None);
        assert!(v142.supports_session_delete());
        assert!(v142.supports_workspace_roots());
        assert!(v142.reports_session_cost());

        // 1.4.3 no longer serves `session/delete`, even when durable.
        let v143 = version("1.4.3-R5018.1", Some("durable"));
        assert!(!v143.supports_session_delete());
        assert!(v143.supports_workspace_roots());
        assert!(v143.reports_session_cost());
        assert!(!version("1.5.0", None).supports_session_delete());

        // 1.4.4 keeps `session/delete` removed; workspace roots and the
        // host-computed session cost stay on.
        let v144 = version("1.4.4-R5419.1", Some("durable"));
        assert!(!v144.supports_session_delete());
        assert!(v144.supports_workspace_roots());
        assert!(v144.reports_session_cost());

        // A memory-only host has no `session/delete`; an unknown profile
        // carries no durability guarantee either.
        for profile in ["ephemeral", "future-profile"] {
            let host = version("1.4.2", Some(profile));
            assert!(!host.supports_session_delete(), "{profile}");
            assert!(host.supports_workspace_roots(), "{profile}");
            assert!(host.reports_session_cost(), "{profile}");
        }

        assert_eq!(
            v142.features_line(),
            "host-features server=s/1.4.2-R4684.1 session_delete=true workspace_roots=true session_cost=true"
        );
    }

    fn sandbox(availability: Option<&str>, reason: Option<&str>) -> HandshakeInfo {
        HandshakeInfo {
            sandbox_availability: availability.map(str::to_string),
            sandbox_unavailable_reason: reason.map(str::to_string),
            ..version("1.4.7-R6101.1", None)
        }
    }

    #[test]
    fn sandbox_facts_are_never_fabricated() {
        // (availability, reason) as sent -> (posture, reported reason, warns)
        for (availability, reason, posture, reported, warns) in [
            (Some("available"), None, "available", None, false),
            (
                Some("unavailable"),
                Some("userNamespacesDenied"),
                "unavailable",
                Some("userNamespacesDenied"),
                true,
            ),
            (
                Some("unavailable"),
                Some("bubblewrapUnavailable"),
                "unavailable",
                Some("bubblewrapUnavailable"),
                true,
            ),
            // No reason sent: none is inferred.
            (Some("unavailable"), None, "unavailable", None, true),
            // An unknown reason stays unknown rather than borrowing a known one.
            (
                Some("unavailable"),
                Some("seccompDenied"),
                "unavailable",
                Some("unknown"),
                true,
            ),
            // A host before 1.4.7 sends nothing: unknown, never a reason.
            (None, None, "unknown", None, false),
            // An unknown posture, or a stray reason beside a non-`unavailable`
            // posture, never surfaces a reason.
            (
                Some("partial"),
                Some("userNamespacesDenied"),
                "unknown",
                None,
                false,
            ),
            (
                Some("available"),
                Some("userNamespacesDenied"),
                "available",
                None,
                false,
            ),
        ] {
            let host = sandbox(availability, reason);
            let case = format!("{availability:?}/{reason:?}");
            assert_eq!(host.sandbox_posture(), posture, "{case}");
            assert_eq!(host.sandbox_reason(), reported, "{case}");
            assert_eq!(host.sandbox_warning().is_some(), warns, "{case}");
            assert_eq!(
                host.sandbox_line(),
                format!(
                    "host-sandbox server=s/1.4.7-R6101.1 availability={posture} reason={}",
                    reported.unwrap_or("none")
                ),
                "{case}"
            );
        }
        assert_eq!(
            sandbox(Some("unavailable"), Some("userNamespacesDenied"))
                .sandbox_warning()
                .unwrap(),
            "warning: this Muse host reports it cannot enforce its sandbox (reason: userNamespacesDenied); tool runs are not isolated by it"
        );
        assert!(
            !sandbox(Some("unavailable"), None)
                .sandbox_warning()
                .unwrap()
                .contains("reason"),
            "a warning without a host reason must not name one"
        );
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::describe_spawn_error;

    #[test]
    fn spawn_errors_name_the_next_user_action() {
        let not_found = std::io::Error::from(std::io::ErrorKind::NotFound);
        let msg = describe_spawn_error("/opt/muse", &not_found);
        assert!(msg.contains("Muse CLI not found"), "{msg}");
        assert!(msg.contains("'/opt/muse'"), "{msg}");
        assert!(msg.contains("MUSE_CLI="), "{msg}");
        assert!(!msg.contains("  "), "stray source indentation: {msg}");

        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let msg = describe_spawn_error("/opt/muse", &denied);
        assert!(msg.contains("not executable"), "{msg}");
        assert!(!msg.contains("  "), "stray source indentation: {msg}");
    }
}

#[cfg(test)]
mod authentication_tests {
    use super::*;

    #[test]
    fn only_a_positive_logged_out_account_state_blocks_sessions() {
        let state = |raw: &str| parse_json(raw).unwrap();
        assert!(account_logged_out(&state(
            r#"{"state":"loggedOut","credentialRequired":true}"#
        )));
        assert!(account_logged_out(&state(r#"{"state":"loggedOut"}"#)));
        for raw in [
            r#"{"state":"loggedOut","credentialRequired":false}"#,
            r#"{"state":"accountLogin","credentialRequired":true}"#,
            r#"{"state":"envKey"}"#,
            r#"{"state":"apiKey"}"#,
            r#"{"state":"somethingNew"}"#,
            r#"{}"#,
        ] {
            assert!(!account_logged_out(&state(raw)), "{raw}");
        }
    }

    #[test]
    fn windows_launcher_search_takes_the_first_path_hit() {
        let root = std::env::temp_dir().join(format!("muse-acp-launcher-{}", std::process::id()));
        let (first, second) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("muse.exe"), "").unwrap();
        std::fs::write(first.join("muse.cmd"), "").unwrap();
        // A relative entry resolves against the working directory (the crate
        // root under cargo test) and must be skipped even when it matches.
        let relative = std::path::PathBuf::from(format!(
            "target/muse-acp-launcher-rel-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&relative).unwrap();
        std::fs::write(relative.join("muse.exe"), "").unwrap();
        let dirs = || {
            vec![
                std::path::PathBuf::new(),
                relative.clone(),
                root.join("missing"),
                first.clone(),
                second.clone(),
            ]
            .into_iter()
        };
        assert_eq!(
            find_windows_launcher(dirs(), "muse").as_deref(),
            first.join("muse.cmd").to_str()
        );
        assert_eq!(find_windows_launcher(dirs(), "absent"), None);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&relative);
    }

    #[test]
    fn default_install_dir_follows_the_muse_installer() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            default_install_dir(os("/opt/muse-bin"), os("/home/u")),
            Some(std::path::PathBuf::from("/opt/muse-bin"))
        );
        let expected = if cfg!(windows) {
            std::path::Path::new("/home/u")
                .join("Programs")
                .join("muse")
        } else {
            std::path::Path::new("/home/u").join(".local").join("bin")
        };
        assert_eq!(default_install_dir(os(""), os("/home/u")), Some(expected));
        assert_eq!(default_install_dir(None, os("")), None);
        assert_eq!(default_install_dir(None, None), None);
    }

    #[test]
    fn unrelated_failures_do_not_request_muse_login() {
        for message in [
            "HTTP 401 Unauthorized",
            "HTTP 403 Forbidden",
            "permission denied",
            "session not found",
            "connection timed out",
            "unsupported schema version",
            "compose session permission profile: reviewer unavailable",
        ] {
            assert_eq!(auth_failure(message), None, "{message}");
            assert_eq!(acp_error_code(&mk_err(-32603, message), -32602), -32602);
        }
    }

    #[test]
    fn external_turn_auth_failures_keep_their_service_diagnostic() {
        let host = HandshakeInfo::default();
        let message = "turn failed (kind 'environmentError'): AWS credentials have expired";

        assert_eq!(
            turn_auth_diagnostic(message, "environmentError", &host),
            None
        );
        assert!(turn_auth_diagnostic(message, "modelError", &host).is_some());
        assert!(
            turn_auth_diagnostic("Muse session has expired", "environmentError", &host).is_some()
        );
    }

    #[test]
    fn stable_auth_required_turn_kind_always_gets_muse_login_guidance() {
        let host = HandshakeInfo::default();
        let detail = turn_auth_diagnostic(
            "provider rejected the session token: secret-sentinel",
            "authRequired",
            &host,
        )
        .expect("authRequired is an authoritative Muse auth failure");
        assert!(detail.contains("Muse is not authenticated"), "{detail}");
        assert!(detail.contains("muse login"), "{detail}");
        assert!(detail.contains("restart"), "{detail}");
        assert!(!detail.contains("secret-sentinel"), "{detail}");
    }
}
