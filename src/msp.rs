//! MSP client: spawns one `muse serve` host and drives it over NDJSON JSON-RPC.
//!
//! Model (per the Muse Code developer docs + the schema the host ships):
//! commands carry caller-minted `commandId`s; the ack is not the outcome —
//! `item/*` and `turn/*` notifications report what happened. Server-initiated
//! `approval/request` gets an immediate `{}` ("handling it"); the verdict
//! travels separately via `approval/decide`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
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

/// A pipe write is separate from the host's admission-ack budget. The host
/// may stop reading stdin while it is shutting down; waiting synchronously on
/// `ChildStdin` would otherwise strand the ACP loop before that budget starts.
const PIPE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Shutdown is best-effort, but must never turn an editor disconnect into an
/// unbounded wait for a child or a poisoned lock.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(7);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);
const STDERR_CAPTURE_LIMIT: usize = 32 * 1024;

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

fn drain_stderr(stderr: ChildStderr, capture: Arc<Mutex<Vec<u8>>>) {
    let mut reader = BufReader::new(stderr);
    let mut buf = [0u8; 4096];
    loop {
        let count = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut stored = capture.lock().unwrap_or_else(|p| p.into_inner());
        stored.extend_from_slice(&buf[..count]);
        if stored.len() > STDERR_CAPTURE_LIMIT {
            let drop_count = stored.len() - STDERR_CAPTURE_LIMIT;
            stored.drain(..drop_count);
        }
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
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
        "model/list" | "session/list" | "view/unsubscribe" | "item/readOutput" => 30_000,
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

pub struct MspHost {
    writer: Mutex<Option<Sender<WriteRequest>>>,
    next_id: AtomicU64,
    cmd_seq: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Result<J, J>>>>,
    handshake: Mutex<HandshakeInfo>,
    stderr: Arc<Mutex<Vec<u8>>>,
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

    /// Settle every command waiter as soon as the host reader loses stdout.
    /// Leaving the senders in `pending` would make callers wait for their
    /// individual admission timeout even though the connection is already
    /// known to be dead.
    fn fail_pending(&self, reason: &str) {
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

    /// Stop and reap the host within a bounded budget. EOF has already been
    /// observed on the normal call site, but a host can close stdout while
    /// leaving a process behind, so a plain `try_wait()` is insufficient.
    pub fn reap(&self) {
        self.shutdown();
    }

    /// Close the host input, give it a short chance to exit cleanly, then kill
    /// and reap it. Lock acquisition and child progress are both bounded so a
    /// broken host cannot strand adapter shutdown.
    pub fn shutdown(&self) {
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
        let mut child = loop {
            match self._child.try_lock() {
                Ok(guard) => break Some(guard),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    break Some(poisoned.into_inner());
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(SHUTDOWN_POLL);
                }
                Err(std::sync::TryLockError::WouldBlock) => break None,
            }
        };
        let Some(mut child) = child.take() else {
            log("serve shutdown timed out waiting for child lock");
            return;
        };

        let mut killed = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if !killed && Instant::now() >= kill_at => {
                    let _ = child.kill();
                    killed = true;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(SHUTDOWN_POLL);
                }
                Ok(None) => {
                    log("serve shutdown timed out waiting for child exit");
                    break;
                }
                Err(e) => {
                    log(&format!("serve shutdown wait failed: {e}"));
                    break;
                }
            }
        }

        let stderr_bytes = self.stderr.lock().unwrap_or_else(|p| p.into_inner()).len();
        if stderr_bytes > 0 {
            // Keep host diagnostics captured without echoing arbitrary host
            // stderr, which may contain credentials or workspace contents.
            log(&format!("serve stderr captured {stderr_bytes} bytes"));
        }
    }
}

impl MspHost {
    pub fn launch() -> Result<(Arc<MspHost>, Receiver<MspEvent>), String> {
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
            .map_err(|e| describe_spawn_error(&bin, &e))?;
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                kill_and_reap(&mut child);
                return Err("serve: no stdout".to_string());
            }
        };
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                kill_and_reap(&mut child);
                return Err("serve: no stdin".to_string());
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                kill_and_reap(&mut child);
                return Err("serve: no stderr".to_string());
            }
        };
        let (writer_tx, writer_rx) = mpsc::channel();
        std::thread::spawn(move || write_loop(stdin, writer_rx));
        let stderr_capture = Arc::new(Mutex::new(Vec::new()));
        let stderr_for_reader = stderr_capture.clone();
        std::thread::spawn(move || drain_stderr(stderr, stderr_for_reader));
        let host = Arc::new(MspHost {
            writer: Mutex::new(Some(writer_tx)),
            next_id: AtomicU64::new(1),
            cmd_seq: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            handshake: Mutex::new(HandshakeInfo::default()),
            stderr: stderr_capture,
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
            Ok(res) => res,
            Err(e) => {
                let message = format!("serve initialize failed: {}", err_message(&e));
                host.shutdown();
                return Err(message);
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
            let message = format!(
                "incompatible host schema: version={} fingerprint={}; upgrade muse-acp",
                schema_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "absent".into()),
                verdict.fingerprint
            );
            host.shutdown();
            return Err(message);
        }
        // Close the handshake (SS1.4.2): no session/turn command is accepted
        // before this notification.
        if let Err(e) = host.notify("initialized", "{}") {
            host.shutdown();
            return Err(e);
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
            return Err(mk_err(-32603, &format!("serve write failed: {e}")));
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
                Err(mk_err(-32603, "serve host closed the connection"))
            }
        }
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
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                host.fail_pending("serve host stdout closed");
                host.shutdown();
                let _ = tx.send(MspEvent::Eof("serve host stdout closed".to_string()));
                break;
            }
            Ok(_) => {}
            Err(e) => {
                let reason = format!("serve stdout error: {e}");
                host.fail_pending(&reason);
                host.shutdown();
                let _ = tx.send(MspEvent::Eof(reason));
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

#[cfg(test)]
mod tests {
    use super::{command_timeout, known_server_request, session_profile_hint, session_suffix};
    use std::time::Duration;

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
