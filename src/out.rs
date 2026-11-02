//! What the adapter sends the editor, as the SDK's typed schema values.
//!
//! Every `session/update` is a v1 `SessionNotification` or a v2
//! `UpdateSessionNotification` holding the version's typed `SessionUpdate`,
//! and the SDK serializes it. The few updates no schema of the negotiated
//! version defines (JetBrains AIR's `subagent_*` and `async_task_*` updates)
//! go out as an `UntypedMessage`, the Rust form of the cast the JetBrains
//! TypeScript adapters make. Only `_meta`, whose members ACP leaves to
//! extensions, is the adapter's own JSON.

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::{UntypedMessage, v1::schema as s1, v2::schema as s2};
use serde_json::{Map, Value};

/// A `_meta` object.
pub type Meta = Map<String, Value>;

/// `_meta` from JSON object text the adapter built for its extension
/// members. `None` for empty or invalid text.
pub fn meta(object: &str) -> Option<Meta> {
    match serde_json::from_str::<Value>(object) {
        Ok(Value::Object(map)) if !map.is_empty() => Some(map),
        Ok(_) => None,
        Err(error) => {
            crate::msp::log(&format!("internal: invalid _meta JSON dropped: {error}"));
            None
        }
    }
}

/// `{"muse":{<fields>}}`, for the `"key":value,...` member list the adapter
/// builds. `None` when there are no fields.
pub fn muse_meta(fields: &str) -> Option<Meta> {
    if fields.is_empty() {
        return None;
    }
    meta(&format!("{{\"muse\":{{{fields}}}}}"))
}

/// A JSON value from text the adapter built (a host fact passed through),
/// or `null` for invalid text.
pub fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// One `session/update` notification.
#[derive(Debug, Clone)]
pub enum Note {
    V1(s1::SessionNotification),
    V2(s2::UpdateSessionNotification),
    /// A `session/update` whose update no schema of the negotiated version
    /// defines.
    Ext(UntypedMessage),
}

impl Note {
    pub fn v1(acp_sid: &str, update: s1::SessionUpdate) -> Note {
        Note::V1(s1::SessionNotification::new(acp_sid.to_string(), update))
    }

    pub fn v2(acp_sid: &str, update: s2::SessionUpdate) -> Note {
        Note::V2(s2::UpdateSessionNotification::new(
            acp_sid.to_string(),
            update,
        ))
    }

    /// A `session/update` carrying an extension update (`update` is the
    /// update object, with its `sessionUpdate` discriminator).
    pub fn ext(acp_sid: &str, update: Value) -> Note {
        let params = serde_json::json!({"sessionId": acp_sid, "update": update});
        Note::Ext(
            UntypedMessage::new("session/update", params)
                .expect("a session/update object serializes"),
        )
    }

    /// The notification's `params`, as the SDK serializes them.
    pub fn params(&self) -> Value {
        match self {
            Note::V1(note) => serde_json::to_value(note).unwrap_or(Value::Null),
            Note::V2(note) => serde_json::to_value(note).unwrap_or(Value::Null),
            Note::Ext(message) => message.params().clone(),
        }
    }

