use rusqlite::Connection;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

const FAKE_MUSE: &str = r#"#!/usr/bin/env python3
import json
import os
import sys
import uuid


def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)


if len(sys.argv) > 1 and sys.argv[1] == "logout":
    print("fake logout", flush=True)
    sys.exit(0)


for line in sys.stdin:
    try:
        message = json.loads(line)
    except Exception:
        continue

    method = message.get("method")
    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "experimentalApi": False,
                "grantedCapabilities": [],
                "museHome": os.environ.get("MUSE_FAKE_HOME", "/tmp"),
                "platformFamily": "unix",
                "platformOs": "linux",
                "schema": {"version": 1, "fingerprint": "test"},
                "serverInfo": {"name": "fake", "version": "0"},
                "userAgent": "fake",
            },
        })
    elif method == "session/start":
        session_id = str(uuid.uuid4())
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "session": {
                    "sessionId": session_id,
                    "activeTurnId": None,
                    "createdAt": "2026-01-01T00:00:00Z",
                    "forkedFrom": None,
                    "modelId": "test-model",
                    "path": "",
                    "providerId": "test-provider",
                    "status": "idle",
                    "turnCount": 0,
                    "updatedAt": "2026-01-01T00:00:00Z",
                    "workspaceRoot": "/tmp",
                },
                "viewCursor": "cursor:start",
            },
        })
    elif method == "model/list":
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "models": [{
                    "providerId": "test-provider",
                    "profileId": "test-profile",
                    "modelId": "test-model",
                    "displayLabel": "Test model",
                    "isActive": True,
                    "isDefault": True,
                    "contextLimit": None,
                    "cost": None,
                    "description": None,
                    "outputLimit": None,
                    "releaseDate": None,
                }],
                "profileId": "test-profile",
                "providerId": "test-provider",
                "source": "static",
            },
        })
    elif method == "turn/start":
        turn_id = message["params"]["commandId"]
        reasoning_effort = message["params"].get("reasoningEffort")
        if reasoning_effort not in {
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "ultra",
        }:
            send({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {
                    "code": -32602,
                    "message": "reasoningEffort is mandatory",
                },
            })
            continue
        input_text = next(
            (
                part.get("text")
                for part in message["params"].get("input", [])
                if part.get("type") == "text"
            ),
            "",
        )
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "commandId": turn_id,
                "disposition": "started",
                "startedNewTurn": True,
                "status": "accepted",
                "turnId": turn_id,
            },
        })
        session_id = message["params"]["sessionId"]
        item_id = "agent-item"
        user_item_id = "user-item"
        send({
            "jsonrpc": "2.0",
            "method": "item/started",
            "params": {
                "sessionId": session_id,
                "viewCursor": "c-user-start",
                "item": {
                    "itemId": user_item_id,
                    "kind": "userMessage",
                    "commandId": turn_id,
                    "revision": 1,
                    "status": "completed",
                    "text": "hello",
                },
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "sessionId": session_id,
                "viewCursor": "c-user-complete",
                "item": {
                    "itemId": user_item_id,
                    "kind": "userMessage",
                    "commandId": turn_id,
                    "revision": 2,
                    "status": "completed",
                    "text": "hello",
                },
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "turn/started",
            "params": {
                "sessionId": session_id,
                "turnId": turn_id,
                "viewCursor": "c1",
            },
        })
        if input_text == "steer me":
            continue
        if input_text == "cancel me":
            continue
        send({
            "jsonrpc": "2.0",
            "method": "item/started",
            "params": {
                "sessionId": session_id,
                "viewCursor": "c2",
                "item": {
                "itemId": item_id,
                "kind": "agentMessage",
                "revision": 1,
                "status": "inProgress",
                "text": "",
                },
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "item/delta",
            "params": {
                "sessionId": session_id,
                "itemId": item_id,
                "delta": f"hello:{reasoning_effort}",
                "viewCursor": "c3",
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "sessionId": session_id,
                "viewCursor": "c4",
                "item": {
                    "itemId": item_id,
                    "kind": "agentMessage",
                    "revision": 2,
                    "status": "completed",
                    "text": f"hello:{reasoning_effort}",
                },
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "turn/completed",
            "params": {
                "sessionId": session_id,
                "turnId": turn_id,
                "commandId": turn_id,
                "terminal": "completed",
                "viewCursor": "c5",
            },
        })
    elif method == "turn/steer":
        params = message["params"]
        input_text = next(
            (
                part.get("text")
                for part in params.get("input", [])
                if part.get("type") == "text"
            ),
            "",
        )
        reasoning_effort = params.get("reasoningEffort")
        if not params.get("expectedTurnId"):
            send({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {
                    "code": -32602,
                    "message": "expectedTurnId is mandatory",
                },
            })
            continue
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "commandId": params["commandId"],
                "status": "accepted",
                "turnId": params["expectedTurnId"],
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "sessionId": params["sessionId"],
                "viewCursor": "c-steer-user",
                "item": {
                    "itemId": "steer-user-item",
                    "kind": "userMessage",
                    "commandId": params["commandId"],
                    "revision": 1,
                    "status": "completed",
                    "text": input_text,
                },
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "sessionId": params["sessionId"],
                "viewCursor": "c-steer-agent",
                "item": {
                    "itemId": "steer-agent-item",
                    "kind": "agentMessage",
                    "revision": 1,
                    "status": "completed",
                    "text": f"steered:{reasoning_effort}",
                },
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "turn/completed",
            "params": {
                "sessionId": params["sessionId"],
                "turnId": params["expectedTurnId"],
                "terminal": "completed",
                "viewCursor": "c-steer-complete",
            },
        })
    elif method == "turn/cancel":
        params = message["params"]
        cancel_command_id = str(uuid.uuid4())
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "commandId": cancel_command_id,
                "status": "accepted",
                "turnId": params["turnId"],
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "turn/completed",
            "params": {
                "sessionId": params["sessionId"],
                "turnId": params["turnId"],
                "commandId": params["turnId"],
                "terminal": "cancelled",
                "viewCursor": "c-cancelled",
            },
        })
    elif message.get("id") is not None:
        send({"jsonrpc": "2.0", "id": message["id"], "result": {}})
