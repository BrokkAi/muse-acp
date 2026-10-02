//! Real `muse serve` tests against a loopback model provider.
//!
//! Each test drives the adapter against a real Muse host whose model calls go
//! to `tests/fixtures/loopback_provider.py` on 127.0.0.1, which answers with
//! scripted Responses API streams. Every test gets a throwaway Muse config
//! with a dummy API key, its own home, and its own workspace, so no
//! credentials or network access are needed and nothing touches the
//! developer's Muse settings.
//!
//! Skipped unless `MUSE_ACP_LOOPBACK=1`. `MUSE_CLI` selects the Muse binary
//! (default `muse`) and `PYTHON` the interpreter (default `python3`). CI runs
//! this suite against each pinned Muse build.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(60);

fn enabled(test: &str) -> bool {
    if std::env::var("MUSE_ACP_LOOPBACK").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("skipped {test}: set MUSE_ACP_LOOPBACK=1 to run real muse serve tests");
    false
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn python() -> String {
    std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string())
}

/// A shell command Muse asks about before running.
fn shell_step() -> Value {
    json!({"tool": {"name": "bash", "arguments": {
        "command": "printf approved > approved.txt",
        "description": "Write the approval artifact",
    }}})
}

/// A loopback provider plus an isolated Muse config, home, and workspace.
struct Host {
    dir: PathBuf,
    provider: Child,
}

