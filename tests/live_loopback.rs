//! Real `muse serve` tests against a loopback model provider.
//!
//! Each test drives the adapter against a real Muse host whose model calls go
//! to `tests/fixtures/loopback_provider.py` on 127.0.0.1, which answers with
//! scripted Responses API streams. Every test gets a throwaway Muse config
//! with a dummy API key, its own home, and its own workspace, and the adapter
//! runs with a cleared environment and Muse's self-update turned off, so no
//! credentials or network access are needed and nothing touches the
//! developer's Muse settings. A failing test keeps its directory and prints
//! the logs.
//!
//! Skipped unless `MUSE_ACP_LOOPBACK=1`. `MUSE_CLI` selects the Muse binary
//! (default: `muse` on `PATH`, else `~/.local/bin/muse`) and `PYTHON` the
//! interpreter (default `python3`). CI runs this suite against each pinned
//! Muse build.

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
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.provider.kill();
        let _ = self.provider.wait();
        if std::thread::panicking() {
            // Keep the evidence: adapter logs, the provider log, Muse's state.
            for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".log") {
                    let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
                    let tail = &text[text.len().saturating_sub(8000)..];
                    eprintln!("---- {name} (tail)\n{tail}");
                }
            }
            eprintln!("kept {} for inspection", self.dir.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The Muse binary to test, resolved with the real environment because the
/// adapter runs with a throwaway home: `MUSE_CLI` (a relative path is taken
/// from the working directory), else `muse` on `PATH`, else the installer's
/// `~/.local/bin/muse`.
fn muse_cli() -> PathBuf {
    let wanted = PathBuf::from(std::env::var_os("MUSE_CLI").unwrap_or_else(|| "muse".into()));
    if wanted.components().count() > 1 {
        return std::path::absolute(&wanted).unwrap();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin")))
        .map(|dir| dir.join(&wanted))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "{} is not on PATH or in ~/.local/bin; set MUSE_CLI",
                wanted.display()
            )
        })
}

/// Variables the adapter keeps from the real environment. Everything else,
/// such as a developer's model, proxy, or API key settings, is dropped.
const KEPT_ENV: [&str; 8] = [
    "PATH",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "WINDIR",
    "LANG",
    "LC_ALL",
];

#[derive(Default)]
struct Output {
    frames: Vec<Value>,
    closed: bool,
}

type Frames = Arc<(Mutex<Output>, Condvar)>;

