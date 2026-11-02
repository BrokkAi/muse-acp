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

/// An `agent_message_chunk` or `agent_thought_chunk` of text. ACP v2 chunks name their message.
#[derive(Clone, Copy)]
pub enum Chunk {
    Agent,
    Thought,
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
            },
        )
    } else {
        let chunk = s1::ContentChunk::new(text_v1(text)).meta(meta);
        Note::v1(
            acp_sid,
            match which {
                Chunk::Agent => s1::SessionUpdate::AgentMessageChunk(chunk),
                Chunk::Thought => s1::SessionUpdate::AgentThoughtChunk(chunk),
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
            return Note::v2(acp_sid, s2::SessionUpdate::ToolCallUpdate(self.update_v2()));
        }
        if !create {
            return Note::v1(acp_sid, s1::SessionUpdate::ToolCallUpdate(self.update_v1()));
        }
        let (content, locations) = self.parts_v1();
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
        Note::v1(acp_sid, s1::SessionUpdate::ToolCall(call))
    }

    #[allow(clippy::type_complexity)]
    fn parts_v1(
        &self,
    ) -> (
        Option<Vec<s1::ToolCallContent>>,
        Option<Vec<s1::ToolCallLocation>>,
    ) {
        let content = self.content.clone().map(|content| {
            content
                .into_iter()
                .filter_map(content_v1)
                .collect::<Vec<_>>()
        });
        let locations = self.locations.clone().map(|locations| {
            locations
                .into_iter()
                .map(|l| s1::ToolCallLocation::new(l.path).line(l.line))
                .collect::<Vec<_>>()
        });
        (content, locations)
    }

    /// The card as an ACP v1 `ToolCallUpdate`.
    pub fn update_v1(self) -> s1::ToolCallUpdate {
        let (content, locations) = self.parts_v1();
        let fields = s1::ToolCallUpdateFields::new()
            .name(self.name)
            .title(self.title)
            .kind(self.kind.as_deref().map(kind_v1))
            .status(self.status.as_deref().map(status_v1))
            .content(content)
            .locations(locations)
            .raw_input(self.raw_input);
        s1::ToolCallUpdate::new(self.id, fields).meta(self.meta)
    }

    /// The card as an ACP v2 `ToolCallUpdate`.
    pub fn update_v2(self) -> s2::ToolCallUpdate {
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
        update
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

/// One slash command the session offers.
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// The input hint, for a command that takes text after its name.
    pub hint: Option<String>,
    /// The command's human-readable name, in `_meta.muse.title`: ACP's
    /// `name` is the slash token.
    pub title: Option<String>,
}

/// An `available_commands_update`.
pub fn available_commands(acp_sid: &str, ver: u8, commands: Vec<Command>) -> Note {
    let meta = |title: Option<String>| {
        title.and_then(|title| meta(&serde_json::json!({"muse": {"title": title}}).to_string()))
    };
    if ver == 2 {
        let commands = commands
            .into_iter()
            .map(|c| {
                s2::AvailableCommand::Command(
                    s2::Command::new(c.name, c.description)
                        .input(c.hint.map(|hint| {
                            s2::AvailableCommandInput::Text(s2::TextCommandInput::new(hint))
                        }))
                        .meta(meta(c.title)),
                )
            })
            .collect();
        return Note::v2(
            acp_sid,
            s2::SessionUpdate::AvailableCommandsUpdate(s2::AvailableCommandsUpdate::new(commands)),
        );
    }
    let commands = commands
        .into_iter()
        .map(|c| {
            s1::AvailableCommand::new(c.name, c.description)
                .input(c.hint.map(|hint| {
                    s1::AvailableCommandInput::Unstructured(s1::UnstructuredCommandInput::new(hint))
                }))
                .meta(meta(c.title))
        })
        .collect();
    Note::v1(
        acp_sid,
        s1::SessionUpdate::AvailableCommandsUpdate(s1::AvailableCommandsUpdate::new(commands)),
    )
}

/// One session configuration option (a selector or a toggle).
#[derive(Debug, Clone)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// An ACP option category (`mode`, `model`, `thought_level`).
    pub category: Option<String>,
    pub kind: ConfigKind,
    pub meta: Option<Meta>,
}

