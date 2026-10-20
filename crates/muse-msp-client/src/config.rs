//! Launch configuration and connection options. Nothing here reads the
//! environment: every executable, argument, variable, and capability is
//! explicit.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::error::Diagnostic;
use crate::handshake::Handshake;

/// Flags that fix a `muse serve` host's behavior for its whole lifetime.
/// They are appended after the configured arguments in the order below.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostFlags {
    /// `--no-session-log`: keep sessions in memory only, out of the saved
    /// session list.
    pub no_session_log: bool,
    /// `--disable-write`: Muse refuses workspace writes.
    pub disable_write: bool,
    /// `--disable-shell`: Muse refuses shell commands.
    pub disable_shell: bool,
}

impl HostFlags {
    /// The command-line flags, in launch order.
    pub fn args(&self) -> Vec<&'static str> {
        [
            (self.no_session_log, "--no-session-log"),
            (self.disable_write, "--disable-write"),
            (self.disable_shell, "--disable-shell"),
        ]
        .into_iter()
        .filter_map(|(set, flag)| set.then_some(flag))
        .collect()
    }
}

/// How `close` stops a host: close its stdin, wait up to `grace` for it to
/// exit, then kill it, and give up waiting after `timeout` in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownPolicy {
    pub grace: Duration,
    pub timeout: Duration,
}

impl Default for ShutdownPolicy {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(5),
            timeout: Duration::from_secs(7),
        }
    }
}

/// `clientInfo` in the `initialize` params. `name` must match `[a-z0-9_]+`;
/// it identifies the client in diagnostics and is never an authority claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    pub title: Option<String>,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            name: "muse_msp_client".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            title: None,
        }
    }
}

/// `capabilities` in the `initialize` params. `None` and empty lists are
/// omitted, which the host reads as its defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// `experimentalApi`: opt into experimental methods and fields.
    pub experimental_api: Option<bool>,
    /// `userInputDialogs`: whether this client can answer
    /// `userInput/request`. The host reads absent as capable.
    pub user_input_dialogs: Option<bool>,
    /// `requestedCapabilities`: names from the host's capability registry.
    /// Unknown names are ignored by the host and never granted.
    pub requested: Vec<String>,
    /// `optOutNotificationMethods`: notification methods to withhold.
    pub opt_out_notification_methods: Vec<String>,
}

/// How the client treats one server-initiated request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRequestMode {
    /// Reply `{}` at once ("a client is handling this"), then deliver the
    /// request as an event. This is MSP's contract for `approval/request` and
    /// `userInput/request`: the decision travels later as a command such as
    /// `approval/decide`.
    Acknowledge,
    /// Deliver the request with its id; the caller must answer it with
    /// [`crate::MspClient::respond`] or [`crate::MspClient::respond_error`].
    Respond,
}

pub(crate) type DiagnosticSink = Arc<dyn Fn(&Diagnostic<'_>) + Send + Sync>;
pub(crate) type Watchdog = Arc<dyn Fn(&'static str) -> Box<dyn Send> + Send + Sync>;
pub(crate) type HandshakeCheck = Arc<dyn Fn(&Handshake) -> Result<(), String> + Send + Sync>;

/// Connection behavior shared by launched hosts and custom transports.
#[derive(Clone)]
pub struct ClientOptions {
    pub(crate) client_info: ClientInfo,
    pub(crate) capabilities: Capabilities,
    pub(crate) server_requests: Vec<(String, ServerRequestMode)>,
    pub(crate) request_timeout: Option<Duration>,
    pub(crate) write_timeout: Duration,
    pub(crate) stderr_events: bool,
    pub(crate) diagnostics: Option<DiagnosticSink>,
    pub(crate) watchdog: Option<Watchdog>,
    pub(crate) handshake_check: Option<HandshakeCheck>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            client_info: ClientInfo::default(),
            capabilities: Capabilities::default(),
            server_requests: Vec::new(),
            request_timeout: None,
            write_timeout: Duration::from_secs(5),
            stderr_events: false,
            diagnostics: None,
            watchdog: None,
            handshake_check: None,
        }
    }
}

