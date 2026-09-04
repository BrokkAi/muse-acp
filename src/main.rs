//! muse-acp: ACP (v2 primary, v1 fallback) server backed by one `muse serve` host.
//!
//! ACP client <-> stdio NDJSON <-> this adapter <-> stdio NDJSON <-> serve host.
//! One host serves all ACP sessions; `session/start` auto-subscribes us to its
//! view, so turns stream in as `item/*` + `turn/*` notifications.

mod acp;
mod fold;
mod json;
mod msp;
mod zed;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

use acp::{AcpSession, InFlight, PendingPerm, Sessions, StdoutShared};
use fold::SessionFold;
use json::{J, esc, j_to_string, mint_id, parse_json};
use msp::{MspEvent, MspHost, err_code, err_message, log};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static VER: AtomicU64 = AtomicU64::new(0); // negotiated ACP version for the connection
static ELICIT_FORM: AtomicU64 = AtomicU64::new(0); // 1 when the v2 client advertises elicitation.form
/// Cached model catalog: (modelId, displayLabel, isDefault).
static CATALOG: std::sync::OnceLock<Mutex<Vec<(String, String, bool)>>> =
    std::sync::OnceLock::new();

/// Folded host approval mode: `session.approvalMode.mode`
/// (EffectiveApprovalModeState; additive-optional, may be absent).
fn host_mode(res: &J) -> Option<String> {
    res.get("session")?
        .get("approvalMode")?
        .get("mode")?
        .as_str()
        .map(|s| s.to_string())
}