#[derive(Debug, Clone)]
pub enum ConfigKind {
    /// A selector: its current value, and `(value, name, description)` per
    /// choice.
    Select {
        current: String,
        choices: Vec<(String, String, Option<String>)>,
    },
    Boolean(bool),
}

/// A session's complete `configOptions`, in its version's schema types.
#[derive(Debug, Clone)]
pub enum ConfigOptions {
    V1(Vec<s1::SessionConfigOption>),
    V2(Vec<s2::SessionConfigOption>),
}

fn category_v1(category: &str) -> s1::SessionConfigOptionCategory {
    match category {
        "mode" => s1::SessionConfigOptionCategory::Mode,
        "model" => s1::SessionConfigOptionCategory::Model,
        "model_config" => s1::SessionConfigOptionCategory::ModelConfig,
        "thought_level" => s1::SessionConfigOptionCategory::ThoughtLevel,
        other => s1::SessionConfigOptionCategory::Other(other.to_string()),
    }
}

fn category_v2(category: &str) -> s2::SessionConfigOptionCategory {
    match category {
        "mode" => s2::SessionConfigOptionCategory::Mode,
        "model" => s2::SessionConfigOptionCategory::Model,
        "model_config" => s2::SessionConfigOptionCategory::ModelConfig,
        "thought_level" => s2::SessionConfigOptionCategory::ThoughtLevel,
        other => s2::SessionConfigOptionCategory::Other(other.to_string()),
    }
}

impl ConfigOptions {
    pub fn new(ver: u8, options: Vec<ConfigOption>) -> ConfigOptions {
        if ver == 2 {
            return ConfigOptions::V2(
                options
                    .into_iter()
                    .map(|o| {
                        let option = match o.kind {
                            ConfigKind::Select { current, choices } => {
                                let choices: Vec<s2::SessionConfigSelectOption> = choices
                                    .into_iter()
                                    .map(|(value, name, description)| {
                                        s2::SessionConfigSelectOption::new(value, name)
                                            .description(description)
                                    })
                                    .collect();
                                s2::SessionConfigOption::select(o.id, o.name, current, choices)
                            }
                            ConfigKind::Boolean(on) => {
                                s2::SessionConfigOption::boolean(o.id, o.name, on)
                            }
                        };
                        option
                            .description(o.description)
                            .category(o.category.as_deref().map(category_v2))
                            .meta(o.meta)
                    })
                    .collect(),
            );
        }
        ConfigOptions::V1(
            options
                .into_iter()
                .map(|o| {
                    let option = match o.kind {
                        ConfigKind::Select { current, choices } => {
                            let choices: Vec<s1::SessionConfigSelectOption> = choices
                                .into_iter()
                                .map(|(value, name, description)| {
                                    s1::SessionConfigSelectOption::new(value, name)
                                        .description(description)
                                })
                                .collect();
                            s1::SessionConfigOption::select(o.id, o.name, current, choices)
                        }
                        ConfigKind::Boolean(on) => {
                            s1::SessionConfigOption::boolean(o.id, o.name, on)
                        }
                    };
                    option
                        .description(o.description)
                        .category(o.category.as_deref().map(category_v1))
                        .meta(o.meta)
                })
                .collect(),
        )
    }

    /// The `configOptions` array as the SDK serializes it.
    #[cfg(test)]
    pub fn json(&self) -> Value {
        match self {
            ConfigOptions::V1(options) => serde_json::to_value(options),
            ConfigOptions::V2(options) => serde_json::to_value(options),
        }
        .unwrap_or(Value::Null)
    }

    /// A `config_option_update` carrying the complete list.
    pub fn update(self, acp_sid: &str) -> Note {
        match self {
            ConfigOptions::V1(options) => Note::v1(
                acp_sid,
                s1::SessionUpdate::ConfigOptionUpdate(s1::ConfigOptionUpdate::new(options)),
            ),
            ConfigOptions::V2(options) => Note::v2(
                acp_sid,
                s2::SessionUpdate::ConfigOptionUpdate(s2::ConfigOptionUpdate::new(options)),
            ),
        }
    }
}

