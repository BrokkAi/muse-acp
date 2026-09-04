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
//! Subcommands:
//! - (no args): run the ACP agent over stdio (what Zed spawns).
//! - `install [options]`: register `muse-acp` (resolved via PATH) as a custom
//!   ACP server in Zed's `settings.json`.
//! - `uninstall [options]`: remove the `settings.json` entry again.
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
// Zed installer: `muse-acp install` / `muse-acp uninstall`
//
// Registers this binary as a custom ACP agent server in Zed's settings.json:
//
//   { "agent_servers": { "muse-acp": {
//       "type": "custom", "command": "<abs path>", "args": [], "env": {} } } }
//
// Edits are surgical: only the touched entry changes, everything else in the
// file (comments, formatting, key order — Zed settings are JSONC) is
// preserved byte-for-byte.
// ---------------------------------------------------------------------------

const ZED_SETTINGS_REL: &str = ".config/zed/settings.json";
const DEFAULT_AGENT_NAME: &str = "muse-acp";
const DEFAULT_COMMAND: &str = "muse-acp";

struct InstallerOpts {
    name: String,
    settings: Option<String>,
    command: String,
    env: Vec<(String, String)>,
    dry_run: bool,
    no_backup: bool,
}

impl InstallerOpts {
    fn new() -> Self {
        Self {
            name: DEFAULT_AGENT_NAME.to_string(),
            settings: None,
            command: DEFAULT_COMMAND.to_string(),
            env: Vec::new(),
            dry_run: false,
            no_backup: false,
        }
    }
}

fn usage() -> &'static str {
    "usage: muse-acp [command] [options]\n\
     \n\
     \x20 (no command)         run the ACP agent over stdio (what Zed spawns)\n\
     \x20 install              register muse-acp as a Zed agent server\n\
     \x20 uninstall            remove the Zed settings entry again\n\
     \x20 help [command]       show this help (-h/--help also work)\n\
     \x20 --version (-V)       print version\n\
     \n\
     install options:\n\
     \x20 --name <name>        agent_servers key (default: muse-acp)\n\
     \x20 --settings <path>    settings.json path (default: ~/.config/zed/settings.json)\n\
     \x20 --command <cmd>      command Zed spawns, resolved via PATH (default: muse-acp)\n\
     \x20 --env KEY=VALUE      extra env for the agent entry (repeatable)\n\
     \x20 --dry-run            print planned changes without writing anything\n\
     \x20 --no-backup          do not write a .bak backup of settings.json\n\
     \n\
     uninstall options:\n\
     \x20 --name, --settings, --dry-run, --no-backup (as above)"
}

enum Cli {
    Serve,
    Install(InstallerOpts),
    Uninstall(InstallerOpts),
    Help,
    Version,
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    let arg = &args[*i];
    if let Some(v) = arg.strip_prefix(&format!("{flag}=")) {
        if v.is_empty() {
            return Err(format!("{flag} requires a value"));
        }
        return Ok(v.to_string());
    }
    *i += 1;
    if *i >= args.len() {
        return Err(format!("{flag} requires a value"));
    }
    Ok(args[*i].clone())
}

