//! Black-box tests for the extracted `acp-lite` SDK.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use acp_lite::{
    AgentProcess, Client, ClientError, ClientEvent, ClientOptions, DenyAllHandler,
    InitializeRequest, NewSessionRequest, PromptRequest, ProtocolVersion,
};

fn fixture() -> String {
    let extension = if cfg!(windows) { "cmd" } else { "py" };
    let root = env!("CARGO_MANIFEST_DIR");
    format!("{root}/tests/fixtures/fake_serve.{extension}")
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "acp-lite-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    root
}

fn agent(root: &Path, scenario: &str) -> AgentProcess {
    let mut agent = AgentProcess::new(env!("CARGO_BIN_EXE_muse-acp"));
    agent
        .env("MUSE_CLI", fixture())
        .env("FAKE_SCENARIO", scenario)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .current_dir(root.join("workspace"));
    agent
}

fn connect(root: &Path, scenario: &str) -> Client {
    Client::spawn(
        agent(root, scenario),
        Arc::new(DenyAllHandler),
        ClientOptions::default()
            .request_timeout(Duration::from_secs(20))
            .shutdown_timeout(Duration::from_secs(3)),
    )
    .expect("spawn muse-acp")
}

fn assert_agent_message(client: &Client, deadline: Instant) {
    let mut message = String::new();
    while Instant::now() < deadline {
        match client.next_event_timeout(Duration::from_millis(100)) {
            Ok(ClientEvent::SessionUpdate { update, .. }) => {
                if update.get("sessionUpdate").and_then(|value| value.as_str())
                    == Some("agent_message_chunk")
                    && let Some(text) = update
                        .get("content")
                        .and_then(|content| content.get("text"))
                        .and_then(|text| text.as_str())
                {
                    message.push_str(text);
                    if message.contains("hello from fake host") {
                        return;
                    }
                }
            }
            Ok(_) | Err(ClientError::EventTimeout { .. }) => {}
            Err(error) => panic!("event stream failed: {error}"),
        }
    }
    assert!(
        message.contains("hello from fake host"),
        "agent message was not observed through the SDK: {message:?}"
    );
}

fn exercise_protocol(version: ProtocolVersion, label: &str) {
    let root = temp_root(label);
    let mut client = connect(&root, "happy");
    let initialize = match version {
        ProtocolVersion::V1 => InitializeRequest::v1(),
        ProtocolVersion::V2 => InitializeRequest::v2(),
        other => panic!("unexpected protocol version {other}"),
    };
    let initialized = client.initialize(initialize).expect("initialize");
    assert_eq!(initialized.protocol_version, version);
    assert_eq!(client.protocol_version(), Some(version));

    let session = client
        .new_session(NewSessionRequest::new(root.join("workspace")))
        .expect("new session");
    let prompted = client
        .prompt(PromptRequest::text(&session.session_id, "Say hello"))
        .expect("prompt");
    match version {
        ProtocolVersion::V2 => assert!(
            prompted
                .message_id
                .as_deref()
                .is_some_and(|id| !id.is_empty()),
            "v2 prompts are accepted with the inserted message id: {:?}",
            prompted.raw
        ),
        _ => assert_eq!(prompted.message_id, None, "{:?}", prompted.raw),
    }

    assert_agent_message(&client, Instant::now() + Duration::from_secs(3));
    client.close().expect("close client");
    std::fs::remove_dir_all(root).expect("remove test directory");
}

#[test]
fn drives_muse_acp_over_v1() {
    exercise_protocol(ProtocolVersion::V1, "v1");
}

#[test]
fn drives_muse_acp_over_v2() {
    exercise_protocol(ProtocolVersion::V2, "v2");
}
