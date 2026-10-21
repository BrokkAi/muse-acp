use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The ACP protocol version requested or returned by `initialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProtocolVersion {
    V1,
    V2,
    Other(u64),
}

impl ProtocolVersion {
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
            Self::Other(value) => value,
        }
    }

    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        match value {
            1 => Self::V1,
            2 => Self::V2,
            other => Self::Other(other),
        }
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_u64().fmt(f)
    }
}

/// The client's identity in the ACP handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
}

impl ClientInfo {
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            version: version.into(),
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self::new("acp-lite", env!("CARGO_PKG_VERSION"))
    }
}

/// Parameters for the ACP `initialize` request.
#[derive(Debug, Clone)]
pub struct InitializeRequest {
    pub protocol_version: ProtocolVersion,
    pub client_info: ClientInfo,
    /// Advertised client capabilities. Use an empty object unless the supplied
    /// `ClientHandler` actually implements the corresponding methods.
    pub capabilities: Value,
    pub meta: Option<Value>,
}

impl InitializeRequest {
    #[must_use]
    pub fn v1() -> Self {
        Self::new(ProtocolVersion::V1)
    }

    #[must_use]
    pub fn v2() -> Self {
        Self::new(ProtocolVersion::V2)
    }

    #[must_use]
    pub fn new(protocol_version: ProtocolVersion) -> Self {
        Self {
            protocol_version,
            client_info: ClientInfo::default(),
            capabilities: Value::Object(Map::new()),
            meta: None,
        }
    }

    #[must_use]
    pub fn client_info(mut self, client_info: ClientInfo) -> Self {
        self.client_info = client_info;
        self
    }

    #[must_use]
    pub fn capabilities(mut self, capabilities: Value) -> Self {
        self.capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub(crate) fn to_params(&self) -> Value {
        let mut params = Map::new();
        params.insert(
            "protocolVersion".to_string(),
            Value::from(self.protocol_version.as_u64()),
        );
        let client_info_key = if self.protocol_version.as_u64() >= 2 {
            "info"
        } else {
            "clientInfo"
        };
        params.insert(
            client_info_key.to_string(),
            serde_json::to_value(&self.client_info).expect("ClientInfo serializes"),
        );
        let capability_key = match self.protocol_version {
            ProtocolVersion::V1 => "clientCapabilities",
            _ => "capabilities",
        };
        params.insert(capability_key.to_string(), self.capabilities.clone());
        if let Some(meta) = &self.meta {
            params.insert("_meta".to_string(), meta.clone());
        }
        Value::Object(params)
    }
}

/// The negotiated ACP session and agent facts returned by `initialize`.
#[derive(Debug, Clone)]
pub struct InitializeResponse {
    pub protocol_version: ProtocolVersion,
    pub agent_info: Option<Value>,
    pub agent_capabilities: Value,
    pub auth_methods: Vec<Value>,
    pub raw: Value,
}

impl InitializeResponse {
    pub(crate) fn from_value(raw: Value) -> Result<Self, String> {
        let version = raw
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| "missing numeric protocolVersion".to_string())?;
        Ok(Self {
            protocol_version: ProtocolVersion::from_u64(version),
            agent_info: raw.get("agentInfo").or_else(|| raw.get("info")).cloned(),
            agent_capabilities: raw
                .get("agentCapabilities")
                .or_else(|| raw.get("capabilities"))
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new())),
            auth_methods: raw
                .get("authMethods")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            raw,
        })
    }
}

/// Parameters for creating an ACP session.
#[derive(Debug, Clone)]
pub struct NewSessionRequest {
    pub cwd: PathBuf,
    pub mcp_servers: Vec<Value>,
    pub additional_directories: Vec<PathBuf>,
    pub meta: Option<Value>,
}

impl NewSessionRequest {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
            meta: None,
        }
    }

    #[must_use]
    pub fn mcp_server(mut self, server: Value) -> Self {
        self.mcp_servers.push(server);
        self
    }

    #[must_use]
    pub fn additional_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.additional_directories.push(directory.into());
        self
    }

    #[must_use]
    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub(crate) fn to_params(&self) -> Value {
        let mut params = Map::new();
        params.insert("cwd".to_string(), path_value(&self.cwd));
        if !self.mcp_servers.is_empty() {
            params.insert(
                "mcpServers".to_string(),
                Value::Array(self.mcp_servers.clone()),
            );
        }
        if !self.additional_directories.is_empty() {
            params.insert(
                "additionalDirectories".to_string(),
                Value::Array(
                    self.additional_directories
                        .iter()
                        .map(|path| path_value(path))
                        .collect(),
                ),
            );
        }
        if let Some(meta) = &self.meta {
            params.insert("_meta".to_string(), meta.clone());
        }
        Value::Object(params)
    }
}

/// One content block in an ACP prompt.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(String),
    Image {
        data: Option<String>,
        uri: Option<String>,
        mime_type: Option<String>,
    },
    ResourceLink {
        uri: String,
        name: Option<String>,
        mime_type: Option<String>,
        description: Option<String>,
        size: Option<u64>,
    },
    Resource(Value),
    Raw(Value),
}

impl ContentBlock {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub(crate) fn to_value(&self) -> Value {
        match self {
            Self::Text(text) => json!({"type": "text", "text": text}),
            Self::Image {
                data,
                uri,
                mime_type,
            } => {
                let mut block = Map::new();
                block.insert("type".to_string(), Value::from("image"));
                if let Some(data) = data {
                    block.insert("data".to_string(), Value::from(data.clone()));
                }
                if let Some(uri) = uri {
                    block.insert("uri".to_string(), Value::from(uri.clone()));
                }
                if let Some(mime_type) = mime_type {
                    block.insert("mimeType".to_string(), Value::from(mime_type.clone()));
                }
                Value::Object(block)
            }
            Self::ResourceLink {
                uri,
                name,
                mime_type,
                description,
                size,
            } => {
                let mut block = Map::new();
                block.insert("type".to_string(), Value::from("resource_link"));
                block.insert("uri".to_string(), Value::from(uri.clone()));
                insert_optional(&mut block, "name", name.as_ref().map(String::as_str));
                insert_optional(
                    &mut block,
                    "mimeType",
                    mime_type.as_ref().map(String::as_str),
                );
                insert_optional(
                    &mut block,
                    "description",
                    description.as_ref().map(String::as_str),
                );
                if let Some(size) = size {
                    block.insert("size".to_string(), Value::from(*size));
                }
                Value::Object(block)
            }
            Self::Resource(resource) => json!({"type": "resource", "resource": resource}),
            Self::Raw(value) => value.clone(),
        }
    }
}

/// Parameters for the ACP `session/prompt` request.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
    pub meta: Option<Value>,
}

impl PromptRequest {
    #[must_use]
    pub fn new(session_id: impl Into<String>, prompt: Vec<ContentBlock>) -> Self {
        Self {
            session_id: session_id.into(),
            prompt,
            meta: None,
        }
    }

    #[must_use]
    pub fn text(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::new(session_id, vec![ContentBlock::text(text)])
    }

    #[must_use]
    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub(crate) fn to_params(&self) -> Value {
        let mut params = Map::new();
        params.insert(
            "sessionId".to_string(),
            Value::from(self.session_id.clone()),
        );
        params.insert(
            "prompt".to_string(),
            Value::Array(self.prompt.iter().map(ContentBlock::to_value).collect()),
        );
        if let Some(meta) = &self.meta {
            params.insert("_meta".to_string(), meta.clone());
        }
        Value::Object(params)
    }
}

/// A JSON-RPC error returned by an ACP agent.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

fn path_value(path: &Path) -> Value {
    Value::from(path.to_string_lossy().into_owned())
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        map.insert(key.to_string(), Value::from(value));
    }
}
