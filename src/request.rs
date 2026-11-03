//! The client's ACP requests, as the adapter reads them.
//!
//! The SDK parses every request into its protocol version's schema type and
//! refuses one that does not fit (`-32602`) before the adapter sees it. ACP
//! v1 and v2 define most methods alike but as distinct Rust types, so this
//! module maps each version's type onto one view per method, and every
//! handler in `main.rs` is written once. Nothing here parses JSON: fields are
//! read from the SDK's structs, and only `_meta` (whose members ACP leaves to
//! extensions) and the params of extension methods stay JSON.

use agent_client_protocol::{v1, v2};
use serde_json::{Map, Value};

use crate::editor::{RequestId, V1Message, V2Message};

/// A request's `_meta`: ACP reserves it for extensions.
pub type Meta = Map<String, Value>;

/// The fields `session/new`, `session/load`, `session/resume` and
/// `session/fork` share.
pub struct Setup {
    /// The session to load, resume or fork; empty for `session/new`.
    pub session_id: String,
    pub cwd: String,
    pub additional_directories: Vec<String>,
    pub mcp_servers: Vec<McpServer>,
    /// ACP v2 `replayFrom` on `session/resume`, and JetBrains AIR's
    /// extension of it to `session/fork`. `None` when absent or `null`.
    pub replay_from: Option<Replay>,
    pub meta: Meta,
}

/// A history replay cursor.
pub enum Replay {
    /// `{"type":"start"}`: all retained history.
    Start,
    /// A custom or future cursor type.
    Other(String),
}

/// One client MCP server.
pub enum McpServer {
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Http {
        name: String,
        url: String,
        headers: Vec<(String, String)>,
    },
    /// ACP v1's SSE transport, which Muse does not run.
    Sse { name: String },
    /// A custom or future transport.
    Other { kind: String, name: Option<String> },
}

impl McpServer {
    pub fn name(&self) -> Option<&str> {
        match self {
            McpServer::Stdio { name, .. }
            | McpServer::Http { name, .. }
            | McpServer::Sse { name } => Some(name),
            McpServer::Other { name, .. } => name.as_deref(),
        }
    }
}

/// `session/prompt`.
pub struct Prompt {
    pub session_id: String,
    pub blocks: Vec<Block>,
    pub meta: Meta,
}

/// One prompt content block, with the block as the client sent it, for
/// echoing it back in the transcript.
pub struct Block {
    pub content: Content,
    pub echo: crate::out::EchoBlock,
}

pub enum Content {
    Text(String),
    Image {
        data: String,
        mime_type: String,
    },
    Audio,
    ResourceLink {
        uri: String,
        name: String,
        mime_type: Option<String>,
    },
    TextResource {
        uri: String,
        text: String,
    },
    BlobResource {
        uri: String,
        blob: String,
        mime_type: Option<String>,
    },
    /// A custom or future block type.
    Other(String),
}

/// A `session/set_config_option` value.
pub enum ConfigValue {
    Id(String),
    Bool(bool),
    /// A custom or future value type.
    Other(String),
}

/// The capabilities the client declared at `initialize`, in the terms the
/// adapter negotiates on.
#[derive(Default)]
pub struct ClientCapabilities {
    pub elicitation_form: bool,
    pub elicitation_url: bool,
    /// `type: "boolean"` config options: always in ACP v2.
    pub boolean_config: bool,
    /// `notice` session updates: always in ACP v2.
    pub notices: bool,
    /// Standard compaction updates: always in ACP v2.
    pub compaction: bool,
    pub terminal_auth: bool,
    /// The ACP subagent capability (ACP v1, unstable).
    pub subagents: bool,
    /// ACP v1 editor file and terminal tools.
    pub fs_read: bool,
    pub fs_write: bool,
    pub terminal: bool,
    /// `capabilities._meta`: JetBrains AIR and Muse opt-ins.
    pub meta: Meta,
}

