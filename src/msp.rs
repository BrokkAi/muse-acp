//! MSP client: spawns one `muse serve` host and drives it over NDJSON JSON-RPC.
//!
//! Model (per the Muse Code developer docs + the schema the host ships):
//! commands carry caller-minted `commandId`s; the ack is not the outcome —
//! `item/*` and `turn/*` notifications report what happened. Server-initiated
//! `approval/request` gets an immediate `{}` ("handling it"); the verdict
//! travels separately via `approval/decide`.

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::{Duration, Instant};

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
        fn number(s: &str) -> Option<u64> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse().ok()
        }
        let mut parts = self.server_version.splitn(3, '.');
        let major = number(parts.next()?)?;
        let minor = number(parts.next()?)?;
        let rest = parts.next()?;
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let patch = number(&rest[..end])?;
        Some((major, minor, patch))
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

pub enum MspEvent {
    Notification {
        method: String,
        params: J,
    },
    /// Server-initiated request (e.g. approval/request, userInput/request).
    /// Already acked `{}` per protocol; the payload still needs handling.
    /// Only methods accepted by [`known_server_request`] reach the loop.
    Request {
        method: String,
        params: J,
    },
    Eof(String),
}

/// Default admission-ack budget in milliseconds for unclassified commands.
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// A pipe write is separate from the host's admission-ack budget. The host
/// may stop reading stdin while it is shutting down; waiting synchronously on
/// `ChildStdin` would otherwise strand the ACP loop before that budget starts.
const PIPE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Shutdown is best-effort, but must never turn an editor disconnect into an
/// unbounded wait for a child or a poisoned lock.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(7);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);

struct WriteRequest {
    line: String,
    result: Sender<std::io::Result<()>>,
}

fn write_loop(mut stdin: ChildStdin, requests: Receiver<WriteRequest>) {
    while let Ok(request) = requests.recv() {
        let result = (|| {
            writeln!(stdin, "{}", request.line)?;
            stdin.flush()
        })();
        let failed = result.as_ref().err().map(|e| (e.kind(), e.to_string()));
        let _ = request.result.send(result);
        let Some((kind, message)) = failed else {
            continue;
        };

        // Every queued writer must settle when the host closes its stdin. Do
        // not leave later sends waiting for the write timeout one by one.
        while let Ok(queued) = requests.try_recv() {
            let _ = queued
                .result
                .send(Err(std::io::Error::new(kind, message.clone())));
        }
        break;
    }
}

fn terminate_child(child: &mut Child) {
    // Windows batch launchers keep the actual host in a descendant process.
    // Kill the tree before the launcher so descendants cannot retain the pipe
    // and execute a command after its caller has received a timeout.
    #[cfg(windows)]
    if matches!(child.try_wait(), Ok(None))
        && let Ok(mut killer) = Command::new("taskkill.exe")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match killer.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(SHUTDOWN_POLL),
                Ok(None) => {
                    let _ = killer.kill();
                    let _ = killer.wait();
                    break;
                }
            }
        }
    }
    let _ = child.kill();
}

fn kill_and_reap(child: &mut Child) {
    terminate_child(child);
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(SHUTDOWN_POLL),
            Ok(None) => return,
        }
    }
}

/// Method-aware admission-ack budgets. Acks are admission-only, not outcomes:
/// a turn may legitimately run for minutes after `turn/start` accepts.
///
/// Retry policy: commands that carry a caller-minted `commandId` may be
/// retried after a timeout *with the same handle* (the host answers a
/// value-identical ack); query-shaped methods without one (`model/list`)
/// may be re-issued. Never mint a fresh `commandId` for a retry.
fn method_timeout_ms(method: &str) -> u64 {
    match method {
        // Handshake: bounded startup, but leave room for cold binary start.
        "initialize" => 30_000,
        // Lifecycle/history work can page and replay large views.
        "session/start" | "session/resume" | "session/read" | "view/page" => 180_000,
        // Cheap queries.
        "model/list" | "session/list" | "usage/read" | "view/subscribe" | "view/unsubscribe"
        | "item/readOutput" | "account/read" => 30_000,
        // Control-plane decisions should be fast but not flaky.
        "approval/decide" | "userInput/answer" | "userInput/cancel" | "userInput/clarify"
        | "task/background" | "task/stop" | "task/stopAll" => 30_000,
        _ => DEFAULT_TIMEOUT_MS,
    }
}

