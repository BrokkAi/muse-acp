//! MSP client: spawns one `muse serve` host and drives it over NDJSON JSON-RPC.
//!
//! Model (per the Muse Code developer docs + the schema the host ships):
//! commands carry caller-minted `commandId`s; the ack is not the outcome —
//! `item/*` and `turn/*` notifications report what happened. Server-initiated
//! `approval/request` gets an immediate `{}` ("handling it"); the verdict
//! travels separately via `approval/decide`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};

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

pub fn log(msg: &str) {
    eprintln!("[muse-acp] {msg}");
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

pub struct MspHost {
    writer: Arc<Mutex<std::process::ChildStdin>>,
    next_id: AtomicU64,
    cmd_seq: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Result<J, J>>>>,
    handshake: Mutex<HandshakeInfo>,
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

    /// Best-effort reap of an exited host child; EOF has already been
    /// observed, so this only prevents a zombie.
    pub fn reap(&self) {
        let _ = self
            ._child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_wait();
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
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to spawn '{bin} serve': {e}"))?;
        let stdout = child.stdout.take().ok_or("serve: no stdout")?;
        let stdin = child.stdin.take().ok_or("serve: no stdin")?;
        let host = Arc::new(MspHost {
            writer: Arc::new(Mutex::new(stdin)),
            next_id: AtomicU64::new(1),
            cmd_seq: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            handshake: Mutex::new(HandshakeInfo::default()),
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
        let res = host
            .command("initialize", &init_params)
            .map_err(|e| format!("serve initialize failed: {}", err_message(&e)))?;
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
            return Err(format!(
                "incompatible host schema: version={} fingerprint={}; upgrade muse-acp",
                schema_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "absent".into()),
                verdict.fingerprint
            ));
        }
        // Close the handshake (SS1.4.2): no session/turn command is accepted
        // before this notification.
        host.notify("initialized", "{}")?;
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
        self.pending.lock().unwrap().insert(id.to_string(), tx);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params_json}}}"
        );
        if let Err(e) = self.send_raw(&line) {
            self.pending.lock().unwrap().remove(&id.to_string());
            return Err(mk_err(-32603, &format!("serve write failed: {e}")));
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id.to_string());
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
                self.pending.lock().unwrap().remove(&id.to_string());
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
        let mut w = self.writer.lock().unwrap();
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
                let _ = tx.send(MspEvent::Eof("serve host stdout closed".to_string()));
                break;
            }
            Ok(_) => {}
            Err(e) => {
                let _ = tx.send(MspEvent::Eof(format!("serve stdout error: {e}")));
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
            let waiter = host.pending.lock().unwrap().remove(&key);
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
            let params = msg.get("params").cloned().unwrap_or(J::Null);
            if tx.send(MspEvent::Notification { method, params }).is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{command_timeout, known_server_request, session_suffix};
    use std::time::Duration;

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