/// A request from the client.
pub enum Request {
    Initialize(ClientCapabilities),
    /// ACP v1 `authenticate`.
    Authenticate {
        method_id: String,
    },
    /// ACP v2 `auth/login`.
    LoginAuth {
        method_id: String,
    },
    /// ACP v1 `logout` and v2 `auth/logout`.
    Logout,
    NewSession(Setup),
    LoadSession(Setup),
    ResumeSession(Setup),
    ForkSession(Setup),
    Prompt(Prompt),
    CloseSession {
        session_id: String,
    },
    DeleteSession {
        session_id: String,
        meta: Meta,
    },
    ListSessions {
        cwd: Option<String>,
        cursor: Option<String>,
    },
    SetConfigOption {
        session_id: String,
        config_id: String,
        value: ConfigValue,
    },
    /// ACP v1 `session/set_mode`.
    SetMode {
        session_id: String,
        mode_id: String,
    },
    /// A method no ACP schema defines: a `_`-prefixed extension, or one of
    /// the compatibility methods in `docs/acp-extensions.md`.
    Extension {
        method: String,
        params: Value,
    },
    /// A schema method the adapter does not serve.
    Unsupported {
        method: String,
    },
    /// The `session/cancel` notification.
    Cancel {
        session_id: String,
    },
    /// `$/cancel_request` for one of the client's requests. The SDK has
    /// already marked that request cancelled.
    CancelPending {
        request_id: RequestId,
    },
}

impl Request {
    /// The ACP method, for traces and errors.
    pub fn method(&self) -> &str {
        match self {
            Request::Initialize(_) => "initialize",
            Request::Authenticate { .. } => "authenticate",
            Request::LoginAuth { .. } => "auth/login",
            Request::Logout => "logout",
            Request::NewSession(_) => "session/new",
            Request::LoadSession(_) => "session/load",
            Request::ResumeSession(_) => "session/resume",
            Request::ForkSession(_) => "session/fork",
            Request::Prompt(_) => "session/prompt",
            Request::CloseSession { .. } => "session/close",
            Request::DeleteSession { .. } => "session/delete",
            Request::ListSessions { .. } => "session/list",
            Request::SetConfigOption { .. } => "session/set_config_option",
            Request::SetMode { .. } => "session/set_mode",
            Request::Extension { method, .. } | Request::Unsupported { method } => method,
            Request::Cancel { .. } => "session/cancel",
            Request::CancelPending { .. } => "$/cancel_request",
        }
    }
}

fn meta(meta: Option<Map<String, Value>>) -> Meta {
    meta.unwrap_or_default()
}

