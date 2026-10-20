//! The MSP connection: NDJSON JSON-RPC over a host's stdin and stdout.
//!
//! Model (per the Muse Code developer docs and the schema the host ships):
//! commands carry caller-minted `commandId`s; the response is an admission
//! ack, not the outcome; `item/*` and `turn/*` notifications report what
//! happened. One writer thread serializes frames, one reader thread routes
//! responses to their waiters and everything else to the event channel.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant};

use crate::config::{ClientOptions, LaunchConfig, ServerRequestMode, ShutdownPolicy, Transport};
use crate::error::{Diagnostic, HandshakeError, LaunchError, MspError};
use crate::exit::{ExitClassification, classify_status};
use crate::handshake::{Handshake, default_check, initialize_params};
use crate::json::{J, esc, j_to_string, parse_json};
use crate::process::{POLL, StderrDone, StderrTail, drain_stderr, kill_and_reap, terminate_child};

/// Default admission-ack budget for methods the table does not name.
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// Method-aware admission-ack budgets. Acks are admission-only, not outcomes:
/// a turn may legitimately run for minutes after `turn/start` accepts.
///
/// Retry policy: commands that carry a caller-minted `commandId` may be
/// retried after a timeout *with the same handle* (the host answers a
/// value-identical ack); query-shaped methods without one (`model/list`)
/// may be re-issued. Never mint a fresh `commandId` for a retry.
pub fn default_timeout(method: &str) -> Duration {
    let ms = match method {
        // Handshake: bounded startup, but leave room for cold binary start.
        "initialize" => 30_000,
        // Lifecycle/history work can page and replay large views.
        "session/start" | "session/resume" | "session/read" | "view/page" => 180_000,
        // Cheap queries.
        "model/list" | "session/list" | "usage/read" | "view/subscribe" | "view/unsubscribe"
        | "item/readOutput" | "account/read" => 30_000,
        // Control-plane decisions should be fast but not flaky.
        "approval/decide" | "userInput/answer" | "userInput/cancel" | "userInput/clarify"
        | "task/background" | "task/stop" | "task/stopAll" => 30_000,
        // Plugin installs and marketplace fetches hold the request open while
        // the host talks to a store or local source.
        "plugin/install" | "plugin/marketplaceAdd" | "plugin/marketplaceUpdate" => 180_000,
        _ => DEFAULT_TIMEOUT_MS,
    };
    Duration::from_millis(ms)
}

/// Something the host sent that is not a response to one of our requests.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MspEvent {
    Notification {
        method: String,
        params: J,
    },
    /// A server-initiated request for a method with a configured
    /// [`ServerRequestMode`]. In `Acknowledge` mode it was already answered
    /// `{}`; in `Respond` mode answer it with [`MspClient::respond`].
    Request {
        id: J,
        method: String,
        params: J,
    },
    /// One line of host stderr, when enabled with
    /// [`ClientOptions::stderr_events`]. Never protocol input.
    Stderr {
        line: String,
    },
    /// The host closed its stdout (or reading it failed). Every pending
    /// request has already failed. Sent once per connection.
    Closed {
        reason: String,
    },
}

/// JSON params, result, or error text for one frame.
#[derive(Debug, Clone)]
pub struct Params<'a>(Cow<'a, str>);

impl<'a> Params<'a> {
    /// Caller-built JSON text, sent verbatim. It must be one complete JSON
    /// value; it is not validated.
    pub fn raw(json: impl Into<Cow<'a, str>>) -> Self {
        Params(json.into())
    }