impl Host {
    /// Starts the provider with `script` (names to replies; see the fixture)
    /// and writes Muse settings that use it, optionally with a saved
    /// permission profile.
    fn start(script: Value, profile: Option<&str>) -> Host {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "muse-acp-loopback-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["config/muse", "home", "ws"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("script.json"), script.to_string()).unwrap();
        let mut provider = Command::new(python())
            .arg(fixture("loopback_provider.py"))
            .arg(dir.join("provider.log"))
            .arg(dir.join("script.json"))
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the loopback provider (is python3 on PATH?)");
        let mut port = String::new();
        BufReader::new(provider.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let port: u16 = port.trim().parse().expect("provider port");
        let mut settings = json!({
            "schema_version": 1,
            "model": "fake-model",
            "reasoning_effort": "none",
            "endpoint_transport": {"base_url": format!("http://127.0.0.1:{port}"), "auth": "bearer"},
        });
        if let Some(profile) = profile {
            settings["permissions"] = json!({"schema_version": 1, "default_profile": profile});
        }
        std::fs::write(dir.join("config/muse/settings.json"), settings.to_string()).unwrap();
        std::fs::write(
            dir.join("config/muse/auth.json"),
            json!({"schema_version": 1, "providers": {"meta": {"api_key": "test-dummy-key"}}})
                .to_string(),
        )
        .unwrap();
        Host { dir, provider }
    }

    fn workspace(&self) -> PathBuf {
        self.dir.join("ws")
    }

    fn settings(&self) -> String {
        std::fs::read_to_string(self.dir.join("config/muse/settings.json")).unwrap()
    }

    /// The replies the provider sent, one per model call.
    fn replies(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("provider.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["reply"].clone())
            .collect()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.provider.kill();
        let _ = self.provider.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

type Frames = Arc<(Mutex<Vec<Value>>, Condvar)>;

/// The adapter under test, launched as an editor would launch it.
struct Adapter {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Frames,
    next_id: u64,
    log: PathBuf,
}

impl Adapter {
    fn launch(host: &Host) -> Adapter {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let home = host.dir.join("home");
        let log = host.dir.join(format!(
            "adapter-{}.log",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut child = Command::new(env!("CARGO_BIN_EXE_muse-acp"))
            .env(
                "MUSE_CLI",
                std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string()),
            )
            .env("XDG_CONFIG_HOME", host.dir.join("config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("HOME", &home)
            .env_remove("MUSE_APPROVAL_MODE")
            .env_remove("MUSE_SERVE_ARGS")
            .current_dir(host.workspace())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .expect("spawn the adapter");
        let stdout = child.stdout.take().unwrap();
        let frames: Frames = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let writer = frames.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let (lock, ready) = &*writer;
                lock.lock().unwrap().push(frame);
                ready.notify_all();
            }
        });
        let mut adapter = Adapter {
            stdin: child.stdin.take(),
            child,
            frames,
            next_id: 0,
            log,
        };
        let id = adapter.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        );
        adapter.result(id);
        adapter
    }

    fn send(&mut self, frame: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{frame}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// The first frame, in arrival order, that matches.
    fn wait(&self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let (lock, ready) = &*self.frames;
        let deadline = Instant::now() + TIMEOUT;
        let mut frames = lock.lock().unwrap();
        loop {
            if let Some(frame) = frames.iter().find(|frame| matches(frame)) {
                return frame.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!(
                    "timed out waiting for {what}\nframes: {frames:#?}\nadapter log:\n{}",
                    std::fs::read_to_string(&self.log).unwrap_or_default()
                );
            }
            frames = ready.wait_timeout(frames, left).unwrap().0;
        }
    }

    fn result(&self, id: u64) -> Value {
        let response = self.wait(&format!("the response to request {id}"), |frame| {
            frame["id"] == id && frame.get("method").is_none()
        });
        assert!(
            response.get("error").is_none(),
            "request {id} failed: {response}"
        );
        response["result"].clone()
    }

    fn new_session(&mut self, host: &Host, mcp_servers: Value) -> String {
        let id = self.request(
            "session/new",
            json!({"cwd": host.workspace(), "mcpServers": mcp_servers}),
        );
        self.result(id)["sessionId"].as_str().unwrap().to_string()
    }

    fn prompt(&mut self, session: &str, text: &str) -> u64 {
        self.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
        )
    }

    fn updates(&self, kind: &str) -> Vec<Value> {
        self.frames
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|frame| {
                frame["method"] == "session/update"
                    && frame["params"]["update"]["sessionUpdate"] == kind
            })
            .map(|frame| frame["params"]["update"].clone())
            .collect()
    }

    fn text(&self, kind: &str) -> String {
        self.updates(kind)
            .iter()
            .filter_map(|update| update["content"]["text"].as_str().map(str::to_string))
            .collect()
    }

    fn permission(&self) -> Value {
        self.wait("a permission request", |frame| {
            frame["method"] == "session/request_permission"
        })
    }

    /// Answers a permission request with its option of the given kind.
    fn choose(&mut self, permission: &Value, kind: &str) {
        let option = permission["params"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|option| option["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} option in {permission}"))["optionId"]
            .clone();
        self.send(json!({
            "jsonrpc": "2.0",
            "id": permission["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": option}},
        }));
    }

    /// Closes stdin, as an editor does, and waits for a clean exit.
    fn finish(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "adapter exited with {status}");
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("adapter did not exit after stdin closed");
    }
}

#[test]
fn a_prompt_streams_the_model_reply_and_ends_the_turn() {
    if !enabled("a_prompt_streams_the_model_reply_and_ends_the_turn") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "say hello [[script:hello]]");
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        adapter
            .text("agent_message_chunk")
            .contains("loopback says hello"),
        "{:?}",
        adapter.updates("agent_message_chunk")
    );
    assert!(
        result["usage"]["totalTokens"].as_u64().unwrap_or(0) > 0,
        "{result}"
    );
    adapter.finish();
}

