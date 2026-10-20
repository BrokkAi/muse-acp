//! Editor file tools: an ACP v1 client's file system, offered to Muse's model
//! as an MCP server.
//!
//! An ACP v1 client that advertises `fs.readTextFile` or `fs.writeTextFile`
//! accepts `fs/read_text_file` and `fs/write_text_file` from the agent and
//! answers from its buffers, unsaved changes included. Muse runs its own file
//! tools and MSP cannot hand them to the adapter, so the adapter gives each
//! session one more MCP server, `editor`, served over HTTP on 127.0.0.1 from
//! this process. Its tools turn each call into an `fs/*` request to the client
//! for the calling session.
//!
//! Every session gets its own bearer token, which both authenticates Muse and
//! names the session. Paths are confined to the session's roots, and writes
//! are refused in Read-only and Plan modes, which Muse does not apply to MCP
//! tools. Muse's approval flow still runs before every call.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::acp::{self, Sessions, StdoutShared};
use crate::json::{J, esc, j_to_string, mint_id, parse_json};
use crate::msp::log;

/// The MCP server name; the model sees `mcp__editor__<tool>`.
pub const SERVER_NAME: &str = "editor";
const TOOL_PREFIX: &str = "mcp__editor__";
const ENDPOINT: &str = "/mcp";
const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 100;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// The most file text one read returns to the model.
const MAX_READ_BYTES: usize = 256 * 1024;
/// The largest old plus new text a diff card carries.
const MAX_DIFF_BYTES: usize = 1024 * 1024;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(120);
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const INSTRUCTIONS: &str = "These tools read and write files through the user's editor. They see unsaved changes in open editor buffers and keep the editor in sync, so prefer them to read_file, write_file, and edit_file for files in the workspace.";

static READ: AtomicBool = AtomicBool::new(false);
static WRITE: AtomicBool = AtomicBool::new(false);
/// The listener's port, or None when it could not bind.
static SERVER: OnceLock<Option<u16>> = OnceLock::new();
/// Bearer token to ACP session id; empty until a new session is bound.
static TOKENS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
/// Client requests awaiting an answer, by JSON request id.
static PENDING: LazyLock<Mutex<HashMap<String, mpsc::Sender<J>>>> = LazyLock::new(Default::default);
static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);
/// Bumped by `session/cancel`; a call started under an older value stops.
static CANCELS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Default::default);
/// A running `tools/call`: its token and MCP request id (JSON text).
type CallKey = (String, String);
/// Running `tools/call` requests, for `notifications/cancelled`.
static RUNNING: LazyLock<Mutex<HashMap<CallKey, Arc<AtomicBool>>>> =
    LazyLock::new(Default::default);
/// Diff content waiting for its tool card to complete.
static CARDS: LazyLock<Mutex<Vec<Card>>> = LazyLock::new(Default::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// Record the ACP v1 client's `fs` capabilities. ACP v2 has no `fs/*`, and
/// `MUSE_EDITOR_TOOLS=0` turns the tools off.
pub fn negotiate(ver: u64, capabilities: Option<&J>) {
    let fs = capabilities.and_then(|c| c.get("fs")).filter(|_| ver == 1);
    let flag = |key: &str| fs.and_then(|fs| fs.get(key)).and_then(J::as_bool) == Some(true);
    let (read, write) = (flag("readTextFile"), flag("writeTextFile"));
    let disabled = std::env::var("MUSE_EDITOR_TOOLS").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    });
    if (read || write) && disabled {
        log("client advertised fs; editor file tools are off (MUSE_EDITOR_TOOLS)");
    }
    READ.store(read && !disabled, Ordering::SeqCst);
    WRITE.store(write && !disabled, Ordering::SeqCst);
    if offered() {
        let names = tool_names().join(", ");
        log(&format!(
            "client advertised fs; Muse sessions get the editor file tools: {names}"
        ));
    }
}

/// Whether sessions on this connection get the editor server.
pub fn offered() -> bool {
    READ.load(Ordering::SeqCst) || WRITE.load(Ordering::SeqCst)
}

fn tool_names() -> Vec<&'static str> {
    let (read, write) = (READ.load(Ordering::SeqCst), WRITE.load(Ordering::SeqCst));
    let mut names = Vec::new();
    if read {
        names.push("read_text_file");
    }
    if write {
        names.push("write_text_file");
    }
    if read && write {
        names.push("edit_text_file");
    }
    names
}

/// Start the listener once and return its port.
pub fn start(stdout: &StdoutShared, sessions: &Sessions) -> Option<u16> {
    *SERVER.get_or_init(|| {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(e) => {
                log(&format!("editor file tools unavailable: {e}"));
                return None;
            }
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(e) => {
                log(&format!("editor file tools unavailable: {e}"));
                return None;
            }
        };
        let shared = Arc::new(Shared {
            stdout: stdout.clone(),
            sessions: sessions.clone(),
        });
        let spawned = std::thread::Builder::new()
            .name("editor-tools".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let shared = shared.clone();
                    let _ = std::thread::Builder::new()
                        .name("editor-tools-conn".into())
                        .spawn(move || serve_connection(stream, &shared));
                }
            });
        if let Err(e) = spawned {
            log(&format!("editor file tools unavailable: {e}"));
            return None;
        }
        log(&format!("editor file tools listening on 127.0.0.1:{port}"));
        Some(port)
    })
}

/// The bridge token for a session: the one already bound to `acp_sid`, else
/// a new one, bound to `acp_sid` when given. Reusing it keeps the session's
/// MCP set identical when the session is loaded again.
pub fn session_token(acp_sid: Option<&str>) -> String {
    let mut tokens = lock(&TOKENS);
    if let Some(sid) = acp_sid
        && let Some((token, _)) = tokens.iter().find(|(_, bound)| bound.as_str() == sid)
    {
        return token.clone();
    }
    let token = random_hex(32);
    tokens.insert(token.clone(), acp_sid.unwrap_or("").to_string());
    token
}

/// Bind a new session's token once its id is known.
pub fn bind(token: &str, acp_sid: &str) {
    lock(&TOKENS).insert(token.to_string(), acp_sid.to_string());
}

