//! Process-level tests against a real child process. Each test launches this
//! test binary again as a scripted fake `muse serve` (selected by
//! `MUSE_MSP_FAKE_HOST`), so the suite needs no Muse, Python, or shell and
//! runs the same on Linux, macOS, and Windows. The target has no libtest
//! harness: `main` either acts as the fake host or runs the tests below,
//! honoring a name filter, `--exact`, `--skip`, and `--list`.

use std::io::{BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muse_msp_client::json::{J, esc, j_to_string, parse_json};
use muse_msp_client::{
    Capabilities, ClientOptions, ExitKind, HandshakeError, LaunchConfig, LaunchError, MspClient,
    MspError, MspEvent, Params, ServerRequestMode, ShutdownPolicy, probe_exit,
};

const HOST_ENV: &str = "MUSE_MSP_FAKE_HOST";
const WAIT: Duration = Duration::from_secs(10);

fn main() {
    if std::env::var_os(HOST_ENV).is_some() {
        fake_host();
        return;
    }
    run_tests(&[
        (
            "launch_handshake_requests_events_and_clean_close",
            launch_handshake_requests_events_and_clean_close,
        ),
        (
            "respond_mode_lets_the_caller_answer",
            respond_mode_lets_the_caller_answer,
        ),
        (
            "exit_codes_are_classified_from_the_real_process",
            exit_codes_are_classified_from_the_real_process,
        ),
        (
            "a_host_that_exits_during_the_handshake_reports_its_exit",
            a_host_that_exits_during_the_handshake_reports_its_exit,
        ),
        (
            "close_kills_a_host_that_ignores_stdin_eof",
            close_kills_a_host_that_ignores_stdin_eof,
        ),
        ("a_stuck_write_kills_the_host", a_stuck_write_kills_the_host),
        (
            "stderr_is_bounded_and_never_protocol_input",
            stderr_is_bounded_and_never_protocol_input,
        ),
        (
            "a_missing_program_is_a_spawn_error",
            a_missing_program_is_a_spawn_error,
        ),
        (
            "probe_exit_classifies_a_stdin_closed_host",
            probe_exit_classifies_a_stdin_closed_host,
        ),
        (
            "a_refused_handshake_closes_the_host",
            a_refused_handshake_closes_the_host,
        ),
        (
            "force_stop_kills_without_waiting",
            force_stop_kills_without_waiting,
        ),
        (
            "environment_and_working_directory_reach_the_host",
            environment_and_working_directory_reach_the_host,
        ),
    ]);
}

type Test = (&'static str, fn());

fn run_tests(tests: &[Test]) {
    let mut filter = None;
    let mut skips = Vec::new();
    let (mut exact, mut list) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exact" => exact = true,
            "--list" => list = true,
            "--skip" => skips.extend(args.next()),
            "--test-threads" | "--format" | "--color" | "--logfile" | "-Z" => {
                args.next();
            }
            flag if flag.starts_with('-') => {}
            name => filter = Some(name.to_string()),
        }
    }
    let selected: Vec<&Test> = tests
        .iter()
        .filter(|(name, _)| match &filter {
            Some(filter) if exact => name == filter,
            Some(filter) => name.contains(filter.as_str()),
            None => true,
        })
        .filter(|(name, _)| !skips.iter().any(|skip| name.contains(skip.as_str())))
        .collect();
    if list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        return;
    }
    println!("\nrunning {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, test) in &selected {
        let ok = panic::catch_unwind(AssertUnwindSafe(test)).is_ok();
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        if !ok {
            failed.push(*name);
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len()
    );
    if !failed.is_empty() {
        println!("failures: {failed:?}");
        std::process::exit(101);
    }
}