fn parse_installer_opts(args: &[String], i: &mut usize, is_install: bool) -> Result<InstallerOpts, String> {
    let cmd = if is_install { "install" } else { "uninstall" };
    let mut o = InstallerOpts::new();
    while *i < args.len() {
        let a = args[*i].clone();
        if a == "-h" || a == "--help" {
            return Err(format!("__help_{cmd}"));
        } else if a == "--name" || a.starts_with("--name=") {
            o.name = take_value(args, i, "--name")?;
            if o.name.trim().is_empty() {
                return Err("--name must not be empty".to_string());
            }
        } else if a == "--settings" || a.starts_with("--settings=") {
            o.settings = Some(take_value(args, i, "--settings")?);
        } else if a == "--command" || a.starts_with("--command=") {
            if !is_install {
                return Err("--command is only valid for install".to_string());
            }
            o.command = take_value(args, i, "--command")?;
            if o.command.trim().is_empty() {
                return Err("--command must not be empty".to_string());
            }
        } else if a == "--env" || a.starts_with("--env=") {
            if !is_install {
                return Err("--env is only valid for install".to_string());
            }
            let kv = take_value(args, i, "--env")?;
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| "--env requires KEY=VALUE".to_string())?;
            if k.is_empty() {
                return Err("--env requires a non-empty key".to_string());
            }
            o.env.push((k.to_string(), v.to_string()));
        } else if a == "--dry-run" {
            o.dry_run = true;
        } else if a == "--no-backup" {
            o.no_backup = true;
        } else {
            return Err(format!("unexpected argument: {a}"));
        }
        *i += 1;
    }
    Ok(o)
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    if args.is_empty() {
        return Ok(Cli::Serve);
    }
    match args[0].as_str() {
        "-h" | "--help" | "help" => Ok(Cli::Help),
        "-V" | "--version" => {
            if args.len() > 1 {
                return Err(format!("unexpected argument: {}", args[1]));
            }
            Ok(Cli::Version)
        }
        "install" | "uninstall" => {
            let is_install = args[0] == "install";
            let mut i = 1;
            match parse_installer_opts(args, &mut i, is_install) {
                Ok(o) => Ok(if is_install { Cli::Install(o) } else { Cli::Uninstall(o) }),
                Err(e) if e == "__help_install" || e == "__help_uninstall" => Ok(Cli::Help),
                Err(e) => Err(e),
            }
        }
        other => Err(format!("unexpected argument: {other}")),
    }
}

// --- JSONC scanner (Zed settings allow comments + trailing commas) ---

#[derive(Debug)]
struct JsoncEntry {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
    comma_after: Option<usize>,
}

#[derive(Debug)]
struct JsoncObject {
    #[allow(dead_code)]
    open: usize,
    close: usize,
    entries: Vec<JsoncEntry>,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_trivia(b: &[u8], mut p: usize) -> usize {
    while p < b.len() {
        if is_ws(b[p]) {
            p += 1;
        } else if b[p] == b'/' && p + 1 < b.len() && b[p + 1] == b'/' {
            p += 2;
            while p < b.len() && b[p] != b'\n' {
                p += 1;
            }
        } else if b[p] == b'/' && p + 1 < b.len() && b[p + 1] == b'*' {
            p += 2;
            while p + 1 < b.len() && !(b[p] == b'*' && b[p + 1] == b'/') {
                p += 1;
            }
            p = (p + 2).min(b.len());
        } else {
            break;
        }
    }
    p
}

fn is_trivia_only(s: &str) -> bool {
    skip_trivia(s.as_bytes(), 0) == s.len()
}

fn slice_is_ws_only(s: &str) -> bool {
    s.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
}

fn parse_jsonc_string(b: &[u8], p: usize) -> Option<(String, usize)> {
    if b.get(p) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = p + 1;
    while i < b.len() {
        match b[i] {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    return None;
                }
                match b[i] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        if i + 4 >= b.len() {
                            return None;
                        }
                        let hex = std::str::from_utf8(&b[i + 1..i + 5]).ok()?;
                        let cp = u32::from_str_radix(hex, 16).ok()?;
                        // BMP only; astral keys compare by replacement char (fine:
                        // we only compare ASCII keys like "agent_servers").
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        i += 4;
                    }
                    _ => return None,
                }
                i += 1;
            }
            _ => {
                let s = std::str::from_utf8(&b[i..]).ok()?;
                let c = s.chars().next()?;
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    None
}

fn scan_jsonc_value_end(b: &[u8], p: usize) -> Option<usize> {
    let c = *b.get(p)?;
    match c {
        b'"' => Some(parse_jsonc_string(b, p)?.1),
        b'{' | b'[' => {
            let close = if c == b'{' { b'}' } else { b']' };
            let mut depth = 0usize;
            let mut i = p;
            while i < b.len() {
                if b[i] == b'"' {
                    i = parse_jsonc_string(b, i)?.1;
                    continue;
                }
                if b[i] == b'/' && i + 1 < b.len() && (b[i + 1] == b'/' || b[i + 1] == b'*') {
                    i = skip_trivia(b, i);
                    continue;
                }
                if b[i] == c {
                    depth += 1;
                } else if b[i] == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                i += 1;
            }
            None
        }
        _ => {
            let mut i = p;
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']') {
                i += 1;
            }
            while i > p && is_ws(b[i - 1]) {
                i -= 1;
            }
            if i == p { None } else { Some(i) }
        }
    }
}