/// ACP v1's legacy session mode state: the current mode and the choices.
pub fn mode_state(current: &str, modes: &[(&str, &str, &str)]) -> s1::SessionModeState {
    s1::SessionModeState::new(
        current.to_string(),
        modes
            .iter()
            .map(|(id, name, description)| {
                s1::SessionMode::new(id.to_string(), name.to_string())
                    .description(description.to_string())
            })
            .collect(),
    )
}

/// A `usage_update`: context occupancy and size, the session's cost when
/// known (`amount`, `currency`, and the cost's `_meta`), and `_meta`.
pub fn usage(
    acp_sid: &str,
    ver: u8,
    used: u64,
    size: u64,
    cost: Option<(f64, String, Option<Meta>)>,
    meta: Option<Meta>,
) -> Note {
    if ver == 2 {
        let mut update = s2::UsageUpdate::new(used, size).meta(meta);
        if let Some((amount, currency, cost_meta)) = cost {
            update = update.cost(s2::Cost::new(amount, currency).meta(cost_meta));
        }
        return Note::v2(acp_sid, s2::SessionUpdate::UsageUpdate(update));
    }
    let cost =
        cost.map(|(amount, currency, cost_meta)| s1::Cost::new(amount, currency).meta(cost_meta));
    Note::v1(
        acp_sid,
        s1::SessionUpdate::UsageUpdate(s1::UsageUpdate::new(used, size).cost(cost).meta(meta)),
    )
}

/// ACP v2 `state_update`: `running`, `requires_action`, or `idle` with why
/// the foreground work stopped (an ACP stop reason, or the adapter's
/// `_`-prefixed extension reason).
pub fn state(acp_sid: &str, state: &str, stop: Option<&str>) -> Note {
    let update = match state {
        "running" => s2::StateUpdate::Running(s2::RunningStateUpdate::new()),
        "requires_action" => s2::StateUpdate::RequiresAction(s2::RequiresActionStateUpdate::new()),
        _ => s2::StateUpdate::Idle(s2::IdleStateUpdate::new().stop_reason(stop.map(
            |stop| match stop {
                "end_turn" => s2::StopReason::EndTurn,
                "max_tokens" => s2::StopReason::MaxTokens,
                "max_turn_requests" => s2::StopReason::MaxTurnRequests,
                "refusal" => s2::StopReason::Refusal,
                "cancelled" => s2::StopReason::Cancelled,
                other => s2::StopReason::Other(s2::OtherStopReason::new(other, Default::default())),
            },
        ))),
    };
    Note::v2(acp_sid, s2::SessionUpdate::StateUpdate(update))
}