    /// The whole JSON-RPC frame as text, as it goes on the wire.
    pub fn line(&self) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": self.params(),
        })
        .to_string()
    }

    /// The update's `sessionUpdate` discriminator.
    pub fn kind(&self) -> String {
        self.params()["update"]["sessionUpdate"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl std::fmt::Display for Note {
    /// The frame as it goes on the wire.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.line())
    }
}

#[cfg(test)]
impl Note {
    /// Whether the wire frame contains `needle`.
    pub fn contains(&self, needle: &str) -> bool {
        self.line().contains(needle)
    }
}

/// The wire frames of `notes`, one per line.
#[cfg(test)]
pub fn joined(notes: &[Note]) -> String {
    notes.iter().map(Note::line).collect::<Vec<_>>().join("\n")
}

/// Plain text content.
pub fn text_v1(text: &str) -> s1::ContentBlock {
    s1::ContentBlock::from(text.to_string())
}

pub fn text_v2(text: &str) -> s2::ContentBlock {
    s2::ContentBlock::from(text.to_string())
}

/// An `agent_message_chunk`, `agent_thought_chunk` or `user_message_chunk`
/// of text. ACP v2 chunks name their message.
#[derive(Clone, Copy)]
pub enum Chunk {
    Agent,
    Thought,
    User,
}

pub fn chunk(
    acp_sid: &str,
    ver: u8,
    which: Chunk,
    msg_id: &str,
    text: &str,
    meta: Option<Meta>,
) -> Note {
    if ver == 2 {
        let chunk = s2::ContentChunk::new(text_v2(text), msg_id.to_string()).meta(meta);
        Note::v2(
            acp_sid,
            match which {
                Chunk::Agent => s2::SessionUpdate::AgentMessageChunk(chunk),
                Chunk::Thought => s2::SessionUpdate::AgentThoughtChunk(chunk),
                Chunk::User => s2::SessionUpdate::UserMessageChunk(chunk),
            },
        )
    } else {
        let chunk = s1::ContentChunk::new(text_v1(text)).meta(meta);
        Note::v1(
            acp_sid,
            match which {
                Chunk::Agent => s1::SessionUpdate::AgentMessageChunk(chunk),
                Chunk::Thought => s1::SessionUpdate::AgentThoughtChunk(chunk),
                Chunk::User => s1::SessionUpdate::UserMessageChunk(chunk),
            },
        )
    }
}

/// One tool call content item.
#[derive(Debug, Clone, PartialEq)]
pub enum CardContent {
    Text(String),
    /// An ACP terminal the client renders by id.
    Terminal(String),
    /// ACP v1's whole-file `diff` content (the editor file tools).
    FileDiff {
        path: String,
        old_text: Option<String>,
        new_text: String,
    },
    /// ACP v2's patch-based `diff` content.
    Diff(Diff),
}

/// ACP v2 `diff` content: the files a patch touches, and the patch.
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    /// `(added, absolute path)` per file: added files are created, the
    /// rest modified.
    pub changes: Vec<(bool, String)>,
    pub patch: String,
}

/// A file a tool call touches.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub path: String,
    pub line: Option<u32>,
}

/// A tool call card: ACP v1 `tool_call` (when created) or `tool_call_update`,
/// and ACP v2 `tool_call_update`. Fields left `None` are not sent.
#[derive(Debug, Clone, Default)]
pub struct Card {
    pub id: String,
    pub name: Option<String>,
    pub title: Option<String>,
    /// An ACP tool kind (`read`, `edit`, `execute`, ...).
    pub kind: Option<String>,
    /// An ACP tool call status (`pending`, `in_progress`, `completed`,
    /// `failed`).
    pub status: Option<String>,
    pub content: Option<Vec<CardContent>>,
    pub locations: Option<Vec<Location>>,
    pub raw_input: Option<Value>,
    pub meta: Option<Meta>,
}

impl Card {
    pub fn new(id: &str) -> Card {
        Card {
            id: id.to_string(),
            ..Card::default()
        }
    }

