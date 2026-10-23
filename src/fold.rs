//! Fold: per-session MSP view state + mapping to ACP `session/update` payloads.
//!
//! Item kinds come from the host schema (`userMessage`, `agentMessage`,
//! `reasoning`, `toolCall`, …). Unknown kinds render generically: message-like
//! items with text stream as message updates, everything else is ignored.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use crate::json::{J, esc, j_to_string, mint_id, parse_json};

const DEFAULT_MAX_CONTENT: usize = 8000;
const MIN_MAX_CONTENT: usize = 200;

/// Editor-facing output bound, configurable via `MUSE_TOOL_OUTPUT_LIMIT`
/// (characters). Values below the floor are clamped so a typo cannot zero out
/// decision-relevant output.
fn max_content() -> usize {
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("MUSE_TOOL_OUTPUT_LIMIT")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|v| *v >= MIN_MAX_CONTENT)
            .unwrap_or(DEFAULT_MAX_CONTENT)
    })
}

/// Truncate for display and report the fact: the second element carries the
/// fields for `_meta.muse.truncated` when the adapter cut the text.
fn trunc(s: &str) -> (String, Option<String>) {
    let limit = max_content();
    let original = s.chars().count();
    if original <= limit {
        return (s.to_string(), None);
    }
    // The marker is part of the display budget; retainedChars counts only
    // original text. Split scalar values, never bytes, to preserve UTF-8.
    const MARKER: &str = "…[truncated]";
    let retained = limit - MARKER.chars().count();
    let head_chars = retained.div_ceil(2);
    let tail_chars = retained / 2;
    let head = s.char_indices().nth(head_chars).unwrap().0;
    let tail = s.char_indices().rev().nth(tail_chars - 1).unwrap().0;
    let meta = format!(
        "\"source\":\"adapter\",\"originalChars\":{original},\"retainedChars\":{retained},\"headChars\":{head_chars},\"tailChars\":{tail_chars}"
    );
    (format!("{}{MARKER}{}", &s[..head], &s[tail..]), Some(meta))
}

pub(crate) fn tool_kind(tool: &str) -> &'static str {
    let t = tool.to_lowercase();
    // An MCP tool, `mcp__<server>__<tool>`, is classified by its own name:
    // a server named `editor` or `readwise` says nothing about the call.
    let t = t
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
        .map_or(t.as_str(), |(_, name)| name);
    if t.contains("read") || t.contains("list") || t.contains("cat") {
        "read"
    } else if t.contains("write")
        || t.contains("edit")
        || t.contains("patch")
        || t.contains("apply")
    {
        "edit"
    } else if t.contains("bash") || t.contains("shell") || t.contains("exec") || t.contains("run") {
        "execute"
    } else if t.contains("search") || t.contains("grep") || t.contains("glob") || t.contains("find")
    {
        "search"
    } else if t.contains("fetch") || t.contains("web") || t.contains("curl") {
        "fetch"
    } else if t.contains("think") {
        "think"
    } else {
        "other"
    }
}

fn msp_status(s: &str) -> &'static str {
    match s {
        "completed" => "completed",
        "failed" => "failed",
        // ACP (v1 and v2) has no `cancelled` tool status; emitting one makes the
        // whole session/update unparseable on the client, which strands the
        // tool card in_progress. Map it to the nearest legal terminal.
        "cancelled" | "rejected" | "timedOut" => "failed",
        "in_progress" | "inProgress" | "running" | "started" => "in_progress",
        _ => "pending",
    }
}

/// A readable title for a host card whose terminal snapshot carries none, so an
/// announced card still settles instead of stranding in_progress. Splits a
/// camelCase item kind, e.g. `reminderChild` -> `Reminder child`.
fn fallback_card_title(kind: &str) -> String {
    let mut title = String::new();
    for (i, ch) in kind.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                title.push(' ');
            }
            title.push(ch.to_ascii_lowercase());
        } else if i == 0 {
            title.extend(ch.to_uppercase());
        } else {
            title.push(ch);
        }
    }
    if title.is_empty() {
        "Item".to_string()
    } else {
        title
    }
}

/// The monitor description is model-controlled tool input. Keep a readable
/// one-line label by replacing terminal/format controls and collapsing
/// whitespace before it reaches a title or client metadata.
fn sanitize_monitor_description(value: &str) -> String {
    fn unsafe_format(c: char) -> bool {
        c.is_control()
            || matches!(
                c,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            )
    }

    let mut out = String::new();
    let mut pending_space = false;
    for c in value.chars() {
        if c.is_whitespace() || unsafe_format(c) {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

fn monitor_description(item: &J) -> String {
    let raw = item
        .get("monitorDescription")
        .and_then(J::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| item.get("fallbackText").and_then(J::as_str))
        .unwrap_or("");
    let raw = raw.strip_prefix("Monitor event: ").unwrap_or(raw);
    sanitize_monitor_description(raw)
}

enum ItemRole {
    Message {
        msg_id: String,
        streamed: usize,
    },
    Thought {
        msg_id: String,
        streamed: usize,
        part: usize,
    },
    Tool {
        tc_id: String,
        announced: bool,
    },
    Monitor {
        tc_id: String,
        title: String,
        muse_fields: Option<String>,
        text: String,
        host_truncated: bool,
    },
    Ignored,
}

pub struct ToolUpdate<'a> {
    pub create: bool,
    pub tc_id: &'a str,
    /// The host's programmatic tool identity, when one was reported.
    pub name: Option<&'a str>,
    pub title: &'a str,
    pub kind: &'a str,
    pub status: &'a str,
    pub content_text: Option<&'a str>,
    pub raw_input: Option<&'a str>,
    /// ACP `locations` (a JSON array of `{path}`), for follow-along.
    pub locations: Option<&'a str>,
    /// One more tool-call content item after the text, such as a `diff`.
    pub extra_content: Option<&'a str>,
    /// Host-authored item references and patch facts for the card's `_meta`.
    pub muse_fields: Option<&'a str>,
    /// The host already saturated this surface (`item.truncated`).
    pub host_truncated: bool,
}

/// An absolute, lexically normalized path for a tool's path argument, or
/// `None` when it cannot be one (empty, control characters, a relative path
/// without an absolute workspace root, or `..` above the filesystem root).
fn absolute_tool_path(workspace_root: &str, raw: &str) -> Option<String> {
    use std::path::{Component, Path, PathBuf};
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return None;
    }
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        let root = Path::new(workspace_root);
        if !root.is_absolute() {
            return None;
        }
        root.join(raw)
    };
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str())
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
        }
    }
    normalized
        .is_absolute()
        .then(|| normalized.to_str().map(str::to_string))
        .flatten()
}

/// An epoch in milliseconds as a UTC RFC 3339 timestamp, without a date
/// library (Howard Hinnant's civil-from-days).
fn utc_from_epoch_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Native tools whose call runs a shell command.
const SHELL_TOOLS: [&str; 3] = ["bash", "shell", "workspace-shell"];

/// One monitor inbox item: its monitor task and the recorded stamps of a
/// drain and a void, which settle it latest-wins.
#[derive(Default)]
struct InboxItem {
    task_id: Option<String>,
    drained_at: Option<String>,
    voided_at: Option<String>,
}

/// A shell card's command facts, kept for its ACP v2 terminal.
struct ShellCommand {
    tc_id: String,
    command: String,
    cwd: Option<String>,
}

pub struct SessionFold {
    items: HashMap<String, ItemRole>,
    /// Latest owner-side observation for each durable subagent. Controls use
    /// this index so a child session id can never become an alternate command
    /// target.
    subagents: HashMap<String, SubagentObservation>,
    /// Completed item ids (gap-refill replays must not re-announce).
    done: std::collections::HashSet<String>,
    idc: AtomicU64,
    /// Whether the client negotiated the draft ACP subagent extension. When
    /// set, `subagent` items map to spawned/state updates on the parent and a
    /// per-child fold; otherwise they render as legacy tool cards.
    pub native_subagents: bool,
    /// Child session ids already announced to this client (idempotent spawn).
    pub spawned_subagents: std::collections::HashSet<String>,
    /// Whether the client negotiated the AIR async-tasks extension.
    pub air_async_tasks: bool,
    /// ACP v2 always supports standard compaction; ACP v1 must opt in.
    pub compaction_supported: bool,
    /// Async task ids already announced (spawned updates are idempotent).
    pub announced_tasks: std::collections::HashSet<String>,
    /// Workflow async task ids, keyed by the AIR task id (workflowRunId).
    workflow_tasks: HashMap<String, String>,
    /// Latest observation of each running workflow, keyed by
    /// `workflowRunId`. Child and run control read the current attempt and
    /// pause state from here.
    workflow_children: HashMap<String, WorkflowObservation>,
    /// Runs superseded by a newer resumed launch (`resumeFromRunId`). A late
    /// or replayed observation of a superseded run never brings it back.
    superseded_workflows: std::collections::HashSet<String>,
    /// The session's installed hook handlers, keyed by the durable hook key,
    /// as the last `hook/list` read reported them. `hookRun` items use this
    /// to name where their handler came from.
    hook_handlers: HashMap<String, HookHandlerFact>,
    /// Last terminal state sent for each async task.
    /// AIR task id -> MSP item id, used when task control targets the host.
    async_task_msp_ids: HashMap<String, String>,
    /// Last terminal state emitted for each AIR task, suppressing duplicate
    /// item/updated + item/completed deliveries.
    async_task_states: HashMap<String, String>,
    /// Tool-call cards already marked backgrounded in AIR, keyed by the card
    /// id (the item's call id).
    backgrounded_cards: std::collections::HashSet<String>,
    /// Card ids for `reminderChild` items, keyed by reminder agent (or by host
    /// item id when the item carries no agent). Only used when reminder cards
    /// are opted in with `MUSE_SHOW_REMINDERS`: the host mints a new item for
    /// every reminder generation and cancels the previous one, so one card per
    /// agent stops the transcript from growing a row per re-check.
    reminder_cards: HashMap<String, String>,
    /// Opt-in transcript cards for background reminder checks
    /// (`MUSE_SHOW_REMINDERS`). Off by default: reminders hold the turn open
    /// but carry no editor-facing content, so they stay out of the transcript
    /// and only their agent ids reach the stderr diagnostics.
    pub show_reminders: bool,
    /// The session's workspace root, which resolves relative tool paths
    /// into the absolute `locations` ACP follow-along needs.
    pub workspace_root: String,
    /// ACP v2 `diff` content fetched for a completed tool's stored patch,
    /// keyed by item id and taken once by that item's completion card.
    pub patch_diffs: HashMap<String, String>,
    /// ACP `terminal` content an editor tool call (bridge.rs) showed, keyed
    /// by item id and taken once by that item's completion card, which shows
    /// the terminal instead of the output text.
    pub editor_terminals: HashMap<String, String>,
    /// Announced, running editor tool calls in start order: item id, card
    /// id, tool, and arguments, so the bridge can find a call's card.
    editor_calls: Vec<(String, String, String, J)>,
    /// Characters of streamed tool output already appended per item id, so
    /// appends stop at the display budget the completion card applies.
    streamed_output: HashMap<String, usize>,
    /// ACP v2 shell cards: the command facts noted from the card, keyed by
    /// item id, for the agent-owned terminal opened once it runs.
    shell_commands: HashMap<String, ShellCommand>,
    /// Open agent-owned terminals by item id: the terminal id and the
    /// characters of output already appended.
    terminals: HashMap<String, (String, usize)>,
    /// ACP monitor cards already announced for a durable monitor task id.
    /// Later feed items update the same row instead of minting one per event
    /// batch.
    monitor_cards: std::collections::HashSet<String>,
    /// Muse 1.4.11 monitor inbox items by inbox item id: the owning monitor
    /// task and the recorded drain and void stamps, if any.
    monitor_inbox: HashMap<String, InboxItem>,
    /// The open "background checks" card of a turn whose foreground work is
    /// done while reminder agents hold it open (`turn/foregroundCompleted`),
    /// as `(turn id, card id)`. Settled at the turn's terminal.
    background_checks: Option<(String, String)>,
    /// The turn whose `_meta.muse.modelRetry` hint is showing
    /// (`turn/modelRetryScheduled`). Withdrawn at the turn's terminal.
    model_retry: Option<String>,
    /// The latest settled agent message as (MSP item id, ACP message id). A
    /// cut-off reply is the last one of its failed run, so a late
    /// `item/updated` carrying `cutOff` can only name this message.
    last_message: Option<(String, String)>,
    /// Agent messages already marked cut off. The host never clears the
    /// flag, so the mark is sent once.
    cut_off: std::collections::HashSet<String>,
}

/// One child of a running workflow, as the latest workflow item reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowChild {
    pub workflow_run_id: String,
    pub child_id: String,
    pub attempt: u64,
    pub status: String,
    pub label: String,
}

/// One live workflow run, as the latest workflow item reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowRun {
    pub workflow_run_id: String,
    /// `true` while the host's durable pause fact is in force.
    pub paused: bool,
    pub child_count: usize,
}

/// The adapter's latest view of one live workflow run.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkflowObservation {
    revision: u64,
    paused: bool,
    children: Vec<WorkflowChild>,
}

/// One installed hook handler from `hook/list`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HookHandlerFact {
    handler_kind: String,
    enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentObservation {
    pub item_status: String,
    pub control_status: String,
    pub child_session_id: String,
}

impl SessionFold {
    pub fn new() -> Self {
        Self {
            workspace_root: String::new(),
            patch_diffs: HashMap::new(),
            editor_terminals: HashMap::new(),
            editor_calls: Vec::new(),
            streamed_output: HashMap::new(),
            shell_commands: HashMap::new(),
            terminals: HashMap::new(),
            items: HashMap::new(),
            subagents: HashMap::new(),
            done: std::collections::HashSet::new(),
            idc: AtomicU64::new(1),
            native_subagents: false,
            spawned_subagents: std::collections::HashSet::new(),
            air_async_tasks: false,
            compaction_supported: false,
            announced_tasks: std::collections::HashSet::new(),
            workflow_tasks: HashMap::new(),
            workflow_children: HashMap::new(),
            superseded_workflows: std::collections::HashSet::new(),
            hook_handlers: HashMap::new(),
            async_task_msp_ids: HashMap::new(),
            async_task_states: HashMap::new(),
            backgrounded_cards: std::collections::HashSet::new(),
            reminder_cards: HashMap::new(),
            show_reminders: false,
            monitor_cards: std::collections::HashSet::new(),
            monitor_inbox: HashMap::new(),
            background_checks: None,
            model_retry: None,
            last_message: None,
            cut_off: std::collections::HashSet::new(),
        }
    }