fn parse_jsonc_object(b: &[u8], open: usize) -> Option<JsoncObject> {
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut entries = Vec::new();
    let mut p = skip_trivia(b, open + 1);
    if b.get(p) == Some(&b'}') {
        return Some(JsoncObject { open, close: p, entries });
    }
    loop {
        let (key, key_end) = parse_jsonc_string(b, p)?;
        let key_start = p;
        p = skip_trivia(b, key_end);
        if b.get(p) != Some(&b':') {
            return None;
        }
        p = skip_trivia(b, p + 1);
        let value_start = p;
        let value_end = scan_jsonc_value_end(b, p)?;
        p = skip_trivia(b, value_end);
        let comma_after = if b.get(p) == Some(&b',') {
            p += 1;
            Some(p - 1)
        } else {
            None
        };
        entries.push(JsoncEntry { key, key_start, value_start, value_end, comma_after });
        p = skip_trivia(b, p);
        match b.get(p) {
            Some(&b'}') => return Some(JsoncObject { open, close: p, entries }),
            Some(&b'"') => {}
            _ => return None,
        }
    }
}

/// Leading whitespace of the line containing `pos` (for matching indent style).
fn line_indent(text: &str, pos: usize) -> String {
    let b = text.as_bytes();
    let mut s = pos.min(b.len());
    while s > 0 && b[s - 1] != b'\n' {
        s -= 1;
    }
    let mut indent = String::new();
    for &c in &b[s..pos.min(b.len())] {
        if c == b' ' || c == b'\t' {
            indent.push(c as char);
        } else {
            break;
        }
    }
    indent
}

fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            match parse_jsonc_string(b, i) {
                Some((_, end)) => {
                    out.push_str(&src[i..end]);
                    i = end;
                }
                None => {
                    out.push_str(&src[i..]);
                    break;
                }
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                if b[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            let c = src[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn strip_trailing_commas(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            match parse_jsonc_string(b, i) {
                Some((_, end)) => {
                    out.push_str(&src[i..end]);
                    i = end;
                }
                None => {
                    out.push_str(&src[i..]);
                    break;
                }
            }
        } else if b[i] == b',' {
            let mut j = i + 1;
            while j < b.len() && is_ws(b[j]) {
                j += 1;
            }
            if j < b.len() && (b[j] == b'}' || b[j] == b']') {
                i += 1; // drop trailing comma
            } else {
                out.push(',');
                i += 1;
            }
        } else {
            let c = src[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn check_valid_jsonc_object(text: &str) -> bool {
    match parse_json(strip_trailing_commas(&strip_comments(text)).trim()) {
        Ok(J::Obj(_)) => true,
        _ => false,
    }
}

fn render_agent_value(command: &str, env: &[(String, String)], fi: &str, ci: &str) -> String {
    let mut s = String::from("{\n");
    s.push_str(&format!("{fi}\"type\": \"custom\",\n"));
    s.push_str(&format!("{fi}\"command\": {},\n", esc(command)));
    s.push_str(&format!("{fi}\"args\": [],\n"));
    if env.is_empty() {
        s.push_str(&format!("{fi}\"env\": {{}}\n"));
    } else {
        s.push_str(&format!("{fi}\"env\": {{\n"));
        for (i, (k, v)) in env.iter().enumerate() {
            s.push_str(&format!("{fi}  {}: {}", esc(k), esc(v)));
            if i + 1 < env.len() {
                s.push(',');
            }
            s.push('\n');
        }
        s.push_str(&format!("{fi}}}\n"));
    }
    s.push_str(ci);
    s.push('}');
    s
}

fn render_agent_entry(name: &str, command: &str, env: &[(String, String)], ki: &str) -> String {
    let fi = format!("{ki}  ");
    format!("{}{}: {}", ki, esc(name), render_agent_value(command, env, &fi, ki))
}

#[derive(Debug, PartialEq)]
enum EditOutcome {
    Added,
    Updated,
}

fn install_settings_edit(
    original: &str,
    name: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(String, EditOutcome), String> {
    if is_trivia_only(original) {
        let entry = render_agent_entry(name, command, env, "    ");
        let fresh = format!("{{\n  \"agent_servers\": {{\n{entry}\n  }}\n}}\n");
        if !check_valid_jsonc_object(&fresh) {
            return Err("internal error: generated invalid settings".to_string());
        }
        return Ok((fresh, EditOutcome::Added));
    }
    if !check_valid_jsonc_object(original) {
        return Err("existing settings file is not valid JSON/JSONC; fix it manually or pass --settings <path>".to_string());
    }
    let b = original.as_bytes();
    let root = parse_jsonc_object(b, skip_trivia(b, 0))
        .ok_or_else(|| "settings file root is not a JSON object; refusing to edit".to_string())?;
    let aservers = root.entries.iter().find(|e| e.key == "agent_servers");
    let Some(aservers) = aservers else {
        // Insert a new top-level "agent_servers" key before the closing brace.
        let ind = root
            .entries
            .last()
            .map(|e| line_indent(original, e.key_start))
            .unwrap_or_else(|| "  ".to_string());
        let entry = render_agent_entry(name, command, env, &format!("{ind}  "));
        let block = format!("\n{ind}\"agent_servers\": {{\n{entry}\n{ind}}}");
        let (ins, prefix) = match root.entries.last() {
            None => (root.close, String::new()), // empty (maybe commented) object
            Some(last) => (
                last.comma_after.map(|c| c + 1).unwrap_or(last.value_end),
                if last.comma_after.is_some() { "" } else { "," }.to_string(),
            ),
        };
        let mut out = String::with_capacity(original.len() + block.len() + 2);
        out.push_str(&original[..ins]);
        out.push_str(&prefix);
        out.push_str(&block);
        if root.entries.is_empty() {
            out.push('\n');
        }
        out.push_str(&original[ins..]);
        if !check_valid_jsonc_object(&out) {
            return Err("internal error: produced invalid settings; file left untouched".to_string());
        }
        return Ok((out, EditOutcome::Added));
    };
    let sub = parse_jsonc_object(b, aservers.value_start)
        .ok_or_else(|| "\"agent_servers\" exists but is not an object; remove or fix it manually".to_string())?;
    let ai_ind = line_indent(original, aservers.key_start);
    if let Some(existing) = sub.entries.iter().find(|e| e.key == name) {
        let ki = line_indent(original, existing.key_start);
        let fi = format!("{ki}  ");
        let value = render_agent_value(command, env, &fi, &ki);
        let mut out = String::with_capacity(original.len() + value.len());
        out.push_str(&original[..existing.value_start]);
        out.push_str(&value);
        out.push_str(&original[existing.value_end..]);
        if !check_valid_jsonc_object(&out) {
            return Err("internal error: produced invalid settings; file left untouched".to_string());
        }
        return Ok((out, EditOutcome::Updated));
    }
    // Insert a new entry into the existing agent_servers object.
    let entry_ind = format!("{ai_ind}  ");
    let entry = render_agent_entry(name, command, env, &entry_ind);
    let mut out = String::with_capacity(original.len() + entry.len() + 8);
    if sub.entries.is_empty() {
        if slice_is_ws_only(&original[aservers.value_start + 1..sub.close]) {
            out.push_str(&original[..aservers.value_start]);
            out.push_str(&format!("{{\n{entry}\n{ai_ind}}}"));
            out.push_str(&original[sub.close + 1..]);
        } else {
            // Non-empty trivia (comments): keep it, append after, re-indent close.
            out.push_str(&original[..sub.close]);
            out.push_str(&format!("\n{entry}\n{ai_ind}}}"));
            out.push_str(&original[sub.close + 1..]);
        }
    } else {
        let last = sub.entries.last().unwrap();
        let ins = last.comma_after.map(|c| c + 1).unwrap_or(last.value_end);
        let prefix = if last.comma_after.is_some() { "" } else { "," };
        out.push_str(&original[..ins]);
        out.push_str(prefix);
        out.push_str(&format!("\n{entry}"));
        out.push_str(&original[ins..]);
    }
    if !check_valid_jsonc_object(&out) {
        return Err("internal error: produced invalid settings; file left untouched".to_string());
    }
    Ok((out, EditOutcome::Added))
}

/// Span covering an object entry plus one adjacent comma, so removal leaves
/// valid JSON.
fn entry_removal_span(obj: &JsoncObject, idx: usize) -> (usize, usize) {
    let e = &obj.entries[idx];
    if let Some(c) = e.comma_after {
        return (e.key_start, c + 1);
    }
    if idx > 0 {
        let prev = &obj.entries[idx - 1];
        return (prev.comma_after.unwrap_or(prev.value_end), e.value_end);
    }
    (e.key_start, e.value_end)
}

fn remove_entry_at(text: &str, obj: &JsoncObject, idx: usize) -> String {
    let (rs, re) = entry_removal_span(obj, idx);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..rs]);
    out.push_str(&text[re..]);
    out
}

fn uninstall_settings_edit(original: &str, name: &str) -> Result<(String, bool), String> {
    if is_trivia_only(original) {
        return Ok((original.to_string(), false));
    }
    if !check_valid_jsonc_object(original) {
        return Err("existing settings file is not valid JSON/JSONC; fix it manually or pass --settings <path>".to_string());
    }
    let b = original.as_bytes();
    let root = parse_jsonc_object(b, skip_trivia(b, 0))
        .ok_or_else(|| "settings file root is not a JSON object; refusing to edit".to_string())?;
    let Some(ai) = root.entries.iter().position(|e| e.key == "agent_servers") else {
        return Ok((original.to_string(), false));
    };
    let sub = parse_jsonc_object(b, root.entries[ai].value_start)
        .ok_or_else(|| "\"agent_servers\" exists but is not an object; remove or fix it manually".to_string())?;
    let Some(idx) = sub.entries.iter().position(|e| e.key == name) else {
        return Ok((original.to_string(), false));
    };
    let mut out = remove_entry_at(original, &sub, idx);
    // Drop agent_servers itself when it is left empty.
    let b2 = out.clone();
    let bb = b2.as_bytes();
    if let Some(root2) = parse_jsonc_object(bb, skip_trivia(bb, 0)) {
        if let Some(ai2) = root2.entries.iter().position(|e| e.key == "agent_servers") {
            let v = &root2.entries[ai2];
            if let Some(sub2) = parse_jsonc_object(bb, v.value_start) {
                if sub2.entries.is_empty() {
                    out = remove_entry_at(&out, &root2, ai2);
                }
            }
        }
    }
    // Normalize a fully-emptied file instead of leaving a whitespace shell.
    let compact: String = out.chars().filter(|c| !c.is_whitespace()).collect();
    if compact == "{}" {
        out = "{}\n".to_string();
    }
    if !check_valid_jsonc_object(&out) {
        return Err("internal error: produced invalid settings; file left untouched".to_string());
    }
    Ok((out, true))
}

// --- paths + binary install ---

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .map_err(|_| "cannot determine home directory (HOME/USERPROFILE unset); pass --settings <path>".to_string())
}

fn default_settings_path() -> Result<std::path::PathBuf, String> {
    Ok(home_dir()?.join(ZED_SETTINGS_REL))
}

fn write_backup(path: &std::path::Path) {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak_path = std::path::Path::new(&bak);
    match std::fs::copy(path, bak_path) {
        Ok(_) => println!("muse-acp: backup: {}", bak_path.display()),
        Err(e) => eprintln!("muse-acp: warning: cannot write backup {}: {e}", bak_path.display()),
    }
}

fn cmd_install(o: &InstallerOpts) -> i32 {
    let settings_path = match &o.settings {
        Some(s) => std::path::PathBuf::from(s),
        None => match default_settings_path() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("muse-acp: {e}");
                return 1;
            }
        },
    };
    // The registered command is resolved via PATH at spawn time; make sure
    // `muse-acp` is on PATH (e.g. `cargo install --path .`) before launching Zed.
    let command = o.command.clone();
    let original = match std::fs::read_to_string(&settings_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("muse-acp: cannot read {}: {e}", settings_path.display());
            return 1;
        }
    };
    let (updated, outcome) = match install_settings_edit(&original, &o.name, &command, &o.env) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("muse-acp: {e}");
            return 1;
        }
    };
    let action = if outcome == EditOutcome::Updated { "update" } else { "add" };
    if o.dry_run {
        println!("muse-acp: dry run — nothing written");
        println!("muse-acp: would {action} entry \"{}\" in {} (command: {command})", o.name, settings_path.display());
        return 0;
    }
    if !original.is_empty() && !o.no_backup {
        write_backup(&settings_path);
    }
    if let Some(parent) = settings_path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("muse-acp: cannot create {}: {e}", parent.display());
                return 1;
            }
        }
    }
    if let Err(e) = std::fs::write(&settings_path, &updated) {
        eprintln!("muse-acp: cannot write {}: {e}", settings_path.display());
        return 1;
    }
    println!(
        "muse-acp: {} entry \"{}\" in {} (command: {command})",
        if outcome == EditOutcome::Updated { "updated" } else { "added" },
        o.name,
        settings_path.display()
    );
    println!("muse-acp: restart Zed (or reload settings) and select \"{}\" in the Agent panel.", o.name);
    0
}

