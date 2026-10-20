use serde_json::{Value, json};

use crate::client::ClientHandle;
use crate::protocol::{ProtocolVersion, RpcError};

/// An agent-initiated ACP request.
#[derive(Debug, Clone)]
pub struct IncomingRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

/// Connection facts and operations available while handling a reverse request.
#[derive(Clone)]
pub struct RequestContext {
    client: ClientHandle,
    protocol_version: ProtocolVersion,
}

impl RequestContext {
    pub(crate) fn new(client: ClientHandle, protocol_version: ProtocolVersion) -> Self {
        Self {
            client,
            protocol_version,
        }
    }

    #[must_use]
    pub fn client(&self) -> &ClientHandle {
        &self.client
    }

    #[must_use]
    pub fn protocol_version(&self) -> ProtocolVersion {
        self.protocol_version
    }
}

/// The one response sent for an agent-initiated request.
#[derive(Debug, Clone)]
pub enum RequestOutcome {
    Result(Value),
    Error(RpcError),
}

impl RequestOutcome {
    #[must_use]
    pub fn result(value: Value) -> Self {
        Self::Result(value)
    }

    #[must_use]
    pub fn error(code: i64, message: impl Into<String>) -> Self {
        Self::Error(RpcError::new(code, message))
    }
}

/// Handles requests sent by the agent to this client.
///
/// The method runs on its own thread, so it may call
/// [`RequestContext::client`] to issue another ACP request before returning.
pub trait ClientHandler: Send + Sync + 'static {
    fn handle_request(&self, context: &RequestContext, request: IncomingRequest) -> RequestOutcome;
}

impl<F> ClientHandler for F
where
    F: Fn(&RequestContext, IncomingRequest) -> RequestOutcome + Send + Sync + 'static,
{
    fn handle_request(&self, context: &RequestContext, request: IncomingRequest) -> RequestOutcome {
        self(context, request)
    }
}

/// A safe default that grants no access and cancels interactive requests.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllHandler;

impl ClientHandler for DenyAllHandler {
    fn handle_request(
        &self,
        _context: &RequestContext,
        request: IncomingRequest,
    ) -> RequestOutcome {
        match request.method.as_str() {
            "session/request_permission" => {
                let rejection = request
                    .params
                    .get("options")
                    .and_then(Value::as_array)
                    .and_then(|options| {
                        options.iter().find_map(|option| {
                            let kind = option.get("kind").and_then(Value::as_str)?;
                            (kind == "reject_once" || kind == "reject_always")
                                .then(|| option.get("optionId").and_then(Value::as_str))
                                .flatten()
                        })
                    });
                if let Some(option_id) = rejection {
                    RequestOutcome::result(json!({
                        "outcome": {
                            "outcome": "selected",
                            "optionId": option_id
                        }
                    }))
                } else {
                    RequestOutcome::result(json!({"outcome": {"outcome": "cancelled"}}))
                }
            }
            "elicitation/create" => RequestOutcome::result(json!({"action": "decline"})),
            _ => RequestOutcome::error(
                -32601,
                format!("client does not implement {}", request.method),
            ),
        }
    }
}
