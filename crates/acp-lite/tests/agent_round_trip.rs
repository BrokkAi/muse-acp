//! The crate's client and agent sides talking to each other over in-process
//! pipes: a full prompt turn with a streamed update and a permission request
//! the agent sends back to the client.

use std::io::{self, BufReader};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use acp_lite::{
    AgentConnection, AgentEvent, AgentOptions, Client, ClientEvent, ClientOptions, ContentBlock,
    IncomingRequest, InitializeRequest, NewSessionRequest, PromptRequest, RequestContext,
    RequestOutcome, RpcError,
};
use serde_json::{Value, json};

/// A tiny agent on the agent side of the crate: one session, and a prompt
/// that streams a chunk, asks permission, and ends the turn.
fn run_agent(reader: io::PipeReader, writer: io::PipeWriter) -> thread::JoinHandle<()> {
    let (connection, events) =
        AgentConnection::channel(BufReader::new(reader), writer, AgentOptions::default()).unwrap();
    thread::spawn(move || {
        for event in events {
            match event {
                AgentEvent::Request(request) => {
                    let id = request.id.clone();
                    let outcome = match request.method.as_str() {
                        "initialize" => Ok(json!({
                            "protocolVersion": 1,
                            "agentCapabilities": {},
                            "authMethods": [],
                        })),
                        "session/new" => Ok(json!({"sessionId": "s1"})),
                        "session/prompt" => {
                            connection
                                .notify(
                                    "session/update",
                                    json!({"sessionId": "s1", "update": {
                                        "sessionUpdate": "agent_message_chunk",
                                        "content": {"type": "text", "text": "working"},
                                    }}),
                                )
                                .unwrap();
                            let permission = connection
                                .request(
                                    "session/request_permission",
                                    json!({
                                        "sessionId": "s1",
                                        "toolCall": {"toolCallId": "call-1", "title": "rm -rf build"},
                                        "options": [
                                            {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                            {"optionId": "deny", "name": "Deny", "kind": "reject_once"},
                                        ],
                                    }),
                                )
                                .unwrap();
                            Ok(
                                json!({"stopReason": "end_turn", "_meta": {"permission": permission}}),
                            )
                        }
                        other => Err(RpcError::new(-32601, format!("no {other}"))),
                    };
                    match outcome {
                        Ok(result) => connection.respond(&id, result).unwrap(),
                        Err(error) => connection.respond_error(Some(&id), error).unwrap(),
                    }
                }
                AgentEvent::Closed { .. } => break,
                _ => {}
            }
        }
    })
}

#[test]
fn a_client_and_an_agent_complete_a_prompt_turn() {
    let (agent_reader, client_writer) = io::pipe().unwrap();
    let (client_reader, agent_writer) = io::pipe().unwrap();
    let agent = run_agent(agent_reader, agent_writer);

    let allow = Arc::new(|_: &RequestContext, request: IncomingRequest| {
        assert_eq!(request.method, "session/request_permission");
        assert_eq!(request.params["toolCall"]["title"], "rm -rf build");
        RequestOutcome::result(json!({"outcome": {"outcome": "selected", "optionId": "allow"}}))
    });
    let mut client = Client::connect(
        BufReader::new(client_reader),
        client_writer,
        allow,
        ClientOptions::default(),
    )
    .unwrap();

    let initialized = client.initialize(InitializeRequest::v1()).unwrap();
    assert_eq!(initialized.protocol_version.as_u64(), 1);
    let session = client.new_session(NewSessionRequest::new("/tmp")).unwrap();
    assert_eq!(session.session_id, "s1");
    let reply = client
        .prompt(PromptRequest::new(
            "s1",
            vec![ContentBlock::text("clean up")],
        ))
        .unwrap();
    assert_eq!(reply.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(
        reply.raw["_meta"]["permission"]["outcome"]["optionId"], "allow",
        "the agent got the client's permission answer"
    );
    let streamed = loop {
        match client.next_event_timeout(Duration::from_secs(5)).unwrap() {
            ClientEvent::SessionUpdate { update, .. } => break update,
            _ => continue,
        }
    };
    assert_eq!(streamed["content"]["text"], Value::from("working"));

    // An unknown method comes back as the agent's JSON-RPC error.
    let error = client.handle().request("nope", json!({})).unwrap_err();
    assert!(error.to_string().contains("no nope"), "{error}");

    client.close().unwrap();
    agent.join().unwrap();
}
