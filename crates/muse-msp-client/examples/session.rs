//! Run one prompt through a real `muse serve` and print what the host
//! reports, without any ACP code:
//!
//!     cargo run -p muse-msp-client --example session -- <muse> <workspace> <prompt> [--approve]
//!
//! The host uses your normal Muse login and settings. Tool approvals are
//! declined unless `--approve` is given.

use std::error::Error;

use muse_msp_client::json::{J, esc, j_to_string};
use muse_msp_client::{
    Capabilities, ClientOptions, LaunchConfig, MspClient, MspEvent, Params, ServerRequestMode,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let approve = args.iter().any(|arg| arg == "--approve");
    let [muse, workspace, prompt] = match &args
        .iter()
        .filter(|arg| *arg != "--approve")
        .collect::<Vec<_>>()[..]
    {
        [muse, workspace, prompt] => [muse.as_str(), workspace.as_str(), prompt.as_str()],
        _ => return Err("usage: session <muse> <workspace> <prompt> [--approve]".into()),
    };

    let mut config = LaunchConfig::new(muse);
    config.current_dir(workspace).options(
        ClientOptions::default()
            .capabilities(Capabilities {
                // This program answers approvals but never questions.
                user_input_dialogs: Some(false),
                ..Capabilities::default()
            })
            .server_request("approval/request", ServerRequestMode::Acknowledge),
    );
    let (client, events) = MspClient::launch(config)?;
    let handshake = client.handshake().expect("launch completed the handshake");
    println!(
        "connected to {} {} (schema {:?}, {})",
        handshake.server_name.as_deref().unwrap_or("?"),
        handshake.server_version.as_deref().unwrap_or("?"),
        handshake.schema_version,
        handshake.fingerprint.as_deref().unwrap_or("no fingerprint")
    );

    let started = client.request(
        "session/start",
        Params::raw(format!(
            r#"{{"commandId":{},"workspaceRoot":{}}}"#,
            esc(&client.mint_command_id()),
            esc(workspace)
        )),
    )?;
    let session = text(&started, &["session", "sessionId"]).to_string();
    let turn = client.request(
        "turn/start",
        Params::raw(format!(
            r#"{{"commandId":{},"sessionId":{},"input":[{{"type":"text","text":{}}}]}}"#,
            esc(&client.mint_command_id()),
            esc(&session),
            esc(prompt)
        )),
    )?;
    let turn_id = text(&turn, &["turnId"]).to_string();

    for event in &events {
        match event {
            MspEvent::Notification { method, params } => {
                let item = params.get("item");
                if method == "item/completed"
                    && item.and_then(|item| item.get("kind")).and_then(J::as_str)
                        == Some("agentMessage")
                {
                    println!("muse: {}", text(&params, &["item", "text"]));
                } else if method == "turn/completed" && text(&params, &["turnId"]) == turn_id {
                    println!("turn {}", text(&params, &["terminal"]));
                    break;
                }
            }
            // Already acknowledged; the decision is a separate command.
            MspEvent::Request { method, params, .. } if method == "approval/request" => {
                let choice = choose(&params, approve)?;
                println!("approval for {}: {choice}", text(&params, &["toolName"]));
                client.request(
                    "approval/decide",
                    Params::raw(format!(
                        r#"{{"commandId":{},"sessionId":{},"approvalId":{},"requirementId":{},"choiceId":{}}}"#,
                        esc(&client.mint_command_id()),
                        esc(&session),
                        esc(text(&params, &["approvalId"])),
                        params
                            .get("currentRequirementId")
                            .map(j_to_string)
                            .unwrap_or_else(|| "null".into()),
                        esc(&choice)
                    )),
                )?;
            }
            MspEvent::Closed { reason } => {
                println!("host closed: {reason}");
                break;
            }
            _ => {}
        }
    }

    match client.close() {
        Some(exit) => println!("{exit}"),
        None => println!("muse serve did not report an exit"),
    }
    Ok(())
}

/// The first choice that approves, or the first that does not.
fn choose(params: &J, approve: bool) -> Result<String, Box<dyn Error>> {
    let Some(J::Arr(choices)) = params.get("availableChoices") else {
        return Err("approval/request without availableChoices".into());
    };
    choices
        .iter()
        .find(|choice| {
            let decision = choice.get("decision").and_then(J::as_str).unwrap_or("");
            decision.starts_with("approved") == approve
        })
        .and_then(|choice| choice.get("choiceId"))
        .and_then(J::as_str)
        .map(str::to_string)
        .ok_or_else(|| "no matching approval choice".into())
}

/// A nested string field, or an empty string.
fn text<'a>(value: &'a J, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .and_then(J::as_str)
        .unwrap_or("")
}
