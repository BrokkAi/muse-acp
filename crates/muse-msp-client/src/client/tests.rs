//! Connection tests over an in-memory transport. Process-level behavior
//! (exit classification, kill after the grace period, stderr capture) is in
//! `tests/process.rs`.

use super::*;
use crate::config::{Capabilities, ClientInfo};
use std::sync::mpsc::TryRecvError;

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn stdout_frames_are_bounded_before_parsing() {
    let mut frame = Vec::new();
    let mut ok = io::Cursor::new(b"1234567\n".to_vec());
    assert_eq!(
        read_frame(&mut ok, &mut frame, 8).unwrap(),
        FrameRead::Complete
    );
    assert_eq!(frame, b"1234567\n");

    let mut too_large = io::Cursor::new(b"12345678\n".to_vec());
    assert_eq!(
        read_frame(&mut too_large, &mut frame, 8).unwrap(),
        FrameRead::TooLarge
    );

    let mut eof = io::Cursor::new(Vec::<u8>::new());
    assert_eq!(read_frame(&mut eof, &mut frame, 8).unwrap(), FrameRead::Eof);
}

/// The host side of an in-memory transport: bytes it sends arrive on the
/// client's reader, and every frame the client flushes arrives here.
struct Peer {
    to_client: Option<Sender<Vec<u8>>>,
    from_client: Receiver<Vec<u8>>,
}

impl Peer {
    fn send(&self, text: &str) {
        self.send_bytes(format!("{text}\n").as_bytes());
    }

    fn send_bytes(&self, bytes: &[u8]) {
        self.to_client
            .as_ref()
            .expect("peer open")
            .send(bytes.to_vec())
            .expect("client reader alive");
    }

    fn recv_text(&self) -> String {
        let bytes = self.from_client.recv_timeout(WAIT).expect("client frame");
        let text = String::from_utf8(bytes).expect("UTF-8 frame");
        assert!(text.ends_with('\n'), "frame is one complete line: {text:?}");
        assert_eq!(
            text.matches('\n').count(),
            1,
            "one frame per flush: {text:?}"
        );
        text.trim_end().to_string()
    }

    fn recv(&self) -> J {
        parse_json(&self.recv_text()).expect("JSON frame")
    }

    /// True when no frame arrives within 100ms (or the writer is gone).
    fn nothing_sent(&self) -> bool {
        self.from_client
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    }

    fn close(&mut self) {
        self.to_client.take();
    }
}

struct PeerReader {
    incoming: Receiver<Vec<u8>>,
    buffer: Vec<u8>,
}

impl Read for PeerReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.buffer.is_empty() {
            match self.incoming.recv() {
                Ok(bytes) => self.buffer = bytes,
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buffer.len());
        out[..n].copy_from_slice(&self.buffer[..n]);
        self.buffer.drain(..n);
        Ok(n)
    }
}

/// Collects a frame until flush. With a frame budget, writes after that many
/// frames block until the terminate hook drops the gate.
struct PeerWriter {
    outgoing: Sender<Vec<u8>>,
    buffer: Vec<u8>,
    frames_left: Option<usize>,
    gate: Option<Receiver<()>>,
}

impl Write for PeerWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.frames_left == Some(0)
            && let Some(gate) = &self.gate
        {
            let _ = gate.recv();
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "peer gone"));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(left) = self.frames_left.as_mut() {
            *left = left.saturating_sub(1);
        }
        self.outgoing
            .send(std::mem::take(&mut self.buffer))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "peer closed"))
    }
}

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<String>>>);

impl Recorded {
    fn push(&self, line: String) {
        self.0.lock().unwrap().push(line);
    }

    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    fn wait_for(&self, line: &str) {
        let until = Instant::now() + WAIT;
        while !self.lines().iter().any(|seen| seen == line) {
            assert!(
                Instant::now() < until,
                "never saw {line:?} in {:?}",
                self.lines()
            );
            std::thread::sleep(POLL);
        }
    }
}

/// A client over an in-memory transport, recording diagnostics and watchdog
/// steps.
struct Harness {
    client: MspClient,
    events: Receiver<MspEvent>,
    peer: Peer,
    diagnostics: Recorded,
    watchdog: Recorded,
    terminated: Arc<AtomicBool>,
}

fn harness(options: ClientOptions) -> Harness {
    harness_with(options, None)
}

