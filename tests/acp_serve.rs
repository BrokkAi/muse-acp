//! Black-box ACP tests against the real adapter binary with a fake MSP host.
//!
//! The fake host ([`fixtures/fake_serve.py`]) speaks just enough MSP JSON-RPC
//! for the adapter handshake, then plays one scripted scenario per run. Each
//! test spawns the adapter with `MUSE_CLI=python3 <fixture>` (the adapter
//! always appends `serve`, which the fixture ignores) and drives ACP over
//! stdio. Requires `python3` on PATH.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
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
}

impl Client {
    /// The adapter always runs `$MUSE_CLI serve ...`, so point MUSE_CLI at a
    /// generated wrapper that execs the Python fixture regardless of argv.
    fn spawn(scenario: &str, extra_env: &[(&str, &str)]) -> Client {
        let dir =
            std::env::temp_dir().join(format!("acp-fake-{}-{}", std::process::id(), scenario));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let wrap = dir.join("fake-muse.sh");
        std::fs::write(
            &wrap,
            format!(
                "#!/bin/sh\nexec \"{}\" \"{}\" \"$@\"\n",
                which_python3(),
                fixture()
            ),
        )
        .expect("wrapper");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&wrap).expect("meta").permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&wrap, p).expect("chmod");
        }
        let stderr_log = dir.join("adapter.stderr").to_str().unwrap().to_string();
        let err_file = std::fs::File::create(&stderr_log).expect("stderr log");
        let mut cmd = Command::new(adapter_bin());
        cmd.env("MUSE_CLI", &wrap);
        cmd.env("FAKE_SCENARIO", scenario);
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
        self.notify("initialized", "{}");
        let dir = std::env::temp_dir().join(format!("acp-test-{}", std::process::id()));
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

fn which_python3() -> String {
    let out = Command::new("sh")
        .arg("-c")
        .arg("command -v python3")
        .output()
        .expect("probe python3");
    assert!(
        out.status.success(),
        "these tests need python3 on PATH for the fake MSP host"
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
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