/// One todo item of a session plan: its text and ACP entry status
/// (`pending`, `in_progress`, `completed`), at medium priority.
pub type PlanEntry = (String, &'static str);

/// The session's plan. ACP v1 has one implicit plan per session, cleared
/// with an empty `plan` update. ACP v2 names plans: a snapshot is a
/// `plan_update` with an `items` checklist, and an empty list is
/// `plan_removed`.
pub fn plan(acp_sid: &str, ver: u8, plan_id: &str, entries: Vec<PlanEntry>) -> Note {
    if ver == 2 {
        if entries.is_empty() {
            return Note::v2(
                acp_sid,
                s2::SessionUpdate::PlanRemoved(s2::PlanRemoved::new(plan_id.to_string())),
            );
        }
        let entries = entries
            .into_iter()
            .map(|(content, status)| {
                let status = match status {
                    "in_progress" => s2::PlanEntryStatus::InProgress,
                    "completed" => s2::PlanEntryStatus::Completed,
                    _ => s2::PlanEntryStatus::Pending,
                };
                s2::PlanEntry::new(content, s2::PlanEntryPriority::Medium, status)
            })
            .collect();
        return Note::v2(
            acp_sid,
            s2::SessionUpdate::PlanUpdate(s2::PlanUpdate::new(s2::PlanUpdateContent::Items(
                s2::PlanItems::new(plan_id.to_string(), entries),
            ))),
        );
    }
    let entries = entries
        .into_iter()
        .map(|(content, status)| {
            let status = match status {
                "in_progress" => s1::PlanEntryStatus::InProgress,
                "completed" => s1::PlanEntryStatus::Completed,
                _ => s1::PlanEntryStatus::Pending,
            };
            s1::PlanEntry::new(content, s1::PlanEntryPriority::Medium, status)
        })
        .collect();
    Note::v1(acp_sid, s1::SessionUpdate::Plan(s1::Plan::new(entries)))
}

/// The content blocks of an accepted prompt, as the client sent them, for
/// echoing the user's message into the transcript.
#[derive(Debug, Clone)]
pub enum Echo {
    V1(Vec<s1::ContentBlock>),
    V2(Vec<s2::ContentBlock>),
}

impl Default for Echo {
    fn default() -> Echo {
        Echo::V1(Vec::new())
    }
}

impl Echo {
    /// Whether the prompt had no blocks.
    pub fn is_empty(&self) -> bool {
        match self {
            Echo::V1(blocks) => blocks.is_empty(),
            Echo::V2(blocks) => blocks.is_empty(),
        }
    }

    /// The text blocks, as the user typed them.
    pub fn text(&self) -> String {
        let texts: Vec<&str> = match self {
            Echo::V1(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    s1::ContentBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect(),
            Echo::V2(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    s2::ContentBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect(),
        };
        texts.join("\n")
    }

    /// The echo as updates: ACP v2's whole `user_message`, or one ACP v1
    /// `user_message_chunk` per block, all naming `msg_id`.
    pub fn notes(&self, acp_sid: &str, msg_id: &str) -> Vec<Note> {
        match self {
            Echo::V2(blocks) => vec![Note::v2(
                acp_sid,
                s2::SessionUpdate::UserMessage(
                    s2::UserMessage::new(msg_id.to_string()).content(blocks.clone()),
                ),
            )],
            Echo::V1(blocks) => blocks
                .iter()
                .map(|block| {
                    Note::v1(
                        acp_sid,
                        s1::SessionUpdate::UserMessageChunk(
                            s1::ContentChunk::new(block.clone())
                                .message_id(s1::MessageId::new(msg_id)),
                        ),
                    )
                })
                .collect(),
        }
    }
}

/// A prompt block of either version, kept for its echo.
#[derive(Debug, Clone)]
pub enum EchoBlock {
    V1(s1::ContentBlock),
    V2(s2::ContentBlock),
}

impl Echo {
    /// Collect accepted blocks; a block of the other version is skipped
    /// (a prompt's blocks all share its connection's version).
    pub fn from_blocks(ver: u8, blocks: Vec<EchoBlock>) -> Echo {
        if ver == 2 {
            Echo::V2(
                blocks
                    .into_iter()
                    .filter_map(|b| match b {
                        EchoBlock::V2(b) => Some(b),
                        EchoBlock::V1(_) => None,
                    })
                    .collect(),
            )
        } else {
            Echo::V1(
                blocks
                    .into_iter()
                    .filter_map(|b| match b {
                        EchoBlock::V1(b) => Some(b),
                        EchoBlock::V2(_) => None,
                    })
                    .collect(),
            )
        }
    }
}

/// A `notice`: information for the user that is not session history.
/// `severity` is `info`, `warning` or `error`.
pub fn notice(acp_sid: &str, ver: u8, severity: &str, title: &str, description: &str) -> Note {
    if ver == 2 {
        let severity = match severity {
            "warning" => s2::NoticeSeverity::Warning,
            "error" => s2::NoticeSeverity::Error,
            _ => s2::NoticeSeverity::Info,
        };
        return Note::v2(
            acp_sid,
            s2::SessionUpdate::Notice(
                s2::Notice::new(severity, title.to_string()).description(description.to_string()),
            ),
        );
    }
    let severity = match severity {
        "warning" => s1::NoticeSeverity::Warning,
        "error" => s1::NoticeSeverity::Error,
        _ => s1::NoticeSeverity::Info,
    };
    Note::v1(
        acp_sid,
        s1::SessionUpdate::Notice(
            s1::Notice::new(severity, title.to_string()).description(description.to_string()),
        ),
    )
}

/// ACP v1 `current_mode_update`, for clients of the legacy session modes.
pub fn current_mode(acp_sid: &str, mode: &str) -> Note {
    Note::v1(
        acp_sid,
        s1::SessionUpdate::CurrentModeUpdate(s1::CurrentModeUpdate::new(mode.to_string())),
    )
}

/// One `session/list` row.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub session_id: String,
    pub cwd: String,
    pub additional_directories: Vec<String>,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub meta: Option<Meta>,
}

/// A request the adapter sends the client, as the negotiated version's SDK
/// request type.
#[derive(Debug, Clone)]
pub enum Ask {
    PermissionV1(s1::RequestPermissionRequest),
    PermissionV2(s2::RequestPermissionRequest),
    ElicitationV1(s1::CreateElicitationRequest),
    ElicitationV2(s2::CreateElicitationRequest),
}

impl Ask {
    /// The request's `params`, for tests.
    #[cfg(test)]
    pub fn params(&self) -> Value {
        match self {
            Ask::PermissionV1(r) => serde_json::to_value(r),
            Ask::PermissionV2(r) => serde_json::to_value(r),
            Ask::ElicitationV1(r) => serde_json::to_value(r),
            Ask::ElicitationV2(r) => serde_json::to_value(r),
        }
        .unwrap_or(Value::Null)
    }
}

/// One `session/request_permission` option.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once`, or `reject_always`.
    pub kind: &'static str,
}

fn option_kind_v1(kind: &str) -> s1::PermissionOptionKind {
    match kind {
        "allow_always" => s1::PermissionOptionKind::AllowAlways,
        "reject_once" => s1::PermissionOptionKind::RejectOnce,
        "reject_always" => s1::PermissionOptionKind::RejectAlways,
        _ => s1::PermissionOptionKind::AllowOnce,
    }
}

fn option_kind_v2(kind: &str) -> s2::PermissionOptionKind {
    match kind {
        "allow_always" => s2::PermissionOptionKind::AllowAlways,
        "reject_once" => s2::PermissionOptionKind::RejectOnce,
        "reject_always" => s2::PermissionOptionKind::RejectAlways,
        _ => s2::PermissionOptionKind::AllowOnce,
    }
}

/// `session/request_permission` for the tool call `card`. ACP v2 titles the
/// prompt and names the tool call as its subject; v1 shows the tool call.
pub fn permission(
    acp_sid: &str,
    ver: u8,
    title: &str,
    card: Card,
    options: Vec<PermissionOption>,
) -> Ask {
    if ver == 2 {
        let options = options
            .into_iter()
            .map(|o| s2::PermissionOption::new(o.id, o.name, option_kind_v2(o.kind)))
            .collect();
        let subject = s2::RequestPermissionSubject::ToolCall(Box::new(
            s2::ToolCallPermissionSubject::new(card.update_v2()),
        ));
        return Ask::PermissionV2(
            s2::RequestPermissionRequest::new(acp_sid.to_string(), title, options).subject(subject),
        );
    }
    let options = options
        .into_iter()
        .map(|o| s1::PermissionOption::new(o.id, o.name, option_kind_v1(o.kind)))
        .collect();
    Ask::PermissionV1(s1::RequestPermissionRequest::new(
        acp_sid.to_string(),
        card.update_v1(),
        options,
    ))
}

/// An `elicitation/create` form: its fields, in display order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Form {
    pub fields: Vec<Field>,
}