/// A scripted host. Requests: `initialize` (grants every requested
/// capability), `test/echo` (result = params), `test/silent` (never
/// answered), `test/exit` (writes `stderr`, exits with `code`),
/// `test/notify` (sends `test/note` first), `test/ask` (sends server
/// request `s-1` and reports the client's answer as `test/answered`),
/// `test/stopReading` (answers, then never reads stdin again),
/// `test/stderr` (writes `count` protocol-looking lines to stderr), and
/// `test/env`. Environment: `FAKE_EXIT_AT_START`, `FAKE_SCHEMA_VERSION`,
/// `FAKE_IGNORE_EOF`, `FAKE_EOF_EXIT`, `FAKE_EOF_STDERR`.
fn fake_host() {
    let env = |name: &str| std::env::var(name).ok();
    if let Some(code) = env("FAKE_EXIT_AT_START") {
        eprintln!("fatal: configuration rejected");
        std::process::exit(code.parse().unwrap());
    }
    let stdout = std::io::stdout();
    let send = |frame: String| {
        let mut out = stdout.lock();
        writeln!(out, "{frame}").unwrap();
        out.flush().unwrap();
    };
    let result = |id: &J, result: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{result}}}"#,
            j_to_string(id)
        )
    };
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let frame = parse_json(&line).expect("client frames are JSON");
        let id = frame.get("id").cloned();
        let params = frame.get("params").cloned().unwrap_or(J::Null);
        let method = frame.get("method").and_then(J::as_str).unwrap_or("");
        let Some(id) = id else {
            continue; // `initialized` and other notifications.
        };
        match method {
            "initialize" => {
                let requested = params
                    .get("capabilities")
                    .and_then(|caps| caps.get("requestedCapabilities"))
                    .map(j_to_string)
                    .unwrap_or_else(|| "[]".into());
                let schema = env("FAKE_SCHEMA_VERSION").unwrap_or_else(|| "1".into());
                send(result(
                    &id,
                    &format!(
                        r#"{{"serverInfo":{{"name":"fake-muse","version":"1.4.11-fake"}},"schema":{{"version":{schema},"fingerprint":"sha256:fake"}},"grantedCapabilities":{requested},"experimentalApi":false,"sessionDurability":"durable"}}"#
                    ),
                ));
            }
            "test/echo" => send(result(&id, &j_to_string(&params))),
            "test/silent" => {}
            "test/exit" => {
                if let Some(text) = params.get("stderr").and_then(J::as_str) {
                    eprint!("{text}");
                }
                std::process::exit(params.get("code").and_then(J::as_u64).unwrap() as i32);
            }
            "test/notify" => {
                send(r#"{"jsonrpc":"2.0","method":"test/note","params":{"n":1}}"#.into());
                send(result(&id, "{}"));
            }
            "test/ask" => {
                send(
                    r#"{"jsonrpc":"2.0","id":"s-1","method":"approval/request","params":{"approvalId":"a-1"}}"#
                        .into(),
                );
                send(result(&id, "{}"));
            }
            "test/stopReading" => {
                send(result(&id, "{}"));
                std::thread::sleep(Duration::from_secs(60));
                return;
            }
            "test/stderr" => {
                let count = params.get("count").and_then(J::as_u64).unwrap();
                let spoof = params.get("spoof").map(j_to_string).unwrap();
                for n in 0..count {
                    eprintln!(
                        r#"{{"jsonrpc":"2.0","id":{spoof},"result":{{"spoofed":true}}}} #{n:03}"#
                    );
                }
                send(result(&id, "{}"));
            }
            "test/env" => {
                let cwd = std::env::current_dir().unwrap();
                send(result(
                    &id,
                    &format!(
                        r#"{{"mark":{},"cwd":{}}}"#,
                        esc(&env("FAKE_MARK").unwrap_or_default()),
                        esc(&cwd.display().to_string())
                    ),
                ));
            }
            "" => {
                // The client's answer to our server request.
                send(format!(
                    r#"{{"jsonrpc":"2.0","method":"test/answered","params":{line}}}"#
                ));
            }
            other => send(format!(
                r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32601,"message":{}}}}}"#,
                j_to_string(&id),
                esc(&format!("unknown {other}"))
            )),
        }
    }
    if env("FAKE_IGNORE_EOF").is_some() {
        std::thread::sleep(Duration::from_secs(60));
    }
    if let Some(text) = env("FAKE_EOF_STDERR") {
        eprintln!("{text}");
    }
    std::process::exit(env("FAKE_EOF_EXIT").map_or(0, |code| code.parse().unwrap()));
}