    /// `{}`.
    pub fn empty() -> Params<'static> {
        Params(Cow::Borrowed("{}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'a> From<&'a J> for Params<'a> {
    fn from(value: &'a J) -> Self {
        Params(Cow::Owned(j_to_string(value)))
    }
}

impl From<J> for Params<'static> {
    fn from(value: J) -> Self {
        Params(Cow::Owned(j_to_string(&value)))
    }
}

/// A request that was written and is waiting for its response.
pub struct PendingRequest {
    handle: RequestHandle,
    method: String,
    timeout: Duration,
    /// `None` when the timeout is too large to represent: wait without one.
    deadline: Option<Instant>,
    reply: Receiver<Reply>,
}

impl fmt::Debug for PendingRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingRequest")
            .field("id", &self.handle.id)
            .field("method", &self.method)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl PendingRequest {
    pub fn id(&self) -> u64 {
        self.handle.id
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    /// A cloneable handle that can cancel this request from another thread.
    pub fn handle(&self) -> RequestHandle {
        self.handle.clone()
    }

    /// Block until the response arrives, the request is cancelled, the
    /// connection closes, or the deadline passes. The deadline was set when
    /// the frame finished writing.
    pub fn wait(self) -> Result<J, MspError> {
        let id = self.handle.id;
        let received = match self.deadline {
            Some(deadline) => self
                .reply
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => self
                .reply
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(Reply::Result(result)) => Ok(result),
            Ok(Reply::Error(error)) => Err(MspError::Rpc(error)),
            Ok(Reply::Closed(reason)) => Err(MspError::Closed(reason)),
            Ok(Reply::Cancelled) => Err(MspError::Cancelled {
                method: self.method,
                id,
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.handle.core.remove_pending(&self.handle.key);
                Err(MspError::Timeout {
                    method: self.method,
                    id,
                    timeout: self.timeout,
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.handle.core.remove_pending(&self.handle.key);
                Err(MspError::HostClosed)
            }
        }
    }
}

/// Cancels one pending request. Cancellation is local: MSP has no
/// request-cancellation message, so the host may still act on the frame it
/// already received. Stop work with the MSP command meant for it, such as
/// `turn/cancel`. A response that arrives after cancellation is ignored.
#[derive(Clone)]
pub struct RequestHandle {
    core: Arc<Core>,
    id: u64,
    key: String,
}

impl fmt::Debug for RequestHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestHandle")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl RequestHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Wake the waiter with [`MspError::Cancelled`]. Returns false when the
    /// request had already settled.
    pub fn cancel(&self) -> bool {
        let waiter = self
            .core
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
        match waiter {
            Some(waiter) => {
                let _ = waiter.send(Reply::Cancelled);
                true
            }
            None => false,
        }
    }
}

/// A connection to one Muse host. It is `Send + Sync`; share it with an
/// `Arc` to issue requests from several threads.
///
/// Dropping a client that was never closed closes it in the background,
/// without diagnostics. Call [`MspClient::close`] to wait for the host to
/// exit.
pub struct MspClient {
    core: Arc<Core>,
}

impl fmt::Debug for MspClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MspClient")
            .field("process", &matches!(self.core.end, End::Process(_)))
            .finish_non_exhaustive()
    }
}

/// A UUIDv7 from a millisecond timestamp, the per-host command sequence
/// `n`, and ten random bytes. The sequence's low 12 bits fill `rand_a`, so
/// ids minted in one millisecond stay distinct even with no entropy at all.
pub(crate) fn uuid_v7(n: u64, ms: u64, mut r: [u8; 10]) -> String {
    r[0] ^= ((n >> 8) & 0x0f) as u8;
    r[1] ^= (n & 0xff) as u8;
    r[9] ^= ((n >> 12) & 0xff) as u8;
    let b = u16::from_be_bytes([r[0], r[1]]) & 0x0fff;
    let c = u16::from_be_bytes([r[2], r[3]]) & 0x3fff | 0x8000;
    let d = ((r[4] as u64) << 40)
        | ((r[5] as u64) << 32)
        | ((r[6] as u64) << 24)
        | ((r[7] as u64) << 16)
        | ((r[8] as u64) << 8)
        | (r[9] as u64);
    format!(
        "{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}",
        (ms >> 16) as u32,
        (ms & 0xffff) as u16,
        b,
        c,
        d
    )
}

impl Drop for MspClient {
    fn drop(&mut self) {
        if !self.core.close_started.load(Ordering::SeqCst) {
            let core = self.core.clone();
            // Never panic in drop, even when no thread can be spawned; the
            // host then outlives this client, as it would without the close.
            let _ = std::thread::Builder::new()
                .name("msp-client-close".into())
                .spawn(move || core.close(true));
        }
    }
}

impl MspClient {
    /// Start the configured host and complete the handshake. If the host
    /// exits during the handshake the error carries its classification;
    /// any other handshake failure closes the host.
    pub fn launch(config: LaunchConfig) -> Result<(MspClient, Receiver<MspEvent>), LaunchError> {
        let (client, events) = Self::spawn(config)?;
        match client.initialize() {
            Ok(_) => Ok((client, events)),
            Err(error) => {
                if !matches!(error, HandshakeError::Rejected(_))
                    && let Some(exit) = client.reap()
                {
                    return Err(LaunchError::HostExit(exit));
                }
                client.close();
                Err(LaunchError::Handshake(error))
            }
        }
    }

    /// Start the configured host and connect to it without a handshake.
    /// Call [`MspClient::initialize`] before any session command.
    pub fn spawn(config: LaunchConfig) -> Result<(MspClient, Receiver<MspEvent>), LaunchError> {
        let shutdown = config.shutdown;
        let mut child = config
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| LaunchError::Spawn {
                program: config.program.to_string_lossy().into_owned(),
                error,
            })?;
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                kill_and_reap(&mut child, shutdown.timeout);
                return Err(LaunchError::Startup("serve: no stdout".to_string()));
            }
        };
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                kill_and_reap(&mut child, shutdown.timeout);
                return Err(LaunchError::Startup("serve: no stdin".to_string()));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                kill_and_reap(&mut child, shutdown.timeout);
                return Err(LaunchError::Startup("serve: no stderr".to_string()));
            }
        };
        let (writer_tx, writer_rx) = mpsc::channel();
        std::thread::spawn(move || write_loop(stdin, writer_rx));
        let (tx, rx) = mpsc::channel();
        let tail = StderrTail::default();
        let stderr_done = drain_stderr(
            stderr,
            tail.clone(),
            config.options.stderr_events.then(|| tx.clone()),
        );
        let core = Arc::new(Core::new(
            config.options,
            writer_tx,
            End::Process(Process {
                child: Mutex::new(child),
                stderr: tail,
                stderr_done,
                exit: Mutex::new(None),
                shutdown,
            }),
        ));
        let reader = core.clone();
        std::thread::spawn(move || reader_loop(reader, stdout, tx));
        Ok((MspClient { core }, rx))
    }

    /// Connect over caller-supplied streams. No process is owned, so
    /// [`MspClient::reap`] and [`MspClient::close`] report no exit.
    pub fn connect(
        transport: Transport,
        options: ClientOptions,
    ) -> (MspClient, Receiver<MspEvent>) {
        let Transport {
            reader,
            writer,
            terminate,
        } = transport;
        let (writer_tx, writer_rx) = mpsc::channel();
        std::thread::spawn(move || write_loop(writer, writer_rx));
        let core = Arc::new(Core::new(
            options,
            writer_tx,
            End::Custom(Mutex::new(terminate)),
        ));
        let (tx, rx) = mpsc::channel();
        let reader_core = core.clone();
        std::thread::spawn(move || reader_loop(reader_core, reader, tx));
        (MspClient { core }, rx)
    }

    /// Run the MSP handshake: send `initialize` with the configured client
    /// info and capabilities, check the result, then send `initialized`
    /// (SS1.4.2: no session or turn command is accepted before it).
    pub fn initialize(&self) -> Result<Handshake, HandshakeError> {
        let options = &self.core.options;
        let params = initialize_params(&options.client_info, &options.capabilities);
        let result = self
            .request_with_timeout(
                "initialize",
                Params::raw(params),
                self.core.timeout_for("initialize"),
            )
            .map_err(HandshakeError::Request)?;
        let handshake = Handshake::from_result(result);
        *self
            .core
            .handshake
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(handshake.clone());
        match &options.handshake_check {
            Some(check) => check(&handshake),
            None => default_check(&handshake),
        }
        .map_err(HandshakeError::Rejected)?;
        self.notify("initialized", Params::empty())
            .map_err(HandshakeError::Initialized)?;
        Ok(handshake)
    }

    /// The handshake facts, once `initialize` has answered.
    pub fn handshake(&self) -> Option<Handshake> {
        self.core
            .handshake
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Send a request with the method's default deadline.
    pub fn request<'a>(&self, method: &str, params: impl Into<Params<'a>>) -> Result<J, MspError> {
        self.request_with_timeout(method, params, self.core.timeout_for(method))
    }

    /// Send a request and wait at most `timeout` for its response after the
    /// frame is written.
    pub fn request_with_timeout<'a>(
        &self,
        method: &str,
        params: impl Into<Params<'a>>,
        timeout: Duration,
    ) -> Result<J, MspError> {
        self.start_request(method, params, timeout)?.wait()
    }

    /// Write a request and return without waiting, so another thread can
    /// cancel it through [`PendingRequest::handle`].
    pub fn start_request<'a>(
        &self,
        method: &str,
        params: impl Into<Params<'a>>,
        timeout: Duration,
    ) -> Result<PendingRequest, MspError> {
        let id = self.core.next_id.fetch_add(1, Ordering::SeqCst);
        let key = id.to_string();
        let (tx, rx) = mpsc::channel();
        self.core
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), tx);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":{},\"params\":{}}}",
            esc(method),
            params.into().as_str()
        );
        if let Err(error) = self.core.send_raw(&line) {
            self.core.remove_pending(&key);
            return Err(MspError::Write(error));
        }
        Ok(PendingRequest {
            handle: RequestHandle {
                core: self.core.clone(),
                id,
                key,
            },
            method: method.to_string(),
            timeout,
            deadline: Instant::now().checked_add(timeout),
            reply: rx,
        })
    }

    /// Send a client-to-server notification (no id, no response).
    pub fn notify<'a>(&self, method: &str, params: impl Into<Params<'a>>) -> Result<(), MspError> {
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":{},\"params\":{}}}",
            esc(method),
            params.into().as_str()
        );
        self.core.send_raw(&line).map_err(MspError::Write)
    }

    /// Answer a server request delivered in `Respond` mode.
    pub fn respond<'a>(&self, id: &J, result: impl Into<Params<'a>>) -> Result<(), MspError> {
        self.core
            .respond(id, result.into().as_str())
            .map_err(MspError::Write)
    }

    /// Answer a server request with a JSON-RPC error object.
    pub fn respond_error(&self, id: &J, error: &J) -> Result<(), MspError> {
        self.core
            .send_raw(&format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{}}}",
                j_to_string(id),
                j_to_string(error)
            ))
            .map_err(MspError::Write)
    }

    /// A fresh UUIDv7 `commandId`; the host rejects any other shape. Mint
    /// once per command and reuse it for every retry of that command.
    pub fn mint_command_id(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = self.core.cmd_seq.fetch_add(1, Ordering::SeqCst);
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0) as u64
            & 0xffffffffffff;
        let mut r = [0u8; 10];
        let urandom =
            std::fs::File::open("/dev/urandom").and_then(|mut f| Read::read_exact(&mut f, &mut r));
        if urandom.is_err() {
            // No /dev/urandom (Windows): std's `RandomState` keys come from
            // the OS generator, and each new state differs.
            use std::hash::{BuildHasher, Hasher};
            let word = |salt: u64| {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u64(n);
                h.write_u64(ms);
                h.write_u64(salt);
                h.finish().to_be_bytes()
            };
            r[..8].copy_from_slice(&word(0));
            r[8..].copy_from_slice(&word(1)[..2]);
        }
        uuid_v7(n, ms, r)
    }

    /// The bounded stderr tail of a launched host (at most 100 lines and
    /// 8 KiB). Empty for custom transports.
    pub fn stderr_tail(&self) -> String {
        match &self.core.end {
            End::Process(process) => process.stderr.snapshot(),
            End::Custom(_) => String::new(),
        }
    }

    /// Classify the host's exit if it has exited, waiting at most 250ms for
    /// the exit status and the end of stderr. The first classification is
    /// kept and returned again. `None` while the host runs, and always for
    /// custom transports.
    pub fn reap(&self) -> Option<ExitClassification> {
        self.core.reap()
    }

    /// Whether a close (or forced stop) has started on this client.
    pub fn is_closing(&self) -> bool {
        self.core.close_started.load(Ordering::SeqCst)
    }

    /// Stop the host: fail every pending request, close its stdin, wait for
    /// it to exit within the [`ShutdownPolicy`] grace period, then kill it.
    /// Returns the exit classification, or `None` when the host could not be
    /// observed exiting (or for custom transports). Every step is bounded,
    /// and calling it again only re-checks the already stopped host.
    pub fn close(&self) -> Option<ExitClassification> {
        self.core.close(false)
    }

    /// Last-resort kill without waiting for locks or a graceful exit. Use it
    /// from a watchdog when `close` itself may be stuck. Returns false when
    /// the child could not be reached within 200ms.
    pub fn force_stop(&self) -> bool {
        self.core.force_stop()
    }
}