fn path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A typed v1 message, with its request id. `None` for a notification the
/// adapter does not act on.
pub fn v1(message: V1Message) -> Option<(Option<RequestId>, Request)> {
    use v1::schema::{ClientNotification as N, ClientRequest as R};
    match message {
        V1Message::Notification(N::CancelNotification(cancel)) => Some((
            None,
            Request::Cancel {
                session_id: cancel.session_id.to_string(),
            },
        )),
        // An extension notification runs its method as the request does,
        // without an answer, as before the SDK.
        V1Message::Notification(N::ExtNotification(ext)) => Some((
            None,
            Request::Extension {
                params: serde_json::from_str(ext.params.get()).unwrap_or(Value::Null),
                method: ext.method.to_string(),
            },
        )),
        V1Message::Notification(_) => None,
        V1Message::Request(id, request) => Some((
            Some(id),
            match *request {
                R::InitializeRequest(init) => {
                    Request::Initialize(v1_capabilities(init.client_capabilities))
                }
                R::AuthenticateRequest(auth) => Request::Authenticate {
                    method_id: auth.method_id.to_string(),
                },
                R::LogoutRequest(_) => Request::Logout,
                R::NewSessionRequest(r) => Request::NewSession(Setup {
                    session_id: String::new(),
                    cwd: path(&r.cwd),
                    additional_directories: r
                        .additional_directories
                        .iter()
                        .map(|p| path(p))
                        .collect(),
                    mcp_servers: r.mcp_servers.into_iter().map(v1_mcp).collect(),
                    replay_from: None,
                    meta: meta(r.meta),
                }),
                R::LoadSessionRequest(r) => Request::LoadSession(v1_load(r)),
                R::ResumeSessionRequest(r) => Request::ResumeSession(Setup {
                    session_id: r.session_id.to_string(),
                    cwd: path(&r.cwd),
                    additional_directories: r
                        .additional_directories
                        .iter()
                        .map(|p| path(p))
                        .collect(),
                    mcp_servers: r.mcp_servers.into_iter().map(v1_mcp).collect(),
                    replay_from: None,
                    meta: meta(r.meta),
                }),
                R::ForkSessionRequest(r) => Request::ForkSession(Setup {
                    session_id: r.session_id.to_string(),
                    cwd: path(&r.cwd),
                    additional_directories: r
                        .additional_directories
                        .iter()
                        .map(|p| path(p))
                        .collect(),
                    mcp_servers: r.mcp_servers.into_iter().map(v1_mcp).collect(),
                    replay_from: None,
                    meta: meta(r.meta),
                }),
                R::PromptRequest(r) => Request::Prompt(Prompt {
                    session_id: r.session_id.to_string(),
                    blocks: r.prompt.iter().map(v1_block).collect(),
                    meta: meta(r.meta),
                }),
                R::CloseSessionRequest(r) => Request::CloseSession {
                    session_id: r.session_id.to_string(),
                },
                R::DeleteSessionRequest(r) => Request::DeleteSession {
                    session_id: r.session_id.to_string(),
                    meta: meta(r.meta),
                },
                R::ListSessionsRequest(r) => Request::ListSessions {
                    cwd: r.cwd.as_deref().map(path),
                    cursor: r.cursor,
                },
                R::SetSessionConfigOptionRequest(r) => Request::SetConfigOption {
                    session_id: r.session_id.to_string(),
                    config_id: r.config_id.to_string(),
                    value: match r.value {
                        v1::schema::SessionConfigOptionValue::ValueId { value } => {
                            ConfigValue::Id(value.to_string())
                        }
                        v1::schema::SessionConfigOptionValue::Boolean { value } => {
                            ConfigValue::Bool(value)
                        }
                        _ => ConfigValue::Other("unknown".to_string()),
                    },
                },
                R::SetSessionModeRequest(r) => Request::SetMode {
                    session_id: r.session_id.to_string(),
                    mode_id: r.mode_id.to_string(),
                },
                R::ExtMethodRequest(ext) => Request::Extension {
                    params: serde_json::from_str(ext.params.get()).unwrap_or(Value::Null),
                    method: ext.method.to_string(),
                },
                other => Request::Unsupported {
                    method: other.method().to_string(),
                },
            },
        )),
    }
}

fn v1_load(r: v1::schema::LoadSessionRequest) -> Setup {
    Setup {
        session_id: r.session_id.to_string(),
        cwd: path(&r.cwd),
        additional_directories: r.additional_directories.iter().map(|p| path(p)).collect(),
        mcp_servers: r.mcp_servers.into_iter().map(v1_mcp).collect(),
        replay_from: None,
        meta: meta(r.meta),
    }
}

fn v1_capabilities(c: v1::schema::ClientCapabilities) -> ClientCapabilities {
    let session = c.session.unwrap_or_default();
    ClientCapabilities {
        elicitation_form: c.elicitation.as_ref().is_some_and(|e| e.form.is_some()),
        elicitation_url: c.elicitation.as_ref().is_some_and(|e| e.url.is_some()),
        boolean_config: session
            .config_options
            .as_ref()
            .is_some_and(|options| options.boolean.is_some()),
        notices: session.notices.is_some(),
        compaction: session.compaction.is_some(),
        terminal_auth: c.auth.terminal,
        subagents: c.subagents.is_some(),
        fs_read: c.fs.read_text_file,
        fs_write: c.fs.write_text_file,
        terminal: c.terminal,
        meta: meta(c.meta),
    }
}