fn fake(env: &[(&str, &str)]) -> LaunchConfig {
    let mut config = LaunchConfig::new(std::env::current_exe().unwrap());
    config.env(HOST_ENV, "1");
    for (key, value) in env {
        config.env(key, value);
    }
    config.shutdown_policy(ShutdownPolicy {
        grace: Duration::from_secs(3),
        timeout: Duration::from_secs(8),
    });
    config
}

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<String>>>);

impl Recorded {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn recording(options: ClientOptions) -> (ClientOptions, Recorded) {
    let recorded = Recorded::default();
    let sink = recorded.clone();
    let options = options.diagnostics(move |diagnostic| {
        sink.0.lock().unwrap().push(diagnostic.to_string());
    });
    (options, recorded)
}

fn next_event(events: &Receiver<MspEvent>) -> MspEvent {
    events.recv_timeout(WAIT).expect("event")
}

fn wait_closed(events: &Receiver<MspEvent>) -> String {
    loop {
        if let MspEvent::Closed { reason } = next_event(events) {
            return reason;
        }
    }
}

fn launch_handshake_requests_events_and_clean_close() {
    let (options, diagnostics) = recording(
        ClientOptions::default()
            .capabilities(Capabilities {
                requested: vec!["sessionRename".into(), "feedback".into()],
                ..Capabilities::default()
            })
            .server_request("approval/request", ServerRequestMode::Acknowledge),
    );
    let mut config = fake(&[]);
    config.options(options);
    let (client, events) = MspClient::launch(config).expect("launch");
    let handshake = client.handshake().expect("handshake");
    assert_eq!(handshake.server_name.as_deref(), Some("fake-muse"));
    assert_eq!(handshake.schema_version, Some(1));
    assert_eq!(
        handshake.granted_capabilities,
        ["sessionRename", "feedback"]
    );
    assert!(handshake.session_durability().is_durable());

    let echoed = client
        .request("test/echo", Params::raw(r#"{"x":1}"#))
        .unwrap();
    assert_eq!(echoed.get("x").and_then(J::as_u64), Some(1));

    client.request("test/notify", Params::empty()).unwrap();
    assert!(matches!(
        next_event(&events),
        MspEvent::Notification { method, .. } if method == "test/note"
    ));

    client.request("test/ask", Params::empty()).unwrap();
    match next_event(&events) {
        MspEvent::Request { id, method, params } => {
            assert_eq!(j_to_string(&id), "\"s-1\"");
            assert_eq!(method, "approval/request");
            assert_eq!(params.get("approvalId").and_then(J::as_str), Some("a-1"));
        }
        other => panic!("unexpected {other:?}"),
    }
    match next_event(&events) {
        MspEvent::Notification { method, params } => {
            assert_eq!(method, "test/answered");
            assert_eq!(
                j_to_string(&params),
                r#"{"jsonrpc":"2.0","id":"s-1","result":{}}"#
            );
        }
        other => panic!("unexpected {other:?}"),
    }

    let started = Instant::now();
    let exit = client.close().expect("exit observed");
    assert_eq!(exit.kind, ExitKind::CleanShutdown);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "a host that exits on stdin EOF is not killed"
    );
    assert_eq!(wait_closed(&events), "serve host stdout closed");
    let again = Instant::now();
    assert_eq!(
        client.close().map(|exit| exit.kind),
        Some(ExitKind::CleanShutdown)
    );
    assert!(
        again.elapsed() < Duration::from_secs(1),
        "a second close is quick"
    );
    assert!(
        diagnostics
            .lines()
            .contains(&"msp<-host method=test/note".to_string())
    );
    assert!(
        !events
            .try_iter()
            .any(|event| matches!(event, MspEvent::Stderr { .. })),
        "stderr events are off by default"
    );
}

fn respond_mode_lets_the_caller_answer() {
    let mut config = fake(&[]);
    config.options(
        ClientOptions::default().server_request("approval/request", ServerRequestMode::Respond),
    );
    let (client, events) = MspClient::launch(config).expect("launch");
    client.request("test/ask", Params::empty()).unwrap();
    let id = match next_event(&events) {
        MspEvent::Request { id, .. } => id,
        other => panic!("unexpected {other:?}"),
    };
    client
        .respond(&id, Params::raw(r#"{"decision":"later"}"#))
        .unwrap();
    match next_event(&events) {
        MspEvent::Notification { params, .. } => assert_eq!(
            params
                .get("result")
                .and_then(|result| result.get("decision"))
                .and_then(J::as_str),
            Some("later")
        ),
        other => panic!("unexpected {other:?}"),
    }
    client.close();
}

fn exit_codes_are_classified_from_the_real_process() {
    for (code, kind) in [
        (0, ExitKind::CleanShutdown),
        (1, ExitKind::UnhandledError),
        (2, ExitKind::UsageError),
        (3, ExitKind::ConfigError),
        (4, ExitKind::LeaseUnavailable),
        (5, ExitKind::SdkSurfaceUnavailable),
        (7, ExitKind::Crash),
    ] {
        let (client, events) = MspClient::launch(fake(&[])).expect("launch");
        let error = client
            .request(
                "test/exit",
                Params::raw(format!(r#"{{"code":{code},"stderr":"why {code}\n"}}"#)),
            )
            .unwrap_err();
        assert!(matches!(error, MspError::HostClosed), "{code}: {error:?}");
        assert_eq!(wait_closed(&events), "serve host stdout closed");
        let exit = client.reap().expect("reaped");
        assert_eq!(exit.kind, kind, "{code}");
        assert_eq!(exit.exit_code, Some(code));
        assert!(
            exit.stderr_tail.contains(&format!("why {code}")),
            "{exit:?}"
        );
        assert_eq!(
            client.reap(),
            Some(exit),
            "the first classification is kept"
        );
    }
}

fn a_host_that_exits_during_the_handshake_reports_its_exit() {
    match MspClient::launch(fake(&[("FAKE_EXIT_AT_START", "3")])) {
        Err(LaunchError::HostExit(exit)) => {
            assert_eq!(exit.kind, ExitKind::ConfigError);
            assert!(
                exit.stderr_tail.contains("configuration rejected"),
                "{exit:?}"
            );
            assert!(!LaunchError::HostExit(exit).retryable());
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn close_kills_a_host_that_ignores_stdin_eof() {
    let mut config = fake(&[("FAKE_IGNORE_EOF", "1")]);
    config.shutdown_policy(ShutdownPolicy {
        grace: Duration::from_millis(300),
        timeout: Duration::from_secs(8),
    });
    let (client, _events) = MspClient::launch(config).expect("launch");
    let started = Instant::now();
    let exit = client.close().expect("killed and reaped");
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(8), "{elapsed:?}");
    assert_ne!(exit.kind, ExitKind::CleanShutdown);
    #[cfg(unix)]
    assert_eq!(exit.signal, Some(9));
}

fn a_stuck_write_kills_the_host() {
    let mut config = fake(&[]);
    config.options(ClientOptions::default().write_timeout(Duration::from_millis(300)));
    let (client, events) = MspClient::launch(config).expect("launch");
    client.request("test/stopReading", Params::empty()).unwrap();
    let pending = client
        .start_request("test/silent", Params::empty(), WAIT)
        .unwrap();
    let huge = format!(r#"{{"blob":"{}"}}"#, "x".repeat(16 << 20));
    match client.notify("test/huge", Params::raw(huge)) {
        Err(MspError::Write(error)) => {
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        }
        other => panic!("unexpected {other:?}"),
    }
    // The killed host's stdout closes while the client fails its waiters, so
    // the pending request settles through whichever path runs first.
    let settled = Instant::now();
    match pending.wait() {
        Err(MspError::HostClosed) => {}
        Err(MspError::Closed(reason)) => {
            assert_eq!(reason, "serve stdin write timed out; host terminated")
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(settled.elapsed() < Duration::from_secs(1));
    let exit = client.reap().expect("the host was killed");
    assert_ne!(exit.kind, ExitKind::CleanShutdown);
    wait_closed(&events);
    assert!(matches!(
        client.request("test/echo", Params::empty()),
        Err(MspError::Write(error)) if error.to_string() == "serve is shut down"
    ));
}

fn stderr_is_bounded_and_never_protocol_input() {
    let (options, diagnostics) = recording(ClientOptions::default().stderr_events(true));
    let mut config = fake(&[]);
    config.options(options);
    let (client, events) = MspClient::launch(config).expect("launch");
    // Every stderr line looks like the answer to this pending request.
    let pending = client
        .start_request("test/silent", Params::empty(), Duration::from_millis(500))
        .unwrap();
    let spoof = pending.id();
    client
        .request(
            "test/stderr",
            Params::raw(format!(r#"{{"count":150,"spoof":{spoof}}}"#)),
        )
        .unwrap();
    assert!(
        matches!(pending.wait(), Err(MspError::Timeout { .. })),
        "stderr never resolves a request"
    );
    let mut lines = Vec::new();
    while lines.len() < 150 {
        match next_event(&events) {
            MspEvent::Stderr { line } => lines.push(line),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(lines[0].ends_with("#000"), "{}", lines[0]);
    assert!(lines[149].ends_with("#149"), "{}", lines[149]);
    let tail = client.stderr_tail();
    assert!(tail.len() <= 8 * 1024, "{} bytes", tail.len());
    assert!(tail.lines().count() <= 100);
    assert!(tail.contains("#149") && !tail.contains("#000"));
    client.close();
    assert!(
        diagnostics
            .lines()
            .iter()
            .any(|line| line.starts_with("serve stderr captured ")),
        "{:?}",
        diagnostics.lines()
    );
}

fn a_missing_program_is_a_spawn_error() {
    let missing = std::env::temp_dir().join(format!("muse-msp-missing-{}", std::process::id()));
    match MspClient::spawn(LaunchConfig::new(&missing)) {
        Err(LaunchError::Spawn { error, .. }) => {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound)
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn probe_exit_classifies_a_stdin_closed_host() {
    let gated = fake(&[
        ("FAKE_EOF_EXIT", "5"),
        ("FAKE_EOF_STDERR", "serve gate is off"),
    ]);
    let exit = probe_exit(&gated, WAIT).unwrap().expect("exited");
    assert_eq!(exit.kind, ExitKind::SdkSurfaceUnavailable);
    assert!(exit.stderr_tail.contains("serve gate is off"));
    let stuck = fake(&[("FAKE_IGNORE_EOF", "1")]);
    assert_eq!(
        probe_exit(&stuck, Duration::from_millis(300)).unwrap(),
        None
    );
}

fn a_refused_handshake_closes_the_host() {
    match MspClient::launch(fake(&[("FAKE_SCHEMA_VERSION", "2")])) {
        Err(error @ LaunchError::Handshake(HandshakeError::Rejected(_))) => {
            assert!(!error.retryable());
            assert!(error.to_string().contains("version=2"), "{error}");
        }
        other => panic!("unexpected {other:?}"),
    }
    let mut accepting = fake(&[("FAKE_SCHEMA_VERSION", "2")]);
    accepting.options(ClientOptions::default().handshake_check(|_| Ok(())));
    let (client, _events) = MspClient::launch(accepting).expect("custom check accepts");
    assert_eq!(client.handshake().unwrap().schema_version, Some(2));
    client.close();
}

fn force_stop_kills_without_waiting() {
    let (client, _events) = MspClient::launch(fake(&[("FAKE_IGNORE_EOF", "1")])).expect("launch");
    assert!(client.force_stop());
    let exit = client.reap().expect("killed");
    assert_ne!(exit.kind, ExitKind::CleanShutdown);
}

fn environment_and_working_directory_reach_the_host() {
    let dir = std::env::temp_dir().join(format!("muse-msp-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = fake(&[("FAKE_MARK", "marked")]);
    config.current_dir(&dir);
    let (client, _events) = MspClient::launch(config).expect("launch");
    let seen = client.request("test/env", Params::empty()).unwrap();
    assert_eq!(seen.get("mark").and_then(J::as_str), Some("marked"));
    let cwd = std::path::PathBuf::from(seen.get("cwd").and_then(J::as_str).unwrap());
    assert_eq!(cwd.canonicalize().unwrap(), dir.canonicalize().unwrap());
    client.close();
    let _ = std::fs::remove_dir_all(&dir);
}
