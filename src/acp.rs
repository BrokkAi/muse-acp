//! ACP layer: session store, emit helpers, permission mapping, config options.
//!
//! v2 per the v2 schema (`state.state`, chunk `messageId`, outcome objects);
//! v1 shapes verified live.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::fold::SessionFold;
use crate::json::{J, esc, j_to_string};

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
    /// Original host labels (for answers).
    pub labels: Vec<String>,
    /// Display labels (deduped; shown to the client).
    pub display: Vec<String>,
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
    pub cwd: String,
    pub ver: u8,
    pub in_flight: Vec<InFlight>,
    pub pending_perm: Option<PendingPerm>,
    /// Approvals awaiting display while another permission is shown. Raw MSP
    /// `approval/request` params; drained one at a time because the adapter
    /// shows one ACP permission request per session at a time.
    pub perm_queue: Vec<J>,
    pub pending_ui: Vec<PendingUi>,
    /// User-input ids already presented or auto-cancelled, so reconciliation
    /// cannot replay a settled question.
    pub ui_seen: std::collections::HashSet<String>,
    pub mode_value: String,
    pub model_value: String,
    pub reasoning_effort: String,
    /// The foreground MSP turn, excluding queued turns.
    pub active_turn: Option<String>,
    pub view_cursor: String,
    pub fold: SessionFold,
    /// Last known context occupancy (`session/contextUsage.usedTokens`).
    pub usage_used: Option<u64>,
    /// Last known context window (`session/contextUsage.windowTokens`).
    pub usage_size: Option<u64>,
    /// Counted-once session cumulative totals (`session/tokenUsage.cumulative`).
    pub cum_prompt: Option<u64>,
    pub cum_output: Option<u64>,
    pub cum_total: Option<u64>,
    /// Running list-price estimate, accumulated per completion from catalog
    /// per-1M rates: (amount, currency). Partial in both directions —
    /// historic and unpriceable completions are excluded, while cached input
    /// is charged at the full input rate — so it is never a billing figure
    /// on plan subscriptions.
    pub cost_amount: Option<(f64, String)>,
    /// View cursors of completions already folded into the totals above.
    /// `view/gap` recovery can replay a completion that also arrives live.
    pub usage_seen: std::collections::HashSet<String>,
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
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{}}}",
            id_json(id),
            result_json
        ),
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

/// `usage_update` for both ACP versions (`{used, size}` plus counted-once
/// session cumulative totals in `_meta`). Emits only when both `used` and
/// `size` are known; callers stash partial state on the session instead.
pub fn send_usage(stdout: &StdoutShared, s: &AcpSession, pressure: Option<&str>) {
    let (Some(used), Some(size)) = (s.usage_used, s.usage_size) else {
        return;
    };
    let mut meta = String::from("\"museCumulative\":{");
    meta.push_str(&format!(
        "\"promptTokens\":{},\"outputTokens\":{},\"totalTokens\":{}",
        s.cum_prompt.map(|v| v.to_string()).unwrap_or("null".into()),
        s.cum_output.map(|v| v.to_string()).unwrap_or("null".into()),
        s.cum_total.map(|v| v.to_string()).unwrap_or("null".into()),
    ));
    meta.push('}');
    if let Some(p) = pressure {
        meta.push_str(&format!(",\"musePressure\":{}", esc(p)));
    }
    // `amount` must be a JSON number: Rust's Display prints `inf`/`NaN`
    // verbatim, which would corrupt the whole frame.
    let cost_f = match &s.cost_amount {
        Some((amount, currency)) if amount.is_finite() => format!(
            ",\"cost\":{{\"amount\":{amount},\"currency\":{}}}",
            esc(currency)
        ),
        _ => String::new(),
    };
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"usage_update\",\"used\":{used},\"size\":{size}{cost_f},\"_meta\":{{{meta}}}}}}}}}",
            esc(&s.acp_sid),
        ),
    );
}