enum Reply {
    Result(J),
    Error(J),
    Closed(String),
    Cancelled,
}

struct WriteRequest {
    line: String,
    result: Sender<io::Result<()>>,
}

struct Process {
    child: Mutex<Child>,
    stderr: StderrTail,
    stderr_done: StderrDone,
    exit: Mutex<Option<ExitClassification>>,
    shutdown: ShutdownPolicy,
}

type Terminate = Mutex<Option<Box<dyn FnOnce() + Send>>>;

/// What sits behind the pipes.
enum End {
    Process(Process),
    Custom(Terminate),
}

struct Core {
    writer: Mutex<Option<Sender<WriteRequest>>>,
    next_id: AtomicU64,
    cmd_seq: AtomicU64,
    pending: Mutex<HashMap<String, Sender<Reply>>>,
    handshake: Mutex<Option<Handshake>>,
    options: ClientOptions,
    end: End,
    close_started: AtomicBool,
}

impl Core {
    fn new(options: ClientOptions, writer: Sender<WriteRequest>, end: End) -> Core {
        Core {
            writer: Mutex::new(Some(writer)),
            next_id: AtomicU64::new(1),
            cmd_seq: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            handshake: Mutex::new(None),
            options,
            end,
            close_started: AtomicBool::new(false),
        }
    }