pub(crate) fn v1_mcp(server: v1::schema::McpServer) -> McpServer {
    use v1::schema::McpServer as S;
    match server {
        S::Stdio(s) => McpServer::Stdio {
            name: s.name,
            command: path(&s.command),
            args: s.args,
            env: s.env.into_iter().map(|e| (e.name, e.value)).collect(),
        },
        S::Http(h) => McpServer::Http {
            name: h.name,
            url: h.url,
            headers: h.headers.into_iter().map(|h| (h.name, h.value)).collect(),
        },
        S::Sse(s) => McpServer::Sse { name: s.name },
        _ => McpServer::Other {
            kind: "unknown".to_string(),
            name: None,
        },
    }
}

fn v1_block(block: &v1::schema::ContentBlock) -> Block {
    use v1::schema::{ContentBlock as C, EmbeddedResourceResource as E};
    let content = match block {
        C::Text(t) => Content::Text(t.text.clone()),
        C::Image(i) => Content::Image {
            data: i.data.clone(),
            mime_type: i.mime_type.clone(),
        },
        C::Audio(_) => Content::Audio,
        C::ResourceLink(l) => Content::ResourceLink {
            uri: l.uri.clone(),
            name: l.name.clone(),
            mime_type: l.mime_type.clone(),
        },
        C::Resource(r) => match &r.resource {
            E::TextResourceContents(t) => Content::TextResource {
                uri: t.uri.clone(),
                text: t.text.clone(),
            },
            E::BlobResourceContents(b) => Content::BlobResource {
                uri: b.uri.clone(),
                blob: b.blob.clone(),
                mime_type: b.mime_type.clone(),
            },
            _ => Content::Other("resource".to_string()),
        },
        _ => Content::Other("unknown".to_string()),
    };
    Block {
        content,
        echo: crate::out::EchoBlock::V1(block.clone()),
    }
}