impl fmt::Debug for ClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientOptions")
            .field("client_info", &self.client_info)
            .field("capabilities", &self.capabilities)
            .field("server_requests", &self.server_requests)
            .field("request_timeout", &self.request_timeout)
            .field("write_timeout", &self.write_timeout)
            .field("stderr_events", &self.stderr_events)
            .finish_non_exhaustive()
    }
}

impl ClientOptions {
    #[must_use]
    pub fn client_info(mut self, info: ClientInfo) -> Self {
        self.client_info = info;
        self
    }

    #[must_use]
    pub fn capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Handle server requests for `method` this way. Methods without a mode
    /// are answered with the typed MSP `methodNotFound` error, never with a
    /// synthetic success.
    #[must_use]
    pub fn server_request(mut self, method: impl Into<String>, mode: ServerRequestMode) -> Self {
        let method = method.into();
        self.server_requests.retain(|(known, _)| *known != method);
        self.server_requests.push((method, mode));
        self
    }

    /// Use one deadline for every request sent with
    /// [`crate::MspClient::request`] (including `initialize`) instead of the
    /// per-method table in [`crate::default_timeout`].
    #[must_use]
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// How long one frame may take to reach the pipe. A write that does not
    /// finish in time terminates the transport (5 seconds by default).
    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Deliver each stderr line of a launched host as [`crate::MspEvent::Stderr`].
    /// Off by default: host stderr can contain credentials or workspace text.
    /// Lines longer than 8 KiB are cut. The bounded tail is kept either way.
    #[must_use]
    pub fn stderr_events(mut self, enabled: bool) -> Self {
        self.stderr_events = enabled;
        self
    }

    /// Receive [`Diagnostic`]s. The sink runs on the client's own threads and
    /// must not block.
    #[must_use]
    pub fn diagnostics(mut self, sink: impl Fn(&Diagnostic<'_>) + Send + Sync + 'static) -> Self {
        self.diagnostics = Some(Arc::new(sink));
        self
    }

    /// Arm a caller-owned watchdog around each cleanup step the client runs:
    /// `"host shutdown"`, `"child reap and stderr drain"`, and `"host reader
    /// disconnect"`. The returned guard is dropped when the step finishes, so
    /// a process that must never hang can bound these steps.
    #[must_use]
    pub fn watchdog<G: Send + 'static>(
        mut self,
        arm: impl Fn(&'static str) -> G + Send + Sync + 'static,
    ) -> Self {
        self.watchdog = Some(Arc::new(move |step| Box::new(arm(step)) as Box<dyn Send>));
        self
    }

    /// Decide whether a negotiated handshake is acceptable before the
    /// `initialized` notification is sent. An `Err` aborts the handshake
    /// with that message. The default refuses only an envelope schema
    /// version other than [`crate::ENVELOPE_SCHEMA_VERSION`].
    #[must_use]
    pub fn handshake_check(
        mut self,
        check: impl Fn(&Handshake) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.handshake_check = Some(Arc::new(check));
        self
    }

    pub(crate) fn server_request_mode(&self, method: &str) -> Option<ServerRequestMode> {
        self.server_requests
            .iter()
            .find(|(known, _)| known == method)
            .map(|(_, mode)| *mode)
    }
}

/// How to start one `muse serve` host: `<program> serve <args...>
/// <host flags...>`, with an explicit environment and working directory.
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    pub(crate) flags: HostFlags,
    pub(crate) env: Vec<(OsString, Option<OsString>)>,
    pub(crate) env_clear: bool,
    pub(crate) current_dir: Option<PathBuf>,
    pub(crate) shutdown: ShutdownPolicy,
    pub(crate) options: ClientOptions,
}