fn catalog(host: &Arc<MspHost>) -> Vec<(String, String, bool)> {
    let cell = CATALOG.get_or_init(|| Mutex::new(Vec::new()));
    {
        let guard = cell.lock().unwrap();
        if !guard.is_empty() {
            return guard.clone();
        }
    }
    let mut out = Vec::new();
    if let Ok(r) = host.command(
        "model/list",
        &format!("{{\"commandId\":{}}}", esc(&host.mint_cmd("cmd-"))),
    ) && let Some(J::Arr(models)) = r.get("models")
    {
        for m in models {
            let id = m
                .get("modelId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let label = m
                .get("displayLabel")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let def = matches!(m.get("isDefault"), Some(J::Bool(true)));
            out.push((id, label, def));
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

const V2_INIT: &str = r#"{"protocolVersion":2,"capabilities":{"session":{"prompt":{"image":{},"embeddedContext":{}}}},"info":{"name":"muse-acp","title":"Muse ACP","version":"0.2.0"},"authMethods":[],"_meta":{"steering":{"supported":true}}}"#;
const V1_INIT: &str = r#"{"protocolVersion":1,"agentCapabilities":{"promptCapabilities":{"text":true,"image":true,"audio":false,"embeddedContext":true},"mcpCapabilities":{"http":false,"sse":false},"loadSession":true,"sessionCapabilities":{"list":{},"resume":{},"close":{}}},"agentInfo":{"name":"muse-acp","title":"Muse ACP","version":"0.2.0"}}"#;

fn has_nonempty_array(params: Option<&J>, key: &str) -> bool {
    matches!(
        params.and_then(|p| p.get(key)),
        Some(J::Arr(values)) if !values.is_empty()
    )
}

fn validate_session_roots(stdout: &StdoutShared, id: &Option<J>, params: Option<&J>) -> bool {
    let cwd = params
        .and_then(|p| p.get("cwd"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if cwd.is_empty() || !Path::new(cwd).is_absolute() {
        acp::send_error(stdout, id, -32602, "params.cwd must be an absolute path");
        return false;
    }
    if has_nonempty_array(params, "mcpServers") {
        acp::send_error(stdout, id, -32602, "MCP servers are not supported");
        return false;
    }
    if has_nonempty_array(params, "additionalDirectories") {
        acp::send_error(
            stdout,
            id,
            -32602,
            "additional directories are not supported",
        );
        return false;
    }
    true
}

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--selftest"] {
        std::process::exit(selftest());
    }
    if let Some(exit_code) = zed::dispatch(&args) {
        std::process::exit(exit_code);
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
            LoopMsg::Msp(MspEvent::Request { method, params }) => {
                // Reissued server requests (multi-stage approvals, resumed
                // questions) carry their own payloads: bridge them too.
                match method.as_str() {
                    "approval/request" => open_approval(&stdout, &sessions, &params),
                    "userInput/request" => {
                        handle_msp(&host, &stdout, &sessions, "userInput/requested", &params);
                    }
                    _ => log(&format!("unhandled MSP request: {method}")),
                }
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

fn send_v2_user_message(stdout: &StdoutShared, sid: &str, content: &str) {
    let msg_id = mint_id("msg-", &ID_COUNTER);
    acp::send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"user_message\",\"messageId\":{},\"content\":{}}}}}}}",
            esc(sid),
            esc(&msg_id),
            content
        ),
    );
}

fn steering_prompt_required(params: Option<&J>) -> Result<bool, String> {
    let Some(meta) = params.and_then(|p| p.get("_meta")) else {
        return Ok(false);
    };
    if matches!(meta, J::Null) {
        return Ok(false);
    }
    if !matches!(meta, J::Obj(_)) {
        return Err("steering _meta must be an object".to_string());
    }
    let Some(steering) = meta.get("steering") else {
        return Ok(false);
    };
    if !matches!(steering, J::Obj(_)) {
        return Err("steering _meta.steering must be an object".to_string());
    }
    match steering.get("idleBehavior") {
        None | Some(J::Null) => Ok(false),
        Some(J::Str(value)) if value == "promptRequired" => Ok(true),
        Some(J::Str(_)) => Err("unsupported steering idleBehavior".to_string()),
        Some(_) => Err("steering idleBehavior must be a string".to_string()),
    }
}

fn handle_acp(host: &Arc<MspHost>, stdout: &StdoutShared, sessions: &Sessions, msg: &J) {
    let method = msg
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
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
                .is_some_and(|f| matches!(f, J::Obj(_)));
            ELICIT_FORM.store(u64::from(v == 2 && form), Ordering::SeqCst);
            if v == 2 {
                acp::send_result(stdout, &id, V2_INIT);
            } else {
                acp::send_result(stdout, &id, V1_INIT);
            }
        }
        "session/new" => {
            let ver = negotiated_ver();
            if !validate_session_roots(stdout, &id, params.as_ref()) {
                return;
            }
            let cwd = params
                .as_ref()
                .and_then(|p| p.get("cwd"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Optional approval posture, applied atomically at start: an
            // operator-specified posture must not silently fall back.
            let mode_env = std::env::var("MUSE_APPROVAL_MODE").unwrap_or_default();
            let resolved_env = acp::resolve_mode(mode_env.trim());
            let start_mode = if mode_env.trim().is_empty() {
                String::new()
            } else {
                match resolved_env {
                    Some(m) => format!(",\"approvalMode\":{}", esc(m)),
                    None => {
                        acp::send_error(
                            stdout,
                            &id,
                            -32602,
                            "MUSE_APPROVAL_MODE must be ask|auto|deny or a host mode",
                        );
                        return;
                    }
                }
            };
            let cmd = host.mint_cmd("cmd-");
            let res = host.command(
                "session/start",
                &format!(
                    "{{\"commandId\":{},\"workspaceRoot\":{}{}}}",
                    esc(&cmd),
                    esc(&cwd),
                    start_mode
                ),
            );
            match res {
                Ok(r) => {
                    let msp_sid = match r
                        .get("session")
                        .and_then(|s| s.get("sessionId"))
                        .and_then(|v| v.as_str())
                    {
                        Some(s) => s.to_string(),
                        None => {
                            acp::send_error(
                                stdout,
                                &id,
                                -32603,
                                "session/start returned no session.sessionId",
                            );
                            return;
                        }
                    };
                    // The host reports the folded mode in
                    // session.approvalMode.mode; without an explicit request
                    // we adopt the host default, with one we require a match.
                    let mut applied_mode =
                        host_mode(&r).unwrap_or_else(|| "promptUnmatched".to_string());
                    if !start_mode.is_empty() {
                        match (resolved_env, host_mode(&r)) {
                            (Some(want), Some(got)) if got.as_str() == want => {
                                applied_mode = got;
                            }
                            (Some(_), Some(got)) => {
                                acp::send_error(
                                    stdout,
                                    &id,
                                    -32603,
                                    &format!(
                                        "requested approval mode was not applied (host reports {got})"
                                    ),
                                );
                                return;
                            }
                            (Some(want), None) => {
                                applied_mode = want.to_string();
                            }
                            (None, _) => {}
                        }
                    }
                    let sid = mint_id("sess-", &ID_COUNTER);
                    let cur_mode = applied_mode;
                    let cur_model = r
                        .get("session")
                        .and_then(|s| s.get("modelId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let cur_cursor = r
                        .get("viewCursor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let active_turn = r
                        .get("session")
                        .and_then(|s| s.get("activeTurnId"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    sessions.lock().unwrap().insert(
                        sid.clone(),
                        AcpSession {
                            acp_sid: sid.clone(),
                            msp_sid: msp_sid.clone(),
                            cwd: cwd.clone(),
                            ver,
                            in_flight: Vec::new(),
                            pending_perm: None,
                            pending_ui: Vec::new(),
                            mode_value: acp::mode_from_msp(&cur_mode).to_string(),
                            model_value: cur_model.clone(),
                            reasoning_effort: "medium".to_string(),
                            active_turn,
                            view_cursor: cur_cursor.clone(),
                            fold: SessionFold::new(),
                        },
                    );
                    // _meta exposes the host session id: pass it back to
                    // session/resume to reconnect after an adapter restart.
                    // Config selectors are standard in v1 and v2; v1 also gets
                    // the legacy mode state for older clients.
                    let result = if ver == 2 {
                        let models = catalog(host);
                        format!(
                            "{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}},\"configOptions\":{}}}",
                            esc(&sid),
                            esc(&msp_sid),
                            acp::config_options(
                                ver,
                                acp::mode_from_msp(&cur_mode),
                                &cur_model,
                                "medium",
                                &models,
                            )
                        )
                    } else {
                        format!(
                            "{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}},\"configOptions\":{},\"modes\":{}}}",
                            esc(&sid),
                            esc(&msp_sid),
                            acp::config_options(
                                ver,
                                acp::mode_from_msp(&cur_mode),
                                &cur_model,
                                "medium",
                                &catalog(host),
                            ),
                            acp::session_modes(acp::mode_from_msp(&cur_mode))
                        )
                    };
                    acp::send_result(stdout, &id, &result);
                    acp::send_available_commands(stdout, &sid, ver);
                }
                Err(e) => acp::send_error(
                    stdout,
                    &id,
                    -32603,
                    &format!("session/start failed: {}", err_message(&e)),
                ),
            }
        }
        "session/resume" | "session/load" => {
            let ver = negotiated_ver();
            if !validate_session_roots(stdout, &id, params.as_ref()) {
                return;
            }
            let resume_cwd = params
                .as_ref()
                .and_then(|p| p.get("cwd"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sid.is_empty() {
                acp::send_error(
                    stdout,
                    &id,
                    -32602,
                    "session resume requires params.sessionId",
                );
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
            // Ask for inline history explicitly; the host may still downgrade
            // (history.mode reports what was served).
            match host.command(
                "session/resume",
                &format!(
                    "{{\"commandId\":{},\"sessionId\":{},\"history\":\"inline\"}}",
                    esc(&cmd),
                    esc(&msp_sid)
                ),
            ) {
                Ok(r) => {
                    // Pending questions/approvals survive reconnects; the host
                    // re-issues their requests, which the normal bridge picks
                    // up. Log them so a stuck-looking turn is diagnosable.
                    if let Some(J::Arr(pend)) = r.get("pendingRequests") {
                        for p in pend {
                            log(&format!("resume: pending request {}", j_to_string(p)));
                        }
                    }
                    if let Some(h) = r.get("history") {
                        let mode = h.get("mode").and_then(|v| v.as_str()).unwrap_or("?");
                        if mode != "inline" {
                            log(&format!(
                                "resume: history downgraded to {mode}; replay may be partial"
                            ));
                        }
                    }
                    let real_msp = r
                        .get("session")
                        .and_then(|s| s.get("sessionId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&msp_sid)
                        .to_string();
                    let real_model = r
                        .get("session")
                        .and_then(|s| s.get("modelId"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // v1 session/load always replays; v2 resumes replay only
                    // with replayFrom; v1 session/resume reconnects silently.
                    let replay = method == "session/load"
                        || (method == "session/resume"
                            && ver == 2
                            && params.as_ref().and_then(|p| p.get("replayFrom")).is_some());
                    {
                        let mut map = sessions.lock().unwrap();
                        let entry = map.entry(sid.clone()).or_insert_with(|| AcpSession {
                            acp_sid: sid.clone(),
                            msp_sid: real_msp.clone(),
                            cwd: resume_cwd.clone(),
                            ver,
                            in_flight: Vec::new(),
                            pending_perm: None,
                            pending_ui: Vec::new(),
                            mode_value: "ask".to_string(),
                            model_value: String::new(),
                            reasoning_effort: "medium".to_string(),
                            active_turn: None,
                            view_cursor: String::new(),
                            fold: SessionFold::new(),
                        });
                        entry.msp_sid = real_msp;
                        entry.ver = ver;
                        if !resume_cwd.is_empty() {
                            entry.cwd = resume_cwd;
                        }
                        if !real_model.is_empty() {
                            entry.model_value = real_model;
                        }
                        entry.active_turn = r
                            .get("session")
                            .and_then(|s| s.get("activeTurnId"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        // Refresh the mode selector from the folded host
                        // mode so resumed clients are not stuck stale.
                        if let Some(m) = host_mode(&r) {
                            entry.mode_value = acp::mode_from_msp(&m).to_string();
                        }
                        if replay {
                            replay_history(stdout, entry, &r);
                        }
                    }
                    let msp_out = sessions
                        .lock()
                        .unwrap()
                        .get(&sid)
                        .map(|s| s.msp_sid.clone())
                        .unwrap_or_default();
                    let (mode_v, model_v, reasoning_v) = sessions
                        .lock()
                        .unwrap()
                        .get(&sid)
                        .map(|s| {
                            (
                                s.mode_value.clone(),
                                s.model_value.clone(),
                                s.reasoning_effort.clone(),
                            )
                        })
                        .unwrap_or_default();
                    // Both versions report current selectors; v1 also keeps the
                    // legacy mode state for clients which predate config options.
                    let result = if ver == 2 {
                        let models = catalog(host);
                        format!(
                            "{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}},\"configOptions\":{}}}",
                            esc(&sid),
                            esc(&msp_out),
                            acp::config_options(ver, &mode_v, &model_v, &reasoning_v, &models)
                        )
                    } else {
                        let models = catalog(host);
                        format!(
                            "{{\"sessionId\":{},\"_meta\":{{\"mspSessionId\":{}}},\"configOptions\":{},\"modes\":{}}}",
                            esc(&sid),
                            esc(&msp_out),
                            acp::config_options(ver, &mode_v, &model_v, &reasoning_v, &models),
                            acp::session_modes(&mode_v)
                        )
                    };
                    acp::send_result(stdout, &id, &result);
                    acp::send_available_commands(stdout, &sid, ver);
                }
                Err(e) => acp::send_error(
                    stdout,
                    &id,
                    -32602,
                    &format!("resume failed: {}", err_message(&e)),
                ),
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
            let (msp_sid, cwd, reasoning_effort) = match sessions.lock().unwrap().get(&sid) {
                Some(s) => (s.msp_sid.clone(), s.cwd.clone(), s.reasoning_effort.clone()),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            let (parts, acp_content) = match extract_prompt_parts(params.as_ref(), &cwd) {
                Ok((p, c)) if !p.is_empty() => (p, c),
                Ok(_) => {
                    acp::send_error(stdout, &id, -32602, "session/prompt requires content");
                    return;
                }
                Err(e) => {
                    acp::send_error(stdout, &id, -32602, &e);
                    return;
                }
            };
            // The host queues concurrent turns itself (ifBusy defaults to
            // queue); track every in-flight turn so each completes its own
            // prompt response.
            let cmd = host.mint_cmd("cmd-");
            let input = format!("[{}]", parts.join(","));
            match host.command(
                "turn/start",
                &format!(
                    "{{\"commandId\":{},\"sessionId\":{},\"input\":{},\"reasoningEffort\":{}}}",
                    esc(&cmd),
                    esc(&msp_sid),
                    input,
                    esc(&reasoning_effort)
                ),
            ) {
                Ok(r) => {
                    let turn = r
                        .get("turnId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if turn.is_empty() {
                        acp::send_error(stdout, &id, -32603, "turn/start returned no turnId");
                        return;
                    }
                    let started = r
                        .get("disposition")
                        .and_then(|v| v.as_str())
                        .is_none_or(|value| value == "started");
                    if let Some(s) = sessions.lock().unwrap().get_mut(&sid) {
                        s.in_flight.push(InFlight {
                            msp_turn: turn.clone(),
                            req_id: id.clone().unwrap_or(J::Null),
                        });
                        if started {
                            s.active_turn = Some(turn);
                        }
                    }
                    if ver == 2 {
                        // Accepted: empty response, then the user-message echo
                        // (v2 MUST), then running.
                        acp::send_result(stdout, &id, "{}");
                        send_v2_user_message(stdout, &sid, &acp_content);
                        acp::send_state(stdout, &sid, "running", None);
                    } else {
                        // v1 prompt flow echoes user content as chunks.
                        let msg_id = mint_id("msg-", &ID_COUNTER);
                        if let Ok(J::Arr(blocks)) = parse_json(&acp_content) {
                            for b in blocks {
                                acp::send_raw(
                                    stdout,
                                    &format!(
                                        "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"user_message_chunk\",\"messageId\":{},\"content\":{}}}}}}}",
                                        esc(&sid),
                                        esc(&msg_id),
                                        j_to_string(&b)
                                    ),
                                );
                            }
                        }
                    }
                    // v1: the prompt response arrives with the terminal.
                }
                Err(e) => {
                    let code = err_code(&e);
                    if code == -32000 || err_message(&e).contains("already_terminal") {
                        acp::send_error(
                            stdout,
                            &id,
                            -32603,
                            &format!("turn rejected: {}", err_message(&e)),
                        );
                    } else {
                        acp::send_error(
                            stdout,
                            &id,
                            -32603,
                            &format!("turn/start failed: {}", err_message(&e)),
                        );
                    }
                }
            }
        }
        "_session/steering" => {
            if negotiated_ver() != 2 {
                acp::send_error(stdout, &id, -32601, "steering requires ACP v2");
                return;
            }
            let prompt_required = match steering_prompt_required(params.as_ref()) {
                Ok(value) => value,
                Err(message) => {
                    acp::send_error(stdout, &id, -32602, &message);
                    return;
                }
            };
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let (msp_sid, cwd, reasoning_effort, active_turn) =
                match sessions.lock().unwrap().get(&sid) {
                    Some(s) => (
                        s.msp_sid.clone(),
                        s.cwd.clone(),
                        s.reasoning_effort.clone(),
                        s.active_turn.clone(),
                    ),
                    None => {
                        acp::send_error(stdout, &id, -32602, "unknown sessionId");
                        return;
                    }
                };
            let (parts, acp_content) = match extract_prompt_parts(params.as_ref(), &cwd) {
                Ok((parts, content)) if !parts.is_empty() => (parts, content),
                Ok(_) => {
                    acp::send_error(stdout, &id, -32602, "steering requires content");
                    return;
                }
                Err(message) => {
                    acp::send_error(stdout, &id, -32602, &message);
                    return;
                }
            };
            if active_turn.is_none() && prompt_required {
                acp::send_result(
                    stdout,
                    &id,
                    "{\"outcome\":\"promptRequired\",\"reason\":\"noRunningTurn\"}",
                );
                return;
            }
            let cmd = host.mint_cmd("cmd-");
            let input = format!("[{}]", parts.join(","));
            let result = match active_turn.as_deref() {
                Some(expected_turn) => host.command(
                    "turn/steer",
                    &format!(
                        "{{\"commandId\":{},\"sessionId\":{},\"expectedTurnId\":{},\"input\":{},\"reasoningEffort\":{}}}",
                        esc(&cmd),
                        esc(&msp_sid),
                        esc(expected_turn),
                        input,
                        esc(&reasoning_effort)
                    ),
                ),
                None => host.command(
                    "turn/start",
                    &format!(
                        "{{\"commandId\":{},\"sessionId\":{},\"input\":{},\"ifBusy\":\"steer\",\"reasoningEffort\":{}}}",
                        esc(&cmd),
                        esc(&msp_sid),
                        input,
                        esc(&reasoning_effort)
                    ),
                ),
            };
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    acp::send_error(
                        stdout,
                        &id,
                        -32603,
                        &format!("steering failed: {}", err_message(&error)),
                    );
                    return;
                }
            };
            let turn = result
                .get("turnId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if turn.is_empty() {
                acp::send_error(stdout, &id, -32603, "steering returned no turnId");
                return;
            }
            let (outcome, started_new) = if let Some(expected) = active_turn.as_deref() {
                if turn != expected {
                    acp::send_error(
                        stdout,
                        &id,
                        -32603,
                        "turn/steer returned a different turnId",
                    );
                    return;
                }
                ("injected", false)
            } else {
                match result.get("disposition").and_then(|v| v.as_str()) {
                    Some("started") => ("startedNewTurn", true),
                    Some("steered") => ("injected", false),
                    Some(other) => {
                        acp::send_error(
                            stdout,
                            &id,
                            -32603,
                            &format!("unexpected steering disposition '{other}'"),
                        );
                        return;
                    }
                    None => {
                        acp::send_error(
                            stdout,
                            &id,
                            -32603,
                            "steering turn/start returned no disposition",
                        );
                        return;
                    }
                }
            };
            if started_new && let Some(s) = sessions.lock().unwrap().get_mut(&sid) {
                s.active_turn = Some(turn.clone());
                s.in_flight.push(InFlight {
                    msp_turn: turn,
                    req_id: J::Null,
                });
            }
            // Acknowledge the extension before emitting the synthetic echo.
            acp::send_result(stdout, &id, &format!("{{\"outcome\":{}}}", esc(outcome)));
            send_v2_user_message(stdout, &sid, &acp_content);
            acp::send_state(stdout, &sid, "running", None);
        }
        "session/close" => {
            // v2 baseline: stop session work, drop local state, resolve
            // pending client interactions as cancelled, return {}.
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sid.is_empty() {
                acp::send_error(
                    stdout,
                    &id,
                    -32602,
                    "session/close requires params.sessionId",
                );
                return;
            }
            let turns = sessions
                .lock()
                .unwrap()
                .get(&sid)
                .map(|s| {
                    let msp = s.msp_sid.clone();
                    s.in_flight
                        .iter()
                        .map(|f| (msp.clone(), f.msp_turn.clone(), f.req_id.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (msp_sid, turn_id, req_id) in &turns {
                let cmd = host.mint_cmd("cmd-");
                let _ = host.command(
                    "turn/cancel",
                    &format!(
                        "{{\"commandId\":{},\"sessionId\":{},\"turnId\":{}}}",
                        esc(&cmd),
                        esc(msp_sid),
                        esc(turn_id)
                    ),
                );
                let _ = req_id;
            }
            let removed = sessions.lock().unwrap().remove(&sid);
            match removed {
                Some(s) => {
                    for f in s.in_flight {
                        if s.ver == 2 {
                            acp::send_state(stdout, &sid, "idle", Some("cancelled"));
                        } else {
                            acp::send_result(
                                stdout,
                                &Some(f.req_id),
                                "{\"stopReason\":\"cancelled\"}",
                            );
                        }
                    }
                    for p in s.pending_ui {
                        acp::send_error(stdout, &Some(p.req_id), -32800, "session closed");
                    }
                    if let Some(p) = s.pending_perm {
                        acp::send_error(stdout, &Some(p.req_id), -32800, "session closed");
                    }
                    acp::send_result(stdout, &id, "{}");
                }
                None => acp::send_error(stdout, &id, -32602, "unknown sessionId"),
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
            let turns = sessions
                .lock()
                .unwrap()
                .get(&sid)
                .map(|s| {
                    let msp = s.msp_sid.clone();
                    s.in_flight
                        .iter()
                        .map(|f| (msp.clone(), f.msp_turn.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            // session/cancel stops all session work: cancel every in-flight turn.
            for (msp_sid, turn_id) in turns {
                let cmd = host.mint_cmd("cmd-");
                match host.command(
                    "turn/cancel",
                    &format!(
                        "{{\"commandId\":{},\"sessionId\":{},\"turnId\":{}}}",
                        esc(&cmd),
                        esc(&msp_sid),
                        esc(&turn_id)
                    ),
                ) {
                    Ok(_) => {}
                    Err(e) => {
                        // already_terminal just means the terminal event is on
                        // its way (or arrived); anything else is real.
                        if !(err_message(&e).contains("already_terminal") || err_code(&e) == -32000)
                        {
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
                    let ids: Vec<String> = map
                        .keys()
                        .map(|k| format!("{{\"sessionId\":{}}}", esc(k)))
                        .collect();
                    drop(map);
                    let _ = r;
                    acp::send_result(
                        stdout,
                        &id,
                        &format!("{{\"sessions\":[{}]}}", ids.join(",")),
                    );
                }
                Err(e) => acp::send_error(
                    stdout,
                    &id,
                    -32603,
                    &format!("session/list failed: {}", err_message(&e)),
                ),
            }
        }
        "session/set_config_option" => {
            // Config selectors: approval posture, model, and per-turn reasoning.
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let key = params
                .as_ref()
                .and_then(|p| p.get("configId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = params
                .as_ref()
                .and_then(|p| p.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let msp_sid = match sessions.lock().unwrap().get(&sid) {
                Some(s) => s.msp_sid.clone(),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            let cmd = host.mint_cmd("cmd-");
            let r = match key.as_str() {
                "mode" => match acp::resolve_mode(&value) {
                    Some(m) => host.command(
                        "session/setApprovalMode",
                        &format!(
                            "{{\"commandId\":{},\"sessionId\":{},\"mode\":{}}}",
                            esc(&cmd),
                            esc(&msp_sid),
                            esc(m)
                        ),
                    ),
                    None => {
                        acp::send_error(stdout, &id, -32602, "mode must be ask|auto|deny");
                        return;
                    }
                },
                "model" => host.command(
                    "session/setModel",
                    &format!(
                        "{{\"commandId\":{},\"sessionId\":{},\"model\":{{\"modelId\":{}}}}}",
                        esc(&cmd),
                        esc(&msp_sid),
                        esc(&value)
                    ),
                ),
                "reasoning_effort" => {
                    if acp::is_reasoning_effort(&value) {
                        Ok(J::Null)
                    } else {
                        acp::send_error(
                            stdout,
                            &id,
                            -32602,
                            "reasoning_effort must be none|minimal|low|medium|high|xhigh|ultra",
                        );
                        return;
                    }
                }
                _ => {
                    acp::send_error(
                        stdout,
                        &id,
                        -32602,
                        "unknown configId (want mode|model|reasoning_effort)",
                    );
                    return;
                }
            };
            match r {
                Ok(res) => {
                    // Return the full updated option set, not just the delta.
                    // The host echoes the folded mode; prefer it over the
                    // request so a downgraded apply cannot desync selectors.
                    let folded = res
                        .get("effectiveMode")
                        .and_then(|e| e.get("mode"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    if let Some(s) = sessions.lock().unwrap().get_mut(&sid) {
                        let ver = s.ver;
                        match key.as_str() {
                            "mode" => {
                                let m = folded
                                    .as_deref()
                                    .or_else(|| acp::resolve_mode(&value))
                                    .unwrap_or("promptUnmatched");
                                s.mode_value = acp::mode_from_msp(m).to_string();
                            }
                            "model" => s.model_value = value.clone(),
                            "reasoning_effort" => s.reasoning_effort = value.clone(),
                            _ => unreachable!(),
                        }
                        let models = catalog(host);
                        acp::send_result(
                            stdout,
                            &id,
                            &format!(
                                "{{\"configOptions\":{}}}",
                                acp::config_options(
                                    ver,
                                    &s.mode_value,
                                    &s.model_value,
                                    &s.reasoning_effort,
                                    &models,
                                )
                            ),
                        );
                    } else {
                        acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    }
                }
                Err(e) => acp::send_error(
                    stdout,
                    &id,
                    -32603,
                    &format!("set failed: {}", err_message(&e)),
                ),
            }
        }
        "session/set_mode" => {
            // v1 operating mode switch, same ask|auto|deny vocabulary.
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = params
                .as_ref()
                .and_then(|p| p.get("mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let msp_sid = match sessions.lock().unwrap().get(&sid) {
                Some(s) => s.msp_sid.clone(),
                None => {
                    acp::send_error(stdout, &id, -32602, "unknown sessionId");
                    return;
                }
            };
            match acp::resolve_mode(&value) {
                Some(m) => {
                    let cmd = host.mint_cmd("cmd-");
                    match host.command(
                        "session/setApprovalMode",
                        &format!(
                            "{{\"commandId\":{},\"sessionId\":{},\"mode\":{}}}",
                            esc(&cmd),
                            esc(&msp_sid),
                            esc(m)
                        ),
                    ) {
                        Ok(_) => {
                            if let Some(s) = sessions.lock().unwrap().get_mut(&sid) {
                                s.mode_value = acp::mode_from_msp(m).to_string();
                            }
                            acp::send_result(stdout, &id, &format!("{{\"mode\":{}}}", esc(&value)))
                        }
                        Err(e) => acp::send_error(
                            stdout,
                            &id,
                            -32603,
                            &format!("set failed: {}", err_message(&e)),
                        ),
                    }
                }
                None => acp::send_error(stdout, &id, -32602, "mode must be ask|auto|deny"),
            }
        }
        "session/set_model" => {
            let sid = params
                .as_ref()
                .and_then(|p| p.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = params
                .as_ref()
                .and_then(|p| p.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if value.is_empty() {
                acp::send_error(
                    stdout,
                    &id,
                    -32602,
                    "session/set_model requires params.model",
                );
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
            match host.command(
                "session/setModel",
                &format!(
                    "{{\"commandId\":{},\"sessionId\":{},\"model\":{{\"modelId\":{}}}}}",
                    esc(&cmd),
                    esc(&msp_sid),
                    esc(&value)
                ),
            ) {
                Ok(_) => {
                    if let Some(s) = sessions.lock().unwrap().get_mut(&sid) {
                        s.model_value = value.clone();
                    }
                    acp::send_result(stdout, &id, &format!("{{\"model\":{}}}", esc(&value)))
                }
                Err(e) => acp::send_error(
                    stdout,
                    &id,
                    -32603,
                    &format!("set failed: {}", err_message(&e)),
                ),
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

/// History replay for `session/load` (always) and v2 `session/resume` with
/// `replayFrom`. Messages replay as message updates/chunks; tool calls replay
/// as completed tool updates (history carries args but no output text).
/// Unknown shapes resume without replay (logged), never fail.
fn replay_history(stdout: &StdoutShared, sess: &mut AcpSession, resume_res: &J) {
    let items = match resume_res.get("history").and_then(|h| h.get("items")) {
        Some(J::Arr(v)) => v.clone(),
        _ => {
            log("resume: unrecognized history shape; resumed without replay");
            return;
        }
    };
    let mut out = Vec::new();
    for it in &items {
        let kind = it.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "toolCall" => {
                // Fold replays the call (title/kind/status/args, no content).
                let wrap = J::Obj(vec![("item".to_string(), it.clone())]);
                sess.fold
                    .on_item_completed(&sess.acp_sid, sess.ver, &wrap, &mut out);
            }
            "userMessage" | "agentMessage" => {
                let text = it.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                let msg_id = mint_id("msg-", &ID_COUNTER);
                let content = format!("[{{\"type\":\"text\",\"text\":{}}}]", esc(text));
                // v1 replays stream chunks; v2 replays full message upserts.
                let update = match (kind, sess.ver) {
                    ("userMessage", 2) => format!(
                        "{{\"sessionUpdate\":\"user_message\",\"messageId\":{},\"content\":{}}}",
                        esc(&msg_id),
                        content
                    ),
                    ("agentMessage", 2) => format!(
                        "{{\"sessionUpdate\":\"agent_message\",\"messageId\":{},\"content\":{}}}",
                        esc(&msg_id),
                        content
                    ),
                    _ => format!(
                        "{{\"sessionUpdate\":\"user_message_chunk\",\"messageId\":{},\"content\":{{\"type\":\"text\",\"text\":{}}}}}",
                        esc(&msg_id),
                        esc(text)
                    ),
                };
                // v1 agent messages replay as agent chunks.
                let update = if kind == "agentMessage" && sess.ver != 2 {
                    format!(
                        "{{\"sessionUpdate\":\"agent_message_chunk\",\"messageId\":{},\"content\":{{\"type\":\"text\",\"text\":{}}}}}",
                        esc(&msg_id),
                        esc(text)
                    )
                } else {
                    update
                };
                out.push(format!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{}}}}}", esc(&sess.acp_sid), update));
            }
            _ => {}
        }
    }
    for line in out {
        acp::send_raw(stdout, &line);
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
    format!(
        "{{\"type\":\"image\",\"base64Data\":{},\"mediaType\":{}}}",
        esc(data_b64),
        esc(mime)
    )
}

/// Build MSP turn input parts: text (+ inlined resource text) and images.
/// Image sources: inline base64 `data`, or a local `file://`/`/` path which
/// is read and encoded here (same machine). Audio has no host surface
/// (TurnInputPartType is closed: text|image) and is rejected.
///
/// Returns `(msp_parts, acp_content)`: the host input and the accepted prompt
/// re-serialized as ACP content for the user-message echo.
fn extract_prompt_parts(params: Option<&J>, cwd: &str) -> Result<(Vec<String>, String), String> {
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
    let mut content: Vec<String> = Vec::new(); // accepted prompt as ACP content
    let flush_text = |texts: &mut Vec<String>, parts: &mut Vec<String>| {
        if texts.is_empty() {
            return;
        }
        let text = normalize_muse_slash_command(&texts.join("\n"));
        parts.push(format!("{{\"type\":\"text\",\"text\":{}}}", esc(&text)));
        texts.clear();
    };
    for b in &blocks {
        match b {
            J::Str(s) => {
                texts.push(s.clone());
                content.push(format!("{{\"type\":\"text\",\"text\":{}}}", esc(s)));
            }
            J::Obj(_) => {
                let t = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "text" => {
                        if let Some(x) = b.get("text").and_then(|v| v.as_str()) {
                            texts.push(x.to_string());
                            content.push(format!("{{\"type\":\"text\",\"text\":{}}}", esc(x)));
                        }
                    }
                    "resource" => {
                        let r = b.get("resource").cloned().unwrap_or(J::Null);
                        let uri = r.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                        let mime = r.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
                        match r.get("text").and_then(|v| v.as_str()) {
                            Some(x) => {
                                texts.push(x.to_string());
                                content.push(j_to_string(b));
                            }
                            None => match r.get("blob").and_then(|v| v.as_str()) {
                                Some(blob) if mime.starts_with("image/") => {
                                    flush_text(&mut texts, &mut parts);
                                    parts.push(image_part(blob, mime));
                                    content.push(j_to_string(b));
                                }
                                Some(_) => return Err("embedded non-image resource blobs are not supported; send text".to_string()),
                                None if !uri.is_empty() => {
                                    texts.push(format!("[resource: {uri}]"));
                                    content.push(j_to_string(b));
                                }
                                None => return Err("resource block needs resource.text, resource.blob, or resource.uri".to_string()),
                            },
                        }
                    }
                    "resource_link" => {
                        // Baseline content: resources the agent can access.
                        let uri = b.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                        let name = b.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
                        let mime = b.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
                        if uri.is_empty() {
                            return Err("resource_link block needs uri".to_string());
                        }
                        match local_file_text(uri, Some(cwd)) {
                            Some(text) if mime.starts_with("text/") || mime.is_empty() || looks_textual(uri) => {
                                texts.push(format!("[{name} {uri}]\n{text}"));
                            }
                            _ => {
                                texts.push(format!("[resource: {name} ({uri})]"));
                            }
                        }
                        content.push(j_to_string(b));
                    }
                    "image" => {
                        flush_text(&mut texts, &mut parts);
                        if let Some(d) = b.get("data").and_then(|v| v.as_str()) {
                            let mime = b.get("mimeType").and_then(|v| v.as_str()).unwrap_or("image/png");
                            parts.push(image_part(d, mime));
                            content.push(j_to_string(b));
                        } else if let Some(uri) = b.get("uri").and_then(|v| v.as_str()) {
                            let (bytes, mime) = read_image_uri(uri, Some(cwd))?;
                            parts.push(image_part(&json::b64(&bytes), &mime));
                            content.push(j_to_string(b));
                        } else {
                            return Err("image block needs data or uri".to_string());
                        }
                    }
                    "audio" => return Err("audio blocks are not supported: the host input type is closed (text|image)".to_string()),
                    _ => return Err(format!("unsupported content block type '{t}'")),
                }
            }
            _ => return Err("prompt blocks must be objects or strings".to_string()),
        }
    }
    flush_text(&mut texts, &mut parts);
    Ok((parts, format!("[{}]", content.join(","))))
}

/// Short editor commands map to Muse's stable skill invocation syntax. The ACP
/// user-message echo keeps what the client sent; only host input is normalized.
fn normalize_muse_slash_command(text: &str) -> String {
    let trimmed = text.trim_start();
    let mut words = trimmed.splitn(2, char::is_whitespace);
    let command = words.next().unwrap_or_default();
    let argument = words.next().unwrap_or_default().trim();
    match command {
        "/plan" | "/doctor" | "/create-skill" | "/create-plugin" | "/import" => {
            let skill = command.trim_start_matches('/');
            if argument.is_empty() {
                format!("/skill {skill}")
            } else {
                format!("/skill {skill} {argument}")
            }
        }
        _ => text.to_string(),
    }
}

/// Decode a `file://` URI to a local path. Rejects hosts, non-file schemes,
/// and bad escapes. Relative paths resolve against `cwd`.
fn file_uri_path(uri: &str, cwd: &str) -> Result<String, String> {
    let rest = match uri.strip_prefix("file://") {
        Some(r) => r,
        None if uri.starts_with('/') => return Ok(uri.to_string()),
        None if !uri.contains("://") => {
            return Ok(format!("{}/{}", cwd.trim_end_matches('/'), uri));
        }
        None => return Err(format!("unsupported URI scheme in {uri}")),
    };
    // file://host/path: only empty/localhost hosts are local files.
    let path = match rest.find('/') {
        Some(i) => {
            let (host, p) = rest.split_at(i);
            if !host.is_empty() && host != "localhost" {
                return Err(format!("remote file host in {uri}"));
            }
            p.to_string()
        }
        None => return Err(format!("bad file URI {uri}")),
    };
    Ok(percent_decode(&path))
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Confine a path to the session workspace unless explicitly opened up.
fn confine(path: &str, cwd: &str) -> Result<(), String> {
    if std::env::var("MUSE_ALLOW_UNSCOPED_READS").is_ok() {
        return Ok(());
    }
    let canon = std::fs::canonicalize(path).map_err(|e| format!("cannot resolve {path}: {e}"))?;
    let root =
        std::fs::canonicalize(cwd).map_err(|e| format!("cannot resolve workspace {cwd}: {e}"))?;
    if canon.starts_with(&root) {
        Ok(())
    } else {
        Err(format!(
            "{path} is outside the session workspace (set MUSE_ALLOW_UNSCOPED_READS=1 to allow)"
        ))
    }
}

fn looks_textual(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".txt")
        || p.ends_with(".md")
        || p.ends_with(".rs")
        || p.ends_with(".py")
        || p.ends_with(".js")
        || p.ends_with(".ts")
        || p.ends_with(".json")
        || p.ends_with(".toml")
        || p.ends_with(".yaml")
        || p.ends_with(".yml")
        || p.ends_with(".sh")
        || p.ends_with(".log")
}

fn read_image_uri(uri: &str, cwd: Option<&str>) -> Result<(Vec<u8>, String), String> {
    let path = file_uri_path(uri, cwd.unwrap_or("/"))?;
    if let Some(c) = cwd {
        confine(&path, c)?;
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read image {path}: {e}"))?;
    Ok((bytes, mime_for(&path).to_string()))
}

/// Read a small text file for resource_link inlining (None = mention only).
fn local_file_text(uri: &str, cwd: Option<&str>) -> Option<String> {
    let path = file_uri_path(uri, cwd.unwrap_or("/")).ok()?;
    if let Some(c) = cwd {
        confine(&path, c).ok()?;
    }
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > 262144 {
        return None;
    }
    std::fs::read_to_string(&path).ok()
}

// ---------------------------------------------------------------------------
// MSP -> ACP event routing (main thread; the serve reader only forwards)
// ---------------------------------------------------------------------------

fn find_acp_sid(sessions: &Sessions, msp_sid: &str) -> Option<String> {
    sessions
        .lock()
        .unwrap()
        .iter()
        .find(|(_, s)| s.msp_sid == msp_sid)
        .map(|(k, _)| k.clone())
}

fn handle_msp(
    host: &Arc<MspHost>,
    stdout: &StdoutShared,
    sessions: &Sessions,
    method: &str,
    params: &J,
) {
    // Track the newest view cursor on every event that carries one, so a
    // view/gap can page forward without duplicates (fold skips settled ids).
    if let Some(cur) = params.get("viewCursor").and_then(|v| v.as_str())
        && !cur.is_empty()
    {
        let msp_sid = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(acp_sid) = find_acp_sid(sessions, msp_sid)
            && let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid)
        {
            s.view_cursor = cur.to_string();
        }
    }
    match method {
        "view/gap" => {
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let (acp_sid, cursor) = match find_acp_sid(sessions, msp_sid) {
                Some(a) => {
                    let c = sessions
                        .lock()
                        .unwrap()
                        .get(&a)
                        .map(|s| s.view_cursor.clone())
                        .unwrap_or_default();
                    (a, c)
                }
                None => return,
            };
            if cursor.is_empty() {
                log("view/gap with no known cursor; cannot refill");
                return;
            }
            let cmd = host.mint_cmd("cmd-");
            match host.command("view/page", &format!("{{\"commandId\":{},\"sessionId\":{},\"cursor\":{},\"direction\":\"forward\",\"limit\":100}}", esc(&cmd), esc(msp_sid), esc(&cursor))) {
                Ok(r) => {
                    let mut n = 0;
                    if let Some(J::Arr(evs)) = r.get("events") {
                        for e in evs.clone() {
                            let m = e.get("method").and_then(|v| v.as_str()).unwrap_or("");
                            let p = e.get("params").cloned().unwrap_or(J::Null);
                            if !m.is_empty() {
                                n += 1;
                                handle_msp(host, stdout, sessions, m, &p);
                            }
                        }
                    } else if let Some(J::Arr(items)) = r.get("items") {
                        for it in items.clone() {
                            let wrap = J::Obj(vec![("item".to_string(), it)]);
                            let mut out = Vec::new();
                            if let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid) {
                                s.fold.on_item_completed(&acp_sid, s.ver, &wrap, &mut out);
                            }
                            for line in out {
                                acp::send_raw(stdout, &line);
                            }
                            n += 1;
                        }
                    } else {
                        log(&format!("view/page returned no events/items: {}", j_to_string(&r)));
                    }
                    log(&format!("view/gap refilled {n} events"));
                }
                Err(e) => log(&format!("view/page failed: {}", err_message(&e))),
            }
        }
        "item/started" | "item/updated" => {
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let turn_id = params.get("turnId").and_then(|v| v.as_str()).unwrap_or("");
            let terminal = params
                .get("terminal")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            log(&format!(
                "turn/completed turn={turn_id} terminal={terminal}"
            ));
            let acp_sid = match find_acp_sid(sessions, msp_sid) {
                Some(s) => s,
                None => return,
            };
            let settled = sessions.lock().unwrap().get_mut(&acp_sid).map(|s| {
                if s.active_turn.as_deref() == Some(turn_id) {
                    s.active_turn = None;
                }
                let pos = s.in_flight.iter().position(|f| f.msp_turn == turn_id);
                let ver = s.ver;
                let req_id = pos.map(|p| s.in_flight.remove(p).req_id);
                let rest = s.in_flight.len();
                (req_id, ver, rest)
            });
            if let Some((req_id, ver, rest)) = settled {
                let stop = fold::stop_reason(terminal);
                if ver == 2 {
                    // Idle only when no session work remains; otherwise
                    // re-assert running so queued work isn't misreported.
                    if rest == 0 {
                        acp::send_state(stdout, &acp_sid, "idle", Some(stop));
                    } else {
                        acp::send_state(stdout, &acp_sid, "running", None);
                    }
                }
                if let Some(req_id) = req_id {
                    if ver != 2 {
                        if terminal == "completed" || terminal == "cancelled" {
                            acp::send_result(
                                stdout,
                                &Some(req_id),
                                &format!("{{\"stopReason\":\"{stop}\"}}"),
                            );
                        } else {
                            acp::send_error(
                                stdout,
                                &Some(req_id),
                                -32603,
                                &format!("turn ended with terminal '{terminal}'"),
                            );
                        }
                    }
                } else {
                    log(&format!("turn/completed for untracked turn {turn_id}"));
                }
            }
        }
        "turn/unqueued" => {
            // A reclaimed queued turn never runs: no started/completed will
            // ever arrive, so settle the tracked prompt now as cancelled.
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let turn_id = params.get("turnId").and_then(|v| v.as_str()).unwrap_or("");
            let acp_sid = match find_acp_sid(sessions, msp_sid) {
                Some(s) => s,
                None => return,
            };
            let settled = sessions.lock().unwrap().get_mut(&acp_sid).map(|s| {
                let pos = s.in_flight.iter().position(|f| f.msp_turn == turn_id)?;
                let ver = s.ver;
                Some((s.in_flight.remove(pos).req_id, ver, s.in_flight.len()))
            });
            if let Some((req_id, ver, rest)) = settled.flatten() {
                if ver == 2 {
                    if rest == 0 {
                        acp::send_state(stdout, &acp_sid, "idle", Some("cancelled"));
                    }
                } else {
                    acp::send_result(stdout, &Some(req_id), "{\"stopReason\":\"cancelled\"}");
                }
                let _ = req_id;
            }
        }
        "approval/requested" => {
            open_approval(stdout, sessions, params);
        }
        "approval/resolved" | "approval/updated" => {
            // Authoritative outcome: if session work continues, re-assert
            // running (a resolved approval unblocks the turn).
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid) {
                let (ver, busy) = sessions
                    .lock()
                    .unwrap()
                    .get(&acp_sid)
                    .map(|s| (s.ver, !s.in_flight.is_empty()))
                    .unwrap_or((1, false));
                if ver == 2 && busy {
                    acp::send_state(stdout, &acp_sid, "running", None);
                }
            }
        }
        "userInput/requested" => {
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let acp_sid = find_acp_sid(sessions, msp_sid);
            // Bridge to ACP elicitation when the client advertised form mode;
            // otherwise cancel so the turn proceeds instead of hanging.
            let bridged = match (&acp_sid, ELICIT_FORM.load(Ordering::SeqCst)) {
                (Some(sid), 1) => {
                    let ver = sessions
                        .lock()
                        .unwrap()
                        .get(sid)
                        .map(|s| s.ver)
                        .unwrap_or(1);
                    ver == 2 && bridge_user_input(host, stdout, sessions, sid, params)
                }
                _ => false,
            };
            if !bridged {
                let qid = params
                    .get("userInputId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                log(&format!(
                    "userInput/requested not bridged (elicit_form={}); falling back",
                    ELICIT_FORM.load(Ordering::SeqCst)
                ));
                if !msp_sid.is_empty() && !qid.is_empty() {
                    let cmd = host.mint_cmd("cmd-");
                    let _ = host.command(
                        "userInput/cancel",
                        &format!(
                            "{{\"commandId\":{},\"sessionId\":{},\"userInputId\":{}}}",
                            esc(&cmd),
                            esc(msp_sid),
                            esc(qid)
                        ),
                    );
                    log(&format!(
                        "userInput {qid} auto-cancelled (client has no elicitation form)"
                    ));
                }
            }
        }
        "userInput/settled" => {}
        "turn/started" => {
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let turn_id = params.get("turnId").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid)
                && !turn_id.is_empty()
                && let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid)
            {
                s.active_turn = Some(turn_id.to_string());
            }
            log(&format!("turn/started turn={turn_id} sess={msp_sid}"));
        }
        "turn/retracted" | "turn/retryScheduled" => {
            log(&format!(
                "{method} turn={} sess={}",
                params.get("turnId").and_then(|v| v.as_str()).unwrap_or("?"),
                params
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
            ));
        }
        "session/approvalModeChanged" => {
            // Audit fact of an accepted mode change: refresh the selector.
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mode = params.get("mode").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid)
                && !mode.is_empty()
            {
                if let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid) {
                    s.mode_value = acp::mode_from_msp(mode).to_string();
                }
                let _ = acp_sid;
            }
        }
        "session/modelChanged" => {
            let msp_sid = params
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let model = params
                .get("modelId")
                .or_else(|| params.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(acp_sid) = find_acp_sid(sessions, msp_sid)
                && !model.is_empty()
            {
                if let Some(s) = sessions.lock().unwrap().get_mut(&acp_sid) {
                    s.model_value = model.to_string();
                }
                let _ = acp_sid;
            }
        }
        "initialized"
        | "session/started"
        | "session/contextUsage"
        | "session/tokenUsage"
        | "session/goalChanged"
        | "session/todoListChanged"
        | "session/branchChanged" => {}
        _ => {
            log(&format!("unhandled MSP notification: {method}"));
        }
    }
}

/// Open an ACP permission request for MSP approval params (from either the
/// `approval/requested` event or a reissued `approval/request`). Dedupes by
/// approval id so multi-stage/resumed flows bridge exactly once.
fn open_approval(stdout: &StdoutShared, sessions: &Sessions, params: &J) {
    let msp_sid = params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let acp_sid = match find_acp_sid(sessions, msp_sid) {
        Some(s) => s,
        None => return,
    };
    let approval_id = params
        .get("approvalId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if approval_id.is_empty() {
        return;
    }
    let already = sessions
        .lock()
        .unwrap()
        .get(&acp_sid)
        .map(|s| match &s.pending_perm {
            Some(p) => p.approval_id == approval_id,
            None => false,
        })
        .unwrap_or(false);
    if already {
        return;
    }
    let requirement = params
        .get("currentRequirementId")
        .cloned()
        .unwrap_or(J::Null);
    let tool_call_id = params
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let subject = params.get("subject").cloned().unwrap_or(J::Null);
    let title = params
        .get("toolName")
        .and_then(|v| v.as_str())
        .or_else(|| subject.get("command").and_then(|v| v.as_str()))
        .or_else(|| subject.get("path").and_then(|v| v.as_str()))
        .or_else(|| subject.get("target").and_then(|v| v.as_str()))
        .or_else(|| subject.get("access").and_then(|v| v.as_str()))
        .unwrap_or("Muse action");
    let kind = match subject.get("kind").and_then(|v| v.as_str()) {
        Some("shell") | Some("process") => "execute",
        Some("fileAccess") => "read",
        Some("network") => "fetch",
        _ => "other",
    };
    let (options_json, choices) = acp::perm_options(params);
    if choices.is_empty() {
        log(&format!(
            "approval {approval_id} has no choices; leaving unresolved"
        ));
        return;
    }
    let req_id = J::Str(mint_id("perm-", &ID_COUNTER));
    {
        let mut map = sessions.lock().unwrap();
        if let Some(s) = map.get_mut(&acp_sid) {
            s.pending_perm = Some(PendingPerm {
                req_id: req_id.clone(),
                approval_id,
                requirement,
                choices,
            });
            if s.ver == 2 {
                acp::send_state(stdout, &acp_sid, "requires_action", None);
            }
        } else {
            return;
        }
    }
    acp::send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"session/request_permission\",\"params\":{{\"sessionId\":{},\"toolCall\":{{\"toolCallId\":{},\"title\":{},\"kind\":\"{kind}\",\"status\":\"pending\"}},\"options\":{}}}}}",
            j_to_string(&req_id),
            esc(&acp_sid),
            esc(&tool_call_id),
            esc(title),
            options_json
        ),
    );
}

/// Client reply to our `session/request_permission` (matched by id).
/// Fail closed: without an explicit approving choice from the client, never
/// send an approving decide — cancel the underlying turn instead.
fn complete_permission(
    host: &Arc<MspHost>,
    stdout: &StdoutShared,
    sessions: &Sessions,
    id: &Option<J>,
    msg: &J,
) {
    let idv = match id {
        Some(v) => v.clone(),
        None => return,
    };
    // Locate the session holding this pending permission.
    let found = sessions
        .lock()
        .unwrap()
        .iter()
        .find_map(|(k, s)| match &s.pending_perm {
            Some(p) if j_to_string(&p.req_id) == j_to_string(&idv) => Some(k.clone()),
            _ => None,
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
        (
            s.msp_sid.clone(),
            s.ver,
            p.approval_id,
            p.requirement,
            p.choices,
        )
    };
    // Outcome -> (choiceId, approved?). Only an explicit client selection of
    // an approving choice may approve. Everything else fails closed: cancel
    // the underlying turn rather than risk an approving decide.
    enum Verdict {
        Approve(String),
        Deny(String),
        FailClosed,
    }
    let is_approving = |cid: &str| {
        choices
            .iter()
            .find(|(id, _)| id == cid)
            .map(|(_, d)| d.to_lowercase().starts_with("approv"))
            .unwrap_or(false)
    };
    let verdict = if msg.get("error").is_some() {
        log("session/request_permission failed at client; failing closed");
        match acp::fallback_deny(&choices) {
            Some(c) => Verdict::Deny(c),
            None => Verdict::FailClosed,
        }
    } else {
        match msg.get("result").and_then(|r| r.get("outcome")) {
            Some(o) => match o.get("outcome").and_then(|v| v.as_str()).unwrap_or("") {
                "selected" => match o.get("optionId").and_then(|v| v.as_str()) {
                    Some(cid) if is_approving(cid) => Verdict::Approve(cid.to_string()),
                    Some(cid) => Verdict::Deny(cid.to_string()),
                    None => match acp::fallback_deny(&choices) {
                        Some(c) => Verdict::Deny(c),
                        None => Verdict::FailClosed,
                    },
                },
                _ => match acp::fallback_deny(&choices) {
                    Some(c) => Verdict::Deny(c),
                    None => Verdict::FailClosed,
                },
            },
            None => match acp::fallback_deny(&choices) {
                Some(c) => Verdict::Deny(c),
                None => Verdict::FailClosed,
            },
        }
    };
    let choice = match verdict {
        Verdict::Approve(c) | Verdict::Deny(c) => c,
        Verdict::FailClosed => {
            log("permission: no deny choice available; cancelling the turn instead of approving");
            cancel_session_turns(host, sessions, &acp_sid);
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
        Ok(r) => {
            // Admission is not the outcome: terminal=false means further
            // requirements remain pending, so stay in requires_action.
            let terminal = r.get("terminal").and_then(|v| match v {
                J::Bool(b) => Some(*b),
                _ => None,
            }).unwrap_or(true);
            if ver == 2 && terminal {
                let busy = sessions.lock().unwrap().get(&acp_sid).map(|s| !s.in_flight.is_empty()).unwrap_or(false);
                if busy {
                    acp::send_state(stdout, &acp_sid, "running", None);
                }
            }
        }
        Err(e) => log(&format!("approval/decide failed: {}", err_message(&e))),
    }
}

/// Cancel every in-flight turn of one ACP session (fail-closed helper).
fn cancel_session_turns(host: &Arc<MspHost>, sessions: &Sessions, acp_sid: &str) {
    let turns = sessions
        .lock()
        .unwrap()
        .get(acp_sid)
        .map(|s| {
            let msp = s.msp_sid.clone();
            s.in_flight
                .iter()
                .map(|f| (msp.clone(), f.msp_turn.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (msp_sid, turn_id) in turns {
        let cmd = host.mint_cmd("cmd-");
        let _ = host.command(
            "turn/cancel",
            &format!(
                "{{\"commandId\":{},\"sessionId\":{},\"turnId\":{}}}",
                esc(&cmd),
                esc(&msp_sid),
                esc(&turn_id)
            ),
        );
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
    let user_input_id = params
        .get("userInputId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tool_call = params
        .get("toolCallId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
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
        let qid = q
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if qid.is_empty() {
            continue;
        }
        let header = q.get("header").and_then(|v| v.as_str()).unwrap_or("");
        let text = q.get("question").and_then(|v| v.as_str()).unwrap_or("");
        // Dedupe display labels (duplicate enum values confuse clients);
        // answers map back to originals by position.
        let mut labels = Vec::new();
        let mut display = Vec::new();
        if let Some(J::Arr(o)) = q.get("options") {
            for x in o {
                if let Some(l) = x.get("label").and_then(|v| v.as_str()) {
                    let mut name = l.to_string();
                    let mut n = 2;
                    while display.iter().any(|e: &String| e == &name) {
                        name = format!("{l} ({n})");
                        n += 1;
                    }
                    labels.push(l.to_string());
                    display.push(name);
                }
            }
        }
        let single = q
            .get("selection")
            .and_then(|s| s.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("single")
            == "single";
        let min = q
            .get("selection")
            .and_then(|s| s.get("minSelections"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1);
        let max = q
            .get("selection")
            .and_then(|s| s.get("maxSelections"))
            .and_then(|v| v.as_u64());
        let key = format!("q{i}");
        let en: Vec<String> = display.iter().map(|l| esc(l)).collect();
        if labels.is_empty() {
            // Free-text question: no options, plain string answer.
            props.push(format!("{}: {{\"type\":\"string\"}}", esc(&key)));
        } else if single {
            props.push(format!(
                "{}: {{\"type\":\"string\",\"enum\":[{}]}}",
                esc(&key),
                en.join(",")
            ));
        } else {
            let mut sch = format!(
                "{}: {{\"type\":\"array\",\"items\":{{\"type\":\"string\",\"enum\":[{}]}}",
                esc(&key),
                en.join(",")
            );
            sch.push_str(&format!("}},\"minItems\":{min}"));
            if let Some(m) = max {
                sch.push_str(&format!(",\"maxItems\":{m}"));
            }
            sch.push('}');
            props.push(sch);
        }
        if single || min > 0 {
            required.push(key.to_string());
        }
        msg.push(format!("{header}: {text}"));
        ui_qs.push(acp::UiQuestion {
            qid,
            labels,
            display,
        });
    }
    if ui_qs.is_empty() {
        return false;
    }
    let schema = format!(
        "{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}]}}",
        props.join(","),
        required
            .iter()
            .map(|k| esc(k))
            .collect::<Vec<_>>()
            .join(",")
    );
    let req_id = J::Str(mint_id("elic-", &ID_COUNTER));
    if let Some(s) = sessions.lock().unwrap().get_mut(acp_sid) {
        s.pending_ui.push(acp::PendingUi {
            req_id: req_id.clone(),
            user_input_id: user_input_id.clone(),
            questions: ui_qs,
        });
        log(&format!(
            "bridging userInput {user_input_id} to elicitation {}",
            j_to_string(&req_id)
        ));
        if s.ver == 2 {
            acp::send_state(stdout, acp_sid, "requires_action", None);
        }
    } else {
        return false;
    }
    let tool_f = if tool_call.is_empty() {
        String::new()
    } else {
        format!(",\"toolCallId\":{}", esc(&tool_call))
    };
    acp::send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"elicitation/create\",\"params\":{{\"sessionId\":{}{},\"mode\":\"form\",\"message\":{},\"requestedSchema\":{}}}}}",
            j_to_string(&req_id),
            esc(acp_sid),
            tool_f,
            esc(&msg.join("\n")),
            schema
        ),
    );
    true
}

/// Match a client-returned display label back to the original host label.
fn ui_original(q: &acp::UiQuestion, shown: &str) -> Option<String> {
    q.display
        .iter()
        .position(|d| d == shown)
        .and_then(|i| q.labels.get(i))
        .cloned()
}

/// Client reply to our `elicitation/create` (matched by id).
fn complete_elicitation(
    host: &Arc<MspHost>,
    stdout: &StdoutShared,
    sessions: &Sessions,
    id: &Option<J>,
    msg: &J,
) {
    let idv = match id {
        Some(v) => v.clone(),
        None => return,
    };
    let found = sessions.lock().unwrap().iter().find_map(|(k, s)| {
        s.pending_ui
            .iter()
            .position(|p| j_to_string(&p.req_id) == j_to_string(&idv))
            .map(|i| (k.clone(), i))
    });
    let (acp_sid, idx) = match found {
        Some(v) => v,
        None => return,
    };
    let (msp_sid, ver, user_input_id, questions) = {
        let mut map = sessions.lock().unwrap();
        let s = match map.get_mut(&acp_sid) {
            Some(s) => s,
            None => return,
        };
        if idx >= s.pending_ui.len() {
            return;
        }
        let p = s.pending_ui.remove(idx);
        (s.msp_sid.clone(), s.ver, p.user_input_id, p.questions)
    };
    let cmd = host.mint_cmd("cmd-");
    // accept + content -> answers; anything else -> cancel the question.
    let mut answers: Option<String> = None;
    if msg.get("error").is_none()
        && let Some(res) = msg.get("result")
        && res.get("action").and_then(|v| v.as_str()).unwrap_or("") == "accept"
    {
        let content = res.get("content").cloned().unwrap_or(J::Null);
        let mut parts = Vec::new();
        for (i, q) in questions.iter().enumerate() {
            let key = format!("q{i}");
            match content.get(key.as_str()) {
                Some(J::Str(v)) => match ui_original(q, v) {
                    Some(orig) => parts.push(format!(
                        "{{\"questionId\":{},\"selectedLabel\":{}}}",
                        esc(&q.qid),
                        esc(&orig)
                    )),
                    None => parts.push(format!(
                        "{{\"questionId\":{},\"freeText\":{}}}",
                        esc(&q.qid),
                        esc(v)
                    )),
                },
                Some(J::Arr(vs)) => {
                    let mut matched = Vec::new();
                    let mut free = Vec::new();
                    for v in vs {
                        match v.as_str().and_then(|s| ui_original(q, s)) {
                            Some(orig) => matched.push(esc(&orig)),
                            None => {
                                if let Some(s) = v.as_str() {
                                    free.push(s.to_string());
                                }
                            }
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
    match answers {
        Some(a) => {
            if let Err(e) = host.command(
                "userInput/answer",
                &format!(
                    "{{\"commandId\":{},\"sessionId\":{},\"userInputId\":{},\"answers\":{}}}",
                    esc(&cmd),
                    esc(&msp_sid),
                    esc(&user_input_id),
                    a
                ),
            ) {
                log(&format!("userInput/answer failed: {}", err_message(&e)));
            } else if ver == 2 {
                let busy = sessions
                    .lock()
                    .unwrap()
                    .get(&acp_sid)
                    .map(|s| !s.in_flight.is_empty())
                    .unwrap_or(false);
                if busy {
                    acp::send_state(stdout, &acp_sid, "running", None);
                }
            }
        }
        None => {
            if let Err(e) = host.command(
                "userInput/cancel",
                &format!(
                    "{{\"commandId\":{},\"sessionId\":{},\"userInputId\":{}}}",
                    esc(&cmd),
                    esc(&msp_sid),
                    esc(&user_input_id)
                ),
            ) {
                log(&format!("userInput/cancel failed: {}", err_message(&e)));
            } else {
                log("elicitation declined/cancelled/failed; question cancelled");
            }
        }
    }
}
