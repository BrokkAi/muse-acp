//! muse-acp: minimal ACP (Agent Client Protocol v1) adapter bridging stdio
//! JSON-RPC to the local `muse-code` CLI.
//!
//! Transport: JSON-RPC 2.0 over stdio, NDJSON (one object per `\n` line).
//! stdin: client -> agent, stdout: agent -> client, stderr: logs only.
//!
//! Mapping:
//! - `initialize` -> advertise text-only prompt caps (no loadSession/auth).
//! - `session/new {cwd}` -> allocate sessionId, store cwd.
//! - `session/prompt {sessionId, prompt}` -> concat text blocks, spawn
//!   `muse exec <extra-args> <prompt>` with cwd, stream stdout chunks as
//!   `session/update agent_message_chunk`, reply `{stopReason}`.
//! - `session/cancel {sessionId}` (notification) -> flag + kill child.
//!
//! Env:
//! - `MUSE_CLI` (default `muse`): binary to spawn.
//! - `MUSE_CLI_EXTRA_ARGS` (optional, space-separated): extra args inserted
//!   before the prompt, e.g. `--model foo`. Default: none.
//!
//! No third-party deps (std only) so `cargo build` works offline.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

// ---------------------------------------------------------------------------
// Minimal JSON value + parser (std only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn get(&self, key: &str) -> Option<&J> {
        if let J::Obj(pairs) = self {
            for (k, v) in pairs {
                if k == key {
                    return Some(v);
                }
            }
        }
        None
    }
    fn as_str(&self) -> Option<&str> {
        if let J::Str(s) = self { Some(s) } else { None }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self { b: s.as_bytes(), pos: 0 }
    }
    fn skip_ws(&mut self) {
        while self.pos < self.b.len() && matches!(self.b[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }
    fn parse_value(&mut self) -> Result<J, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.parse_lit("null", J::Null),
            Some(b't') => self.parse_lit("true", J::Bool(true)),
            Some(b'f') => self.parse_lit("false", J::Bool(false)),
            Some(b'"') => Ok(J::Str(self.parse_string()?)),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(format!("unexpected char '{}' at {}", c as char, self.pos)),
            None => Err("unexpected end of input".to_string()),
        }
    }
    fn parse_lit(&mut self, lit: &str, v: J) -> Result<J, String> {
        if self.b.len() >= self.pos + lit.len() && &self.b[self.pos..self.pos + lit.len()] == lit.as_bytes() {
            self.pos += lit.len();
            Ok(v)
        } else {
            Err(format!("invalid literal at {}", self.pos))
        }
    }
    fn parse_number(&mut self) -> Result<J, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self.pos < self.b.len()
            && (self.b[self.pos].is_ascii_digit()
                || matches!(self.b[self.pos], b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("invalid number at {}", start));
        }
        Ok(J::Num(String::from_utf8_lossy(&self.b[start..self.pos]).into_owned()))
    }
    fn hex4(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.b.len() {
            return Err("truncated \\u escape".to_string());
        }
        let s = std::str::from_utf8(&self.b[self.pos..self.pos + 4])
            .map_err(|_| "\\u not utf8".to_string())?;
        let v = u32::from_str_radix(s, 16).map_err(|_| "bad \\u hex".to_string())?;
        self.pos += 4;
        Ok(v)
    }
    fn parse_string(&mut self) -> Result<String, String> {
        // assumes current char is '"'
        self.pos += 1; // open quote
        let mut out = String::new();
        loop {
            if self.pos >= self.b.len() {
                return Err("unterminated string".to_string());
            }
            let c = self.b[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.b.len() {
                        return Err("truncated escape".to_string());
                    }
                    let e = self.b[self.pos];
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            match char::from_u32(cp) {
                                Some(ch) => out.push(ch),
                                None => out.push('\u{FFFD}'),
                            }
                        }
                        _ => return Err(format!("bad escape \\{}", e as char)),
                    }
                }
                _ => {
                    // copy one UTF-8 codepoint
                    let rest = &self.b[self.pos..];
                    let s = std::str::from_utf8(rest).map_err(|_| "invalid utf8 in string".to_string())?;
                    let ch = s.chars().next().ok_or("empty string tail")?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }
    fn parse_array(&mut self) -> Result<J, String> {
        self.pos += 1; // [
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(J::Arr(items));
            }
            let v = self.parse_value()?;
            items.push(v);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(J::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at {}", self.pos)),
            }
        }
    }
    fn parse_object(&mut self) -> Result<J, String> {
        self.pos += 1; // {
        let mut pairs = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(J::Obj(pairs));
            }
            if self.peek() != Some(b'"') {
                return Err(format!("expected string key at {}", self.pos));
            }
            let k = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(format!("expected ':' at {}", self.pos));
            }
            self.pos += 1;
            let v = self.parse_value()?;
            pairs.push((k, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(J::Obj(pairs));
                }
                _ => return Err(format!("expected ',' or '}}' at {}", self.pos)),
            }
        }
    }
}

