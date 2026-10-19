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
fn a_reply_cut_off_mid_stream_is_marked_truncated() {
    if !enabled("a_reply_cut_off_mid_stream_is_marked_truncated") {
        return;
    }
    let host = Host::start(
        json!({"cut": {"text": "the answer is fo", "cut": true}}),
        None,
    );
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(
        adapter,
        release,
        (1, 4, 7),
        "a_reply_cut_off_mid_stream_is_marked_truncated",
    ) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "answer [[script:cut]]");
    let mark = adapter.wait("the cut-off mark", |frame| {
        frame["params"]["update"]["_meta"]["muse"]["cutOff"] == true
    });
    assert_eq!(
        mark["params"]["update"]["sessionUpdate"], "agent_message_chunk",
        "{mark}"
    );
    assert!(
        adapter
            .text("agent_message_chunk")
            .contains("the answer is fo"),
        "{:?}",
        adapter.updates("agent_message_chunk")
    );
    let response = adapter.response(id);
    assert!(
        response.get("error").is_some(),
        "a cut-off reply fails the turn: {response}"
    );
    adapter.finish();
}

/// A provider backoff behind one model attempt (Muse 1.4.8's
/// `turn/modelRetryScheduled`). The loopback provider answers the first call
/// with a 503, so the host schedules a model retry mid-turn: the editor sees
/// the attempt facts as `_meta.muse.modelRetry` while the prompt stays open,
/// the retried attempt's reply still streams, and the prompt settles once.
#[test]
fn a_model_retry_is_shown_while_the_turn_stays_open() {
    let test = "a_model_retry_is_shown_while_the_turn_stays_open";
    if !enabled(test) {
        return;
    }
    let host = Host::start(
        json!({"flaky": {"text": "after the retry", "fail_first": 1}}),
        None,
    );
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 8), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "answer [[script:flaky]]");
    let hint = adapter.wait("the model-retry hint", |frame| {
        frame["params"]["update"]["_meta"]["muse"]["modelRetry"].is_object()
    });
    let retry = &hint["params"]["update"]["_meta"]["muse"]["modelRetry"];
    assert_eq!(retry["attempt"], 1, "{hint}");
    assert_eq!(retry["nextAttempt"], 2, "{hint}");
    assert!(
        retry["maxAttempts"].as_u64().unwrap_or(0) >= 2,
        "a retry chain bound: {hint}"
    );
    assert!(retry["retryDelayMs"].is_u64(), "{hint}");
    assert!(retry["errorKind"].is_string(), "{hint}");
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        adapter
            .text("agent_message_chunk")
            .contains("after the retry"),
        "{:?}",
        adapter.updates("agent_message_chunk")
    );
    adapter.wait("the hint withdrawn", |frame| {
        frame["params"]["update"]["_meta"]["muse"]["modelRetry"].is_null()
            && frame["params"]["update"]["_meta"]["muse"]
                .as_object()
                .is_some_and(|muse| muse.contains_key("modelRetry"))
    });
    let log = std::fs::read_to_string(&adapter.log).unwrap();
    assert!(
        log.contains("turn/modelRetryScheduled turn="),
        "the attempt facts reach the diagnostics:\n{log}"
    );
    adapter.finish();
}

#[test]
fn host_feature_gates_read_the_installed_muse_version() {
    if !enabled("host_feature_gates_read_the_installed_muse_version") {
        return;
    }
    // Session delete, workspace roots, and host cost are gated on the
    // reported `serverInfo.version`. A version the gate cannot read would
    // silently turn them all off, so the real host's version must parse and
    // give the gates the release that added each feature (and, for delete,
    // the release that removed it).
    let host = Host::start(json!({}), None);
    let adapter = Adapter::launch(&host);
    let (release, label) = adapter_host_release(&adapter);
    // `muse serve` without --no-session-log is durable.
    let want = format!(
        "host-features server={label} session_delete={} workspace_roots={} session_cost={}",
        serves_session_delete(release),
        release >= (1, 4, 1),
        release >= (1, 4, 2)
    );
    let log = std::fs::read_to_string(&adapter.log).unwrap();
    assert!(log.contains(&want), "expected {want:?} in:\n{log}");
    adapter.finish();
}

/// `/rename` against a real host, across the `sessionRename` registry entry.
///
/// The adapter requests `sessionRename` at `initialize` unconditionally
/// (Muse 1.4.8 added it to the grantable registry) and records the grant on
/// its handshake. A host that predates the entry ignores the unknown name and
/// withholds it, yet still serves `session/rename`, so `/rename` must keep
/// working on every pinned build; the 1.4.8 pin proves the granted path. The
/// startup log names both facts, so each pin asserts what it actually saw
/// rather than what the test assumes.
#[test]
fn rename_drives_a_real_host_across_the_capability_registry() {
    let test = "rename_drives_a_real_host_across_the_capability_registry";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let mut adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let want = if release >= (1, 4, 8) {
        "session rename: granted=true available=true"
    } else {
        "session rename: granted=false available=true"
    };
    let log = std::fs::read_to_string(&adapter.log).unwrap();
    assert!(log.contains(want), "expected {want:?} in:\n{log}");
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "/rename Live rename probe");
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    // The host normalizes the requested name (a live 1.4.7 lower-cases it),
    // so the title is compared case-insensitively against the probe.
    let renamed = adapter.wait("the rename to reach ACP", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["sessionId"] == json!(session)
            && frame["params"]["update"]["sessionUpdate"] == "session_info_update"
            && frame["params"]["update"]["title"]
                .as_str()
                .is_some_and(|title| title.eq_ignore_ascii_case("Live rename probe"))
    });
    assert!(
        renamed["params"]["update"]["title"]
            .as_str()
            .is_some_and(|title| title.eq_ignore_ascii_case("Live rename probe")),
        "{renamed}"
    );
    adapter.finish();
}