/// Resolve a command timeout from an optional environment override
/// (`MUSE_COMMAND_TIMEOUT_MS`, milliseconds) and the method table.
pub fn command_timeout(env_override: Option<&str>, method: &str) -> std::time::Duration {
    if let Some(raw) = env_override
        && let Ok(ms) = raw.trim().parse::<u64>()
        && ms > 0
    {
        return std::time::Duration::from_millis(ms);
    }
    std::time::Duration::from_millis(method_timeout_ms(method))
}

/// Guidance for a host that refuses to open a session over its permission
/// profile (seen live on 1.2.1: `compose session permission profile:
/// ... ':auto-review' ... reviewer is unavailable`). Profiles compose
/// host-side from Muse settings — nothing on the MSP wire selects one.
/// The launch configuration handles the built-in auto-review profile;
/// other profile refusals still need an actionable diagnostic. Returns
/// None for unrelated errors; the caller keeps the host text either way.
pub fn session_profile_hint(host_message: &str) -> Option<String> {
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

/// ACP reserves -32000 for authentication required. The MSP skill lookup
/// error is also stable across the two protocols, so preserve its registry
/// code; other MSP errors retain the caller's existing ACP mapping.
pub fn acp_error_code(error: &J, fallback: i64) -> i64 {
    if auth_failure(&err_message(error)).is_some() {
        -32000
    } else if err_code(error) == -32032
        || error
            .get("data")
            .and_then(|data| data.get("kind"))
            .and_then(|kind| kind.as_str())
            == Some("skillNotFound")
    {
        -32032
    } else {
        fallback
    }
}

const STDERR_MAX_BYTES: usize = 8 * 1024;
const STDERR_MAX_LINES: usize = 100;

/// The reason a `muse serve` child ended. The names and retry posture follow
/// MSP §2.11; stderr is evidence attached to the surrounding classification,
/// never an input to this mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    CleanShutdown,
    UnhandledError,
    UsageError,
    ConfigError,
    LeaseUnavailable,
    SdkSurfaceUnavailable,
    Crash,
}

/// A process exit observed after the child was successfully spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitClassification {
    pub kind: ExitKind,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr_tail: String,
}

impl ExitClassification {
    /// Whether an automatic durable-host relaunch can reasonably help.
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ExitKind::UnhandledError | ExitKind::Crash)
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            ExitKind::CleanShutdown => "cleanShutdown",
            ExitKind::UnhandledError => "unhandledError",
            ExitKind::UsageError => "usageError",
            ExitKind::ConfigError => "configError",
            ExitKind::LeaseUnavailable => "leaseUnavailable",
            ExitKind::SdkSurfaceUnavailable => "sdkSurfaceUnavailable",
            ExitKind::Crash => "crash",
        }
    }

    pub fn retry_advice(&self) -> &'static str {
        match self.kind {
            ExitKind::CleanShutdown => "none",
            ExitKind::UnhandledError | ExitKind::Crash => "retry",
            ExitKind::UsageError | ExitKind::SdkSurfaceUnavailable => "never",
            ExitKind::ConfigError => "fix-config",
            ExitKind::LeaseUnavailable => "after-lease-release",
        }
    }

    /// Message suitable for an editor-facing terminal or launch diagnostic.
    pub fn editor_message(&self) -> String {
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

    pub fn support_lines(&self, prefix: &str) -> Vec<String> {
        let exit = self
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string());
        let signal = self
            .signal
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string());
        let mut lines = vec![format!(
            "{prefix} kind={} exit-code={} signal={} retry={}",
            self.kind_name(),
            exit,
            signal,
            self.retry_advice()
        )];
        if self.stderr_tail.is_empty() {
            lines.push(format!("{prefix} stderr-tail=(empty)"));
        } else {
            for line in self.stderr_tail.lines() {
                lines.push(format!("{prefix} stderr-tail {line}"));
            }
        }
        lines
    }
}

