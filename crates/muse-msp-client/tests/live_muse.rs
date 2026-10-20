//! The crate alone against a real `muse serve`: no ACP code is involved.
//!
//! Model calls go to `tests/fixtures/loopback_provider.py` (in the workspace
//! root) on 127.0.0.1. The host runs with a cleared environment, a throwaway
//! Muse config with a dummy API key, its own home and workspace, and
//! self-update off, so no credentials or network access are needed and
//! nothing touches the developer's Muse settings. A failing run keeps its
//! directory and prints the provider log and the host's stderr tail.
//!
//! Skipped unless `MUSE_ACP_LOOPBACK=1`. `MUSE_CLI` selects the Muse binary
//! (default: `muse` on `PATH`, else `~/.local/bin/muse`) and `PYTHON` the
//! interpreter (default `python3`).

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use muse_msp_client::json::{J, esc, j_to_string};
use muse_msp_client::{
    Capabilities, ClientInfo, ClientOptions, ExitKind, LaunchConfig, MspClient, MspError, MspEvent,
    Params, ServerRequestMode,
};

const WAIT: Duration = Duration::from_secs(60);

/// Variables kept from the real environment; a developer's model, proxy, or
/// API key settings are dropped.
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

/// The loopback provider and the isolated directories it serves.
struct Provider {
    dir: PathBuf,
    child: Child,
    port: u16,
}