/// The Muse release and full label the adapter reported for its host, read
/// from its own `host-ready` line.
fn adapter_host_release(adapter: &Adapter) -> ((u64, u64, u64), String) {
    let log = std::fs::read_to_string(&adapter.log).unwrap();
    let label = log
        .lines()
        .find_map(|line| line.split("host-ready server=").nth(1))
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("no host-ready line:\n{log}"))
        .to_string();
    let version = label.rsplit('/').next().unwrap();
    let parts: Vec<u64> = version
        .split(|c: char| !c.is_ascii_digit())
        .take(3)
        .map(|part| {
            part.parse()
                .unwrap_or_else(|_| panic!("unreadable host version {version:?}"))
        })
        .collect();
    assert_eq!(parts.len(), 3, "unreadable host version {version:?}");
    ((parts[0], parts[1], parts[2]), label)
}

/// Returns the adapter when the host is new enough, else closes it and
/// reports the skip. Feature tests use this instead of an assertion so CI's
/// older pinned builds report a skip, not a failure.
fn require_release(
    adapter: Adapter,
    release: (u64, u64, u64),
    want: (u64, u64, u64),
    test: &str,
) -> Option<Adapter> {
    if release < want {
        eprintln!("skipped {test}: host release {release:?} predates {want:?}");
        adapter.finish();
        return None;
    }
    Some(adapter)
}

/// Whether this Muse release serves MSP `session/delete`: 1.4.1 added it
/// and 1.4.3 removed it.
fn serves_session_delete(release: (u64, u64, u64)) -> bool {
    ((1, 4, 1)..(1, 4, 3)).contains(&release)
}

/// Like `require_release`, for the releases that serve `session/delete`.
fn require_session_delete(
    adapter: Adapter,
    release: (u64, u64, u64),
    test: &str,
) -> Option<Adapter> {
    if release >= (1, 4, 3) {
        eprintln!("skipped {test}: host release {release:?} no longer serves session/delete");
        adapter.finish();
        return None;
    }
    require_release(adapter, release, (1, 4, 1), test)
}

/// The live host's own model catalog, fetched over a direct MSP handshake.
fn host_catalog(host: &Host) -> Vec<Value> {
    let catalog = probe_host(host, "model/list", json!({}));
    assert!(
        catalog.get("error").is_none(),
        "probe model/list: {catalog}"
    );
    catalog["result"]["models"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The live host's response to one request, sent over a direct MSP
/// handshake in the same throwaway environment the adapter uses.
fn probe_host(host: &Host, method: &str, params: Value) -> Value {
    let home = host.dir.join("home");
    let mut command = Command::new(muse_cli());
    command.env_clear();
    for name in KEPT_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if cfg!(target_os = "macos") {
        command.env("TBH_CREDENTIAL_BACKEND", "file");
    }
    let mut child = command
        .env("MUSE_NO_AUTO_UPDATE", "1")
        .env("XDG_CONFIG_HOME", host.dir.join("config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("HOME", &home)
        .current_dir(host.workspace())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a probe muse serve");
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let wait_for =
        |lines: &mut std::io::Lines<BufReader<std::process::ChildStdout>>, id: u64| -> Value {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                assert!(
                    Instant::now() < deadline,
                    "probe host never answered request {id}"
                );
                let Some(Ok(line)) = lines.next() else {
                    panic!("probe host closed before answering request {id}");
                };
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if frame["id"] == id && frame.get("method").is_none() {
                    return frame;
                }
            }
        };
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "muse_acp_probe", "version": "0"},
            "capabilities": {"experimentalApi": true},
        }})
    )
    .unwrap();
    let init = wait_for(&mut lines, 1);
    assert!(init.get("error").is_none(), "probe initialize: {init}");
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params})
    )
    .unwrap();
    let response = wait_for(&mut lines, 2);
    let _ = child.kill();
    let _ = child.wait();
    response
}

#[test]
fn session_delete_removes_a_fresh_session() {
    let test = "session_delete_removes_a_fresh_session";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_session_delete(adapter, release, test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "say hello [[script:hello]]");
    assert_eq!(adapter.result(id)["stopReason"], "end_turn");
    let delete = adapter.request("session/delete", json!({"sessionId": session}));
    let response = adapter.response(delete);
    if let Some(error) = response.get("error") {
        // macOS Muse 1.4.2 can fail a fresh delete's cleanup after removing
        // some data; the adapter must surface the host's evidence rather than
        // claim the session was deleted. Any other failure is a regression.
        #[cfg(not(target_os = "macos"))]
        panic!("a fresh session must delete: {response} ({error})");
        #[cfg(target_os = "macos")]
        {
            let reason = error["data"]["reason"].as_str().unwrap_or("");
            let message = error["message"].as_str().unwrap_or("");
            match reason {
                // A background reminder observer can still be finishing, so
                // the host refuses the delete as busy.
                "writerBusy" | "quiescenceFailed" => assert!(
                    message.contains("work is still running"),
                    "busy refusal mapped: {error}"
                ),
                "cleanupIncomplete" => assert!(
                    message.contains("Some of its data may already be removed."),
                    "cleanup refusal mapped: {error}"
                ),
                other => panic!("unexpected fresh-delete refusal on macOS: {other} ({response})"),
            }
            adapter.finish();
            return;
        }
    }
    assert_eq!(response["result"], json!({}), "a fresh session must delete");
    let list = adapter.request(
        "session/list",
        json!({"cwd": host.workspace().to_string_lossy()}),
    );
    let listed = adapter.result(list);
    assert!(
        !listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["sessionId"] == session),
        "a deleted session must not be listed: {listed}"
    );
    let load = adapter.request("session/load", json!({"sessionId": session}));
    let response = adapter.response(load);
    let error = response
        .get("error")
        .expect("loading a deleted session fails");
    assert_eq!(error["code"], -32002, "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("may have been deleted"),
        "{error}"
    );
    adapter.finish();
}