impl fmt::Display for ExitClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.editor_message())
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

#[derive(Clone, Default)]
struct StderrTail {
    text: Arc<Mutex<String>>,
}

impl StderrTail {
    fn push(&self, bytes: &[u8]) {
        let mut text = self.text.lock().unwrap_or_else(|p| p.into_inner());
        text.push_str(&String::from_utf8_lossy(bytes));
        let mut lines = text.matches('\n').count();
        if !text.ends_with('\n') && !text.is_empty() {
            lines += 1;
        }
        while lines > STDERR_MAX_LINES {
            let Some(newline) = text.find('\n') else {
                break;
            };
            text.drain(..=newline);
            lines -= 1;
        }
        if text.len() > STDERR_MAX_BYTES {
            let mut start = text.len() - STDERR_MAX_BYTES;
            while start < text.len() && !text.is_char_boundary(start) {
                start += 1;
            }
            text.drain(..start);
        }
    }

    fn snapshot(&self) -> String {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

fn status_parts(status: &ExitStatus) -> (Option<i32>, Option<i32>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        (status.code(), status.signal())
    }
    #[cfg(not(unix))]
    {
        (status.code(), None)
    }
}

pub fn classify_exit_parts(
    exit_code: Option<i32>,
    signal: Option<i32>,
    stderr_tail: String,
) -> ExitClassification {
    let kind = match exit_code {
        Some(0) => ExitKind::CleanShutdown,
        Some(1) => ExitKind::UnhandledError,
        Some(2) => ExitKind::UsageError,
        Some(3) => ExitKind::ConfigError,
        Some(4) => ExitKind::LeaseUnavailable,
        Some(5) => ExitKind::SdkSurfaceUnavailable,
        _ => ExitKind::Crash,
    };
    ExitClassification {
        kind,
        exit_code,
        signal,
        stderr_tail,
    }
}

fn classify_status(status: &ExitStatus, stderr_tail: String) -> ExitClassification {
    let (exit_code, signal) = status_parts(status);
    classify_exit_parts(exit_code, signal, stderr_tail)
}

/// Structured data for the only MSP request error that ACP clients need to
/// branch on here. In particular, keep the rejected selector visible.
pub fn skill_error_data(error: &J) -> Option<String> {
    let data = error.get("data")?;
    (data.get("kind").and_then(|kind| kind.as_str()) == Some("skillNotFound"))
        .then(|| j_to_string(data))
}

pub struct MspHost {
    writer: Mutex<Option<Sender<WriteRequest>>>,
    next_id: AtomicU64,
    cmd_seq: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Result<J, J>>>>,
    handshake: Mutex<HandshakeInfo>,
    stderr: StderrTail,
    stderr_done: Arc<(Mutex<bool>, std::sync::Condvar)>,
    exit: Mutex<Option<ExitClassification>>,
    _child: Mutex<Child>,
    host_config: Mutex<Option<crate::host_config::HostConfig>>,
}

impl MspHost {
    /// Last-resort kill without waiting for a lock or child progress.
    pub fn force_stop(&self) {
        let until = Instant::now() + Duration::from_millis(200);
        loop {
            let mut child = match self._child.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < until => {
                    std::thread::sleep(SHUTDOWN_POLL);
                    continue;
                }
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            terminate_child(&mut child);
            // The deadline exits the whole adapter without running Drop.
            // Reap within our existing budget, then clean up the settings
            // view even if the main loop is blocked writing to the editor.
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < until => std::thread::sleep(SHUTDOWN_POLL),
                    Ok(None) | Err(_) => return,
                }
            }
            drop(child);
            let config = match self.host_config.try_lock() {
                Ok(mut config) => config.take(),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
                Err(std::sync::TryLockError::WouldBlock) => None,
            };
            drop(config);
            return;
        }
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
    /// window before the evidence is exposed.
    pub fn reap(&self) -> Option<ExitClassification> {
        let _deadline = crate::shutdown::deadline("child reap and stderr drain");
        if let Some(exit) = self.exit.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Some(exit);
        }
        // The stdout reader can observe EOF a few milliseconds before the
        // parent-visible wait status is published. Poll briefly so the
        // normal EOF path does not discard an otherwise available exit code.
        let deadline = Instant::now() + Duration::from_millis(250);
        let status = loop {
            let result = self
                ._child
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .try_wait();
            match result {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) | Err(_) => break None,
            }
        };
        let status = status?;
        self.host_config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        self.wait_for_stderr();
        let exit = classify_status(&status, self.stderr.snapshot());
        *self.exit.lock().unwrap_or_else(|p| p.into_inner()) = Some(exit.clone());
        Some(exit)
    }

    fn wait_for_stderr(&self) {
        let (done, cv) = &*self.stderr_done;
        let guard = done.lock().unwrap_or_else(|p| p.into_inner());
        let _ = cv.wait_timeout_while(guard, Duration::from_millis(250), |finished| !*finished);
    }

    /// Settle every command waiter as soon as the host reader loses stdout.
    /// Leaving the senders in `pending` would make callers wait for their
    /// individual admission timeout even though the connection is already
    /// known to be dead.
    fn fail_pending(&self, reason: &str) {
        // Reject new commands before draining waiters. Otherwise a command
        // can be queued between the drain and shutdown after stdout is gone.
        self.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
        let waiters = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain()
            .map(|(_, waiter)| waiter)
            .collect::<Vec<_>>();
        for waiter in waiters {
            let _ = waiter.send(Err(mk_err(-32603, reason)));
        }
    }

    /// Close the host input, give it a short chance to exit cleanly, then kill
    /// and reap it. Lock acquisition and child progress are both bounded so a
    /// broken host cannot strand adapter shutdown.
    pub fn shutdown(&self) {
        let _deadline_guard = crate::shutdown::deadline("host shutdown");
        self.fail_pending("serve shutting down");
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        let writer = loop {
            match self.writer.try_lock() {
                Ok(mut guard) => break Some(guard.take()),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    break Some(poisoned.into_inner().take());
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(SHUTDOWN_POLL);
                }
                Err(std::sync::TryLockError::WouldBlock) => break None,
            }
        };
        if writer.is_none() {
            log("serve shutdown timed out waiting for writer lock");
        }
        drop(writer);

        let kill_at = Instant::now() + SHUTDOWN_GRACE;
        let mut killed = false;
        loop {
            let mut child = match self._child.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(SHUTDOWN_POLL);
                    continue;
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    log("serve shutdown timed out waiting for child lock");
                    return;
                }
            };
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.host_config
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .take();
                    break;
                }
                Ok(None) if !killed && Instant::now() >= kill_at => {
                    terminate_child(&mut child);
                    killed = true;
                }
                Ok(None) if Instant::now() >= deadline => {
                    log("serve shutdown timed out waiting for child exit");
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    log(&format!("serve shutdown wait failed: {e}"));
                    break;
                }
            }
            // The deadline worker must be able to kill even while this path
            // gives the child its graceful-exit window.
            drop(child);
            std::thread::sleep(SHUTDOWN_POLL);
        }

        self.wait_for_stderr();
        let stderr_bytes = self.stderr.snapshot().len();
        if stderr_bytes > 0 {
            // Keep host diagnostics captured without echoing arbitrary host
            // stderr, which may contain credentials or workspace contents.
            log(&format!("serve stderr captured {stderr_bytes} bytes"));
        }
    }
}