/// The MSP `SessionMcpServerConfig` for the editor server.
pub fn server_entry(port: u16, token: &str) -> String {
    format!(
        "{{\"transport\":\"streamableHttp\",\"url\":{},\"headers\":{{\"Authorization\":{}}},\"mode\":\"optional\"}}",
        esc(&format!("http://127.0.0.1:{port}{ENDPOINT}")),
        esc(&format!("Bearer {token}"))
    )
}

/// Route a client response to the editor request waiting for it. True when
/// the id was one of ours.
pub fn complete(id: &Option<J>, msg: &J) -> bool {
    let Some(id) = id else { return false };
    let Some(waiter) = lock(&PENDING).remove(&j_to_string(id)) else {
        return false;
    };
    let _ = waiter.send(msg.clone());
    true
}

/// Stop the session's running editor calls (`session/cancel`).
pub fn cancel_session(acp_sid: &str) {
    *lock(&CANCELS).entry(acp_sid.to_string()).or_default() += 1;
}

fn cancel_generation(acp_sid: &str) -> u64 {
    lock(&CANCELS).get(acp_sid).copied().unwrap_or(0)
}

/// The diff recorded for a completed editor tool item, as ACP tool call
/// content. Matched by `callId` when Muse sent one with the call (1.4.11+),
/// else by tool and `path` argument, oldest first.
pub fn take_card_content(acp_sid: &str, item: &J) -> Option<String> {
    let tool = item
        .get("tool")
        .and_then(J::as_str)?
        .strip_prefix(TOOL_PREFIX)?;
    let call_id = item.get("callId").and_then(J::as_str);
    let mut cards = lock(&CARDS);
    let by_call = call_id.and_then(|call_id| {
        cards
            .iter()
            .position(|c| c.acp_sid == acp_sid && c.call_id.as_deref() == Some(call_id))
    });
    let position = by_call.or_else(|| {
        let args = match item.get("args")? {
            J::Str(raw) => parse_json(raw).ok()?,
            args @ J::Obj(_) => args.clone(),
            _ => return None,
        };
        let path = args.get("path").and_then(J::as_str)?;
        cards.iter().position(|c| {
            c.acp_sid == acp_sid && c.call_id.is_none() && c.tool == tool && c.path_arg == path
        })
    })?;
    Some(cards.remove(position).content)
}

struct Card {
    acp_sid: String,
    call_id: Option<String>,
    tool: String,
    path_arg: String,
    content: String,
    at: Instant,
}

fn record_card(card: Card) {
    let mut cards = lock(&CARDS);
    // A card whose item never completed (a dropped turn) ages out.
    cards.retain(|c| c.at.elapsed() < Duration::from_secs(600));
    if cards.len() >= 64 {
        cards.remove(0);
    }
    cards.push(card);
}