#[test]
fn an_approved_shell_command_runs_and_a_rejected_one_does_not() {
    if !enabled("an_approved_shell_command_runs_and_a_rejected_one_does_not") {
        return;
    }
    for (kind, runs) in [("allow_once", true), ("reject_once", false)] {
        let host = Host::start(json!({"shell": shell_step()}), None);
        let mut adapter = Adapter::launch(&host);
        let session = adapter.new_session(&host, json!([]));
        let id = adapter.prompt(&session, "write it [[script:shell]]");
        let permission = adapter.permission();
        assert_eq!(
            permission["params"]["toolCall"]["kind"], "execute",
            "{permission}"
        );
        adapter.choose(&permission, kind);
        let result = adapter.result(id);
        assert_eq!(result["stopReason"], "end_turn", "{kind}: {result}");
        let artifact = std::fs::read_to_string(host.workspace().join("approved.txt")).ok();
        assert_eq!(artifact.as_deref() == Some("approved"), runs, "{kind}");
        adapter.finish();
    }
}

#[test]
fn cancel_stops_a_streaming_turn() {
    if !enabled("cancel_stops_a_streaming_turn") {
        return;
    }
    let host = Host::start(
        json!({"slow": {"text": "thinking", "hold_ms": 30000}}),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "take your time [[script:slow]]");
    adapter.wait("the first streamed text", |frame| {
        frame["params"]["update"]["content"]["text"] == "thinking"
    });
    let started = Instant::now();
    adapter.send(
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}),
    );
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "cancel took {:?}",
        started.elapsed()
    );
    adapter.finish();
}

#[test]
fn a_restarted_adapter_loads_the_session_with_its_history() {
    if !enabled("a_restarted_adapter_loads_the_session_with_its_history") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut first = Adapter::launch(&host);
    let session = first.new_session(&host, json!([]));
    let id = first.prompt(&session, "remember this [[script:hello]]");
    first.result(id);
    first.finish();

    let mut second = Adapter::launch(&host);
    let id = second.request(
        "session/load",
        json!({"sessionId": session, "cwd": host.workspace(), "mcpServers": []}),
    );
    second.result(id);
    assert!(
        second.text("user_message_chunk").contains("remember this"),
        "{:?}",
        second.updates("user_message_chunk")
    );
    assert!(
        second
            .text("agent_message_chunk")
            .contains("loopback says hello"),
        "{:?}",
        second.updates("agent_message_chunk")
    );
    let id = second.prompt(&session, "and again [[script:hello]]");
    assert_eq!(second.result(id)["stopReason"], "end_turn");
    second.finish();
}

#[test]
fn an_editor_mcp_server_tool_asks_first_and_returns_its_output() {
    if !enabled("an_editor_mcp_server_tool_asks_first_and_returns_its_output") {
        return;
    }
    let host = Host::start(
        json!({"mcp": {"tool": {"name": "mcp__probe__secret_word", "arguments": {}}}}),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(
        &host,
        json!([{"name": "probe", "command": python(), "args": [fixture("mcp_probe.py")], "env": []}]),
    );
    let id = adapter.prompt(&session, "use the probe [[script:mcp]]");
    let permission = adapter.permission();
    assert_eq!(
        permission["params"]["toolCall"]["title"], "mcp__probe__secret_word",
        "{permission}"
    );
    adapter.choose(&permission, "allow_once");
    assert_eq!(adapter.result(id)["stopReason"], "end_turn");
    let output = adapter
        .updates("tool_call_update")
        .iter()
        .map(|update| update["content"].to_string())
        .collect::<String>();
    assert!(output.contains("pineapple"), "{output}");
    adapter.finish();
}

#[test]
fn a_saved_auto_review_profile_still_sends_approvals_to_the_editor() {
    if !enabled("a_saved_auto_review_profile_still_sends_approvals_to_the_editor") {
        return;
    }
    let host = Host::start(json!({"shell": shell_step()}), Some(":auto-review"));
    let saved = host.settings();
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "write it [[script:shell]]");
    let permission = adapter.permission();
    adapter.choose(&permission, "reject_once");
    assert_eq!(adapter.result(id)["stopReason"], "end_turn");
    adapter.finish();
    assert_eq!(host.settings(), saved, "saved settings must not change");
    assert!(
        host.replies()
            .iter()
            .any(|reply| reply["tool"]["name"] == "bash"),
        "the scripted shell call must have been served"
    );
}