/// v2 `state_update`. v1 callers use the prompt response instead.
pub fn send_state(stdout: &StdoutShared, acp_sid: &str, state: &str, stop: Option<&str>) {
    let stop_f = stop
        .map(|s| format!(",\"stopReason\":{}", esc(s)))
        .unwrap_or_default();
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
    let always =
        scope.eq_ignore_ascii_case("session") || scope.eq_ignore_ascii_case("localpersistent");
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
            let id = c
                .get("choiceId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let label = c
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let decision = c
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let scope = c
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("once")
                .to_string();
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

/// Deny-safe fallback choice: first non-approved decision, else None. A
/// client cancellation/error must never resolve to an approving choice,
/// so an all-approve (or empty) list fails closed upstream.
pub fn fallback_deny(choices: &[(String, String)]) -> Option<String> {
    for (id, d) in choices {
        if !d.to_lowercase().starts_with("approv") {
            return Some(id.clone());
        }
    }
    None
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

/// Resolve a configured mode in either vocabulary to the host enum.
pub fn resolve_mode(value: &str) -> Option<&'static str> {
    if let Some(m) = mode_to_msp(value) {
        return Some(m);
    }
    match value {
        "allowAll" | "promptUnmatched" | "onRequest" | "denyUnmatched" => Some(match value {
            "allowAll" => "allowAll",
            "promptUnmatched" => "promptUnmatched",
            "onRequest" => "onRequest",
            _ => "denyUnmatched",
        }),
        _ => None,
    }
}

pub fn is_reasoning_effort(value: &str) -> bool {
    matches!(
        value,
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "ultra"
    )
}

/// `configOptions`: mode, model, and reasoning selectors. ACP v1 calls the
/// selector key `id`; v2 renamed it to `configId` (the setter still uses
/// `configId` in both versions).
pub fn config_options(
    ver: u8,
    current_mode: &str,
    current_model: &str,
    reasoning_effort: &str,
    models_json: &[(String, String, bool)],
) -> String {
    let mut model_opts = Vec::new();
    for (id, label, _) in models_json {
        model_opts.push(format!("{{\"value\":{},\"name\":{}}}", esc(id), esc(label)));
    }
    let id_key = if ver == 1 { "id" } else { "configId" };
    format!(
        "[{{\"{id_key}\":\"mode\",\"name\":\"Session Mode\",\"description\":\"How the agent handles tool approvals\",\"category\":\"mode\",\"type\":\"select\",\"currentValue\":{},\"options\":[{{\"value\":\"ask\",\"name\":\"Ask\",\"description\":\"Request permission for unmatched tools\"}},{{\"value\":\"auto\",\"name\":\"Auto\",\"description\":\"Allow all tools without asking\"}},{{\"value\":\"deny\",\"name\":\"Deny\",\"description\":\"Deny unmatched tools\"}}]}},{{\"{id_key}\":\"model\",\"name\":\"Model\",\"category\":\"model\",\"type\":\"select\",\"currentValue\":{},\"options\":[{}]}},{{\"{id_key}\":\"reasoning_effort\",\"name\":\"Reasoning Effort\",\"description\":\"Reasoning effort sent with each prompt and steering message\",\"category\":\"thought_level\",\"type\":\"select\",\"currentValue\":{},\"options\":[{{\"value\":\"none\",\"name\":\"None\"}},{{\"value\":\"minimal\",\"name\":\"Minimal\"}},{{\"value\":\"low\",\"name\":\"Low\"}},{{\"value\":\"medium\",\"name\":\"Medium\"}},{{\"value\":\"high\",\"name\":\"High\"}},{{\"value\":\"xhigh\",\"name\":\"Extra High\"}},{{\"value\":\"ultra\",\"name\":\"Ultra\"}}]}}]",
        esc(current_mode),
        esc(current_model),
        model_opts.join(","),
        esc(reasoning_effort)
    )
}

/// Legacy v1 mode state for clients which predate `configOptions`.
pub fn session_modes(current_mode: &str) -> String {
    format!(
        "{{\"currentModeId\":{},\"availableModes\":[{{\"id\":\"ask\",\"name\":\"Ask\",\"description\":\"Request permission for unmatched tools\"}},{{\"id\":\"auto\",\"name\":\"Auto\",\"description\":\"Allow all tools without asking\"}},{{\"id\":\"deny\",\"name\":\"Deny\",\"description\":\"Deny unmatched tools\"}}]}}",
        esc(current_mode)
    )
}

/// Advertise the Muse skills which are useful from an editor session. Commands
/// still travel as ordinary prompts; short aliases are normalized to Muse's
/// stable `/skill <id>` spelling before they reach the host.
fn available_commands_json(ver: u8) -> String {
    let input = |hint: &str| {
        if ver == 1 {
            format!("{{\"hint\":{}}}", esc(hint))
        } else {
            format!("{{\"type\":\"text\",\"hint\":{}}}", esc(hint))
        }
    };
    let commands = [
        (
            "skill",
            "Invoke a Muse skill",
            Some("skill id and optional prompt"),
        ),
        (
            "plan",
            "Create a grounded plan and stop for approval",
            Some("what to plan"),
        ),
        ("compact", "Compact the session context", None),
        (
            "doctor",
            "Diagnose a Muse runtime or session issue",
            Some("symptom or session"),
        ),
        (
            "create-skill",
            "Create a Muse skill",
            Some("what the skill should do"),
        ),
        (
            "create-plugin",
            "Create a Muse plugin",
            Some("what the plugin should do"),
        ),
        (
            "import",
            "Import another agent's session",
            Some("transcript, path, or session id"),
        ),
    ];
    let items = commands
        .into_iter()
        .map(|(name, description, hint)| {
            let input = hint
                .map(|hint| format!(",\"input\":{}", input(hint)))
                .unwrap_or_default();
            format!("{{\"name\":\"{name}\",\"description\":\"{description}\"{input}}}")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
}

pub fn send_available_commands(stdout: &StdoutShared, acp_sid: &str, ver: u8) {
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"available_commands_update\",\"availableCommands\":{}}}}}}}",
            esc(acp_sid),
            available_commands_json(ver)
        ),
    );
}

/// Map one MSP `TodoItem` to an ACP plan entry.
///
/// MSP statuses are a superset (`cancelled` and open values): anything that is
/// not `inProgress`/`completed` stays `pending` so an unknown state can never
/// be presented as finished work.
fn todo_entry(item: &J) -> Option<String> {
    let text = item.get("text").and_then(|v| v.as_str())?;
    if text.trim().is_empty() {
        return None;
    }
    let status = match item.get("status").and_then(|v| v.as_str()).unwrap_or("") {
        "inProgress" => "in_progress",
        "completed" => "completed",
        _ => "pending",
    };
    Some(format!(
        "{{\"content\":{},\"priority\":\"medium\",\"status\":\"{status}\"}}",
        esc(text)
    ))
}

/// Emit an ACP `plan` update from an MSP todo list. The whole list is
/// replaced on every event (and an empty list is a cleared plan, not a
/// no-op), matching both protocols' replace-wholesale semantics.
pub fn send_plan(stdout: &StdoutShared, acp_sid: &str, items: Option<&J>) {
    let entries = match items {
        Some(J::Arr(values)) => values
            .iter()
            .filter_map(todo_entry)
            .collect::<Vec<_>>()
            .join(","),
        // A missing or malformed list carries no authoritative fact; keep the
        // last plan rather than clearing on garbage.
        _ => return,
    };
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{{\"sessionUpdate\":\"plan\",\"entries\":[{}]}}}}}}",
            esc(acp_sid),
            entries
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_and_command_literals_are_valid_json() {
        let models = vec![("fake-model".to_string(), "Fake".to_string(), true)];
        for ver in [1, 2] {
            let options = config_options(ver, "ask", "fake-model", "medium", &models);
            let parsed = crate::json::parse_json(&options).expect("config options JSON");
            let J::Arr(items) = parsed else {
                panic!("config options must be an array");
            };
            assert_eq!(items.len(), 3);

            let commands = available_commands_json(ver);
            let parsed = crate::json::parse_json(&commands).expect("available commands JSON");
            let J::Arr(items) = parsed else {
                panic!("available commands must be an array");
            };
            assert_eq!(items.len(), 7);
        }
        assert!(crate::json::parse_json(&session_modes("ask")).is_ok());
    }
}