fn random_hex(bytes: usize) -> String {
    let mut out = vec![0u8; bytes];
    let urandom = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut out));
    if urandom.is_err() {
        // No /dev/urandom (Windows): std's `RandomState` keys come from the
        // OS generator, and each new state differs.
        use std::hash::{BuildHasher, Hasher};
        for (index, chunk) in out.chunks_mut(8).enumerate() {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_usize(index);
            let word = hasher.finish().to_be_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

struct Shared {
    stdout: StdoutShared,
    sessions: Sessions,
}

// ---- HTTP ----

struct Request {
    method: String,
    target: String,
    /// Lowercased names.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    close: bool,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, PartialEq)]
enum ReadError {
    /// The peer closed the connection or the read failed.
    Closed,
    /// A malformed request, answered with this status before closing.
    Bad(u16, &'static str),
}

struct Response {
    status: u16,
    json: Option<String>,
    headers: Vec<(&'static str, &'static str)>,
}

impl Response {
    fn status(status: u16) -> Response {
        Response {
            status,
            json: None,
            headers: Vec::new(),
        }
    }

    fn json(status: u16, json: String) -> Response {
        Response {
            status,
            json: Some(json),
            headers: Vec::new(),
        }
    }
}

fn serve_connection(stream: TcpStream, shared: &Shared) {
    let _ = stream.set_nodelay(true);
    // Muse keeps the connection between calls; one idle for five minutes is
    // dropped, and Muse opens a new one.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut writer = stream;
    loop {
        let request = match read_request(&mut reader) {
            Ok(request) => request,
            Err(ReadError::Closed) => return,
            Err(ReadError::Bad(status, _)) => {
                let _ = write_response(&mut writer, &Response::status(status), true);
                return;
            }
        };
        let response = route(&request, &|token, method, id, params| {
            handle_request(shared, token, method, id, params)
        });
        if write_response(&mut writer, &response, request.close).is_err() || request.close {
            return;
        }
    }
}

/// One line of the request head, without its line ending, charged to `budget`.
fn read_line(reader: &mut impl BufRead, budget: &mut usize) -> Result<Option<String>, ReadError> {
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(*budget as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(|_| ReadError::Closed)?;
    if read == 0 {
        return Ok(None);
    }
    if read > *budget {
        return Err(ReadError::Bad(431, "request head too large"));
    }
    if !line.ends_with(b"\n") {
        return Err(ReadError::Closed);
    }
    *budget -= read;
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|_| ReadError::Bad(400, "request head is not UTF-8"))
}

fn read_request(reader: &mut impl BufRead) -> Result<Request, ReadError> {
    let mut budget = MAX_HEAD_BYTES;
    // A blank line before the request line is allowed (RFC 9112 2.2).
    let request_line = loop {
        match read_line(reader, &mut budget)? {
            None => return Err(ReadError::Closed),
            Some(line) if line.is_empty() => continue,
            Some(line) => break line,
        }
    };
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Bad(400, "malformed request line"));
    };
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(ReadError::Bad(505, "unsupported HTTP version"));
    }
    let mut headers = Vec::new();
    loop {
        let Some(line) = read_line(reader, &mut budget)? else {
            return Err(ReadError::Closed);
        };
        if line.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            return Err(ReadError::Bad(431, "too many headers"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ReadError::Bad(400, "malformed header"));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body: Vec::new(),
        close: false,
    };
    let connection = request
        .header("connection")
        .unwrap_or("")
        .to_ascii_lowercase();
    request.close = if version == "HTTP/1.0" {
        !connection.contains("keep-alive")
    } else {
        connection.contains("close")
    };
    let chunked = request
        .header("transfer-encoding")
        .is_some_and(|te| te.to_ascii_lowercase().contains("chunked"));
    if chunked {
        request.body = read_chunked(reader)?;
    } else if let Some(length) = request.header("content-length") {
        let length: usize = length
            .parse()
            .map_err(|_| ReadError::Bad(400, "malformed content-length"))?;
        if length > MAX_BODY_BYTES {
            return Err(ReadError::Bad(413, "request body too large"));
        }
        let mut body = vec![0; length];
        reader
            .read_exact(&mut body)
            .map_err(|_| ReadError::Closed)?;
        request.body = body;
    }
    Ok(request)
}

fn read_chunked(reader: &mut impl BufRead) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    loop {
        let mut budget = 1024;
        let Some(size_line) = read_line(reader, &mut budget)? else {
            return Err(ReadError::Closed);
        };
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| ReadError::Bad(400, "malformed chunk size"))?;
        if size == 0 {
            // Trailers end at a blank line.
            let mut budget = MAX_HEAD_BYTES;
            loop {
                match read_line(reader, &mut budget)? {
                    None => return Err(ReadError::Closed),
                    Some(line) if line.is_empty() => return Ok(body),
                    Some(_) => {}
                }
            }
        }
        if body.len() + size > MAX_BODY_BYTES {
            return Err(ReadError::Bad(413, "request body too large"));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader
            .read_exact(&mut body[start..])
            .map_err(|_| ReadError::Closed)?;
        let mut budget = 2;
        if read_line(reader, &mut budget)? != Some(String::new()) {
            return Err(ReadError::Bad(400, "malformed chunk"));
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

fn write_response(out: &mut impl Write, response: &Response, close: bool) -> std::io::Result<()> {
    let body = response.json.as_deref().unwrap_or("");
    let mut head = format!(
        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\n",
        response.status,
        reason(response.status),
        body.len()
    );
    if response.json.is_some() {
        head.push_str("content-type: application/json\r\n");
    }
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if close {
        head.push_str("connection: close\r\n");
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

/// Answer one HTTP request. `handle` answers a JSON-RPC request with the
/// response's JSON text, given the caller's token.
/// Answers one JSON-RPC request: caller's token, method, id, params.
type Handler<'a> = dyn Fn(&str, &str, &J, Option<&J>) -> String + 'a;

fn route(request: &Request, handle: &Handler<'_>) -> Response {
    // A browser always sends Origin; Muse never does. Refusing it keeps web
    // pages (DNS rebinding included) away from the editor's files.
    if request.header("origin").is_some() {
        return Response::status(403);
    }
    if request.target.split('?').next() != Some(ENDPOINT) {
        return Response::status(404);
    }
    let token = request
        .header("authorization")
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
        })
        .filter(|token| lock(&TOKENS).contains_key(*token));
    let Some(token) = token else {
        let mut response = Response::status(401);
        response.headers.push(("www-authenticate", "Bearer"));
        return response;
    };
    if request.method != "POST" {
        // No server-to-client stream and no session termination.
        let mut response = Response::status(405);
        response.headers.push(("allow", "POST"));
        return response;
    }
    let message = std::str::from_utf8(&request.body)
        .ok()
        .and_then(|text| parse_json(text).ok());
    let Some(message @ J::Obj(_)) = message else {
        return Response::json(
            400,
            rpc_error(&J::Null, -32700, "expected one JSON-RPC message"),
        );
    };
    let method = message.get("method").and_then(J::as_str);
    let id = message.get("id").filter(|id| !matches!(id, J::Null));
    match (method, id) {
        (Some(method), Some(id)) => {
            Response::json(200, handle(token, method, id, message.get("params")))
        }
        (Some("notifications/cancelled"), None) => {
            if let Some(request_id) = message.get("params").and_then(|p| p.get("requestId")) {
                let key = (token.to_string(), j_to_string(request_id));
                if let Some(flag) = lock(&RUNNING).get(&key) {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            Response::status(202)
        }
        // Other notifications, and responses to requests the server never
        // sends.
        _ => Response::status(202),
    }
}

// ---- MCP ----

fn rpc_result(id: &J, result: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{result}}}",
        j_to_string(id)
    )
}

fn rpc_error(id: &J, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":{}}}}}",
        j_to_string(id),
        esc(message)
    )
}

fn handle_request(
    shared: &Shared,
    token: &str,
    method: &str,
    id: &J,
    params: Option<&J>,
) -> String {
    match method {
        "initialize" => rpc_result(id, &initialize_result(params)),
        "ping" => rpc_result(id, "{}"),
        "tools/list" => rpc_result(id, &format!("{{\"tools\":[{}]}}", tool_list().join(","))),
        "tools/call" => {
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(J::as_str)
                .unwrap_or("");
            if !tool_names().contains(&name) {
                return rpc_error(id, -32602, &format!("unknown tool: {name}"));
            }
            rpc_result(id, &call(shared, token, id, name, params))
        }
        _ => rpc_error(id, -32601, &format!("method not found: {method}")),
    }
}

fn initialize_result(params: Option<&J>) -> String {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(J::as_str);
    let version = requested
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[1]);
    format!(
        "{{\"protocolVersion\":{},\"capabilities\":{{\"tools\":{{\"listChanged\":false}}}},\"serverInfo\":{{\"name\":\"muse-acp-editor\",\"title\":\"Editor\",\"version\":{}}},\"instructions\":{}}}",
        esc(version),
        esc(env!("CARGO_PKG_VERSION")),
        esc(INSTRUCTIONS)
    )
}

const PATH_SCHEMA: &str = r#""path":{"type":"string","description":"Absolute path, or a path relative to the workspace root."}"#;

fn tool_list() -> Vec<String> {
    tool_names()
        .into_iter()
        .map(|name| match name {
            "read_text_file" => format!(
                r#"{{"name":"read_text_file","title":"Read file in editor","description":"Read a text file through the user's editor. Returns the editor's current text, including unsaved changes in open buffers, so prefer it to read_file for files the user may have open. line (1-based) and limit (a line count) read part of the file.","inputSchema":{{"type":"object","properties":{{{PATH_SCHEMA},"line":{{"type":"integer","minimum":1,"description":"First line to read, 1-based."}},"limit":{{"type":"integer","minimum":1,"description":"Most lines to read."}}}},"required":["path"]}},"annotations":{{"readOnlyHint":true,"openWorldHint":false}}}}"#
            ),
            "write_text_file" => format!(
                r#"{{"name":"write_text_file","title":"Write file in editor","description":"Write a whole text file through the user's editor, creating it if needed. The editor updates open buffers and tracks the change, so prefer it to write_file. Not allowed in Read-only or Plan mode.","inputSchema":{{"type":"object","properties":{{{PATH_SCHEMA},"content":{{"type":"string","description":"The file's complete new text."}}}},"required":["path","content"]}},"annotations":{{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":false}}}}"#
            ),
            _ => format!(
                r#"{{"name":"edit_text_file","title":"Edit file in editor","description":"Replace text in a file through the user's editor, working on its current text including unsaved changes, so prefer it to edit_file for files the user may have open. old_string must match exactly once unless replace_all is true. Not allowed in Read-only or Plan mode.","inputSchema":{{"type":"object","properties":{{{PATH_SCHEMA},"old_string":{{"type":"string","description":"The exact text to replace."}},"new_string":{{"type":"string","description":"The replacement text."}},"replace_all":{{"type":"boolean","description":"Replace every occurrence instead of exactly one."}}}},"required":["path","old_string","new_string"]}},"annotations":{{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false}}}}"#
            ),
        })
        .collect()
}

fn tool_result(text: &str, is_error: bool) -> String {
    format!(
        "{{\"content\":[{{\"type\":\"text\",\"text\":{}}}],\"isError\":{is_error}}}",
        esc(text)
    )
}

/// Run one `tools/call` for the session the token names.
fn call(shared: &Shared, token: &str, request_id: &J, name: &str, params: Option<&J>) -> String {
    let empty = J::Obj(Vec::new());
    let args = params
        .and_then(|p| p.get("arguments"))
        .filter(|args| matches!(args, J::Obj(_)))
        .unwrap_or(&empty);
    let call_id = params
        .and_then(|p| p.get("_meta"))
        .and_then(|m| m.get("callId"))
        .and_then(J::as_str)
        .map(str::to_string);
    let acp_sid = lock(&TOKENS)
        .get(token)
        .filter(|sid| !sid.is_empty())
        .cloned();
    let context = acp_sid.and_then(|acp_sid| {
        let sessions = lock(&shared.sessions);
        let s = sessions.get(&acp_sid)?;
        Some(ToolContext {
            acp_sid: acp_sid.clone(),
            cwd: s.cwd.clone(),
            roots: s.roots.clone(),
            mode: s.session_mode.clone(),
        })
    });
    let Some(context) = context else {
        return tool_result("The editor session for this call is not open.", true);
    };
    let key = (token.to_string(), j_to_string(request_id));
    let cancelled = Arc::new(AtomicBool::new(false));
    lock(&RUNNING).insert(key.clone(), cancelled.clone());
    let generation = cancel_generation(&context.acp_sid);
    let abort =
        || cancelled.load(Ordering::SeqCst) || cancel_generation(&context.acp_sid) != generation;
    let client =
        |method: &str, params: String| client_request(&shared.stdout, method, &params, &abort);
    let outcome = run_tool(&context, &client, name, args);
    lock(&RUNNING).remove(&key);
    match outcome {
        Ok(output) => {
            if let Some(diff) = output.diff {
                record_card(Card {
                    acp_sid: context.acp_sid.clone(),
                    call_id,
                    tool: name.to_string(),
                    path_arg: args
                        .get("path")
                        .and_then(J::as_str)
                        .unwrap_or("")
                        .to_string(),
                    content: diff,
                    at: Instant::now(),
                });
            }
            tool_result(&output.text, false)
        }
        Err(message) => tool_result(&message, true),
    }
}

/// Send one ACP request to the client and wait for its answer: the result,
/// or the error's message.
fn client_request(
    stdout: &StdoutShared,
    method: &str,
    params: &str,
    abort: &dyn Fn() -> bool,
) -> Result<J, String> {
    let id = J::Str(mint_id("editor-", &REQUEST_IDS));
    let key = j_to_string(&id);
    let (waiter, answer) = mpsc::channel();
    lock(&PENDING).insert(key.clone(), waiter);
    crate::msp::trace_method("acp->client", method);
    acp::send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{key},\"method\":{},\"params\":{params}}}",
            esc(method)
        ),
    );
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    let outcome = loop {
        match answer.recv_timeout(Duration::from_millis(200)) {
            Ok(response) => break Ok(response),
            Err(RecvTimeoutError::Disconnected) => break Err("the editor did not answer".into()),
            Err(RecvTimeoutError::Timeout) if abort() => {
                break Err("The user cancelled the turn.".to_string());
            }
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                break Err(format!(
                    "the editor did not answer {method} within {} seconds",
                    CLIENT_TIMEOUT.as_secs()
                ));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    };
    if lock(&PENDING).remove(&key).is_some() {
        acp::send_cancel_request(stdout, &id);
    }
    let response = outcome?;
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(J::as_str)
            .filter(|m| !m.is_empty())
            .unwrap_or("request failed");
        return Err(message.to_string());
    }
    Ok(response.get("result").cloned().unwrap_or(J::Null))
}

