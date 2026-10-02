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
        // On macOS Muse reads credentials from the Keychain, which waits for
        // an approval no headless run can give. Its file backend reads the
        // throwaway auth.json instead.
        if cfg!(target_os = "macos") {
            command.env("TBH_CREDENTIAL_BACKEND", "file");
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

    /// The response to request `id`, error or not.
    fn response(&self, id: u64) -> Value {
        self.wait(&format!("the response to request {id}"), |frame| {
            frame["id"] == id && frame.get("method").is_none()
        })
    }

    fn result(&self, id: u64) -> Value {
        let response = self.response(id);
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
        self.prompt_with(session, text, Value::Null)
    }

    fn prompt_with(&mut self, session: &str, text: &str, meta: Value) -> u64 {
        let mut params = json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]});
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        self.request("session/prompt", params)
    }

    /// Prompts and requires the turn to end normally.
    fn turn(&mut self, session: &str, text: &str) -> Value {
        let id = self.prompt(session, text);
        let result = self.result(id);
        assert_eq!(result["stopReason"], "end_turn", "{text}: {result}");
        result
    }

    /// Waits until the adapter's log contains `needle`.
    #[cfg(target_os = "linux")]
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
        let adapter = self.child.id();
        std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .find(|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                // The parent pid follows the state, after the command name.
                let parent = stat
                    .rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse::<u32>().ok());
                parent == Some(adapter)
                    && std::fs::read(format!("/proc/{pid}/cmdline"))
                        .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg == b"serve"))
            })
            .expect("a muse serve child of the adapter")
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