impl Provider {
    fn start(script: &str) -> Provider {
        let dir = std::env::temp_dir().join(format!("muse-msp-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["config/muse", "home", "ws"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("script.json"), script).unwrap();
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/loopback_provider.py");
        let mut child = Command::new(std::env::var("PYTHON").unwrap_or_else(|_| "python3".into()))
            .arg(fixture)
            .arg(dir.join("provider.log"))
            .arg(dir.join("script.json"))
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the loopback provider (is python3 on PATH?)");
        let mut port = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let port = port.trim().parse().expect("provider port");
        std::fs::write(
            dir.join("config/muse/settings.json"),
            format!(
                r#"{{"schema_version":1,"model":"fake-model","reasoning_effort":"none","endpoint_transport":{{"base_url":"http://127.0.0.1:{port}","auth":"bearer"}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("config/muse/auth.json"),
            r#"{"schema_version":1,"providers":{"meta":{"api_key":"test-dummy-key"}}}"#,
        )
        .unwrap();
        Provider { dir, child, port }
    }

    fn workspace(&self) -> PathBuf {
        self.dir.join("ws")
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let log = std::fs::read_to_string(self.dir.join("provider.log")).unwrap_or_default();
            eprintln!(
                "---- provider.log (port {}, tail)\n{}",
                self.port,
                &log[log.len().saturating_sub(4000)..]
            );
            eprintln!("kept {} for inspection", self.dir.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `MUSE_CLI` (a relative path is taken from the working directory), else
/// `muse` on `PATH`, else the installer's `~/.local/bin/muse`.
fn muse_binary() -> PathBuf {
    let wanted = PathBuf::from(std::env::var_os("MUSE_CLI").unwrap_or_else(|| "muse".into()));
    if wanted.components().count() > 1 {
        return std::path::absolute(&wanted).unwrap();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin")))
        .flat_map(|dir| [dir.join(&wanted), dir.join(&wanted).with_extension("exe")])
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "{} is not on PATH or in ~/.local/bin; set MUSE_CLI",
                wanted.display()
            )
        })
}

fn launch_config(provider: &Provider) -> LaunchConfig {
    let mut config = LaunchConfig::new(muse_binary());
    config.env_clear();
    for name in KEPT_ENV {
        if let Some(value) = std::env::var_os(name) {
            config.env(name, value);
        }
    }
    // On macOS Muse reads credentials from the Keychain, which waits for an
    // approval no headless run can give; the file backend reads auth.json.
    if cfg!(target_os = "macos") {
        config.env("TBH_CREDENTIAL_BACKEND", "file");
    }
    let home = provider.dir.join("home");
    config
        .env("MUSE_NO_AUTO_UPDATE", "1")
        .env("XDG_CONFIG_HOME", provider.dir.join("config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("HOME", &home)
        .current_dir(provider.workspace());
    config.options(
        ClientOptions::default()
            .client_info(ClientInfo {
                name: "muse_msp_client_test".into(),
                ..ClientInfo::default()
            })
            .capabilities(Capabilities {
                // Approvals only; the host must not ask questions.
                user_input_dialogs: Some(false),
                requested: vec!["sessionRename".into()],
                ..Capabilities::default()
            })
            // Answer the JSON-RPC request ourselves; the decision follows
            // as `approval/decide`.
            .server_request("approval/request", ServerRequestMode::Respond),
    );
    config
}

fn next_event(events: &Receiver<MspEvent>, deadline: Instant) -> MspEvent {
    let left = deadline.saturating_duration_since(Instant::now());
    events
        .recv_timeout(left)
        .expect("host event before the deadline")
}

#[test]
fn the_client_alone_drives_a_real_host_through_a_session() {
    if std::env::var("MUSE_ACP_LOOPBACK").as_deref() != Ok("1") {
        eprintln!("skipped: set MUSE_ACP_LOOPBACK=1 to run against a real muse serve");
        return;
    }
    let provider = Provider::start(
        r#"{"shell":{"tool":{"name":"bash","arguments":{"command":"printf approved > approved.txt","description":"Write the approval artifact"}}}}"#,
    );
    let (client, events) = MspClient::launch(launch_config(&provider)).expect("launch muse serve");
    let stderr_on_failure = StderrTail(&client);

    // The handshake facts are typed values.
    let handshake = client.handshake().expect("handshake");
    assert_eq!(handshake.server_name.as_deref(), Some("muse"));
    assert_eq!(handshake.schema_version, Some(1));
    assert!(
        handshake
            .fingerprint
            .as_deref()
            .is_some_and(|fingerprint| fingerprint.starts_with("sha256:")),
        "{handshake:?}"
    );
    assert!(
        handshake.release().is_some(),
        "{:?}",
        handshake.server_version
    );
    eprintln!(
        "host {} fingerprint {} durable={}",
        handshake.server_version.as_deref().unwrap_or("?"),
        handshake.fingerprint.as_deref().unwrap_or("?"),
        handshake.session_durability().is_durable()
    );

    // Create a session.
    let workspace = provider.workspace().canonicalize().unwrap();
    let started = client
        .request(
            "session/start",
            Params::raw(format!(
                r#"{{"commandId":{},"workspaceRoot":{}}}"#,
                esc(&client.mint_command_id()),
                esc(&workspace.display().to_string())
            )),
        )
        .expect("session/start");
    let session = started
        .get("session")
        .and_then(|session| session.get("sessionId"))
        .and_then(J::as_str)
        .expect("session id")
        .to_string();

    // Cancel one pending request: the waiter settles at once, and the late
    // ack is dropped without disturbing the next request.
    let pending = client
        .start_request("model/list", Params::empty(), WAIT)
        .expect("model/list written");
    assert!(pending.handle().cancel(), "the request was still pending");
    assert!(matches!(
        pending.wait(),
        Err(MspError::Cancelled { ref method, .. }) if method == "model/list"
    ));
    let models = client
        .request("model/list", Params::empty())
        .expect("model/list");
    assert!(
        matches!(models.get("models"), Some(J::Arr(_))),
        "{}",
        j_to_string(&models)
    );

    // Send a command: a turn whose scripted reply asks to run a shell command.
    let turn = client
        .request(
            "turn/start",
            Params::raw(format!(
                r#"{{"commandId":{},"sessionId":{},"input":[{{"type":"text","text":"write the artifact [[script:shell]]"}}]}}"#,
                esc(&client.mint_command_id()),
                esc(&session)
            )),
        )
        .expect("turn/start");
    let turn_id = turn
        .get("turnId")
        .and_then(J::as_str)
        .expect("turn id")
        .to_string();

    // Read notifications, answer the server request, decide the approval.
    // Muse 1.3.0 announces approvals only with the `approval/requested`
    // notification; later hosts also send the `approval/request` server
    // request. Decide each approval once, from whichever arrives first.
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    let mut decided = false;
    let mut decided_ids: Vec<String> = Vec::new();
    let mut decide = |params: &J| {
        let approval = params
            .get("approvalId")
            .and_then(J::as_str)
            .expect("approval id")
            .to_string();
        if decided_ids.contains(&approval) {
            return;
        }
        let approve = match params.get("availableChoices") {
            Some(J::Arr(choices)) => choices
                .iter()
                .find(|choice| choice.get("decision").and_then(J::as_str) == Some("approved"))
                .and_then(|choice| choice.get("choiceId"))
                .and_then(J::as_str)
                .expect("an approve-once choice")
                .to_string(),
            _ => panic!("approval without choices: {}", j_to_string(params)),
        };
        let decision = client
            .request(
                "approval/decide",
                Params::raw(format!(
                    r#"{{"commandId":{},"sessionId":{},"approvalId":{},"requirementId":{},"choiceId":{}}}"#,
                    esc(&client.mint_command_id()),
                    esc(&session),
                    esc(&approval),
                    j_to_string(params.get("currentRequirementId").expect("requirement")),
                    esc(&approve)
                )),
            )
            .expect("approval/decide");
        assert!(
            decision.get("status").is_some(),
            "{}",
            j_to_string(&decision)
        );
        decided_ids.push(approval);
    };
    let terminal = loop {
        match next_event(&events, deadline) {
            MspEvent::Request { id, method, params } => {
                assert_eq!(method, "approval/request");
                client
                    .respond(&id, Params::empty())
                    .expect("answer approval/request");
                decide(&params);
                decided = true;
            }
            MspEvent::Notification { method, params } => {
                if method == "approval/requested" {
                    decide(&params);
                    decided = true;
                }
                seen.push(method.clone());
                if method == "turn/completed"
                    && params.get("turnId").and_then(J::as_str) == Some(turn_id.as_str())
                {
                    break params
                        .get("terminal")
                        .and_then(J::as_str)
                        .unwrap_or("")
                        .to_string();
                }
            }
            MspEvent::Closed { reason } => panic!("host closed mid-turn: {reason}"),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert!(decided, "the turn asked for approval: {seen:?}");
    assert_eq!(terminal, "completed", "{seen:?}");
    assert!(
        seen.iter().any(|method| method == "turn/started"),
        "{seen:?}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("approved.txt"))
            .ok()
            .as_deref(),
        Some("approved"),
        "the approved command ran"
    );

    // Close the host.
    let exit = client.close().expect("the host exited");
    assert_eq!(exit.kind, ExitKind::CleanShutdown, "{exit:?}");
    loop {
        if let MspEvent::Closed { .. } = next_event(&events, deadline) {
            break;
        }
    }
    drop(stderr_on_failure);
}

/// Prints the host's bounded stderr tail if the test panics.
struct StderrTail<'a>(&'a MspClient);

impl Drop for StderrTail<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("---- muse serve stderr (tail)\n{}", self.0.stderr_tail());
        }
    }
}