// ---- Tools ----

/// What a tool call knows about its session.
#[derive(Debug, Clone)]
struct ToolContext {
    acp_sid: String,
    cwd: String,
    roots: Vec<String>,
    /// `default`, `readOnly`, or `plan`.
    mode: String,
}

#[derive(Debug)]
struct ToolOutput {
    text: String,
    /// ACP `diff` tool call content for the card.
    diff: Option<String>,
}

/// Sends one ACP request to the client: method and params JSON.
type Client<'a> = dyn Fn(&str, String) -> Result<J, String> + 'a;

fn run_tool(
    context: &ToolContext,
    client: &Client<'_>,
    name: &str,
    args: &J,
) -> Result<ToolOutput, String> {
    match name {
        "read_text_file" => read_tool(context, client, args),
        "write_text_file" => write_tool(context, client, args),
        "edit_text_file" => edit_tool(context, client, args),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn string_arg<'a>(args: &'a J, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(J::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn line_arg(args: &J, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(J::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(n) if n >= 1 => Ok(Some(n)),
            _ => Err(format!("{key} must be a positive integer")),
        },
    }
}

fn writable(context: &ToolContext) -> Result<(), String> {
    match context.mode.as_str() {
        "readOnly" => Err("This session is in Read-only mode, which does not allow writing files. The user can switch the mode to Default.".to_string()),
        "plan" => Err("This session is in Plan mode, which does not allow writing files. Describe the change in the plan instead.".to_string()),
        _ => Ok(()),
    }
}

fn editor_read(
    context: &ToolContext,
    client: &Client<'_>,
    path: &str,
    range: (Option<u64>, Option<u64>),
) -> Result<String, String> {
    let mut params = format!(
        "{{\"sessionId\":{},\"path\":{}",
        esc(&context.acp_sid),
        esc(path)
    );
    if let Some(line) = range.0 {
        params.push_str(&format!(",\"line\":{line}"));
    }
    if let Some(limit) = range.1 {
        params.push_str(&format!(",\"limit\":{limit}"));
    }
    params.push('}');
    let result = client("fs/read_text_file", params)
        .map_err(|e| format!("The editor could not read {path}: {e}"))?;
    result
        .get("content")
        .and_then(J::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("The editor returned no text for {path}."))
}

fn editor_write(
    context: &ToolContext,
    client: &Client<'_>,
    path: &str,
    content: &str,
) -> Result<(), String> {
    client(
        "fs/write_text_file",
        format!(
            "{{\"sessionId\":{},\"path\":{},\"content\":{}}}",
            esc(&context.acp_sid),
            esc(path),
            esc(content)
        ),
    )
    .map(|_| ())
    .map_err(|e| format!("The editor could not write {path}: {e}"))
}

fn read_tool(context: &ToolContext, client: &Client<'_>, args: &J) -> Result<ToolOutput, String> {
    let path = resolve_path(string_arg(args, "path")?, &context.cwd, &context.roots)?;
    let range = (line_arg(args, "line")?, line_arg(args, "limit")?);
    let text = editor_read(context, client, &path, range)?;
    Ok(ToolOutput {
        text: bounded(text),
        diff: None,
    })
}

/// Cut a long read at a character boundary and say how to read the rest.
fn bounded(mut text: String) -> String {
    if text.len() <= MAX_READ_BYTES {
        return text;
    }
    let mut cut = MAX_READ_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let rest = text.len() - cut;
    text.truncate(cut);
    text.push_str(&format!(
        "\n[truncated: {rest} more bytes; read a range with line and limit]"
    ));
    text
}

/// The file's text before a write: the editor's when it can read files, else
/// the disk's. None for a new file.
fn previous_text(context: &ToolContext, client: &Client<'_>, path: &str) -> Option<String> {
    if READ.load(Ordering::SeqCst)
        && let Ok(text) = editor_read(context, client, path, (None, None))
    {
        return Some(text);
    }
    std::fs::read_to_string(path).ok()
}

fn write_tool(context: &ToolContext, client: &Client<'_>, args: &J) -> Result<ToolOutput, String> {
    writable(context)?;
    let path = resolve_path(string_arg(args, "path")?, &context.cwd, &context.roots)?;
    let content = string_arg(args, "content")?;
    let old = previous_text(context, client, &path);
    editor_write(context, client, &path, content)?;
    Ok(ToolOutput {
        text: format!(
            "{} {path} through the editor ({} lines).",
            if old.is_some() { "Wrote" } else { "Created" },
            content.lines().count()
        ),
        diff: diff_content(&path, old.as_deref(), content),
    })
}

fn edit_tool(context: &ToolContext, client: &Client<'_>, args: &J) -> Result<ToolOutput, String> {
    writable(context)?;
    let path = resolve_path(string_arg(args, "path")?, &context.cwd, &context.roots)?;
    let old_string = string_arg(args, "old_string")?;
    let new_string = string_arg(args, "new_string")?;
    let replace_all = args.get("replace_all").and_then(J::as_bool) == Some(true);
    if old_string.is_empty() {
        return Err("old_string must not be empty; use write_text_file to create a file.".into());
    }
    if old_string == new_string {
        return Err("old_string and new_string are the same.".into());
    }
    let text = editor_read(context, client, &path, (None, None))?;
    let count = text.matches(old_string).count();
    if count == 0 {
        return Err(format!(
            "old_string was not found in {path}. Read the file again: the editor's text may differ from the file on disk."
        ));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string occurs {count} times in {path}. Include more surrounding text to make it unique, or set replace_all."
        ));
    }
    let edited = if replace_all {
        text.replace(old_string, new_string)
    } else {
        text.replacen(old_string, new_string, 1)
    };
    editor_write(context, client, &path, &edited)?;
    Ok(ToolOutput {
        text: format!(
            "Edited {path} through the editor ({count} replacement{}).",
            if count == 1 { "" } else { "s" }
        ),
        diff: diff_content(&path, Some(&text), &edited),
    })
}

