//! A complete ACP agent in one file: it opens sessions and answers each
//! prompt by streaming the prompt's text back.
//!
//! Run it as an ACP stdio agent, for example from an editor's agent settings:
//!
//!     cargo run -p acp-lite --example echo_agent

use acp_lite::{AgentConnection, AgentHandler, AgentOptions, ClientRequest, RpcError, serve_stdio};
use serde_json::{Value, json};

#[derive(Default)]
struct Echo {
    sessions: u64,
}

impl AgentHandler for Echo {
    fn handle_request(
        &mut self,
        connection: &AgentConnection,
        request: ClientRequest,
    ) -> Result<Value, RpcError> {
        match request.method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": 1,
                "agentCapabilities": {"promptCapabilities": {}},
                "agentInfo": {"name": "echo-agent", "version": env!("CARGO_PKG_VERSION")},
                "authMethods": [],
            })),
            "session/new" => {
                self.sessions += 1;
                Ok(json!({"sessionId": format!("echo-{}", self.sessions)}))
            }
            "session/prompt" => {
                let session_id = request.params["sessionId"].clone();
                let blocks = request.params["prompt"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for block in blocks.iter().filter(|block| block["type"] == "text") {
                    connection
                        .notify(
                            "session/update",
                            json!({"sessionId": session_id, "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": {"type": "text", "text": block["text"]},
                            }}),
                        )
                        .map_err(|error| RpcError::new(-32603, error.to_string()))?;
                }
                Ok(json!({"stopReason": "end_turn"}))
            }
            other => Err(RpcError::new(-32601, format!("method not found: {other}"))),
        }
    }
}

fn main() -> std::io::Result<()> {
    serve_stdio(AgentOptions::default(), Echo::default())
}