#[test]
fn session_delete_of_an_earlier_run_reports_ownership() {
    let test = "session_delete_of_an_earlier_run_reports_ownership";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let session = {
        let adapter = Adapter::launch(&host);
        let (release, _) = adapter_host_release(&adapter);
        let Some(mut adapter) = require_session_delete(adapter, release, test) else {
            return;
        };
        let session = adapter.new_session(&host, json!([]));
        let id = adapter.prompt(&session, "say hello [[script:hello]]");
        assert_eq!(adapter.result(id)["stopReason"], "end_turn");
        adapter.finish();
        session
    };
    // A second adapter run starts a new muse serve process, which cannot
    // always prove it owns the earlier process's logs. Muse 1.4.x either
    // refuses with `ownershipUnavailable` or, when the log is provably
    // ownerless, completes the delete; both are honest outcomes.
    let mut adapter = Adapter::launch(&host);
    let delete = adapter.request("session/delete", json!({"sessionId": session}));
    let response = adapter.response(delete);
    match response.get("error") {
        Some(error) => {
            assert_eq!(error["code"], -32603, "{error}");
            assert_eq!(error["data"]["reason"], "ownershipUnavailable", "{error}");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("cannot prove it owns"),
                "{error}"
            );
        }
        None => assert_eq!(response["result"], json!({}), "{response}"),
    }
    adapter.finish();
}

#[test]
fn session_delete_of_an_unknown_uuid_succeeds() {
    let test = "session_delete_of_an_unknown_uuid_succeeds";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_session_delete(adapter, release, test) else {
        return;
    };
    let _session = adapter.new_session(&host, json!([]));
    let ghost = "01a10c8b-2222-7333-8444-555566667777";
    let delete = adapter.request("session/delete", json!({"sessionId": ghost}));
    let response = adapter.response(delete);
    if release >= (1, 4, 2) {
        assert!(
            response.get("error").is_none(),
            "list filters prove a session never existed: {response}"
        );
        assert_eq!(response["result"], json!({}));
    } else {
        // 1.4.1 has no `session/list` filters, so the host's
        // `ownershipUnavailable` terminal cannot be told apart from a real
        // session it cannot prove it owns. Reporting the refusal is the
        // honest answer; claiming success could hide a kept session.
        let error = response
            .get("error")
            .expect("without list filters the absence cannot be proven");
        assert_eq!(error["data"]["reason"], "ownershipUnavailable", "{error}");
    }
    adapter.finish();
}

#[test]
fn session_delete_is_refused_once_the_host_drops_it() {
    let test = "session_delete_is_refused_once_the_host_drops_it";
    if !enabled(test) {
        return;
    }
    // Muse 1.4.3 answers `session/delete` with `methodNotFound`, so the
    // adapter must stop advertising delete and refuse it itself, keeping
    // the session.
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 3), test) else {
        return;
    };
    // Muse itself, asked directly, does not serve the method. A later
    // release that serves it again fails here, so the gate gets revisited.
    let direct = probe_host(
        &host,
        "session/delete",
        json!({
            "sessionId": "01a10c8b-2222-7333-8444-555566667777",
            "commandId": "01a10c8b-3333-7444-8555-666677778888",
        }),
    );
    assert_eq!(direct["error"]["code"], -32601, "{direct}");
    let capabilities = adapter.result(1)["agentCapabilities"]["sessionCapabilities"].clone();
    assert!(
        capabilities.get("delete").is_none(),
        "delete must not be advertised: {capabilities}"
    );
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.prompt(&session, "say hello [[script:hello]]");
    assert_eq!(adapter.result(id)["stopReason"], "end_turn");
    let delete = adapter.request("session/delete", json!({"sessionId": session}));
    let response = adapter.response(delete);
    let error = response.get("error").expect("delete is refused");
    assert_eq!(error["code"], -32601, "{error}");
    let list = adapter.request(
        "session/list",
        json!({"cwd": host.workspace().to_string_lossy()}),
    );
    let listed = adapter.result(list);
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["sessionId"] == session),
        "a refused delete must keep the session: {listed}"
    );
    adapter.finish();
}