fn cmd_uninstall(o: &InstallerOpts) -> i32 {
    let settings_path = match &o.settings {
        Some(s) => std::path::PathBuf::from(s),
        None => match default_settings_path() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("muse-acp: {e}");
                return 1;
            }
        },
    };
    let original = match std::fs::read_to_string(&settings_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("muse-acp: nothing to do (no settings file at {})", settings_path.display());
            return 0;
        }
        Err(e) => {
            eprintln!("muse-acp: cannot read {}: {e}", settings_path.display());
            return 1;
        }
    };
    let (updated, removed) = match uninstall_settings_edit(&original, &o.name) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("muse-acp: {e}");
            return 1;
        }
    };
    if !removed {
        println!("muse-acp: nothing to do (no \"{}\" entry in {})", o.name, settings_path.display());
        return 0;
    }
    if o.dry_run {
        println!("muse-acp: dry run — nothing written");
        println!("muse-acp: would remove entry \"{}\" from {}", o.name, settings_path.display());
        return 0;
    }
    if !o.no_backup {
        write_backup(&settings_path);
    }
    if let Err(e) = std::fs::write(&settings_path, &updated) {
        eprintln!("muse-acp: cannot write {}: {e}", settings_path.display());
        return 1;
    }
    println!("muse-acp: removed entry \"{}\" from {}", o.name, settings_path.display());
    0
}