/// A typed v2 message, with its request id. `None` for a notification the
/// adapter does not act on.
pub fn v2(message: V2Message) -> Option<(Option<RequestId>, Request)> {
    use v2::schema::{ClientNotification as N, ClientRequest as R};
    match message {
        V2Message::Notification(N::CancelSessionNotification(cancel)) => Some((
            None,
            Request::Cancel {
                session_id: cancel.session_id.to_string(),
            },
        )),
        V2Message::Notification(N::ExtNotification(ext)) => Some((
            None,
            Request::Extension {
                params: serde_json::from_str(ext.params.get()).unwrap_or(Value::Null),
                method: ext.method.to_string(),
            },
        )),
        V2Message::Notification(_) => None,
        V2Message::Request(id, request) => Some((
            Some(id),
            match *request {
                R::InitializeRequest(init) => {
                    Request::Initialize(v2_capabilities(init.capabilities))
                }
                R::LoginAuthRequest(auth) => Request::LoginAuth {
                    method_id: auth.method_id.to_string(),
                },
                R::LogoutAuthRequest(_) => Request::Logout,
                R::NewSessionRequest(r) => Request::NewSession(Setup {
                    session_id: String::new(),
                    cwd: path(&r.cwd.0),
                    additional_directories: r
                        .additional_directories
                        .iter()
                        .map(|p| path(&p.0))
                        .collect(),
                    mcp_servers: r.mcp_servers.into_iter().map(v2_mcp).collect(),
                    replay_from: None,
                    meta: meta(r.meta),
                }),
                R::ResumeSessionRequest(r) => Request::ResumeSession(Setup {
                    session_id: r.session_id.to_string(),
                    cwd: path(&r.cwd.0),
                    additional_directories: r
                        .additional_directories
                        .iter()
                        .map(|p| path(&p.0))
                        .collect(),
                    mcp_servers: r.mcp_servers.into_iter().map(v2_mcp).collect(),
                    replay_from: r.replay_from.map(v2_replay),
                    meta: meta(r.meta),
                }),
                R::ForkSessionRequest(r) => Request::ForkSession(Setup {
                    session_id: r.session_id.to_string(),
                    cwd: path(&r.cwd.0),
                    additional_directories: r
                        .additional_directories
                        .iter()
                        .map(|p| path(&p.0))
                        .collect(),
                    mcp_servers: r.mcp_servers.into_iter().map(v2_mcp).collect(),
                    replay_from: None,
                    meta: meta(r.meta),
                }),
                R::PromptRequest(r) => Request::Prompt(Prompt {
                    session_id: r.session_id.to_string(),
                    blocks: r.prompt.iter().map(v2_block).collect(),
                    meta: meta(r.meta),
                }),
                R::CloseSessionRequest(r) => Request::CloseSession {
                    session_id: r.session_id.to_string(),
                },
                R::DeleteSessionRequest(r) => Request::DeleteSession {
                    session_id: r.session_id.to_string(),
                    meta: meta(r.meta),
                },
                R::ListSessionsRequest(r) => Request::ListSessions {
                    cwd: r.cwd.map(|cwd| path(&cwd.0)),
                    cursor: r.cursor.map(|cursor| cursor.to_string()),
                },
                R::SetSessionConfigOptionRequest(r) => Request::SetConfigOption {
                    session_id: r.session_id.to_string(),
                    config_id: r.config_id.to_string(),
                    value: match r.value {
                        v2::schema::SessionConfigOptionValue::Id { value } => {
                            ConfigValue::Id(value.to_string())
                        }
                        v2::schema::SessionConfigOptionValue::Boolean { value } => {
                            ConfigValue::Bool(value)
                        }
                        v2::schema::SessionConfigOptionValue::Other(other) => {
                            ConfigValue::Other(other.type_)
                        }
                        _ => ConfigValue::Other("unknown".to_string()),
                    },
                },
                R::ExtMethodRequest(ext) => Request::Extension {
                    params: serde_json::from_str(ext.params.get()).unwrap_or(Value::Null),
                    method: ext.method.to_string(),
                },
                other => Request::Unsupported {
                    method: other.method().to_string(),
                },
            },
        )),
    }
}

fn v2_capabilities(c: v2::schema::ClientCapabilities) -> ClientCapabilities {
    ClientCapabilities {
        elicitation_form: c.elicitation.as_ref().is_some_and(|e| e.form.is_some()),
        elicitation_url: c.elicitation.as_ref().is_some_and(|e| e.url.is_some()),
        boolean_config: true,
        notices: true,
        compaction: true,
        terminal_auth: c.auth.as_ref().is_some_and(|auth| auth.terminal.is_some()),
        subagents: false,
        fs_read: false,
        fs_write: false,
        terminal: false,
        meta: meta(c.meta),
    }
}

fn v2_replay(replay: v2::schema::ReplayFrom) -> Replay {
    match replay {
        v2::schema::ReplayFrom::Start(_) => Replay::Start,
        v2::schema::ReplayFrom::Other(other) => Replay::Other(other.type_),
        _ => Replay::Other("unknown".to_string()),
    }
}

pub(crate) fn v2_mcp(server: v2::schema::McpServer) -> McpServer {
    use v2::schema::McpServer as S;
    match server {
        S::Stdio(s) => McpServer::Stdio {
            name: s.name,
            command: path(&s.command.0),
            args: s.args,
            env: s.env.into_iter().map(|e| (e.name, e.value)).collect(),
        },
        S::Http(h) => McpServer::Http {
            name: h.name,
            url: h.url,
            headers: h.headers.into_iter().map(|h| (h.name, h.value)).collect(),
        },
        S::Other(other) => McpServer::Other {
            name: other
                .fields
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
            kind: other.type_,
        },
        _ => McpServer::Other {
            kind: "unknown".to_string(),
            name: None,
        },
    }
}