#[test]
fn workspace_roots_let_muse_read_an_extra_directory() {
    let test = "workspace_roots_let_muse_read_an_extra_directory";
    if !enabled(test) {
        return;
    }
    let extra = std::env::temp_dir().join(format!(
        "muse-acp-live-extra-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&extra);
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("secret.txt"), "extra-root-secret\n").unwrap();
    let secret = extra.join("secret.txt");
    let host = Host::start(
        json!({"read": {"tool": {"name": "read_file", "arguments": {
            "path": secret.to_string_lossy(),
        }}}}),
        None,
    );
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 1), test) else {
        return;
    };
    let id = adapter.request(
        "session/new",
        json!({
            "cwd": host.workspace(),
            "additionalDirectories": [extra],
            "mcpServers": [],
        }),
    );
    let session = adapter.result(id)["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    adapter.turn(&session, "read the secret [[script:read]]");
    let results = tool_results(&host);
    assert!(
        results
            .iter()
            .any(|result| result.contains("extra-root-secret")),
        "Muse's own read tool must see the extra root: {results:?}"
    );
    adapter.finish();
    let _ = std::fs::remove_dir_all(&extra);
}

#[test]
fn reasoning_selector_matches_the_live_model_catalog() {
    let test = "reasoning_selector_matches_the_live_model_catalog";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({}), None);
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    let frame = adapter.wait("the session/new result", |frame| {
        frame["result"]["sessionId"] == session
    });
    let options = frame["result"]["configOptions"].as_array().unwrap();
    let model = options
        .iter()
        .find(|option| option["configId"] == "model" || option["id"] == "model")
        .unwrap()["currentValue"]
        .as_str()
        .unwrap()
        .to_string();
    let reasoning = options
        .iter()
        .find(|option| {
            option["configId"] == "reasoning_effort" || option["id"] == "reasoning_effort"
        })
        .unwrap();
    let values: Vec<&str> = reasoning["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|option| option["value"].as_str().unwrap())
        .collect();
    let catalog = host_catalog(&host);
    let row = catalog
        .iter()
        .find(|row| row["modelId"] == model.as_str())
        .unwrap_or_else(|| panic!("no catalog row for {model}: {catalog:?}"));
    let want: Vec<&str> = match row["variants"].as_array() {
        Some(variants) => variants.iter().map(|v| v.as_str().unwrap()).collect(),
        // A catalog that cannot describe its tiers keeps the fixed list.
        None => vec![
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ],
    };
    let have: Vec<&str> = values.iter().copied().filter(|v| *v != "default").collect();
    assert_eq!(
        have, want,
        "selector vs live catalog for {model}: {reasoning}"
    );
    assert_eq!(values[0], "default", "{reasoning}");
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
fn auto_review_decides_a_shell_approval_against_a_real_host() {
    let test = "auto_review_decides_a_shell_approval_against_a_real_host";
    if !enabled(test) {
        return;
    }
    // The reviewer host runs a real `muse serve` too, so its turn must use the
    // same `input` shape the schema requires. The guardian marker routes this
    // reply to the reviewer and `[[script:shell]]` to the agent.
    let allow = json!({"text":
        "{\"risk_level\":\"low\",\"user_authorization\":\"high\",\"outcome\":\"allow\",\"rationale\":\"Routine workspace command.\"}"});
    let host = Host::start(json!({"shell": shell_step(), "review": allow}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    // Through Muse 1.4.1 the reviewer host's own background reminder checks
    // hold the review turn open, so it never completes and every review times
    // out and denies. 1.4.2 settles the turn, and Auto-review is documented as
    // requiring 1.4.2 or newer.
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 2), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let set = adapter.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "auto_review", "value": "on"}),
    );
    adapter.result(set);
    let id = adapter.prompt(&session, "write it [[script:shell]]");
    // The turn only ends once the reviewer has decided. If the reviewer host
    // rejects the review turn, the adapter denies the approval instead.
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    {
        let output = adapter.frames.0.lock().unwrap();
        assert!(
            !output
                .frames
                .iter()
                .any(|frame| frame["method"] == "session/request_permission"),
            "auto-review must decide without prompting the editor: {:#?}",
            output.frames
        );
    }
    let log = std::fs::read_to_string(&adapter.log).unwrap_or_default();
    assert!(
        log.contains("auto-review allow"),
        "the reviewer's decision is logged:\n{log}"
    );
    assert_eq!(
        std::fs::read_to_string(host.workspace().join("approved.txt"))
            .ok()
            .as_deref(),
        Some("approved"),
        "the reviewer-approved command ran"
    );
    adapter.finish();
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

