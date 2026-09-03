//! muse-acp: ACP (v2 primary, v1 fallback) server backed by one `muse serve` host.
//!
//! ACP client <-> stdio NDJSON <-> this adapter <-> stdio NDJSON <-> serve host.
//! One host serves all ACP sessions; `session/start` auto-subscribes us to its
//! view, so turns stream in as `item/*` + `turn/*` notifications.

mod acp;
mod fold;
mod json;
mod msp;

use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}, mpsc};

use acp::{AcpSession, InFlight, PendingPerm, Sessions, StdoutShared};
use fold::SessionFold;
use json::{esc, j_to_string, mint_id, parse_json, J};
use msp::{MspEvent, MspHost, err_code, err_message, log};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static VER: AtomicU64 = AtomicU64::new(0); // negotiated ACP version for the connection
static ELICIT_FORM: AtomicU64 = AtomicU64::new(0); // 1 when the v2 client advertises elicitation.form
/// Cached model catalog: (modelId, displayLabel, isDefault).
static CATALOG: std::sync::OnceLock<Mutex<Vec<(String, String, bool)>>> = std::sync::OnceLock::new();

fn catalog(host: &Arc<MspHost>) -> Vec<(String, String, bool)> {
    let cell = CATALOG.get_or_init(|| Mutex::new(Vec::new()));
    {
        let guard = cell.lock().unwrap();
        if !guard.is_empty() {
            return guard.clone();
        }
    }
    let mut out = Vec::new();
    if let Ok(r) = host.command("model/list", &format!("{{\"commandId\":{}}}", esc(&host.mint_cmd("cmd-")))) {
        if let Some(J::Arr(models)) = r.get("models") {
            for m in models {
                let id = m.get("modelId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if id.is_empty() {
                    continue;
                }
                let label = m.get("displayLabel").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
                let def = matches!(m.get("isDefault"), Some(J::Bool(true)));
                out.push((id, label, def));
            }
        }
    }
    *cell.lock().unwrap() = out.clone();
    out
}

enum LoopMsg {
    AcpLine(String),
    AcpEof,
    Msp(MspEvent),
}

const V2_INIT: &str = r#"{"protocolVersion":2,"capabilities":{"session":{"prompt":{"image":{},"embeddedContext":{}}}},"info":{"name":"muse-acp","title":"Muse ACP","version":"0.2.0"},"authMethods":[]}"#;
const V1_INIT: &str = r#"{"protocolVersion":1,"agentCapabilities":{"promptCapabilities":{"text":true,"image":true,"audio":false,"embeddedContext":true},"mcpCapabilities":false,"loadSession":true},"agentInfo":{"name":"muse-acp","title":"Muse ACP","version":"0.2.0"}}"#;

fn selftest() -> i32 {
    // Validate every static emitted literal with our own parser, so a
    // misplaced brace fails here instead of at a live client.
    for lit in [V2_INIT, V1_INIT] {
        if let Err(e) = parse_json(lit) {
            eprintln!("[muse-acp] selftest FAIL: {e} in {lit}");
            return 1;
        }
    }
    println!("[muse-acp] selftest: static literals OK");
    0
}

fn main() {
    if std::env::args().any(|a| a == "--selftest") {
        std::process::exit(selftest());
    }
    let stdout: StdoutShared = Arc::new(Mutex::new(std::io::stdout()));
    let sessions: Sessions = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let (tx, rx) = mpsc::channel::<LoopMsg>();

    // ACP stdin pump. EOF ends the adapter: the client is gone, and the
    // serve host (our child) dies with us.
    let stdin_tx = tx.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(std::io::stdin());
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = stdin_tx.send(LoopMsg::AcpEof);
                    break;
                }
                Ok(_) => {
                    if stdin_tx.send(LoopMsg::AcpLine(line.clone())).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = stdin_tx.send(LoopMsg::AcpEof);
                    break;
                }
            }
        }
    });

    // Serve host + notification forwarder.
    let (host, msp_rx) = match MspHost::launch() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[muse-acp] fatal: {e}");
            std::process::exit(1);
        }
    };
    let fwd_tx = tx.clone();
    std::thread::spawn(move || {
        for ev in msp_rx {
            if fwd_tx.send(LoopMsg::Msp(ev)).is_err() {
                break;
            }
        }
    });

    for msg in rx {
        match msg {
            LoopMsg::AcpLine(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match parse_json(trimmed) {
                    Ok(v) => handle_acp(&host, &stdout, &sessions, &v),
                    Err(e) => acp::send_error(&stdout, &None, -32700, &format!("parse error: {e}")),
                }
            }
            LoopMsg::Msp(MspEvent::Notification { method, params }) => {
                handle_msp(&host, &stdout, &sessions, &method, &params)
            }
            LoopMsg::AcpEof => {
                std::process::exit(0);
            }
            LoopMsg::Msp(MspEvent::Eof(why)) => {
                log(&format!("serve host gone ({why}); failing in-flight turns"));
                fail_all(&stdout, &sessions);
                std::process::exit(1);
            }
        }
    }
}

