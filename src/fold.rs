//! Fold: per-session MSP view state + mapping to ACP `session/update` payloads.
//!
//! Item kinds come from the host schema (`userMessage`, `agentMessage`,
//! `reasoning`, `toolCall`, …). Unknown kinds render generically: message-like
//! items with text stream as message updates, everything else is ignored.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use crate::json::{J, esc, j_to_string, mint_id};

const MAX_CONTENT: usize = 8000;

fn trunc(s: &str) -> String {
    if s.len() <= MAX_CONTENT {
        return s.to_string();
    }
    let mut cut = MAX_CONTENT;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…[truncated]", &s[..cut])
}

fn tool_kind(tool: &str) -> &'static str {
    let t = tool.to_lowercase();
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
        "cancelled" => "cancelled",
        "in_progress" | "running" | "started" => "in_progress",
        _ => "pending",
    }
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
    Ignored,
}

pub struct ToolUpdate<'a> {
    pub create: bool,
    pub tc_id: &'a str,
    pub title: &'a str,
    pub kind: &'a str,
    pub status: &'a str,
    pub content_text: Option<&'a str>,
    pub raw_input: Option<&'a str>,
}

pub struct SessionFold {
    items: HashMap<String, ItemRole>,
    /// Completed item ids (gap-refill replays must not re-announce).
    done: std::collections::HashSet<String>,
    idc: AtomicU64,
}

impl SessionFold {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            done: std::collections::HashSet::new(),
            idc: AtomicU64::new(1),
        }
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

    fn tool_line(acp_sid: &str, ver: u8, u: &ToolUpdate<'_>) -> String {
        let session_update = if ver == 2 || !u.create {
            "tool_call_update"
        } else {
            "tool_call"
        };
        let mut f = vec![
            format!("\"sessionUpdate\":\"{session_update}\""),
            format!("\"toolCallId\":{}", esc(u.tc_id)),
            format!("\"title\":{}", esc(u.title)),
            format!("\"kind\":\"{}\"", u.kind),
            format!("\"status\":{}", esc(u.status)),
        ];
        if let Some(t) = u.content_text {
            f.push(format!(
                "\"content\":[{{\"type\":\"content\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}]",
                esc(&trunc(t))
            ));
        }
        if let Some(r) = u.raw_input {
            f.push(format!("\"rawInput\":{r}"));
        }
        Self::update_line(acp_sid, &format!("{{{}}}", f.join(",")))
    }

    fn tool_title(item: &J) -> (String, String) {
        let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
        let detail = item
            .get("commandText")
            .or_else(|| item.get("displayText"))
            .or_else(|| item.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let title = if detail.is_empty() {
            tool.to_string()
        } else {
            format!("{tool}: {detail}")
        };
        (title, tool.to_string())
    }

    /// item/started and item/updated share snapshotting.
    pub fn on_item_snapshot(&mut self, acp_sid: &str, ver: u8, item: &J, out: &mut Vec<String>) {
        let item_id = match item.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        if self.known(&item_id) {
            return; // gap-refill replay of a settled item
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
                out.push(Self::tool_line(
                    acp_sid,
                    ver,
                    &ToolUpdate {
                        create: !announced,
                        tc_id: &tc_id,
                        title: &title,
                        kind: tool_kind(&tool),
                        status: msp_status(status),
                        content_text: None,
                        raw_input: Some(&raw),
                    },
                ));
                if let Some(ItemRole::Tool { announced, .. }) = self.items.get_mut(&item_id) {
                    *announced = true;
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
            _ => {
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
        if field == "text" {
            if let Some(ItemRole::Message { msg_id, streamed }) = self.items.get_mut(item_id) {
                *streamed += delta.len();
                out.push(Self::chunk(acp_sid, ver, msg_id, delta));
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
        let item = match params.get("item") {
            Some(i) => i,
            None => return,
        };
        let item_id = match item.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        if self.known(&item_id) {
            return; // gap-refill replay of a settled item
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
                    .or_else(|| item.get("result"))
                    .or_else(|| item.get("failureReason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = if text.is_empty() { None } else { Some(text) };
                let raw = item.get("args").map(j_to_string);
                out.push(Self::tool_line(
                    acp_sid,
                    ver,
                    &ToolUpdate {
                        create: !announced,
                        tc_id: &tc_id,
                        title: &title,
                        kind: tool_kind(&tool),
                        status: msp_status(status),
                        content_text: content,
                        raw_input: raw.as_deref(),
                    },
                ));
                self.items.remove(&item_id);
            }
            "agentMessage" => {
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let streamed = match self.items.get(&item_id) {
                    Some(ItemRole::Message { streamed, .. }) => *streamed,
                    _ => 0,
                };
                // Deltas are authoritative-in-motion; the completed frame is the
                // truth. Resend only if nothing streamed (missed deltas).
                if streamed == 0 && !text.is_empty() {
                    let msg_id = mint_id("msg-", &self.idc);
                    out.push(Self::chunk(acp_sid, ver, &msg_id, text));
                }
                self.items.remove(&item_id);
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
            _ => {
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
    use super::SessionFold;
    use crate::json::{J, parse_json};
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
}