"#;

#[test]
fn acp_v2_streams_msp_turn_events() {
    run_prompt_stream_test(2);
}

#[test]
fn acp_v1_waits_for_turn_completion() {
    run_prompt_stream_test(1);
}

#[test]
fn acp_v2_cancels_running_msp_turn() {
    let script = std::env::temp_dir().join(format!("muse-fake-cancel-{}.py", std::process::id()));
    std::fs::write(&script, FAKE_MUSE).unwrap();
    make_executable(&script);

    let mut child = Command::new(env!("CARGO_BIN_EXE_muse-acp"))
        .env("MUSE_CLI", &script)
        .env_remove("MUSE_SERVE_EXTRA_ARGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = tx.send(line);
        }
    });

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 2,
                "info": {"name": "test", "version": "0"},
                "capabilities": {}
            }
        }),
    );
    assert_eq!(read_json(&rx)["result"]["protocolVersion"], 2);
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {"cwd": "/tmp"}}),
    );
    let session_id = read_json(&rx)["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "cancel me"}]
            }
        }),
    );

    let mut saw_response = false;
    let mut saw_running = false;
    for _ in 0..20 {
        let message = read_json(&rx);
        saw_response |= message.get("id") == Some(&json!(3)) && message.get("result").is_some();
        if message.get("method") == Some(&json!("session/update")) {
            let update = &message["params"]["update"];
            saw_running |=
                update["sessionUpdate"] == "state_update" && update["state"] == "running";
        }
        if saw_response && saw_running {
            break;
        }
    }
    assert!(saw_response);
    assert!(saw_running);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {"sessionId": session_id}
        }),
    );
    let mut cancelled = false;
    for _ in 0..20 {
        let message = read_json(&rx);
        if message.get("method") == Some(&json!("session/update")) {
            let update = &message["params"]["update"];
            cancelled |= update["sessionUpdate"] == "state_update"
                && update["state"] == "idle"
                && update["stopReason"] == "cancelled";
            if cancelled {
                break;
            }
        }
    }
    assert!(cancelled);

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_file(script);
}