/// Waits until the model has been called with a last input item whose text
/// contains `needle`.
fn wait_for_model_input(host: &Host, needle: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let seen = std::fs::read_to_string(host.dir.join("provider.log"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|call| {
                call["last"]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(needle))
            });
        if seen {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the model never received {needle:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Byte length of the provider log, for scoping a later assertion to calls
/// made after this point.
fn provider_log_len(host: &Host) -> usize {
    std::fs::metadata(host.dir.join("provider.log"))
        .map(|meta| meta.len() as usize)
        .unwrap_or(0)
}

/// The tool results Muse returned to the model after `since` bytes of the
/// provider log, one per model call that followed a tool call.
fn tool_results_since(host: &Host, since: usize) -> Vec<String> {
    let log = std::fs::read(host.dir.join("provider.log")).unwrap_or_default();
    String::from_utf8_lossy(log.get(since..).unwrap_or_default())
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call["last"]["type"] == "function_call_output")
        .map(|call| call["last"]["output"].to_string())
        .collect()
}

/// The tool results Muse returned to the model, one per model call that
/// followed a tool call.
fn tool_results(host: &Host) -> Vec<String> {
    tool_results_since(host, 0)
}

/// The adapter's `elicitation/create` form whose schema asks for `field`.
fn form(adapter: &Adapter, field: &str) -> Value {
    adapter.wait(&format!("a form asking for {field}"), |frame| {
        frame["method"] == "elicitation/create"
            && frame["params"]["requestedSchema"]["properties"]
                .get(field)
                .is_some()
    })
}

#[test]
fn a_user_input_question_becomes_an_editor_form_and_the_answer_reaches_muse() {
    if !enabled("a_user_input_question_becomes_an_editor_form_and_the_answer_reaches_muse") {
        return;
    }
    // Muse offers request_user_input only to a client that can show forms.
    let host = Host::start(
        json!({"ask": {"tool": {"name": "request_user_input", "arguments": {"questions": [{
            "id": "color",
            "header": "Color",
            "question": "Which color should the notes use?",
            "options": [
                {"label": "Blue (Recommended)", "description": "Calm."},
                {"label": "Red", "description": "Loud."},
            ],
        }]}}}}),
        None,
    );
    let mut adapter = Adapter::launch_with(&host, json!({"elicitation": {"form": {}}}));
    let session = adapter.new_session(&host, json!([]));
    let prompt = adapter.prompt(&session, "ask me [[script:ask]]");
    let route = form(&adapter, "route");
    adapter.send(json!({
        "jsonrpc": "2.0",
        "id": route["id"],
        "result": {"action": "accept", "content": {"route": "Answer questions"}},
    }));
    let question = form(&adapter, "q0");
    assert!(
        question["params"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Which color should the notes use?")),
        "{question}"
    );
    let red = question["params"]["requestedSchema"]["properties"]["q0"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice.as_str().is_some_and(|label| label.contains("Red")))
        .unwrap_or_else(|| panic!("no Red choice in {question}"))
        .clone();
    adapter.send(json!({
        "jsonrpc": "2.0",
        "id": question["id"],
        "result": {"action": "accept", "content": {"q0": red}},
    }));
    let result = adapter.result(prompt);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    let results = tool_results(&host);
    assert!(
        results.iter().any(|output| output.contains("Red")),
        "Muse returns the chosen answer to the model: {results:?}"
    );
    adapter.finish();
}

#[test]
fn stopping_a_staged_question_interrupts_with_the_confirmed_answer() {
    let test = "stopping_a_staged_question_interrupts_with_the_confirmed_answer";
    if !enabled(test) {
        return;
    }
    // userInput/interrupt arrived in Muse 1.4.3 (ADR 45450 D1): it settles
    // the open prompt with the confirmed partial answers and stops the turn.
    // ACP forms are atomic, so the two questions arrive as two staged forms
    // and the first answer is held until the stop carries it.
    let host = Host::start(
        json!({"ask": {"tool": {"name": "request_user_input", "arguments": {"questions": [
            {"id": "color", "header": "Color",
             "question": "Which color should the notes use?",
             "options": [{"label": "Blue (Recommended)"}, {"label": "Red"}]},
            {"id": "size", "header": "Size", "question": "Which size?",
             "options": [{"label": "Small"}, {"label": "Large"}]},
        ]}}}}),
        None,
    );
    let adapter = Adapter::launch_with(&host, json!({"elicitation": {"form": {}}}));
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 3), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let prompt = adapter.prompt(&session, "ask me [[script:ask]]");
    let route = form(&adapter, "route");
    adapter.send(json!({
        "jsonrpc": "2.0",
        "id": route["id"],
        "result": {"action": "accept", "content": {"route": "Answer questions"}},
    }));
    let first = form(&adapter, "q0");
    assert!(
        first["params"]["requestedSchema"]["properties"]
            .get("q1")
            .is_none(),
        "questions are staged one at a time: {first}"
    );
    let red = first["params"]["requestedSchema"]["properties"]["q0"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice.as_str().is_some_and(|label| label.contains("Red")))
        .unwrap_or_else(|| panic!("no Red choice in {first}"))
        .clone();
    adapter.send(json!({
        "jsonrpc": "2.0",
        "id": first["id"],
        "result": {"action": "accept", "content": {"q0": red}},
    }));
    let second = form(&adapter, "q1");
    assert!(
        second["params"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Question 2 of 2")),
        "the staged walk reports its position: {second}"
    );
    // Stop while the second question waits: the held answer rides the
    // interrupt, and the host stops the turn with it.
    adapter.send(json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {"sessionId": session},
    }));
    let result = adapter.result(prompt);
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    // The turn stops before the model is called again, so the confirmed
    // answer lives on the interrupted tool result in the stored session
    // rather than in a later provider request.
    let read = probe_host(
        &host,
        "session/read",
        json!({"sessionId": session, "excludeItems": false}),
    );
    assert!(read.get("error").is_none(), "session/read: {read}");
    let read = read.to_string();
    assert!(
        read.contains("selected_label") && read.contains("Red"),
        "Muse records the confirmed partial answer on the interrupted result: {read}"
    );
    adapter.finish();
}

/// `_session/hooks` returns the host's `hook/list` rows once the client
/// negotiates the `hooks` extension. `hook/list` arrived in Muse 1.4.4.
#[test]
fn session_hook_reads_reach_a_real_host_when_negotiated() {
    let test = "session_hook_reads_reach_a_real_host_when_negotiated";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch_with(&host, json!({"_meta": {"muse": {"hooks": true}}}));
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 4), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let read = adapter.request("_session/hooks", json!({"sessionId": session}));
    let result = adapter.result(read);
    assert!(
        result["hooks"].is_array(),
        "the host's hook rows come back: {result}"
    );
    adapter.finish();
}