fn harness_with(options: ClientOptions, frames_before_block: Option<usize>) -> Harness {
    let (to_client, incoming) = mpsc::channel();
    let (outgoing, from_client) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let terminated = Arc::new(AtomicBool::new(false));
    let flag = terminated.clone();
    let transport = Transport::new(
        PeerReader {
            incoming,
            buffer: Vec::new(),
        },
        PeerWriter {
            outgoing,
            buffer: Vec::new(),
            frames_left: frames_before_block,
            gate: frames_before_block.map(|_| gate_rx),
        },
    )
    .on_terminate(move || {
        flag.store(true, Ordering::SeqCst);
        drop(gate_tx);
    });
    let diagnostics = Recorded::default();
    let watchdog = Recorded::default();
    let sink = diagnostics.clone();
    let steps = watchdog.clone();
    let options = options
        .diagnostics(move |diagnostic| sink.push(diagnostic.to_string()))
        .watchdog(move |step| steps.push(step.to_string()));
    let (client, events) = MspClient::connect(transport, options);
    Harness {
        client,
        events,
        peer: Peer {
            to_client: Some(to_client),
            from_client,
        },
        diagnostics,
        watchdog,
        terminated,
    }
}

fn response(id: &str, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#)
}

#[test]
fn responses_are_correlated_by_id_while_notifications_flow() {
    let h = harness(ClientOptions::default());
    let first = h
        .client
        .start_request("test/first", Params::empty(), WAIT)
        .unwrap();
    let second = h
        .client
        .start_request("test/second", Params::raw(r#"{"n":2}"#), WAIT)
        .unwrap();
    assert_eq!(
        h.peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":1,"method":"test/first","params":{}}"#
    );
    assert_eq!(
        h.peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":2,"method":"test/second","params":{"n":2}}"#
    );
    h.peer.send(&response("2", r#"{"second":true}"#));
    let answered = second.wait().unwrap();
    assert_eq!(answered.get("second").and_then(J::as_bool), Some(true));

    h.peer
        .send(r#"{"jsonrpc":"2.0","method":"turn/started","params":{"turnId":"t"}}"#);
    match h.events.recv_timeout(WAIT).unwrap() {
        MspEvent::Notification { method, params } => {
            assert_eq!(method, "turn/started");
            assert_eq!(params.get("turnId").and_then(J::as_str), Some("t"));
        }
        other => panic!("unexpected {other:?}"),
    }
    h.peer.send(&response("1", r#"{"first":true}"#));
    assert!(first.wait().unwrap().get("first").is_some());
    assert_eq!(h.diagnostics.lines(), ["msp<-host method=turn/started"]);
}

#[test]
fn concurrent_requests_each_get_their_own_response() {
    let Harness { client, peer, .. } = harness(ClientOptions::default());
    let client = Arc::new(client);
    let echo = std::thread::spawn(move || {
        for _ in 0..(8 * 25) {
            let frame = peer.recv();
            let id = j_to_string(frame.get("id").unwrap());
            let n = j_to_string(frame.get("params").unwrap().get("n").unwrap());
            peer.send(&response(&id, &format!(r#"{{"n":{n}}}"#)));
        }
        peer
    });
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let client = client.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    let n = worker * 1000 + i;
                    let result = client
                        .request("test/echo", Params::raw(format!(r#"{{"n":{n}}}"#)))
                        .unwrap();
                    assert_eq!(result.get("n").and_then(J::as_u64), Some(n));
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    echo.join().unwrap();
}

#[test]
fn a_timed_out_request_ignores_its_late_response() {
    let h = harness(ClientOptions::default());
    let error = h
        .client
        .request_with_timeout("test/slow", Params::empty(), Duration::from_millis(50))
        .unwrap_err();
    assert!(
        matches!(&error, MspError::Timeout { method, id: 1, timeout }
            if method == "test/slow" && *timeout == Duration::from_millis(50)),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "serve command timed out after 50ms method=test/slow id=1"
    );
    h.peer.recv();
    h.peer.send(&response("1", r#"{"late":true}"#));
    h.diagnostics.wait_for("serve response for unknown id 1");

    let next = h
        .client
        .start_request("test/next", Params::empty(), WAIT)
        .unwrap();
    assert_eq!(next.id(), 2);
    h.peer.recv();
    h.peer.send(&response("2", r#"{"next":true}"#));
    assert!(next.wait().unwrap().get("next").is_some());
}

#[test]
fn the_request_timeout_option_overrides_the_method_table() {
    assert_eq!(default_timeout("session/start"), Duration::from_secs(180));
    assert_eq!(default_timeout("initialize"), Duration::from_secs(30));
    assert_eq!(default_timeout("approval/decide"), Duration::from_secs(30));
    assert_eq!(default_timeout("turn/start"), Duration::from_secs(60));
    assert_eq!(default_timeout("future/method"), Duration::from_secs(60));
    let h = harness(ClientOptions::default().request_timeout(Duration::from_millis(20)));
    let error = h
        .client
        .request("session/start", Params::empty())
        .unwrap_err();
    assert!(
        matches!(error, MspError::Timeout { timeout, .. } if timeout == Duration::from_millis(20))
    );
}

#[test]
fn a_timeout_too_large_to_represent_waits_without_a_deadline() {
    let h = harness(ClientOptions::default());
    let pending = h
        .client
        .start_request("test/forever", Params::empty(), Duration::MAX)
        .unwrap();
    assert_eq!(
        h.peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":1,"method":"test/forever","params":{}}"#
    );
    h.peer.send(&response("1", r#"{"done":true}"#));
    assert!(pending.wait().unwrap().get("done").is_some());
}

#[test]
fn a_cancelled_request_settles_once_and_ignores_its_late_response() {
    let h = harness(ClientOptions::default());
    let pending = h
        .client
        .start_request("test/slow", Params::empty(), WAIT)
        .unwrap();
    let handle = pending.handle();
    let canceller = handle.clone();
    assert!(
        std::thread::spawn(move || canceller.cancel())
            .join()
            .unwrap()
    );
    assert!(
        !handle.cancel(),
        "a settled request cannot be cancelled twice"
    );
    let error = pending.wait().unwrap_err();
    assert!(
        matches!(&error, MspError::Cancelled { method, id: 1 } if method == "test/slow"),
        "{error:?}"
    );
    h.peer.recv();
    h.peer.send(&response("1", "{}"));
    h.diagnostics.wait_for("serve response for unknown id 1");

    let answered = h
        .client
        .start_request("test/fast", Params::empty(), WAIT)
        .unwrap();
    let late_cancel = answered.handle();
    h.peer.recv();
    h.peer.send(&response("2", "{}"));
    assert!(answered.wait().is_ok());
    assert!(!late_cancel.cancel());
}

#[test]
fn malformed_frames_are_skipped_and_the_connection_survives() {
    let h = harness(ClientOptions::default());
    let pending = h
        .client
        .start_request("test/after", Params::empty(), WAIT)
        .unwrap();
    h.peer.recv();
    h.peer.send("not json");
    h.peer.send_bytes(b"\xff\xfe\n");
    h.peer.send("");
    h.peer.send("   ");
    h.peer
        .send(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}"#);
    // One frame split across two reads still parses.
    h.peer.send_bytes(br#"{"jsonrpc":"2.0","id":1,"#);
    h.peer.send_bytes(b"\"result\":{\"ok\":true}}\n");
    assert!(pending.wait().unwrap().get("ok").is_some());
    let lines = h.diagnostics.lines();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].starts_with("serve parse error: "), "{lines:?}");
    assert_eq!(lines[1], "serve parse error: invalid UTF-8 frame");
    assert!(lines[2].starts_with("serve parse error: "), "{lines:?}");
    assert!(matches!(h.events.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn eof_fails_every_pending_request_and_reports_closure() {
    let mut h = harness(ClientOptions::default());
    let first = h
        .client
        .start_request("test/a", Params::empty(), WAIT)
        .unwrap();
    let second = h
        .client
        .start_request("test/b", Params::empty(), WAIT)
        .unwrap();
    h.peer.close();
    let started = Instant::now();
    assert!(matches!(first.wait(), Err(MspError::HostClosed)));
    assert!(matches!(second.wait(), Err(MspError::HostClosed)));
    assert!(
        started.elapsed() < WAIT,
        "settled at EOF, not at the deadline"
    );
    match h.events.recv_timeout(WAIT).unwrap() {
        MspEvent::Closed { reason } => assert_eq!(reason, "serve host stdout closed"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(
        h.watchdog
            .lines()
            .contains(&"host reader disconnect".to_string())
    );
    match h.client.request("test/c", Params::empty()) {
        Err(MspError::Write(error)) => {
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(error.to_string(), "serve is shut down");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn server_requests_follow_their_configured_mode() {
    let h = harness(
        ClientOptions::default()
            .server_request("approval/request", ServerRequestMode::Acknowledge)
            .server_request("test/ask", ServerRequestMode::Respond),
    );
    h.peer.send(
        r#"{"jsonrpc":"2.0","id":7,"method":"approval/request","params":{"approvalId":"a-1"}}"#,
    );
    match h.events.recv_timeout(WAIT).unwrap() {
        MspEvent::Request { id, method, params } => {
            assert_eq!(j_to_string(&id), "7");
            assert_eq!(method, "approval/request");
            assert_eq!(params.get("approvalId").and_then(J::as_str), Some("a-1"));
        }
        other => panic!("unexpected {other:?}"),
    }
    // The ack was written before the event was delivered.
    assert_eq!(
        h.peer.from_client.try_recv().map(String::from_utf8),
        Ok(Ok(
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n".to_string()
        ))
    );

    h.peer
        .send(r#"{"jsonrpc":"2.0","id":"s-1","method":"test/ask","params":{}}"#);
    let id = match h.events.recv_timeout(WAIT).unwrap() {
        MspEvent::Request { id, .. } => id,
        other => panic!("unexpected {other:?}"),
    };
    assert!(h.peer.nothing_sent(), "respond mode waits for the caller");
    h.client
        .respond(&id, Params::raw(r#"{"answer":42}"#))
        .unwrap();
    assert_eq!(
        h.peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":"s-1","result":{"answer":42}}"#
    );
    h.client
        .respond_error(
            &id,
            &parse_json(r#"{"code":-32603,"message":"no"}"#).unwrap(),
        )
        .unwrap();
    assert_eq!(
        h.peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":"s-1","error":{"code":-32603,"message":"no"}}"#
    );

    h.peer
        .send(r#"{"jsonrpc":"2.0","id":9,"method":"future/request","params":{}}"#);
    assert_eq!(
        h.peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":9,"error":{"code":-32601,"message":"method not found: future/request","data":{"kind":"methodNotFound","retryable":false}}}"#
    );
    assert_eq!(
        h.diagnostics.lines(),
        ["unsupported MSP server request: future/request id=9"]
    );
    assert!(matches!(h.events.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn a_stuck_write_terminates_the_transport_and_fails_pending_requests() {
    let h = harness_with(
        ClientOptions::default().write_timeout(Duration::from_millis(100)),
        Some(1),
    );
    let pending = h
        .client
        .start_request("test/pending", Params::empty(), WAIT)
        .unwrap();
    assert!(h.peer.recv_text().contains("test/pending"));
    match h.client.request("test/stuck", Params::empty()) {
        Err(MspError::Write(error)) => {
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_eq!(
                error.to_string(),
                "serve stdin write timed out; host terminated"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(
        h.terminated.load(Ordering::SeqCst),
        "the transport was terminated"
    );
    match pending.wait() {
        Err(MspError::Closed(reason)) => {
            assert_eq!(reason, "serve stdin write timed out; host terminated")
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        h.client.notify("test/later", Params::empty()),
        Err(MspError::Write(error)) if error.to_string() == "serve is shut down"
    ));
    // The undelivered frame never reaches the host.
    assert!(h.peer.nothing_sent());
}

#[test]
fn dropping_the_event_receiver_closes_the_connection() {
    let Harness {
        client,
        events,
        peer,
        ..
    } = harness(ClientOptions::default());
    let pending = client
        .start_request("test/pending", Params::empty(), WAIT)
        .unwrap();
    peer.recv();
    drop(events);
    peer.send(r#"{"jsonrpc":"2.0","method":"turn/started","params":{}}"#);
    match pending.wait() {
        Err(MspError::Closed(reason)) => assert_eq!(reason, "serve event loop closed"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn initialize_sends_the_configured_handshake_then_initialized() {
    let options = ClientOptions::default()
        .client_info(ClientInfo {
            name: "muse_acp".into(),
            version: "0.15.0".into(),
            title: None,
        })
        .capabilities(Capabilities {
            experimental_api: Some(true),
            user_input_dialogs: Some(true),
            requested: vec!["sessionRename".into()],
            ..Capabilities::default()
        });
    let Harness { client, peer, .. } = harness(options);
    let client = Arc::new(client);
    let initializing = {
        let client = client.clone();
        std::thread::spawn(move || client.initialize())
    };
    assert_eq!(
        peer.recv_text(),
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"muse_acp","version":"0.15.0"},"capabilities":{"experimentalApi":true,"userInputDialogs":true,"requestedCapabilities":["sessionRename"]}}}"#
    );
    peer.send(&response(
        "1",
        r#"{"serverInfo":{"name":"muse","version":"1.4.8"},"schema":{"version":1,"fingerprint":"sha256:00"},"grantedCapabilities":["sessionRename"],"experimentalApi":true}"#,
    ));
    assert_eq!(
        peer.recv_text(),
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#
    );
    let handshake = initializing.join().unwrap().unwrap();
    assert!(handshake.granted("sessionRename"));
    assert_eq!(handshake.server_version.as_deref(), Some("1.4.8"));
    assert_eq!(
        client.handshake().and_then(|h| h.fingerprint),
        Some("sha256:00".to_string())
    );
}

#[test]
fn an_unsupported_envelope_is_refused_before_initialized() {
    let Harness { client, peer, .. } = harness(ClientOptions::default());
    let client = Arc::new(client);
    let initializing = {
        let client = client.clone();
        std::thread::spawn(move || client.initialize())
    };
    peer.recv();
    peer.send(&response(
        "1",
        r#"{"schema":{"version":2,"fingerprint":"x"}}"#,
    ));
    assert!(matches!(
        initializing.join().unwrap(),
        Err(HandshakeError::Rejected(reason)) if reason.contains("version=2")
    ));
    assert!(peer.nothing_sent(), "initialized must not follow a refusal");

    let accepting =
        harness(ClientOptions::default().handshake_check(
            |handshake| match handshake.schema_version {
                Some(2) => Ok(()),
                _ => Err("want 2".into()),
            },
        ));
    let client = Arc::new(accepting.client);
    let initializing = {
        let client = client.clone();
        std::thread::spawn(move || client.initialize())
    };
    accepting.peer.recv();
    accepting.peer.send(&response(
        "1",
        r#"{"schema":{"version":2,"fingerprint":"x"}}"#,
    ));
    assert!(accepting.peer.recv_text().contains("initialized"));
    assert!(initializing.join().unwrap().is_ok());
}

#[test]
fn close_on_a_custom_transport_fails_pending_requests_and_closes_the_writer() {
    let h = harness(ClientOptions::default());
    let pending = h
        .client
        .start_request("test/pending", Params::empty(), WAIT)
        .unwrap();
    h.peer.recv();
    assert!(h.client.close().is_none());
    match pending.wait() {
        Err(MspError::Closed(reason)) => assert_eq!(reason, "serve shutting down"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        h.peer.from_client.recv_timeout(WAIT),
        Err(RecvTimeoutError::Disconnected)
    ));
    assert!(h.client.close().is_none(), "close is safe to repeat");
    assert!(h.watchdog.lines().contains(&"host shutdown".to_string()));
    assert!(h.client.reap().is_none());
    assert!(!h.terminated.load(Ordering::SeqCst));
    assert!(h.client.force_stop());
    assert!(h.terminated.load(Ordering::SeqCst));
}

#[test]
fn dropping_an_unclosed_client_closes_it_in_the_background() {
    let Harness {
        client,
        peer,
        watchdog,
        diagnostics,
        ..
    } = harness(ClientOptions::default());
    drop(client);
    assert!(matches!(
        peer.from_client.recv_timeout(WAIT),
        Err(RecvTimeoutError::Disconnected)
    ));
    assert!(watchdog.lines().is_empty() && diagnostics.lines().is_empty());
}

#[test]
fn params_serialize_values_and_pass_raw_text_through() {
    let value = parse_json(r#"{"a":[1,"two",null]}"#).unwrap();
    assert_eq!(Params::from(&value).as_str(), r#"{"a":[1,"two",null]}"#);
    assert_eq!(Params::from(value).as_str(), r#"{"a":[1,"two",null]}"#);
    assert_eq!(Params::raw("{ \"kept\" : 1 }").as_str(), "{ \"kept\" : 1 }");
    assert_eq!(Params::empty().as_str(), "{}");
}

#[test]
fn command_ids_stay_unique_within_a_millisecond_without_entropy() {
    // The worst case where no random source is available: only the
    // sequence separates ids minted in the same millisecond.
    let mut seen = std::collections::HashSet::new();
    for n in 0..4096 {
        let id = super::uuid_v7(n, 1_792_432_422_000, [0; 10]);
        assert_eq!(id.as_bytes()[14], b'7', "{id}");
        assert!(seen.insert(id), "sequence {n} collided");
    }
}

#[test]
fn command_ids_are_unique_uuid_v7() {
    let h = harness(ClientOptions::default());
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1000 {
        let id = h.client.mint_command_id();
        let bytes = id.as_bytes();
        assert_eq!(bytes.len(), 36, "{id}");
        for dash in [8, 13, 18, 23] {
            assert_eq!(bytes[dash], b'-', "{id}");
        }
        assert_eq!(bytes[14], b'7', "version 7: {id}");
        assert!(b"89ab".contains(&bytes[19]), "RFC 4122 variant: {id}");
        assert!(
            id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
            "{id}"
        );
        assert!(seen.insert(id));
    }
}
