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

use crate::json::{J, j_to_string, parse_json};

/// Fingerprint our client was built against (host 1.0.2). A mismatch warns;
/// the local schema, not the docs site, is authoritative for shapes.
pub const EXPECTED_FINGERPRINT: &str =
    "sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7";

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
    Request {
        method: String,
        params: J,
    },
    Eof(String),
}

/// How long a host command may take to acknowledge (acks are admission-only).
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub struct MspHost {
    writer: Arc<Mutex<std::process::ChildStdin>>,
    next_id: AtomicU64,
    cmd_seq: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Result<J, J>>>>,
    _child: Child,
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
            _child: child,
        });
        let (tx, rx) = mpsc::channel();
        let reader_host = host.clone();
        std::thread::spawn(move || reader_loop(reader_host, stdout, tx));
        // Handshake.
        let res = host
            .command(
                "initialize",
                r#"{"clientInfo":{"name":"muse_acp","version":"0.2.0"}}"#,
            )
            .map_err(|e| format!("serve initialize failed: {}", err_message(&e)))?;
        let fp = res
            .get("schema")
            .and_then(|s| s.get("fingerprint"))
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        if fp != EXPECTED_FINGERPRINT {
            log(&format!(
                "schema fingerprint mismatch: host reports {fp}, built against {EXPECTED_FINGERPRINT}; shapes may differ"
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
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id.to_string(), tx);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params_json}}}"
        );
        if let Err(e) = self.send_raw(&line) {
            self.pending.lock().unwrap().remove(&id.to_string());
            return Err(mk_err(-32603, &format!("serve write failed: {e}")));
        }
        match rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err(mk_err(-32603, "serve command timed out"))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
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
}

pub fn mk_err(code: i64, message: &str) -> J {
    J::Obj(vec![
        ("code".to_string(), J::Num(code.to_string())),
        ("message".to_string(), J::Str(message.to_string())),
    ])
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
            // Server-initiated request (e.g. approval/request): ack handling
            // now ("a client is handling this"), but still forward the payload
            // — reissued multi-stage/resumed requests carry their own choices.
            host.reply_ok(idv);
            let params = msg.get("params").cloned().unwrap_or(J::Null);
            if tx.send(MspEvent::Request { method, params }).is_err() {
                break;
            }
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