    /// The card as an update. ACP v1 announces a new card with `tool_call`;
    /// v2 has only `tool_call_update`.
    pub fn note(self, acp_sid: &str, ver: u8, create: bool) -> Note {
        if ver == 2 {
            let mut update = s2::ToolCallUpdate::new(self.id.clone());
            if let Some(name) = self.name {
                update = update.name(name);
            }
            if let Some(title) = self.title {
                update = update.title(title);
            }
            if let Some(kind) = self.kind.as_deref() {
                update = update.kind(kind_v2(kind));
            }
            if let Some(status) = self.status.as_deref() {
                update = update.status(status_v2(status));
            }
            if let Some(content) = self.content {
                update = update.content(
                    content
                        .into_iter()
                        .filter_map(content_v2)
                        .collect::<Vec<_>>(),
                );
            }
            if let Some(locations) = self.locations {
                update = update.locations(
                    locations
                        .into_iter()
                        .map(|l| {
                            s2::ToolCallLocation::new(std::path::PathBuf::from(l.path)).line(l.line)
                        })
                        .collect::<Vec<_>>(),
                );
            }
            if let Some(raw_input) = self.raw_input {
                update = update.raw_input(raw_input);
            }
            if let Some(meta) = self.meta {
                update = update.meta(meta);
            }
            return Note::v2(acp_sid, s2::SessionUpdate::ToolCallUpdate(update));
        }
        let content = self.content.map(|content| {
            content
                .into_iter()
                .filter_map(content_v1)
                .collect::<Vec<_>>()
        });
        let locations = self.locations.map(|locations| {
            locations
                .into_iter()
                .map(|l| s1::ToolCallLocation::new(l.path).line(l.line))
                .collect::<Vec<_>>()
        });
        if create {
            let mut call = s1::ToolCall::new(self.id, self.title.unwrap_or_default())
                .name(self.name)
                .raw_input(self.raw_input)
                .meta(self.meta);
            if let Some(kind) = self.kind.as_deref() {
                call = call.kind(kind_v1(kind));
            }
            if let Some(status) = self.status.as_deref() {
                call = call.status(status_v1(status));
            }
            if let Some(content) = content {
                call = call.content(content);
            }
            if let Some(locations) = locations {
                call = call.locations(locations);
            }
            return Note::v1(acp_sid, s1::SessionUpdate::ToolCall(call));
        }
        let fields = s1::ToolCallUpdateFields::new()
            .name(self.name)
            .title(self.title)
            .kind(self.kind.as_deref().map(kind_v1))
            .status(self.status.as_deref().map(status_v1))
            .content(content)
            .locations(locations)
            .raw_input(self.raw_input);
        Note::v1(
            acp_sid,
            s1::SessionUpdate::ToolCallUpdate(
                s1::ToolCallUpdate::new(self.id, fields).meta(self.meta),
            ),
        )
    }
}

fn kind_v1(kind: &str) -> s1::ToolKind {
    match kind {
        "read" => s1::ToolKind::Read,
        "edit" => s1::ToolKind::Edit,
        "delete" => s1::ToolKind::Delete,
        "move" => s1::ToolKind::Move,
        "search" => s1::ToolKind::Search,
        "execute" => s1::ToolKind::Execute,
        "think" => s1::ToolKind::Think,
        "fetch" => s1::ToolKind::Fetch,
        "switch_mode" => s1::ToolKind::SwitchMode,
        _ => s1::ToolKind::Other,
    }
}

fn kind_v2(kind: &str) -> s2::ToolKind {
    match kind {
        "read" => s2::ToolKind::Read,
        "edit" => s2::ToolKind::Edit,
        "delete" => s2::ToolKind::Delete,
        "move" => s2::ToolKind::Move,
        "search" => s2::ToolKind::Search,
        "execute" => s2::ToolKind::Execute,
        "think" => s2::ToolKind::Think,
        "fetch" => s2::ToolKind::Fetch,
        "switch_mode" => s2::ToolKind::SwitchMode,
        _ => s2::ToolKind::Other,
    }
}

fn status_v1(status: &str) -> s1::ToolCallStatus {
    match status {
        "pending" => s1::ToolCallStatus::Pending,
        "completed" => s1::ToolCallStatus::Completed,
        "failed" => s1::ToolCallStatus::Failed,
        _ => s1::ToolCallStatus::InProgress,
    }
}

fn status_v2(status: &str) -> s2::ToolCallStatus {
    match status {
        "pending" => s2::ToolCallStatus::Pending,
        "completed" => s2::ToolCallStatus::Completed,
        "failed" => s2::ToolCallStatus::Failed,
        "in_progress" => s2::ToolCallStatus::InProgress,
        other => s2::ToolCallStatus::Other(other.to_string()),
    }
}

fn content_v1(content: CardContent) -> Option<s1::ToolCallContent> {
    match content {
        CardContent::Text(text) => Some(s1::ToolCallContent::Content(s1::Content::new(text_v1(
            &text,
        )))),
        CardContent::Terminal(id) => Some(s1::ToolCallContent::Terminal(s1::Terminal::new(id))),
        CardContent::FileDiff {
            path,
            old_text,
            new_text,
        } => Some(s1::ToolCallContent::Diff(
            s1::Diff::new(path, new_text).old_text(old_text),
        )),
        // ACP v1's diff content is whole-file text, which a stored patch
        // cannot give.
        CardContent::Diff(_) => None,
    }
}