fn parse_json(s: &str) -> Result<J, String> {
    let mut p = Parser::new(s);
    let v = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.b.len() {
        return Err("trailing characters".to_string());
    }
    Ok(v)
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{0008}' => o.push_str("\\b"),
            '\u{000C}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                o.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn j_to_string(j: &J) -> String {
    match j {
        J::Null => "null".to_string(),
        J::Bool(true) => "true".to_string(),
        J::Bool(false) => "false".to_string(),
        J::Num(n) => n.clone(),
        J::Str(s) => esc(s),
        J::Arr(items) => {
            let parts: Vec<String> = items.iter().map(j_to_string).collect();
            format!("[{}]", parts.join(","))
        }
        J::Obj(pairs) => {
            let parts: Vec<String> =
                pairs.iter().map(|(k, v)| format!("{}:{}", esc(k), j_to_string(v))).collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

// ---------------------------------------------------------------------------
// Sessions + running children
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Session {
    cwd: String,
}

struct Running {
    cancelled: AtomicBool,
    child: Mutex<Option<Child>>,
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;
type RunningMap = Arc<Mutex<HashMap<String, Arc<Running>>>>;
type StdoutShared = Arc<Mutex<std::io::Stdout>>;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn gen_session_id() -> String {
    // Prefer /dev/urandom UUIDv4 (exactly 16 bytes); fallback to time+pid+counter.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut b = [0u8; 16];
        if Read::read_exact(&mut f, &mut b).is_ok() {
            b[6] = (b[6] & 0x0f) | 0x40;
            b[8] = (b[8] & 0x3f) | 0x80;
            return format!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
            );
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("sess-{}-{}-{}", std::process::id(), now, n)
}

fn log(msg: &str) {
    eprintln!("[muse-acp] {msg}");
}

// ---------------------------------------------------------------------------
// JSON-RPC send helpers (stdout only; stderr is logs)
// ---------------------------------------------------------------------------

fn send_raw(stdout: &StdoutShared, line: String) {
    let mut out = stdout.lock().unwrap();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn id_json(id: &Option<J>) -> String {
    match id {
        None => "null".to_string(),
        Some(j) => j_to_string(j),
    }
}

fn send_result(stdout: &StdoutShared, id: &Option<J>, result_json: &str) {
    if id.is_none() {
        return; // notification: no response
    }
    send_raw(
        stdout,
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{}}}",
            id_json(id),
            result_json
        ),
    );
}

fn send_error(stdout: &StdoutShared, id: &Option<J>, code: i32, message: &str) {
    send_raw(
        stdout,
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":{}}}}}",
            id_json(id),
            esc(message)
        ),
    );
}

fn send_update_chunk(stdout: &StdoutShared, session_id: &str, text: &str) {
    send_raw(
        stdout,
        format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}}}}}",
            esc(session_id),
            esc(text)
        ),
    );
}

// ---------------------------------------------------------------------------
// Prompt text extraction: concat {type:"text", text} blocks
// ---------------------------------------------------------------------------