/// ACP `diff` tool call content, or None when the texts are too large to
/// show.
fn diff_content(path: &str, old: Option<&str>, new: &str) -> Option<String> {
    if old.map_or(0, str::len) + new.len() > MAX_DIFF_BYTES {
        return None;
    }
    Some(format!(
        "{{\"type\":\"diff\",\"path\":{},\"oldText\":{},\"newText\":{}}}",
        esc(path),
        old.map_or_else(|| "null".to_string(), esc),
        esc(new)
    ))
}

/// The absolute path for a tool's `path` argument, confined to the session's
/// roots. A relative path resolves against `cwd`, and `.` and `..` resolve
/// lexically. Containment is checked on the deepest part of the path that
/// exists, with symlinks resolved, so a link cannot lead out of the roots;
/// the rest does not exist yet and cannot be a link. Returns the lexical
/// path, which is the one the editor knows.
fn resolve_path(raw: &str, cwd: &str, roots: &[String]) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Err("path must not be empty".to_string());
    }
    let given = Path::new(raw);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else if !cwd.is_empty() && Path::new(cwd).is_absolute() {
        Path::new(cwd).join(given)
    } else {
        return Err(format!(
            "{raw} is relative, and the session has no absolute workspace root"
        ));
    };
    let mut lexical = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // `..` at the root stays at the root.
                if lexical.file_name().is_some() {
                    lexical.pop();
                }
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    let mut existing = lexical.as_path();
    let mut missing = Vec::new();
    let canonical = loop {
        match std::fs::canonicalize(existing) {
            Ok(canonical) => break canonical,
            Err(_) => {
                let (Some(name), Some(parent)) = (existing.file_name(), existing.parent()) else {
                    return Err(format!("cannot resolve {}", lexical.display()));
                };
                missing.push(name);
                existing = parent;
            }
        }
    };
    let canonical = missing
        .iter()
        .rev()
        .fold(canonical, |path, name| path.join(name));
    let inside = roots
        .iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|root| canonical.starts_with(&root)));
    if !inside {
        return Err(format!(
            "{} is outside the session's workspace roots",
            lexical.display()
        ));
    }
    Ok(lexical.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("muse-acp-bridge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws/src")).unwrap();
        std::fs::create_dir_all(dir.join("outside")).unwrap();
        std::fs::write(dir.join("ws/src/a.rs"), "disk text\n").unwrap();
        std::fs::write(dir.join("outside/secret.txt"), "secret\n").unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    /// `dir` joined with `parts` one component at a time, as a string.
    fn file(dir: &Path, parts: &[&str]) -> String {
        parts
            .iter()
            .fold(dir.to_path_buf(), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned()
    }

    fn context(dir: &Path, mode: &str) -> ToolContext {
        ToolContext {
            acp_sid: "sess-1".to_string(),
            cwd: dir.join("ws").to_string_lossy().into_owned(),
            roots: vec![dir.join("ws").to_string_lossy().into_owned()],
            mode: mode.to_string(),
        }
    }

    fn args(json: &str) -> J {
        parse_json(json).unwrap()
    }

    /// A fake editor: answers reads from `buffers` and records every request.
    struct Editor {
        buffers: RefCell<HashMap<String, String>>,
        requests: RefCell<Vec<(String, J)>>,
    }

    impl Editor {
        fn new(buffers: &[(&str, &str)]) -> Editor {
            Editor {
                buffers: RefCell::new(
                    buffers
                        .iter()
                        .map(|(path, text)| (path.to_string(), text.to_string()))
                        .collect(),
                ),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn answer(&self, method: &str, params: String) -> Result<J, String> {
            let params = parse_json(&params).unwrap();
            self.requests
                .borrow_mut()
                .push((method.to_string(), params.clone()));
            let path = params.get("path").and_then(J::as_str).unwrap().to_string();
            match method {
                "fs/read_text_file" => self
                    .buffers
                    .borrow()
                    .get(&path)
                    .map(|text| parse_json(&format!("{{\"content\":{}}}", esc(text))).unwrap())
                    .ok_or_else(|| "Resource not found".to_string()),
                "fs/write_text_file" => {
                    let content = params.get("content").and_then(J::as_str).unwrap();
                    self.buffers.borrow_mut().insert(path, content.to_string());
                    Ok(J::Null)
                }
                other => panic!("unexpected {other}"),
            }
        }

        fn methods(&self) -> Vec<String> {
            self.requests
                .borrow()
                .iter()
                .map(|(m, _)| m.clone())
                .collect()
        }
    }

    fn run(
        editor: &Editor,
        context: &ToolContext,
        name: &str,
        json: &str,
    ) -> Result<ToolOutput, String> {
        READ.store(true, Ordering::SeqCst);
        WRITE.store(true, Ordering::SeqCst);
        let client = |method: &str, params: String| editor.answer(method, params);
        run_tool(context, &client, name, &args(json))
    }

    #[test]
    fn read_returns_the_editor_text_for_a_relative_path() {
        let dir = workspace("read");
        let file = file(&dir, &["ws", "src", "a.rs"]);
        let editor = Editor::new(&[(&file, "unsaved buffer\n")]);
        let output = run(
            &editor,
            &context(&dir, "readOnly"),
            "read_text_file",
            r#"{"path":"src/./a.rs","line":2,"limit":5}"#,
        )
        .unwrap();
        assert_eq!(output.text, "unsaved buffer\n");
        assert!(output.diff.is_none());
        let requests = editor.requests.borrow();
        let (method, params) = &requests[0];
        assert_eq!(method, "fs/read_text_file");
        assert_eq!(params.get("sessionId").and_then(J::as_str), Some("sess-1"));
        assert_eq!(params.get("path").and_then(J::as_str), Some(file.as_str()));
        assert_eq!(params.get("line").and_then(J::as_u64), Some(2));
        assert_eq!(params.get("limit").and_then(J::as_u64), Some(5));
    }

    #[test]
    fn paths_outside_the_roots_never_reach_the_editor() {
        let dir = workspace("outside");
        let editor = Editor::new(&[]);
        let ctx = context(&dir, "default");
        for path in [
            "../outside/secret.txt".to_string(),
            file(&dir, &["outside", "secret.txt"]),
            "/".to_string(),
        ] {
            let error = run(
                &editor,
                &ctx,
                "read_text_file",
                &format!("{{\"path\":{}}}", esc(&path)),
            )
            .unwrap_err();
            assert!(
                error.contains("outside the session's workspace roots"),
                "{path}: {error}"
            );
        }
        let error = run(
            &editor,
            &ctx,
            "write_text_file",
            r#"{"path":"../outside/new.txt","content":"x"}"#,
        )
        .unwrap_err();
        assert!(error.contains("outside"), "{error}");
        assert!(editor.methods().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_lead_out_of_the_roots() {
        let dir = workspace("symlink");
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("ws/link")).unwrap();
        let editor = Editor::new(&[]);
        let ctx = context(&dir, "default");
        for path in ["link/secret.txt", "link/new.txt"] {
            let error = run(
                &editor,
                &ctx,
                "read_text_file",
                &format!("{{\"path\":{}}}", esc(path)),
            )
            .unwrap_err();
            assert!(error.contains("outside"), "{path}: {error}");
        }
        assert!(editor.methods().is_empty());
    }

    #[test]
    fn write_creates_a_file_in_a_new_folder_and_shows_a_diff() {
        let dir = workspace("create");
        let editor = Editor::new(&[]);
        let output = run(
            &editor,
            &context(&dir, "default"),
            "write_text_file",
            r#"{"path":"new/b.txt","content":"one\ntwo\n"}"#,
        )
        .unwrap();
        let file = file(&dir, &["ws", "new", "b.txt"]);
        assert_eq!(
            output.text,
            format!("Created {file} through the editor (2 lines).")
        );
        let diff = parse_json(output.diff.as_deref().unwrap()).unwrap();
        assert_eq!(diff.get("type").and_then(J::as_str), Some("diff"));
        assert_eq!(diff.get("path").and_then(J::as_str), Some(file.as_str()));
        assert!(matches!(diff.get("oldText"), Some(J::Null)));
        assert_eq!(diff.get("newText").and_then(J::as_str), Some("one\ntwo\n"));
        assert_eq!(
            editor.methods(),
            ["fs/read_text_file", "fs/write_text_file"]
        );
        assert_eq!(editor.buffers.borrow()[&file], "one\ntwo\n");
    }

    #[test]
    fn write_diffs_against_the_editor_text() {
        let dir = workspace("overwrite");
        let file = file(&dir, &["ws", "src", "a.rs"]);
        let editor = Editor::new(&[(&file, "buffer text\n")]);
        let output = run(
            &editor,
            &context(&dir, "default"),
            "write_text_file",
            r#"{"path":"src/a.rs","content":"new\n"}"#,
        )
        .unwrap();
        assert!(output.text.starts_with("Wrote "), "{}", output.text);
        let diff = parse_json(output.diff.as_deref().unwrap()).unwrap();
        assert_eq!(
            diff.get("oldText").and_then(J::as_str),
            Some("buffer text\n")
        );
    }

    #[test]
    fn edit_replaces_one_match_in_the_editor_text() {
        let dir = workspace("edit");
        let file = file(&dir, &["ws", "src", "a.rs"]);
        let editor = Editor::new(&[(&file, "fn a() {}\nfn b() {}\n")]);
        let ctx = context(&dir, "default");
        let output = run(
            &editor,
            &ctx,
            "edit_text_file",
            r#"{"path":"src/a.rs","old_string":"fn b","new_string":"fn c"}"#,
        )
        .unwrap();
        assert_eq!(
            output.text,
            format!("Edited {file} through the editor (1 replacement).")
        );
        assert_eq!(editor.buffers.borrow()[&file], "fn a() {}\nfn c() {}\n");
        let diff = parse_json(output.diff.as_deref().unwrap()).unwrap();
        assert_eq!(
            diff.get("oldText").and_then(J::as_str),
            Some("fn a() {}\nfn b() {}\n")
        );

        let error = run(
            &editor,
            &ctx,
            "edit_text_file",
            r#"{"path":"src/a.rs","old_string":"fn","new_string":"pub fn"}"#,
        )
        .unwrap_err();
        assert!(error.contains("occurs 2 times"), "{error}");
        let output = run(
            &editor,
            &ctx,
            "edit_text_file",
            r#"{"path":"src/a.rs","old_string":"fn","new_string":"pub fn","replace_all":true}"#,
        )
        .unwrap();
        assert!(output.text.contains("2 replacements"), "{}", output.text);
        let error = run(
            &editor,
            &ctx,
            "edit_text_file",
            r#"{"path":"src/a.rs","old_string":"missing","new_string":"x"}"#,
        )
        .unwrap_err();
        assert!(error.contains("not found"), "{error}");
        let error = run(
            &editor,
            &ctx,
            "edit_text_file",
            r#"{"path":"src/a.rs","old_string":"","new_string":"x"}"#,
        )
        .unwrap_err();
        assert!(error.contains("must not be empty"), "{error}");
    }

    #[test]
    fn read_only_and_plan_refuse_writes_before_asking_the_editor() {
        let dir = workspace("modes");
        let editor = Editor::new(&[]);
        for mode in ["readOnly", "plan"] {
            for (tool, json) in [
                ("write_text_file", r#"{"path":"src/a.rs","content":"x"}"#),
                (
                    "edit_text_file",
                    r#"{"path":"src/a.rs","old_string":"a","new_string":"b"}"#,
                ),
            ] {
                let error = run(&editor, &context(&dir, mode), tool, json).unwrap_err();
                assert!(
                    error.contains("does not allow writing files"),
                    "{mode} {tool}: {error}"
                );
            }
        }
        assert!(editor.methods().is_empty());
    }

    #[test]
    fn bad_arguments_are_tool_errors() {
        let dir = workspace("args");
        let editor = Editor::new(&[]);
        let ctx = context(&dir, "default");
        for (tool, json, expected) in [
            ("read_text_file", "{}", "path must be a string"),
            ("read_text_file", r#"{"path":""}"#, "path must not be empty"),
            (
                "read_text_file",
                r#"{"path":"src/a.rs","line":0}"#,
                "line must be a positive integer",
            ),
            (
                "write_text_file",
                r#"{"path":"src/a.rs"}"#,
                "content must be a string",
            ),
        ] {
            let error = run(&editor, &ctx, tool, json).unwrap_err();
            assert!(error.contains(expected), "{tool} {json}: {error}");
        }
    }

    #[test]
    fn an_editor_error_explains_the_failed_read() {
        let dir = workspace("missing");
        let editor = Editor::new(&[]);
        let error = run(
            &editor,
            &context(&dir, "default"),
            "read_text_file",
            r#"{"path":"src/none.rs"}"#,
        )
        .unwrap_err();
        assert!(
            error.contains("The editor could not read") && error.contains("Resource not found"),
            "{error}"
        );
    }

    #[test]
    fn long_reads_are_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_READ_BYTES);
        let cut = bounded(text);
        assert!(cut.contains("[truncated: "), "marker");
        assert!(cut.len() < MAX_READ_BYTES + 100);
        assert_eq!(bounded("short".to_string()), "short");
    }

    fn request(raw: &str) -> Result<Request, ReadError> {
        read_request(&mut BufReader::new(raw.as_bytes()))
    }

    #[test]
    fn http_requests_parse_with_length_or_chunks() {
        let r = request("POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nabcd").unwrap();
        assert_eq!(
            (
                r.method.as_str(),
                r.target.as_str(),
                r.body.as_slice(),
                r.close
            ),
            ("POST", "/mcp", &b"abcd"[..], false)
        );
        assert_eq!(r.header("host"), Some("x"));
        let r = request("POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n2;x=y\r\nde\r\n0\r\nTrailer: t\r\n\r\n").unwrap();
        assert_eq!(r.body, b"abcde");
        assert!(r.close);
        assert!(request("GET /mcp HTTP/1.0\r\n\r\n").unwrap().close);
        assert_eq!(request("").err(), Some(ReadError::Closed));
        assert!(matches!(
            request("BROKEN\r\n\r\n"),
            Err(ReadError::Bad(400, _))
        ));
        assert!(matches!(
            request("GET / HTTP/2\r\n\r\n"),
            Err(ReadError::Bad(505, _))
        ));
        let huge = format!(
            "GET / HTTP/1.1\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_HEAD_BYTES)
        );
        assert!(matches!(request(&huge), Err(ReadError::Bad(431, _))));
        let big = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert!(matches!(request(&big), Err(ReadError::Bad(413, _))));
    }

    fn http(method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> Request {
        Request {
            method: method.to_string(),
            target: target.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
            close: false,
        }
    }

    #[test]
    fn routing_requires_the_token_and_refuses_browsers() {
        let token = session_token(Some("sess-route"));
        let auth = format!("Bearer {token}");
        let handle = |t: &str, method: &str, id: &J, _: Option<&J>| {
            rpc_result(
                id,
                &format!("{{\"token\":{},\"method\":{}}}", esc(t), esc(method)),
            )
        };
        let ping = r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;
        let ok = route(
            &http("POST", "/mcp", &[("authorization", &auth)], ping),
            &handle,
        );
        assert_eq!(ok.status, 200);
        let body = parse_json(ok.json.as_deref().unwrap()).unwrap();
        assert_eq!(body.get("id").and_then(J::as_u64), Some(7));
        assert_eq!(
            body.get("result")
                .and_then(|r| r.get("token"))
                .and_then(J::as_str),
            Some(token.as_str())
        );
        assert_eq!(route(&http("POST", "/mcp", &[], ping), &handle).status, 401);
        assert_eq!(
            route(
                &http("POST", "/mcp", &[("authorization", "Bearer nope")], ping),
                &handle
            )
            .status,
            401
        );
        assert_eq!(
            route(
                &http(
                    "POST",
                    "/mcp",
                    &[("authorization", &auth), ("origin", "http://evil")],
                    ping
                ),
                &handle
            )
            .status,
            403
        );
        assert_eq!(
            route(
                &http("POST", "/other", &[("authorization", &auth)], ping),
                &handle
            )
            .status,
            404
        );
        assert_eq!(
            route(
                &http("GET", "/mcp", &[("authorization", &auth)], ""),
                &handle
            )
            .status,
            405
        );
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let accepted = route(
            &http("POST", "/mcp", &[("authorization", &auth)], note),
            &handle,
        );
        assert_eq!((accepted.status, accepted.json.is_none()), (202, true));
        assert_eq!(
            route(
                &http("POST", "/mcp", &[("authorization", &auth)], "[1]"),
                &handle
            )
            .status,
            400
        );
    }

    #[test]
    fn a_notification_cancels_its_running_call() {
        let token = session_token(Some("sess-cancel"));
        let flag = Arc::new(AtomicBool::new(false));
        lock(&RUNNING).insert((token.clone(), "9".to_string()), flag.clone());
        let note =
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}"#;
        let handle = |_: &str, _: &str, id: &J, _: Option<&J>| rpc_result(id, "{}");
        let response = route(
            &http(
                "POST",
                "/mcp",
                &[("authorization", &format!("Bearer {token}"))],
                note,
            ),
            &handle,
        );
        assert_eq!(response.status, 202);
        assert!(flag.load(Ordering::SeqCst));
        lock(&RUNNING).remove(&(token, "9".to_string()));
    }

    #[test]
    fn a_session_keeps_its_token() {
        let token = session_token(Some("sess-keep"));
        assert_eq!(session_token(Some("sess-keep")), token);
        assert_eq!(token.len(), 64);
        let fresh = session_token(None);
        assert_ne!(fresh, token);
        bind(&fresh, "sess-new");
        assert_eq!(session_token(Some("sess-new")), fresh);
    }

    #[test]
    fn initialize_echoes_a_known_protocol_version() {
        let result = |version: &str| {
            let params = parse_json(&format!("{{\"protocolVersion\":{}}}", esc(version))).unwrap();
            let result = parse_json(&initialize_result(Some(&params))).unwrap();
            result
                .get("protocolVersion")
                .and_then(J::as_str)
                .unwrap()
                .to_string()
        };
        assert_eq!(result("2025-03-26"), "2025-03-26");
        assert_eq!(result("1999-01-01"), "2025-06-18");
    }

    #[test]
    fn card_content_matches_by_call_id_then_by_path() {
        for (call_id, path) in [(Some("call-1"), "a.rs"), (None, "b.rs")] {
            record_card(Card {
                acp_sid: "sess-cards".into(),
                call_id: call_id.map(str::to_string),
                tool: "write_text_file".into(),
                path_arg: path.into(),
                content: format!("diff-{path}"),
                at: Instant::now(),
            });
        }
        let item = |call: &str, path: &str| {
            parse_json(&format!(
                "{{\"tool\":\"mcp__editor__write_text_file\",\"callId\":{},\"args\":{}}}",
                esc(call),
                esc(&format!("{{\"path\":{}}}", esc(path)))
            ))
            .unwrap()
        };
        assert_eq!(take_card_content("other", &item("call-1", "a.rs")), None);
        assert_eq!(
            take_card_content("sess-cards", &item("call-1", "a.rs")).as_deref(),
            Some("diff-a.rs")
        );
        assert_eq!(
            take_card_content("sess-cards", &item("call-9", "b.rs")).as_deref(),
            Some("diff-b.rs")
        );
        assert_eq!(
            take_card_content("sess-cards", &item("call-9", "b.rs")),
            None
        );
    }
}