    fn observe_subagent(&mut self, item: &J) {
        if item.get("kind").and_then(|v| v.as_str()) != Some("subagent") {
            return;
        }
        let Some(subagent_id) = item.get("subagentId").and_then(|v| v.as_str()) else {
            return;
        };
        if subagent_id.is_empty() {
            return;
        }
        self.subagents.insert(
            subagent_id.to_string(),
            SubagentObservation {
                item_status: item
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("inProgress")
                    .to_string(),
                control_status: item
                    .get("controlStatus")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                child_session_id: item
                    .get("childSessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }

    /// Track the children of a running workflow from its item snapshots. A
    /// terminal item (or a completed delivery) drops the run, and an older
    /// revision never replaces a newer one. Children without an id or a valid
    /// attempt (integer >= 1) are not controllable and are skipped. A
    /// resumed launch (`resumeFromRunId`) supersedes the paused run it names.
    fn observe_workflow(&mut self, item: &J, completed: bool) {
        if item.get("kind").and_then(|v| v.as_str()) != Some("workflow") {
            return;
        }
        let Some(run_id) = item
            .get("workflowRunId")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        if self.superseded_workflows.contains(run_id) {
            return;
        }
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if completed || matches!(status, "completed" | "failed" | "cancelled") {
            self.workflow_children.remove(run_id);
            return;
        }
        // A resumed launch is the successor generation: the paused run it
        // names is no longer live (ADR 45437 D2/D3), so a late observation
        // of it must never re-add it to the control listings. Muse 1.4.6
        // reuses the run id for the resumed generation, so a same-id resume
        // is a generation boundary of one run rather than a supersession.
        let resumed_from = item
            .get("resumeFromRunId")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty());
        if let Some(predecessor) = resumed_from
            && predecessor != run_id
        {
            self.workflow_children.remove(predecessor);
            self.superseded_workflows.insert(predecessor.to_string());
        }
        let revision = item.get("revision").and_then(|v| v.as_u64()).unwrap_or(0);
        // A resumed generation restarts the item's revision sequence under
        // the same run id, so the resume fact itself is its ordering guard.
        if resumed_from.is_none()
            && self
                .workflow_children
                .get(run_id)
                .is_some_and(|seen| seen.revision > revision)
        {
            return;
        }
        let paused = item
            .get("paused")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let children = match item.get("children") {
            Some(J::Arr(children)) => children
                .iter()
                .filter_map(|child| {
                    let child_id = child.get("childId")?.as_str()?;
                    let attempt = child.get("attempt")?.as_u64()?;
                    (!child_id.is_empty() && attempt >= 1).then(|| WorkflowChild {
                        workflow_run_id: run_id.to_string(),
                        child_id: child_id.to_string(),
                        attempt,
                        status: child
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        label: child
                            .get("label")
                            .and_then(|v| v.as_str())
                            .unwrap_or("child")
                            .to_string(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        self.workflow_children.insert(
            run_id.to_string(),
            WorkflowObservation {
                revision,
                paused,
                children,
            },
        );
    }

    /// Children of every running workflow, ordered by run id and then by
    /// the host's child order.
    pub fn workflow_children(&self) -> Vec<WorkflowChild> {
        let mut runs: Vec<_> = self.workflow_children.iter().collect();
        runs.sort_by(|a, b| a.0.cmp(b.0));
        runs.into_iter()
            .flat_map(|(_, seen)| seen.children.iter().cloned())
            .collect()
    }

    /// Every live workflow run, ordered by run id, with its pause state.
    pub fn workflow_runs(&self) -> Vec<WorkflowRun> {
        let mut runs: Vec<_> = self.workflow_children.iter().collect();
        runs.sort_by(|a, b| a.0.cmp(b.0));
        runs.into_iter()
            .map(|(run_id, seen)| WorkflowRun {
                workflow_run_id: run_id.clone(),
                paused: seen.paused,
                child_count: seen.children.len(),
            })
            .collect()
    }

    /// Replace the session's hook inventory from a `hook/list` result. The
    /// host promises the set, not an order, and non-enabled rows are included
    /// as `false` rather than dropped. Rows without a key are skipped.
    pub fn note_hook_inventory(&mut self, result: &J) {
        let mut handlers = HashMap::new();
        if let Some(J::Arr(hooks)) = result.get("hooks") {
            for hook in hooks {
                let Some(key) = hook
                    .get("key")
                    .and_then(|v| v.as_str())
                    .filter(|key| !key.is_empty())
                else {
                    continue;
                };
                handlers.insert(
                    key.to_string(),
                    HookHandlerFact {
                        handler_kind: hook
                            .get("handlerKind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        enabled: matches!(hook.get("enabled"), Some(J::Bool(true))),
                    },
                );
            }
        }
        self.hook_handlers = handlers;
    }

    /// Return the latest owner-side observation for a durable child.
    pub fn subagent(&self, subagent_id: &str) -> Option<SubagentObservation> {
        self.subagents.get(subagent_id).cloned()
    }

    /// Find the durable child id for a child transcript session.
    pub fn subagent_id_for_child(&self, child_session_id: &str) -> Option<String> {
        self.subagents
            .iter()
            .find(|(_, observation)| observation.child_session_id == child_session_id)
            .map(|(id, _)| id.clone())
    }

    /// Resolve an AIR async task to a workflow run. Shell task ids do not
    /// resolve here.
    pub fn workflow_run_id_for_task(&self, task_id: &str) -> Option<String> {
        self.air_async_tasks
            .then_some(())
            .and_then(|_| self.workflow_tasks.get(task_id).cloned())
    }

    /// Seed one AIR async task from a `task/list` row. A row whose host task
    /// id is already known (announced from item events or an earlier seed) is
    /// left to those events. The host task id becomes both the AIR task id
    /// and the MSP task id, so a stop and a later item terminal resolve to the
    /// same task. Returns true when the row was announced.
    ///
    /// Muse 1.4.12 also lists foreground work, marked `executionMode:
    /// "foreground"`; that is the running turn's own tool call, not background
    /// work, so it is skipped. An absent mode means background (older hosts
    /// list background rows only), and an unknown mode is ignored as the
    /// schema requires, which leaves the row background too.
    pub fn seed_async_task(&mut self, acp_sid: &str, row: &J, out: &mut Vec<String>) -> bool {
        if !self.air_async_tasks {
            return false;
        }
        if row.get("executionMode").and_then(|v| v.as_str()) == Some("foreground") {
            return false;
        }
        let Some(task_id) = row
            .get("taskId")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
        else {
            return false;
        };
        if self.announced_tasks.contains(task_id)
            || self.async_task_states.contains_key(task_id)
            || self.async_task_msp_ids.values().any(|msp| msp == task_id)
        {
            return false;
        }
        let tool = row.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let name = if !tool.is_empty() {
            tool.to_string()
        } else if !kind.is_empty() {
            kind.to_string()
        } else {
            "task".to_string()
        };
        // Item-visible tool rows are shell-family work; registry-only rows
        // carry the host's own kind string.
        let task_type = if kind == "tool" || kind.starts_with("tool.") || kind.is_empty() {
            "shell"
        } else {
            kind
        };
        self.async_task_msp_ids
            .insert(task_id.to_string(), task_id.to_string());
        self.announced_tasks.insert(task_id.to_string());
        out.push(Self::async_task_spawned_line(
            acp_sid, task_id, &name, task_type, true, None,
        ));
        true
    }

    /// The AIR async-task id already announced for a host item id (from a
    /// `task/list` seed), or `fallback` when none exists. Reusing the seeded
    /// id keeps one AIR task per host task when the item's own events arrive
    /// later.
    fn async_task_id_for(&self, msp_task_id: &str, fallback: &str) -> String {
        self.async_task_msp_ids
            .iter()
            .find(|(_, msp)| msp.as_str() == msp_task_id)
            .map(|(air, _)| air.clone())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Resolve the adapter-facing AIR task id to MSP's durable item id.
    pub fn msp_task_id(&self, async_task_id: &str) -> Option<&str> {
        self.async_task_msp_ids
            .get(async_task_id)
            .map(String::as_str)
    }

    pub fn has_active_item(&self, item_id: &str) -> bool {
        self.items.contains_key(item_id)
    }

    /// The ACP tool-call id already assigned to a toolCall item, so an
    /// approval for it can point at the same card.
    /// The announced card of a running editor tool call: the one with
    /// `call_id` when Muse sent it, else the oldest `tool` call whose `key`
    /// argument is `value`.
    pub fn editor_card(
        &self,
        call_id: Option<&str>,
        tool: &str,
        key: &str,
        value: &str,
    ) -> Option<String> {
        self.editor_calls
            .iter()
            .find(|(_, tc_id, name, args)| match call_id {
                Some(call_id) => tc_id == call_id,
                None => name == tool && args.get(key).and_then(J::as_str) == Some(value),
            })
            .map(|(_, tc_id, _, _)| tc_id.clone())
    }

    pub fn tool_call_id(&self, item_id: &str) -> Option<&str> {
        match self.items.get(item_id) {
            Some(ItemRole::Tool { tc_id, .. }) => Some(tc_id),
            _ => None,
        }
    }

    /// Whether work this session started may still run on its host after the
    /// turn: a tool call, subagent, or workflow not yet reported finished.
    pub fn has_running_work(&self) -> bool {
        self.items
            .values()
            .any(|role| matches!(role, ItemRole::Tool { .. }))
            || self.subagents.values().any(|s| {
                !matches!(
                    s.item_status.as_str(),
                    "completed" | "failed" | "rejected" | "cancelled" | "timedOut"
                )
            })
            || !self.workflow_children.is_empty()
    }

    fn known(&self, item_id: &str) -> bool {
        self.done.contains(item_id)
    }

    fn update_line(acp_sid: &str, update_json: &str) -> String {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":{},\"update\":{}}}}}",
            esc(acp_sid),
            update_json
        )
    }

    fn chunk(acp_sid: &str, ver: u8, msg_id: &str, text: &str) -> String {
        let content = format!("{{\"type\":\"text\",\"text\":{}}}", esc(text));
        let update = if ver == 2 {
            format!(
                "{{\"sessionUpdate\":\"agent_message_chunk\",\"messageId\":{},\"content\":{}}}",
                esc(msg_id),
                content
            )
        } else {
            format!(
                "{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{}}}",
                content
            )
        };
        Self::update_line(acp_sid, &update)
    }

    /// `Item.cutOff`: the provider stream died mid-reply and the run failed
    /// with this partial text committed. Only an explicit `true` counts; the
    /// adapter never infers the mark.
    fn is_cut_off(item: &J) -> bool {
        item.get("kind").and_then(|v| v.as_str()) == Some("agentMessage")
            && matches!(item.get("cutOff"), Some(J::Bool(true)))
    }

    /// `Item.messagePhase` (Muse 1.4.8): the phase the model declared on
    /// the commit — running commentary or the final answer. Only an
    /// explicit `"commentary"` counts; an absent or unknown value reads as
    /// `"finalAnswer"`. Never inferred from position, and never a
    /// turn-done signal.
    fn message_phase(item: &J) -> &'static str {
        if item.get("kind").and_then(|v| v.as_str()) == Some("agentMessage")
            && item.get("messagePhase").and_then(|v| v.as_str()) == Some("commentary")
        {
            "commentary"
        } else {
            "finalAnswer"
        }
    }

    /// The `_meta.muse` member marking a completed agent reply: its message
    /// phase, plus the cut-off mark when the host committed a partial reply.
    /// Ready to append to an update object. `None` on both sides means the
    /// update carries no mark at all.
    fn message_meta(phase: Option<&str>, cut_off: Option<&str>) -> Option<String> {
        let mut fields = String::new();
        if let Some(phase) = phase {
            fields.push_str(&format!("\"messagePhase\":{}", esc(phase)));
        }
        if let Some(item_id) = cut_off {
            if !fields.is_empty() {
                fields.push(',');
            }
            fields.push_str(&format!("\"cutOff\":true,\"itemId\":{}", esc(item_id)));
        }
        (!fields.is_empty()).then(|| format!(",\"_meta\":{{\"muse\":{{{fields}}}}}"))
    }

    /// An agent-message chunk carrying the completed reply's `_meta.muse`
    /// mark, so an editor can tell commentary from the final answer (and a
    /// truncated reply from a complete one). `text` is whatever the
    /// completion still has to show; empty when the deltas carried it.
    fn marked_chunk(acp_sid: &str, ver: u8, msg_id: &str, text: &str, meta: &str) -> String {
        let mut line = Self::chunk(acp_sid, ver, msg_id, text);
        // The chunk is the update object followed by the two closing braces
        // of `params` and the envelope.
        line.insert_str(line.len() - 3, meta);
        line
    }

    /// Claim the one-time mark for a cut-off reply about to be shown.
    fn claim_cut_off(&mut self, item: &J, item_id: &str) -> bool {
        Self::is_cut_off(item) && self.cut_off.insert(item_id.to_string())
    }

    /// Mark a reply the client already shows as cut off, once. A reply with
    /// no text shown yet gets the mark with the text that shows it. A settled
    /// reply can only be the latest one; on ACP v1, whose chunks attach by
    /// position, only while no newer message is streaming.
    fn mark_cut_off(&mut self, acp_sid: &str, ver: u8, item: &J, out: &mut Vec<String>) {
        let Some(item_id) = item.get("itemId").and_then(|v| v.as_str()) else {
            return;
        };
        if !Self::is_cut_off(item) || self.cut_off.contains(item_id) {
            return;
        }
        let msg_id = match self.items.get(item_id) {
            Some(ItemRole::Message { msg_id, streamed }) if *streamed > 0 => msg_id.clone(),
            Some(_) => return,
            None => match &self.last_message {
                Some((id, msg_id)) if id == item_id && (ver == 2 || !self.message_streaming()) => {
                    msg_id.clone()
                }
                _ => return,
            },
        };
        self.cut_off.insert(item_id.to_string());
        // The mark never declares a phase: on a settled item the completion
        // already emitted it, and on an in-flight item the model has not
        // committed one yet — inferring `finalAnswer` here would contradict
        // the completion's later declaration.
        let meta = Self::message_meta(None, Some(item_id))
            .expect("a cut-off mark always carries its cut-off");
        out.push(Self::marked_chunk(acp_sid, ver, &msg_id, "", &meta));
    }

    fn message_streaming(&self) -> bool {
        self.items
            .values()
            .any(|role| matches!(role, ItemRole::Message { streamed, .. } if *streamed > 0))
    }

    fn thought_chunk(acp_sid: &str, ver: u8, msg_id: &str, text: &str) -> String {
        let content = format!("{{\"type\":\"text\",\"text\":{}}}", esc(text));
        let update = if ver == 2 {
            format!(
                "{{\"sessionUpdate\":\"agent_thought_chunk\",\"messageId\":{},\"content\":{}}}",
                esc(msg_id),
                content
            )
        } else {
            format!(
                "{{\"sessionUpdate\":\"agent_thought_chunk\",\"content\":{}}}",
                content
            )
        };
        Self::update_line(acp_sid, &update)
    }

    /// A synthetic tool card for host-side work that is not a model tool
    /// call (subagents, workflows, the user shell, unknown future kinds).
    /// `_meta` carries the MSP facts so support can see the source item.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn card_line(
        acp_sid: &str,
        ver: u8,
        create: bool,
        tc_id: &str,
        title: &str,
        kind: &str,
        status: &str,
        content_text: Option<&str>,
        muse_fields: Option<&str>,
        host_truncated: bool,
    ) -> String {
        let session_update = if ver == 2 || !create {
            "tool_call_update"
        } else {
            "tool_call"
        };
        let mut f = vec![
            format!("\"sessionUpdate\":\"{session_update}\""),
            format!("\"toolCallId\":{}", esc(tc_id)),
            format!("\"title\":{}", esc(title)),
            format!("\"kind\":\"{kind}\""),
            format!("\"status\":{}", esc(status)),
        ];
        let mut fields = muse_fields.unwrap_or_default().to_string();
        if let Some(t) = content_text {
            let (text, adapter_cut) = trunc(t);
            f.push(format!(
                "\"content\":[{{\"type\":\"content\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}]",
                esc(&text)
            ));
            if let Some(cut) = adapter_cut.filter(|_| !host_truncated) {
                if !fields.is_empty() {
                    fields.push(',');
                }
                fields.push_str(&format!("\"truncated\":{{{cut}}}"));
            }
        }
        if host_truncated {
            if !fields.is_empty() {
                fields.push(',');
            }
            fields.push_str("\"truncated\":{\"source\":\"host\"}");
        }
        if !fields.is_empty() {
            f.push(format!("\"_meta\":{{\"muse\":{{{fields}}}}}"));
        }
        Self::update_line(acp_sid, &format!("{{{}}}", f.join(",")))
    }

    /// Preserve stable host references beside the bounded editor surface.
    /// `outputRef` and `patchRef` are intentionally passed through as JSON so
    /// clients can use their availability, byte length, and host id.
    fn item_muse_fields(item: &J) -> Option<String> {
        let fields = [
            ("itemId", item.get("itemId")),
            ("outputRef", item.get("outputRef")),
            ("patchRef", item.get("patchRef")),
            ("patchSummary", item.get("patchSummary")),
        ];
        let parts: Vec<String> = fields
            .into_iter()
            .filter_map(|(key, value)| {
                value
                    .filter(|v| !matches!(v, J::Null))
                    .map(|v| format!("\"{key}\":{}", j_to_string(v)))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(","))
    }

    fn merge_muse_fields(base: Option<&str>, item: Option<&str>) -> Option<String> {
        let mut fields = String::new();
        if let Some(item) = item.filter(|value| !value.is_empty()) {
            fields.push_str(item);
        }
        if let Some(base) = base.filter(|value| !value.is_empty()) {
            if !fields.is_empty() {
                fields.push(',');
            }
            fields.push_str(base);
        }
        (!fields.is_empty()).then_some(fields)
    }

    fn compaction_status(item: &J) -> &'static str {
        let status = item.get("status").and_then(J::as_str).unwrap_or("");
        if matches!(status, "inProgress" | "in_progress" | "running" | "started") {
            return "in_progress";
        }
        if status == "cancelled" {
            return "cancelled";
        }
        match item.get("outcome").and_then(J::as_str).unwrap_or("") {
            "compacted" => "completed",
            "noop" => "_museNoop",
            "cancelled" => "cancelled",
            "failed" => "failed",
            _ if status == "completed" => "completed",
            _ if status == "failed" || status == "rejected" || status == "timedOut" => "failed",
            _ => "_museUnknown",
        }
    }

    fn compaction_meta(item: &J) -> String {
        let mut fields = vec!["\"version\":1".to_string()];
        for key in [
            "status",
            "outcome",
            "reason",
            "trigger",
            "strategyId",
            "summarizedThrough",
        ] {
            if let Some(value) = item.get(key).and_then(J::as_str) {
                fields.push(format!("\"{key}\":{}", esc(value)));
            }
        }
        for key in ["tokensBefore", "tokensAfter"] {
            if let Some(value) = item.get(key).and_then(J::as_u64) {
                fields.push(format!("\"{key}\":{value}"));
            }
        }
        fields.join(",")
    }

    fn standard_compaction_line(acp_sid: &str, item_id: &str, item: &J) -> String {
        let mut fields = vec![
            "\"sessionUpdate\":\"compaction_update\"".to_string(),
            format!("\"compactionId\":{}", esc(item_id)),
            format!("\"status\":\"{}\"", Self::compaction_status(item)),
        ];
        if Self::compaction_status(item) == "failed"
            && let Some(reason) = item
                .get("reason")
                .and_then(J::as_str)
                .filter(|reason| !reason.is_empty())
        {
            fields.push(format!("\"error\":{}", esc(reason)));
        }
        fields.push(format!(
            "\"_meta\":{{\"muse\":{{\"contextCompaction\":{{{}}}}}}}",
            Self::compaction_meta(item)
        ));
        Self::update_line(acp_sid, &format!("{{{}}}", fields.join(",")))
    }

    /// Older ACP v1 clients did not negotiate standard compaction. Present the
    /// same host work as a think-kind tool call with provenance metadata.
    fn legacy_compaction_line(
        acp_sid: &str,
        ver: u8,
        item_id: &str,
        status: &str,
        content_text: Option<&str>,
    ) -> String {
        let session_update = if ver == 2 || status != "in_progress" {
            "tool_call_update"
        } else {
            "tool_call"
        };
        let mut f = vec![
            format!("\"sessionUpdate\":\"{session_update}\""),
            format!("\"toolCallId\":{}", esc(&format!("compact-{item_id}"))),
            "\"title\":\"Compact conversation\"".to_string(),
            "\"kind\":\"think\"".to_string(),
            format!("\"status\":{}", esc(status)),
        ];
        if let Some(t) = content_text {
            f.push(format!(
                "\"content\":[{{\"type\":\"content\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}]",
                esc(t)
            ));
        }
        f.push("\"_meta\":{\"contextCompaction\":{\"version\":1}}".to_string());
        Self::update_line(acp_sid, &format!("{{{}}}", f.join(",")))
    }

    fn compaction_content(item: &J) -> Option<String> {
        let outcome = item.get("outcome").and_then(|v| v.as_str()).unwrap_or("");
        match outcome {
            "compacted" => {
                let before = item.get("tokensBefore").and_then(|v| v.as_u64());
                let after = item.get("tokensAfter").and_then(|v| v.as_u64());
                match (before, after) {
                    (Some(b), Some(a)) => Some(format!("Context compacted ({b} → {a} tokens)")),
                    _ => Some("Context compacted".to_string()),
                }
            }
            "noop" => Some(format!(
                "Nothing to compact ({})",
                item.get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("noop")
            )),
            "failed" | "cancelled" => Some(format!(
                "Compaction {outcome}: {}",
                item.get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no reason given")
            )),
            // In progress or an unknown outcome: the host has not spoken yet.
            _ => None,
        }
    }

    /// Build (title, content, `_meta` JSON) for host-side item kinds that
    /// render as synthetic tool cards. `meta` is a complete JSON object.
    fn host_card_parts(&self, kind: &str, item: &J) -> (String, Option<String>, Option<String>) {
        let str_field = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or("");
        let meta_obj = |namespace: &str, fields: Vec<(&str, &J)>| {
            let parts: Vec<String> = fields
                .into_iter()
                .filter(|(_, v)| !matches!(v, J::Null))
                .map(|(k, v)| format!("\"{k}\":{}", j_to_string(v)))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(format!("\"{namespace}\":{{{}}}", parts.join(",")))
            }
        };
        match kind {
            "subagent" => {
                let agent = str_field("agentPath");
                let objective = str_field("objective");
                let title = match (agent.is_empty(), objective.is_empty()) {
                    (false, false) => format!("{agent}: {objective}"),
                    (false, true) => agent.to_string(),
                    (true, false) => objective.to_string(),
                    (true, true) => str_field("fallbackText").to_string(),
                };
                // Muse 1.4.11 says when the parent consumed the result
                // (`result.drained`, never cleared once set). The card's
                // completion still follows the item's own status; the join
                // is the host's fact to report, never derived here.
                let drained = item
                    .get("result")
                    .and_then(|r| r.get("drained"))
                    .is_some_and(|v| matches!(v, J::Bool(true)));
                let content = item
                    .get("result")
                    .and_then(|r| r.get("summary"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        let failure = str_field("failureReason");
                        (!failure.is_empty()).then_some(failure.to_string())
                    });
                let content = if drained {
                    Some(match content {
                        Some(text) => format!("{text}\nDelivered to the parent agent."),
                        None => "Delivered to the parent agent.".to_string(),
                    })
                } else {
                    content
                };
                let drained_fact = J::Bool(true);
                let meta = meta_obj(
                    "subagent",
                    vec![
                        ("subagentId", item.get("subagentId").unwrap_or(&J::Null)),
                        (
                            "childSessionId",
                            item.get("childSessionId").unwrap_or(&J::Null),
                        ),
                        (
                            "controlStatus",
                            item.get("controlStatus").unwrap_or(&J::Null),
                        ),
                        ("depth", item.get("depth").unwrap_or(&J::Null)),
                        ("drained", if drained { &drained_fact } else { &J::Null }),
                    ],
                );
                (title, content, meta)
            }
            "workflow" => {
                let entry = str_field("entryId");
                let script = str_field("scriptId");
                let mut title = if !entry.is_empty() {
                    format!("Workflow {entry}")
                } else if !script.is_empty() {
                    format!("Workflow {script}")
                } else {
                    "Workflow".to_string()
                };
                if item.get("paused").and_then(|v| v.as_bool()) == Some(true) {
                    title.push_str(" (paused)");
                }
                let content = match item.get("children") {
                    Some(J::Arr(children)) if !children.is_empty() => {
                        let lines: Vec<String> = children
                            .iter()
                            .map(|c| {
                                let label =
                                    c.get("label").and_then(|v| v.as_str()).unwrap_or("child");
                                // The id is what `/workflow-child` takes.
                                let label = match c.get("childId").and_then(|v| v.as_str()) {
                                    Some(id) if !id.is_empty() => format!("{label} [{id}]"),
                                    _ => label.to_string(),
                                };
                                let status = c
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let phase = c.get("phase").and_then(|v| v.as_str());
                                match phase {
                                    Some(phase) => format!("{label}: {status} ({phase})"),
                                    None => format!("{label}: {status}"),
                                }
                            })
                            .collect();
                        Some(lines.join("\n"))
                    }
                    _ => None,
                };
                let meta = meta_obj(
                    "workflow",
                    vec![
                        (
                            "workflowRunId",
                            item.get("workflowRunId").unwrap_or(&J::Null),
                        ),
                        ("entryId", item.get("entryId").unwrap_or(&J::Null)),
                        ("scriptId", item.get("scriptId").unwrap_or(&J::Null)),
                        (
                            "triggerSource",
                            item.get("triggerSource").unwrap_or(&J::Null),
                        ),
                        ("paused", item.get("paused").unwrap_or(&J::Null)),
                        (
                            "resumeFromRunId",
                            item.get("resumeFromRunId").unwrap_or(&J::Null),
                        ),
                    ],
                );
                (title, content, meta)
            }
            "hookRun" => {
                let key = str_field("hookKey");
                let label = str_field("label");
                let title = match (label.is_empty(), key.is_empty()) {
                    (false, _) => format!("Hook {label}"),
                    (true, false) => format!("Hook {key}"),
                    (true, true) => "Hook".to_string(),
                };
                let reason = str_field("reason");
                let content = (!reason.is_empty()).then(|| reason.to_string());
                // The inventory names where the handler came from; a hook run
                // whose row is not in the last read stays honesty-unknown.
                let handler = self.hook_handlers.get(key).filter(|_| !key.is_empty());
                let handler_kind = handler
                    .map(|h| J::Str(h.handler_kind.clone()))
                    .unwrap_or(J::Null);
                let enabled = handler.map(|h| J::Bool(h.enabled)).unwrap_or(J::Null);
                let meta = meta_obj(
                    "hookRun",
                    vec![
                        ("hookKey", item.get("hookKey").unwrap_or(&J::Null)),
                        ("event", item.get("event").unwrap_or(&J::Null)),
                        ("runStatus", item.get("runStatus").unwrap_or(&J::Null)),
                        ("reason", item.get("reason").unwrap_or(&J::Null)),
                        ("handlerKind", &handler_kind),
                        ("enabled", &enabled),
                    ],
                );
                (title, content, meta)
            }
            "userShell" => {
                let command = str_field("commandText");
                let title = if command.is_empty() {
                    "User shell".to_string()
                } else {
                    command.to_string()
                };
                let mut content = str_field("visibleOutput").to_string();
                // Exit facts are verbatim host facts: code and signal stay
                // distinct, and signal numbers are never mapped to names.
                if let Some(code) = item.get("exitCode").and_then(|v| v.as_u64()) {
                    let fact = format!("exited with code {code}");
                    if content.is_empty() {
                        content = fact;
                    } else {
                        content = format!("{content}\n{fact}");
                    }
                } else if let Some(signal) = item.get("exitSignal").and_then(|v| v.as_u64()) {
                    let fact = format!("terminated by signal {signal}");
                    if content.is_empty() {
                        content = fact;
                    } else {
                        content = format!("{content}\n{fact}");
                    }
                }
                let content = (!content.is_empty()).then_some(content);
                let meta = meta_obj(
                    "userShell",
                    vec![
                        ("exitCode", item.get("exitCode").unwrap_or(&J::Null)),
                        ("exitSignal", item.get("exitSignal").unwrap_or(&J::Null)),
                    ],
                );
                (title, content, meta)
            }
            "reminderChild" => {
                let agent = str_field("reminderAgentId");
                let task = str_field("taskId");
                let child = str_field("childSessionId");
                let log = str_field("childSessionLogPath");
                let generation = item.get("generationId").and_then(|v| v.as_u64());
                let fallback = str_field("fallbackText");
                // The title names the agent (or task) and stays stable across
                // generations. The host re-checks the same reminder agent
                // repeatedly, and a per-generation title would append a row
                // for every re-check instead of updating the agent's one row.
                let title = match (agent.is_empty(), task.is_empty()) {
                    (false, _) => format!("Reminder from {agent}"),
                    (true, false) => format!("Reminder {task}"),
                    _ if !fallback.is_empty() => fallback.to_string(),
                    _ if generation.is_some() || !child.is_empty() || !log.is_empty() => {
                        fallback_card_title(kind)
                    }
                    _ => String::new(),
                };
                let mut lines = Vec::new();
                // The legacy generic placeholder carries no signal once the
                // title names the agent or task; any other distinct summary
                // is kept as the first content line.
                if !fallback.is_empty() && fallback != title && fallback != "Reminder child session"
                {
                    lines.push(fallback.to_string());
                }
                // The generation keeps the 0.11.0 attribution the title used
                // to carry, without minting a row per generation.
                if let Some(num) = generation {
                    lines.push(format!("generation: {num}"));
                }
                if !task.is_empty() {
                    lines.push(format!("task: {task}"));
                }
                if !child.is_empty() {
                    lines.push(format!("child session: {child}"));
                }
                if !log.is_empty() {
                    lines.push(format!("log: {log}"));
                }
                let content = (!lines.is_empty()).then(|| lines.join("\n"));
                let meta = meta_obj(
                    "reminderChild",
                    vec![
                        (
                            "reminderAgentId",
                            item.get("reminderAgentId").unwrap_or(&J::Null),
                        ),
                        ("generationId", item.get("generationId").unwrap_or(&J::Null)),
                        ("taskId", item.get("taskId").unwrap_or(&J::Null)),
                        (
                            "childSessionId",
                            item.get("childSessionId").unwrap_or(&J::Null),
                        ),
                        (
                            "childSessionLogPath",
                            item.get("childSessionLogPath").unwrap_or(&J::Null),
                        ),
                    ],
                );
                (title, content, meta)
            }
            "monitorFeed" => {
                let description = monitor_description(item);
                let (description, _) = trunc(&description);
                let title = if description.is_empty() {
                    "Monitor event".to_string()
                } else {
                    format!("Monitor: {description}")
                };
                let (title, _) = trunc(&title);
                let text = str_field("text");
                let content = (!text.is_empty()).then(|| text.to_string());
                let mut fields = vec!["\"itemKind\":\"monitorFeed\"".to_string()];
                if let Some(monitor_id) = item
                    .get("monitorId")
                    .and_then(J::as_str)
                    .filter(|value| !value.is_empty())
                {
                    fields.push(format!("\"monitorId\":{}", esc(monitor_id)));
                }
                if !description.is_empty() {
                    fields.push(format!("\"monitorDescription\":{}", esc(&description)));
                }
                (title, content, Some(fields.join(",")))
            }
            "damagedRange" => {
                // Muse 1.4.11: what one damaged range of the session log
                // cost, derived from the readable records around it. The
                // facts ride `_meta` verbatim; the card states them and the
                // safe fork cut, when one exists.
                let Some(range @ J::Obj(_)) = item.get("damagedRange") else {
                    let text = str_field("fallbackText");
                    return (
                        text.to_string(),
                        None,
                        (!text.is_empty()).then(|| format!("\"itemKind\":{}", esc(kind))),
                    );
                };
                let count = |key: &str| range.get(key).and_then(J::as_u64).unwrap_or(0);
                let stamp =
                    |key: &str| range.get(key).and_then(J::as_str).filter(|s| !s.is_empty());
                let mut lines = Vec::new();
                let fallback = str_field("fallbackText");
                if !fallback.is_empty() {
                    lines.push(fallback.to_string());
                }
                lines.push(format!(
                    "{} missing record{}, {} unreadable line{}",
                    count("missingRecords"),
                    if count("missingRecords") == 1 {
                        ""
                    } else {
                        "s"
                    },
                    count("unreadableLines"),
                    if count("unreadableLines") == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
                match (stamp("readableBeforeAt"), stamp("readableAfterAt")) {
                    (Some(before), Some(after)) => {
                        lines.push(format!("Readable before {before} and again from {after}"))
                    }
                    (None, Some(after)) => lines.push(format!(
                        "The log starts damaged; readable again from {after}"
                    )),
                    _ => {}
                }
                if let Some(turn) = stamp("lastGoodTurnId") {
                    lines.push(format!(
                        "Safe fork point: turn {turn}, the last finished turn before the damage"
                    ));
                }
                (
                    "Damaged session log range".to_string(),
                    Some(lines.join("\n")),
                    Some(format!("\"damagedRange\":{}", j_to_string(range))),
                )
            }
            "scheduledFire" => {
                // Muse 1.4.11: one delivery of a scheduled (cron) job, with
                // its occurrence facts verbatim in `_meta`. Epochs render as
                // UTC; a missing next fire means a final delivery only when
                // the occurrence facts are present.
                let job = str_field("jobId");
                let delivery = item.get("deliveryNumber").and_then(J::as_u64);
                let at = |key: &str| item.get(key).and_then(J::as_u64).map(utc_from_epoch_ms);
                let mut lines = Vec::new();
                let fallback = str_field("fallbackText");
                if !fallback.is_empty() {
                    lines.push(fallback.to_string());
                }
                if let Some(n) = delivery {
                    lines.push(format!("Occurrence {n}"));
                }
                if let Some(scheduled) = at("scheduledFireAtMs") {
                    lines.push(format!("Scheduled for {scheduled}"));
                }
                match at("nextFireAtMs") {
                    Some(next) => lines.push(format!("Next fire: {next}")),
                    None if delivery.is_some() => lines.push("Final delivery".to_string()),
                    None => {}
                }
                let title = if job.is_empty() {
                    if fallback.is_empty() {
                        String::new()
                    } else {
                        "Scheduled job fired".to_string()
                    }
                } else {
                    format!("Scheduled job {job} fired")
                };
                let fields: Vec<String> = [
                    "jobId",
                    "deliveryNumber",
                    "scheduledFireAtMs",
                    "nextFireAtMs",
                ]
                .into_iter()
                .filter_map(|key| {
                    item.get(key)
                        .map(|v| format!("\"{key}\":{}", j_to_string(v)))
                })
                .collect();
                (
                    title,
                    (!lines.is_empty()).then(|| lines.join("\n")),
                    Some(format!("\"scheduledFire\":{{{}}}", fields.join(","))),
                )
            }
            _ => {
                // Unknown future kinds: the schema says a one-line summary
                // SHOULD ride `fallbackText`; without it there is nothing
                // honest to render.
                let text = str_field("fallbackText");
                (
                    text.to_string(),
                    None,
                    (!text.is_empty()).then(|| format!("\"itemKind\":{}", esc(kind))),
                )
            }
        }
    }

    /// Draft ACP subagent extension: announce a child session on the parent.
    fn subagent_spawned_line(
        acp_sid: &str,
        subagent_session_id: &str,
        name: &str,
        task: &str,
        muse_fields: Option<&str>,
    ) -> String {
        let mut update = format!(
            "{{\"sessionUpdate\":\"subagent_spawned\",\"subagentSessionId\":{},\"name\":{},\"task\":{},\"capabilities\":{{}}",
            esc(subagent_session_id),
            esc(name),
            esc(task)
        );
        if let Some(fields) = muse_fields {
            update.push_str(&format!(",\"_meta\":{{\"muse\":{{{fields}}}}}"));
        }
        update.push('}');
        Self::update_line(acp_sid, &update)
    }

    /// Draft ACP subagent extension: terminal state on the parent. MSP's
    /// generic item status is the terminal authority; recovery-pending
    /// control states map to `disconnected` rather than a guess.
    fn subagent_state_line(acp_sid: &str, subagent_session_id: &str, state: &str) -> String {
        Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"subagent_state_update\",\"subagentSessionId\":{},\"state\":\"{state}\"}}",
                esc(subagent_session_id)
            ),
        )
    }

    /// AIR async-task extension. MSP 1.3.0 accepts task control commands; the
    /// terminal outcome still arrives through the item view.
    fn async_task_spawned_line(
        acp_sid: &str,
        task_id: &str,
        name: &str,
        task_type: &str,
        can_stop: bool,
        tool_call_id: Option<&str>,
    ) -> String {
        let mut update = format!(
            "{{\"sessionUpdate\":\"async_task_spawned\",\"asyncTaskId\":{},\"name\":{},\"taskType\":{},\"showInTranscript\":false,\"canStop\":{}",
            esc(task_id),
            esc(name),
            esc(task_type),
            can_stop
        );
        if let Some(tc) = tool_call_id {
            update.push_str(&format!(",\"toolCallId\":{}", esc(tc)));
        }
        update.push('}');
        Self::update_line(acp_sid, &update)
    }

    fn async_task_state_once(
        &mut self,
        acp_sid: &str,
        task_id: &str,
        state: &str,
        out: &mut Vec<String>,
    ) {
        self.workflow_tasks.remove(task_id);
        if self
            .async_task_states
            .get(task_id)
            .is_some_and(|previous| previous == state)
        {
            return;
        }
        self.async_task_states
            .insert(task_id.to_string(), state.to_string());
        out.push(Self::async_task_state_line(acp_sid, task_id, state));
    }

    /// A permanent transport loss cannot establish a host terminal outcome.
    /// Retire the editor's running indicator with explicit unknown-outcome
    /// metadata; durable restart reconciliation happens before this fallback.
    pub fn disconnect_tasks(&mut self, acp_sid: &str) -> Vec<String> {
        let mut out = Vec::new();
        for task in self.announced_tasks.clone() {
            if self.async_task_states.contains_key(&task) {
                continue;
            }
            self.async_task_states
                .insert(task.clone(), "failed".to_string());
            out.push(Self::update_line(acp_sid, &format!(
                "{{\"sessionUpdate\":\"async_task_state_update\",\"asyncTaskId\":{},\"state\":\"failed\",\"_meta\":{{\"muse\":{{\"terminalUnknown\":true,\"reason\":\"connection closed\"}}}}}}", esc(&task)
            )));
        }
        out
    }

    fn async_task_state_line(acp_sid: &str, task_id: &str, state: &str) -> String {
        Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"async_task_state_update\",\"asyncTaskId\":{},\"state\":\"{state}\"}}",
                esc(task_id)
            ),
        )
    }

    fn announce_async_task(
        &mut self,
        acp_sid: &str,
        item_id: &str,
        task_id: &str,
        name: &str,
        tool_call_id: Option<&str>,
        out: &mut Vec<String>,
    ) {
        self.async_task_msp_ids
            .entry(task_id.to_string())
            .or_insert_with(|| item_id.to_string());
        if self.announced_tasks.insert(task_id.to_string()) {
            out.push(Self::async_task_spawned_line(
                acp_sid,
                task_id,
                name,
                "shell",
                true,
                tool_call_id,
            ));
        }
    }

    fn emit_async_task_state(
        &mut self,
        acp_sid: &str,
        task_id: &str,
        item: &J,
        out: &mut Vec<String>,
    ) {
        let Some(state) = Self::async_task_state(item) else {
            return;
        };
        self.async_task_msp_ids.remove(task_id);
        if self
            .async_task_states
            .get(task_id)
            .is_some_and(|previous| previous == state)
        {
            return;
        }
        self.async_task_states
            .insert(task_id.to_string(), state.to_string());
        out.push(Self::async_task_state_line(acp_sid, task_id, state));
    }

    /// AIR marks the owning command card as backgrounded so its output stays
    /// live without duplicating the transcript.
    fn backgrounded_tool_line(acp_sid: &str, ver: u8, tc_id: &str) -> String {
        let session_update = if ver == 2 {
            "tool_call_update"
        } else {
            "tool_call"
        };
        Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"{session_update}\",\"toolCallId\":{},\"_meta\":{{\"jetbrains\":{{\"air\":{{\"asyncTasks\":{{\"backgrounded\":true}}}}}}}}}}",
                esc(tc_id)
            ),
        )
    }

    fn async_task_state(item: &J) -> Option<&'static str> {
        match item.get("status").and_then(|v| v.as_str()).unwrap_or("") {
            "completed" => Some("completed"),
            "failed" | "rejected" => Some("failed"),
            "cancelled" | "timedOut" => Some("stopped"),
            _ => None,
        }
    }

    fn subagent_state(item: &J) -> Option<&'static str> {
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let control = item
            .get("controlStatus")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let state = match status {
            // The generic item status is the terminal authority; a completed
            // item is a settled child outcome even if control lagged behind.
            "completed" => "completed",
            "failed" | "rejected" => "failed",
            "cancelled" | "timedOut" => "cancelled",
            _ => match control {
                "recoveryPending" | "manualReconciliation" => "disconnected",
                _ => return None, // still running: no state update
            },
        };
        Some(state)
    }

    /// Stable synthetic toolCall id + first-announcement flag for host cards.
    fn host_item_role(&mut self, item_id: &str, kind: &str) -> (String, bool) {
        let prefix = match kind {
            "subagent" => "subagent-",
            "workflow" => "workflow-",
            "userShell" => "shell-",
            _ => "item-",
        };
        match self.items.get(item_id) {
            Some(ItemRole::Tool { tc_id, announced }) => (tc_id.clone(), *announced),
            _ => {
                let id = format!("{prefix}{item_id}");
                self.items.insert(
                    item_id.to_string(),
                    ItemRole::Tool {
                        tc_id: id.clone(),
                        announced: false,
                    },
                );
                (id, false)
            }
        }
    }

    /// Reminder-child card status. A superseded generation is host churn, not
    /// editor-facing failure: the host cancels the previous check when a newer
    /// one starts, ACP has no `cancelled` tool status, and ADR 41191 D4 keeps
    /// the reminder's status word off the wire. `cancelled` therefore settles
    /// the card as completed instead of inventing a red failure; a real
    /// `failed`/`rejected`/`timedOut` outcome still shows.
    fn reminder_status(status: &str) -> &'static str {
        if status == "cancelled" {
            "completed"
        } else {
            msp_status(status)
        }
    }

