//! Fold: per-session MSP view state + mapping to ACP `session/update` payloads.
//!
//! Item kinds come from the host schema (`userMessage`, `agentMessage`,
//! `reasoning`, `toolCall`, …). Unknown kinds render generically: message-like
//! items with text stream as message updates, everything else is ignored.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use crate::json::{esc, j_to_string, mint_id, J};

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
    } else if t.contains("write") || t.contains("edit") || t.contains("patch") || t.contains("apply") {
        "edit"
    } else if t.contains("bash") || t.contains("shell") || t.contains("exec") || t.contains("run") {
        "execute"
    } else if t.contains("search") || t.contains("grep") || t.contains("glob") || t.contains("find") {
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
    Message { msg_id: String, streamed: usize },
    Tool { tc_id: String, announced: bool },
    Ignored,
}

pub struct SessionFold {
    items: HashMap<String, ItemRole>,
    idc: AtomicU64,
}

impl SessionFold {
    pub fn new() -> Self {
        Self { items: HashMap::new(), idc: AtomicU64::new(1) }
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
            format!("{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{}}}", content)
        };
        Self::update_line(acp_sid, &update)
    }

    fn tool_line(
        acp_sid: &str,
        ver: u8,
        create: bool,
        tc_id: &str,
        title: &str,
        kind: &str,
        status: &str,
        content_text: Option<&str>,
        raw_input: Option<&str>,
    ) -> String {
        let session_update = if ver == 2 || !create { "tool_call_update" } else { "tool_call" };
        let mut f = vec![
            format!("\"sessionUpdate\":\"{session_update}\""),
            format!("\"toolCallId\":{}", esc(tc_id)),
            format!("\"title\":{}", esc(title)),
            format!("\"kind\":\"{kind}\""),
            format!("\"status\":{}", esc(status)),
        ];
        if let Some(t) = content_text {
            f.push(format!(
                "\"content\":[{{\"type\":\"content\",\"content\":{{\"type\":\"text\",\"text\":{}}}}}]",
                esc(&trunc(t))
            ));
        }
        if let Some(r) = raw_input {
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
        let title = if detail.is_empty() { tool.to_string() } else { format!("{tool}: {detail}") };
        (title, tool.to_string())
    }

    /// item/started and item/updated share snapshotting.
    pub fn on_item_snapshot(&mut self, acp_sid: &str, ver: u8, item: &J, out: &mut Vec<String>) {
        let item_id = match item.get("itemId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return,
        };
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("pending");
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
                        self.items.insert(item_id.clone(), ItemRole::Tool { tc_id: id.clone(), announced: false });
                        (id, false)
                    }
                };
                let (title, tool) = Self::tool_title(item);
                let raw = item.get("args").map(j_to_string).unwrap_or_else(|| "{}".to_string());
                out.push(Self::tool_line(acp_sid, ver, !announced, &tc_id, &title, tool_kind(&tool), msp_status(status), None, Some(&raw)));
                if let Some(ItemRole::Tool { announced, .. }) = self.items.get_mut(&item_id) {
                    *announced = true;
                }
            }
            "agentMessage" | "userMessage" => {
                if !self.items.contains_key(&item_id) {
                    let msg_id = mint_id("msg-", &self.idc);
                    self.items.insert(item_id, ItemRole::Message { msg_id, streamed: 0 });
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
        if params.get("field").and_then(|v| v.as_str()).unwrap_or("text") != "text" {
            return;
        }
        let delta = match params.get("delta").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };
        if let Some(ItemRole::Message { msg_id, streamed }) = self.items.get_mut(item_id) {
            *streamed += delta.len();
            out.push(Self::chunk(acp_sid, ver, msg_id, delta));
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
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("completed");
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
                        self.items.insert(item_id.clone(), ItemRole::Tool { tc_id: id.clone(), announced: false });
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
                    acp_sid, ver, !announced, &tc_id, &title, tool_kind(&tool),
                    msp_status(status), content, raw.as_deref(),
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
            _ => {
                self.items.remove(&item_id);
            }
        }
    }
}

/// MSP turn terminal -> ACP stop reason (v1 + v2 share the vocabulary).
pub fn stop_reason(terminal: &str) -> Option<&'static str> {
    match terminal {
        "completed" => Some("end_turn"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}
