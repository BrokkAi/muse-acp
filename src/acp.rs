//! ACP layer: session store, emit helpers, permission mapping, config options.
//!
//! v2 per the v2 schema (`state.state`, chunk `messageId`, outcome objects);
//! v1 shapes verified live.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::fold::SessionFold;
use crate::json::{esc, j_to_string, J};

pub type StdoutShared = Arc<Mutex<std::io::Stdout>>;

pub struct InFlight {
    pub msp_turn: String,
    pub req_id: J,
}

pub struct PendingPerm {
    /// ACP `session/request_permission` request id awaiting the client reply.
    pub req_id: J,
    pub approval_id: String,
    pub requirement: J,
    /// (choiceId, decision) in host order; first reject-ish is the deny fallback.
    pub choices: Vec<(String, String)>,
}

pub struct UiQuestion {
    pub qid: String,
    pub labels: Vec<String>,
}

pub struct PendingUi {
    /// ACP `elicitation/create` request id awaiting the client reply.
    pub req_id: J,
    pub user_input_id: String,
    pub questions: Vec<UiQuestion>,
}

pub struct AcpSession {
    pub acp_sid: String,
    pub msp_sid: String,
    pub ver: u8,
    pub in_flight: Vec<InFlight>,
    pub pending_perm: Option<PendingPerm>,
    pub pending_ui: Option<PendingUi>,
    pub fold: SessionFold,
}

pub type Sessions = Arc<Mutex<HashMap<String, AcpSession>>>;

pub fn send_raw(stdout: &StdoutShared, line: &str) {
    let mut out = stdout.lock().unwrap();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

pub fn id_json(id: &Option<J>) -> String {
    match id {
        None => "null".to_string(),
        Some(j) => j_to_string(j),
    }
}

pub fn send_result(stdout: &StdoutShared, id: &Option<J>, result_json: &str) {
    if id.is_none() {
        return;
    }
    send_raw(
        stdout,
        &format!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{}}}", id_json(id), result_json),
    );
}

pub fn send_error(stdout: &StdoutShared, id: &Option<J>, code: i64, message: &str) {
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":{}}}}}",
            id_json(id),
            esc(message)
        ),
    );
}

/// v2 `state_update`. v1 callers use the prompt response instead.
pub fn send_state(stdout: &StdoutShared, acp_sid: &str, state: &str, stop: Option<&str>) {
    let stop_f = stop.map(|s| format!(",\"stopReason\":{}", esc(s))).unwrap_or_default();
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"state_update\",\"state\":\"{state}\"{stop_f}}}}}}}",
            esc(acp_sid),
        ),
    );
}

fn perm_kind(decision: &str, scope: &str) -> &'static str {
    let approved = decision.to_lowercase().starts_with("approv");
    let always = scope.eq_ignore_ascii_case("session");
    match (approved, always) {
        (true, true) => "allow_always",
        (true, false) => "allow_once",
        (false, true) => "reject_always",
        (false, false) => "reject_once",
    }
}

/// Build ACP permission `options` from MSP `availableChoices`; returns
/// (options_json, choices) for later decision mapping.
pub fn perm_options(params: &J) -> (String, Vec<(String, String)>) {
    let mut opts = Vec::new();
    let mut choices = Vec::new();
    if let J::Arr(items) = params.get("availableChoices").cloned().unwrap_or(J::Null) {
        for c in items {
            let id = c.get("choiceId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() {
                continue;
            }
            let label = c.get("label").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            let decision = c.get("decision").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let scope = c.get("scope").and_then(|v| v.as_str()).unwrap_or("once").to_string();
            opts.push(format!(
                "{{\"optionId\":{},\"name\":{},\"kind\":\"{}\"}}",
                esc(&id),
                esc(&label),
                perm_kind(&decision, &scope)
            ));
            choices.push((id, decision));
        }
    }
    (format!("[{}]", opts.join(",")), choices)
}

/// Deny-safe fallback choice: first non-approved decision, else first choice.
pub fn fallback_deny(choices: &[(String, String)]) -> Option<String> {
    for (id, d) in choices {
        if !d.to_lowercase().starts_with("approv") {
            return Some(id.clone());
        }
    }
    choices.first().map(|(id, _)| id.clone())
}

/// Our mode vocabulary -> MSP ApprovalMode.
pub fn mode_to_msp(mode: &str) -> Option<&'static str> {
    match mode {
        "ask" => Some("promptUnmatched"),
        "auto" => Some("allowAll"),
        "deny" => Some("denyUnmatched"),
        _ => None,
    }
}

/// MSP ApprovalMode -> our mode vocabulary.
pub fn mode_from_msp(mode: &str) -> &'static str {
    match mode {
        "allowAll" => "auto",
        "denyUnmatched" => "deny",
        _ => "ask",
    }
}

/// v2 `configOptions` for a fresh session: mode + model selectors.
pub fn config_options(current_mode: &str, models_json: &[(String, String, bool)]) -> String {
    let mut model_opts = Vec::new();
    let mut current_model = String::new();
    for (id, label, is_default) in models_json {
        if *is_default && current_model.is_empty() {
            current_model = id.clone();
        }
        model_opts.push(format!(
            "{{\"value\":{},\"name\":{}}}",
            esc(id),
            esc(label)
        ));
    }
    format!(
        "[{{\"configId\":\"mode\",\"name\":\"Session Mode\",\"description\":\"How the agent handles tool approvals\",\"category\":\"mode\",\"type\":\"select\",\"currentValue\":{},\"options\":[{{\"value\":\"ask\",\"name\":\"Ask\",\"description\":\"Request permission for unmatched tools\"}},{{\"value\":\"auto\",\"name\":\"Auto\",\"description\":\"Allow all tools without asking\"}},{{\"value\":\"deny\",\"name\":\"Deny\",\"description\":\"Deny unmatched tools\"}}]}},{{\"configId\":\"model\",\"name\":\"Model\",\"category\":\"model\",\"type\":\"select\",\"currentValue\":{},\"options\":[{}]}}]",
        esc(current_mode),
        esc(&current_model),
        model_opts.join(",")
    )
}