impl MspHost {
    /// Launches `muse serve` with `extra_args` after `MUSE_SERVE_ARGS`.
    pub fn launch(
        user_input_dialogs: bool,
        user_shell: bool,
        extra_args: &[&str],
    ) -> Result<(Arc<MspHost>, Receiver<MspEvent>), LaunchError> {
        let bin = muse_cli();
        let (mut cmd, host_config) = serve_command(&bin, extra_args);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                let message = describe_spawn_error(&bin, &e);
                if e.kind() == std::io::ErrorKind::NotFound {
                    LaunchError::NotInstalled(message)
                } else {
                    LaunchError::Spawn(message)
                }
            })?;
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                kill_and_reap(&mut child);
                return Err(LaunchError::Startup("serve: no stdout".to_string()));
            }
        };
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                kill_and_reap(&mut child);
                return Err(LaunchError::Startup("serve: no stdin".to_string()));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                kill_and_reap(&mut child);
                return Err(LaunchError::Startup("serve: no stderr".to_string()));
            }
        };
        let (writer_tx, writer_rx) = mpsc::channel();
        std::thread::spawn(move || write_loop(stdin, writer_rx));
        let stderr_tail = StderrTail::default();
        let stderr_done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let reader_tail = stderr_tail.clone();
        let reader_done = stderr_done.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader_tail.push(&buf[..n]),
                }
            }
            let (done, cv) = &*reader_done;
            *done.lock().unwrap_or_else(|p| p.into_inner()) = true;
            cv.notify_all();
        });
        let host = Arc::new(MspHost {
            writer: Mutex::new(Some(writer_tx)),
            next_id: AtomicU64::new(1),
            cmd_seq: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            handshake: Mutex::new(HandshakeInfo::default()),
            stderr: stderr_tail,
            stderr_done,
            exit: Mutex::new(None),
            _child: Mutex::new(child),
            host_config: Mutex::new(host_config),
        });
        crate::shutdown::register_host(&host);
        let (tx, rx) = mpsc::channel();
        let reader_host = host.clone();
        std::thread::spawn(move || reader_loop(reader_host, stdout, tx));
        // Handshake.
        let shell_capability = if user_shell { ",\"userShell\"" } else { "" };
        // `experimentalApi` is the only way to reach `account/read`, which
        // lets session/new report a logged-out host before the first turn
        // (see `MspHost::logged_out`). On Muse 1.4.0 it gates nothing else.
        let init_params = format!(
            r#"{{"clientInfo":{{"name":"muse_acp","version":{ver}}},"capabilities":{{"experimentalApi":true,"userInputDialogs":{user_input_dialogs},"requestedCapabilities":["sessionListStream","sessionMcp","feedback"{shell_capability}]}}}}"#,
            ver = crate::json::esc(env!("CARGO_PKG_VERSION")),
            user_input_dialogs = user_input_dialogs
        );
        let res = match host.command("initialize", &init_params) {
            Ok(result) => result,
            Err(error) => {
                let message = format!("serve initialize failed: {}", err_message(&error));
                if let Some(exit) = host.reap() {
                    return Err(LaunchError::HostExit(exit));
                }
                host.shutdown();
                return Err(LaunchError::Startup(message));
            }
        };
        let schema = res.get("schema").cloned().unwrap_or(J::Null);
        let schema_version = schema.get("version").and_then(|v| v.as_u64());
        let fp = schema
            .get("fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let server = res.get("serverInfo").cloned().unwrap_or(J::Null);
        let server_name = server
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let server_version = server
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let verdict = compat::classify(schema_version, fp);
        let host_label = format!("{server_name}/{server_version}");
        log(&verdict.log_line(env!("CARGO_PKG_VERSION"), &host_label));
        let durability = res
            .get("sessionDurability")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let session_list_stream = matches!(
            res.get("grantedCapabilities"),
            Some(J::Arr(caps)) if caps.iter().any(|cap| cap.as_str() == Some("sessionListStream"))
        );
        *host.handshake.lock().unwrap_or_else(|p| p.into_inner()) = HandshakeInfo {
            server_name,
            server_version,
            schema_version,
            fingerprint: verdict.fingerprint.clone(),
            status: verdict.status.as_str(),
            detail: verdict.detail,
            durability,
            session_list_stream,
            user_shell: user_shell
                && matches!(res.get("grantedCapabilities"), Some(J::Arr(caps)) if caps.iter().any(|c| c.as_str() == Some("userShell"))),
            session_mcp: matches!(
                res.get("grantedCapabilities"),
                Some(J::Arr(caps)) if caps.iter().any(|c| c.as_str() == Some("sessionMcp"))
            ),
            feedback: matches!(
                res.get("grantedCapabilities"),
                Some(J::Arr(caps)) if caps.iter().any(|c| c.as_str() == Some("feedback"))
            ),
        };
        if verdict.is_fatal() {
            host.shutdown();
            return Err(LaunchError::Startup(format!(
                "incompatible host schema: version={} fingerprint={}; upgrade muse-acp",
                schema_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "absent".into()),
                verdict.fingerprint
            )));
        }
        // Close the handshake (SS1.4.2): no session/turn command is accepted
        // before this notification.
        if let Err(error) = host.notify("initialized", "{}") {
            if let Some(exit) = host.reap() {
                return Err(LaunchError::HostExit(exit));
            }
            host.shutdown();
            return Err(LaunchError::Startup(error));
        }
        Ok((host, rx))
    }

    /// UUIDv7 command ids: the host rejects anything else.
    pub fn mint_cmd(&self, _prefix: &str) -> String {
        use std::io::Read;
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = self.cmd_seq.fetch_add(1, Ordering::SeqCst);
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0) as u64
            & 0xffffffffffff;
        let mut r = [0u8; 10];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = Read::read_exact(&mut f, &mut r);
        }
        r[0] ^= (n & 0xff) as u8;
        r[9] ^= ((n >> 8) & 0xff) as u8;
        let b = u16::from_be_bytes([r[0], r[1]]) & 0x0fff;
        let c = u16::from_be_bytes([r[2], r[3]]) & 0x3fff | 0x8000;
        let d = ((r[4] as u64) << 40)
            | ((r[5] as u64) << 32)
            | ((r[6] as u64) << 24)
            | ((r[7] as u64) << 16)
            | ((r[8] as u64) << 8)
            | (r[9] as u64);
        format!(
            "{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}",
            (ms >> 16) as u32,
            (ms & 0xffff) as u16,
            b,
            c,
            d
        )
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
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let timeout = command_timeout(
            std::env::var("MUSE_COMMAND_TIMEOUT_MS").ok().as_deref(),
            method,
        );
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_string(), tx);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params_json}}}"
        );
        if let Err(e) = self.send_raw(&line) {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id.to_string());
            let fallback = format!("serve write failed: {e}");
            if e.kind() == std::io::ErrorKind::TimedOut {
                return Err(mk_err(-32603, &fallback));
            }
            return Err(mk_err(-32603, &self.host_closed_message(&fallback)));
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r.map_err(|e| {
                auth_diagnostic(&err_message(&e), &self.handshake())
                    .map(|message| mk_err(-32000, &message))
                    .unwrap_or(e)
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id.to_string());
                Err(mk_err(
                    -32603,
                    &format!(
                        "serve command timed out after {}ms method={method} id={id}{}",
                        timeout.as_millis(),
                        session_suffix(params_json)
                    ),
                ))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id.to_string());
                Err(mk_err(
                    -32603,
                    &self.host_closed_message("serve host closed the connection"),
                ))
            }
        }
    }

    fn host_closed_message(&self, fallback: &str) -> String {
        self.reap()
            .map(|exit| exit.editor_message())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Client-to-server notification (no id, no response).
    pub fn notify(&self, method: &str, params_json: &str) -> Result<(), String> {
        let line =
            format!("{{\"jsonrpc\":\"2.0\",\"method\":\"{method}\",\"params\":{params_json}}}");
        self.send_raw(&line)
            .map_err(|e| format!("serve notify failed: {e}"))
    }

    pub fn send_raw(&self, line: &str) -> std::io::Result<()> {
        let sender = self
            .writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "serve is shut down")
            })?;
        let (result_tx, result_rx) = mpsc::channel();
        sender
            .send(WriteRequest {
                line: line.to_string(),
                result: result_tx,
            })
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "serve stdin closed")
            })?;
        match result_rx.recv_timeout(PIPE_WRITE_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // A partially written frame cannot be withdrawn from a pipe.
                // Terminate this transport before reporting failure so its
                // writer cannot later deliver an untracked command.
                self.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
                kill_and_reap(&mut self._child.lock().unwrap_or_else(|p| p.into_inner()));
                self.fail_pending("serve stdin write timed out; host terminated");
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "serve stdin write timed out; host terminated",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "serve stdin writer stopped",
            )),
        }
    }

    /// Reply `{}` to a server-initiated request ("a client is handling this").
    pub fn reply_ok(&self, id: &J) {
        let _ = self.send_raw(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{}}}}",
            j_to_string(id)
        ));
    }

    /// Reply with the typed MSP `methodNotFound` error to a server request we
    /// cannot handle, matching the reference SDK client's failure shape.
    pub fn reply_method_not_found(&self, id: &J, method: &str) {
        let _ = self.send_raw(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":-32601,\"message\":{},\"data\":{{\"kind\":\"methodNotFound\",\"retryable\":false}}}}}}",
            j_to_string(id),
            crate::json::esc(&format!("method not found: {method}"))
        ));
    }
}

