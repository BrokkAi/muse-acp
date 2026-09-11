//! Optional live-host smoke test.
//!
//! Skipped unless `MUSE_ACP_LIVE_HOST=1` is set, so CI stays hermetic. When
//! enabled, it drives one real `muse serve` through the adapter's handshake
//! and session lifecycle. Authentication is required; failures here mean the
//! host, auth, or protocol drifted — not a flaky unit test.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn adapter_bin() -> String {
    env!("CARGO_BIN_EXE_muse-acp").to_string()
}

struct LiveClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    frames: Arc<Mutex<Vec<String>>>,
    next_id: u64,
    #[allow(dead_code)]
    _dir: std::path::PathBuf,
}

impl LiveClient {
    fn spawn() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "muse-acp-live-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("live tmpdir");
        let mut child = Command::new(adapter_bin())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn adapter against real host");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
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
        Self {
            child,
            stdin,
            frames,
            next_id: 1,
            // keep the workspace alive for the session lifetime
            _dir: dir,
        }
    }

    fn req(&mut self, method: &str, params: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let frame = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}\n"
        );
        self.stdin.write_all(frame.as_bytes()).expect("write");
        self.stdin.flush().expect("flush");
        id
    }

    fn wait_for(&self, want: &str, timeout: Duration) -> String {
        let start = std::time::Instant::now();
        loop {
            if let Some(f) = self
                .frames
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.contains(want))
                .cloned()
            {
                return f;
            }
            if start.elapsed() > timeout {
                panic!("live host never produced {want:?}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn live_host_smoke() {
    if std::env::var("MUSE_ACP_LIVE_HOST").ok().as_deref() != Some("1") {
        eprintln!("skipped: set MUSE_ACP_LIVE_HOST=1 (and authenticate Muse) to run");
        return;
    }
    let mut c = LiveClient::spawn();
    let id = c.req("initialize", "{\"protocolVersion\":2}");
    let init = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(30));
    assert!(init.contains("\"result\""), "handshake failed: {init}");

    let dir = std::env::temp_dir().join("muse-acp-live-workspace");
    std::fs::create_dir_all(&dir).expect("workspace");
    let cwd = dir.to_str().unwrap().replace('\\', "\\\\");
    let id = c.req("session/new", &format!("{{\"cwd\":\"{cwd}\"}}"));
    let started = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(30));
    assert!(
        started.contains("\"result\""),
        "session/new failed (is `muse login` current?): {started}"
    );
    let sid = started
        .split("\"sessionId\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("sessionId")
        .to_string();

    let id = c.req("session/close", &format!("{{\"sessionId\":\"{sid}\"}}"));
    let closed = c.wait_for(&format!("\"id\":{id}"), Duration::from_secs(30));
    assert!(closed.contains("\"result\""), "close failed: {closed}");
    drop(c.stdin);
    let _ = c.child.wait();
}