impl LaunchConfig {
    /// Run `program serve`. Pass the full path of the Muse executable; a bare
    /// name is resolved by the operating system's program search.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            flags: HostFlags::default(),
            env: Vec::new(),
            env_clear: false,
            current_dir: None,
            shutdown: ShutdownPolicy::default(),
            options: ClientOptions::default(),
        }
    }

    /// Add one argument after `serve`.
    pub fn arg(&mut self, arg: impl Into<OsString>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn host_flags(&mut self, flags: HostFlags) -> &mut Self {
        self.flags = flags;
        self
    }

    pub fn env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> &mut Self {
        self.env.push((key.into(), Some(value.into())));
        self
    }

    pub fn env_remove(&mut self, key: impl Into<OsString>) -> &mut Self {
        self.env.push((key.into(), None));
        self
    }

    /// Start from an empty environment instead of inheriting this process's.
    pub fn env_clear(&mut self) -> &mut Self {
        self.env_clear = true;
        self.env.clear();
        self
    }

    pub fn current_dir(&mut self, directory: impl Into<PathBuf>) -> &mut Self {
        self.current_dir = Some(directory.into());
        self
    }

    pub fn shutdown_policy(&mut self, policy: ShutdownPolicy) -> &mut Self {
        self.shutdown = policy;
        self
    }

    pub fn options(&mut self, options: ClientOptions) -> &mut Self {
        self.options = options;
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// The arguments after `serve`, then the host flags, as launched.
    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.args
            .iter()
            .map(OsString::as_os_str)
            .chain(self.flags.args().into_iter().map(OsStr::new))
    }

    /// The environment changes, in order: `None` removes a variable. Like
    /// [`Command::get_envs`], this does not list the inherited environment.
    pub fn get_envs(&self) -> impl Iterator<Item = (&OsStr, Option<&OsStr>)> {
        self.env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_deref()))
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("serve");
        command.args(&self.args);
        command.args(self.flags.args());
        if self.env_clear {
            command.env_clear();
        }
        for (key, value) in &self.env {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        if let Some(directory) = &self.current_dir {
            command.current_dir(directory);
        }
        command
    }
}

/// A connection over caller-supplied streams instead of a launched process,
/// for tests and for embedding a host in-process.
pub struct Transport {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) terminate: Option<Box<dyn FnOnce() + Send>>,
}

impl Transport {
    pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
            terminate: None,
        }
    }

    /// Called at most once when the client must abandon the transport: a
    /// frame did not finish writing in time, or [`crate::MspClient::force_stop`]
    /// ran. It must make sure a partially written frame can never be
    /// delivered later, for example by closing the peer.
    #[must_use]
    pub fn on_terminate(mut self, terminate: impl FnOnce() + Send + 'static) -> Self {
        self.terminate = Some(Box::new(terminate));
        self
    }
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("terminate", &self.terminate.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_order_is_serve_then_args_then_host_flags() {
        let mut config = LaunchConfig::new("/opt/muse");
        config
            .args(["--trust-workspace"])
            .host_flags(HostFlags {
                no_session_log: true,
                disable_write: true,
                disable_shell: true,
            })
            .env("A", "1")
            .env_remove("B")
            .current_dir("/tmp");
        let command = config.command();
        assert_eq!(command.get_program(), "/opt/muse");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "serve",
                "--trust-workspace",
                "--no-session-log",
                "--disable-write",
                "--disable-shell"
            ]
        );
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&(OsStr::new("A"), Some(OsStr::new("1")))));
        assert!(envs.contains(&(OsStr::new("B"), None)));
        assert_eq!(
            command.get_current_dir(),
            Some(std::path::Path::new("/tmp"))
        );

        let read_only = HostFlags {
            disable_write: true,
            disable_shell: true,
            ..HostFlags::default()
        };
        assert_eq!(read_only.args(), ["--disable-write", "--disable-shell"]);
        assert!(HostFlags::default().args().is_empty());
    }

    #[test]
    fn a_later_server_request_mode_replaces_an_earlier_one() {
        let options = ClientOptions::default()
            .server_request("approval/request", ServerRequestMode::Respond)
            .server_request("approval/request", ServerRequestMode::Acknowledge);
        assert_eq!(
            options.server_request_mode("approval/request"),
            Some(ServerRequestMode::Acknowledge)
        );
        assert_eq!(options.server_request_mode("future/request"), None);
    }
}