    fn timeout_for(&self, method: &str) -> Duration {
        self.options
            .request_timeout
            .unwrap_or_else(|| default_timeout(method))
    }

    fn diagnose(&self, diagnostic: Diagnostic<'_>) {
        if let Some(sink) = &self.options.diagnostics {
            sink(&diagnostic);
        }
    }

    fn arm(&self, step: &'static str) -> Option<Box<dyn Send>> {
        self.options.watchdog.as_ref().map(|arm| arm(step))
    }

    fn remove_pending(&self, key: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(key);
    }

    fn send_raw(&self, line: &str) -> io::Result<()> {
        let sender = self
            .writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "serve is shut down"))?;
        let (result_tx, result_rx) = mpsc::channel();
        sender
            .send(WriteRequest {
                line: line.to_string(),
                result: result_tx,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "serve stdin closed"))?;
        match result_rx.recv_timeout(self.options.write_timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                // A partially written frame cannot be withdrawn from a pipe.
                // Terminate this transport before reporting failure so its
                // writer cannot later deliver an untracked command.
                self.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
                self.terminate();
                self.fail_pending("serve stdin write timed out; host terminated");
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "serve stdin write timed out; host terminated",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "serve stdin writer stopped",
            )),
        }
    }

    fn respond(&self, id: &J, result: &str) -> io::Result<()> {
        self.send_raw(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{result}}}",
            j_to_string(id)
        ))
    }

    /// The typed MSP `methodNotFound` error, matching the reference SDK
    /// client's failure shape.
    fn reply_method_not_found(&self, id: &J, method: &str) {
        let _ = self.send_raw(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":-32601,\"message\":{},\"data\":{{\"kind\":\"methodNotFound\",\"retryable\":false}}}}}}",
            j_to_string(id),
            esc(&format!("method not found: {method}"))
        ));
    }

    fn terminate(&self) {
        match &self.end {
            End::Process(process) => kill_and_reap(
                &mut process.child.lock().unwrap_or_else(|p| p.into_inner()),
                process.shutdown.timeout,
            ),
            End::Custom(terminate) => {
                let hook = terminate.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some(hook) = hook {
                    hook();
                }
            }
        }
    }

    /// Settle every waiter as soon as the connection is known to be dead.
    /// Leaving the senders in `pending` would make callers wait for their
    /// individual deadlines.
    fn fail_pending(&self, reason: &str) {
        // Reject new requests before draining waiters. Otherwise one can be
        // queued between the drain and shutdown.
        self.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
        let waiters = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain()
            .map(|(_, waiter)| waiter)
            .collect::<Vec<_>>();
        for waiter in waiters {
            let _ = waiter.send(Reply::Closed(reason.to_string()));
        }
    }

    /// Wake requests waiting for a response when the host pipe closes.
    /// Without this, a host that dies during initialization leaves its
    /// pending receiver asleep until the full deadline, delaying exit
    /// classification and recovery decisions.
    fn host_closed(&self, events: &Sender<MspEvent>, reason: &str) {
        let _guard = self.arm("host reader disconnect");
        self.writer.lock().unwrap_or_else(|p| p.into_inner()).take();
        let _pending = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain()
            .collect::<Vec<_>>();
        let _ = events.send(MspEvent::Closed {
            reason: reason.to_string(),
        });
    }

    fn reap(&self) -> Option<ExitClassification> {
        let End::Process(process) = &self.end else {
            return None;
        };
        let _guard = self.arm("child reap and stderr drain");
        if let Some(exit) = process
            .exit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Some(exit);
        }
        // The stdout reader can observe EOF a few milliseconds before the
        // parent-visible wait status is published. Poll briefly so the
        // normal EOF path does not discard an otherwise available exit code.
        let deadline = Instant::now() + Duration::from_millis(250);
        let status = loop {
            let result = process
                .child
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .try_wait();
            match result {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) | Err(_) => break None,
            }
        }?;
        process.stderr_done.wait();
        let exit = classify_status(&status, process.stderr.snapshot());
        *process.exit.lock().unwrap_or_else(|p| p.into_inner()) = Some(exit.clone());
        Some(exit)
    }

    /// Close the host input, give it a short chance to exit cleanly, then
    /// kill and reap it. Lock acquisition and child progress are both bounded
    /// so a broken host cannot strand the caller. `quiet` skips diagnostics
    /// and the watchdog, for the background close after a drop.
    fn close(&self, quiet: bool) -> Option<ExitClassification> {
        // Any close, including one the reader starts, makes a later drop a
        // no-op.
        self.close_started.store(true, Ordering::SeqCst);
        let _guard = if quiet {
            None
        } else {
            self.arm("host shutdown")
        };
        let diagnose = |diagnostic: Diagnostic<'_>| {
            if !quiet {
                self.diagnose(diagnostic);
            }
        };
        self.fail_pending("serve shutting down");
        let End::Process(process) = &self.end else {
            return None;
        };
        let deadline = Instant::now() + process.shutdown.timeout;
        let writer = loop {
            match self.writer.try_lock() {
                Ok(mut guard) => break Some(guard.take()),
                Err(TryLockError::Poisoned(poisoned)) => {
                    break Some(poisoned.into_inner().take());
                }
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(POLL);
                }
                Err(TryLockError::WouldBlock) => break None,
            }
        };
        if writer.is_none() {
            diagnose(Diagnostic::ShutdownWriterLockTimeout);
        }
        drop(writer);

        let kill_at = Instant::now() + process.shutdown.grace;
        let mut killed = false;
        let mut exited = None;
        loop {
            let mut child = match process.child.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(POLL);
                    continue;
                }
                Err(TryLockError::WouldBlock) => {
                    diagnose(Diagnostic::ShutdownChildLockTimeout);
                    return None;
                }
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited = Some(status);
                    break;
                }
                Ok(None) if !killed && Instant::now() >= kill_at => {
                    terminate_child(&mut child);
                    killed = true;
                }
                Ok(None) if Instant::now() >= deadline => {
                    diagnose(Diagnostic::ShutdownChildExitTimeout);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    diagnose(Diagnostic::ShutdownWaitFailed { error: &error });
                    break;
                }
            }
            // A watchdog must be able to kill even while this path gives the
            // child its graceful-exit window.
            drop(child);
            std::thread::sleep(POLL);
        }

        process.stderr_done.wait();
        let stderr = process.stderr.snapshot();
        if !stderr.is_empty() {
            // Report the size only: host stderr may contain credentials or
            // workspace contents.
            diagnose(Diagnostic::StderrCaptured {
                bytes: stderr.len(),
            });
        }
        exited.map(|status| classify_status(&status, stderr))
    }

    fn force_stop(&self) -> bool {
        let process = match &self.end {
            End::Process(process) => process,
            End::Custom(terminate) => {
                let hook = match terminate.try_lock() {
                    Ok(mut hook) => hook.take(),
                    Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
                    Err(TryLockError::WouldBlock) => None,
                };
                if let Some(hook) = hook {
                    hook();
                }
                return true;
            }
        };
        let until = Instant::now() + Duration::from_millis(200);
        loop {
            let mut child = match process.child.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) if Instant::now() < until => {
                    std::thread::sleep(POLL);
                    continue;
                }
                Err(TryLockError::WouldBlock) => return false,
            };
            terminate_child(&mut child);
            // `terminate_child` can spend up to two seconds in `taskkill`, so
            // the reap gets a fresh budget.
            let reap_until = Instant::now() + Duration::from_millis(200);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < reap_until => std::thread::sleep(POLL),
                    Ok(None) | Err(_) => break,
                }
            }
            return true;
        }
    }
}