/// One form field.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub kind: FieldKind,
}

/// What a form field asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text {
        max_length: Option<u32>,
        default: Option<String>,
    },
    /// One of a fixed list of strings.
    Enum {
        values: Vec<String>,
        default: Option<String>,
    },
    /// One titled choice.
    OneOf(Vec<Choice>),
    /// Any number of titled choices.
    AnyOf {
        choices: Vec<Choice>,
        min_items: Option<u64>,
        max_items: Option<u64>,
    },
    Flag {
        default: Option<bool>,
    },
}

/// One titled choice. `meta` is the choice's `_meta`.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub value: String,
    pub title: String,
    pub description: Option<String>,
    pub meta: Option<Meta>,
}

impl Field {
    pub fn new(key: &str, kind: FieldKind) -> Field {
        Field {
            key: key.to_string(),
            title: None,
            description: None,
            required: false,
            kind,
        }
    }
    pub fn title(mut self, title: &str) -> Field {
        self.title = Some(title.to_string());
        self
    }
    pub fn description(mut self, description: &str) -> Field {
        self.description = Some(description.to_string());
        self
    }
    pub fn required(mut self, required: bool) -> Field {
        self.required = required;
        self
    }
}

/// The form's `requestedSchema`, in one version's schema types. The two
/// versions define the same elicitation schema in separate modules.
macro_rules! requested_schema {
    ($s:ident, $form:expr) => {{
        let mut schema = $s::ElicitationSchema::new();
        for field in $form.fields {
            let property: $s::ElicitationPropertySchema = match field.kind {
                FieldKind::Text {
                    max_length,
                    default,
                } => $s::StringPropertySchema::new()
                    .title(field.title)
                    .description(field.description)
                    .max_length(max_length)
                    .default_value(default)
                    .into(),
                FieldKind::Enum { values, default } => $s::StringPropertySchema::new()
                    .title(field.title)
                    .description(field.description)
                    .enum_values(values)
                    .default_value(default)
                    .into(),
                FieldKind::OneOf(choices) => $s::StringPropertySchema::new()
                    .title(field.title)
                    .description(field.description)
                    .one_of(
                        choices
                            .into_iter()
                            .map(|c| {
                                $s::EnumOption::new(c.value, c.title)
                                    .description(c.description)
                                    .meta(c.meta)
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into(),
                FieldKind::AnyOf {
                    choices,
                    min_items,
                    max_items,
                } => $s::MultiSelectPropertySchema::titled(
                    choices
                        .into_iter()
                        .map(|c| {
                            $s::EnumOption::new(c.value, c.title)
                                .description(c.description)
                                .meta(c.meta)
                        })
                        .collect(),
                )
                .title(field.title)
                .description(field.description)
                .min_items(min_items)
                .max_items(max_items)
                .into(),
                FieldKind::Flag { default } => $s::BooleanPropertySchema::new()
                    .title(field.title)
                    .description(field.description)
                    .default_value(default)
                    .into(),
            };
            schema = schema.property(field.key, property, field.required);
        }
        schema
    }};
}

/// A session-scoped `elicitation/create` form, tied to the tool call
/// `tool_call_id` when there is one.
pub fn form(acp_sid: &str, ver: u8, tool_call_id: Option<&str>, message: &str, form: Form) -> Ask {
    if ver == 2 {
        let scope = s2::ElicitationSessionScope::new(acp_sid.to_string())
            .tool_call_id(tool_call_id.map(|id| s2::ToolCallId::new(id.to_string())));
        let mode = s2::ElicitationFormMode::new(scope, requested_schema!(s2, form));
        return Ask::ElicitationV2(s2::CreateElicitationRequest::new(mode, message));
    }
    let scope = s1::ElicitationSessionScope::new(acp_sid.to_string())
        .tool_call_id(tool_call_id.map(|id| s1::ToolCallId::new(id.to_string())));
    let mode = s1::ElicitationFormMode::new(scope, requested_schema!(s1, form));
    Ask::ElicitationV1(s1::CreateElicitationRequest::new(mode, message))
}

/// A request-scoped URL `elicitation/create`: open `url` while the client's
/// request `request_id` is outstanding. ACP v2 only (`auth/login`).
pub fn url_elicitation(
    request_id: &crate::editor::RequestId,
    elicitation_id: &str,
    url: &str,
    message: &str,
) -> Option<Ask> {
    let request_id: s2::RequestId = serde_json::from_value(request_id.value()).ok()?;
    let scope = s2::ElicitationRequestScope::new(request_id);
    let mode = s2::ElicitationUrlMode::new(scope, elicitation_id.to_string(), url);
    Some(Ask::ElicitationV2(s2::CreateElicitationRequest::new(
        mode, message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_form() -> Form {
        Form {
            fields: vec![
                Field::new(
                    "q0",
                    FieldKind::AnyOf {
                        choices: vec![Choice {
                            value: "a".to_string(),
                            title: "A".to_string(),
                            description: Some("first".to_string()),
                            meta: muse_meta(r#""preview":{"content":"x","format":"markdown"}"#),
                        }],
                        min_items: Some(0),
                        max_items: Some(2),
                    },
                )
                .title("Pick")
                .required(true),
                Field::new(
                    "ok",
                    FieldKind::Flag {
                        default: Some(false),
                    },
                )
                .title("OK"),
            ],
        }
    }

    #[test]
    fn forms_serialize_as_each_versions_elicitation_schema() {
        for ver in [1, 2] {
            let params = form("s-1", ver, Some("call-1"), "Answer", sample_form()).params();
            assert_eq!(params["sessionId"], "s-1", "v{ver}");
            assert_eq!(params["toolCallId"], "call-1", "v{ver}");
            assert_eq!(params["mode"], "form", "v{ver}");
            assert_eq!(params["message"], "Answer", "v{ver}");
            let schema = &params["requestedSchema"];
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["required"], json!(["q0"]));
            assert_eq!(
                schema["properties"]["q0"],
                json!({
                    "type": "array",
                    "title": "Pick",
                    "minItems": 0,
                    "maxItems": 2,
                    "items": {"anyOf": [{
                        "const": "a",
                        "title": "A",
                        "description": "first",
                        "_meta": {"muse": {"preview": {"content": "x", "format": "markdown"}}}
                    }]}
                }),
                "v{ver}"
            );
            assert_eq!(
                schema["properties"]["ok"],
                json!({"type": "boolean", "title": "OK", "default": false})
            );
        }
        // No tool call: the scope is the session alone.
        let params = form("s-1", 2, None, "m", Form::default()).params();
        assert!(params.get("toolCallId").is_none());
        assert!(params["requestedSchema"].get("required").is_none());
    }

    #[test]
    fn permission_requests_follow_each_versions_shape() {
        let mut card = Card::new("call-1");
        card.title = Some("Run ls".to_string());
        card.kind = Some("execute".to_string());
        card.status = Some("pending".to_string());
        let options = vec![
            PermissionOption {
                id: "c-once".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once",
            },
            PermissionOption {
                id: "c-deny".to_string(),
                name: "Reject".to_string(),
                kind: "reject_always",
            },
        ];
        let v1 = permission("s-1", 1, "Run ls", card.clone(), options.clone()).params();
        assert_eq!(
            v1,
            json!({
                "sessionId": "s-1",
                "toolCall": {"toolCallId": "call-1", "title": "Run ls", "kind": "execute", "status": "pending"},
                "options": [
                    {"optionId": "c-once", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "c-deny", "name": "Reject", "kind": "reject_always"}
                ]
            })
        );
        let v2 = permission("s-1", 2, "Run ls", card, options).params();
        assert_eq!(v2["title"], "Run ls");
        assert_eq!(v2["subject"]["type"], "tool_call");
        assert_eq!(v2["subject"]["toolCall"]["toolCallId"], "call-1");
        assert_eq!(v2["subject"]["toolCall"]["kind"], "execute");
        assert_eq!(v2["options"][1]["kind"], "reject_always");
    }

    #[test]
    fn a_url_elicitation_is_scoped_to_the_login_request() {
        let id = crate::editor::RequestId::of_value(&7);
        let params = url_elicitation(&id, "login-1", "https://example.test/device", "Sign in")
            .expect("numeric id")
            .params();
        assert_eq!(
            params,
            json!({
                "requestId": 7,
                "mode": "url",
                "elicitationId": "login-1",
                "url": "https://example.test/device",
                "message": "Sign in"
            })
        );
    }
}