    /// Resolve the card for one `reminderChild` frame: the ACP tool-call id and
    /// whether this frame must create the card. Generations of one reminder
    /// agent reuse their agent's card id, so a re-check updates the existing
    /// row. Items without an agent id keep one card per host item.
    fn reminder_role(&mut self, item_id: &str, item: &J) -> (String, bool) {
        if let Some(ItemRole::Tool { tc_id, announced }) = self.items.get(item_id) {
            return (tc_id.clone(), !*announced);
        }
        let key = item
            .get("reminderAgentId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|agent| format!("agent:{agent}"))
            .unwrap_or_else(|| format!("item:{item_id}"));
        let (tc_id, create) = match self.reminder_cards.get(&key) {
            Some(tc_id) => (tc_id.clone(), false),
            None => {
                let tc_id = format!("reminder-{item_id}");
                self.reminder_cards.insert(key, tc_id.clone());
                (tc_id, true)
            }
        };
        self.items.insert(
            item_id.to_string(),
            ItemRole::Tool {
                tc_id: tc_id.clone(),
                announced: true,
            },
        );
        (tc_id, create)
    }

    /// Emit one frame of a reminder-child card. `settle` marks a terminal frame
    /// (the caller has already recorded the item as done): it releases the
    /// item's role while the agent's card id stays reusable for the next
    /// generation the host mints.
    #[allow(clippy::too_many_arguments)]
    fn reminder_child_card(
        &mut self,
        acp_sid: &str,
        ver: u8,
        item_id: &str,
        item: &J,
        status: &str,
        settle: bool,
        out: &mut Vec<String>,
    ) {
        let (title, content, meta) = self.host_card_parts("reminderChild", item);
        let announced_card = matches!(self.items.get(item_id), Some(ItemRole::Tool { .. }));
        if title.is_empty() && !announced_card {
            // Never announced and no title to settle with: there is nothing
            // honest to render, and no card is stranded by staying silent.
            if settle {
                self.items.remove(item_id);
            } else {
                self.items
                    .entry(item_id.to_string())
                    .or_insert(ItemRole::Ignored);
            }
            return;
        }
        let (tc_id, create) = self.reminder_role(item_id, item);
        let card_title = if title.is_empty() {
            fallback_card_title("reminderChild")
        } else {
            title
        };
        let muse_fields =
            Self::merge_muse_fields(meta.as_deref(), Self::item_muse_fields(item).as_deref());
        out.push(Self::card_line(
            acp_sid,
            ver,
            create,
            &tc_id,
            &card_title,
            "other",
            Self::reminder_status(status),
            content.as_deref(),
            muse_fields.as_deref(),
            item.get("truncated")
                .is_some_and(|v| matches!(v, J::Bool(true))),
        ));
        if settle {
            self.items.remove(item_id);
        }
    }

    /// `turn/foregroundCompleted`: the answer is done, but the named reminder
    /// agents hold the turn open until `turn/completed` (tdd SS4.5.12). The
    /// prompt stays pending, so tell the editor why: a `_meta.muse` hint
    /// (the SDK fold's `foregroundCompleted`) and, unless per-agent reminder
    /// cards are opted in, one card naming the checks. A later firing in the
    /// same turn settles the previous card and opens a new one after any
    /// work the reminder resumed.
    pub fn on_foreground_completed(
        &mut self,
        acp_sid: &str,
        ver: u8,
        turn_id: &str,
        agents: &[&str],
        out: &mut Vec<String>,
    ) {
        self.settle_background_checks(acp_sid, None, "completed", false, out);
        let agents_json = agents.iter().map(|a| esc(a)).collect::<Vec<_>>().join(",");
        out.push(Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"session_info_update\",\"_meta\":{{\"muse\":{{\"foregroundCompleted\":{{\"turnId\":{},\"blockingAgents\":[{agents_json}]}}}}}}}}",
                esc(turn_id)
            ),
        ));
        if self.show_reminders {
            self.background_checks = Some((turn_id.to_string(), String::new()));
            return;
        }
        let tc_id = mint_id("checks-", &self.idc);
        let title = if agents.is_empty() {
            "Background checks".to_string()
        } else {
            format!("Background checks: {}", agents.join(", "))
        };
        out.push(Self::card_line(
            acp_sid,
            ver,
            true,
            &tc_id,
            &title,
            "other",
            "in_progress",
            None,
            Some(&format!(
                "\"turnId\":{},\"blockingAgents\":[{agents_json}]",
                esc(turn_id)
            )),
            false,
        ));
        self.background_checks = Some((turn_id.to_string(), tc_id));
    }

    /// Settle the background-checks card and clear the hint. `turn` limits
    /// this to one turn's terminal; `None` settles whatever is open. `clear`
    /// also withdraws the `_meta.muse.foregroundCompleted` hint.
    pub fn settle_background_checks(
        &mut self,
        acp_sid: &str,
        turn: Option<&str>,
        status: &str,
        clear: bool,
        out: &mut Vec<String>,
    ) {
        if turn.is_some_and(|t| {
            self.background_checks
                .as_ref()
                .is_some_and(|(open, _)| open != t)
        }) {
            return;
        }
        let Some((_, tc_id)) = self.background_checks.take() else {
            return;
        };
        if !tc_id.is_empty() {
            out.push(Self::update_line(
                acp_sid,
                &format!(
                    "{{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":{},\"status\":{}}}",
                    esc(&tc_id),
                    esc(status)
                ),
            ));
        }
        if clear {
            out.push(Self::update_line(
                acp_sid,
                "{\"sessionUpdate\":\"session_info_update\",\"_meta\":{\"muse\":{\"foregroundCompleted\":null}}}",
            ));
        }
    }

    /// `turn/modelRetryScheduled`: a provider backoff behind one model
    /// attempt of a running turn (Muse 1.4.8). Non-terminal, so the prompt
    /// stays pending; publish the attempt facts as a `_meta.muse.modelRetry`
    /// hint so the editor can show "retrying in Ss, attempt N/M" instead of
    /// dead air. `retryDelayMs` is the recorded backoff, not a fire time, so
    /// any countdown is the client's. Absent optional members are omitted
    /// rather than fabricated, and `errorKind` passes through verbatim: its
    /// vocabulary is open.
    pub fn on_model_retry_scheduled(
        &mut self,
        acp_sid: &str,
        turn_id: &str,
        params: &J,
        out: &mut Vec<String>,
    ) {
        let mut fields = vec![format!("\"turnId\":{}", esc(turn_id))];
        for key in ["attempt", "nextAttempt", "maxAttempts", "retryDelayMs"] {
            if let Some(n) = params.get(key).and_then(J::as_u64) {
                fields.push(format!("\"{key}\":{n}"));
            }
        }
        for key in ["errorKind", "system"] {
            if let Some(text) = params.get(key).and_then(J::as_str) {
                fields.push(format!("\"{key}\":{}", esc(text)));
            }
        }
        if let Some(status) = params.get("httpStatus").and_then(J::as_u64) {
            fields.push(format!("\"httpStatus\":{status}"));
        }
        out.push(Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"session_info_update\",\"_meta\":{{\"muse\":{{\"modelRetry\":{{{}}}}}}}}}",
                fields.join(",")
            ),
        ));
        self.model_retry = Some(turn_id.to_string());
    }

    /// Withdraw the `_meta.muse.modelRetry` hint at a turn's terminal. `turn`
    /// limits this to that turn; `None` clears whatever is showing.
    pub fn clear_model_retry(&mut self, acp_sid: &str, turn: Option<&str>, out: &mut Vec<String>) {
        if turn.is_some_and(|t| self.model_retry.as_deref().is_some_and(|open| open != t)) {
            return;
        }
        if self.model_retry.take().is_some() {
            out.push(Self::update_line(
                acp_sid,
                "{\"sessionUpdate\":\"session_info_update\",\"_meta\":{\"muse\":{\"modelRetry\":null}}}",
            ));
        }
    }

    fn tool_line(acp_sid: &str, ver: u8, u: &ToolUpdate<'_>) -> String {
        let session_update = if ver == 2 || !u.create {
            "tool_call_update"
        } else {
            "tool_call"
        };
        let mut f = vec![
            format!("\"sessionUpdate\":\"{session_update}\""),
            format!("\"toolCallId\":{}", esc(u.tc_id)),
        ];
        if let Some(name) = u.name.filter(|name| !name.is_empty()) {
            f.push(format!("\"name\":{}", esc(name)));
        }
        f.extend([
            format!("\"title\":{}", esc(u.title)),
            format!("\"kind\":\"{}\"", u.kind),
            format!("\"status\":{}", esc(u.status)),
        ]);
        let mut muse_fields = u.muse_fields.unwrap_or_default().to_string();
        let mut content = Vec::new();
        if let Some(t) = u.content_text {
            let (text, adapter_cut) = trunc(t);
            content.push(format!(
                "{{\"type\":\"content\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}",
                esc(&text)
            ));
            let truncation = if u.host_truncated {
                Some("\"source\":\"host\"".to_string())
            } else {
                adapter_cut
            };
            if let Some(cut) = truncation {
                if !muse_fields.is_empty() {
                    muse_fields.push(',');
                }
                muse_fields.push_str(&format!("\"truncated\":{{{cut}}}"));
            }
        }
        content.extend(u.extra_content.map(str::to_string));
        if !content.is_empty() {
            f.push(format!("\"content\":[{}]", content.join(",")));
        }
        if let Some(r) = u.raw_input {
            f.push(format!("\"rawInput\":{r}"));
        }
        if let Some(locations) = u.locations {
            f.push(format!("\"locations\":{locations}"));
        }
        if !muse_fields.is_empty() {
            f.push(format!("\"_meta\":{{\"muse\":{{{muse_fields}}}}}"));
        }
        Self::update_line(acp_sid, &format!("{{{}}}", f.join(",")))
    }

    /// The command (and an explicit working directory, resolved absolute)
    /// of a shell tool call or user shell, or `None` for other items.
    fn shell_facts(&self, item: &J) -> Option<(String, Option<String>)> {
        match item.get("kind").and_then(J::as_str)? {
            "userShell" => Some((
                item.get("commandText").and_then(J::as_str)?.to_string(),
                None,
            )),
            "toolCall" => {
                let tool = item.get("tool").and_then(J::as_str)?.to_ascii_lowercase();
                if !SHELL_TOOLS.contains(&tool.as_str()) {
                    return None;
                }
                let args = match item.get("args")? {
                    J::Str(raw) => parse_json(raw).ok()?,
                    args @ J::Obj(_) => args.clone(),
                    _ => return None,
                };
                let command = args.get("command").and_then(J::as_str)?.to_string();
                let cwd = args
                    .get("workdir")
                    .and_then(J::as_str)
                    .and_then(|dir| absolute_tool_path(&self.workspace_root, dir));
                Some((command, cwd))
            }
            _ => None,
        }
    }

    /// Note an ACP v2 shell card's command so a terminal can show it once
    /// the command runs.
    fn note_shell(&mut self, ver: u8, item_id: &str, tc_id: &str, item: &J) {
        if ver != 2 || self.shell_commands.contains_key(item_id) {
            return;
        }
        if let Some((command, cwd)) = self.shell_facts(item) {
            self.shell_commands.insert(
                item_id.to_string(),
                ShellCommand {
                    tc_id: tc_id.to_string(),
                    command,
                    cwd,
                },
            );
        }
    }

    /// The agent-owned terminal for a noted shell card, opening it on first
    /// use: `terminal_update` with the command, then the card's content
    /// becomes a reference to it.
    fn open_terminal(
        &mut self,
        acp_sid: &str,
        item_id: &str,
        out: &mut Vec<String>,
    ) -> Option<String> {
        if let Some((terminal_id, _)) = self.terminals.get(item_id) {
            return Some(terminal_id.clone());
        }
        let shell = self.shell_commands.get(item_id)?;
        let terminal_id = format!("term-{item_id}");
        let cwd = shell
            .cwd
            .as_deref()
            .map(|cwd| format!(",\"cwd\":{}", esc(cwd)))
            .unwrap_or_default();
        out.push(Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"terminal_update\",\"terminalId\":{},\"command\":{}{cwd}}}",
                esc(&terminal_id),
                esc(&shell.command)
            ),
        ));
        out.push(Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":{},\"content\":[{}]}}",
                esc(&shell.tc_id),
                Self::terminal_content(&terminal_id)
            ),
        ));
        self.terminals
            .insert(item_id.to_string(), (terminal_id.clone(), 0));
        Some(terminal_id)
    }

    fn terminal_content(terminal_id: &str) -> String {
        format!(
            "{{\"type\":\"terminal\",\"terminalId\":{}}}",
            esc(terminal_id)
        )
    }

    /// Settle a shell card's terminal at the item's terminal state, opening
    /// it if the command ran without streaming. Returns the terminal content
    /// item for the card, or `None` to keep the text card: a command that
    /// never ran (rejected or cancelled before output) gets no terminal.
    /// The output snapshot is the bounded final output. Exit facts are the
    /// host's: a user shell's code (its signal number stays in `_meta`, never
    /// mapped to a name); a shell tool call records none, so its terminal is
    /// only marked exited.
    fn finish_terminal(
        &mut self,
        acp_sid: &str,
        item_id: &str,
        item: &J,
        status: &str,
        out: &mut Vec<String>,
    ) -> Option<String> {
        let output = item.get("visibleOutput").and_then(J::as_str).unwrap_or("");
        let ran = self.terminals.contains_key(item_id)
            || status == "completed"
            || (status == "failed" && !output.is_empty());
        if !ran {
            self.shell_commands.remove(item_id);
            return None;
        }
        let terminal_id = self.open_terminal(acp_sid, item_id, out)?;
        self.terminals.remove(item_id);
        self.shell_commands.remove(item_id);
        let (shown, adapter_cut) = trunc(output);
        let truncation = if item
            .get("truncated")
            .is_some_and(|v| matches!(v, J::Bool(true)))
        {
            Some("\"source\":\"host\"".to_string())
        } else {
            adapter_cut
        };
        let mut exit = Vec::new();
        if let Some(code) = item.get("exitCode").and_then(J::as_u64) {
            exit.push(format!("\"exitCode\":{code}"));
        }
        if let Some(signal) = item.get("exitSignal").and_then(J::as_u64) {
            exit.push(format!(
                "\"_meta\":{{\"muse\":{{\"exitSignal\":{signal}}}}}"
            ));
        }
        let meta = truncation
            .map(|cut| format!(",\"_meta\":{{\"muse\":{{\"truncated\":{{{cut}}}}}}}"))
            .unwrap_or_default();
        out.push(Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"terminal_update\",\"terminalId\":{},\"output\":{{\"data\":{}}},\"exitStatus\":{{{}}}{meta}}}",
                esc(&terminal_id),
                esc(&crate::json::b64(shown.as_bytes())),
                exit.join(",")
            ),
        ));
        Some(Self::terminal_content(&terminal_id))
    }

    /// ACP `locations` for a native file tool, from its explicit path
    /// arguments only: `read_file`, `write_file`, and `edit_file` (and the
    /// older single-path and move tools, and the adapter's editor file
    /// tools) name the files they touch. Shell,
    /// search, MCP, and unknown tools get no location, because a path in a
    /// command line or in tool output is a guess. Relative paths resolve
    /// against the workspace root; `..` is resolved lexically.
    fn tool_locations(&self, item: &J) -> Option<String> {
        let tool = item.get("tool").and_then(J::as_str)?.to_ascii_lowercase();
        let keys: &[&str] = match tool.as_str() {
            "read_file" | "write_file" | "edit_file" | "create_file" | "delete_file"
            | "remove_file" => &["path", "filePath"],
            // The adapter's editor file tools (bridge.rs).
            "mcp__editor__read_text_file"
            | "mcp__editor__write_text_file"
            | "mcp__editor__edit_text_file" => &["path"],
            "move_file" | "rename_file" => &[
                "oldPath",
                "from",
                "sourcePath",
                "newPath",
                "to",
                "destinationPath",
            ],
            _ => return None,
        };
        let args = match item.get("args")? {
            J::Str(raw) => parse_json(raw).ok()?,
            args @ J::Obj(_) => args.clone(),
            _ => return None,
        };
        let mut paths: Vec<String> = Vec::new();
        for key in keys {
            if let Some(path) = args
                .get(key)
                .and_then(J::as_str)
                .and_then(|raw| absolute_tool_path(&self.workspace_root, raw))
                && !paths.contains(&path)
            {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return None;
        }
        let entries: Vec<String> = paths
            .iter()
            .map(|path| format!("{{\"path\":{}}}", esc(path)))
            .collect();
        Some(format!("[{}]", entries.join(",")))
    }

    fn args_detail(item: &J) -> Option<String> {
        let args = item.get("args")?;
        // `args` is usually a JSON-encoded string (`"{\"command\":\"...\"}"`);
        // it can also arrive as an already-decoded object in replays.
        let parsed;
        let obj = match args {
            J::Str(s) => match parse_json(s) {
                Ok(obj @ J::Obj(_)) => {
                    parsed = obj;
                    &parsed
                }
                // A plain non-JSON string beats a bare tool name.
                _ => return (!s.trim().is_empty() && s.trim() != "{}").then(|| s.clone()),
            },
            J::Obj(_) => args,
            _ => return None,
        };
        for key in [
            "command",
            "cmd",
            "commandText",
            "displayText",
            "summary",
            "task",
            "objective",
            "query",
            "pattern",
            "path",
            "file",
            "filePath",
            "target",
            "url",
            "question",
            "prompt",
        ] {
            if let Some(s) = obj.get(key).and_then(|v| v.as_str())
                && !s.trim().is_empty()
            {
                return Some(s.to_string());
            }
        }
        None
    }

    fn tool_display_label(item: &J) -> Option<&str> {
        let labels = item.get("displayLabel")?;
        let key = match item.get("status").and_then(J::as_str) {
            Some("inProgress") => "active",
            Some("completed") => "done",
            _ => "neutral",
        };
        labels
            .get(key)
            .and_then(J::as_str)
            .filter(|label| !label.trim().is_empty())
    }

    fn tool_title(item: &J) -> (String, String) {
        let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
        if let Some(label) = Self::tool_display_label(item) {
            return (label.to_string(), tool.to_string());
        }
        let top = item
            .get("commandText")
            .or_else(|| item.get("displayText"))
            .or_else(|| item.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let detail = if !top.is_empty() {
            top.to_string()
        } else {
            Self::args_detail(item).unwrap_or_default()
        };
        let title = if detail.is_empty() {
            tool.to_string()
        } else {
            format!("{tool}: {detail}")
        };
        (title, tool.to_string())
    }

    /// A replayed message's text. A user message shows the prompt as the
    /// user wrote it (`displayText`), without text the adapter added, such
    /// as the plan-mode instruction.
    fn message_text(item: &J) -> &str {
        let display = (item.get("kind").and_then(|v| v.as_str()) == Some("userMessage"))
            .then(|| item.get("displayText").and_then(|v| v.as_str()))
            .flatten();
        display
            .or_else(|| item.get("text").and_then(|v| v.as_str()))
            .unwrap_or("")
    }

    /// `cut_off` names the item when the replayed reply carries the cut-off
    /// mark, and `phase` carries its declared message phase; both ride the
    /// message itself. A settled agent-message replay carries its phase; an
    /// in-flight prefix records none yet, and user messages carry neither.
    fn replay_message_line(
        acp_sid: &str,
        ver: u8,
        kind: &str,
        msg_id: &str,
        text: &str,
        cut_off: Option<&str>,
        phase: Option<&str>,
    ) -> String {
        let content = format!("[{{\"type\":\"text\",\"text\":{}}}]", esc(text));
        let mut update = match (kind, ver) {
            ("userMessage", 2) => format!(
                "{{\"sessionUpdate\":\"user_message\",\"messageId\":{},\"content\":{}}}",
                esc(msg_id),
                content
            ),
            ("agentMessage", 2) => format!(
                "{{\"sessionUpdate\":\"agent_message\",\"messageId\":{},\"content\":{}}}",
                esc(msg_id),
                content
            ),
            ("userMessage", _) => format!(
                "{{\"sessionUpdate\":\"user_message_chunk\",\"messageId\":{},\"content\":{{\"type\":\"text\",\"text\":{}}}}}",
                esc(msg_id),
                esc(text)
            ),
            ("agentMessage", _) => format!(
                "{{\"sessionUpdate\":\"agent_message_chunk\",\"messageId\":{},\"content\":{{\"type\":\"text\",\"text\":{}}}}}",
                esc(msg_id),
                esc(text)
            ),
            _ => unreachable!("replay_message_line called for {kind}"),
        };
        if let Some(meta) = Self::message_meta(phase, cut_off) {
            update.insert_str(update.len() - 1, &meta);
        }
        Self::update_line(acp_sid, &update)
    }

    fn replay_is_in_flight(item: &J) -> bool {
        matches!(
            item.get("status").and_then(|v| v.as_str()),
            Some("pending" | "inProgress" | "in_progress" | "running" | "started")
        )
    }

    /// Seed the fold from one history item without turning an in-flight item
    /// into a settled item. Snapshot items are the latest state at the
    /// history cursor, so the live stream may still deliver their deltas and
    /// terminal completion after attach.
    pub fn replay_item(&mut self, acp_sid: &str, ver: u8, item: &J, out: &mut Vec<String>) {
        let item_id = match item.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        self.observe_workflow(item, !Self::replay_is_in_flight(item));
        if self.known(&item_id) {
            return;
        }
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if Self::replay_is_in_flight(item) {
            match kind {
                "toolCall" | "subagent" | "monitorFeed" | "compaction" => {
                    self.on_item_snapshot(acp_sid, ver, item, out)
                }
                "agentMessage" | "userMessage" => {
                    self.on_item_snapshot(acp_sid, ver, item, out);
                    let text = Self::message_text(item);
                    if text.is_empty() {
                        return;
                    }
                    let unshown = matches!(
                        self.items.get(&item_id),
                        Some(ItemRole::Message { streamed: 0, .. })
                    );
                    let mark = unshown && self.claim_cut_off(item, &item_id);
                    if let Some(ItemRole::Message { msg_id, streamed }) =
                        self.items.get_mut(&item_id)
                        && *streamed == 0
                    {
                        *streamed = text.len();
                        out.push(Self::replay_message_line(
                            acp_sid,
                            ver,
                            kind,
                            msg_id,
                            text,
                            mark.then_some(item_id.as_str()),
                            None,
                        ));
                    }
                }
                _ => {}
            }
            return;
        }

        match kind {
            "toolCall" | "subagent" | "monitorFeed" | "compaction" => {
                let wrap = J::Obj(vec![("item".to_string(), item.clone())]);
                self.on_item_completed(acp_sid, ver, &wrap, out);
            }
            "agentMessage" | "userMessage" => {
                self.done.insert(item_id.clone());
                let text = Self::message_text(item);
                if !text.is_empty() {
                    let msg_id = mint_id("msg-", &self.idc);
                    let mark = self.claim_cut_off(item, &item_id);
                    let phase = (kind == "agentMessage").then(|| Self::message_phase(item));
                    out.push(Self::replay_message_line(
                        acp_sid,
                        ver,
                        kind,
                        &msg_id,
                        text,
                        mark.then_some(item_id.as_str()),
                        phase,
                    ));
                    if kind == "agentMessage" {
                        self.last_message = Some((item_id, msg_id));
                    }
                }
            }
            _ => {}
        }
    }

    /// Upsert and emit the single stable card for one monitor task. The feed
    /// item id remains in metadata, while `monitorId` selects the ACP row.
    fn monitor_feed_card(
        &mut self,
        acp_sid: &str,
        ver: u8,
        item_id: &str,
        item: &J,
        status: &str,
        out: &mut Vec<String>,
    ) {
        let (title, content, meta) = self.host_card_parts("monitorFeed", item);
        let muse_fields =
            Self::merge_muse_fields(meta.as_deref(), Self::item_muse_fields(item).as_deref());
        let monitor_id = item
            .get("monitorId")
            .and_then(J::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(item_id);
        let muse_fields = Self::merge_muse_fields(
            muse_fields.as_deref(),
            self.inbox_counts(monitor_id).as_deref(),
        );
        let tc_id = format!("monitor-{monitor_id}");
        let create = self.monitor_cards.insert(tc_id.clone());
        let host_truncated = item
            .get("truncated")
            .is_some_and(|value| matches!(value, J::Bool(true)));
        let (text, effective_truncated) = match self.items.get_mut(item_id) {
            Some(ItemRole::Monitor {
                title: seen_title,
                muse_fields: seen_fields,
                text: seen_text,
                host_truncated: seen_truncated,
                ..
            }) => {
                *seen_title = title.clone();
                *seen_fields = muse_fields.clone();
                if let Some(content) = content {
                    *seen_text = content;
                }
                *seen_truncated |= host_truncated;
                (seen_text.clone(), *seen_truncated)
            }
            _ => {
                let text = content.unwrap_or_default();
                self.items.insert(
                    item_id.to_string(),
                    ItemRole::Monitor {
                        tc_id: tc_id.clone(),
                        title: title.clone(),
                        muse_fields: muse_fields.clone(),
                        text: text.clone(),
                        host_truncated,
                    },
                );
                (text, host_truncated)
            }
        };
        out.push(Self::card_line(
            acp_sid,
            ver,
            create,
            &tc_id,
            &title,
            "other",
            msp_status(status),
            (!text.is_empty()).then_some(text.as_str()),
            muse_fields.as_deref(),
            effective_truncated,
        ));
    }

    /// The `monitorInbox` counts for one monitor task, once it has inbox
    /// items: still pending, drained to the agent, and voided (a missed
    /// wake). A drain and a void of one item settle latest-wins on their
    /// recorded RFC 3339 stamps, as Muse's own fold does.
    fn inbox_counts(&self, task_id: &str) -> Option<String> {
        let (mut pending, mut drained, mut voided) = (0, 0, 0);
        for item in self
            .monitor_inbox
            .values()
            .filter(|item| item.task_id.as_deref() == Some(task_id))
        {
            match (&item.drained_at, &item.voided_at) {
                (None, None) => pending += 1,
                (Some(_), None) => drained += 1,
                (None, Some(_)) => voided += 1,
                (Some(drain), Some(void)) if void > drain => voided += 1,
                (Some(_), Some(_)) => drained += 1,
            }
        }
        (pending + drained + voided > 0).then(|| {
            format!(
                "\"monitorInbox\":{{\"pending\":{pending},\"drained\":{drained},\"voided\":{voided}}}"
            )
        })
    }

    /// Muse 1.4.11 monitor inbox lifecycle (`monitor/inboxItemAdmitted`,
    /// `...Drained`, `...Voided`). Each change updates the owning monitor's
    /// card with fresh `_meta.muse.monitorInbox` counts. A void is count-only
    /// by schema design, so nothing reaches the transcript; inbox items of a
    /// monitor without a card yet are counted and shown once its card exists.
    pub fn on_monitor_inbox(
        &mut self,
        acp_sid: &str,
        method: &str,
        params: &J,
        out: &mut Vec<String>,
    ) {
        let Some(item_id) = params.get("itemId").and_then(J::as_str) else {
            return;
        };
        let stamp = |key: &str| params.get(key).and_then(J::as_str).map(str::to_string);
        let entry = self.monitor_inbox.entry(item_id.to_string()).or_default();
        match method {
            "monitor/inboxItemAdmitted" => {
                if let Some(task) = params.get("taskId").and_then(J::as_str) {
                    entry.task_id = Some(task.to_string());
                }
            }
            "monitor/inboxItemDrained" => entry.drained_at = stamp("drainedAt"),
            "monitor/inboxItemVoided" => entry.voided_at = stamp("voidedAt"),
            _ => return,
        }
        let Some(task_id) = entry.task_id.clone() else {
            return;
        };
        let tc_id = format!("monitor-{task_id}");
        if !self.monitor_cards.contains(&tc_id) {
            return;
        }
        let card_fields = self.items.values().find_map(|role| match role {
            ItemRole::Monitor {
                tc_id: card,
                muse_fields,
                ..
            } if *card == tc_id => muse_fields.clone(),
            _ => None,
        });
        // Clients may replace `_meta.muse` wholesale, so the card's own
        // fields ride along with the counts.
        let Some(fields) = Self::merge_muse_fields(
            card_fields.as_deref(),
            self.inbox_counts(&task_id).as_deref(),
        ) else {
            return;
        };
        out.push(Self::update_line(
            acp_sid,
            &format!(
                "{{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":{},\"_meta\":{{\"muse\":{{{fields}}}}}}}",
                esc(&tc_id)
            ),
        ));
    }

    /// item/started and item/updated share snapshotting.
    pub fn on_item_snapshot(&mut self, acp_sid: &str, ver: u8, item: &J, out: &mut Vec<String>) {
        let item_id = match item.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        self.observe_subagent(item);
        self.observe_workflow(item, false);
        if self.known(&item_id) {
            // A gap-refill replay of a settled item, or a later revision
            // that marks the settled reply cut off.
            self.mark_cut_off(acp_sid, ver, item, out);
            return;
        }
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        match kind {
            "toolCall" => {
                let (tc_id, announced) = match self.items.get(&item_id) {
                    Some(ItemRole::Tool { tc_id, announced }) => (tc_id.clone(), *announced),
                    _ => {
                        // Prefer the host callId so later approval/requested
                        // references resolve to the announced call.
                        let id = item
                            .get("callId")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| mint_id("tc-", &self.idc));
                        self.items.insert(
                            item_id.clone(),
                            ItemRole::Tool {
                                tc_id: id.clone(),
                                announced: false,
                            },
                        );
                        (id, false)
                    }
                };
                let (title, tool) = Self::tool_title(item);
                let raw = item
                    .get("args")
                    .map(j_to_string)
                    .unwrap_or_else(|| "{}".to_string());
                let item_fields = Self::item_muse_fields(item);
                let locations = self.tool_locations(item);
                out.push(Self::tool_line(
                    acp_sid,
                    ver,
                    &ToolUpdate {
                        create: !announced,
                        tc_id: &tc_id,
                        name: item
                            .get("tool")
                            .and_then(J::as_str)
                            .filter(|name| !name.is_empty()),
                        title: &title,
                        kind: tool_kind(&tool),
                        status: msp_status(status),
                        content_text: None,
                        raw_input: Some(&raw),
                        locations: locations.as_deref(),
                        extra_content: None,
                        muse_fields: item_fields.as_deref(),
                        host_truncated: false,
                    },
                ));
                self.note_shell(ver, &item_id, &tc_id, item);
                let task_id = self.async_task_id_for(&item_id, &tc_id);
                let backgrounded = item
                    .get("background")
                    .is_some_and(|v| matches!(v, J::Bool(true)))
                    || self.announced_tasks.contains(&tc_id)
                    || self.backgrounded_cards.contains(&tc_id);
                if backgrounded && self.air_async_tasks {
                    if self.backgrounded_cards.insert(tc_id.clone()) {
                        out.push(Self::backgrounded_tool_line(acp_sid, ver, &tc_id));
                    }
                    self.announce_async_task(
                        acp_sid,
                        &item_id,
                        &task_id,
                        &title,
                        Some(&tc_id),
                        out,
                    );
                    self.emit_async_task_state(acp_sid, &task_id, item, out);
                }
                if let Some(ItemRole::Tool { announced, .. }) = self.items.get_mut(&item_id) {
                    *announced = true;
                }
                if tool.starts_with("mcp__editor__")
                    && !self.editor_calls.iter().any(|(id, ..)| *id == item_id)
                {
                    let args = match item.get("args") {
                        Some(J::Str(raw)) => parse_json(raw).unwrap_or(J::Null),
                        Some(args) => args.clone(),
                        None => J::Null,
                    };
                    self.editor_calls
                        .push((item_id.clone(), tc_id.clone(), tool, args));
                }
            }
            "agentMessage" | "userMessage" => {
                if !self.items.contains_key(&item_id) {
                    let msg_id = mint_id("msg-", &self.idc);
                    self.items.insert(
                        item_id,
                        ItemRole::Message {
                            msg_id,
                            streamed: 0,
                        },
                    );
                }
                self.mark_cut_off(acp_sid, ver, item, out);
            }
            "reasoning" => {
                if !self.items.contains_key(&item_id) {
                    let msg_id = mint_id("thought-", &self.idc);
                    self.items.insert(
                        item_id,
                        ItemRole::Thought {
                            msg_id,
                            streamed: 0,
                            part: 0,
                        },
                    );
                }
            }
            "compaction" => {
                if self.compaction_supported {
                    out.push(Self::standard_compaction_line(acp_sid, &item_id, item));
                } else {
                    let status = msp_status(status);
                    out.push(Self::legacy_compaction_line(
                        acp_sid,
                        ver,
                        &item_id,
                        status,
                        Self::compaction_content(item).as_deref(),
                    ));
                }
            }
            "subagent" if self.native_subagents => {
                if let Some(child) = item.get("childSessionId").and_then(|v| v.as_str())
                    && !child.is_empty()
                    && !self.spawned_subagents.contains(child)
                {
                    self.spawned_subagents.insert(child.to_string());
                    let name = item
                        .get("agentPath")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("subagent");
                    let task = item
                        .get("objective")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Delegated task");
                    let fields = [
                        ("subagentId", item.get("subagentId")),
                        ("depth", item.get("depth")),
                        ("controlStatus", item.get("controlStatus")),
                    ];
                    let parts: Vec<String> = fields
                        .into_iter()
                        .filter(|(_, v)| matches!(v, Some(J::Num(_)) | Some(J::Str(_))))
                        .map(|(k, v)| format!("\"{k}\":{}", j_to_string(v.unwrap())))
                        .collect();
                    let fields = (!parts.is_empty()).then(|| parts.join(","));
                    out.push(Self::subagent_spawned_line(
                        acp_sid,
                        child,
                        name,
                        task,
                        fields.as_deref(),
                    ));
                    if let Some(state) = Self::subagent_state(item) {
                        out.push(Self::subagent_state_line(acp_sid, child, state));
                    }
                }
            }
            "subagent" | "workflow" | "userShell" | "hookRun" => {
                let (tc_id, announced) = self.host_item_role(&item_id, kind);
                let (title, content, meta) = self.host_card_parts(kind, item);
                let muse_fields = Self::merge_muse_fields(
                    meta.as_deref(),
                    Self::item_muse_fields(item).as_deref(),
                );
                // Once an ACP v2 user shell streams into its terminal, its
                // text snapshots must not replace the terminal reference.
                let content = if kind == "userShell" && self.terminals.contains_key(&item_id) {
                    None
                } else {
                    content
                };
                if !title.is_empty() {
                    out.push(Self::card_line(
                        acp_sid,
                        ver,
                        !announced,
                        &tc_id,
                        &title,
                        "other",
                        msp_status(status),
                        content.as_deref(),
                        muse_fields.as_deref(),
                        item.get("truncated")
                            .is_some_and(|v| matches!(v, J::Bool(true))),
                    ));
                }
                if kind == "userShell" {
                    self.note_shell(ver, &item_id, &tc_id, item);
                }
                if kind == "userShell" && self.air_async_tasks {
                    let task_id = self.async_task_id_for(&item_id, &tc_id);
                    self.announce_async_task(
                        acp_sid,
                        &item_id,
                        &task_id,
                        &title,
                        Some(&tc_id),
                        out,
                    );
                    self.emit_async_task_state(acp_sid, &task_id, item, out);
                } else if kind == "workflow"
                    && self.air_async_tasks
                    && let Some(run_id) = item
                        .get("workflowRunId")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                {
                    self.workflow_tasks
                        .entry(run_id.to_string())
                        .or_insert_with(|| run_id.to_string());
                    if !self.announced_tasks.contains(run_id) {
                        self.announced_tasks.insert(run_id.to_string());
                        out.push(Self::async_task_spawned_line(
                            acp_sid,
                            run_id,
                            &title,
                            "workflow",
                            true,
                            Some(&tc_id),
                        ));
                    }
                    if let Some(state) = Self::async_task_state(item) {
                        self.async_task_state_once(acp_sid, run_id, state, out);
                    }
                }
            }
            "monitorFeed" => {
                self.monitor_feed_card(acp_sid, ver, &item_id, item, status, out);
            }
            "reminderChild" => {
                if self.show_reminders {
                    self.reminder_child_card(acp_sid, ver, &item_id, item, status, false, out);
                } else {
                    // Silent by default: background reminder checks hold the
                    // turn open but carry no editor-facing content, so they
                    // never reach the transcript. The blocking agent ids are
                    // still logged with turn/foregroundCompleted.
                    self.items.entry(item_id).or_insert(ItemRole::Ignored);
                }
            }
            _ => {
                let (title, content, meta) = self.host_card_parts(kind, item);
                let muse_fields = Self::merge_muse_fields(
                    meta.as_deref(),
                    Self::item_muse_fields(item).as_deref(),
                );
                if !title.is_empty() {
                    let (tc_id, announced) = self.host_item_role(&item_id, "item-");
                    out.push(Self::card_line(
                        acp_sid,
                        ver,
                        !announced,
                        &tc_id,
                        &title,
                        "other",
                        msp_status(status),
                        content.as_deref(),
                        muse_fields.as_deref(),
                        item.get("truncated")
                            .is_some_and(|v| matches!(v, J::Bool(true))),
                    ));
                }
                self.items.entry(item_id).or_insert(ItemRole::Ignored);
            }
        }
    }

    /// item/delta: stream message text; tool text deltas are folded into the
    /// completion snapshot instead (v1 content arrays replace wholesale).
    pub fn on_item_delta(&mut self, acp_sid: &str, ver: u8, params: &J, out: &mut Vec<String>) {
        let item_id = match params.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return,
        };
        let field = params
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or("text")
            .to_string();
        let delta = match params.get("delta").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };
        // Tool and shell output (`visibleOutput`) streams as `output`. ACP v2
        // appends it to the card with `tool_call_content_chunk`; v1 has no
        // append, so its card keeps the completion's full content. Appends
        // stop at the display budget, and the completion still replaces the
        // whole collection with the bounded final text, so the client ends
        // with exactly what a replacement alone would have shown.
        if field == "output" {
            if ver != 2 {
                return;
            }
            // Shell output goes to the card's agent-owned terminal as raw
            // bytes, within the same budget.
            if self.shell_commands.contains_key(item_id) {
                let Some(terminal_id) = self.open_terminal(acp_sid, item_id, out) else {
                    return;
                };
                let Some((_, streamed)) = self.terminals.get_mut(item_id) else {
                    return;
                };
                let budget = max_content().saturating_sub(*streamed);
                let text: String = delta.chars().take(budget).collect();
                if text.is_empty() {
                    return;
                }
                *streamed += text.chars().count();
                out.push(Self::update_line(
                    acp_sid,
                    &format!(
                        "{{\"sessionUpdate\":\"terminal_output_chunk\",\"terminalId\":{},\"data\":{}}}",
                        esc(&terminal_id),
                        esc(&crate::json::b64(text.as_bytes()))
                    ),
                ));
                return;
            }
            let Some(ItemRole::Tool { tc_id, .. }) = self.items.get(item_id) else {
                return;
            };
            let tc_id = tc_id.clone();
            let streamed = self.streamed_output.entry(item_id.to_string()).or_insert(0);
            let budget = max_content().saturating_sub(*streamed);
            let text: String = delta.chars().take(budget).collect();
            if text.is_empty() {
                return;
            }
            *streamed += text.chars().count();
            out.push(Self::update_line(
                acp_sid,
                &format!(
                    "{{\"sessionUpdate\":\"tool_call_content_chunk\",\"toolCallId\":{},\"content\":{{\"type\":\"content\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}}}",
                    esc(&tc_id),
                    esc(&text)
                ),
            ));
            return;
        }
        if field == "text" {
            match self.items.get_mut(item_id) {
                Some(ItemRole::Message { msg_id, streamed }) => {
                    *streamed += delta.len();
                    out.push(Self::chunk(acp_sid, ver, msg_id, delta));
                }
                Some(ItemRole::Monitor {
                    tc_id,
                    title,
                    muse_fields,
                    text,
                    host_truncated,
                }) => {
                    text.push_str(delta);
                    out.push(Self::card_line(
                        acp_sid,
                        ver,
                        false,
                        tc_id,
                        title,
                        "other",
                        "in_progress",
                        Some(text.as_str()),
                        muse_fields.as_deref(),
                        *host_truncated,
                    ));
                }
                _ => {}
            }
            return;
        }
        // Reasoning summaries stream part-wise as `summary.<n>`; a part-index
        // advance is a section break, matching codex-acp's presentation.
        if let Some(part) = field.strip_prefix("summary.")
            && let Ok(part) = part.parse::<usize>()
            && let Some(ItemRole::Thought {
                msg_id,
                streamed,
                part: current,
            }) = self.items.get_mut(item_id)
        {
            let mut text = String::new();
            if part > *current {
                text.push_str("\n\n");
                *current = part;
            } else if part < *current {
                return; // stale replay of an earlier part
            }
            text.push_str(delta);
            *streamed += text.len();
            out.push(Self::thought_chunk(acp_sid, ver, msg_id, &text));
        }
    }

    /// item/completed carries the authoritative final object.
    pub fn on_item_completed(&mut self, acp_sid: &str, ver: u8, params: &J, out: &mut Vec<String>) {
        if let Some(item_id) = params
            .get("item")
            .and_then(|item| item.get("itemId"))
            .and_then(J::as_str)
        {
            self.streamed_output.remove(item_id);
        }
        let item = match params.get("item") {
            Some(i) => i,
            None => return,
        };
        let item_id = match item.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        self.observe_subagent(item);
        self.observe_workflow(item, true);
        if self.known(&item_id) {
            // A gap-refill replay of a settled item, possibly the first to
            // carry its cut-off mark.
            self.mark_cut_off(acp_sid, ver, item, out);
            return;
        }
        self.done.insert(item_id.clone());
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed");
        match kind {
            "toolCall" => {
                let (tc_id, announced) = match self.items.get(&item_id) {
                    Some(ItemRole::Tool { tc_id, announced }) => (tc_id.clone(), *announced),
                    _ => {
                        let id = item
                            .get("callId")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| mint_id("tc-", &self.idc));
                        self.items.insert(
                            item_id.clone(),
                            ItemRole::Tool {
                                tc_id: id.clone(),
                                announced: false,
                            },
                        );
                        (id, false)
                    }
                };
                let (title, tool) = Self::tool_title(item);
                let text = item
                    .get("visibleOutput")
                    .and_then(|v| v.as_str())
                    .filter(|text| !text.is_empty())
                    .or_else(|| item.get("result").and_then(|v| v.as_str()))
                    .filter(|text| !text.is_empty())
                    .or_else(|| item.get("failureReason").and_then(|v| v.as_str()))
                    .filter(|text| !text.is_empty())
                    .unwrap_or("");
                // An ACP v2 shell call shows its output in an agent-owned
                // terminal instead of a text block.
                self.note_shell(ver, &item_id, &tc_id, item);
                self.editor_calls.retain(|(id, ..)| *id != item_id);
                let terminal = self
                    .finish_terminal(acp_sid, &item_id, item, status, out)
                    .or_else(|| self.editor_terminals.remove(&item_id));
                let content = if text.is_empty() || terminal.is_some() {
                    None
                } else {
                    Some(text)
                };
                let raw = item.get("args").map(j_to_string);
                let item_fields = Self::item_muse_fields(item);
                let locations = self.tool_locations(item);
                let diff = self.patch_diffs.remove(&item_id).or(terminal);
                out.push(Self::tool_line(
                    acp_sid,
                    ver,
                    &ToolUpdate {
                        create: !announced,
                        tc_id: &tc_id,
                        name: item
                            .get("tool")
                            .and_then(J::as_str)
                            .filter(|name| !name.is_empty()),
                        title: &title,
                        kind: tool_kind(&tool),
                        status: msp_status(status),
                        content_text: content,
                        raw_input: raw.as_deref(),
                        locations: locations.as_deref(),
                        extra_content: diff.as_deref(),
                        muse_fields: item_fields.as_deref(),
                        host_truncated: item
                            .get("truncated")
                            .is_some_and(|v| matches!(v, J::Bool(true))),
                    },
                ));
                let backgrounded = item
                    .get("background")
                    .is_some_and(|v| matches!(v, J::Bool(true)))
                    || self.announced_tasks.contains(&tc_id)
                    || self.backgrounded_cards.contains(&tc_id);
                if backgrounded && self.air_async_tasks {
                    let task_id = self.async_task_id_for(&item_id, &tc_id);
                    if self.backgrounded_cards.insert(tc_id.clone()) {
                        out.push(Self::backgrounded_tool_line(acp_sid, ver, &tc_id));
                    }
                    self.announce_async_task(
                        acp_sid,
                        &item_id,
                        &task_id,
                        &title,
                        Some(&tc_id),
                        out,
                    );
                    self.emit_async_task_state(acp_sid, &task_id, item, out);
                }
                self.items.remove(&item_id);
            }
            "agentMessage" => {
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let (msg_id, streamed) = match self.items.remove(&item_id) {
                    Some(ItemRole::Message { msg_id, streamed }) => (Some(msg_id), streamed),
                    _ => (None, 0),
                };
                // Deltas are authoritative-in-motion; the completed frame is the
                // truth. Resend only if nothing streamed (missed deltas).
                let shown = if streamed == 0 && !text.is_empty() {
                    Some((msg_id.unwrap_or_else(|| mint_id("msg-", &self.idc)), text))
                } else {
                    msg_id.filter(|_| streamed > 0).map(|id| (id, ""))
                };
                if let Some((msg_id, text)) = shown {
                    // The completion always declares the reply's phase — on
                    // the resent text, or on an empty chunk after streamed
                    // text — so commentary stays distinguishable from the
                    // final answer even when the deltas carried the words.
                    let phase = Self::message_phase(item);
                    let cut = self
                        .claim_cut_off(item, &item_id)
                        .then_some(item_id.as_str());
                    let meta = Self::message_meta(Some(phase), cut)
                        .expect("a completion always declares its phase");
                    out.push(Self::marked_chunk(acp_sid, ver, &msg_id, text, &meta));
                    self.last_message = Some((item_id.clone(), msg_id));
                }
            }
            "reasoning" => {
                // The committed summary (or raw text when no summary exists)
                // is emitted exactly once when no delta was observed; deltas
                // are authoritative-in-motion and already carried the text.
                let summary = match item.get("summary") {
                    Some(J::Arr(parts)) => parts
                        .iter()
                        .filter_map(|p| p.as_str())
                        .filter(|p| !p.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                    _ => String::new(),
                };
                let text = if summary.is_empty() {
                    item.get("text").and_then(|v| v.as_str()).unwrap_or("")
                } else {
                    &summary
                };
                if item
                    .get("truncated")
                    .is_some_and(|v| matches!(v, J::Bool(true)))
                {
                    // The host saturated this surface; the durable full text
                    // stays in the log. Emit the visible prefix, never as a
                    // claim of completeness.
                    crate::msp::log(
                        "reasoning item arrived truncated; showing the bounded surface",
                    );
                }
                let streamed = match self.items.get(&item_id) {
                    Some(ItemRole::Thought { streamed, .. }) => *streamed,
                    _ => 0,
                };
                if streamed == 0 && !text.is_empty() {
                    let msg_id = mint_id("thought-", &self.idc);
                    out.push(Self::thought_chunk(acp_sid, ver, &msg_id, text));
                }
                self.items.remove(&item_id);
            }
            "compaction" => {
                if self.compaction_supported {
                    out.push(Self::standard_compaction_line(acp_sid, &item_id, item));
                } else {
                    let status = msp_status(status);
                    out.push(Self::legacy_compaction_line(
                        acp_sid,
                        ver,
                        &item_id,
                        status,
                        Self::compaction_content(item).as_deref(),
                    ));
                }
            }
            "subagent" if self.native_subagents => {
                if let Some(child) = item.get("childSessionId").and_then(|v| v.as_str())
                    && !child.is_empty()
                {
                    if !self.spawned_subagents.contains(child) {
                        // A completion can be the first durable-sourced frame a
                        // reconnecting client sees; spawn before settling.
                        self.spawned_subagents.insert(child.to_string());
                        let name = item
                            .get("agentPath")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .unwrap_or("subagent");
                        let task = item
                            .get("objective")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .unwrap_or("Delegated task");
                        out.push(Self::subagent_spawned_line(
                            acp_sid, child, name, task, None,
                        ));
                    }
                    if let Some(state) = Self::subagent_state(item) {
                        out.push(Self::subagent_state_line(acp_sid, child, state));
                    }
                }
                self.items.remove(&item_id);
            }
            "subagent" | "workflow" | "userShell" | "hookRun" => {
                let (tc_id, announced) = self.host_item_role(&item_id, kind);
                let (title, content, meta) = self.host_card_parts(kind, item);
                let muse_fields = Self::merge_muse_fields(
                    meta.as_deref(),
                    Self::item_muse_fields(item).as_deref(),
                );
                // An ACP v2 user shell that ran settles in its terminal.
                let terminal = if kind == "userShell" {
                    self.note_shell(ver, &item_id, &tc_id, item);
                    self.finish_terminal(acp_sid, &item_id, item, status, out)
                } else {
                    None
                };
                let content = if terminal.is_some() { None } else { content };
                if !title.is_empty() {
                    out.push(Self::card_line(
                        acp_sid,
                        ver,
                        !announced,
                        &tc_id,
                        &title,
                        "other",
                        msp_status(status),
                        content.as_deref(),
                        muse_fields.as_deref(),
                        item.get("truncated")
                            .is_some_and(|v| matches!(v, J::Bool(true))),
                    ));
                }
                if let Some(terminal) = terminal {
                    out.push(Self::update_line(
                        acp_sid,
                        &format!(
                            "{{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":{},\"content\":[{terminal}]}}",
                            esc(&tc_id)
                        ),
                    ));
                }
                if kind == "userShell" && self.air_async_tasks {
                    let task_id = self.async_task_id_for(&item_id, &tc_id);
                    self.announce_async_task(
                        acp_sid,
                        &item_id,
                        &task_id,
                        &title,
                        Some(&tc_id),
                        out,
                    );
                    self.emit_async_task_state(acp_sid, &task_id, item, out);
                }
                if kind == "workflow"
                    && self.air_async_tasks
                    && let Some(run_id) = item
                        .get("workflowRunId")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                {
                    self.workflow_tasks
                        .entry(run_id.to_string())
                        .or_insert_with(|| run_id.to_string());
                    if !self.announced_tasks.contains(run_id) {
                        self.announced_tasks.insert(run_id.to_string());
                        out.push(Self::async_task_spawned_line(
                            acp_sid,
                            run_id,
                            &title,
                            "workflow",
                            true,
                            Some(&tc_id),
                        ));
                    }
                    if let Some(state) = Self::async_task_state(item) {
                        self.async_task_state_once(acp_sid, run_id, state, out);
                    }
                }
                self.items.remove(&item_id);
            }
            "monitorFeed" => {
                self.monitor_feed_card(acp_sid, ver, &item_id, item, status, out);
                self.items.remove(&item_id);
            }
            "reminderChild" => {
                if self.show_reminders {
                    self.reminder_child_card(acp_sid, ver, &item_id, item, status, true, out);
                } else {
                    // Silent by default (see the snapshot arm): release any
                    // role so a terminal frame cannot strand a card that was
                    // never announced.
                    self.items.remove(&item_id);
                }
            }
            _ => {
                // An unknown future kind. If a card was announced at start (it
                // is tracked as a Tool role) but this terminal snapshot carries
                // no title, still settle it with a fallback title so the tool
                // never stays in_progress (#1007).
                let announced_card =
                    matches!(self.items.get(&item_id), Some(ItemRole::Tool { .. }));
                let (tc_id, announced) = self.host_item_role(&item_id, "item-");
                let (title, content, meta) = self.host_card_parts(kind, item);
                let muse_fields = Self::merge_muse_fields(
                    meta.as_deref(),
                    Self::item_muse_fields(item).as_deref(),
                );
                if !title.is_empty() || announced_card {
                    let card_title = if title.is_empty() {
                        fallback_card_title(kind)
                    } else {
                        title.clone()
                    };
                    out.push(Self::card_line(
                        acp_sid,
                        ver,
                        !announced,
                        &tc_id,
                        &card_title,
                        "other",
                        msp_status(status),
                        content.as_deref(),
                        muse_fields.as_deref(),
                        item.get("truncated")
                            .is_some_and(|v| matches!(v, J::Bool(true))),
                    ));
                }
                self.items.remove(&item_id);
            }
        }
    }
}