/// The adapter under test, launched as an editor would launch it.
struct Adapter {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Frames,
    next_id: u64,
    log: PathBuf,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Adapter {
    fn launch(host: &Host) -> Adapter {
        Adapter::launch_with(host, json!({}))
    }

    /// Launches and initializes the adapter with the given ACP v1 client
    /// capabilities.
    fn launch_with(host: &Host, capabilities: Value) -> Adapter {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let home = host.dir.join("home");
        let log = host.dir.join(format!(
            "adapter-{}.log",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut command = Command::new(env!("CARGO_BIN_EXE_muse-acp"));
        command.env_clear();
        for name in KEPT_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command
            .env("MUSE_CLI", muse_cli())
            .env("MUSE_NO_AUTO_UPDATE", "1")
            .env("XDG_CONFIG_HOME", host.dir.join("config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("HOME", &home)
            .current_dir(host.workspace())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .expect("spawn the adapter");
        let stdout = child.stdout.take().unwrap();
        let frames: Frames = Arc::new((Mutex::new(Output::default()), Condvar::new()));
        let writer = frames.clone();
        std::thread::spawn(move || {
            let (lock, ready) = &*writer;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                lock.lock().unwrap().frames.push(frame);
                ready.notify_all();
            }
            lock.lock().unwrap().closed = true;
            ready.notify_all();
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
            json!({"protocolVersion": 1, "clientCapabilities": capabilities}),
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
        let mut output = lock.lock().unwrap();
        loop {
            if let Some(frame) = output.frames.iter().find(|frame| matches(frame)) {
                return frame.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if output.closed || left.is_zero() {
                let why = if output.closed {
                    "the adapter exited"
                } else {
                    "timed out"
                };
                panic!(
                    "{why} waiting for {what}\nframes: {:#?}\nadapter log:\n{}",
                    output.frames,
                    std::fs::read_to_string(&self.log).unwrap_or_default()
                );
            }
            output = ready.wait_timeout(output, left).unwrap().0;
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

    /// Prompts and returns the stop reason once the turn ends.
    fn turn(&mut self, session: &str, text: &str) -> String {
        let id = self.prompt(session, text);
        self.result(id)["stopReason"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Waits until the adapter's log contains `needle`.
    fn wait_log(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let log = std::fs::read_to_string(&self.log).unwrap_or_default();
            if log.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the adapter never logged {needle:?}:\n{log}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The process id of the adapter's `muse serve` child (Linux only).
    #[cfg(target_os = "linux")]
    fn host_pid(&self) -> u32 {
        let tasks = format!("/proc/{}/task", self.child.id());
        std::fs::read_dir(&tasks)
            .unwrap()
            .flatten()
            .map(|task| std::fs::read_to_string(task.path().join("children")).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .filter_map(|pid| pid.parse::<u32>().ok())
            .find(|pid| {
                std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg == b"serve"))
            })
            .expect("a muse serve child")
    }

    fn updates(&self, kind: &str) -> Vec<Value> {
        self.frames
            .0
            .lock()
            .unwrap()
            .frames
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
        let _ = self.child.wait();
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
    let started = Instant::now();
    let id = adapter.prompt(&session, "take your time [[script:slow]]");
    // The provider holds the stream open for 30 s after this text.
    adapter.wait("the first streamed text", |frame| {
        frame["params"]["update"]["content"]["text"] == "thinking"
    });
    adapter.send(
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}),
    );
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the turn must stop while the model is still streaming, not after {:?}",
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
}

#[test]
fn a_fork_continues_on_its_own() {
    if !enabled("a_fork_continues_on_its_own") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    assert_eq!(adapter.turn(&session, "first [[script:hello]]"), "end_turn");
    let id = adapter.request("session/fork", json!({"sessionId": session}));
    let fork = adapter.result(id)["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(fork, session);
    assert_eq!(
        adapter.turn(&fork, "in the fork [[script:hello]]"),
        "end_turn"
    );
    assert_eq!(
        adapter.turn(&session, "in the source [[script:hello]]"),
        "end_turn"
    );
    adapter.finish();
}

#[test]
fn compact_reaches_the_hosts_compaction() {
    if !enabled("compact_reaches_the_hosts_compaction") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    assert_eq!(
        adapter.turn(&session, "context [[script:hello]]"),
        "end_turn"
    );
    let id = adapter.prompt(&session, "/compact");
    let response = adapter.wait("the /compact response", |frame| {
        frame["id"] == id && frame.get("method").is_none()
    });
    // Muse may decline to compact so short a session; either way the
    // command reached its native compaction rather than the model.
    let compacted = response["result"]["stopReason"] == "end_turn";
    let declined = response["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("compaction_unavailable"));
    assert!(compacted || declined, "{response}");
    assert_eq!(adapter.turn(&session, "after [[script:hello]]"), "end_turn");
    adapter.finish();
}

#[test]
fn a_goal_runs_until_the_model_completes_it() {
    if !enabled("a_goal_runs_until_the_model_completes_it") {
        return;
    }
    let host = Host::start(
        json!({"goal": {"tool": {"name": "update_goal", "arguments": {"status": "complete"}}}}),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    // The goal's own turn completes it right away.
    assert_eq!(
        adapter.turn(&session, "/goal say hello once [[script:goal]]"),
        "end_turn"
    );
    adapter.finish();
}

#[test]
fn each_stage_of_a_piped_shell_command_asks_first() {
    if !enabled("each_stage_of_a_piped_shell_command_asks_first") {
        return;
    }
    let host = Host::start(
        json!({"pipe": {"tool": {"name": "bash", "arguments": {
            "command": "printf a | tr a b > piped.txt",
            "description": "Write the piped artifact",
        }}}}),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "pipe it [[script:pipe]]");
    let mut answered = Vec::new();
    // Each stage is its own permission request; answer them in turn.
    let result = loop {
        let next = adapter.wait("a permission request or the turn's end", |frame| {
            (frame["method"] == "session/request_permission" && !answered.contains(&frame["id"]))
                || (frame["id"] == id && frame.get("method").is_none())
        });
        if next.get("method").is_none() {
            break next;
        }
        answered.push(next["id"].clone());
        adapter.choose(&next, "allow_once");
    };
    assert_eq!(result["result"]["stopReason"], "end_turn", "{result}");
    assert!(!answered.is_empty(), "the pipeline must ask first");
    assert_eq!(
        std::fs::read_to_string(host.workspace().join("piped.txt"))
            .ok()
            .as_deref(),
        Some("b")
    );
    adapter.finish();
}

#[test]
fn todos_become_an_editor_plan() {
    if !enabled("todos_become_an_editor_plan") {
        return;
    }
    let host = Host::start(
        json!({"todos": {"tool": {"name": "write_todos", "arguments": {"todos": [
            {"text": "Write the parser", "status": "in_progress"},
            {"text": "Test the parser", "status": "pending"},
        ]}}}}),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    assert_eq!(
        adapter.turn(&session, "plan it [[script:todos]]"),
        "end_turn"
    );
    let plans = adapter.updates("plan");
    let plan = plans.last().expect("a plan update");
    let entries = plan["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{plan}");
    assert_eq!(entries[0]["content"], "Write the parser");
    assert_eq!(entries[0]["status"], "in_progress");
    assert_eq!(entries[1]["status"], "pending");
    adapter.finish();
}

#[test]
fn a_requested_file_change_report_lists_the_written_file() {
    if !enabled("a_requested_file_change_report_lists_the_written_file") {
        return;
    }
    let host = Host::start(
        json!({"write": {"tool": {"name": "write_file", "arguments": {
            "path": "src/lib.rs", "content": "written\n",
        }}}}),
        None,
    );
    let mut adapter = Adapter::launch_with(
        &host,
        json!({"_meta": {"jetbrains": {"air": {"version": 1, "capabilities": ["agentFileChangeReport"]}}}}),
    );
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.request(
        "session/prompt",
        json!({
            "sessionId": session,
            "prompt": [{"type": "text", "text": "write it [[script:write]]"}],
            "_meta": {"jetbrains": {"air": {"agentFileChangeReportRequest": {"version": 1, "requestId": "report-1"}}}},
        }),
    );
    assert_eq!(adapter.result(id)["stopReason"], "end_turn");
    let report = adapter.wait("the file change report", |frame| {
        frame.to_string().contains("\"agentFileChangeReport\":{")
    });
    assert!(report.to_string().contains("src/lib.rs"), "{report}");
    assert_eq!(
        std::fs::read_to_string(host.workspace().join("src/lib.rs")).unwrap(),
        "written\n"
    );
    adapter.finish();
}

#[cfg(target_os = "linux")]
#[test]
fn a_crashed_host_restarts_and_the_session_continues() {
    if !enabled("a_crashed_host_restarts_and_the_session_continues") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    assert_eq!(
        adapter.turn(&session, "before [[script:hello]]"),
        "end_turn"
    );
    let status = Command::new("kill")
        .args(["-9", &adapter.host_pid().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    adapter.wait_log("host-restarted attempt=1");
    assert_eq!(adapter.turn(&session, "after [[script:hello]]"), "end_turn");
    adapter.finish();
}