#[test]
fn acp_v2_steers_running_msp_turn() {
    let script = std::env::temp_dir().join(format!("muse-fake-steer-{}.py", std::process::id()));
    std::fs::write(&script, FAKE_MUSE).unwrap();
    make_executable(&script);

    let mut child = Command::new(env!("CARGO_BIN_EXE_muse-acp"))
        .env("MUSE_CLI", &script)
        .env_remove("MUSE_SERVE_EXTRA_ARGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = tx.send(line);
        }
    });

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 2,
                "info": {"name": "test", "version": "0"},
                "capabilities": {}
            }
        }),
    );
    let initialize = read_json(&rx);
    assert_eq!(
        initialize["result"]["_meta"]["steering"]["supported"],
        json!(true)
    );

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {"cwd": "/tmp"}}),
    );
    let session_id = read_json(&rx)["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "session/set_config_option",
            "params": {
                "sessionId": session_id,
                "configId": "muse.reasoningEffort",
                "value": {"value": "high"}
            }
        }),
    );
    let _ = read_json(&rx);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "steer me"}]
            }
        }),
    );
    let mut running = false;
    for _ in 0..20 {
        let message = read_json(&rx);
        if message.get("method") == Some(&json!("session/update")) {
            let update = &message["params"]["update"];
            running |= update["sessionUpdate"] == "state_update" && update["state"] == "running";
            if running {
                break;
            }
        }
    }
    assert!(running, "initial turn did not enter running state");

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "_session/steering",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "change course"}],
                "_meta": {"steering": {"idleBehavior": "promptRequired"}}
            }
        }),
    );
    let mut steer_response = None;
    let mut steered_user_messages = 0;
    let mut saw_steered_output = false;
    let mut steer_idle = false;
    for _ in 0..30 {
        let message = read_json(&rx);
        if message.get("id") == Some(&json!(4)) && message.get("result").is_some() {
            steer_response = Some(message.clone());
        }
        if message.get("method").and_then(Value::as_str) != Some("session/update") {
            continue;
        }
        let update = &message["params"]["update"];
        if update["sessionUpdate"] == "user_message"
            && update["content"][0]["text"] == "change course"
        {
            steered_user_messages += 1;
        }
        saw_steered_output |= update["sessionUpdate"] == "agent_message"
            && update["content"][0]["text"] == "steered:high";
        steer_idle |= update["sessionUpdate"] == "state_update" && update["state"] == "idle";
        if steer_response.is_some() && saw_steered_output && steer_idle {
            break;
        }
    }
    let steer_response = steer_response.expect("steering request was not answered");
    assert_eq!(steer_response["result"]["outcome"], json!("injected"));
    assert_eq!(
        steered_user_messages, 1,
        "Muse steering echo must be deduplicated"
    );
    assert!(saw_steered_output);
    assert!(steer_idle);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "_session/steering",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "not consumed"}],
                "_meta": {"steering": {"idleBehavior": "promptRequired"}}
            }
        }),
    );
    let mut idle_response = None;
    for _ in 0..20 {
        let message = read_json(&rx);
        if message.get("id") == Some(&json!(5)) && message.get("result").is_some() {
            idle_response = Some(message);
            break;
        }
    }
    let idle_response = idle_response.expect("idle steering request was not answered");
    assert_eq!(idle_response["result"]["outcome"], json!("promptRequired"));
    assert_eq!(idle_response["result"]["reason"], json!("noRunningTurn"));

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "_session/steering",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "fallback turn"}]
            }
        }),
    );
    let mut fallback_response = None;
    for _ in 0..30 {
        let message = read_json(&rx);
        if message.get("id") == Some(&json!(6)) && message.get("result").is_some() {
            fallback_response = Some(message);
            break;
        }
    }
    assert_eq!(
        fallback_response.expect("fallback steering request was not answered")["result"]["outcome"],
        json!("startedNewTurn")
    );

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_file(script);
}

