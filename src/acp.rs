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

/// Host-authored title candidates. The adapter never derives a title from
/// transcript items; it only chooses among facts the host explicitly sends.
#[derive(Clone, Default)]
pub struct HostTitleFacts {
    pub name: Option<String>,
    pub title: Option<String>,
    pub first_user_prompt: Option<String>,
}

impl HostTitleFacts {
    pub fn selected(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.title.as_deref())
            .or(self.first_user_prompt.as_deref())
    }
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
    /// Latest goal block as raw MSP JSON (`"null"` after an explicit clear;
    /// `None` before any fact arrives).
    pub goal_meta: Option<String>,
    /// Latest branch observation as raw MSP JSON (`None` before any fact).
    pub branch_meta: Option<String>,
    /// Latest host attention fact as raw MSP JSON (`None` before any fact).
    pub attention_meta: Option<String>,
    /// Host-authored title candidates used for `SessionInfo.title`.
    pub title_facts: HostTitleFacts,
    /// Per-child folds for negotiated native subagent sessions, keyed by the
    /// MSP child session id. Holds replayed child history dedup state.
    pub child_folds: HashMap<String, SessionFold>,
    /// Per-turn `session/tokenUsage` legs, keyed by MSP turn id. Drained when
    /// the turn settles; bounded so turns that never settle cannot grow it
    /// without limit.
    pub turn_usage: Vec<TurnUsage>,
}

pub type Sessions = Arc<Mutex<HashMap<String, AcpSession>>>;

/// How many unsettled turns keep their accumulated legs.
const TURN_USAGE_KEEP: usize = 16;

/// One turn's token counters in ACP v1 `Usage` terms.
///
/// `input` is MSP's counted-once `promptTokens` (cached input already inside
/// it, under the provider's own cache convention) and `output` is
/// `totalTokens - promptTokens`, so reasoning tokens are already inside
/// `output`. The optional members stay `None` until a leg reports them: an
/// absent counter is never sent as zero.
#[derive(Default, Clone)]
pub struct UsageTotals {
    pub total: u64,
    pub input: u64,
    pub output: u64,
    pub thought: Option<u64>,
    pub cached_read: Option<u64>,
    pub cached_write: Option<u64>,
}

fn add_opt(slot: &mut Option<u64>, add: Option<u64>) {
    if let Some(v) = add {
        *slot = Some(slot.unwrap_or(0).saturating_add(v));
    }
}

impl UsageTotals {
    fn add(&mut self, leg: &UsageTotals) {
        self.total = self.total.saturating_add(leg.total);
        self.input = self.input.saturating_add(leg.input);
        self.output = self.output.saturating_add(leg.output);
        add_opt(&mut self.thought, leg.thought);
        add_opt(&mut self.cached_read, leg.cached_read);
        add_opt(&mut self.cached_write, leg.cached_write);
    }

    /// The ACP `Usage` members, without the surrounding braces.
    fn members(&self) -> String {
        let mut s = format!(
            "\"totalTokens\":{},\"inputTokens\":{},\"outputTokens\":{}",
            self.total, self.input, self.output
        );
        for (key, value) in [
            ("thoughtTokens", self.thought),
            ("cachedReadTokens", self.cached_read),
            ("cachedWriteTokens", self.cached_write),
        ] {
            if let Some(v) = value {
                s.push_str(&format!(",\"{key}\":{v}"));
            }
        }
        s
    }
}

/// Accumulated `session/tokenUsage` legs for one MSP turn.
pub struct TurnUsage {
    pub turn_id: String,
    /// Model completions folded in (view-cursor replays excluded).
    pub calls: u64,
    /// Summed `durationMs`; `None` until a leg reports one.
    pub duration_ms: Option<u64>,
    pub totals: UsageTotals,
    /// Per-model breakdown in first-seen order. A leg with no `modelId` is
    /// counted in `totals` and left out here — the adapter never invents a
    /// model name for it.
    pub by_model: Vec<(String, UsageTotals)>,
}

impl TurnUsage {
    pub fn new(turn_id: &str) -> Self {
        Self {
            turn_id: turn_id.to_string(),
            calls: 0,
            duration_ms: None,
            totals: UsageTotals::default(),
            by_model: Vec::new(),
        }
    }