/// A local video resource link uploads to the real host's video store and
/// reaches the turn as a `video` part. `media/upload` arrived in Muse 1.4.4,
/// so older CI pins skip.
#[test]
fn video_resource_links_reach_a_real_host() {
    let test = "video_resource_links_reach_a_real_host";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 4), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let clip = host.workspace().join("clip.mp4");
    std::fs::write(&clip, vec![7u8; 300_000]).expect("write the video fixture");
    let path = clip.display().to_string().replace('\\', "/");
    let uri = if cfg!(windows) {
        format!("file:///{path}")
    } else {
        format!("file://{path}")
    };
    let id = adapter.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [
            {"type": "text", "text": "watch [[script:hello]]"},
            {"type": "resource_link", "uri": uri, "name": "clip.mp4",
             "mimeType": "video/mp4"},
        ]}),
    );
    let response = adapter.response(id);
    if response.get("result").is_some() {
        assert_eq!(response["result"]["stopReason"], "end_turn", "{response}");
    } else {
        // The loopback provider serves only the model endpoints, so the
        // host's own attempt to ship the uploaded video to a Files API is
        // the observable proof that the upload and the video part were
        // accepted; a host-side upload failure would have failed earlier.
        // Both failures happen after the host accepted the upload and built
        // the video part: 1.4.6 tries to ship it to the model provider's
        // Files API (which the loopback provider does not serve), and older
        // builds gate video input behind TBH_VIDEO_INPUT_ENABLED. An
        // upload-side failure would have failed earlier with the chunk text.
        let message = response["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("Files API") || message.contains("video input is disabled"),
            "the video reached the turn: {response}"
        );
    }
    adapter.finish();
}

/// The opt-in computer-use extension reaches a real host: either it answers
/// with the setup state and settings, or the CUA gate answers its typed
/// `computerUseUnavailable`, never a generic failure. The capability and the
/// methods arrived in Muse 1.4.6, so older CI pins skip.
#[test]
fn computer_use_reads_reach_a_real_host_when_negotiated() {
    let test = "computer_use_reads_reach_a_real_host_when_negotiated";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch_with(&host, json!({"_meta": {"muse": {"computerUse": true}}}));
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 6), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let log = std::fs::read_to_string(&adapter.log).unwrap();
    assert!(
        log.contains("client negotiated computer-use setup"),
        "{log}"
    );
    let id = adapter.request("_session/computerUse", json!({"sessionId": session}));
    let response = adapter.response(id);
    if response.get("error").is_some() {
        // Either the host withheld the grant on this platform (the adapter
        // stays dark) or the CUA gate answered its typed refusal.
        match response["error"]["code"].as_i64() {
            Some(-32601) => {}
            Some(-32033) => assert_eq!(
                response["error"]["data"]["kind"],
                json!("computerUseUnavailable"),
                "{response}"
            ),
            _ => panic!("unexpected computer-use refusal: {response}"),
        }
    } else {
        assert!(
            response["result"]["setup"].is_object() && response["result"]["settings"].is_object(),
            "{response}"
        );
    }
    adapter.finish();
}

/// `_session/plugins` reads the real host's installed-plugin inventory
/// (`plugin/list`, Muse 1.4.4+); on 1.4.6 a refused marketplace removal keeps
/// the host's typed error.
#[test]
fn plugin_inventory_and_typed_refusals_reach_a_real_host() {
    let test = "plugin_inventory_and_typed_refusals_reach_a_real_host";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch_with(&host, json!({"_meta": {"muse": {"plugins": true}}}));
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 4), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let read = adapter.request("_session/plugins", json!({"sessionId": session}));
    let result = adapter.result(read);
    assert!(
        result["plugins"].is_array(),
        "the host's plugin rows come back: {result}"
    );
    if release >= (1, 4, 6) {
        let id = adapter.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{
                "type": "text",
                "text": "/plugin marketplace remove no-such-marketplace-muse-acp",
            }]}),
        );
        let response = adapter.response(id);
        assert_eq!(response["error"]["code"], json!(-32034), "{response}");
        assert_eq!(
            response["error"]["data"]["kind"],
            json!("marketplaceNotFound"),
            "{response}"
        );
    }
    adapter.finish();
}

/// Resuming with AIR async tasks negotiated asks the real host for its
/// authoritative `task/list` inventory. `task/list` arrived in Muse 1.4.6,
/// so older CI pins skip.
#[test]
fn task_list_seeds_the_async_task_inventory_after_resume() {
    let test = "task_list_seeds_the_async_task_inventory_after_resume";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch_with(
        &host,
        json!({"_meta": {"jetbrains": {"air": {"version": 1, "capabilities": ["asyncTasks"]}}}}),
    );
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 6), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let resume = adapter.request("session/resume", json!({"sessionId": session}));
    let _ = adapter.result(resume);
    // A fresh session has no running work, so the host's answer is the empty
    // inventory; the point is that the authoritative read happens and
    // settles the resume.
    let log = std::fs::read_to_string(&adapter.log).unwrap();
    assert!(
        log.contains("task/list seeded 0 async task(s)"),
        "the resume seeds from task/list:\n{log}"
    );
    adapter.finish();
}