// ---------------------------------------------------------------------------
// Main stdio loop (default: ACP over stdio)
// ---------------------------------------------------------------------------

fn run_server() {
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

// ---------------------------------------------------------------------------
// Entry point: subcommand dispatch (default = ACP server over stdio)
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Cli::Serve) => run_server(),
        Ok(Cli::Install(o)) => std::process::exit(cmd_install(&o)),
        Ok(Cli::Uninstall(o)) => std::process::exit(cmd_uninstall(&o)),
        Ok(Cli::Help) => {
            println!("muse-acp {} — ACP adapter for the muse CLI", env!("CARGO_PKG_VERSION"));
            println!();
            println!("{}", usage());
        }
        Ok(Cli::Version) => println!("muse-acp {}", env!("CARGO_PKG_VERSION")),
        Err(e) => {
            eprintln!("muse-acp: {e}");
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env1() -> Vec<(String, String)> {
        vec![("FOO".to_string(), "bar".to_string())]
    }

    #[test]
    fn fresh_file_creates_agent_servers() {
        let (out, outcome) = install_settings_edit("", "muse-acp", "/bin/muse-acp", &[]).unwrap();
        assert_eq!(outcome, EditOutcome::Added);
        assert!(check_valid_jsonc_object(&out));
        assert!(out.contains("\"agent_servers\""));
        assert!(out.contains("\"command\": \"/bin/muse-acp\""));
        assert!(out.contains("\"type\": \"custom\""));
    }

    #[test]
    fn preserves_comments_and_other_keys() {
        let original = "{\n  // theme comment\n  \"theme\": \"One Dark\",\n  /* block\n     comment */\n  \"tab_size\": 4,\n}\n";
        let (out, _) = install_settings_edit(original, "muse-acp", "/bin/muse-acp", &env1()).unwrap();
        assert!(out.contains("// theme comment"));
        assert!(out.contains("/* block\n     comment */"));
        assert!(out.contains("\"theme\": \"One Dark\""));
        assert!(out.contains("\"FOO\": \"bar\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn install_is_idempotent() {
        let original = "{\n  \"theme\": \"x\",\n}\n";
        let (once, _) = install_settings_edit(original, "muse-acp", "/bin/muse-acp", &[]).unwrap();
        let (twice, outcome) = install_settings_edit(&once, "muse-acp", "/bin/muse-acp", &[]).unwrap();
        assert_eq!(outcome, EditOutcome::Updated);
        assert_eq!(once, twice);
    }

    #[test]
    fn replaces_existing_entry_keeps_siblings() {
        let original = "{\n  \"agent_servers\": {\n    \"other\": {\n      \"type\": \"custom\",\n      \"command\": \"other-bin\"\n    },\n    \"muse-acp\": {\n      \"type\": \"custom\",\n      \"command\": \"/old/path\"\n    }\n  }\n}\n";
        let (out, outcome) = install_settings_edit(original, "muse-acp", "/new/path", &[]).unwrap();
        assert_eq!(outcome, EditOutcome::Updated);
        assert!(out.contains("\"command\": \"/new/path\""));
        assert!(!out.contains("/old/path"));
        assert!(out.contains("\"other\""));
        assert!(out.contains("other-bin"));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn handles_trailing_commas() {
        let original = "{\n  \"agent_servers\": {\n    \"other\": {\"command\": \"x\",},\n  },\n  \"theme\": \"y\",\n}\n";
        let (out, _) = install_settings_edit(original, "muse-acp", "/bin/muse-acp", &[]).unwrap();
        assert!(out.contains("\"other\""));
        assert!(out.contains("\"muse-acp\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn rejects_non_object_root_and_agent_servers() {
        assert!(install_settings_edit("[1,2]", "muse-acp", "/b", &[]).is_err());
        let bad = "{ \"agent_servers\": null }";
        assert!(install_settings_edit(bad, "muse-acp", "/b", &[]).is_err());
    }

    #[test]
    fn uninstall_removes_entry_and_empty_parent() {
        let original = "{\n  // keep me\n  \"theme\": \"x\",\n  \"agent_servers\": {\n    \"muse-acp\": {\n      \"type\": \"custom\",\n      \"command\": \"/bin/muse-acp\"\n    }\n  }\n}\n";
        let (out, removed) = uninstall_settings_edit(original, "muse-acp").unwrap();
        assert!(removed);
        assert!(!out.contains("muse-acp"));
        assert!(!out.contains("agent_servers"));
        assert!(out.contains("// keep me"));
        assert!(out.contains("\"theme\": \"x\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn uninstall_keeps_siblings() {
        let original = "{\n  \"agent_servers\": {\n    \"other\": {\"command\": \"x\"},\n    \"muse-acp\": {\"command\": \"y\"}\n  }\n}\n";
        let (out, removed) = uninstall_settings_edit(original, "muse-acp").unwrap();
        assert!(removed);
        assert!(!out.contains("muse-acp"));
        assert!(out.contains("\"agent_servers\""));
        assert!(out.contains("\"other\""));
        assert!(check_valid_jsonc_object(&out));
    }

    #[test]
    fn uninstall_last_entry_collapses_file() {
        let (out, _) = install_settings_edit("", "muse-acp", "muse-acp", &[]).unwrap();
        let (out, removed) = uninstall_settings_edit(&out, "muse-acp").unwrap();
        assert!(removed);
        assert_eq!(out, "{}\n");
    }

    #[test]
    fn uninstall_missing_entry_is_noop() {
        let original = "{ \"theme\": \"x\" }\n";
        let (out, removed) = uninstall_settings_edit(original, "muse-acp").unwrap();
        assert!(!removed);
        assert_eq!(out, original);
    }

    #[test]
    fn arg_parsing() {
        assert!(matches!(parse_args(&[]).unwrap(), Cli::Serve));
        assert!(matches!(parse_args(&["help".into()]).unwrap(), Cli::Help));
        let args = vec!["install".into(), "--name=n".into(), "--env".into(), "A=B".into(), "--dry-run".into()];
        match parse_args(&args).unwrap() {
            Cli::Install(o) => {
                assert_eq!(o.name, "n");
                assert_eq!(o.command, "muse-acp");
                assert_eq!(o.env, vec![("A".to_string(), "B".to_string())]);
                assert!(o.dry_run);
            }
            _ => panic!("expected install"),
        }
        match parse_args(&["install".into(), "--command=/x/y".into()]).unwrap() {
            Cli::Install(o) => assert_eq!(o.command, "/x/y"),
            _ => panic!("expected install"),
        }
        assert!(parse_args(&["install".into(), "--bogus".into()]).is_err());
        assert!(parse_args(&["uninstall".into(), "--env".into(), "A=B".into()]).is_err());
        assert!(parse_args(&["frobnicate".into()]).is_err());
    }
}