/// MSP turn terminal -> ACP stop reason (v1 + v2 share the vocabulary).
/// `failed` and unknown terminals map to the implementation-specific
/// `_failed` (spec-sanctioned `_` prefix): never omit completion metadata.
pub fn stop_reason(terminal: &str) -> &'static str {
    match terminal {
        "completed" => "end_turn",
        "cancelled" => "cancelled",
        _ => "_failed",
    }
}

#[cfg(test)]
mod corpus_tests {
    use super::{SessionFold, max_content, sanitize_monitor_description, trunc};
    use crate::json::{J, j_to_string, parse_json};
    use std::path::{Path, PathBuf};

    fn transcript_paths() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/protocol/transcripts");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&root)
            .expect("vendored transcript corpus")
            .filter_map(|e| e.ok())
            .map(|e| e.path().join("transcript.ndjson"))
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        assert!(
            !paths.is_empty(),
            "tests/protocol/transcripts is empty; corpus was not vendored"
        );
        paths
    }

    /// Replay every server-side item event in the pinned SDK corpus through
    /// the notification fold. Unknown kinds and future shapes must tolerate
    /// (not panic), and every emitted ACP frame must parse with our own
    /// dependency-free JSON parser.
    #[test]
    fn replays_every_vendored_transcript_through_the_fold() {
        let mut scenarios = 0usize;
        let mut item_events = 0usize;
        let mut emitted = 0usize;
        for path in transcript_paths() {
            let scenario = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("scenario")
                .to_string();
            scenarios += 1;
            let mut fold = SessionFold::new();
            let mut out = Vec::new();
            for (lineno, line) in std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{scenario}: read: {e}"))
                .lines()
                .enumerate()
            {
                if line.trim().is_empty() {
                    continue;
                }
                let envelope = parse_json(line)
                    .unwrap_or_else(|e| panic!("{scenario}:{lineno}: envelope: {e}"));
                if envelope.get("dir").and_then(|v| v.as_str()) != Some("server") {
                    continue;
                }
                let raw = envelope
                    .get("raw")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{scenario}:{lineno}: missing raw"));
                let frame =
                    parse_json(raw).unwrap_or_else(|e| panic!("{scenario}:{lineno}: frame: {e}"));
                let method = frame.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let params = frame.get("params").cloned().unwrap_or(J::Null);
                if !matches!(
                    method,
                    "item/started" | "item/updated" | "item/delta" | "item/completed"
                ) {
                    continue;
                }
                item_events += 1;
                match method {
                    "item/started" | "item/updated" => {
                        let item = params.get("item").cloned().unwrap_or(J::Null);
                        fold.on_item_snapshot(&scenario, 2, &item, &mut out);
                    }
                    "item/delta" => fold.on_item_delta(&scenario, 2, &params, &mut out),
                    "item/completed" => fold.on_item_completed(&scenario, 2, &params, &mut out),
                    _ => unreachable!("item method filter above"),
                }
            }
            for line in &out {
                parse_json(line)
                    .unwrap_or_else(|e| panic!("{scenario}: emitted invalid JSON {line}: {e}"));
                emitted += 1;
            }
        }
        // The corpus must stay meaningful: if these ever hit zero the vendor
        // step or the schema drifted in a way this test can no longer see.
        assert!(scenarios >= 40, "unexpectedly small corpus: {scenarios}");
        assert!(
            item_events >= 60,
            "too few item events replayed: {item_events}"
        );
        assert!(emitted >= 30, "fold emitted too little: {emitted}");
    }

    fn workflow_item(revision: u64, status: &str, children: &str) -> J {
        parse_json(&format!(
            r#"{{"itemId":"it-wf","kind":"workflow","status":"{status}","revision":{revision},"workflowRunId":"wfr-1","children":[{children}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn workflow_children_track_the_latest_attempt() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_snapshot(
            "sid",
            2,
            &workflow_item(
                2,
                "inProgress",
                r#"{"childId":"c1","attempt":1,"status":"started","label":"one"},{"childId":"","attempt":1,"status":"started"},{"childId":"c3","attempt":0,"status":"started"}"#,
            ),
            &mut out,
        );
        let children = fold.workflow_children();
        assert_eq!(
            children.len(),
            1,
            "unkeyed children are skipped: {children:?}"
        );
        assert_eq!(children[0].child_id, "c1");
        assert_eq!(children[0].attempt, 1);
        assert_eq!(children[0].workflow_run_id, "wfr-1");

        // A retry's snapshot advances the attempt.
        fold.on_item_snapshot(
            "sid",
            2,
            &workflow_item(
                4,
                "inProgress",
                r#"{"childId":"c1","attempt":2,"status":"started"}"#,
            ),
            &mut out,
        );
        assert_eq!(fold.workflow_children()[0].attempt, 2);

        // An older revision delivered late never rolls it back.
        fold.on_item_snapshot(
            "sid",
            2,
            &workflow_item(
                3,
                "inProgress",
                r#"{"childId":"c1","attempt":1,"status":"started"}"#,
            ),
            &mut out,
        );
        assert_eq!(fold.workflow_children()[0].attempt, 2);

        // A terminal workflow has nothing left to control.
        let completed = parse_json(&format!(
            r#"{{"sessionId":"s","item":{}}}"#,
            j_to_string(&workflow_item(
                5,
                "completed",
                r#"{"childId":"c1","attempt":2,"status":"completed"}"#
            ))
        ))
        .unwrap();
        fold.on_item_completed("sid", 2, &completed, &mut out);
        assert!(fold.workflow_children().is_empty());
    }

    #[test]
    fn monitor_descriptions_are_sanitized_to_one_safe_line() {
        let description = " Build\twatcher \n\u{1b}[31m\u{202e}ready \u{2066}now\u{2069} ";
        let sanitized = sanitize_monitor_description(description);
        assert_eq!(sanitized, "Build watcher [31m ready now");
        assert!(!sanitized.chars().any(char::is_control));
        assert!(!sanitized.contains('\u{202e}'));
        assert!(!sanitized.contains('\u{2066}'));

        let fold = SessionFold::new();
        let fallback = J::Obj(vec![(
            "fallbackText".to_string(),
            J::Str("Monitor event: Fallback label".to_string()),
        )]);
        let (title, _, meta) = fold.host_card_parts("monitorFeed", &fallback);
        assert_eq!(title, "Monitor: Fallback label");
        assert!(
            meta.as_deref()
                .is_some_and(|meta| meta.contains("\"monitorDescription\":\"Fallback label\""))
        );

        let long = J::Obj(vec![(
            "monitorDescription".to_string(),
            J::Str("x".repeat(max_content() + 1000)),
        )]);
        let (title, _, _) = fold.host_card_parts("monitorFeed", &long);
        assert!(title.contains("…[truncated]"), "{title}");
        assert!(title.chars().count() <= max_content(), "{title}");
    }

    #[test]
    fn monitor_feed_streams_into_one_stable_card() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        let started = parse_json(
            r#"{"itemId":"feed-1","kind":"monitorFeed","monitorId":"monitor-1","monitorDescription":"Build\twatcher\u001b[31m","status":"inProgress","turnId":null,"revision":1,"text":"watching\n"}"#,
        )
        .unwrap();
        fold.on_item_snapshot("sid", 1, &started, &mut out);
        let initial = parse_json(&out[0]).unwrap();
        let update = initial.get("params").and_then(|p| p.get("update")).unwrap();
        assert_eq!(
            update.get("sessionUpdate").and_then(J::as_str),
            Some("tool_call")
        );
        assert_eq!(
            update.get("toolCallId").and_then(J::as_str),
            Some("monitor-monitor-1")
        );
        assert!(
            update
                .get("title")
                .and_then(J::as_str)
                .is_some_and(|title| title == "Monitor: Build watcher [31m"),
            "{}",
            j_to_string(&initial)
        );
        let muse = update
            .get("_meta")
            .and_then(|meta| meta.get("muse"))
            .unwrap();
        assert_eq!(muse.get("monitorId").and_then(J::as_str), Some("monitor-1"));
        assert_eq!(
            muse.get("monitorDescription").and_then(J::as_str),
            Some("Build watcher [31m")
        );
        assert_eq!(
            muse.get("itemKind").and_then(J::as_str),
            Some("monitorFeed")
        );

        let delta =
            parse_json(r#"{"itemId":"feed-1","field":"text","delta":"event one\n"}"#).unwrap();
        fold.on_item_delta("sid", 1, &delta, &mut out);
        let streamed = parse_json(&out[1]).unwrap();
        let update = streamed
            .get("params")
            .and_then(|p| p.get("update"))
            .unwrap();
        assert_eq!(
            update.get("status").and_then(J::as_str),
            Some("in_progress")
        );
        assert!(
            update
                .get("content")
                .is_some_and(|content| j_to_string(content).contains("watching\\nevent one"))
        );
        assert!(
            update
                .get("toolCallId")
                .and_then(J::as_str)
                .is_some_and(|id| id == "monitor-monitor-1")
        );

        let updated = parse_json(
            r#"{"itemId":"feed-1","kind":"monitorFeed","monitorId":"monitor-1","monitorDescription":"Build\twatcher\u001b[31m","status":"inProgress","turnId":null,"revision":2,"text":"watching\nevent one\n","truncated":true}"#,
        )
        .unwrap();
        let mut revised = Vec::new();
        fold.on_item_snapshot("sid", 1, &updated, &mut revised);
        let revised = parse_json(&revised[0]).unwrap();
        let update = revised.get("params").and_then(|p| p.get("update")).unwrap();
        assert!(
            update
                .get("_meta")
                .and_then(|meta| meta.get("muse"))
                .and_then(|muse| muse.get("truncated"))
                .and_then(|cut| cut.get("source"))
                .and_then(J::as_str)
                == Some("host")
        );

        let completed_item = parse_json(
            r#"{"itemId":"feed-1","kind":"monitorFeed","monitorId":"monitor-1","monitorDescription":"Build\twatcher\u001b[31m","status":"completed","turnId":null,"revision":3,"text":"watching\nevent one\n","truncated":true}"#,
        )
        .unwrap();
        let completed = J::Obj(vec![("item".to_string(), completed_item.clone())]);
        let mut terminal = Vec::new();
        fold.on_item_completed("sid", 1, &completed, &mut terminal);
        let completed = parse_json(&terminal[0]).unwrap();
        let update = completed
            .get("params")
            .and_then(|p| p.get("update"))
            .unwrap();
        assert_eq!(update.get("status").and_then(J::as_str), Some("completed"));
        assert!(
            update
                .get("_meta")
                .and_then(|meta| meta.get("muse"))
                .and_then(|muse| muse.get("truncated"))
                .and_then(|cut| cut.get("source"))
                .and_then(J::as_str)
                == Some("host"),
            "{}",
            j_to_string(&completed)
        );
        let mut duplicate = Vec::new();
        fold.on_item_completed("sid", 1, &completed, &mut duplicate);
        assert!(
            duplicate.is_empty(),
            "a terminal refill must not settle the monitor twice: {duplicate:?}"
        );

        // A later feed batch reuses the monitor card and is an update on v1.
        let second = parse_json(
            r#"{"itemId":"feed-2","kind":"monitorFeed","monitorId":"monitor-1","monitorDescription":"Build\twatcher","status":"inProgress","turnId":null,"revision":1,"text":"event two\n"}"#,
        )
        .unwrap();
        let mut reused = Vec::new();
        fold.on_item_snapshot("sid", 1, &second, &mut reused);
        let reused = parse_json(&reused[0]).unwrap();
        let update = reused.get("params").and_then(|p| p.get("update")).unwrap();
        assert_eq!(
            update.get("sessionUpdate").and_then(J::as_str),
            Some("tool_call_update")
        );
        assert_eq!(
            update.get("toolCallId").and_then(J::as_str),
            Some("monitor-monitor-1")
        );

        // Replayed terminal history uses the same monitor identity and title.
        let mut replayed = SessionFold::new();
        let mut replay = Vec::new();
        replayed.replay_item("sid", 1, &completed_item, &mut replay);
        let replay = parse_json(&replay[0]).unwrap();
        let update = replay.get("params").and_then(|p| p.get("update")).unwrap();
        assert_eq!(update.get("status").and_then(J::as_str), Some("completed"));
        assert_eq!(
            update.get("toolCallId").and_then(J::as_str),
            Some("monitor-monitor-1")
        );
        assert_eq!(
            update.get("title").and_then(J::as_str),
            Some("Monitor: Build watcher [31m")
        );
    }

    #[test]
    fn foreground_task_rows_are_never_seeded_as_async_tasks() {
        let mut fold = SessionFold::new();
        fold.air_async_tasks = true;
        let mut out = Vec::new();
        let foreground = parse_json(
            r#"{"taskId":"it-fg","kind":"tool.workspace-shell","status":"inProgress","tool":"workspace-shell","executionMode":"foreground"}"#,
        )
        .unwrap();
        assert!(!fold.seed_async_task("sid", &foreground, &mut out));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(fold.msp_task_id("it-fg"), None);

        // Explicit background, an unknown mode (ignored), and an absent mode
        // (an older host) are all background work.
        for (id, mode) in [
            ("it-bg", r#","executionMode":"background""#),
            ("it-new", r#","executionMode":"detached""#),
            ("it-old", ""),
        ] {
            let row = parse_json(&format!(
                r#"{{"taskId":"{id}","kind":"tool.workspace-shell","status":"inProgress","tool":"workspace-shell"{mode}}}"#
            ))
            .unwrap();
            assert!(fold.seed_async_task("sid", &row, &mut out), "{id}");
        }
        assert_eq!(out.len(), 3, "{out:?}");
    }

    #[test]
    fn seeded_async_tasks_keep_their_id_when_the_item_arrives() {
        let mut fold = SessionFold::new();
        fold.air_async_tasks = true;
        let mut out = Vec::new();
        let row = parse_json(
            r#"{"taskId":"it-bg","kind":"tool.workspace-shell","status":"inProgress","tool":"workspace-shell"}"#,
        )
        .unwrap();
        assert!(fold.seed_async_task("sid", &row, &mut out));
        assert!(
            !fold.seed_async_task("sid", &row, &mut out),
            "a known row is never announced twice"
        );
        let spawned = out.join("\n");
        assert!(
            spawned.contains("\"asyncTaskId\":\"it-bg\"")
                && spawned.contains("\"name\":\"workspace-shell\"")
                && spawned.contains("\"taskType\":\"shell\""),
            "{spawned}"
        );
        assert_eq!(fold.msp_task_id("it-bg"), Some("it-bg"));

        // The item's own terminal reuses the seeded id, not the call id.
        let mut out = Vec::new();
        let wrapped = parse_json(
            r#"{"item":{"itemId":"it-bg","kind":"toolCall","callId":"call-bg","status":"completed","revision":2,"tool":"workspace-shell","args":"{}","background":true}}"#,
        )
        .unwrap();
        fold.on_item_completed("sid", 2, &wrapped, &mut out);
        let settled = out.join("\n");
        assert!(
            settled.contains("async_task_state_update")
                && settled.contains("\"asyncTaskId\":\"it-bg\"")
                && settled.contains("\"state\":\"completed\""),
            "{settled}"
        );
        assert!(
            !settled.contains("\"asyncTaskId\":\"call-bg\""),
            "{settled}"
        );
    }

    #[test]
    fn hook_run_cards_use_the_inventory_when_present() {
        let item = parse_json(
            r#"{"itemId":"h1","kind":"hookRun","status":"completed","revision":2,"hookKey":"fmt","event":"postToolUse","label":"Format on save","runStatus":"completed","reason":"formatted 2 files"}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        fold.note_hook_inventory(
            &parse_json(
                r#"{"hooks":[{"key":"fmt","event":"postToolUse","handlerKind":"project","enabled":true}]}"#,
            )
            .unwrap(),
        );
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &item, &mut out);
        let card = out.join("\n");
        assert!(card.contains("Hook Format on save"), "{card}");
        assert!(card.contains("\"handlerKind\":\"project\""), "{card}");
        assert!(card.contains("\"enabled\":true"), "{card}");
        assert!(card.contains("\"runStatus\":\"completed\""), "{card}");
        assert!(card.contains("formatted 2 files"), "{card}");

        // Without a matching inventory row the card still renders, without
        // inventing a handler origin.
        let mut bare = SessionFold::new();
        let mut out = Vec::new();
        bare.on_item_snapshot("sid", 2, &item, &mut out);
        let card = out.join("\n");
        assert!(card.contains("Hook Format on save"), "{card}");
        assert!(!card.contains("handlerKind"), "{card}");
    }

    #[test]
    fn workflow_runs_track_pause_and_resume_supersession() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        let live = workflow_item(
            2,
            "inProgress",
            r#"{"childId":"c1","attempt":1,"status":"started"}"#,
        );
        fold.on_item_snapshot("sid", 2, &live, &mut out);
        let runs = fold.workflow_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].workflow_run_id, "wfr-1");
        assert!(!runs[0].paused);
        assert_eq!(runs[0].child_count, 1);

        // A pause folds as the item's `paused` member.
        let paused = parse_json(
            r#"{"itemId":"it-wf","kind":"workflow","status":"inProgress","revision":3,"workflowRunId":"wfr-1","paused":true,"children":[{"childId":"c1","attempt":1,"status":"started"}]}"#,
        )
        .unwrap();
        fold.on_item_snapshot("sid", 2, &paused, &mut out);
        let runs = fold.workflow_runs();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].paused);

        // A resumed launch supersedes the paused run it names.
        let resumed = parse_json(
            r#"{"itemId":"it-wf-2","kind":"workflow","status":"inProgress","revision":2,"workflowRunId":"wfr-2","resumeFromRunId":"wfr-1","children":[{"childId":"c1","attempt":1,"status":"started"}]}"#,
        )
        .unwrap();
        fold.on_item_snapshot("sid", 2, &resumed, &mut out);
        let runs = fold.workflow_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].workflow_run_id, "wfr-2");
        assert!(!runs[0].paused);

        // A late observation of the superseded run never brings it back.
        fold.on_item_snapshot("sid", 2, &paused, &mut out);
        let runs = fold.workflow_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].workflow_run_id, "wfr-2");

        // Muse 1.4.6 reuses the run id for the resumed generation, restarting
        // its revision sequence: the resume fact is the generation boundary.
        let same_id_resume = parse_json(
            r#"{"itemId":"it-wf-3","kind":"workflow","status":"inProgress","revision":1,"workflowRunId":"wfr-2","resumeFromRunId":"wfr-2","children":[{"childId":"c1","attempt":1,"status":"started"}]}"#,
        )
        .unwrap();
        fold.on_item_snapshot("sid", 2, &same_id_resume, &mut out);
        let runs = fold.workflow_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].workflow_run_id, "wfr-2");
        assert!(!runs[0].paused);
        // The resumed generation's later snapshots still land.
        let advanced = parse_json(
            r#"{"itemId":"it-wf-3","kind":"workflow","status":"inProgress","revision":3,"workflowRunId":"wfr-2","children":[{"childId":"c1","attempt":2,"status":"started"}]}"#,
        )
        .unwrap();
        fold.on_item_snapshot("sid", 2, &advanced, &mut out);
        assert_eq!(fold.workflow_children()[0].attempt, 2);
    }

    #[test]
    fn tool_title_uses_args_command_instead_of_bare_tool() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-1","kind":"toolCall","status":"inProgress","tool":"bash","callId":"call-1","args":"{\"command\":\"cargo test --help\"}"}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &item, &mut out);
        assert_eq!(out.len(), 1, "snapshot should announce: {out:?}");
        assert!(
            out[0].contains("bash: cargo test --help"),
            "title should carry the command, not bare tool: {}",
            out[0]
        );
    }

    // The literals are Unix paths; Windows resolution is covered by the
    // fake-host test, which builds its expectations from real paths.
    #[cfg(unix)]
    #[test]
    fn tool_locations_come_only_from_explicit_file_tool_paths() {
        let mut fold = SessionFold::new();
        fold.workspace_root = "/ws".to_string();
        let item = |tool: &str, args: &str| {
            parse_json(&format!(
                r#"{{"tool":{},"args":{}}}"#,
                crate::json::esc(tool),
                crate::json::esc(args)
            ))
            .unwrap()
        };
        assert_eq!(
            fold.tool_locations(&item("read_file", r#"{"path":"src/a.rs","offset":3}"#))
                .as_deref(),
            Some(r#"[{"path":"/ws/src/a.rs"}]"#)
        );
        assert_eq!(
            fold.tool_locations(&item("write_file", r#"{"path":"/abs/./b/../c.txt"}"#))
                .as_deref(),
            Some(r#"[{"path":"/abs/c.txt"}]"#)
        );
        assert_eq!(
            fold.tool_locations(&item("move_file", r#"{"from":"a","to":"a"}"#))
                .as_deref(),
            Some(r#"[{"path":"/ws/a"}]"#)
        );
        for (tool, args) in [
            ("bash", r#"{"command":"cat src/a.rs > out.txt"}"#),
            ("search", r#"{"pattern":"x","paths":["src"]}"#),
            ("mcp__probe__read", r#"{"path":"src/a.rs"}"#),
            ("read_file", r#"{"path":"../../../../.."}"#),
            ("read_file", r#"{"path":""}"#),
            ("read_file", "not json"),
        ] {
            assert_eq!(
                fold.tool_locations(&item(tool, args)),
                None,
                "{tool} {args}"
            );
        }
        fold.workspace_root = String::new();
        assert_eq!(
            fold.tool_locations(&item("read_file", r#"{"path":"relative.rs"}"#)),
            None,
            "a relative path needs an absolute workspace root"
        );
    }

    #[test]
    fn streamed_tool_output_appends_content_chunks_on_v2_only() {
        let started = parse_json(
            r#"{"itemId":"it-run","kind":"toolCall","callId":"call-run","tool":"web_fetch","args":"{\"url\":\"https://example.com\"}","status":"inProgress","revision":1}"#,
        )
        .unwrap();
        let delta = |text: &str| {
            parse_json(&format!(
                r#"{{"itemId":"it-run","field":"output","delta":{},"sessionId":"s","viewCursor":"c"}}"#,
                crate::json::esc(text)
            ))
            .unwrap()
        };
        for ver in [1u8, 2] {
            let mut fold = SessionFold::new();
            let mut out = Vec::new();
            fold.on_item_snapshot("sid", ver, &started, &mut out);
            out.clear();
            fold.on_item_delta("sid", ver, &delta("building\n"), &mut out);
            fold.on_item_delta("sid", ver, &delta("done\n"), &mut out);
            // A delta for an item without a card has nowhere to go.
            fold.on_item_delta(
                "sid",
                ver,
                &parse_json(r#"{"itemId":"it-unknown","field":"output","delta":"x"}"#).unwrap(),
                &mut out,
            );
            if ver == 1 {
                assert!(out.is_empty(), "v1 has no append: {out:?}");
                continue;
            }
            assert_eq!(out.len(), 2, "{out:?}");
            let chunk = parse_json(&out[0]).unwrap();
            let update = chunk.get("params").and_then(|p| p.get("update")).unwrap();
            assert_eq!(
                update.get("sessionUpdate").and_then(J::as_str),
                Some("tool_call_content_chunk")
            );
            assert_eq!(
                update.get("toolCallId").and_then(J::as_str),
                Some("call-run")
            );
            assert_eq!(
                update
                    .get("content")
                    .and_then(|c| c.get("content"))
                    .and_then(|c| c.get("text"))
                    .and_then(J::as_str),
                Some("building\n")
            );
            // The completion still replaces the whole collection.
            out.clear();
            fold.on_item_completed(
                "sid",
                ver,
                &parse_json(
                    r#"{"item":{"itemId":"it-run","kind":"toolCall","callId":"call-run","tool":"web_fetch","status":"completed","visibleOutput":"building\ndone\n"}}"#,
                )
                .unwrap(),
                &mut out,
            );
            assert!(
                out[0].contains(r#""content":[{"type":"content","content":{"type":"text","text":"building\ndone\n"}}]"#),
                "{out:?}"
            );
        }
    }

    fn updates(out: &[String]) -> Vec<J> {
        out.iter()
            .map(|line| {
                parse_json(line)
                    .unwrap()
                    .get("params")
                    .and_then(|p| p.get("update"))
                    .cloned()
                    .unwrap()
            })
            .collect()
    }

    fn field<'a>(update: &'a J, key: &str) -> Option<&'a str> {
        update.get(key).and_then(J::as_str)
    }

    #[test]
    fn v2_shell_calls_stream_into_an_agent_owned_terminal() {
        let started = parse_json(
            r#"{"itemId":"it-sh","kind":"toolCall","callId":"call-sh","tool":"bash","args":"{\"command\":\"make\",\"workdir\":\"/ws/sub\"}","status":"inProgress"}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        fold.workspace_root = "/ws".to_string();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &started, &mut out);
        // Nothing ran yet: no terminal for a call that may still be refused.
        assert!(!out.iter().any(|l| l.contains("terminal")), "{out:?}");
        out.clear();
        fold.on_item_delta(
            "sid",
            2,
            &parse_json(r#"{"itemId":"it-sh","field":"output","delta":"ok\n"}"#).unwrap(),
            &mut out,
        );
        let u = updates(&out);
        assert_eq!(field(&u[0], "sessionUpdate"), Some("terminal_update"));
        assert_eq!(field(&u[0], "terminalId"), Some("term-it-sh"));
        assert_eq!(field(&u[0], "command"), Some("make"));
        if cfg!(unix) {
            assert_eq!(field(&u[0], "cwd"), Some("/ws/sub"));
        }
        assert_eq!(field(&u[1], "sessionUpdate"), Some("tool_call_update"));
        assert_eq!(
            crate::json::j_to_string(u[1].get("content").unwrap()),
            r#"[{"type":"terminal","terminalId":"term-it-sh"}]"#
        );
        assert_eq!(field(&u[2], "sessionUpdate"), Some("terminal_output_chunk"));
        assert_eq!(field(&u[2], "data"), Some("b2sK"));
        out.clear();
        fold.on_item_completed(
            "sid",
            2,
            &parse_json(
                r#"{"item":{"itemId":"it-sh","kind":"toolCall","callId":"call-sh","tool":"bash","status":"completed","visibleOutput":"ok\n"}}"#,
            )
            .unwrap(),
            &mut out,
        );
        let u = updates(&out);
        assert_eq!(field(&u[0], "sessionUpdate"), Some("terminal_update"));
        assert_eq!(
            u[0].get("output")
                .and_then(|o| o.get("data"))
                .and_then(J::as_str),
            Some("b2sK")
        );
        // A shell call records no exit code: exited, nothing invented.
        assert_eq!(
            crate::json::j_to_string(u[0].get("exitStatus").unwrap()),
            "{}"
        );
        let card = u.last().unwrap();
        assert_eq!(field(card, "status"), Some("completed"));
        assert_eq!(
            crate::json::j_to_string(card.get("content").unwrap()),
            r#"[{"type":"terminal","terminalId":"term-it-sh"}]"#
        );
    }

    #[test]
    fn shell_terminals_need_a_run_and_v2() {
        let rejected = parse_json(
            r#"{"item":{"itemId":"it-no","kind":"toolCall","callId":"call-no","tool":"bash","args":"{\"command\":\"rm -rf x\"}","status":"rejected","failureReason":"denied"}}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 2, &rejected, &mut out);
        assert!(!out.iter().any(|l| l.contains("terminal")), "{out:?}");
        assert!(
            out[0].contains("denied"),
            "the refusal keeps its text: {out:?}"
        );
        let ran = parse_json(
            r#"{"item":{"itemId":"it-v1","kind":"toolCall","callId":"call-v1","tool":"bash","args":"{\"command\":\"ls\"}","status":"completed","visibleOutput":"a\n"}}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 1, &ran, &mut out);
        assert!(!out.iter().any(|l| l.contains("terminal")), "{out:?}");
        assert!(out[0].contains(r#""text":"a\n""#), "{out:?}");
    }

    #[test]
    fn v2_user_shells_settle_their_terminal_with_the_exit_code() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_snapshot(
            "sid",
            2,
            &parse_json(
                r#"{"itemId":"it-us","kind":"userShell","commandText":"cargo test","status":"inProgress"}"#,
            )
            .unwrap(),
            &mut out,
        );
        out.clear();
        fold.on_item_completed(
            "sid",
            2,
            &parse_json(
                r#"{"item":{"itemId":"it-us","kind":"userShell","commandText":"cargo test","status":"completed","visibleOutput":"fail\n","exitCode":101,"exitSignal":9}}"#,
            )
            .unwrap(),
            &mut out,
        );
        let u = updates(&out);
        let settled = u
            .iter()
            .find(|u| u.get("exitStatus").is_some())
            .expect("exit status");
        assert_eq!(
            crate::json::j_to_string(settled.get("exitStatus").unwrap()),
            r#"{"exitCode":101,"_meta":{"muse":{"exitSignal":9}}}"#
        );
        assert_eq!(field(&u[0], "command"), Some("cargo test"));
        let last = u.last().unwrap();
        assert_eq!(
            crate::json::j_to_string(last.get("content").unwrap()),
            r#"[{"type":"terminal","terminalId":"term-it-us"}]"#
        );
        assert!(
            !out.iter().any(|l| l.contains("exited with code")),
            "the terminal carries the exit, not a text block: {out:?}"
        );
    }

    #[test]
    fn streamed_tool_output_stops_at_the_display_budget() {
        let started = parse_json(
            r#"{"itemId":"it-big","kind":"toolCall","callId":"call-big","tool":"web_fetch","status":"inProgress"}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &started, &mut out);
        out.clear();
        let big = "x".repeat(max_content() + 10);
        let delta = parse_json(&format!(
            r#"{{"itemId":"it-big","field":"output","delta":"{big}"}}"#
        ))
        .unwrap();
        fold.on_item_delta("sid", 2, &delta, &mut out);
        fold.on_item_delta("sid", 2, &delta, &mut out);
        assert_eq!(out.len(), 1, "appends stop at the budget");
        let chunk = parse_json(&out[0]).unwrap();
        let text = chunk
            .get("params")
            .and_then(|p| p.get("update"))
            .and_then(|u| u.get("content"))
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("text"))
            .and_then(J::as_str)
            .unwrap();
        assert_eq!(text.chars().count(), max_content());
    }

    #[test]
    fn epochs_render_as_utc_timestamps() {
        for (ms, text) in [
            (0, "1970-01-01T00:00:00Z"),
            (1_792_432_422_000, "2026-10-19T17:53:42Z"),
            (951_782_400_000, "2000-02-29T00:00:00Z"),
            (4_102_444_799_000, "2099-12-31T23:59:59Z"),
        ] {
            assert_eq!(super::utc_from_epoch_ms(ms), text);
        }
    }

    fn completed_card(item: &str) -> J {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_completed(
            "sid",
            2,
            &parse_json(&format!(r#"{{"item":{item}}}"#)).unwrap(),
            &mut out,
        );
        parse_json(&out[0])
            .unwrap()
            .get("params")
            .and_then(|p| p.get("update"))
            .cloned()
            .unwrap()
    }

    fn card_text(update: &J) -> String {
        update
            .get("content")
            .and_then(|c| match c {
                J::Arr(items) => items.first().cloned(),
                _ => None,
            })
            .and_then(|c| c.get("content").and_then(|c| c.get("text")).cloned())
            .and_then(|t| t.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    #[test]
    fn damaged_ranges_state_their_cost_and_the_safe_fork_cut() {
        let update = completed_card(
            r#"{"itemId":"dr-1","kind":"damagedRange","turnId":null,"status":"completed","fallbackText":"Part of this session log is unreadable","damagedRange":{"missingRecords":3,"unreadableLines":1,"readableBeforeAt":"2026-10-19T10:00:00Z","readableAfterAt":"2026-10-19T10:05:00Z","lastGoodTurnId":"turn-7"}}"#,
        );
        assert_eq!(
            update.get("title").and_then(J::as_str),
            Some("Damaged session log range")
        );
        assert_eq!(
            card_text(&update),
            "Part of this session log is unreadable\n3 missing records, 1 unreadable line\nReadable before 2026-10-19T10:00:00Z and again from 2026-10-19T10:05:00Z\nSafe fork point: turn turn-7, the last finished turn before the damage"
        );
        let range = update
            .get("_meta")
            .and_then(|m| m.get("muse"))
            .and_then(|m| m.get("damagedRange"))
            .unwrap();
        assert_eq!(range.get("missingRecords").and_then(J::as_u64), Some(3));
        // A range that starts the log has no before stamp and no fork cut.
        let update = completed_card(
            r#"{"itemId":"dr-2","kind":"damagedRange","status":"completed","damagedRange":{"missingRecords":0,"unreadableLines":2,"readableAfterAt":"2026-10-19T10:05:00Z"}}"#,
        );
        assert_eq!(
            card_text(&update),
            "0 missing records, 2 unreadable lines\nThe log starts damaged; readable again from 2026-10-19T10:05:00Z"
        );
    }

    #[test]
    fn scheduled_fires_carry_their_occurrence_facts() {
        let update = completed_card(
            r#"{"itemId":"sf-1","kind":"scheduledFire","status":"completed","jobId":"job-9","deliveryNumber":4,"scheduledFireAtMs":1792432422000,"nextFireAtMs":1792436022000}"#,
        );
        assert_eq!(
            update.get("title").and_then(J::as_str),
            Some("Scheduled job job-9 fired")
        );
        assert_eq!(
            card_text(&update),
            "Occurrence 4\nScheduled for 2026-10-19T17:53:42Z\nNext fire: 2026-10-19T18:53:42Z"
        );
        assert_eq!(
            crate::json::j_to_string(
                update
                    .get("_meta")
                    .and_then(|m| m.get("muse"))
                    .and_then(|m| m.get("scheduledFire"))
                    .unwrap()
            ),
            r#"{"jobId":"job-9","deliveryNumber":4,"scheduledFireAtMs":1792432422000,"nextFireAtMs":1792436022000}"#
        );
        let last = completed_card(
            r#"{"itemId":"sf-2","kind":"scheduledFire","status":"completed","jobId":"job-9","deliveryNumber":5,"scheduledFireAtMs":1792436022000}"#,
        );
        assert!(
            card_text(&last).ends_with("Final delivery"),
            "{}",
            card_text(&last)
        );
    }

    #[test]
    fn a_drained_subagent_result_says_the_parent_consumed_it() {
        let card = |result: &str| {
            let mut fold = SessionFold::new();
            let mut out = Vec::new();
            fold.on_item_completed(
                "sid",
                2,
                &parse_json(&format!(
                    r#"{{"item":{{"itemId":"sa-1","kind":"subagent","status":"completed","subagentId":"sub-1","agentPath":"helper","objective":"check","result":{result}}}}}"#
                ))
                .unwrap(),
                &mut out,
            );
            out[0].clone()
        };
        let drained =
            card(r#"{"summary":"all good","artifactRefs":[],"evidenceRefs":[],"drained":true}"#);
        assert!(
            drained.contains(r#""text":"all good\nDelivered to the parent agent.""#),
            "{drained}"
        );
        assert!(drained.contains(r#""drained":true"#), "{drained}");
        let pending = card(r#"{"summary":"all good","artifactRefs":[],"evidenceRefs":[]}"#);
        assert!(pending.contains(r#""text":"all good""#), "{pending}");
        assert!(
            !pending.contains("drained") && !pending.contains("Delivered"),
            "{pending}"
        );
    }

    #[test]
    fn monitor_inbox_counts_ride_the_monitor_card_latest_wins() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_snapshot(
            "sid",
            2,
            &parse_json(
                r#"{"itemId":"feed-1","kind":"monitorFeed","monitorId":"mon-1","monitorDescription":"Build watcher","status":"inProgress","turnId":null,"text":"watching\n"}"#,
            )
            .unwrap(),
            &mut out,
        );
        out.clear();
        let event = |method: &str, body: &str, fold: &mut SessionFold, out: &mut Vec<String>| {
            fold.on_monitor_inbox("sid", method, &parse_json(body).unwrap(), out);
        };
        for id in ["in-1", "in-2", "in-3"] {
            event(
                "monitor/inboxItemAdmitted",
                &format!(
                    r#"{{"itemId":"{id}","taskId":"mon-1","sessionId":"s","body":"b","summary":"s","eventIndex":1,"eventKind":"line","queuedAt":"2026-10-20T10:00:00Z","showLines":true}}"#
                ),
                &mut fold,
                &mut out,
            );
        }
        event(
            "monitor/inboxItemDrained",
            r#"{"itemId":"in-1","drainedAt":"2026-10-20T10:01:00Z","deliveryVisible":true}"#,
            &mut fold,
            &mut out,
        );
        event(
            "monitor/inboxItemVoided",
            r#"{"itemId":"in-2","voidedAt":"2026-10-20T10:01:00Z"}"#,
            &mut fold,
            &mut out,
        );
        // A later void beats an earlier drain of the same item.
        event(
            "monitor/inboxItemDrained",
            r#"{"itemId":"in-3","drainedAt":"2026-10-20T10:02:00Z","deliveryVisible":true}"#,
            &mut fold,
            &mut out,
        );
        event(
            "monitor/inboxItemVoided",
            r#"{"itemId":"in-3","voidedAt":"2026-10-20T10:03:00Z"}"#,
            &mut fold,
            &mut out,
        );
        let last = out.last().unwrap();
        assert!(
            last.contains(r#""monitorInbox":{"pending":0,"drained":1,"voided":2}"#),
            "{last}"
        );
        assert!(
            last.contains(r#""toolCallId":"monitor-mon-1""#)
                && last.contains(r#""monitorId":"mon-1""#),
            "the card keeps its own metadata: {last}"
        );
        assert!(
            out[0].contains(r#""monitorInbox":{"pending":1,"drained":0,"voided":0}"#),
            "{out:?}"
        );
        // Inbox events for a monitor with no card stay off the transcript.
        out.clear();
        event(
            "monitor/inboxItemAdmitted",
            r#"{"itemId":"in-9","taskId":"mon-unknown"}"#,
            &mut fold,
            &mut out,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn tool_calls_carry_the_standard_programmatic_name() {
        let item = parse_json(
            r#"{"itemId":"it-tool","kind":"toolCall","callId":"call-1","tool":"bash","args":"{\"command\":\"pwd\"}","status":"inProgress","revision":1}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &item, &mut out);
        let frame = parse_json(&out[0]).unwrap();
        let update = frame.get("params").and_then(|p| p.get("update")).unwrap();
        assert_eq!(update.get("name").and_then(J::as_str), Some("bash"));
        assert_eq!(update.get("title").and_then(J::as_str), Some("bash: pwd"));
    }

    #[test]
    fn preferred_compaction_uses_the_standard_update() {
        let item = parse_json(
            r#"{"itemId":"it-c1","kind":"compaction","status":"completed","revision":2,"outcome":"compacted","tokensBefore":12000,"tokensAfter":8000,"trigger":"manual"}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        fold.compaction_supported = true;
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &item, &mut out);
        let frame = parse_json(&out[0]).unwrap();
        let update = frame.get("params").and_then(|p| p.get("update")).unwrap();
        assert_eq!(
            update.get("sessionUpdate").and_then(J::as_str),
            Some("compaction_update")
        );
        assert_eq!(
            update.get("compactionId").and_then(J::as_str),
            Some("it-c1")
        );
        assert_eq!(update.get("status").and_then(J::as_str), Some("completed"));
        assert!(update.get("summary").is_none());
        let provenance = update
            .get("_meta")
            .and_then(|meta| meta.get("muse"))
            .and_then(|muse| muse.get("contextCompaction"))
            .unwrap();
        assert_eq!(
            provenance.get("tokensBefore").and_then(J::as_u64),
            Some(12000)
        );
        assert_eq!(
            provenance.get("trigger").and_then(J::as_str),
            Some("manual")
        );
    }

    #[test]
    fn legacy_compaction_remains_a_tool_card() {
        let item = parse_json(
            r#"{"itemId":"it-c1","kind":"compaction","status":"completed","revision":2,"outcome":"compacted","tokensBefore":12000,"tokensAfter":8000}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &item, &mut out);
        let frame = parse_json(&out[0]).unwrap();
        let update = frame.get("params").and_then(|p| p.get("update")).unwrap();
        assert_eq!(
            update.get("sessionUpdate").and_then(J::as_str),
            Some("tool_call_update")
        );
        assert!(out[0].contains("\"_meta\":{\"contextCompaction\":{\"version\":1}}"));
    }

    #[test]
    fn standard_compaction_replays_as_the_same_entity() {
        let item = parse_json(
            r#"{"itemId":"it-c1","kind":"compaction","status":"completed","revision":2,"outcome":"compacted","tokensBefore":12000,"tokensAfter":8000}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        fold.compaction_supported = true;
        let mut out = Vec::new();
        fold.replay_item("sid", 2, &item, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            out[0].contains("\"sessionUpdate\":\"compaction_update\"")
                && out[0].contains("\"compactionId\":\"it-c1\"")
                && out[0].contains("\"status\":\"completed\""),
            "{}",
            out[0]
        );
    }

    #[test]
    fn unknown_compaction_status_stays_generic() {
        let item = parse_json(
            r#"{"itemId":"it-c1","kind":"compaction","status":"futureTerminal","revision":2}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        fold.compaction_supported = true;
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &item, &mut out);
        assert!(out[0].contains("\"status\":\"_museUnknown\""), "{}", out[0]);
    }

    #[test]
    fn noop_compaction_does_not_claim_context_was_compacted() {
        let item = parse_json(
            r#"{"itemId":"it-c1","kind":"compaction","status":"completed","revision":2,"outcome":"noop","reason":"no_compactable_history"}"#,
        )
        .unwrap();
        let mut fold = SessionFold::new();
        fold.compaction_supported = true;
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &item, &mut out);
        assert!(out[0].contains("\"status\":\"_museNoop\""), "{}", out[0]);
        assert!(
            out[0].contains("\"outcome\":\"noop\"")
                && out[0].contains("\"reason\":\"no_compactable_history\""),
            "{}",
            out[0]
        );
    }

    #[test]
    fn mcp_tools_are_classified_by_their_own_name() {
        assert_eq!(super::tool_kind("mcp__editor__run_in_terminal"), "execute");
        assert_eq!(super::tool_kind("mcp__editor__read_text_file"), "read");
        assert_eq!(super::tool_kind("mcp__editor__write_text_file"), "edit");
        assert_eq!(super::tool_kind("mcp__readwise__fetch_article"), "fetch");
        assert_eq!(super::tool_kind("mcp__probe__secret_word"), "other");
        assert_eq!(super::tool_kind("edit_file"), "edit");
    }

    #[test]
    fn tool_title_uses_args_path_for_file_tools() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-2","kind":"toolCall","status":"inProgress","tool":"read_file","callId":"call-2","args":"{\"path\":\"Cargo.toml\"}"}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &item, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].contains("read_file: Cargo.toml"),
            "title should carry the path: {}",
            out[0]
        );
    }

    #[test]
    fn tool_title_prefers_top_level_display_text_over_args() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-3","kind":"toolCall","status":"inProgress","tool":"bash","callId":"call-3","commandText":"ls -la","args":"{\"command\":\"ignored\"}"}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &item, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].contains("bash: ls -la"),
            "host display text stays authoritative: {}",
            out[0]
        );
    }

    #[test]
    fn tool_display_label_follows_item_status() {
        for (status, want) in [
            ("inProgress", "Setting up Computer Use"),
            ("completed", "Set up Computer Use"),
            ("failed", "Computer Use setup"),
            ("rejected", "Computer Use setup"),
            ("timedOut", "Computer Use setup"),
            ("cancelled", "Computer Use setup"),
            ("futureStatus", "Computer Use setup"),
        ] {
            let item = parse_json(&format!(
                r#"{{"itemId":"it-label","kind":"toolCall","status":"{status}","tool":"computer_use","callId":"call-label","displayLabel":{{"active":"Setting up Computer Use","done":"Set up Computer Use","neutral":"Computer Use setup"}}}}"#
            ))
            .unwrap();
            let (title, tool) = SessionFold::tool_title(&item);
            assert_eq!(title, want, "{status}");
            assert_eq!(tool, "computer_use", "{status}");
        }

        let no_status = parse_json(
            r#"{"itemId":"it-label","kind":"toolCall","tool":"computer_use","callId":"call-label","displayLabel":{"active":"Setting up Computer Use","done":"Set up Computer Use","neutral":"Computer Use setup"}}"#,
        )
        .unwrap();
        assert_eq!(
            SessionFold::tool_title(&no_status).0,
            "Computer Use setup",
            "an absent status uses the neutral form"
        );
    }

    #[test]
    fn tool_display_label_changes_on_snapshot_update() {
        let mut fold = SessionFold::new();
        let active = parse_json(
            r#"{"itemId":"it-label","kind":"toolCall","status":"inProgress","tool":"computer_use","callId":"call-label","displayLabel":{"active":"Setting up Computer Use","done":"Set up Computer Use","neutral":"Computer Use setup"}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &active, &mut out);
        assert!(
            out[0].contains("\"title\":\"Setting up Computer Use\""),
            "{out:?}"
        );

        let done = parse_json(
            r#"{"itemId":"it-label","kind":"toolCall","status":"completed","revision":2,"tool":"computer_use","callId":"call-label","displayLabel":{"active":"Setting up Computer Use","done":"Set up Computer Use","neutral":"Computer Use setup"}}"#,
        )
        .unwrap();
        out.clear();
        fold.on_item_snapshot("sid", 2, &done, &mut out);
        assert!(
            out[0].contains("\"title\":\"Set up Computer Use\"")
                && out[0].contains("\"status\":\"completed\""),
            "{out:?}"
        );
    }

    #[test]
    fn empty_or_missing_tool_display_label_uses_the_fallback() {
        for display in [
            "",
            ",\"displayLabel\":null",
            ",\"displayLabel\":\"bad\"",
            ",\"displayLabel\":{\"done\":\"done\",\"neutral\":\"neutral\"}",
            ",\"displayLabel\":{\"active\":\"   \",\"done\":\"done\",\"neutral\":\"neutral\"}",
        ] {
            let item = parse_json(&format!(
                r#"{{"itemId":"it-fallback","kind":"toolCall","status":"inProgress","tool":"bash","callId":"call-fallback"{display},"args":"{{\"command\":\"cargo test\"}}"}}"#
            ))
            .unwrap();
            let (title, tool) = SessionFold::tool_title(&item);
            assert_eq!(title, "bash: cargo test", "{display}");
            assert_eq!(tool, "bash");
        }
    }

    #[test]
    fn replayed_tool_display_label_is_preserved() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-replay-label","kind":"toolCall","status":"completed","tool":"computer_use","callId":"call-replay-label","displayLabel":{"active":"Setting up Computer Use","done":"Set up Computer Use","neutral":"Computer Use setup"}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.replay_item("sid", 2, &item, &mut out);
        assert!(
            out.iter()
                .any(|line| line.contains("\"title\":\"Set up Computer Use\"")),
            "{out:?}"
        );
    }

    #[test]
    fn async_task_uses_the_tool_display_label() {
        let mut fold = SessionFold::new();
        fold.air_async_tasks = true;
        let item = parse_json(
            r#"{"itemId":"it-bg-label","kind":"toolCall","status":"inProgress","tool":"bash","callId":"call-bg-label","background":true,"displayLabel":{"active":"Running the test suite","done":"Ran the test suite","neutral":"Test suite"}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 2, &item, &mut out);
        assert!(
            out.iter().any(
                |line| line.contains("\"sessionUpdate\":\"async_task_spawned\"")
                    && line.contains("\"name\":\"Running the test suite\"")
            ),
            "{out:?}"
        );
    }

    #[test]
    fn replayed_in_flight_tool_stays_open_for_live_completion() {
        let mut fold = SessionFold::new();
        let snapshot = parse_json(
            r#"{"itemId":"it-run","kind":"toolCall","status":"inProgress","tool":"run","callId":"call-run","args":{"command":"work"}}"#,
        )
        .unwrap();
        let mut replay = Vec::new();
        fold.replay_item("sid", 1, &snapshot, &mut replay);
        assert!(
            replay.iter().any(|line| line.contains("\"in_progress\"")),
            "snapshot announces the running call: {replay:?}"
        );

        let completed = parse_json(
            r#"{"item":{"itemId":"it-run","kind":"toolCall","status":"completed","tool":"run","callId":"call-run","result":"running tool finished"}}"#,
        )
        .unwrap();
        let mut live = Vec::new();
        fold.on_item_completed("sid", 1, &completed, &mut live);
        assert_eq!(live.len(), 1, "live completion is forwarded: {live:?}");
        assert!(
            live[0].contains("running tool finished") && live[0].contains("\"completed\""),
            "live completion settles the replayed call: {}",
            live[0]
        );
    }

    #[test]
    fn replayed_in_flight_message_continues_into_live_completion() {
        let mut fold = SessionFold::new();
        let snapshot = parse_json(
            r#"{"itemId":"it-msg","kind":"agentMessage","status":"inProgress","text":"the answer is fo"}"#,
        )
        .unwrap();
        let mut replay = Vec::new();
        fold.replay_item("sid", 2, &snapshot, &mut replay);
        assert_eq!(
            replay.len(),
            1,
            "snapshot prefix is emitted once: {replay:?}"
        );
        assert!(replay[0].contains("the answer is fo"));

        let delta = parse_json(r#"{"itemId":"it-msg","field":"text","delta":"rty-two"}"#).unwrap();
        let mut live = Vec::new();
        fold.on_item_delta("sid", 2, &delta, &mut live);
        assert_eq!(live.len(), 1, "live delta is forwarded: {live:?}");
        assert!(live[0].contains("rty-two"));

        let completed = parse_json(
            r#"{"item":{"itemId":"it-msg","kind":"agentMessage","status":"completed","text":"the answer is forty-two"}}"#,
        )
        .unwrap();
        let mut terminal = Vec::new();
        fold.on_item_completed("sid", 2, &completed, &mut terminal);
        assert_eq!(
            terminal.len(),
            1,
            "completion declares the phase without resending the message: {terminal:?}"
        );
        assert!(
            terminal[0].contains("\"messagePhase\":\"finalAnswer\"")
                && terminal[0].contains("\"text\":\"\""),
            "the streamed reply is marked final without repeating its text: {}",
            terminal[0]
        );
    }

    #[test]
    fn replayed_terminal_message_is_deduplicated_by_later_refill() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-done","kind":"agentMessage","status":"completed","text":"already shown"}"#,
        )
        .unwrap();
        let mut replay = Vec::new();
        fold.replay_item("sid", 1, &item, &mut replay);
        assert_eq!(replay.len(), 1);

        let refill = parse_json(
            r#"{"item":{"itemId":"it-done","kind":"agentMessage","status":"completed","text":"already shown"}}"#,
        )
        .unwrap();
        let mut duplicate = Vec::new();
        fold.on_item_completed("sid", 1, &refill, &mut duplicate);
        assert!(
            duplicate.is_empty(),
            "refill does not duplicate replay: {duplicate:?}"
        );
    }

    #[test]
    fn cut_off_reply_is_marked_once_on_its_streamed_message() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        let started =
            parse_json(r#"{"itemId":"it-cut","kind":"agentMessage","status":"inProgress"}"#)
                .unwrap();
        fold.on_item_snapshot("sid", 2, &started, &mut out);
        let delta = parse_json(r#"{"itemId":"it-cut","field":"text","delta":"the answ"}"#).unwrap();
        fold.on_item_delta("sid", 2, &delta, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            !out[0].contains("cutOff"),
            "a streaming reply is not marked"
        );
        let msg_id = out[0]
            .split("\"messageId\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .to_string();

        let completed = parse_json(
            r#"{"item":{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"the answ","cutOff":true}}"#,
        )
        .unwrap();
        let mut terminal = Vec::new();
        fold.on_item_completed("sid", 2, &completed, &mut terminal);
        assert_eq!(terminal.len(), 1, "one mark: {terminal:?}");
        assert!(
            terminal[0].contains(
                "\"_meta\":{\"muse\":{\"messagePhase\":\"finalAnswer\",\"cutOff\":true,\"itemId\":\"it-cut\"}}"
            ) && terminal[0].contains(&format!("\"messageId\":\"{msg_id}\""))
                && terminal[0].contains("\"text\":\"\""),
            "the mark names the streamed message without repeating its text: {}",
            terminal[0]
        );

        let updated = parse_json(
            r#"{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"the answ","cutOff":true}"#,
        )
        .unwrap();
        let mut late = Vec::new();
        fold.on_item_snapshot("sid", 2, &updated, &mut late);
        assert!(late.is_empty(), "the mark is never repeated: {late:?}");
    }

    #[test]
    fn an_in_flight_cut_off_mark_claims_no_phase() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        let started =
            parse_json(r#"{"itemId":"it-cut","kind":"agentMessage","status":"inProgress"}"#)
                .unwrap();
        fold.on_item_snapshot("sid", 2, &started, &mut out);
        let delta = parse_json(r#"{"itemId":"it-cut","field":"text","delta":"the answ"}"#).unwrap();
        fold.on_item_delta("sid", 2, &delta, &mut out);
        assert_eq!(out.len(), 1);

        // A revision that first sets cutOff mid-stream marks the reply but
        // invents no phase: the model has not committed one yet.
        let updated = parse_json(
            r#"{"itemId":"it-cut","kind":"agentMessage","status":"inProgress","text":"the answ","cutOff":true}"#,
        )
        .unwrap();
        let mut marks = Vec::new();
        fold.on_item_snapshot("sid", 2, &updated, &mut marks);
        assert_eq!(marks.len(), 1, "one mark: {marks:?}");
        assert!(
            marks[0].contains("\"cutOff\":true") && !marks[0].contains("messagePhase"),
            "the in-flight mark carries no phase: {}",
            marks[0]
        );

        // The eventual completion declares commentary; it must not
        // contradict the earlier mark.
        let completed = parse_json(
            r#"{"item":{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"the answ","cutOff":true,"messagePhase":"commentary"}}"#,
        )
        .unwrap();
        let mut terminal = Vec::new();
        fold.on_item_completed("sid", 2, &completed, &mut terminal);
        assert_eq!(terminal.len(), 1, "one phase declaration: {terminal:?}");
        assert!(
            terminal[0].contains("\"messagePhase\":\"commentary\"")
                && !terminal[0].contains("cutOff"),
            "the completion declares its phase without repeating the mark: {}",
            terminal[0]
        );
    }

    #[test]
    fn cut_off_rides_the_completion_resend_when_nothing_streamed() {
        let mut fold = SessionFold::new();
        let completed = parse_json(
            r#"{"item":{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"partial","cutOff":true}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 1, &completed, &mut out);
        assert_eq!(out.len(), 1, "text and mark arrive together: {out:?}");
        assert!(out[0].contains("\"text\":\"partial\"") && out[0].contains("\"cutOff\":true"));
        assert!(
            !out[0].contains("messageId"),
            "v1 chunks carry no message id"
        );
    }

    #[test]
    fn a_late_update_marks_a_settled_reply_cut_off() {
        let mut fold = SessionFold::new();
        let completed = parse_json(
            r#"{"item":{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"partial"}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 2, &completed, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            !out[0].contains("cutOff"),
            "no mark without the host's flag"
        );

        let updated = parse_json(
            r#"{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"partial","cutOff":true}"#,
        )
        .unwrap();
        let mut late = Vec::new();
        fold.on_item_snapshot("sid", 2, &updated, &mut late);
        assert_eq!(
            late.len(),
            1,
            "the later revision marks the reply: {late:?}"
        );
        let msg_id = out[0].split("\"messageId\":").nth(1).unwrap();
        let msg_id = &msg_id[..msg_id.find(',').unwrap()];
        assert!(
            late[0].contains("\"cutOff\":true") && late[0].contains(msg_id),
            "the mark names the settled message: {}",
            late[0]
        );
    }

    #[test]
    fn a_late_v1_mark_waits_out_a_newer_streaming_reply() {
        let mut fold = SessionFold::new();
        let settled = parse_json(
            r#"{"item":{"itemId":"it-old","kind":"agentMessage","status":"completed","text":"partial"}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 1, &settled, &mut out);
        let started =
            parse_json(r#"{"itemId":"it-new","kind":"agentMessage","status":"inProgress"}"#)
                .unwrap();
        fold.on_item_snapshot("sid", 1, &started, &mut out);
        let delta = parse_json(r#"{"itemId":"it-new","field":"text","delta":"fresh"}"#).unwrap();
        fold.on_item_delta("sid", 1, &delta, &mut out);

        let late = parse_json(
            r#"{"itemId":"it-old","kind":"agentMessage","status":"completed","text":"partial","cutOff":true}"#,
        )
        .unwrap();
        let mut marks = Vec::new();
        fold.on_item_snapshot("sid", 1, &late, &mut marks);
        assert!(
            marks.is_empty(),
            "a v1 chunk would attach to the streaming reply: {marks:?}"
        );
    }

    #[test]
    fn a_late_mark_names_only_the_latest_settled_reply() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        for id in ["it-a", "it-b"] {
            let settled = parse_json(&format!(
                r#"{{"item":{{"itemId":"{id}","kind":"agentMessage","status":"completed","text":"text"}}}}"#
            ))
            .unwrap();
            fold.on_item_completed("sid", 2, &settled, &mut out);
        }
        let stale = parse_json(
            r#"{"itemId":"it-a","kind":"agentMessage","status":"completed","text":"text","cutOff":true}"#,
        )
        .unwrap();
        let mut marks = Vec::new();
        fold.on_item_snapshot("sid", 2, &stale, &mut marks);
        assert!(
            marks.is_empty(),
            "an older reply is not the cut-off one: {marks:?}"
        );
    }

    #[test]
    fn cut_off_is_only_an_explicit_true() {
        for flag in [r#""cutOff":false,"#, r#""cutOff":"true","#, ""] {
            let mut fold = SessionFold::new();
            let completed = parse_json(&format!(
                r#"{{"item":{{"itemId":"it-msg","kind":"agentMessage","status":"completed",{flag}"text":"done"}}}}"#
            ))
            .unwrap();
            let mut out = Vec::new();
            fold.on_item_completed("sid", 2, &completed, &mut out);
            assert_eq!(out.len(), 1);
            assert!(
                !out[0].contains("cutOff"),
                "{flag:?} is not a mark: {}",
                out[0]
            );
        }
    }

    #[test]
    fn completed_agent_message_carries_its_declared_phase() {
        for (phase_json, want) in [
            (r#""messagePhase":"commentary","#, "commentary"),
            (r#""messagePhase":"finalAnswer","#, "finalAnswer"),
            ("", "finalAnswer"),
        ] {
            let mut fold = SessionFold::new();
            let completed = parse_json(&format!(
                r#"{{"item":{{"itemId":"it-msg","kind":"agentMessage","status":"completed",{phase_json}"text":"the answer"}}}}"#
            ))
            .unwrap();
            let mut out = Vec::new();
            fold.on_item_completed("sid", 2, &completed, &mut out);
            assert_eq!(out.len(), 1, "one completion update: {out:?}");
            assert!(
                out[0].contains("\"text\":\"the answer\"")
                    && out[0].contains(&format!("\"messagePhase\":\"{want}\"")),
                "{phase_json:?} rides the completion as {want}: {}",
                out[0]
            );
        }
    }

    #[test]
    fn unknown_message_phase_reads_as_final_answer() {
        let mut fold = SessionFold::new();
        let completed = parse_json(
            r#"{"item":{"itemId":"it-msg","kind":"agentMessage","status":"completed","messagePhase":"someday","text":"the answer"}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 2, &completed, &mut out);
        assert_eq!(out.len(), 1, "an unknown phase still renders: {out:?}");
        assert!(
            out[0].contains("\"text\":\"the answer\"")
                && out[0].contains("\"messagePhase\":\"finalAnswer\""),
            "an unknown phase falls back to the final answer: {}",
            out[0]
        );
    }

    #[test]
    fn streamed_completion_marks_the_phase_without_resending_text() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        let started =
            parse_json(r#"{"itemId":"it-msg","kind":"agentMessage","status":"inProgress"}"#)
                .unwrap();
        fold.on_item_snapshot("sid", 2, &started, &mut out);
        let delta =
            parse_json(r#"{"itemId":"it-msg","field":"text","delta":"the answer"}"#).unwrap();
        fold.on_item_delta("sid", 2, &delta, &mut out);
        assert_eq!(out.len(), 1);
        let msg_id = out[0]
            .split("\"messageId\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .to_string();

        let completed = parse_json(
            r#"{"item":{"itemId":"it-msg","kind":"agentMessage","status":"completed","messagePhase":"commentary","text":"the answer"}}"#,
        )
        .unwrap();
        let mut terminal = Vec::new();
        fold.on_item_completed("sid", 2, &completed, &mut terminal);
        assert_eq!(terminal.len(), 1, "one phase mark: {terminal:?}");
        assert!(
            terminal[0].contains("\"messagePhase\":\"commentary\"")
                && terminal[0].contains(&format!("\"messageId\":\"{msg_id}\""))
                && terminal[0].contains("\"text\":\"\""),
            "the mark names the streamed message without repeating its text: {}",
            terminal[0]
        );
    }

    #[test]
    fn replayed_settled_message_keeps_its_phase_but_a_prefix_claims_none() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-msg","kind":"agentMessage","status":"completed","text":"the answer","messagePhase":"commentary"}"#,
        )
        .unwrap();
        let mut replay = Vec::new();
        fold.replay_item("sid", 2, &item, &mut replay);
        assert_eq!(replay.len(), 1);
        assert!(
            replay[0].contains("the answer")
                && replay[0].contains("\"messagePhase\":\"commentary\""),
            "a settled replay carries its phase: {}",
            replay[0]
        );

        let mut fold = SessionFold::new();
        let prefix = parse_json(
            r#"{"itemId":"it-msg","kind":"agentMessage","status":"inProgress","text":"the ans"}"#,
        )
        .unwrap();
        let mut replay = Vec::new();
        fold.replay_item("sid", 2, &prefix, &mut replay);
        assert_eq!(replay.len(), 1);
        assert!(
            !replay[0].contains("messagePhase"),
            "an in-flight prefix records no phase yet: {}",
            replay[0]
        );
    }

    #[test]
    fn replayed_cut_off_reply_keeps_its_mark() {
        let mut fold = SessionFold::new();
        let item = parse_json(
            r#"{"itemId":"it-cut","kind":"agentMessage","status":"completed","text":"partial","cutOff":true}"#,
        )
        .unwrap();
        for ver in [1, 2] {
            let mut fold = SessionFold::new();
            let mut replay = Vec::new();
            fold.replay_item("sid", ver, &item, &mut replay);
            assert_eq!(replay.len(), 1, "the mark rides the message: {replay:?}");
            let parsed = parse_json(&replay[0]).unwrap();
            let update = parsed.get("params").and_then(|p| p.get("update")).unwrap();
            assert!(
                update.get("messageId").is_some()
                    && update
                        .get("content")
                        .is_some_and(|c| j_to_string(c).contains("partial"))
                    && update
                        .get("_meta")
                        .and_then(|m| m.get("muse"))
                        .and_then(|m| m.get("cutOff"))
                        .is_some_and(|v| matches!(v, J::Bool(true))),
                "v{ver} replay carries text, id, and mark together: {}",
                replay[0]
            );
        }
        let mut replay = Vec::new();
        fold.replay_item("sid", 2, &item, &mut replay);

        let refill = parse_json(&format!(r#"{{"item":{}}}"#, j_to_string(&item))).unwrap();
        let mut duplicate = Vec::new();
        fold.on_item_completed("sid", 2, &refill, &mut duplicate);
        assert!(
            duplicate.is_empty(),
            "refill repeats neither: {duplicate:?}"
        );
    }

    #[test]
    fn empty_visible_output_falls_back_to_failure_reason() {
        let completed = parse_json(
            r#"{"item":{"itemId":"it-fail","kind":"toolCall","status":"failed","tool":"read_file","callId":"call-1","args":{},"visibleOutput":"","failureReason":"file does not exist"}}"#,
        )
        .unwrap();

        for version in [1, 2] {
            let mut fold = SessionFold::new();
            let mut out = Vec::new();
            fold.on_item_completed("sid", version, &completed, &mut out);

            assert_eq!(
                out.len(),
                1,
                "v{version} emitted unexpected frames: {out:?}"
            );
            assert!(
                out[0].contains("\"status\":\"failed\"")
                    && out[0].contains("\"content\"")
                    && out[0].contains("file does not exist"),
                "v{version} preserved the failure explanation: {}",
                out[0]
            );
        }
    }

    #[test]
    fn terminal_failure_statuses_map_to_legal_acp_statuses() {
        // ACP has no statuses for these MSP terminal failures; emitting one
        // unchanged strands the card.
        assert_eq!(super::msp_status("cancelled"), "failed");
        assert_eq!(super::msp_status("rejected"), "failed");
        assert_eq!(super::msp_status("timedOut"), "failed");
        assert_eq!(super::msp_status("completed"), "completed");
        assert_eq!(super::msp_status("in_progress"), "in_progress");
    }

    #[test]
    fn rejected_and_timed_out_tool_completions_settle_cards() {
        for ver in [1, 2] {
            for terminal in ["rejected", "timedOut"] {
                let mut fold = SessionFold::new();
                let started = parse_json(
                    r#"{"itemId":"it-t1","kind":"toolCall","callId":"call-1","revision":1,"status":"inProgress","tool":"read_file","args":"{}"}"#,
                )
                .unwrap();
                let mut out = Vec::new();
                fold.on_item_snapshot("sid", ver, &started, &mut out);
                assert!(
                    out[0].contains("\"status\":\"in_progress\""),
                    "tool starts in progress for v{ver} {terminal}: {out:?}"
                );

                let completed = parse_json(&format!(
                    r#"{{"item":{{"itemId":"it-t1","kind":"toolCall","callId":"call-1","revision":2,"status":"{terminal}","tool":"read_file","args":"{{}}","visibleOutput":"Tool did not execute","failureReason":"Tool did not execute"}}}}"#
                ))
                .unwrap();
                out.clear();
                fold.on_item_completed("sid", ver, &completed, &mut out);
                assert_eq!(out.len(), 1, "terminal update for v{ver} {terminal}");
                assert!(
                    out[0].contains("\"toolCallId\":\"call-1\"")
                        && out[0].contains("\"status\":\"failed\""),
                    "tool settles as failed for v{ver} {terminal}: {out:?}"
                );
            }
        }
    }

    #[test]
    fn fallback_card_title_humanizes_a_camel_case_kind() {
        assert_eq!(
            super::fallback_card_title("reminderChild"),
            "Reminder child"
        );
        assert_eq!(super::fallback_card_title("workflow"), "Workflow");
        assert_eq!(super::fallback_card_title(""), "Item");
    }

    #[test]
    fn web_search_items_render_through_the_generic_card_path() {
        // `webSearch` is a Muse 1.4.6 item kind with no dedicated ACP
        // presentation. The unknown-kind path renders its `fallbackText` as a
        // generic card and tags the item kind, so the new kind is tolerated
        // and never dropped.
        for ver in [1, 2] {
            let mut fold = SessionFold::new();
            let started = parse_json(
                r#"{"itemId":"ws1","kind":"webSearch","status":"in_progress","fallbackText":"Searching the web for: muse-acp"}"#,
            )
            .unwrap();
            let mut out = Vec::new();
            fold.on_item_snapshot("sid", ver, &started, &mut out);
            assert_eq!(
                out.len(),
                1,
                "webSearch start emits one card for v{ver}: {out:?}"
            );
            assert!(
                out[0].contains("\"title\":\"Searching the web for: muse-acp\"")
                    && out[0].contains("\"itemKind\":\"webSearch\""),
                "generic card titles from fallbackText and names the kind for v{ver}: {out:?}"
            );
        }
    }

    #[test]
    fn reminder_child_start_emits_nothing() {
        // Background reminder checks hold the turn open but carry no
        // editor-facing content: the host shows nothing per agent, so the
        // adapter emits no card at start and none at terminal.
        let mut fold = SessionFold::new();
        let started = parse_json(
            r#"{"itemId":"r1","kind":"reminderChild","status":"in_progress","fallbackText":"Reminder child session"}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &started, &mut out);
        assert!(out.is_empty(), "reminder start stays silent: {out:?}");

        let completed =
            parse_json(r#"{"item":{"itemId":"r1","kind":"reminderChild","status":"completed"}}"#)
                .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 1, &completed, &mut out);
        assert!(out.is_empty(), "reminder terminal stays silent: {out:?}");
    }

    #[test]
    fn reminder_child_with_agent_and_generation_emits_nothing() {
        let mut fold = SessionFold::new();
        let started = parse_json(
            r#"{"itemId":"r2","kind":"reminderChild","status":"in_progress","fallbackText":"Reminder child session","reminderAgentId":"skill-reminder","generationId":3,"taskId":"t-123","childSessionId":"sess-abc","childSessionLogPath":"child.log"}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &started, &mut out);
        assert!(out.is_empty(), "named reminder start stays silent: {out:?}");

        // A superseded generation is host churn, not an editor-facing
        // outcome, so it stays silent too.
        let cancelled = parse_json(
            r#"{"item":{"itemId":"r2","kind":"reminderChild","status":"cancelled","reminderAgentId":"skill-reminder","generationId":3}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 1, &cancelled, &mut out);
        assert!(out.is_empty(), "superseded reminder stays silent: {out:?}");
    }

    #[test]
    fn parallel_reminder_agents_emit_nothing() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        for (item, agent) in [("r2", "skill-reminder"), ("r3", "todo-reminder")] {
            let started = parse_json(&format!(
                r#"{{"itemId":"{item}","kind":"reminderChild","status":"in_progress","reminderAgentId":"{agent}","generationId":1}}"#
            ))
            .unwrap();
            fold.on_item_snapshot("sid", 1, &started, &mut out);
        }
        assert!(out.is_empty(), "parallel reminders stay silent: {out:?}");
    }

    #[test]
    fn opted_in_reminder_cards_name_the_agent() {
        // MUSE_SHOW_REMINDERS restores the per-agent card for debugging.
        let mut fold = SessionFold::new();
        fold.show_reminders = true;
        let started = parse_json(
            r#"{"itemId":"r2","kind":"reminderChild","status":"in_progress","fallbackText":"Reminder child session","reminderAgentId":"skill-reminder","generationId":3}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        fold.on_item_snapshot("sid", 1, &started, &mut out);
        assert!(
            out.iter()
                .any(|line| line.contains("Reminder from skill-reminder")),
            "opted-in card names the agent: {out:?}"
        );
    }

    #[test]
    fn foreground_completed_opens_one_checks_card_until_the_terminal() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_foreground_completed("sid", 1, "t1", &["verify-reminder"], &mut out);
        assert_eq!(out.len(), 2, "hint plus card: {out:?}");
        assert!(out[0].contains(
            r#""foregroundCompleted":{"turnId":"t1","blockingAgents":["verify-reminder"]}"#
        ));
        assert!(out[1].contains(r#""sessionUpdate":"tool_call""#));
        assert!(out[1].contains("Background checks: verify-reminder"));
        assert!(out[1].contains(r#""status":"in_progress""#));

        // Another turn's terminal leaves this turn's card alone.
        out.clear();
        fold.settle_background_checks("sid", Some("t0"), "completed", true, &mut out);
        assert!(out.is_empty(), "other turn: {out:?}");

        fold.settle_background_checks("sid", Some("t1"), "completed", true, &mut out);
        assert_eq!(out.len(), 2, "settle plus clear: {out:?}");
        assert!(out[0].contains(r#""status":"completed""#));
        assert!(out[1].contains(r#""foregroundCompleted":null"#));

        out.clear();
        fold.settle_background_checks("sid", Some("t1"), "completed", true, &mut out);
        assert!(out.is_empty(), "settles once: {out:?}");
    }

    #[test]
    fn model_retry_hint_carries_the_attempt_facts_until_the_terminal() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        let params = parse_json(
            r#"{"turnId":"t1","sessionId":"s1","attempt":1,"nextAttempt":2,"maxAttempts":4,"retryDelayMs":1500,"errorKind":"someFutureKind","httpStatus":503,"system":"meta","viewCursor":"c1","sourceRange":{"first":{"id":"e3","sequence":3},"last":{"id":"e3","sequence":3},"stream":{"id":"s","kind":"session"}}}"#,
        )
        .unwrap();
        fold.on_model_retry_scheduled("sid", "t1", &params, &mut out);
        assert_eq!(out.len(), 1, "one hint: {out:?}");
        assert!(out[0].contains(
            r#""modelRetry":{"turnId":"t1","attempt":1,"nextAttempt":2,"maxAttempts":4,"retryDelayMs":1500,"errorKind":"someFutureKind","system":"meta","httpStatus":503}"#
        ), "{out:?}");

        // The optional members are omitted, never fabricated.
        out.clear();
        let bare = parse_json(
            r#"{"turnId":"t1","attempt":2,"nextAttempt":3,"maxAttempts":4,"retryDelayMs":3000}"#,
        )
        .unwrap();
        fold.on_model_retry_scheduled("sid", "t1", &bare, &mut out);
        assert!(out[0].contains(
            r#""modelRetry":{"turnId":"t1","attempt":2,"nextAttempt":3,"maxAttempts":4,"retryDelayMs":3000}"#
        ), "{out:?}");

        // Another turn's terminal leaves the hint alone; this turn's clears
        // it exactly once.
        out.clear();
        fold.clear_model_retry("sid", Some("t0"), &mut out);
        assert!(out.is_empty(), "other turn: {out:?}");
        fold.clear_model_retry("sid", Some("t1"), &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains(r#""modelRetry":null"#));
        out.clear();
        fold.clear_model_retry("sid", None, &mut out);
        assert!(out.is_empty(), "clears once: {out:?}");
    }

    #[test]
    fn a_second_firing_settles_the_previous_checks_card() {
        let mut fold = SessionFold::new();
        let mut out = Vec::new();
        fold.on_foreground_completed("sid", 2, "t1", &["goal-reminder"], &mut out);
        let first = parse_json(&out[1]).unwrap();
        let first_id = first
            .get("params")
            .and_then(|p| p.get("update"))
            .and_then(|u| u.get("toolCallId"))
            .and_then(J::as_str)
            .unwrap()
            .to_string();
        out.clear();
        fold.on_foreground_completed("sid", 2, "t1", &["verify-reminder"], &mut out);
        assert_eq!(out.len(), 3, "settle, hint, new card: {out:?}");
        assert!(out[0].contains(&first_id) && out[0].contains(r#""status":"completed""#));
        assert!(
            !out[2].contains(&first_id),
            "a new card follows resumed work"
        );
    }

    #[test]
    fn opted_in_reminder_cards_replace_the_aggregate_checks_card() {
        let mut fold = SessionFold::new();
        fold.show_reminders = true;
        let mut out = Vec::new();
        fold.on_foreground_completed("sid", 1, "t1", &["todo-reminder"], &mut out);
        assert_eq!(out.len(), 1, "hint only: {out:?}");
        out.clear();
        fold.settle_background_checks("sid", Some("t1"), "completed", true, &mut out);
        assert_eq!(out.len(), 1, "clear only: {out:?}");
        assert!(out[0].contains(r#""foregroundCompleted":null"#));
    }

    #[test]
    fn an_unannounced_titleless_reminder_emits_nothing() {
        // Never announced and no title: there is nothing honest to settle.
        let mut fold = SessionFold::new();
        let completed =
            parse_json(r#"{"item":{"itemId":"x9","kind":"reminderChild","status":"completed"}}"#)
                .unwrap();
        let mut out = Vec::new();
        fold.on_item_completed("sid", 1, &completed, &mut out);
        assert!(out.is_empty(), "nothing to settle: {out:?}");
    }
    #[test]
    fn truncation_keeps_unicode_head_and_tail_with_exact_metadata() {
        let limit = max_content();
        let short = "🦀".repeat(limit);
        assert_eq!(trunc(&short), (short, None));
        let original = format!("開始{}最後の失敗", "🦀".repeat(limit));
        let (text, meta) = trunc(&original);
        assert!(text.starts_with("開始") && text.ends_with("最後の失敗"));
        assert_eq!(text.chars().count(), limit);
        let meta = parse_json(&format!("{{{}}}", meta.unwrap())).unwrap();
        assert_eq!(
            meta.get("originalChars").unwrap().as_u64(),
            Some(original.chars().count() as u64)
        );
        assert_eq!(
            meta.get("retainedChars").unwrap().as_u64(),
            Some((limit - 12) as u64)
        );
    }

    #[test]
    fn truncated_nested_json_remains_valid_editor_content() {
        let nested = format!(
            r#"{{"outer":{{"quoted":"\\\"{}","error":"final failure"}}}}"#,
            "界".repeat(max_content())
        );
        let line = SessionFold::card_line(
            "sid",
            2,
            true,
            "id",
            "title",
            "read",
            "failed",
            Some(&nested),
            None,
            false,
        );
        let parsed = parse_json(&line).unwrap();
        assert!(line.contains("final failure"));
        assert_eq!(
            parsed
                .get("params")
                .unwrap()
                .get("update")
                .unwrap()
                .get("status")
                .unwrap()
                .as_str(),
            Some("failed")
        );
    }
}