fn content_v2(content: CardContent) -> Option<s2::ToolCallContent> {
    Some(match content {
        CardContent::Text(text) => {
            s2::ToolCallContent::Content(Box::new(s2::Content::new(text_v2(&text))))
        }
        CardContent::Terminal(id) => s2::ToolCallContent::Terminal(s2::Terminal::new(id)),
        // The editor file tools exist only in ACP v1.
        CardContent::FileDiff { .. } => return None,
        CardContent::Diff(diff) => {
            let changes = diff
                .changes
                .into_iter()
                .map(|(added, path)| {
                    let path = std::path::PathBuf::from(path);
                    if added {
                        s2::DiffChange::add(path)
                    } else {
                        s2::DiffChange::modify(path)
                    }
                })
                .collect();
            s2::ToolCallContent::Diff(s2::Diff::patch(diff.patch, changes))
        }
    })
}

/// A v2 field that is cleared with `null`.
pub fn clear<T>() -> MaybeUndefined<T> {
    MaybeUndefined::Null
}

/// A standard `compaction_update`. `status` is an ACP compaction status
/// (`in_progress`, `completed`, `failed`, `cancelled`) or the adapter's
/// `_`-prefixed extension status.
pub fn compaction(
    acp_sid: &str,
    ver: u8,
    compaction_id: &str,
    status: &str,
    error: Option<String>,
    meta: Option<Meta>,
) -> Note {
    if ver == 2 {
        let status = match status {
            "in_progress" => s2::CompactionStatus::InProgress,
            "completed" => s2::CompactionStatus::Completed,
            "failed" => s2::CompactionStatus::Failed,
            "cancelled" => s2::CompactionStatus::Cancelled,
            other => s2::CompactionStatus::Other(other.to_string()),
        };
        let mut update = s2::CompactionUpdate::new(compaction_id.to_string(), status);
        if let Some(error) = error {
            update = update.error(error);
        }
        if let Some(meta) = meta {
            update = update.meta(meta);
        }
        return Note::v2(acp_sid, s2::SessionUpdate::CompactionUpdate(update));
    }
    let status = match status {
        "in_progress" => s1::CompactionStatus::InProgress,
        "completed" => s1::CompactionStatus::Completed,
        "failed" => s1::CompactionStatus::Failed,
        "cancelled" => s1::CompactionStatus::Cancelled,
        other => s1::CompactionStatus::Other(other.to_string()),
    };
    let mut update = s1::CompactionUpdate::new(compaction_id.to_string(), status);
    if let Some(error) = error {
        update = update.error(error);
    }
    if let Some(meta) = meta {
        update = update.meta(meta);
    }
    Note::v1(acp_sid, s1::SessionUpdate::CompactionUpdate(update))
}

/// A JetBrains AIR update no ACP schema of the version defines
/// (`subagent_spawned`, `subagent_state_update`, `async_task_spawned`,
/// `async_task_state_update`). ACP v2 carries it as its typed `Other`
/// update; v1 has no such variant, so it goes out untyped. `update` is the
/// update object with its `sessionUpdate` discriminator.
pub fn air(acp_sid: &str, ver: u8, update: Value) -> Note {
    if ver == 2
        && let Value::Object(fields) = &update
        && let Some(kind) = fields.get("sessionUpdate").and_then(Value::as_str)
    {
        let fields = fields
            .iter()
            .filter(|(key, _)| key.as_str() != "sessionUpdate")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        return Note::v2(
            acp_sid,
            s2::SessionUpdate::Other(s2::OtherSessionUpdate::new(kind, fields)),
        );
    }
    Note::ext(acp_sid, update)
}

/// A `session_info_update` title change: leave it, set it, or clear it.
#[derive(Debug, Clone)]
pub enum Title {
    Keep,
    Set(String),
    Clear,
}