    fn record(&mut self, model: Option<&str>, duration_ms: Option<u64>, leg: &UsageTotals) {
        self.calls = self.calls.saturating_add(1);
        add_opt(&mut self.duration_ms, duration_ms);
        self.totals.add(leg);
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            match self.by_model.iter_mut().find(|(id, _)| id == m) {
                Some((_, totals)) => totals.add(leg),
                None => self.by_model.push((m.to_string(), leg.clone())),
            }
        }
    }

    /// The `"usage":{...}` member for a v1 prompt result, comma-prefixed.
    /// Empty when no leg landed: a turn with no reported usage carries no
    /// usage at all rather than a row of zeros.
    pub fn result_member(&self) -> String {
        if self.calls == 0 {
            return String::new();
        }
        let mut muse = format!("\"modelCalls\":{}", self.calls);
        if let Some(ms) = self.duration_ms {
            muse.push_str(&format!(",\"apiDurationMs\":{ms}"));
        }
        if !self.by_model.is_empty() {
            let per = self
                .by_model
                .iter()
                .map(|(id, t)| format!("{}:{{{}}}", esc(id), t.members()))
                .collect::<Vec<_>>()
                .join(",");
            muse.push_str(&format!(",\"modelUsage\":{{{per}}}"));
        }
        format!(
            ",\"usage\":{{{},\"_meta\":{{\"mjolnir.dev/usage-scope\":\"turn\",\"muse\":{{{muse}}}}}}}",
            self.totals.members()
        )
    }
}

/// Fold one `session/tokenUsage` leg into its turn's accumulator.
///
/// Totals use the server-derived counted-once `promptTokens`/`totalTokens`
/// (MSP SS4.6.5: clients sum these and never re-derive the provider's cache
/// convention); the raw `usage` block supplies only the reasoning and cache
/// counters. `cachedReadTokens` prefers `cacheReadTokens` and falls back to
/// `cachedTokens` for providers that do not split reads from writes; MSP has
/// no second cache-write counter, so `cachedWriteTokens` stays absent unless
/// the host reports one.
pub fn record_turn_leg(s: &mut AcpSession, turn_id: &str, params: &J) {
    if turn_id.is_empty() {
        return;
    }
    let num = |v: Option<&J>| v.and_then(|v| v.as_u64());
    let prompt = num(params.get("promptTokens")).unwrap_or(0);
    let total = num(params.get("totalTokens")).unwrap_or(0);
    let raw = params.get("usage");
    let raw_num = |key: &str| num(raw.and_then(|u| u.get(key)));
    let leg = UsageTotals {
        total,
        input: prompt,
        output: total.saturating_sub(prompt),
        thought: raw_num("reasoningTokens"),
        cached_read: raw_num("cacheReadTokens").or_else(|| raw_num("cachedTokens")),
        cached_write: raw_num("cacheWriteTokens"),
    };
    let model = params.get("modelId").and_then(|v| v.as_str());
    let duration = num(params.get("durationMs"));
    if let Some(entry) = s.turn_usage.iter_mut().find(|t| t.turn_id == turn_id) {
        entry.record(model, duration, &leg);
        return;
    }
    let mut entry = TurnUsage::new(turn_id);
    entry.record(model, duration, &leg);
    s.turn_usage.push(entry);
    if s.turn_usage.len() > TURN_USAGE_KEEP {
        s.turn_usage.remove(0);
    }
}

/// Take a settled turn's accumulated legs, if any arrived.
pub fn take_turn_usage(s: &mut AcpSession, turn_id: &str) -> Option<TurnUsage> {
    let pos = s.turn_usage.iter().position(|t| t.turn_id == turn_id)?;
    Some(s.turn_usage.remove(pos))
}

pub fn send_raw(stdout: &StdoutShared, line: &str) {
    let mut out = stdout.lock().unwrap_or_else(|p| p.into_inner());
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
            ",\"cost\":{{\"amount\":{amount},\"currency\":{},\"source\":\"adapter-estimate\",\"basis\":\"catalog-list-price\",\"billing\":false}}",
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

/// The MSP `ApprovalMode` enum, in the order the selector lists it. The ACP
/// mode ids ARE these names: a client selects one of the host's
/// preconfigured modes and never authors a policy, so renaming them on the
/// editor side only hid one mode (`onRequest`) and confused the other three.
pub const APPROVAL_MODES: [(&str, &str, &str); 4] = [
    ("allowAll", "Allow all", "Allow everything"),
    (
        "promptUnmatched",
        "Prompt unmatched",
        "Prompt on unmatched subjects",
    ),
    ("onRequest", "On request", "Approve only on request"),
    ("denyUnmatched", "Deny unmatched", "Deny unmatched subjects"),
];

/// Host-reported ApprovalMode -> the id we show the client. Identity for
/// the four known modes; an unknown spelling (a future host) falls back to
/// the conservative prompting mode rather than claiming `allowAll`.
pub fn mode_from_msp(mode: &str) -> &'static str {
    APPROVAL_MODES
        .iter()
        .find(|(id, _, _)| *id == mode)
        .map(|(id, _, _)| *id)
        .unwrap_or("promptUnmatched")
}

/// Validate a requested mode: only the MSP `ApprovalMode` spellings are
/// accepted. There is no adapter-side vocabulary.
pub fn resolve_mode(value: &str) -> Option<&'static str> {
    APPROVAL_MODES
        .iter()
        .find(|(id, _, _)| *id == value)
        .map(|(id, _, _)| *id)
}

/// Human-readable list of accepted mode ids for error text.
pub const MODE_HELP: &str = "allowAll|promptUnmatched|onRequest|denyUnmatched";