#[test]
fn acp_v2_authenticates_and_deletes_durable_sessions() {
    let fake_home = std::env::temp_dir().join(format!(
        "muse-fake-home-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let sessions = fake_home.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let script =
        std::env::temp_dir().join(format!("muse-fake-lifecycle-{}.py", std::process::id()));
    std::fs::write(&script, FAKE_MUSE).unwrap();
    make_executable(&script);

    let mut child = Command::new(env!("CARGO_BIN_EXE_muse-acp"))
        .env("MUSE_CLI", &script)
        .env("MUSE_FAKE_HOME", &fake_home)
        .env_remove("MUSE_SERVE_EXTRA_ARGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = tx.send(line);
        }
    });

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 2,
                "info": {"name": "test", "version": "0"},
                "capabilities": {"auth": {"terminal": true}, "_meta": {"terminal-auth": true}},
                "clientCapabilities": {"auth": {"terminal": true}, "_meta": {"terminal-auth": true}}
            }
        }),
    );
    let initialize = read_json(&rx);
    assert_eq!(initialize["result"]["protocolVersion"], 2);
    assert_eq!(initialize["result"]["agentInfo"]["name"], "muse-acp");
    assert_eq!(
        initialize["result"]["authMethods"][0]["methodId"],
        "muse-login"
    );
    assert_eq!(initialize["result"]["authMethods"][0]["type"], "terminal");
    assert_eq!(
        initialize["result"]["agentCapabilities"]["session"]["delete"],
        json!({})
    );

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "auth/login",
            "params": {"methodId": "muse-login"}
        }),
    );
    let delete_response = read_json(&rx);
    assert!(delete_response["result"] == json!({}), "{delete_response}");
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "auth/logout",
            "params": {}
        }),
    );
    assert_eq!(read_json(&rx)["result"], json!({}));

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 4, "method": "session/new", "params": {"cwd": "/tmp"}}),
    );
    let session_id = read_json(&rx)["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let session_dir = sessions.join(&session_id);
    let view_dir = sessions.join(".msp-view-v1").join(&session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::create_dir_all(&view_dir).unwrap();
    std::fs::write(session_dir.join("session.jsonl"), "{}\n").unwrap();
    let database = Connection::open(fake_home.join("session-index.db")).unwrap();
    database
        .execute_batch(
            r#"
            CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE sessions (
                session_id TEXT PRIMARY KEY,
                session_stream_id TEXT NOT NULL,
                session_dir TEXT NOT NULL,
                session_log_path TEXT NOT NULL UNIQUE,
                layout TEXT NOT NULL,
                title TEXT NOT NULL,
                search_text TEXT NOT NULL,
                status TEXT NOT NULL,
                status_rank INTEGER NOT NULL,
                indexed_at_us INTEGER NOT NULL,
                latest_segment_terminated INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO schema_meta VALUES ('schema_version', '1');
            "#,
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO sessions VALUES (?1, ?1, ?2, ?3, 'v1', 'test', 'test', 'idle', 0, 0, 0)",
            rusqlite::params![
                session_id,
                session_dir.to_string_lossy(),
                session_dir.join("session.jsonl").to_string_lossy()
            ],
        )
        .unwrap();
    drop(database);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/delete",
            "params": {"sessionId": session_id}
        }),
    );
    let delete_response = read_json(&rx);
    assert!(delete_response["result"] == json!({}), "{delete_response}");
    assert!(!session_dir.exists());
    assert!(!view_dir.exists());
    let database = Connection::open(fake_home.join("session-index.db")).unwrap();
    let count: i64 = database
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_file(script);
    let _ = std::fs::remove_dir_all(fake_home);
}