/// All host launches, including restarts and support probes, use the same
/// process-local settings and host-lifetime flags.
fn serve_command(
    bin: &str,
    extra_args: &[&str],
) -> (Command, Option<crate::host_config::HostConfig>) {
    let mut cmd = Command::new(bin);
    cmd.arg("serve");
    for arg in std::env::var("MUSE_SERVE_ARGS")
        .unwrap_or_default()
        .split_whitespace()
    {
        cmd.arg(arg);
    }
    cmd.args(extra_args);
    let config = crate::host_config::configure(&mut cmd);
    (cmd, config)
}

/// Run a bounded, stdin-EOF-only serve probe for `--support`. The probe does
/// not send protocol frames; it records the observed process status and
/// bounded stderr evidence for a human diagnostic.
pub fn probe_serve_exit(timeout: Duration) -> Result<Option<ExitClassification>, String> {
    let bin = muse_cli();
    let (mut cmd, _host_config) = serve_command(&bin, &[]);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| describe_spawn_error(&bin, &error))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "serve support probe: no stderr".to_string())?;
    let tail = StderrTail::default();
    let done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let reader_tail = tail.clone();
    let reader_done = done.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => reader_tail.push(&buf[..n]),
            }
        }
        let (finished, cv) = &*reader_done;
        *finished.lock().unwrap_or_else(|p| p.into_inner()) = true;
        cv.notify_all();
    });
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("serve support probe wait failed: {error}"))?
        {
            let (finished, cv) = &*done;
            let guard = finished.lock().unwrap_or_else(|p| p.into_inner());
            let _ = cv.wait_timeout_while(guard, Duration::from_millis(250), |complete| !*complete);
            return Ok(Some(classify_status(&status, tail.snapshot())));
        }
        if started.elapsed() >= timeout {
            terminate_child(&mut child);
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Server-initiated request methods this adapter deliberately handles.
/// Everything else must receive `methodNotFound`, never a synthetic `{}`.
pub fn known_server_request(method: &str) -> bool {
    matches!(method, "approval/request" | "userInput/request")
}

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

fn reader_loop(host: Arc<MspHost>, stdout: std::process::ChildStdout, tx: Sender<MspEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut frame = Vec::new();
    loop {
        frame.clear();
        match reader.read_until(b'\n', &mut frame) {
            Ok(0) => {
                host_closed(&host, &tx, "serve host stdout closed");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                host_closed(&host, &tx, &format!("serve stdout error: {e}"));
                break;
            }
        }
        let line = match std::str::from_utf8(&frame) {
            Ok(line) => line,
            Err(_) => {
                log("serve parse error: invalid UTF-8 frame");
                continue;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg = match parse_json(trimmed) {
            Ok(v) => v,
            Err(e) => {
                log(&format!("serve parse error: {e}"));
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !method.is_empty()
            && let Some(idv) = id.as_ref()
        {
            // Server-initiated request. Known methods are acked `{}` now and
            // forwarded — reissued multi-stage/resumed requests carry their
            // own choices. Unknown methods get the typed methodNotFound error:
            // a synthetic success result could corrupt host state.
            if known_server_request(&method) {
                host.reply_ok(idv);
                let params = msg.get("params").cloned().unwrap_or(J::Null);
                if tx.send(MspEvent::Request { method, params }).is_err() {
                    host.fail_pending("serve event loop closed");
                    host.shutdown();
                    break;
                }
                continue;
            }
            log(&format!(
                "unsupported MSP server request: {method} id={}",
                j_to_string(idv)
            ));
            host.reply_method_not_found(idv, &method);
            continue;
        }
        if let Some(idv) = id {
            let key = j_to_string(&idv);
            let waiter = host
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&key);
            if let Some(tx1) = waiter {
                if let Some(err) = msg.get("error") {
                    let _ = tx1.send(Err(err.clone()));
                } else {
                    let _ = tx1.send(Ok(msg.get("result").cloned().unwrap_or(J::Null)));
                }
            } else {
                log(&format!("serve response for unknown id {key}"));
            }
            continue;
        }
        if !method.is_empty() {
            trace_method("msp<-host", &method);
            let params = msg.get("params").cloned().unwrap_or(J::Null);
            if tx.send(MspEvent::Notification { method, params }).is_err() {
                host.fail_pending("serve event loop closed");
                host.shutdown();
                break;
            }
        }
    }
}

/// Wake commands that are waiting for a response when the host pipe closes.
/// Without this, a host that dies during initialization leaves its pending
/// receiver asleep until the full command timeout, delaying exit
/// classification and durable-host recovery decisions.
fn host_closed(host: &MspHost, tx: &Sender<MspEvent>, reason: &str) {
    let _deadline = crate::shutdown::deadline("host reader disconnect");
    host.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
    let _pending = host
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain()
        .collect::<Vec<_>>();
    let _ = tx.send(MspEvent::Eof(reason.to_string()));
}

#[cfg(test)]
mod tests {
    use super::{
        ExitKind, StderrTail, classify_exit_parts, command_timeout,
        is_session_configuration_conflict, known_server_request, rejection_reason,
        session_profile_hint, session_suffix,
    };
    use crate::json::parse_json;
    use std::time::Duration;

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
    fn stderr_tail_is_bounded_and_keeps_recent_evidence() {
        let tail = StderrTail::default();
        for n in 0..150 {
            tail.push(format!("diagnostic-{n:03}: {}\n", "x".repeat(100)).as_bytes());
        }
        let text = tail.snapshot();
        assert!(
            text.len() <= super::STDERR_MAX_BYTES,
            "{} bytes",
            text.len()
        );
        assert!(text.lines().count() <= super::STDERR_MAX_LINES);
        assert!(text.contains("diagnostic-149"));
        assert!(!text.contains("diagnostic-000"));
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
    fn only_deliberately_handled_server_requests_are_forwarded() {
        for method in ["approval/request", "userInput/request"] {
            assert!(known_server_request(method), "{method} must be known");
        }
        for method in ["future/request", "approval/decide", "userInput/answer", ""] {
            assert!(
                !known_server_request(method),
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