fn write_loop<W: Write>(mut writer: W, requests: Receiver<WriteRequest>) {
    while let Ok(request) = requests.recv() {
        let result = (|| {
            writeln!(writer, "{}", request.line)?;
            writer.flush()
        })();
        let failed = result.as_ref().err().map(|e| (e.kind(), e.to_string()));
        let _ = request.result.send(result);
        let Some((kind, message)) = failed else {
            continue;
        };

        // Every queued writer must settle when the host closes its stdin. Do
        // not leave later sends waiting for the write timeout one by one.
        while let Ok(queued) = requests.try_recv() {
            let _ = queued
                .result
                .send(Err(io::Error::new(kind, message.clone())));
        }
        break;
    }
}

fn reader_loop<R: Read>(core: Arc<Core>, stdout: R, events: Sender<MspEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut frame = Vec::new();
    loop {
        frame.clear();
        match reader.read_until(b'\n', &mut frame) {
            Ok(0) => {
                core.host_closed(&events, "serve host stdout closed");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                core.host_closed(&events, &format!("serve stdout error: {e}"));
                break;
            }
        }
        let line = match std::str::from_utf8(&frame) {
            Ok(line) => line,
            Err(_) => {
                core.diagnose(Diagnostic::InvalidUtf8Frame);
                continue;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg = match parse_json(trimmed) {
            Ok(v) => v,
            Err(e) => {
                core.diagnose(Diagnostic::MalformedFrame { error: &e });
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !method.is_empty()
            && let Some(idv) = id
        {
            // Server-initiated request. A synthetic success result could
            // corrupt host state, so a method without a configured mode gets
            // the typed methodNotFound error.
            let mode = core.options.server_request_mode(&method);
            if mode.is_none() {
                core.diagnose(Diagnostic::UnsupportedServerRequest {
                    method: &method,
                    id: &idv,
                });
                core.reply_method_not_found(&idv, &method);
                continue;
            }
            if mode == Some(ServerRequestMode::Acknowledge) {
                let _ = core.respond(&idv, "{}");
            }
            let params = msg.get("params").cloned().unwrap_or(J::Null);
            let request = MspEvent::Request {
                id: idv,
                method,
                params,
            };
            if events.send(request).is_err() {
                core.fail_pending("serve event loop closed");
                core.close(false);
                break;
            }
            continue;
        }
        if let Some(idv) = id {
            let key = j_to_string(&idv);
            let waiter = core
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&key);
            if let Some(waiter) = waiter {
                if let Some(err) = msg.get("error") {
                    let _ = waiter.send(Reply::Error(err.clone()));
                } else {
                    let _ =
                        waiter.send(Reply::Result(msg.get("result").cloned().unwrap_or(J::Null)));
                }
            } else {
                core.diagnose(Diagnostic::UnknownResponse { id: &key });
            }
            continue;
        }
        if !method.is_empty() {
            core.diagnose(Diagnostic::Notification { method: &method });
            let params = msg.get("params").cloned().unwrap_or(J::Null);
            if events
                .send(MspEvent::Notification { method, params })
                .is_err()
            {
                core.fail_pending("serve event loop closed");
                core.close(false);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests;
