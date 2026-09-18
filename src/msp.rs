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
use std::process::{Child, Command, ExitStatus, Stdio};
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
}

impl HandshakeInfo {
    /// Whether a dead host of this profile may be restarted and its sessions
    /// re-attached. Only `durable` (including the absent-means-durable arm)
    /// carries the recovery guarantee.
    pub fn restartable(&self) -> bool {
        matches!(self.durability.as_deref(), None | Some("durable"))
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
        "model/list" | "session/list" | "view/unsubscribe" => 30_000,
        // Control-plane decisions should be fast but not flaky.
        "approval/decide" | "userInput/answer" | "userInput/cancel" | "userInput/clarify" => 30_000,
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
/// host-side from the user's Muse settings — nothing on the MSP wire
/// selects one — so the fix is the settings key, never a retry. Returns
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
    Some(format!(
        " Hint: the host refused its own permission profile{profile}. That profile is composed from Muse settings (`permissions.default_profile`) and nothing on the wire overrides it — remove or change that setting; a profile whose reviewer is unavailable to `muse serve` refuses every session."
    ))
}

/// Turn a host spawn failure into the next user action. MSP exposes no
/// auth/health probe, so the readiness diagnosis starts at "can we even run
/// the CLI"; session errors carry host text from there.
pub fn describe_spawn_error(bin: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "Muse CLI not found: '{bin}'. Install Muse Code              (https://dev.meta.ai/docs/muse-code), ensure it is on PATH, or set              MUSE_CLI=/absolute/path/to/muse"
        ),
        std::io::ErrorKind::PermissionDenied => format!(
            "Muse CLI is not executable: '{bin}'. Fix permissions or set              MUSE_CLI=/absolute/path/to/muse"
        ),
        _ => format!("failed to spawn '{bin} serve': {e}"),
    }
}

/// MSP v1 has no auth method or stable auth error kind. Match explicit login
/// diagnostics only; a bare 401/403 or permission denial may belong to a tool.
/// Never echo raw authentication errors: they can contain credentials.
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

pub fn auth_diagnostic(message: &str, host: &HandshakeInfo) -> Option<String> {
    let failure = auth_failure(message)?;
    let bin = std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".into());
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
/// Generic auth text is attributable to Muse only for model failures; other
/// failure classes need to name Muse explicitly so service guidance survives.
pub fn turn_auth_diagnostic(
    message: &str,
    error_kind: &str,
    host: &HandshakeInfo,
) -> Option<String> {
    if error_kind != "modelError" && !message.to_ascii_lowercase().contains("muse") {
        return None;
    }
    auth_diagnostic(message, host)
}

/// ACP reserves -32000 for authentication required. Other MSP errors retain
/// the caller's existing ACP mapping; MSP numeric codes are not ACP codes.
pub fn acp_error_code(error: &J, fallback: i64) -> i64 {
    if auth_failure(&err_message(error)).is_some() {
        -32000
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
    Spawn(String),
    Startup(String),
    HostExit(ExitClassification),
}

impl LaunchError {
    pub fn retryable(&self) -> bool {
        match self {
            LaunchError::Spawn(_) => false,
            LaunchError::Startup(_) => true,
            LaunchError::HostExit(exit) => exit.retryable(),
        }
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::Spawn(message) | LaunchError::Startup(message) => f.write_str(message),
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

pub struct MspHost {
    writer: Arc<Mutex<std::process::ChildStdin>>,
    next_id: AtomicU64,
    cmd_seq: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Result<J, J>>>>,
    handshake: Mutex<HandshakeInfo>,
    stderr: StderrTail,
    stderr_done: Arc<(Mutex<bool>, std::sync::Condvar)>,
    exit: Mutex<Option<ExitClassification>>,
    _child: Mutex<Child>,
}

impl MspHost {
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
}

impl MspHost {
    pub fn launch() -> Result<(Arc<MspHost>, Receiver<MspEvent>), LaunchError> {
        let bin = std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string());
        let mut cmd = Command::new(&bin);
        cmd.arg("serve");
        // Host-lifetime posture from env (see `muse serve --help`).
        for a in std::env::var("MUSE_SERVE_ARGS")
            .unwrap_or_default()
            .split_whitespace()
        {
            cmd.arg(a);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LaunchError::Spawn(describe_spawn_error(&bin, &e)))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LaunchError::Startup("serve: no stdout".to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LaunchError::Startup("serve: no stdin".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| LaunchError::Startup("serve: no stderr".to_string()))?;
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
            writer: Arc::new(Mutex::new(stdin)),
            next_id: AtomicU64::new(1),
            cmd_seq: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            handshake: Mutex::new(HandshakeInfo::default()),
            stderr: stderr_tail,
            stderr_done,
            exit: Mutex::new(None),
            _child: Mutex::new(child),
        });
        let (tx, rx) = mpsc::channel();
        let reader_host = host.clone();
        std::thread::spawn(move || reader_loop(reader_host, stdout, tx));
        // Handshake.
        let init_params = format!(
            r#"{{"clientInfo":{{"name":"muse_acp","version":{ver}}}}}"#,
            ver = crate::json::esc(env!("CARGO_PKG_VERSION"))
        );
        let res = match host.command("initialize", &init_params) {
            Ok(result) => result,
            Err(error) => {
                let message = format!("serve initialize failed: {}", err_message(&error));
                if let Some(exit) = host.reap() {
                    return Err(LaunchError::HostExit(exit));
                }
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
        *host.handshake.lock().unwrap_or_else(|p| p.into_inner()) = HandshakeInfo {
            server_name,
            server_version,
            schema_version,
            fingerprint: verdict.fingerprint.clone(),
            status: verdict.status.as_str(),
            detail: verdict.detail,
            durability,
        };
        if verdict.is_fatal() {
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
        let mut w = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        writeln!(w, "{line}")?;
        w.flush()
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

/// Run a bounded, stdin-EOF-only serve probe for `--support`. The probe does
/// not send protocol frames or inspect stderr; it only records the observed
/// process status and bounded stderr evidence for a human diagnostic.
pub fn probe_serve_exit(timeout: Duration) -> Result<Option<ExitClassification>, String> {
    let bin = std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string());
    let mut cmd = Command::new(&bin);
    cmd.arg("serve");
    for arg in std::env::var("MUSE_SERVE_ARGS")
        .unwrap_or_default()
        .split_whitespace()
    {
        cmd.arg(arg);
    }
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
            let _ = child.kill();
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

pub fn err_code(e: &J) -> i64 {
    e.get("code")
        .and_then(|v| v.as_u64())
        .map(|n| n as i64)
        .unwrap_or(-32603)
}

fn reader_loop(host: Arc<MspHost>, stdout: std::process::ChildStdout, tx: Sender<MspEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
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
        ExitKind, StderrTail, classify_exit_parts, command_timeout, known_server_request,
        session_profile_hint, session_suffix,
    };
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
        assert_eq!(t("model/list"), Duration::from_millis(30_000));
        assert_eq!(t("approval/decide"), Duration::from_millis(30_000));
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

        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let msg = describe_spawn_error("/opt/muse", &denied);
        assert!(msg.contains("not executable"), "{msg}");
    }
}

#[cfg(test)]
mod authentication_tests {
    use super::*;

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
}