/// A `session_info_update`.
pub fn session_info(
    acp_sid: &str,
    ver: u8,
    title: Title,
    updated_at: Option<String>,
    meta: Option<Meta>,
) -> Note {
    if ver == 2 {
        let mut update = s2::SessionInfoUpdate::new();
        match title {
            Title::Keep => {}
            Title::Set(title) => update = update.title(title),
            Title::Clear => update = update.title(clear()),
        }
        if let Some(updated_at) = updated_at {
            update = update.updated_at(updated_at);
        }
        if let Some(meta) = meta {
            update = update.meta(meta);
        }
        return Note::v2(acp_sid, s2::SessionUpdate::SessionInfoUpdate(update));
    }
    let mut update = s1::SessionInfoUpdate::new();
    match title {
        Title::Keep => {}
        Title::Set(title) => update = update.title(title),
        Title::Clear => update = update.title(clear()),
    }
    if let Some(updated_at) = updated_at {
        update = update.updated_at(updated_at);
    }
    if let Some(meta) = meta {
        update = update.meta(meta);
    }
    Note::v1(acp_sid, s1::SessionUpdate::SessionInfoUpdate(update))
}

/// ACP v2 `terminal_update` opening an agent-owned terminal for a command.
pub fn terminal_opened(acp_sid: &str, terminal_id: &str, command: &str, cwd: Option<&str>) -> Note {
    let mut update = s2::TerminalUpdate::new(terminal_id.to_string()).command(command.to_string());
    if let Some(cwd) = cwd {
        update = update.cwd(s2::AbsolutePath::new(cwd));
    }
    Note::v2(acp_sid, s2::SessionUpdate::TerminalUpdate(update))
}

/// ACP v2 `terminal_update` settling a terminal: its final output snapshot
/// (`data`, base64) and exit status.
pub fn terminal_exited(
    acp_sid: &str,
    terminal_id: &str,
    data: String,
    exit_code: Option<i64>,
    exit_meta: Option<Meta>,
    meta: Option<Meta>,
) -> Note {
    let exit = s2::TerminalExitStatus::new()
        .exit_code(exit_code)
        .meta(exit_meta);
    let mut update = s2::TerminalUpdate::new(terminal_id.to_string())
        .output(s2::TerminalOutput::new(data))
        .exit_status(exit);
    if let Some(meta) = meta {
        update = update.meta(meta);
    }
    Note::v2(acp_sid, s2::SessionUpdate::TerminalUpdate(update))
}

/// ACP v2 `terminal_output_chunk`: output a terminal produced (`data`,
/// base64).
pub fn terminal_output(acp_sid: &str, terminal_id: &str, data: String) -> Note {
    Note::v2(
        acp_sid,
        s2::SessionUpdate::TerminalOutputChunk(s2::TerminalOutputChunk::new(
            terminal_id.to_string(),
            data,
        )),
    )
}

/// ACP v2 `tool_call_content_chunk`: text appended to a tool call's content.
pub fn tool_text_chunk(acp_sid: &str, tool_call_id: &str, text: &str) -> Note {
    Note::v2(
        acp_sid,
        s2::SessionUpdate::ToolCallContentChunk(s2::ToolCallContentChunk::new(
            tool_call_id.to_string(),
            s2::ToolCallContent::Content(Box::new(s2::Content::new(text_v2(text)))),
        )),
    )
}

/// A whole message on replay: ACP v2 `user_message` or `agent_message`, and
/// for ACP v1 the one chunk of it, which names its message.
pub fn message(
    acp_sid: &str,
    ver: u8,
    user: bool,
    msg_id: &str,
    text: &str,
    meta: Option<Meta>,
) -> Note {
    if ver == 2 {
        let content = vec![text_v2(text)];
        let update = if user {
            let mut message = s2::UserMessage::new(msg_id.to_string()).content(content);
            if let Some(meta) = meta {
                message = message.meta(meta);
            }
            s2::SessionUpdate::UserMessage(message)
        } else {
            let mut message = s2::AgentMessage::new(msg_id.to_string()).content(content);
            if let Some(meta) = meta {
                message = message.meta(meta);
            }
            s2::SessionUpdate::AgentMessage(message)
        };
        return Note::v2(acp_sid, update);
    }
    let chunk = s1::ContentChunk::new(text_v1(text))
        .message_id(s1::MessageId::new(msg_id))
        .meta(meta);
    Note::v1(
        acp_sid,
        if user {
            s1::SessionUpdate::UserMessageChunk(chunk)
        } else {
            s1::SessionUpdate::AgentMessageChunk(chunk)
        },
    )
}