fn extract_prompt_text(params: Option<&J>) -> String {
    let Some(p) = params else { return String::new() };
    let prompt = p.get("prompt").unwrap_or(p);
    match prompt {
        J::Str(s) => s.clone(),
        J::Arr(blocks) => {
            let mut parts = Vec::new();
            for b in blocks {
                match b {
                    J::Str(s) => parts.push(s.clone()),
                    J::Obj(_) => {
                        let t = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if t == "text" {
                            if let Some(txt) = b.get("text").and_then(|v| v.as_str()) {
                                parts.push(txt.to_string());
                            }
                        }
                        // image/audio/resource/embedded parts are unsupported (text-only caps)
                    }
                    _ => {}
                }
            }
            parts.join("\n")
        }
        J::Obj(_) => {
            // params itself was the object and prompt lives under "prompt"
            if let Some(arr) = p.get("prompt") {
                return extract_prompt_text(Some(&J::Arr(match arr {
                    J::Arr(v) => v.clone(),
                    J::Str(s) => vec![J::Str(s.clone())],
                    _ => vec![],
                })));
            }
            String::new()
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Prompt worker: spawn muse CLI, stream stdout, reply stopReason
// ---------------------------------------------------------------------------

fn muse_bin() -> String {
    std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string())
}

fn extra_args() -> Vec<String> {
    match std::env::var("MUSE_CLI_EXTRA_ARGS") {
        Ok(s) => s.split_whitespace().map(|x| x.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

fn run_prompt(
    stdout: StdoutShared,
    sessions: Sessions,
    running: RunningMap,
    id: Option<J>,
    session_id: String,
    prompt_text: String,
) {
    let cwd = {
        let map = sessions.lock().unwrap();
        match map.get(&session_id) {
            Some(s) => s.cwd.clone(),
            None => {
                send_error(&stdout, &id, -32602, "unknown sessionId");
                return;
            }
        }
    };

    let handle = Arc::new(Running {
        cancelled: AtomicBool::new(false),
        child: Mutex::new(None),
    });
    running.lock().unwrap().insert(session_id.clone(), handle.clone());

    let bin = muse_bin();
    let mut cmd = Command::new(&bin);
    cmd.arg("exec");
    for a in extra_args() {
        cmd.arg(a);
    }
    cmd.arg(&prompt_text);
    cmd.current_dir(&cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            running.lock().unwrap().remove(&session_id);
            send_error(&stdout, &id, -32603, &format!("failed to spawn '{bin}': {e}"));
            return;
        }
    };

    let mut child_stdout = child.stdout.take();
    let mut child_stderr = child.stderr.take();
    *handle.child.lock().unwrap() = Some(child);

    // Stream stdout chunks as agent_message_chunk notifications.
    if let Some(mut out) = child_stdout.take() {
        let mut buf = [0u8; 2048];
        loop {
            if handle.cancelled.load(Ordering::SeqCst) {
                break;
            }
            match out.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if !text.is_empty() && !handle.cancelled.load(Ordering::SeqCst) {
                        send_update_chunk(&stdout, &session_id, &text);
                    }
                }
                Err(_) => break,
            }
        }
    }

    // Drain stderr to our logs (ACP forbids agent stdout pollution).
    if let Some(mut err) = child_stderr.take() {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        if !s.trim().is_empty() {
            log(&format!("muse stderr ({session_id}): {}", s.trim()));
        }
    }

    let cancelled = handle.cancelled.load(Ordering::SeqCst);
    let status = handle
        .child
        .lock()
        .unwrap()
        .as_mut()
        .map(|c| c.wait())
        .transpose();

    running.lock().unwrap().remove(&session_id);

    if cancelled {
        send_result(&stdout, &id, r#"{"stopReason":"cancelled"}"#);
        return;
    }
    match status {
        Ok(Some(st)) if st.success() => {
            send_result(&stdout, &id, r#"{"stopReason":"end_turn"}"#);
        }
        Ok(Some(st)) => {
            send_error(
                &stdout,
                &id,
                -32603,
                &format!("muse exited with {}", st.code().unwrap_or(-1)),
            );
        }
        Ok(None) => {
            send_error(&stdout, &id, -32603, "muse process already reaped");
        }
        Err(e) => {
            send_error(&stdout, &id, -32603, &format!("muse wait failed: {e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Main stdio loop
// ---------------------------------------------------------------------------

fn main() {
    let stdout: StdoutShared = Arc::new(Mutex::new(std::io::stdout()));
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let running: RunningMap = Arc::new(Mutex::new(HashMap::new()));

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                log(&format!("stdin read error: {e}"));
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
                send_error(&stdout, &None, -32700, &format!("parse error: {e}"));
                continue;
            }
        };
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned();

        match method.as_str() {
            "" => {
                // No method: likely a client response to our request; ignore.
                continue;
            }
            "initialize" => {
                send_result(
                    &stdout,
                    &id,
                    r#"{"protocolVersion":1,"agentCapabilities":{"promptCapabilities":{"text":true,"image":false,"audio":false,"embeddedContext":true},"mcpCapabilities":false,"loadSession":false}}"#,
                );
            }
            "session/new" => {
                let cwd = params
                    .as_ref()
                    .and_then(|p| p.get("cwd"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if cwd.is_empty() {
                    send_error(&stdout, &id, -32602, "session/new requires params.cwd");
                    continue;
                }
                // mcpServers intentionally ignored (unsupported).
                let sid = gen_session_id();
                sessions.lock().unwrap().insert(sid.clone(), Session { cwd });
                send_result(&stdout, &id, &format!("{{\"sessionId\":{}}}", esc(&sid)));
            }
            "session/prompt" => {
                let sid = params
                    .as_ref()
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() {
                    send_error(&stdout, &id, -32602, "session/prompt requires params.sessionId");
                    continue;
                }
                let text = extract_prompt_text(params.as_ref());
                if text.trim().is_empty() {
                    send_error(&stdout, &id, -32602, "session/prompt requires text content");
                    continue;
                }
                let (so, ss, rr) = (stdout.clone(), sessions.clone(), running.clone());
                std::thread::spawn(move || run_prompt(so, ss, rr, id, sid, text));
            }
            "session/cancel" => {
                let sid = params
                    .as_ref()
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() {
                    continue; // notification: nothing to acknowledge
                }
                let target = running.lock().unwrap().get(&sid).cloned();
                if let Some(h) = target {
                    h.cancelled.store(true, Ordering::SeqCst);
                    if let Some(child) = h.child.lock().unwrap().as_mut() {
                        let _ = child.kill();
                    }
                }
                // Notification: no response per ACP.
            }
            "authenticate" | "session/load" | "session/set_mode" | "session/set_model" | "logout" => {
                send_error(&stdout, &id, -32601, "method not supported by this agent");
            }
            "shutdown" | "exit" => {
                if method == "shutdown" {
                    send_result(&stdout, &id, "null");
                }
                break;
            }
            _ => {
                // Only respond when an id is present (request vs notification).
                if id.is_some() {
                    send_error(&stdout, &id, -32601, "method not found");
                }
            }
        }
    }
}
