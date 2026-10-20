//! The `muse serve` child process: bounded stderr capture, termination, and
//! the stdin-closed exit probe.

use std::io::{BufReader, Read};
use std::process::{Child, ChildStderr, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::client::MspEvent;
use crate::config::LaunchConfig;
use crate::error::LaunchError;
use crate::exit::{ExitClassification, classify_status};

pub(crate) const STDERR_MAX_BYTES: usize = 8 * 1024;
pub(crate) const STDERR_MAX_LINES: usize = 100;
pub(crate) const POLL: Duration = Duration::from_millis(10);
/// How long a reap or close waits for the stderr reader to reach EOF after
/// the child exited, so the captured tail includes the last lines.
const STDERR_DRAIN: Duration = Duration::from_millis(250);

/// The most recent host stderr, bounded by lines and bytes. It is evidence
/// for exit diagnostics only and is never parsed as protocol input.
#[derive(Clone, Default)]
pub(crate) struct StderrTail {
    text: Arc<Mutex<String>>,
}

impl StderrTail {
    pub(crate) fn push(&self, bytes: &[u8]) {
        let mut text = self.text.lock().unwrap_or_else(|p| p.into_inner());
        text.push_str(&String::from_utf8_lossy(bytes));
        let mut lines = text.matches('\n').count();
        if !text.ends_with('\n') && !text.is_empty() {
            lines += 1;
        }
        while lines > STDERR_MAX_LINES {
            let Some(newline) = text.find('\n') else {
                break;
            };
            text.drain(..=newline);
            lines -= 1;
        }
        if text.len() > STDERR_MAX_BYTES {
            let mut start = text.len() - STDERR_MAX_BYTES;
            while start < text.len() && !text.is_char_boundary(start) {
                start += 1;
            }
            text.drain(..start);
        }
    }

    pub(crate) fn snapshot(&self) -> String {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Set once the stderr reader reached EOF.
#[derive(Clone, Default)]
pub(crate) struct StderrDone(Arc<(Mutex<bool>, Condvar)>);

impl StderrDone {
    fn finish(&self) {
        let (done, cv) = &*self.0;
        *done.lock().unwrap_or_else(|p| p.into_inner()) = true;
        cv.notify_all();
    }

    pub(crate) fn wait(&self) {
        let (done, cv) = &*self.0;
        let guard = done.lock().unwrap_or_else(|p| p.into_inner());
        let _ = cv.wait_timeout_while(guard, STDERR_DRAIN, |finished| !*finished);
    }
}

/// Read the child's stderr into `tail` until EOF, optionally also sending
/// each line as an event.
pub(crate) fn drain_stderr(
    stderr: ChildStderr,
    tail: StderrTail,
    events: Option<Sender<MspEvent>>,
) -> StderrDone {
    let done = StderrDone::default();
    let reader_done = done.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf = [0u8; 4096];
        let mut line = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.push(&buf[..n]);
                    if let Some(events) = &events {
                        forward_lines(&mut line, &buf[..n], events);
                    }
                }
            }
        }
        if let Some(events) = &events
            && !line.is_empty()
        {
            send_line(&line, events);
        }
        reader_done.finish();
    });
    done
}

fn forward_lines(line: &mut Vec<u8>, bytes: &[u8], events: &Sender<MspEvent>) {
    for &byte in bytes {
        if byte == b'\n' {
            send_line(line, events);
            line.clear();
        } else if line.len() < STDERR_MAX_BYTES {
            line.push(byte);
        }
    }
}

fn send_line(line: &[u8], events: &Sender<MspEvent>) {
    let text = String::from_utf8_lossy(line);
    let _ = events.send(MspEvent::Stderr {
        line: text.trim_end_matches('\r').to_string(),
    });
}

/// Kill the child. Windows batch launchers keep the actual host in a
/// descendant process, so the tree is killed before the launcher; otherwise
/// a descendant could retain the pipe and execute a command after its
/// caller has received a timeout.
pub(crate) fn terminate_child(child: &mut Child) {
    #[cfg(windows)]
    if matches!(child.try_wait(), Ok(None))
        && let Ok(mut killer) = std::process::Command::new("taskkill.exe")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match killer.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                Ok(None) => {
                    let _ = killer.kill();
                    let _ = killer.wait();
                    break;
                }
            }
        }
    }
    let _ = child.kill();
}

/// Kill the child and wait at most `timeout` for it to be reaped.
pub(crate) fn kill_and_reap(child: &mut Child, timeout: Duration) {
    terminate_child(child);
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => return,
        }
    }
}

/// Run the configured host with stdin and stdout closed, send no protocol
/// frames, and classify how it exits. `Ok(None)` means it was still running
/// after `timeout` and was killed. Useful for support diagnostics, such as
/// telling a disabled serve surface (exit code 5) from a bad configuration
/// (exit code 3).
pub fn probe_exit(
    config: &LaunchConfig,
    timeout: Duration,
) -> Result<Option<ExitClassification>, LaunchError> {
    let mut child = config
        .command()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| LaunchError::Spawn {
            program: config.program.to_string_lossy().into_owned(),
            error,
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| LaunchError::Startup("serve support probe: no stderr".to_string()))?;
    let tail = StderrTail::default();
    let done = drain_stderr(stderr, tail.clone(), None);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            LaunchError::Startup(format!("serve support probe wait failed: {error}"))
        })? {
            done.wait();
            return Ok(Some(classify_status(&status, tail.snapshot())));
        }
        if started.elapsed() >= timeout {
            terminate_child(&mut child);
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_tail_is_bounded_and_keeps_recent_evidence() {
        let tail = StderrTail::default();
        for n in 0..150 {
            tail.push(format!("diagnostic-{n:03}: {}\n", "x".repeat(100)).as_bytes());
        }
        let text = tail.snapshot();
        assert!(text.len() <= STDERR_MAX_BYTES, "{} bytes", text.len());
        assert!(text.lines().count() <= STDERR_MAX_LINES);
        assert!(text.contains("diagnostic-149"));
        assert!(!text.contains("diagnostic-000"));
    }

    #[test]
    fn stderr_tail_trims_at_a_character_boundary() {
        let tail = StderrTail::default();
        tail.push("é".repeat(STDERR_MAX_BYTES).as_bytes());
        let text = tail.snapshot();
        assert!(text.len() <= STDERR_MAX_BYTES);
        assert!(text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn stderr_lines_are_cut_and_split() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut line = Vec::new();
        forward_lines(&mut line, b"one\r\ntw", &tx);
        forward_lines(&mut line, b"o\n", &tx);
        forward_lines(&mut line, &vec![b'x'; STDERR_MAX_BYTES + 10], &tx);
        forward_lines(&mut line, b"\n", &tx);
        let lines: Vec<String> = rx
            .try_iter()
            .map(|event| match event {
                MspEvent::Stderr { line } => line,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(lines[0], "one");
        assert_eq!(lines[1], "two");
        assert_eq!(lines[2].len(), STDERR_MAX_BYTES);
    }
}