/// `_session/sideChat` opens a side chat from a loaded session; the side's
/// provenance names its source, and it attaches as an ordinary session.
/// `session/sideChat` arrived in Muse 1.4.4, so older CI pins skip.
#[test]
fn side_chat_reaches_a_real_host_and_attaches_with_provenance() {
    let test = "side_chat_reaches_a_real_host_and_attaches_with_provenance";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch_with(&host, json!({"_meta": {"muse": {"sideChat": true}}}));
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 4), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.request("_session/sideChat", json!({"sessionId": session}));
    let result = adapter.result(id);
    let side = result["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the extension names the side: {result}"))
        .to_string();
    assert_ne!(side, session, "{result}");
    assert_eq!(result["sideFrom"]["sessionId"], json!(session), "{result}");
    let announced = adapter.wait("the side's provenance", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["sessionId"] == json!(side)
            && frame["params"]["update"]["_meta"]["muse"]["sideFrom"]["sessionId"] == json!(session)
    });
    assert!(announced["params"]["update"]["sessionUpdate"] == "session_info_update");
    // The side attaches through the ordinary resume path and serves a turn.
    let resume = adapter.request("session/resume", json!({"sessionId": side}));
    let attached = adapter.result(resume);
    assert_eq!(attached["sessionId"], json!(side), "{attached}");
    let turn = adapter.prompt(&side, "say hello [[script:hello]]");
    assert_eq!(adapter.result(turn)["stopReason"], "end_turn");
    adapter.finish();
}

/// `/skill-activation` writes one skill's activation through
/// `skill/setActivation`. Muse 1.4.4 added the method, so older CI pins skip.
#[test]
fn skill_activation_reaches_a_real_host() {
    let test = "skill_activation_reaches_a_real_host";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 4), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let commands = adapter.wait("the command catalog", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["update"]["sessionUpdate"] == "available_commands_update"
    });
    let local = [
        "compact",
        "cua",
        "feedback",
        "goal",
        "plugin",
        "recap",
        "rename",
        "skill-activation",
        "skill-import",
        "workflow",
        "workflow-child",
    ];
    let skill = commands["params"]["update"]["availableCommands"]
        .as_array()
        .expect("commands")
        .iter()
        .filter_map(|command| command["name"].as_str())
        .find(|name| !local.contains(name))
        .unwrap_or_else(|| panic!("a host skill in {commands}"))
        .to_string();
    // The write must carry the row's own scope: a throwaway host's skills are
    // bundled, and the host refuses to record one against another scope. Try
    // the closed vocabulary and require at least one accepted write.
    let mut accepted = None;
    let mut refusals = Vec::new();
    for scope in ["bundled", "user", "plugin", "project"] {
        let id = adapter.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{
                "type": "text",
                "text": format!("/skill-activation {skill} userInvocableOnly {scope}"),
            }]}),
        );
        let response = adapter.response(id);
        if response.get("result").is_some() {
            accepted = Some(scope);
            break;
        }
        refusals.push(response);
    }
    assert!(
        accepted.is_some(),
        "no scope accepted the activation write: {refusals:?}"
    );
    adapter.finish();
}

/// `/skill-import` dry-runs a foreign-ecosystem import through the host's own
/// directories. `skill/import` arrived in Muse 1.4.6, so older pins skip.
#[test]
fn skill_import_dry_run_reaches_a_real_host() {
    let test = "skill_import_dry_run_reaches_a_real_host";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 6), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let id = adapter.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{
            "type": "text",
            "text": "/skill-import claude dry-run",
        }]}),
    );
    let result = adapter.result(id);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    // The throwaway home has no Claude skills, so the dry run reports zero
    // writes; the point is that the receipt settles the prompt.
    let receipt = adapter.wait("the import receipt", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            && frame["params"]["update"]["content"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("Skill import dry run from claude"))
    });
    assert!(
        receipt["params"]["update"]["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("installed")),
        "{receipt}"
    );
    adapter.finish();
}

/// `/recap` asks the host for a while-you-were-away summary; the held prompt
/// settles when `recap/completed` arrives. `recap/request` arrived in Muse
/// 1.4.6, so older CI pins skip.
#[test]
fn recap_settles_the_held_prompt_with_a_real_host() {
    let test = "recap_settles_the_held_prompt_with_a_real_host";
    if !enabled(test) {
        return;
    }
    let host = Host::start(json!({"hello": {"text": "hello"}}), None);
    let adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    let Some(mut adapter) = require_release(adapter, release, (1, 4, 6), test) else {
        return;
    };
    let session = adapter.new_session(&host, json!([]));
    let turn = adapter.prompt(&session, "say hello [[script:hello]]");
    assert_eq!(adapter.result(turn)["stopReason"], "end_turn");
    let recap = adapter.prompt(&session, "/recap");
    // The host answers the recap's own model call through the loopback
    // provider; the summary arrives as agent text and then the prompt
    // settles, never as a hung request.
    adapter.wait("the recap summary", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
    });
    let result = adapter.result(recap);
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    adapter.finish();
}