fn negotiated_ver() -> u8 {
    match VER.load(Ordering::SeqCst) {
        2 => 2,
        _ => 1,
    }
}

fn handle_acp(host: &Arc<MspHost>, stdout: &StdoutShared, sessions: &Sessions, msg: &J) {
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned();

    // No method: a client response — maybe to our session/request_permission
    // or elicitation/create.
    if method.is_empty() {
        if id.is_some() {
            complete_permission(host, stdout, sessions, &id, msg);
            complete_elicitation(host, stdout, sessions, &id, msg);
        }
        return;
    }

    match method.as_str() {
        "initialize" => {
            let v = params
                .as_ref()
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|n| n.as_u64())
                .unwrap_or(1);
            let v = if v >= 2 { 2 } else { 1 };
            VER.store(v, Ordering::SeqCst);
            // v2 elicitation support gates the userInput bridge.
            let form = params
                .as_ref()
                .and_then(|p| p.get("capabilities"))
                .and_then(|c| c.get("elicitation"))
                .and_then(|e| e.get("form"))
                .map(|f| !matches!(f, J::Null))
                .unwrap_or(false);
            ELICIT_FORM.store(u64::from(v == 2 && form), Ordering::SeqCst);
            if v == 2 {
                acp::send_result(stdout, &id, V2_INIT);
            } else {
                acp::send_result(stdout, &id, V1_INIT);
            }
        }
        "session/new" => {
            let ver = negotiated_ver();
            let cwd = params
                .as_ref()
                .and_then(|p| p.get("cwd"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if cwd.is_empty() {
                acp::send_error(stdout, &id, -32602, "session/new requires params.cwd");
                return;
            }
            let cmd = host.mint_cmd("cmd-");
            let res = host.command("session/start", &format!("{{\"commandId\":{},\"workspaceRoot\":{}}}", esc(&cmd), esc(&cwd)));
            match res {
                Ok(r) => {
                    let msp_sid = match r.get("session").and_then(|s| s.get("sessionId")).and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => {
                            acp::send_error(stdout, &id, -32603, "session/start returned no session.sessionId");
                            return;
                        }
                    };
                    let sid = mint_id("sess-", &ID_COUNTER);
                    // Optional approval posture (allowAll|promptUnmatched|onRequest|denyUnmatched).
                    let mode = std::env::var("MUSE_APPROVAL_MODE").unwrap_or_default();
                    if !mode.trim().is_empty() {
                        let cmd2 = host.mint_cmd("cmd-");
                        if let Err(e) = host.command("session/setApprovalMode", &format!("{{\"commandId\":{},\"sessionId\":{},\"mode\":{}}}", esc(&cmd2), esc(&msp_sid), esc(mode.trim()))) {
                            log(&format!("setApprovalMode failed: {}", err_message(&e)));
                        }
                    }
                    let cur_mode = r
                        .get("session")
                        .and_then(|s| s.get("approvalMode"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("promptUnmatched");
                    sessions.lock().unwrap().insert(sid.clone(), AcpSession {
                        acp_sid: sid.clone(),
                        msp_sid: msp_sid.clone(),
                        ver,
                        in_flight: Vec::new(),
                        pending_perm: None,
                        pending_ui: None,
                        fold: SessionFold::new(),
                    });
                    // _meta exposes the host session id: pass it back to
                    // session/resume to reconnect after an adapter restart.
                    // v2 also advertises mode + model selectors.
                    let mut result = format!("{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}}}}", esc(&sid), esc(&msp_sid));
                    if ver == 2 {
                        let models = catalog(host);
                        result = format!(
                            "{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}},\"configOptions\":{}}}",
                            esc(&sid),
                            esc(&msp_sid),
                            acp::config_options(acp::mode_from_msp(cur_mode), &models)
                        );
                    }
                    acp::send_result(stdout, &id, &result);
                }
                Err(e) => acp::send_error(stdout, &id, -32603, &format!("session/start failed: {}", err_message(&e))),
            }
        }
        "session/resume" | "session/load" => {
            let ver = negotiated_ver();
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sid.is_empty() {
                acp::send_error(stdout, &id, -32602, "session resume requires params.sessionId");
                return;
            }
            // Known ACP session: re-attach. Unknown id: try it as a host
            // session id directly (cross-restart resume), then adopt it.
            let msp_sid = sessions
                .lock()
                .unwrap()
                .get(&sid)
                .map(|s| s.msp_sid.clone())
                .unwrap_or_else(|| sid.clone());
            let cmd = host.mint_cmd("cmd-");
            match host.command("session/resume", &format!("{{\"commandId\":{},\"sessionId\":{}}}", esc(&cmd), esc(&msp_sid))) {
                Ok(r) => {
                    // Pending questions/approvals survive reconnects; the host
                    // re-issues their requests, which the normal bridge picks
                    // up. Log them so a stuck-looking turn is diagnosable.
                    if let Some(J::Arr(pend)) = r.get("pendingRequests") {
                        for p in pend {
                            log(&format!("resume: pending request {}", j_to_string(p)));
                        }
                    }
                    let real_msp = r
                        .get("session")
                        .and_then(|s| s.get("sessionId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&msp_sid)
                        .to_string();
                    let replay = method == "session/resume"
                        && params.as_ref().and_then(|p| p.get("replayFrom")).is_some();
                    {
                        let mut map = sessions.lock().unwrap();
                        let entry = map.entry(sid.clone()).or_insert_with(|| AcpSession {
                            acp_sid: sid.clone(),
                            msp_sid: real_msp.clone(),
                            ver,
                            in_flight: Vec::new(),
                            pending_perm: None,
                            pending_ui: None,
                            fold: SessionFold::new(),
                        });
                        entry.msp_sid = real_msp;
                        entry.ver = ver;
                        if replay {
                            replay_history(stdout, entry, &r);
                        }
                    }
                    let msp_out = sessions.lock().unwrap().get(&sid).map(|s| s.msp_sid.clone()).unwrap_or_default();
                    acp::send_result(stdout, &id, &format!("{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}}}}", esc(&sid), esc(&msp_out)));
                }
                Err(e) => acp::send_error(stdout, &id, -32602, &format!("resume failed: {}", err_message(&e))),
            }
        }
        "session/prompt" => {
            let ver = negotiated_ver();
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let parts = match extract_prompt_parts(params.as_ref()) {
                Ok(p) if !p.is_empty() => p,
                Ok(_) => {
                    acp::send_error(stdout, &id, -32602, "session/prompt requires content");
                    return;
                }
                Err(e) => {
                    acp::send_error(stdout, &id, -32602, &e);
                    return;
                }
            };
            let msp_sid = match sessions.lock().unwrap().get(&sid) {
                Some(s) => s.msp_sid.clone(),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            // The host queues concurrent turns itself (ifBusy defaults to
            // queue); track every in-flight turn so each completes its own
            // prompt response.
            let cmd = host.mint_cmd("cmd-");
            let input = format!("[{}]", parts.join(","));
            match host.command("turn/start", &format!("{{\"commandId\":{},\"sessionId\":{},\"input\":{}}}", esc(&cmd), esc(&msp_sid), input)) {
                Ok(r) => {
                    let turn = r.get("turnId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if let Some(s) = sessions.lock().unwrap().get_mut(&sid) {
                        s.in_flight.push(InFlight { msp_turn: turn, req_id: id.clone().unwrap_or(J::Null) });
                    }
                    if ver == 2 {
                        // Accepted: empty response; completion via state_update.
                        acp::send_result(stdout, &id, "{}");
                        acp::send_state(stdout, &sid, "running", None);
                    }
                    // v1: the prompt response arrives with the terminal.
                }
                Err(e) => {
                    let code = err_code(&e);
                    if code == -32000 || err_message(&e).contains("already_terminal") {
                        acp::send_error(stdout, &id, -32603, &format!("turn rejected: {}", err_message(&e)));
                    } else {
                        acp::send_error(stdout, &id, -32603, &format!("turn/start failed: {}", err_message(&e)));
                    }
                }
            }
        }
        "session/cancel" => {
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sid.is_empty() {
                return; // notification: nothing to acknowledge
            }
            let turns = sessions.lock().unwrap().get(&sid).map(|s| {
                let msp = s.msp_sid.clone();
                s.in_flight.iter().map(|f| (msp.clone(), f.msp_turn.clone())).collect::<Vec<_>>()
            }).unwrap_or_default();
            // session/cancel stops all session work: cancel every in-flight turn.
            for (msp_sid, turn_id) in turns {
                let cmd = host.mint_cmd("cmd-");
                match host.command("turn/cancel", &format!("{{\"commandId\":{},\"sessionId\":{},\"turnId\":{}}}", esc(&cmd), esc(&msp_sid), esc(&turn_id))) {
                    Ok(_) => {}
                    Err(e) => {
                        // already_terminal just means the terminal event is on
                        // its way (or arrived); anything else is real.
                        if !(err_message(&e).contains("already_terminal") || err_code(&e) == -32000) {
                            log(&format!("turn/cancel failed: {}", err_message(&e)));
                        }
                    }
                }
            }
            // Pending permission answers stay open: per ACP the client answers
            // them Cancelled itself as part of cancellation.
        }
        "session/list" => {
            let cmd = host.mint_cmd("cmd-");
            match host.command("session/list", &format!("{{\"commandId\":{}}}", esc(&cmd))) {
                Ok(r) => {
                    // Report the sessions this adapter owns.
                    let map = sessions.lock().unwrap();
                    let ids: Vec<String> = map.keys().map(|k| format!("{{\"sessionId\":{}}}", esc(k))).collect();
                    drop(map);
                    let _ = r;
                    acp::send_result(stdout, &id, &format!("{{\"sessions\":[{}]}}", ids.join(",")));
                }
                Err(e) => acp::send_error(stdout, &id, -32603, &format!("session/list failed: {}", err_message(&e))),
            }
        }
        "session/set_config_option" => {
            // v2 config selectors: mode -> approval posture, model -> catalog id.
            let sid = params.as_ref().and_then(|p| p.get("sessionId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let key = params.as_ref().and_then(|p| p.get("configId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let value = params.as_ref().and_then(|p| p.get("value")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let msp_sid = match sessions.lock().unwrap().get(&sid) {
                Some(s) => s.msp_sid.clone(),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            let cmd = host.mint_cmd("cmd-");
            let r = match key.as_str() {
                "mode" => match acp::mode_to_msp(&value) {
                    Some(m) => host.command("session/setApprovalMode", &format!("{{\"commandId\":{},\"sessionId\":{},\"mode\":{}}}", esc(&cmd), esc(&msp_sid), esc(m))),
                    None => {
                        acp::send_error(stdout, &id, -32602, "mode must be ask|auto|deny");
                        return;
                    }
                },
                "model" => host.command("session/setModel", &format!("{{\"commandId\":{},\"sessionId\":{},\"model\":{{\"modelId\":{}}}}}", esc(&cmd), esc(&msp_sid), esc(&value))),
                _ => {
                    acp::send_error(stdout, &id, -32602, "unknown configId (want mode|model)");
                    return;
                }
            };
            match r {
                Ok(_) => acp::send_result(stdout, &id, &format!("{{\"configId\":{},\"value\":{}}}", esc(&key), esc(&value))),
                Err(e) => acp::send_error(stdout, &id, -32603, &format!("set failed: {}", err_message(&e))),
            }
        }
        "session/set_mode" => {
            // v1 operating mode switch, same ask|auto|deny vocabulary.
            let sid = params.as_ref().and_then(|p| p.get("sessionId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let value = params.as_ref().and_then(|p| p.get("mode")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let msp_sid = match sessions.lock().unwrap().get(&sid) {
                Some(s) => s.msp_sid.clone(),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            match acp::mode_to_msp(&value) {
                Some(m) => {
                    let cmd = host.mint_cmd("cmd-");
                    match host.command("session/setApprovalMode", &format!("{{\"commandId\":{},\"sessionId\":{},\"mode\":{}}}", esc(&cmd), esc(&msp_sid), esc(m))) {
                        Ok(_) => acp::send_result(stdout, &id, &format!("{{\"mode\":{}}}", esc(&value))),
                        Err(e) => acp::send_error(stdout, &id, -32603, &format!("set failed: {}", err_message(&e))),
                    }
                }
                None => acp::send_error(stdout, &id, -32602, "mode must be ask|auto|deny"),
            }
        }
        "session/set_model" => {
            let sid = params.as_ref().and_then(|p| p.get("sessionId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let value = params.as_ref().and_then(|p| p.get("model")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if value.is_empty() {
                acp::send_error(stdout, &id, -32602, "session/set_model requires params.model");
                return;
            }
            let msp_sid = match sessions.lock().unwrap().get(&sid) {
                Some(s) => s.msp_sid.clone(),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            let cmd = host.mint_cmd("cmd-");
            match host.command("session/setModel", &format!("{{\"commandId\":{},\"sessionId\":{},\"model\":{{\"modelId\":{}}}}}", esc(&cmd), esc(&msp_sid), esc(&value))) {
                Ok(_) => acp::send_result(stdout, &id, &format!("{{\"model\":{}}}", esc(&value))),
                Err(e) => acp::send_error(stdout, &id, -32603, &format!("set failed: {}", err_message(&e))),
            }
        }
        "authenticate" | "auth/login" | "auth/logout" | "logout" => {
            // The host exposes no auth surface (authMethods is []); there is
            // nothing to log in to. muse credentials live outside ACP.
            acp::send_error(stdout, &id, -32601, "method not supported by this agent");
        }
        "shutdown" | "exit" => {
            if method == "shutdown" {
                acp::send_result(stdout, &id, "null");
            }
            std::process::exit(0);
        }
        _ => {
            if id.is_some() {
                acp::send_error(stdout, &id, -32601, "method not found");
            }
        }
    }
}

/// Best-effort history replay for `session/resume` with `replayFrom`.
/// Unknown history shapes resume without replay (logged), never fail.
fn replay_history(stdout: &StdoutShared, sess: &mut AcpSession, resume_res: &J) {
    let items = match resume_res.get("history").and_then(|h| h.get("items")) {
        Some(J::Arr(v)) => v.clone(),
        _ => {
            log("resume: unrecognized history shape; resumed without replay");
            return;
        }
    };
    for it in items {
        let kind = it.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let text = it.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let msg_id = mint_id("msg-", &ID_COUNTER);
        let content = format!("[{{\"type\":\"text\",\"text\":{}}}]", esc(text));
        let update = match kind {
            "userMessage" => format!("{{\"sessionUpdate\":\"user_message\",\"messageId\":{},\"content\":{}}}", esc(&msg_id), content),
            "agentMessage" => {
                if sess.ver == 2 {
                    format!("{{\"sessionUpdate\":\"agent_message_chunk\",\"messageId\":{},\"content\":{{\"type\":\"text\",\"text\":{}}}}}", esc(&msg_id), esc(text))
                } else {
                    format!("{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}", esc(text))
                }
            }
            _ => continue,
        };
        acp::send_raw(stdout, &format!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{}}}}}", esc(&sess.acp_sid), update));
    }
}

fn mime_for(path: &str) -> &'static str {
    let p = path.to_lowercase();
    if p.ends_with(".png") {
        "image/png"
    } else if p.ends_with(".jpg") || p.ends_with(".jpeg") {
        "image/jpeg"
    } else if p.ends_with(".gif") {
        "image/gif"
    } else if p.ends_with(".webp") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

fn image_part(data_b64: &str, mime: &str) -> String {
    format!("{{\"type\":\"image\",\"base64Data\":{},\"mediaType\":{}}}", esc(data_b64), esc(mime))
}

/// Build MSP turn input parts: text (+ inlined resource text) and images.
/// Image sources: inline base64 `data`, or a local `file://`/`/` path which
/// is read and encoded here (same machine). Audio has no host surface
/// (TurnInputPartType is closed: text|image) and is rejected.
fn extract_prompt_parts(params: Option<&J>) -> Result<Vec<String>, String> {
    let p = params.ok_or("session/prompt requires params")?;
    let prompt = p.get("prompt").unwrap_or(p);
    let blocks: Vec<J> = match prompt {
        J::Arr(b) => b.clone(),
        J::Str(s) => vec![J::Str(s.clone())],
        J::Obj(_) => match p.get("prompt") {
            Some(J::Arr(b)) => b.clone(),
            Some(J::Str(s)) => vec![J::Str(s.clone())],
            _ => return Err("session/prompt requires a prompt array".to_string()),
        },
        _ => return Err("session/prompt requires a prompt array".to_string()),
    };
    let mut texts = Vec::new();
    let mut parts = Vec::new();
    let flush_text = |texts: &mut Vec<String>, parts: &mut Vec<String>| {
        if texts.is_empty() {
            return;
        }
        parts.push(format!("{{\"type\":\"text\",\"text\":{}}}", esc(&texts.join("\n"))));
        texts.clear();
    };
    for b in &blocks {
        match b {
            J::Str(s) => texts.push(s.clone()),
            J::Obj(_) => {
                let t = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "text" => {
                        if let Some(x) = b.get("text").and_then(|v| v.as_str()) {
                            texts.push(x.to_string());
                        }
                    }
                    "resource" => {
                        if let Some(r) = b.get("resource") {
                            if let Some(x) = r.get("text").and_then(|v| v.as_str()) {
                                texts.push(x.to_string());
                            } else if let Some(uri) = r.get("uri").and_then(|v| v.as_str()) {
                                texts.push(format!("[resource: {uri}]"));
                            }
                        }
                    }
                    "image" => {
                        flush_text(&mut texts, &mut parts);
                        if let Some(d) = b.get("data").and_then(|v| v.as_str()) {
                            let mime = b.get("mimeType").and_then(|v| v.as_str()).unwrap_or("image/png");
                            parts.push(image_part(d, mime));
                        } else if let Some(uri) = b.get("uri").and_then(|v| v.as_str()) {
                            let path = uri.strip_prefix("file://").unwrap_or(uri);
                            if path.starts_with("http://") || path.starts_with("https://") {
                                return Err("remote image URIs are not supported; send base64 data".to_string());
                            }
                            let bytes = std::fs::read(path).map_err(|e| format!("cannot read image {path}: {e}"))?;
                            parts.push(image_part(&json::b64(&bytes), mime_for(path)));
                        } else {
                            return Err("image block needs data or uri".to_string());
                        }
                    }
                    "audio" => return Err("audio blocks are not supported: the host input type is closed (text|image)".to_string()),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    flush_text(&mut texts, &mut parts);
    Ok(parts)
}

// ---------------------------------------------------------------------------
// MSP -> ACP event routing (main thread; the serve reader only forwards)
// ---------------------------------------------------------------------------

fn find_acp_sid(sessions: &Sessions, msp_sid: &str) -> Option<String> {
    sessions.lock().unwrap().iter().find(|(_, s)| s.msp_sid == msp_sid).map(|(k, _)| k.clone())
}

fn handle_msp(host: &Arc<MspHost>, stdout: &StdoutShared, sessions: &Sessions, method: &str, params: &J) {
    match method {
        "item/started" | "item/updated" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            let item = params.get("item").cloned().unwrap_or(J::Null);
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid) {
                let mut out = Vec::new();
                if let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid) {
                    s.fold.on_item_snapshot(&acp_sid, s.ver, &item, &mut out);
                }
                for line in out {
                    acp::send_raw(stdout, &line);
                }
            }
        }
        "item/delta" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid) {
                let mut out = Vec::new();
                if let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid) {
                    s.fold.on_item_delta(&acp_sid, s.ver, params, &mut out);
                }
                for line in out {
                    acp::send_raw(stdout, &line);
                }
            }
        }
        "item/completed" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid) {
                let mut out = Vec::new();
                if let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid) {
                    s.fold.on_item_completed(&acp_sid, s.ver, params, &mut out);
                }
                for line in out {
                    acp::send_raw(stdout, &line);
                }
            }
        }
        "turn/completed" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            let turn_id = params.get("turnId").and_then(|v| v.as_str()).unwrap_or("");
            let terminal = params.get("terminal").and_then(|v| v.as_str()).unwrap_or("");
            log(&format!("turn/completed turn={turn_id} terminal={terminal}"));
            let acp_sid = match find_acp_sid(sessions, msp_sid) {
                Some(s) => s,
                None => return,
            };
            let req = sessions.lock().unwrap().get_mut(&acp_sid).and_then(|s| {
                let pos = s.in_flight.iter().position(|f| f.msp_turn == turn_id)?;
                let ver = s.ver;
                Some((s.in_flight.remove(pos).req_id, ver))
            });
            if let Some((req_id, ver)) = req {
                match fold::stop_reason(terminal) {
                    Some(stop) => {
                        if ver == 2 {
                            acp::send_state(stdout, &acp_sid, "idle", Some(stop));
                        } else {
                            acp::send_result(stdout, &Some(req_id), &format!("{{\"stopReason\":\"{stop}\"}}"));
                        }
                    }
                    None => {
                        // e.g. terminal "failed": no clean stop reason vocabulary.
                        if ver == 2 {
                            acp::send_state(stdout, &acp_sid, "idle", None);
                            log(&format!("turn {turn_id} ended with terminal '{terminal}'"));
                        } else {
                            acp::send_error(stdout, &Some(req_id), -32603, &format!("turn ended with terminal '{terminal}'"));
                        }
                    }
                }
            }
        }
        "approval/requested" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            let acp_sid = match find_acp_sid(sessions, msp_sid) {
                Some(s) => s,
                None => return,
            };
            let approval_id = params.get("approvalId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let requirement = params.get("currentRequirementId").cloned().unwrap_or(J::Null);
            let tool_call_id = params.get("toolCallId").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            let (options_json, choices) = acp::perm_options(params);
            let req_id = J::Str(mint_id("perm-", &ID_COUNTER));
            {
                let mut map = sessions.lock().unwrap();
                if let Some(s) = map.get_mut(&acp_sid) {
                    s.pending_perm = Some(PendingPerm { req_id: req_id.clone(), approval_id, requirement, choices });
                    if s.ver == 2 {
                        acp::send_state(stdout, &acp_sid, "requires_action", None);
                    }
                } else {
                    return;
                }
            }
            acp::send_raw(stdout, &format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"session/request_permission\",\"params\":{{\"sessionId\":{},\"toolCall\":{{\"toolCallId\":{}}},\"options\":{}}}}}",
                j_to_string(&req_id),
                esc(&acp_sid),
                esc(&tool_call_id),
                options_json
            ));
        }
        "approval/resolved" | "approval/updated" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            log(&format!("{} for {}", method, if msp_sid.is_empty() { "?" } else { msp_sid }));
        }
        "userInput/requested" => {
            let msp_sid = params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            let acp_sid = find_acp_sid(sessions, msp_sid);
            // Bridge to ACP elicitation when the client advertised form mode;
            // otherwise cancel so the turn proceeds instead of hanging.
            let bridged = match (&acp_sid, ELICIT_FORM.load(Ordering::SeqCst)) {
                (Some(sid), 1) => {
                    let ver = sessions.lock().unwrap().get(sid).map(|s| s.ver).unwrap_or(1);
                    ver == 2 && bridge_user_input(host, stdout, sessions, sid, params)
                }
                _ => false,
            };
            if !bridged {
                let qid = params.get("userInputId").and_then(|v| v.as_str()).unwrap_or("");
                log(&format!("userInput/requested not bridged (elicit_form={}); falling back", ELICIT_FORM.load(Ordering::SeqCst)));
                if !msp_sid.is_empty() && !qid.is_empty() {
                    let cmd = host.mint_cmd("cmd-");
                    let _ = host.command("userInput/cancel", &format!("{{\"commandId\":{},\"sessionId\":{},\"userInputId\":{}}}", esc(&cmd), esc(&msp_sid), esc(qid)));
                    log(&format!("userInput {qid} auto-cancelled (client has no elicitation form)"));
                }
            }
        }
        "userInput/settled" => {}
        "turn/started" | "turn/unqueued" | "turn/retracted" | "turn/retryScheduled" => {
            log(&format!(
                "{method} turn={} sess={}",
                params.get("turnId").and_then(|v| v.as_str()).unwrap_or("?"),
                params.get("sessionId").and_then(|v| v.as_str()).unwrap_or("?")
            ));
        }
        "initialized" | "view/gap" | "session/started" | "session/contextUsage" | "session/tokenUsage"
        | "session/goalChanged" | "session/todoListChanged" | "session/approvalModeChanged"
        | "session/branchChanged" | "session/modelChanged" => {}
        _ => {
            log(&format!("unhandled MSP notification: {method}"));
        }
    }
}

/// Client reply to our `session/request_permission` (matched by id).
fn complete_permission(host: &Arc<MspHost>, stdout: &StdoutShared, sessions: &Sessions, id: &Option<J>, msg: &J) {
    let idv = match id {
        Some(v) => v.clone(),
        None => return,
    };
    // Locate the session holding this pending permission.
    let found = sessions.lock().unwrap().iter().find_map(|(k, s)| {
        match &s.pending_perm {
            Some(p) if j_to_string(&p.req_id) == j_to_string(&idv) => Some(k.clone()),
            _ => None,
        }
    });
    let acp_sid = match found {
        Some(s) => s,
        None => return, // not ours; ignore (e.g. late duplicate)
    };
    let (msp_sid, ver, approval_id, requirement, choices) = {
        let mut map = sessions.lock().unwrap();
        let s = match map.get_mut(&acp_sid) {
            Some(s) => s,
            None => return,
        };
        let p = match s.pending_perm.take() {
            Some(p) => p,
            None => return,
        };
        (s.msp_sid.clone(), s.ver, p.approval_id, p.requirement, p.choices)
    };
    // Outcome -> choiceId. Deny-safe fallback when the client errors or the
    // outcome is unrecognized: pick the first reject-ish choice.
    let choice = if msg.get("error").is_some() {
        log("session/request_permission failed at client; denying");
        acp::fallback_deny(&choices)
    } else {
        match msg.get("result").and_then(|r| r.get("outcome")) {
            Some(o) => match o.get("outcome").and_then(|v| v.as_str()).unwrap_or("") {
                "selected" => o
                    .get("optionId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| acp::fallback_deny(&choices)),
                _ => acp::fallback_deny(&choices),
            },
            None => acp::fallback_deny(&choices),
        }
    };
    let choice = match choice {
        Some(c) => c,
        None => {
            log("permission: no choices to decide with; leaving pending");
            return;
        }
    };
    let cmd = host.mint_cmd("cmd-");
    match host.command(
        "approval/decide",
        &format!(
            "{{\"commandId\":{},\"sessionId\":{},\"approvalId\":{},\"requirementId\":{},\"choiceId\":{}}}",
            esc(&cmd),
            esc(&msp_sid),
            esc(&approval_id),
            j_to_string(&requirement),
            esc(&choice)
        ),
    ) {
        Ok(_) => {
            if ver == 2 {
                acp::send_state(stdout, &acp_sid, "running", None);
            }
        }
        Err(e) => log(&format!("approval/decide failed: {}", err_message(&e))),
    }
}

fn fail_all(stdout: &StdoutShared, sessions: &Sessions) {
    let mut map = sessions.lock().unwrap();
    for s in map.values_mut() {
        for f in s.in_flight.drain(..) {
            if s.ver == 2 {
                acp::send_state(stdout, &s.acp_sid, "idle", Some("cancelled"));
            } else {
                acp::send_result(stdout, &Some(f.req_id), "{\"stopReason\":\"cancelled\"}");
            }
        }
    }
}

/// Bridge an MSP `userInput/requested` to ACP `elicitation/create` (form mode).
/// Returns false when there is nothing bridgeable (caller falls back).
fn bridge_user_input(
    _host: &Arc<MspHost>,
    stdout: &StdoutShared,
    sessions: &Sessions,
    acp_sid: &str,
    params: &J,
) -> bool {
    let user_input_id = params.get("userInputId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let tool_call = params.get("toolCallId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let questions = match params.get("questions") {
        Some(J::Arr(q)) => q.clone(),
        _ => return false,
    };
    if user_input_id.is_empty() || questions.is_empty() {
        return false;
    }
    let mut props = Vec::new();
    let mut required = Vec::new();
    let mut msg = Vec::new();
    let mut ui_qs = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        let qid = q.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let header = q.get("header").and_then(|v| v.as_str()).unwrap_or("");
        let text = q.get("question").and_then(|v| v.as_str()).unwrap_or("");
        let labels: Vec<String> = match q.get("options") {
            Some(J::Arr(o)) => o.iter().filter_map(|x| x.get("label")).filter_map(|v| v.as_str()).map(|s| s.to_string()).collect(),
            _ => Vec::new(),
        };
        if qid.is_empty() || labels.is_empty() {
            continue;
        }
        let single = q.get("selection").and_then(|s| s.get("mode")).and_then(|v| v.as_str()).unwrap_or("single") == "single";
        let min = q.get("selection").and_then(|s| s.get("minSelections")).and_then(|v| v.as_u64()).unwrap_or(1);
        let key = format!("q{i}");
        let en: Vec<String> = labels.iter().map(|l| esc(l)).collect();
        if single {
            props.push(format!("{}: {{\"type\":\"string\",\"enum\":[{}]}}", esc(&key), en.join(",")));
        } else {
            props.push(format!("{}: {{\"type\":\"array\",\"items\":{{\"type\":\"string\",\"enum\":[{}]}}}}", esc(&key), en.join(",")));
        }
        if single || min > 0 {
            required.push(format!("{key}"));
        }
        msg.push(format!("{header}: {text}"));
        ui_qs.push(acp::UiQuestion { qid, labels });
    }
    if ui_qs.is_empty() {
        return false;
    }
    let schema = format!(
        "{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}]}}",
        props.join(","),
        required.iter().map(|k| esc(k)).collect::<Vec<_>>().join(",")
    );
    let req_id = J::Str(mint_id("elic-", &ID_COUNTER));
    if let Some(s) = sessions.lock().unwrap().get_mut(acp_sid) {
        s.pending_ui = Some(acp::PendingUi { req_id: req_id.clone(), user_input_id: user_input_id.clone(), questions: ui_qs });
        log(&format!("bridging userInput {user_input_id} to elicitation {}", j_to_string(&req_id)));
    } else {
        return false;
    }
    let tool_f = if tool_call.is_empty() { String::new() } else { format!(",\"toolCallId\":{}", esc(&tool_call)) };
    acp::send_raw(stdout, &format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"elicitation/create\",\"params\":{{\"sessionId\":{}{},\"mode\":\"form\",\"message\":{},\"requestedSchema\":{}}}}}",
        j_to_string(&req_id),
        esc(acp_sid),
        tool_f,
        esc(&msg.join("\n")),
        schema
    ));
    true
}

/// Client reply to our `elicitation/create` (matched by id).
fn complete_elicitation(host: &Arc<MspHost>, stdout: &StdoutShared, sessions: &Sessions, id: &Option<J>, msg: &J) {
    let _ = stdout;
    let idv = match id {
        Some(v) => v.clone(),
        None => return,
    };
    let found = sessions.lock().unwrap().iter().find_map(|(k, s)| {
        match &s.pending_ui {
            Some(p) if j_to_string(&p.req_id) == j_to_string(&idv) => Some(k.clone()),
            _ => None,
        }
    });
    let acp_sid = match found {
        Some(s) => s,
        None => return,
    };
    let (msp_sid, user_input_id, questions) = {
        let mut map = sessions.lock().unwrap();
        let s = match map.get_mut(&acp_sid) {
            Some(s) => s,
            None => return,
        };
        let p = match s.pending_ui.take() {
            Some(p) => p,
            None => return,
        };
        (s.msp_sid.clone(), p.user_input_id, p.questions)
    };
    let cmd = host.mint_cmd("cmd-");
    // accept + content -> answers; anything else -> cancel the question.
    let mut answers: Option<String> = None;
    if msg.get("error").is_none() {
        if let Some(res) = msg.get("result") {
            if res.get("action").and_then(|v| v.as_str()).unwrap_or("") == "accept" {
                let content = res.get("content").cloned().unwrap_or(J::Null);
                let mut parts = Vec::new();
                for (i, q) in questions.iter().enumerate() {
                    let key = format!("q{i}");
                    match content.get(key.as_str()) {
                        Some(J::Str(v)) => {
                            if q.labels.iter().any(|l| l == v) {
                                parts.push(format!("{{\"questionId\":{},\"selectedLabel\":{}}}", esc(&q.qid), esc(v)));
                            } else {
                                parts.push(format!("{{\"questionId\":{},\"freeText\":{}}}", esc(&q.qid), esc(v)));
                            }
                        }
                        Some(J::Arr(vs)) => {
                            let mut matched = Vec::new();
                            let mut free = Vec::new();
                            for v in vs {
                                match v.as_str() {
                                    Some(s) if q.labels.iter().any(|l| l == s) => matched.push(esc(s)),
                                    Some(s) => free.push(s.to_string()),
                                    None => {}
                                }
                            }
                            let mut f = vec![format!("\"questionId\":{}", esc(&q.qid))];
                            if !matched.is_empty() {
                                f.push(format!("\"selectedLabels\":[{}]", matched.join(",")));
                            }
                            if !free.is_empty() {
                                f.push(format!("\"freeText\":{}", esc(&free.join(", "))));
                            }
                            parts.push(format!("{{{}}}", f.join(",")));
                        }
                        _ => {}
                    }
                }
                answers = Some(format!("[{}]", parts.join(",")));
            }
        }
    }
    match answers {
        Some(a) => {
            let _ = host.command(
                "userInput/answer",
                &format!("{{\"commandId\":{},\"sessionId\":{},\"userInputId\":{},\"answers\":{}}}", esc(&cmd), esc(&msp_sid), esc(&user_input_id), a),
            );
        }
        None => {
            let _ = host.command(
                "userInput/cancel",
                &format!("{{\"commandId\":{},\"sessionId\":{},\"userInputId\":{}}}", esc(&cmd), esc(&msp_sid), esc(&user_input_id)),
            );
            log("elicitation declined/cancelled/failed; question cancelled");
        }
    }
}
