//! Black-box ACP tests against the real adapter binary with a fake MSP host.
//!
//! The fake host ([`fixtures/fake_serve.py`]) speaks just enough MSP JSON-RPC
//! for the adapter handshake, then plays one scripted scenario per run. Each
//! test points `MUSE_CLI` at the executable Python fixture (the adapter appends
//! `serve`, which the fixture ignores) and drives ACP over stdio. Requires
//! `python3` on PATH.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

fn fixture() -> String {
    let dir = env!("CARGO_MANIFEST_DIR");
    format!("{dir}/tests/fixtures/fake_serve.py")
}

fn adapter_bin() -> String {
    env!("CARGO_BIN_EXE_muse-acp").to_string()
}

struct Client {
    child: Child,
    stdin: std::process::ChildStdin,
    frames: Arc<Mutex<Vec<String>>>,
    next_id: u64,
    stderr_log: String,
    fake_log: String,
}

impl Client {
    /// The Python fixture ignores the `serve` argument the adapter appends.
    fn spawn(scenario: &str, extra_env: &[(&str, &str)]) -> Client {
        static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);
        let client_id = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "acp-fake-{}-{scenario}-{client_id}-{started_at}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let stderr_log = dir.join("adapter.stderr").to_str().unwrap().to_string();
        let err_file = std::fs::File::create(&stderr_log).expect("stderr log");
        let fake_log = dir.join("fake.log").to_str().unwrap().to_string();
        let mut cmd = Command::new(adapter_bin());
        cmd.env("MUSE_CLI", fixture());
        cmd.env("FAKE_SCENARIO", scenario);
        cmd.env("FAKE_LOG", &fake_log);
        cmd.env("FAKE_INPUT", format!("{fake_log}.input"));
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(err_file);
        let mut child = cmd.spawn().expect("spawn adapter");
        let stdin = child.stdin.take().expect("adapter stdin");
        let stdout: ChildStdout = child.stdout.take().expect("adapter stdout");
        let frames: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = frames.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let t = line.trim();
                        if !t.is_empty() {
                            writer.lock().unwrap().push(t.to_string());
                        }
                    }
                }
            }
        });
        Client {
            child,
            stdin,
            frames,
            next_id: 1,
            stderr_log,
            fake_log,
        }
    }

    fn req(&mut self, method: &str, params: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let frame = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}\n"
        );
        self.stdin.write_all(frame.as_bytes()).expect("write frame");
        self.stdin.flush().expect("flush");
        id
    }

    /// Raw bytes (e.g. malformed JSON probes).
    fn raw(&mut self, text: &str) {
        self.stdin.write_all(text.as_bytes()).expect("write raw");
        self.stdin.write_all(b"\n").expect("write nl");
        self.stdin.flush().expect("flush");
    }

    /// Client response to a server-initiated request (permission/elicitation).
    fn respond_error(&mut self, server_id: &str) {
        let frame = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":\"{server_id}\",\"error\":{{\"code\":-32800,\"message\":\"cancelled\"}}}}\n"
        );
        self.stdin.write_all(frame.as_bytes()).expect("write frame");
        self.stdin.flush().expect("flush");
    }

    /// Wait until the fake host logged turn input containing `want`.
    fn wait_input(&self, want: &str, timeout: Duration) {
        let path = format!("{}.input", self.fake_log);
        let start = Instant::now();
        loop {
            if let Ok(t) = std::fs::read_to_string(&path)
                && t.contains(want)
            {
                return;
            }
            if start.elapsed() > timeout {
                panic!("fake host never received input containing {want:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait until the fake host observed a given MSP method.
    fn wait_log(&self, want: &str, timeout: Duration) {
        let start = Instant::now();
        loop {
            if let Ok(t) = std::fs::read_to_string(&self.fake_log)
                && t.lines().any(|l| l == want)
            {
                return;
            }
            if start.elapsed() > timeout {
                panic!("fake host never saw {want:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Raw client->server notification (e.g. initialized).
    fn notify(&mut self, method: &str, params: &str) {
        let frame =
            format!("{{\"jsonrpc\":\"2.0\",\"method\":\"{method}\",\"params\":{params}}}\n");
        self.stdin.write_all(frame.as_bytes()).expect("write frame");
        self.stdin.flush().expect("flush");
    }

    fn wait_for(&self, want: &str, timeout: Duration) -> String {
        let start = Instant::now();
        loop {
            {
                let frames = self.frames.lock().unwrap();
                if let Some(f) = frames.iter().find(|f| f.contains(want)) {
                    return f.clone();
                }
            }
            if start.elapsed() > timeout {
                let tail = std::fs::read_to_string(&self.stderr_log)
                    .unwrap_or_default()
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                panic!(
                    "timed out waiting for {want:?}; got:\n{}\nadapter stderr:\n{tail}",
                    self.frames.lock().unwrap().join("\n")
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// New session via initialize + initialized + session/new. Returns the
    /// ACP session id.
    fn new_session(&mut self, ver: u64, extra_init: &str) -> String {
        let init = self.req(
            "initialize",
            &format!("{{\"protocolVersion\":{ver}{extra_init}}}"),
        );
        let frame = self.wait_for(&format!("\"id\":{init}"), Duration::from_secs(15));
        assert!(frame.contains("\"result\""), "init failed: {frame}");
        assert!(
            frame.contains(&format!("\"protocolVersion\":{ver}")),
            "wrong version echoed: {frame}"
        );
        if ver == 1 {
            assert!(
                frame.contains("\"agentCapabilities\"")
                    && frame.contains("\"agentInfo\"")
                    && frame.contains("\"mcpCapabilities\":{")
                    && frame.contains("\"sessionCapabilities\":"),
                "invalid v1 initialize shape: {frame}"
            );
        } else {
            assert!(
                frame.contains("\"capabilities\":{")
                    && frame.contains("\"info\":")
                    && frame.contains("\"steering\":{\"supported\":true}"),
                "invalid v2 initialize shape: {frame}"
            );
        }
        self.notify("initialized", "{}");
        let dir = std::path::Path::new(&self.fake_log)
            .parent()
            .expect("fake log parent")
            .join("workspace");
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let cwd = dir.to_str().unwrap().replace('\\', "\\\\");
        let id = self.req("session/new", &format!("{{\"cwd\":\"{cwd}\"}}"));
        let frame = self.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
        assert!(frame.contains("\"result\""), "session/new failed: {frame}");
        extract_str(&frame, "sessionId").expect("sessionId in result")
    }

    fn prompt(&mut self, sid: &str, text: &str) -> u64 {
        self.req(
            "session/prompt",
            &format!(
                "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}"
            ),
        )
    }

    fn finish(mut self) {
        drop(self.stdin);
        let status = self
            .child
            .wait_timeout(Duration::from_secs(10))
            .expect("wait")
            .expect("adapter exited");
        assert!(status.success(), "adapter exit: {status}");
    }
}

trait WaitTimeout {
    fn wait_timeout(&mut self, t: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl WaitTimeout for Child {
    fn wait_timeout(&mut self, t: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = Instant::now();
        loop {
            match self.try_wait()? {
                Some(s) => return Ok(Some(s)),
                None if start.elapsed() > t => return Ok(None),
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}

/// Extract `"key":"value"` (first string occurrence) from a JSON frame.
fn extract_str(frame: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let i = frame.find(&needle)? + needle.len();
    let rest = &frame[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[test]
fn v1_prompt_happy_path_ends_end_turn() {
    let mut c = Client::spawn("happy", &[]);
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "hi");
    // The assistant message must stream before the terminal result.
    let chunk = c.wait_for("agent_message_chunk", Duration::from_secs(15));
    assert!(chunk.contains(&sid), "chunk for our session: {chunk}");
    assert!(
        chunk.contains("hello from fake host"),
        "message text bridged: {chunk}"
    );
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(
        done.contains("\"stopReason\":\"end_turn\""),
        "terminal result: {done}"
    );
    c.finish();
}

#[test]
fn v1_failed_terminal_is_an_error_never_completed() {
    let mut c = Client::spawn("failed", &[]);
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "hi");
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    // A failed turn must surface as an error, never as stopReason completed.
    assert!(done.contains("\"error\""), "failed turn errors: {done}");
    assert!(
        !done.contains("stopReason"),
        "no fake completion metadata: {done}"
    );
    c.finish();
}

#[test]
fn v2_prompt_accepts_then_echoes_and_idles() {
    let mut c = Client::spawn("happy", &[]);
    let sid = c.new_session(2, "");
    let pid = c.prompt(&sid, "hi");
    // v2 MUST: empty accepted response first...
    let accepted = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(accepted.contains("\"result\":{}"), "accepted: {accepted}");
    // ...then the user-message echo, then running, then idle(end_turn).
    let echo = c.wait_for("user_message", Duration::from_secs(15));
    assert!(echo.contains(&sid), "echo for our session: {echo}");
    let running = c.wait_for("\"running\"", Duration::from_secs(15));
    assert!(running.contains(&sid), "running: {running}");
    let idle = c.wait_for("\"idle\"", Duration::from_secs(15));
    assert!(
        idle.contains("end_turn"),
        "v2 completion metadata never omitted: {idle}"
    );
    c.finish();
}

#[test]
fn tool_call_completion_bridges_with_result_text() {
    let mut c = Client::spawn("tool", &[]);
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "read it");
    // Completion carries create+update in one tool_call frame with status.
    let created = c.wait_for("tool_call", Duration::from_secs(15));
    assert!(
        created.contains(&sid),
        "tool_call for our session: {created}"
    );
    assert!(
        created.contains("file bytes"),
        "result text bridged: {created}"
    );
    assert!(
        created.contains("\"status\":\"completed\""),
        "terminal status present: {created}"
    );
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(done.contains("end_turn"), "terminal: {done}");
    c.finish();
}

#[test]
fn approval_preserves_all_choices_with_deny_option() {
    let mut c = Client::spawn("approval", &[]);
    let sid = c.new_session(1, "");
    let _pid = c.prompt(&sid, "do it");
    let perm = c.wait_for("request_permission", Duration::from_secs(15));
    assert!(perm.contains(&sid), "permission for our session: {perm}");
    assert!(perm.contains("c-allow"), "allow choice kept: {perm}");
    assert!(perm.contains("c-deny"), "deny choice kept: {perm}");
    assert!(
        perm.contains("deny"),
        "deny-safe option kind present: {perm}"
    );
    c.finish();
}

#[test]
fn user_input_options_reach_the_client() {
    let mut c = Client::spawn("questions", &[]);
    let sid = c.new_session(2, ",\"capabilities\":{\"elicitation\":{\"form\":{}}}");
    let _pid = c.prompt(&sid, "ask me");
    // The bridge announces requires_action, then offers the host options as
    // an elicitation/create enum schema (there is no user_input update).
    let state = c.wait_for("requires_action", Duration::from_secs(15));
    assert!(state.contains(&sid), "state for our session: {state}");
    let elicit = c.wait_for("elicitation/create", Duration::from_secs(15));
    assert!(
        elicit.contains(&sid),
        "elicitation for our session: {elicit}"
    );
    assert!(elicit.contains("Alpha"), "option Alpha bridged: {elicit}");
    assert!(elicit.contains("Beta"), "option Beta bridged: {elicit}");
    let log = c.frames.lock().unwrap().join("\n");
    assert!(
        log.find("requires_action").unwrap() < log.find("elicitation/create").unwrap(),
        "state precedes the offer"
    );
    c.finish();
}

#[test]
fn false_elicitation_capability_is_not_treated_as_supported() {
    let mut c = Client::spawn("questions", &[]);
    let sid = c.new_session(2, ",\"capabilities\":{\"elicitation\":{\"form\":false}}");
    let _pid = c.prompt(&sid, "ask me");
    c.wait_log("userInput/cancel", Duration::from_secs(15));
    let frames = c.frames.lock().unwrap().join("\n");
    assert!(
        !frames.contains("elicitation/create"),
        "false form capability must not enable elicitation: {frames}"
    );
    c.finish();
}

#[test]
fn v2_failed_terminal_yields_failed_stop() {
    let mut c = Client::spawn("failed", &[]);
    let sid = c.new_session(2, "");
    let _pid = c.prompt(&sid, "hi");
    let idle = c.wait_for("\"idle\"", Duration::from_secs(15));
    assert!(idle.contains(&sid), "idle for our session: {idle}");
    assert!(
        idle.contains("_failed"),
        "failed terminal keeps its stop reason: {idle}"
    );
    c.finish();
}

#[test]
fn turn_unqueued_settles_prompt_cancelled() {
    let mut c = Client::spawn("unqueued", &[]);
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "hi");
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(
        done.contains("\"stopReason\":\"cancelled\""),
        "reclaimed turn settles: {done}"
    );
    c.finish();
}

#[test]
fn queued_turns_share_running_until_drained() {
    let mut c = Client::spawn("queued", &[]);
    let sid = c.new_session(2, "");
    let p1 = c.prompt(&sid, "one");
    let a1 = c.wait_for(&format!("\"id\":{p1}"), Duration::from_secs(15));
    assert!(a1.contains("\"result\":{}"), "first accepted: {a1}");
    let p2 = c.prompt(&sid, "two");
    let a2 = c.wait_for(&format!("\"id\":{p2}"), Duration::from_secs(15));
    assert!(a2.contains("\"result\":{}"), "second accepted: {a2}");
    let idle = c.wait_for("\"idle\"", Duration::from_secs(15));
    assert!(idle.contains("end_turn"), "drains with metadata: {idle}");
    let (idles, log) = {
        let frames = c.frames.lock().unwrap();
        let idles = frames.iter().filter(|f| f.contains("\"idle\"")).count();
        (idles, frames.join("\n"))
    };
    assert_eq!(idles, 1, "exactly one idle transition");
    assert!(
        log.rfind("\"running\"").unwrap() < log.find("\"idle\"").unwrap(),
        "running persists until the last turn drains"
    );
    c.finish();
}

#[test]
fn session_close_cancels_in_flight() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "hi");
    c.wait_log("turn/start", Duration::from_secs(15));
    let cid = c.req("session/close", &format!("{{\"sessionId\":\"{sid}\"}}"));
    let closed = c.wait_for(&format!("\"id\":{cid}"), Duration::from_secs(15));
    assert!(closed.contains("\"result\":{}"), "close ack: {closed}");
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(
        done.contains("\"stopReason\":\"cancelled\""),
        "in-flight prompt settles: {done}"
    );
    c.finish();
}

#[test]
fn approval_request_without_notification_is_bridged() {
    // Reissued server-initiated approval/request (no notification): the
    // reader acks it AND the bridge still reaches the client.
    let mut c = Client::spawn("approval_req", &[]);
    let sid = c.new_session(1, "");
    let _pid = c.prompt(&sid, "do it");
    let perm = c.wait_for("request_permission", Duration::from_secs(15));
    assert!(perm.contains(&sid), "permission for our session: {perm}");
    assert!(perm.contains("c-allow"), "allow choice kept: {perm}");
    assert!(perm.contains("c-deny"), "deny choice kept: {perm}");
    c.finish();
}

#[test]
fn session_load_replays_history() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(1, "");
    let cwd = std::path::Path::new(&c.fake_log)
        .parent()
        .expect("fake log parent")
        .join("workspace");
    let lid = c.req(
        "session/load",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"cwd\":\"{}\"}}",
            cwd.to_str().expect("utf8 cwd").replace('\\', "\\\\")
        ),
    );
    let load = c.wait_for(&format!("\"id\":{lid}"), Duration::from_secs(15));
    assert!(load.contains("\"result\""), "load ok: {load}");
    let log = c.frames.lock().unwrap().join("\n");
    let end = log.find(&format!("\"id\":{lid}")).unwrap();
    let replay = &log[..end];
    assert!(replay.contains("old question"), "user history replayed");
    assert!(replay.contains("old answer"), "agent history replayed");
    assert!(replay.contains("old bytes"), "tool history replayed");
    c.finish();
}

#[test]
fn host_default_mode_is_reflected() {
    let mut c = Client::spawn("quiet", &[("FAKE_MODE", "allowAll")]);
    let _sid = c.new_session(2, "");
    let log = c.frames.lock().unwrap().join("\n");
    let cfg = log
        .lines()
        .find(|l| l.contains("configOptions"))
        .expect("config in session/new")
        .to_string();
    assert!(
        cfg.contains("\"currentValue\":\"auto\""),
        "host allowAll shows as auto, not ask: {cfg}"
    );
    c.finish();
}

#[test]
fn set_config_option_returns_full_state() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(2, "");
    let id = c.req(
        "session/set_config_option",
        &format!("{{\"sessionId\":\"{sid}\",\"configId\":\"mode\",\"value\":\"ask\"}}"),
    );
    let done = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
    assert!(done.contains("\"configOptions\""), "full state: {done}");
    assert!(
        done.contains("\"currentValue\":\"ask\""),
        "updated value reflected: {done}"
    );
    c.finish();
}

#[test]
fn reasoning_effort_is_selected_and_sent_to_msp() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(2, "");
    let initial = c.frames.lock().unwrap().join("\n");
    assert!(
        initial.contains("\"configId\":\"reasoning_effort\"")
            && initial.contains("\"currentValue\":\"medium\""),
        "reasoning selector is initialized: {initial}"
    );

    let invalid_id = c.req(
        "session/set_config_option",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"configId\":\"reasoning_effort\",\"value\":\"extreme\"}}"
        ),
    );
    let invalid = c.wait_for(&format!("\"id\":{invalid_id}"), Duration::from_secs(15));
    assert!(
        invalid.contains("\"error\""),
        "invalid effort accepted: {invalid}"
    );

    let set_id = c.req(
        "session/set_config_option",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"configId\":\"reasoning_effort\",\"value\":\"high\"}}"
        ),
    );
    let set = c.wait_for(&format!("\"id\":{set_id}"), Duration::from_secs(15));
    assert!(
        set.contains("\"currentValue\":\"high\""),
        "updated reasoning value reflected: {set}"
    );

    let _pid = c.prompt(&sid, "think carefully");
    c.wait_input("\"reasoningEffort\": \"high\"", Duration::from_secs(15));
    c.finish();
}

#[test]
fn steering_injects_into_the_exact_active_turn() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(2, "");
    let prompt_id = c.prompt(&sid, "start");
    c.wait_for(&format!("\"id\":{prompt_id}"), Duration::from_secs(15));

    let steer_id = c.req(
        "_session/steering",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"change course\"}}]}}"
        ),
    );
    let response = c.wait_for(&format!("\"id\":{steer_id}"), Duration::from_secs(15));
    assert!(response.contains("\"outcome\":\"injected\""), "{response}");
    c.wait_log("turn/steer", Duration::from_secs(15));
    c.wait_input("\"expectedTurnId\": \"turn-1\"", Duration::from_secs(15));

    let frames = c.frames.lock().unwrap();
    let response_pos = frames
        .iter()
        .position(|frame| frame.contains(&format!("\"id\":{steer_id}")))
        .expect("steering response");
    let echo_pos = frames
        .iter()
        .position(|frame| frame.contains("change course"))
        .expect("steering echo");
    assert!(
        response_pos < echo_pos,
        "response must precede the user echo"
    );
    drop(frames);
    c.finish();
}

#[test]
fn steering_idle_policy_is_race_safe() {
    let mut c = Client::spawn("happy", &[]);
    let sid = c.new_session(2, "");
    let invalid_id = c.req(
        "_session/steering",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"bad policy\"}}],\"_meta\":{{\"steering\":{{\"idleBehavior\":\"queue\"}}}}}}"
        ),
    );
    let invalid = c.wait_for(&format!("\"id\":{invalid_id}"), Duration::from_secs(15));
    assert!(
        invalid.contains("\"error\""),
        "invalid policy accepted: {invalid}"
    );

    let required_id = c.req(
        "_session/steering",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"only if busy\"}}],\"_meta\":{{\"steering\":{{\"idleBehavior\":\"promptRequired\"}}}}}}"
        ),
    );
    let required = c.wait_for(&format!("\"id\":{required_id}"), Duration::from_secs(15));
    assert!(
        required.contains("\"outcome\":\"promptRequired\"")
            && required.contains("\"reason\":\"noRunningTurn\""),
        "{required}"
    );

    let fallback_id = c.req(
        "_session/steering",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"start safely\"}}]}}"
        ),
    );
    let fallback = c.wait_for(&format!("\"id\":{fallback_id}"), Duration::from_secs(15));
    assert!(
        fallback.contains("\"outcome\":\"startedNewTurn\""),
        "{fallback}"
    );
    c.wait_input("\"ifBusy\": \"steer\"", Duration::from_secs(15));
    let idle = c.wait_for("\"idle\"", Duration::from_secs(15));
    assert!(idle.contains("end_turn"), "steered start completes: {idle}");
    c.finish();
}

#[test]
fn steering_targets_a_turn_rehydrated_by_resume() {
    let mut c = Client::spawn("resume_active", &[]);
    let init = c.req("initialize", "{\"protocolVersion\":2}");
    c.wait_for(&format!("\"id\":{init}"), Duration::from_secs(15));
    c.notify("initialized", "{}");
    let resume_id = c.req(
        "session/resume",
        "{\"sessionId\":\"existing-session\",\"cwd\":\"/tmp\"}",
    );
    let resumed = c.wait_for(&format!("\"id\":{resume_id}"), Duration::from_secs(15));
    assert!(resumed.contains("\"result\""), "resume failed: {resumed}");

    let steer_id = c.req(
        "_session/steering",
        "{\"sessionId\":\"existing-session\",\"prompt\":[{\"type\":\"text\",\"text\":\"continue differently\"}]}",
    );
    let steered = c.wait_for(&format!("\"id\":{steer_id}"), Duration::from_secs(15));
    assert!(steered.contains("\"outcome\":\"injected\""), "{steered}");
    c.wait_input(
        "\"expectedTurnId\": \"turn-resumed\"",
        Duration::from_secs(15),
    );
    c.finish();
}

#[test]
fn steering_is_rejected_for_v1_connections() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(1, "");
    let steer_id = c.req(
        "_session/steering",
        &format!("{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"no\"}}]}}"),
    );
    let response = c.wait_for(&format!("\"id\":{steer_id}"), Duration::from_secs(15));
    assert!(
        response.contains("\"code\":-32601"),
        "v1 steering must be unavailable: {response}"
    );
    c.finish();
}