fn v2_block(block: &v2::schema::ContentBlock) -> Block {
    use v2::schema::{ContentBlock as C, EmbeddedResourceResource as E};
    let content = match block {
        C::Text(t) => Content::Text(t.text.clone()),
        C::Image(i) => Content::Image {
            data: i.data.clone(),
            mime_type: i.mime_type.to_string(),
        },
        C::Audio(_) => Content::Audio,
        C::ResourceLink(l) => Content::ResourceLink {
            uri: l.uri.clone(),
            name: l.name.clone(),
            mime_type: l.mime_type.as_ref().map(ToString::to_string),
        },
        C::Resource(r) => match &r.resource {
            E::TextResourceContents(t) => Content::TextResource {
                uri: t.uri.clone(),
                text: t.text.clone(),
            },
            E::BlobResourceContents(b) => Content::BlobResource {
                uri: b.uri.clone(),
                blob: b.blob.clone(),
                mime_type: b.mime_type.as_ref().map(ToString::to_string),
            },
            _ => Content::Other("resource".to_string()),
        },
        C::Other(other) => Content::Other(other.type_.clone()),
        _ => Content::Other("unknown".to_string()),
    };
    Block {
        content,
        echo: crate::out::EchoBlock::V2(block.clone()),
    }
}

/// Prompt content an extension method carries (`_session/steering`), read
/// with the negotiated version's schema type, as `session/prompt` is.
pub fn prompt_blocks(ver: u8, prompt: Option<&Value>) -> Result<Vec<Block>, String> {
    let prompt = prompt.ok_or("steering requires a prompt array")?;
    let invalid = |error: serde_json::Error| format!("invalid prompt content: {error}");
    if ver == 2 {
        let blocks: Vec<v2::schema::ContentBlock> =
            serde_json::from_value(prompt.clone()).map_err(invalid)?;
        Ok(blocks.iter().map(v2_block).collect())
    } else {
        let blocks: Vec<v1::schema::ContentBlock> =
            serde_json::from_value(prompt.clone()).map_err(invalid)?;
        Ok(blocks.iter().map(v1_block).collect())
    }
}

/// The client's answer to a request the adapter sent, read from the
/// version's SDK response type.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// `session/request_permission`.
    Permission(Permission),
    /// `elicitation/create`.
    Form(FormAnswer),
}

/// A `session/request_permission` outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    /// The user chose the option with this id.
    Selected(String),
    /// The prompt was cancelled (the turn was cancelled).
    Cancelled,
    /// An outcome this adapter does not know, by name.
    Other(String),
}

/// An `elicitation/create` action.
#[derive(Debug, Clone, PartialEq)]
pub enum FormAnswer {
    /// The user submitted the form; for a URL elicitation, agreed to open it.
    Accept(Fields),
    Decline,
    Cancel,
    /// An action this adapter does not know.
    Other,
}

impl FormAnswer {
    pub fn accepted(&self) -> Option<&Fields> {
        match self {
            FormAnswer::Accept(fields) => Some(fields),
            _ => None,
        }
    }
}

/// A submitted form's values, by field key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fields(pub std::collections::BTreeMap<String, FieldValue>);

/// One submitted form value.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Integer(i64),
    Number(f64),
    Flag(bool),
    List(Vec<String>),
    Other,
}

impl Fields {
    pub fn get(&self, key: &str) -> Option<&FieldValue> {
        self.0.get(key)
    }

    /// A text value; `None` when absent or not text.
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(FieldValue::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// Whether a boolean field was set to true.
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.0.get(key), Some(FieldValue::Flag(true)))
    }
}