/// The replies the provider sent, one per model call.
fn replies(host: &Host) -> Vec<Value> {
    std::fs::read_to_string(host.dir.join("provider.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|call| call["reply"].clone())
        .collect()
}

#[test]
fn a_fork_starts_from_the_source_history_and_continues_on_its_own() {
    if !enabled("a_fork_starts_from_the_source_history_and_continues_on_its_own") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    adapter.turn(&session, "first [[script:hello]]");
    let id = adapter.request(
        "session/fork",
        json!({"sessionId": session, "cwd": host.workspace(), "mcpServers": []}),
    );
    let fork = adapter.result(id)["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(fork, session);
    adapter.wait("the fork's copy of the first prompt", |frame| {
        frame["params"]["sessionId"] == fork.as_str()
            && frame["params"]["update"]["sessionUpdate"] == "user_message_chunk"
            && frame["params"]["update"]["content"]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("first"))
    });
    adapter.turn(&fork, "in the fork [[script:hello]]");
    adapter.turn(&session, "in the source [[script:hello]]");
    adapter.finish();
}

#[test]
fn compact_on_a_short_session_ends_with_muses_reason() {
    if !enabled("compact_on_a_short_session_ends_with_muses_reason") {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "loopback says hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    adapter.turn(&session, "context [[script:hello]]");
    let calls = replies(&host).len();
    adapter.turn(&session, "/compact");
    // Every pinned build declines a session this short, and the command
    // goes to Muse's compaction, never to the model as text.
    assert!(
        adapter
            .text("agent_message_chunk")
            .contains("Muse did not compact this session ("),
        "{:?}",
        adapter.updates("agent_message_chunk")
    );
    assert!(
        !replies(&host)[calls..]
            .iter()
            .any(|reply| reply["text"] == "ok"),
        "/compact must not reach the model as a prompt"
    );
    adapter.turn(&session, "after [[script:hello]]");
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
    adapter.turn(&session, "/goal say hello once [[script:goal]]");
    assert!(
        replies(&host)
            .iter()
            .any(|reply| reply["tool"]["name"] == "update_goal"),
        "the goal's turn must have reached the model and completed the goal"
    );
    adapter.finish();
}

#[test]
fn a_piped_shell_command_asks_with_every_stage() {
    if !enabled("a_piped_shell_command_asks_with_every_stage") {
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
    // Answer every permission request until the turn ends.
    let result = loop {
        let next = adapter.wait("a permission request or the turn's end", |frame| {
            (frame["method"] == "session/request_permission" && !answered.contains(&frame["id"]))
                || (frame["id"] == id && frame.get("method").is_none())
        });
        if next.get("method").is_none() {
            break next;
        }
        answered.push(next["id"].clone());
        if answered.len() == 1 {
            let stages = next["params"]["toolCall"]["rawInput"]["stages"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(stages.len(), 2, "both stages reach the editor: {next}");
        }
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
    adapter.turn(&session, "plan it [[script:todos]]");
    let plan = adapter.wait("a plan update", |frame| {
        frame["params"]["update"]["sessionUpdate"] == "plan"
    });
    let entries = plan["params"]["update"]["entries"].as_array().unwrap();
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
    let id = adapter.prompt_with(
        &session,
        "write it [[script:write]]",
        json!({"jetbrains": {"air": {"agentFileChangeReportRequest": {"version": 1, "requestId": "report-1"}}}}),
    );
    assert_eq!(adapter.result(id)["stopReason"], "end_turn");
    let frame = adapter.wait("the file change report", |frame| {
        !frame["params"]["update"]["_meta"]["jetbrains"]["air"]["agentFileChangeReport"].is_null()
    });
    let report = &frame["params"]["update"]["_meta"]["jetbrains"]["air"]["agentFileChangeReport"];
    assert_eq!(report["requestId"], "report-1", "{report}");
    assert_eq!(report["declaredComplete"], true, "{report}");
    let written = host.workspace().join("src/lib.rs").canonicalize().unwrap();
    let paths: Vec<PathBuf> = report["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| {
            PathBuf::from(path.as_str().unwrap())
                .canonicalize()
                .unwrap()
        })
        .collect();
    assert_eq!(paths, vec![written], "{report}");
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
    adapter.turn(&session, "before [[script:hello]]");
    let status = Command::new("kill")
        .args(["-9", &adapter.host_pid().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    // The session is re-attached to the new host, not lost.
    adapter.wait_log("host-restarted attempt=1 sessions=1 failures=0");
    adapter.turn(&session, "after [[script:hello]]");
    adapter.finish();
}

fn write_step() -> Value {
    json!({"tool": {"name": "write_file", "arguments": {
        "path": "notes.txt", "content": "written\n",
    }}})
}

fn set_mode(adapter: &mut Adapter, session: &str, mode: &str) -> Value {
    let id = adapter.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "mode", "value": mode}),
    );
    adapter.result(id)
}

/// The text of every tool result the editor saw.
fn tool_output(adapter: &Adapter) -> String {
    adapter
        .updates("tool_call_update")
        .iter()
        .map(|update| update["content"].to_string())
        .collect()
}

#[test]
fn read_only_mode_blocks_writes_until_switched_back() {
    if !enabled("read_only_mode_blocks_writes_until_switched_back") {
        return;
    }
    let host = Host::start(json!({"write": write_step()}), None);
    let notes = host.workspace().join("notes.txt");
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    set_mode(&mut adapter, &session, "readOnly");
    adapter.turn(&session, "write it [[script:write]]");
    assert!(!notes.exists(), "a read-only session must not write");
    assert!(
        tool_output(&adapter).contains("denied"),
        "Muse reports the refusal: {}",
        tool_output(&adapter)
    );
    set_mode(&mut adapter, &session, "default");
    adapter.turn(&session, "write it now [[script:write]]");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "written\n");
    adapter.finish();
}

#[test]
fn a_mode_change_keeps_other_new_sessions_usable() {
    if !enabled("a_mode_change_keeps_other_new_sessions_usable") {
        return;
    }
    let host = Host::start(json!({"write": write_step()}), None);
    let mut adapter = Adapter::launch(&host);
    // Neither session has run a turn, so Muse has not saved either. Moving
    // one restarts the main host, which must start the other again.
    let waiting = adapter.new_session(&host, json!([]));
    let planning = adapter.new_session(&host, json!([]));
    set_mode(&mut adapter, &planning, "plan");
    adapter.turn(&waiting, "write it [[script:write]]");
    assert_eq!(
        std::fs::read_to_string(host.workspace().join("notes.txt")).unwrap(),
        "written\n"
    );
    adapter.finish();
}

#[test]
fn plan_mode_survives_an_adapter_restart() {
    if !enabled("plan_mode_survives_an_adapter_restart") {
        return;
    }
    let host = Host::start(json!({"write": write_step()}), None);
    let mut first = Adapter::launch(&host);
    let session = first.new_session(&host, json!([]));
    first.turn(&session, "/plan");
    // Muse saves a session with its first turn; one that never ran cannot be
    // loaded again.
    first.turn(&session, "look around");
    first.finish();

    let mut second = Adapter::launch(&host);
    let id = second.request(
        "session/load",
        json!({"sessionId": session, "cwd": host.workspace(), "mcpServers": []}),
    );
    let loaded = second.result(id);
    assert_eq!(loaded["modes"]["currentModeId"], "plan", "{loaded}");
    second.turn(&session, "write it [[script:write]]");
    assert!(
        !host.workspace().join("notes.txt").exists(),
        "a reloaded plan session must still not write"
    );
    assert!(
        tool_output(&second).contains("denied"),
        "Muse reports the refusal: {}",
        tool_output(&second)
    );
    second.finish();
}