#[test]
fn all_approve_cancellation_fails_closed() {
    // Every choice approves and the client cancels: no approval/decide may
    // be sent; the turn is cancelled instead.
    let mut c = Client::spawn("approval_hang", &[("FAKE_APPROVAL", "all-approve")]);
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "do it");
    let perm = c.wait_for("request_permission", Duration::from_secs(15));
    assert!(perm.contains("c-yes"), "all-approve choices shown: {perm}");
    let perm_id = extract_str(&perm, "id").expect("perm request id");
    c.respond_error(&perm_id);
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(
        done.contains("\"stopReason\":\"cancelled\""),
        "turn cancelled, never approved: {done}"
    );
    c.wait_log("turn/cancel", Duration::from_secs(15));
    // Give the adapter a beat to (not) send approval/decide, then check.
    std::thread::sleep(Duration::from_millis(500));
    let seen = std::fs::read_to_string(&c.fake_log).unwrap_or_default();
    assert!(
        !seen.lines().any(|l| l == "approval/decide"),
        "no approving decide was sent; host saw:\n{seen}"
    );
    c.finish();
}

#[test]
fn resource_link_inlines_workspace_text() {
    let mut c = Client::spawn("quiet", &[]);
    let dir = std::env::temp_dir().join(format!("acp-res-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    // Percent-encoded name exercises URI decoding + workspace confinement.
    std::fs::write(dir.join("sp ace.txt"), "secret file text").expect("write");
    let uri = format!("file://{}", dir.join("sp%20ace.txt").to_str().unwrap());
    let init = c.req("initialize", "{\"protocolVersion\":1}");
    c.wait_for(&format!("\"id\":{init}"), Duration::from_secs(15));
    c.notify("initialized", "{}");
    let cwd = dir.to_str().unwrap();
    let id = c.req("session/new", &format!("{{\"cwd\":\"{cwd}\"}}"));
    c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
    // Re-read the session id from the accumulated frames.
    let log = c.frames.lock().unwrap().join("\n");
    let sid = extract_str(
        log.lines()
            .find(|l| l.contains(&format!("\"id\":{id}")))
            .unwrap(),
        "sessionId",
    )
    .expect("sessionId");
    let pid = c.req(
        "session/prompt",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"resource_link\",\"uri\":\"{uri}\",\"name\":\"notes\"}}]}}"
        ),
    );
    // Prompt accepted (v1 answers at the terminal, which never comes in
    // quiet mode); the host must have received the inlined text.
    let _pid = pid;
    c.wait_input("secret file text", Duration::from_secs(15));
    c.finish();
}

#[test]
fn embedded_blob_resource_is_rejected() {
    let mut c = Client::spawn("quiet", &[]);
    let sid = c.new_session(1, "");
    let id = c.req(
        "session/prompt",
        &format!(
            "{{\"sessionId\":\"{sid}\",\"prompt\":[{{\"type\":\"resource\",\"resource\":{{\"uri\":\"u\",\"mimeType\":\"application/octet-stream\",\"blob\":\"AA==\"}}}}]}}"
        ),
    );
    let done = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
    assert!(
        done.contains("\"error\"") && done.contains("blob"),
        "blob rejected explicitly: {done}"
    );
    c.finish();
}

#[test]
fn malformed_json_is_rejected_and_survived() {
    let mut c = Client::spawn("happy", &[]);
    c.raw("{\"jsonrpc\":\"2.0\",\"id\":01,\"method\":\"initialize\",\"params\":{}}");
    let err = c.wait_for("-32700", Duration::from_secs(15));
    assert!(err.contains("\"error\""), "parse error: {err}");
    // The adapter is still alive for well-formed frames.
    let sid = c.new_session(1, "");
    let pid = c.prompt(&sid, "hi");
    let done = c.wait_for(&format!("\"id\":{pid}"), Duration::from_secs(15));
    assert!(done.contains("end_turn"), "terminal: {done}");
    c.finish();
}

#[test]
fn unsupported_or_invalid_session_roots_are_rejected() {
    let mut c = Client::spawn("happy", &[]);
    let init = c.req("initialize", "{\"protocolVersion\":2}");
    c.wait_for(&format!("\"id\":{init}"), Duration::from_secs(15));
    c.notify("initialized", "{}");

    for params in [
        "{\"cwd\":\"relative\"}",
        "{\"cwd\":\"/tmp\",\"mcpServers\":[{\"name\":\"x\"}]}",
        "{\"cwd\":\"/tmp\",\"additionalDirectories\":[\"/var/tmp\"]}",
    ] {
        let id = c.req("session/new", params);
        let frame = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
        assert!(frame.contains("\"error\""), "request must fail: {frame}");
    }

    let seen = std::fs::read_to_string(&c.fake_log).unwrap_or_default();
    assert!(
        !seen.lines().any(|line| line == "session/start"),
        "invalid requests must not reach the host: {seen}"
    );
    c.finish();
}

#[test]
fn approval_mode_mismatch_fails_session_new() {
    // Operator asked for auto but the host folded promptUnmatched: the
    // session must fail, not silently run under the wrong posture.
    let mut c = Client::spawn("quiet", &[("MUSE_APPROVAL_MODE", "auto")]);
    let init = c.req("initialize", "{\"protocolVersion\":1}");
    let frame = c.wait_for(&format!("\"id\":{init}"), Duration::from_secs(15));
    assert!(frame.contains("\"result\""), "init failed: {frame}");
    c.notify("initialized", "{}");
    let id = c.req("session/new", "{\"cwd\":\"/tmp\"}");
    let frame = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
    assert!(
        frame.contains("not applied"),
        "mismatch must fail loudly: {frame}"
    );
    c.finish();
}

#[test]
fn bogus_approval_mode_fails_session_new_atomically() {
    let mut c = Client::spawn("happy", &[("MUSE_APPROVAL_MODE", "bogus-mode")]);
    let init = c.req("initialize", "{\"protocolVersion\":1}");
    let frame = c.wait_for(&format!("\"id\":{init}"), Duration::from_secs(15));
    assert!(frame.contains("\"result\""), "init failed: {frame}");
    c.notify("initialized", "{}");
    let id = c.req("session/new", "{\"cwd\":\"/tmp\"}");
    let frame = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(15));
    assert!(
        frame.contains("\"error\""),
        "bogus mode must fail, not fall back: {frame}"
    );
    c.finish();
}