/// Pausing a live workflow folds the pause fact onto its card, and resuming
/// it launches a new run whose item names the paused one. `workflow/pause`
/// arrived in Muse 1.4.4; `workflow/resume` in 1.4.6, so the resume half
/// skips on the older CI pins.
#[test]
fn workflow_pause_and_resume_drive_a_real_run() {
    let test = "workflow_pause_and_resume_drive_a_real_run";
    if !enabled(test) {
        return;
    }
    // The child's model call holds the workflow open long enough to pause it.
    let host = Host::start(
        json!({
            "workflow": {"tool": {"name": "workflow", "arguments": {
                "name": "Pausable run",
                "script": "export default async function workflow(host) { return await host.agent({ input: \"child task [[script:hold]]\", label: \"helper\" }); }",
            }}},
            "hold": {"text": "working", "hold_ms": 60000},
        }),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let (release, _) = adapter_host_release(&adapter);
    if release < (1, 4, 4) {
        eprintln!("skipped {test}: host release {release:?} predates workflow/pause");
        adapter.finish();
        return;
    }
    let session = adapter.new_session(&host, json!([]));
    adapter.turn(&session, "delegate it [[script:workflow]]");
    adapter.wait("the running workflow card", |frame| {
        let update = &frame["params"]["update"];
        frame["method"] == "session/update"
            && update["title"] == "Workflow Pausable run"
            && update["status"] == "in_progress"
    });
    let paused = adapter.prompt(&session, "/workflow pause");
    let result = adapter.result(paused);
    assert!(result.is_object(), "pause settles on the ack: {result}");
    let paused_card = adapter.wait("the paused workflow card", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["update"]["title"] == "Workflow Pausable run (paused)"
    });
    assert_eq!(
        paused_card["params"]["update"]["_meta"]["muse"]["workflow"]["paused"],
        json!(true),
        "{paused_card}"
    );
    let run_id = paused_card["params"]["update"]["_meta"]["muse"]["workflow"]["workflowRunId"]
        .as_str()
        .expect("the card names its run id")
        .to_string();
    if release < (1, 4, 6) {
        eprintln!(
            "skipped the resume half of {test}: host release {release:?} predates workflow/resume"
        );
        adapter.finish();
        return;
    }
    let resumed = adapter.prompt(&session, &format!("/workflow resume {run_id}"));
    let result = adapter.result(resumed);
    assert!(result.is_object(), "resume settles on the ack: {result}");
    let card = adapter.wait("the resumed workflow card", |frame| {
        frame["method"] == "session/update"
            && frame["params"]["update"]["_meta"]["muse"]["workflow"]["resumeFromRunId"]
                == json!(run_id)
    });
    // Muse 1.4.6 reuses the run id for the resumed generation, so the new
    // item (a fresh card) is the observable boundary, not a new run id.
    assert_ne!(
        card["params"]["update"]["toolCallId"], paused_card["params"]["update"]["toolCallId"],
        "the resumed launch is a new item: {card}"
    );
    assert_eq!(
        card["params"]["update"]["_meta"]["muse"]["workflow"]["workflowRunId"],
        json!(run_id),
        "{card}"
    );
    adapter.finish();
}

#[test]
fn a_workflow_child_runs_and_its_result_reaches_the_parent() {
    if !enabled("a_workflow_child_runs_and_its_result_reaches_the_parent") {
        return;
    }
    // `muse serve` starts child agents through the workflow tool; the native
    // subagent tools stay hidden there. The child reports through
    // submit_result, which the provider answers with "child finished".
    let host = Host::start(
        json!({"workflow": {"tool": {"name": "workflow", "arguments": {
            "name": "Helper run",
            "script": "export default async function workflow(host) { return await host.agent({ input: \"child task\", label: \"helper\" }); }",
        }}}}),
        None,
    );
    let mut adapter = Adapter::launch(&host);
    let session = adapter.new_session(&host, json!([]));
    adapter.turn(&session, "delegate it [[script:workflow]]");
    // The workflow runs on after the turn. Its card names the child session
    // and settles when the child does.
    let done = adapter.wait("the workflow card to complete", |frame| {
        let update = &frame["params"]["update"];
        frame["method"] == "session/update"
            && update["title"] == "Workflow Helper run"
            && update["status"] == "completed"
    });
    let card: String = adapter
        .updates("tool_call")
        .iter()
        .filter(|update| update["title"] == "Workflow Helper run")
        .map(|update| update["content"].to_string())
        .collect();
    assert!(
        card.contains("helper [") && card.contains("]: completed"),
        "the card follows the child to completion: {card}"
    );
    assert_eq!(done["params"]["sessionId"], session.as_str(), "{done}");
    // Muse wakes the parent with the child's result.
    wait_for_model_input(&host, "child finished");
    adapter.finish();
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

/// Muse's refusal of a write in a read-only or plan session. Through 1.4.2
/// the write tool is offered and denied, so the editor sees the denial;
/// 1.4.3 no longer offers write tools at all, so the model's call comes back
/// as an unknown tool. Muse reports that only to the model: the editor gets
/// no separate tool event, so the provider log is the strongest signal this
/// test has for the 1.4.3 path.
fn assert_write_refused(adapter: &Adapter, host: &Host, log_mark: usize) {
    let (release, _) = adapter_host_release(adapter);
    if release >= (1, 4, 3) {
        let results = tool_results_since(host, log_mark);
        assert!(
            results
                .iter()
                .any(|result| result.contains("unknown tool `write_file`")),
            "Muse does not offer the write tool: {results:?}"
        );
    } else {
        assert!(
            tool_output(adapter).contains("denied"),
            "Muse reports the refusal: {}",
            tool_output(adapter)
        );
    }
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
    let log_mark = provider_log_len(&host);
    adapter.turn(&session, "write it [[script:write]]");
    assert!(!notes.exists(), "a read-only session must not write");
    assert_write_refused(&adapter, &host, log_mark);
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
    let log_mark = provider_log_len(&host);
    second.turn(&session, "write it [[script:write]]");
    assert!(
        !host.workspace().join("notes.txt").exists(),
        "a reloaded plan session must still not write"
    );
    assert_write_refused(&second, &host, log_mark);
    second.finish();
}