fn mode_options_json(key: &str) -> String {
    APPROVAL_MODES
        .iter()
        .map(|(id, name, desc)| {
            format!(
                "{{\"{key}\":{},\"name\":{},\"description\":{}}}",
                esc(id),
                esc(name),
                esc(desc)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn is_reasoning_effort(value: &str) -> bool {
    matches!(
        value,
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
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
    recommended_model: Option<&str>,
) -> String {
    let mut model_opts = Vec::new();
    for (id, label, _) in models_json {
        model_opts.push(format!("{{\"value\":{},\"name\":{}}}", esc(id), esc(label)));
    }
    let id_key = if ver == 1 { "id" } else { "configId" };
    // AIR recommendedValue is additive metadata: emit it only when the client
    // negotiated it and the value is present among this selector's options.
    let recommended_meta = match recommended_model {
        Some(model) if models_json.iter().any(|(id, _, _)| id == model) => format!(
            ",\"_meta\":{{\"jetbrains\":{{\"air\":{{\"version\":1,\"recommendedValue\":{}}}}}}}",
            esc(model)
        ),
        _ => String::new(),
    };
    format!(
        "[{{\"{id_key}\":\"mode\",\"name\":\"Approval Mode\",\"description\":\"Muse approval enforcement mode for tool actions\",\"category\":\"mode\",\"type\":\"select\",\"currentValue\":{},\"options\":[{}]}},{{\"{id_key}\":\"model\",\"name\":\"Model\",\"category\":\"model\",\"type\":\"select\",\"currentValue\":{},\"options\":[{}]{recommended_meta}}},{{\"{id_key}\":\"reasoning_effort\",\"name\":\"Reasoning Effort\",\"description\":\"Reasoning effort sent with each prompt and steering message\",\"category\":\"thought_level\",\"type\":\"select\",\"currentValue\":{},\"options\":[{{\"value\":\"none\",\"name\":\"None\"}},{{\"value\":\"minimal\",\"name\":\"Minimal\"}},{{\"value\":\"low\",\"name\":\"Low\"}},{{\"value\":\"medium\",\"name\":\"Medium\"}},{{\"value\":\"high\",\"name\":\"High\"}},{{\"value\":\"xhigh\",\"name\":\"Extra High\"}},{{\"value\":\"max\",\"name\":\"Max\"}},{{\"value\":\"ultra\",\"name\":\"Ultra\"}}]}}]",
        esc(current_mode),
        mode_options_json("value"),
        esc(current_model),
        model_opts.join(","),
        esc(reasoning_effort)
    )
}

/// Legacy v1 mode state for clients which predate `configOptions`.
pub fn session_modes(current_mode: &str) -> String {
    format!(
        "{{\"currentModeId\":{},\"availableModes\":[{}]}}",
        esc(current_mode),
        mode_options_json("id")
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

/// Publish provider-neutral goal state (the presentation codex-acp uses) and
/// a namespaced branch observation through one `session_info_update`.
pub fn send_session_meta(
    stdout: &StdoutShared,
    acp_sid: &str,
    goal: Option<&str>,
    branch: Option<&str>,
) {
    let mut meta = Vec::new();
    if let Some(goal) = goal {
        meta.push(format!("\"goal\":{goal}"));
    }
    if let Some(branch) = branch {
        meta.push(format!("\"muse\":{{\"branch\":{branch}}}"));
    }
    if meta.is_empty() {
        return;
    }
    let update = format!(
        "{{\"sessionUpdate\":\"session_info_update\",\"_meta\":{{{}}}}}",
        meta.join(",")
    );
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{}}}}}",
            esc(acp_sid),
            update
        ),
    );
}

/// Publish a host-authored title change. `None` is an explicit clear so a
/// client can remove a title after the host renames a session back to blank.
pub fn send_session_title(stdout: &StdoutShared, acp_sid: &str, title: Option<&str>) {
    let title = title.map(esc).unwrap_or_else(|| "null".to_string());
    let update = format!("{{\"sessionUpdate\":\"session_info_update\",\"title\":{title}}}");
    send_raw(
        stdout,
        &format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{update}}}}}",
            esc(acp_sid)
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
            let options = config_options(
                ver,
                "promptUnmatched",
                "fake-model",
                "medium",
                &models,
                None,
            );
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
        assert!(crate::json::parse_json(&session_modes("promptUnmatched")).is_ok());
    }

    #[test]
    fn reasoning_effort_admits_the_121_max_tier() {
        for tier in [
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ] {
            assert!(is_reasoning_effort(tier), "{tier} must be selectable");
        }
        assert!(!is_reasoning_effort("extreme"));
        let models = vec![("fake-model".to_string(), "Fake".to_string(), true)];
        let options = config_options(1, "promptUnmatched", "fake-model", "max", &models, None);
        crate::json::parse_json(&options).expect("config options JSON");
        assert!(
            options.contains("\"value\":\"max\""),
            "max tier must be advertised: {options}"
        );
    }
}