macro_rules! permission_answer {
    ($response:expr, $s:path) => {{
        use $s as s;
        #[allow(unreachable_patterns)]
        match $response.outcome {
            s::RequestPermissionOutcome::Selected(selected) => {
                Permission::Selected(selected.option_id.to_string())
            }
            s::RequestPermissionOutcome::Cancelled { .. } => Permission::Cancelled,
            other => Permission::Other(
                serde_json::to_value(&other)
                    .ok()
                    .and_then(|v| v.get("outcome").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default(),
            ),
        }
    }};
}

macro_rules! form_answer {
    ($response:expr, $s:path) => {{
        use $s as s;
        #[allow(unreachable_patterns)]
        match $response.action {
            s::ElicitationAction::Accept(accept) => FormAnswer::Accept(Fields(
                accept
                    .content
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(key, value)| {
                        let value = match value {
                            s::ElicitationContentValue::String(text) => FieldValue::Text(text),
                            s::ElicitationContentValue::Integer(n) => FieldValue::Integer(n),
                            s::ElicitationContentValue::Number(n) => FieldValue::Number(n),
                            s::ElicitationContentValue::Boolean(b) => FieldValue::Flag(b),
                            s::ElicitationContentValue::StringArray(list) => FieldValue::List(list),
                            _ => FieldValue::Other,
                        };
                        (key, value)
                    })
                    .collect(),
            )),
            s::ElicitationAction::Decline => FormAnswer::Decline,
            s::ElicitationAction::Cancel => FormAnswer::Cancel,
            _ => FormAnswer::Other,
        }
    }};
}

impl From<v1::schema::RequestPermissionResponse> for Answer {
    fn from(response: v1::schema::RequestPermissionResponse) -> Answer {
        Answer::Permission(permission_answer!(response, v1::schema))
    }
}

impl From<v2::schema::RequestPermissionResponse> for Answer {
    fn from(response: v2::schema::RequestPermissionResponse) -> Answer {
        Answer::Permission(permission_answer!(response, v2::schema))
    }
}

impl From<v1::schema::CreateElicitationResponse> for Answer {
    fn from(response: v1::schema::CreateElicitationResponse) -> Answer {
        Answer::Form(form_answer!(response, v1::schema))
    }
}

impl From<v2::schema::CreateElicitationResponse> for Answer {
    fn from(response: v2::schema::CreateElicitationResponse) -> Answer {
        Answer::Form(form_answer!(response, v2::schema))
    }
}

#[cfg(test)]
mod answer_tests {
    use super::*;
    use serde_json::json;

    fn v2_permission(result: Value) -> Answer {
        serde_json::from_value::<v2::schema::RequestPermissionResponse>(result)
            .expect("v2 permission response")
            .into()
    }

    #[test]
    fn permission_outcomes_read_from_the_sdk_types() {
        let v1: Answer = serde_json::from_value::<v1::schema::RequestPermissionResponse>(
            json!({"outcome": {"outcome": "selected", "optionId": "c-1"}}),
        )
        .expect("v1 permission response")
        .into();
        assert_eq!(v1, Answer::Permission(Permission::Selected("c-1".into())));
        assert_eq!(
            v2_permission(json!({"outcome": {"outcome": "cancelled"}})),
            Answer::Permission(Permission::Cancelled)
        );
        assert_eq!(
            v2_permission(json!({"outcome": {"outcome": "_vendor_later"}})),
            Answer::Permission(Permission::Other("_vendor_later".into()))
        );
    }

    #[test]
    fn form_answers_keep_each_value_type() {
        let answer: Answer =
            serde_json::from_value::<v1::schema::CreateElicitationResponse>(json!({
                "action": "accept",
                "content": {"note": "hi", "withFiles": true, "q0": ["a", "b"], "n": 3}
            }))
            .expect("v1 elicitation response")
            .into();
        let Answer::Form(FormAnswer::Accept(fields)) = answer else {
            panic!("accepted form");
        };
        assert_eq!(fields.text("note"), Some("hi"));
        assert!(fields.flag("withFiles"));
        assert_eq!(
            fields.get("q0"),
            Some(&FieldValue::List(vec!["a".into(), "b".into()]))
        );
        assert_eq!(fields.get("n"), Some(&FieldValue::Integer(3)));
        let declined: Answer = serde_json::from_value::<v2::schema::CreateElicitationResponse>(
            json!({"action": "decline"}),
        )
        .expect("v2 elicitation response")
        .into();
        assert_eq!(declined, Answer::Form(FormAnswer::Decline));
    }
}