fn run_prompt_stream_test(protocol_version: u32) {
    let script = std::env::temp_dir().join(format!(
        "muse-fake-{protocol_version}-{}.py",
        std::process::id()
    ));
    std::fs::write(&script, FAKE_MUSE).unwrap();
    make_executable(&script);

    let mut child = Command::new(env!("CARGO_BIN_EXE_muse-acp"))
        .env("MUSE_CLI", &script)
        .env_remove("MUSE_SERVE_EXTRA_ARGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = tx.send(line);
        }
    });

    let initialize_params = if protocol_version == 1 {
        json!({
            "protocolVersion": 1,
            "clientInfo": {"name": "test", "version": "0"},
            "clientCapabilities": {}
        })
    } else {
        json!({
            "protocolVersion": 2,
            "info": {"name": "test", "version": "0"},
            "capabilities": {}
        })
    };
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": initialize_params
        }),
    );
    let initialize = read_json(&rx);
    assert_eq!(initialize["result"]["protocolVersion"], protocol_version);

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {"cwd": "/tmp"}}),
    );
    let new_session = read_json(&rx);
    let session_id = new_session["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let config_id_field = if protocol_version == 1 {
        "id"
    } else {
        "configId"
    };
    let configs = new_session["result"]["configOptions"].as_array().unwrap();
    let model = configs
        .iter()
        .find(|config| config[config_id_field] == "muse.model")
        .unwrap();
    let effort = configs
        .iter()
        .find(|config| config[config_id_field] == "muse.reasoningEffort")
        .unwrap();
    if protocol_version == 1 {
        assert_eq!(model["id"], "muse.model");
        assert!(model.get("configId").is_none());
        assert_eq!(effort["id"], "muse.reasoningEffort");
        assert!(effort.get("configId").is_none());
    } else {
        assert_eq!(model["configId"], "muse.model");
        assert!(model.get("id").is_none());
        assert_eq!(effort["configId"], "muse.reasoningEffort");
        assert!(effort.get("id").is_none());
    }
    assert_eq!(effort["currentValue"], "medium");
    assert_eq!(effort["category"], "thought_level");

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "session/set_config_option",
            "params": {
                "sessionId": session_id,
                "configId": "muse.reasoningEffort",
                "value": {"value": "high"}
            }
        }),
    );
    let set_effort = read_json(&rx);
    let updated_effort = set_effort["result"]["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|config| config[config_id_field] == "muse.reasoningEffort")
        .unwrap();
    assert_eq!(updated_effort["currentValue"], "high");

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "hello"}]
            }
        }),
    );

    let mut saw_user = false;
    let mut user_message_count = 0;
    let mut saw_chunk = false;
    let mut saw_selected_effort = false;
    let mut saw_idle = false;
    let mut first_update = None;
    let mut prompt_result = None;
    for _ in 0..30 {
        let message = read_json(&rx);
        if message.get("id") == Some(&json!(3)) && message.get("result").is_some() {
            prompt_result = Some(message.clone());
        }
        if saw_user && saw_chunk && saw_idle && prompt_result.is_some() {
            break;
        }
        if message.get("method").and_then(Value::as_str) != Some("session/update") {
            continue;
        }
        let update = &message["params"]["update"]["sessionUpdate"];
        if first_update.is_none() {
            first_update = Some(update.clone());
        }
        saw_user |= update == "user_message";
        user_message_count += usize::from(update == "user_message");
        saw_chunk |= update == "agent_message_chunk";
        saw_selected_effort |= update == "agent_message_chunk"
            && message["params"]["update"]["content"]["text"] == "hello:high";
        saw_idle |= update == "state_update" && message["params"]["update"]["state"] == "idle";
        if saw_user && saw_chunk && saw_idle && prompt_result.is_some() {
            break;
        }
    }

    assert_eq!(first_update, Some(json!("user_message")));
    assert!(saw_user);
    assert_eq!(user_message_count, 1);
    assert!(saw_chunk);
    assert!(saw_selected_effort);
    assert!(saw_idle);
    let prompt_result = prompt_result.expect("prompt request was not answered");
    let expected_stop_reason = if protocol_version == 1 {
        json!("end_turn")
    } else {
        Value::Null
    };
    assert_eq!(prompt_result["result"]["stopReason"], expected_stop_reason);

    drop(stdin);
    let _ = child.wait();
    let _ = std::fs::remove_file(script);
}

fn send(stdin: &mut impl Write, message: Value) {
    let mut line = serde_json::to_string(&message).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
}

fn read_json(rx: &mpsc::Receiver<String>) -> Value {
    let line = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("timed out waiting for an ACP message");
    serde_json::from_str(&line).unwrap()
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &std::path::Path) {}
