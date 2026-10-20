use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::error::ClientError;
use crate::handler::{ClientHandler, IncomingRequest, RequestContext, RequestOutcome};
use crate::process::{AgentProcess, StderrTail, drain_stderr};
use crate::protocol::{
    InitializeRequest, InitializeResponse, NewSessionRequest, PromptRequest, ProtocolVersion,
    RpcError,
};

/// Runtime limits and shutdown behavior for one client connection.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub request_timeout: Duration,
    pub write_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_frame_bytes: usize,
    pub stderr_tail_lines: usize,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(5),
            max_frame_bytes: 16 * 1024 * 1024,
            stderr_tail_lines: 100,
        }
    }
}

impl ClientOptions {
    #[must_use]
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    #[must_use]
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    #[must_use]
    pub fn max_frame_bytes(mut self, bytes: usize) -> Self {
        self.max_frame_bytes = bytes;
        self
    }

    #[must_use]
    pub fn stderr_tail_lines(mut self, lines: usize) -> Self {
        self.stderr_tail_lines = lines;
        self
    }
}

/// A notification or lifecycle event received from the agent.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ClientEvent {
    SessionUpdate {
        session_id: String,
        update: Value,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    ProtocolError {
        message: String,
    },
    HandlerPanicked {
        method: String,
    },
    Closed {
        reason: String,
    },
}

/// The response to `session/new`.
#[derive(Debug, Clone)]
pub struct NewSessionResponse {
    pub session_id: String,
    pub config_options: Option<Value>,
    pub modes: Option<Value>,
    pub meta: Option<Value>,
    pub raw: Value,
}

/// The response to `session/prompt`.
#[derive(Debug, Clone)]
pub struct PromptResponse {
    /// ACP v2: the inserted user message's id, the prompt's acceptance
    /// receipt. ACP v1 prompts have none.
    pub message_id: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: Option<Value>,
    pub raw: Value,
}

/// A connection that can issue requests and notifications but does not own the
/// agent process.
#[derive(Clone)]
pub struct ClientHandle {
    core: Arc<Core>,
}

impl fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientHandle")
            .field("protocol_version", &self.protocol_version())
            .field("closed", &self.core.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl ClientHandle {
    /// Send a request using the client's default request timeout.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        self.request_with_timeout(method, params, self.core.options.request_timeout)
    }

    /// Send a request with an explicit timeout.
    pub fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        self.core.request(method, params, timeout)
    }

    /// Send a JSON-RPC notification and wait until it is flushed.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), ClientError> {
        self.core.notify(method, params)
    }

    #[must_use]
    pub fn protocol_version(&self) -> Option<ProtocolVersion> {
        self.core.protocol_version()
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.core.closed.load(Ordering::SeqCst)
    }
}

/// A synchronous client for one ACP agent connection.
pub struct Client {
    core: Arc<Core>,
    events: Mutex<Receiver<ClientEvent>>,
    threads: Mutex<Option<ConnectionThreads>>,
    process: Option<ProcessState>,
    stderr_tail: Arc<StderrTail>,
    options: ClientOptions,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("protocol_version", &self.protocol_version())
            .field("closed", &self.core.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Launch an ACP agent and connect over its stdin and stdout.
    pub fn spawn(
        agent: AgentProcess,
        handler: Arc<dyn ClientHandler>,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        let mut child = agent.command().spawn()?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ClientError::Protocol(
                "agent process has no stdout pipe".to_string(),
            ));
        };
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ClientError::Protocol(
                "agent process has no stdin pipe".to_string(),
            ));
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ClientError::Protocol(
                "agent process has no stderr pipe".to_string(),
            ));
        };

        let stderr_tail = StderrTail::new(options.stderr_tail_lines);
        let stderr_thread = drain_stderr(stderr, Arc::clone(&stderr_tail));
        let connected =
            match start_connection(BufReader::new(stdout), stdin, handler, options.clone()) {
                Ok(connected) => connected,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };
        Ok(Self {
            core: connected.core,
            events: Mutex::new(connected.events),
            threads: Mutex::new(Some(connected.threads)),
            process: Some(ProcessState {
                child,
                stderr_thread: Some(stderr_thread),
            }),
            stderr_tail,
            options,
        })
    }

    /// Connect to an already-created ACP transport.
    ///
    /// The writer is owned by the client and is closed by [`Client::close`].
    /// This constructor is useful for in-process agents, custom transports,
    /// and tests. It does not own or terminate the reader.
    pub fn connect<R, W>(
        reader: R,
        writer: W,
        handler: Arc<dyn ClientHandler>,
        options: ClientOptions,
    ) -> Result<Self, ClientError>
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
    {
        let connected = start_connection(reader, writer, handler, options.clone())?;
        Ok(Self {
            core: connected.core,
            events: Mutex::new(connected.events),
            threads: Mutex::new(Some(connected.threads)),
            process: None,
            stderr_tail: StderrTail::new(0),
            options,
        })
    }

    #[must_use]
    pub fn handle(&self) -> ClientHandle {
        ClientHandle {
            core: Arc::clone(&self.core),
        }
    }

    #[must_use]
    pub fn protocol_version(&self) -> Option<ProtocolVersion> {
        self.core.protocol_version()
    }

    /// Initialize the connection and send the required `initialized`
    /// notification.
    pub fn initialize(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, ClientError> {
        self.core.begin_initialize()?;
        let result = self
            .core
            .request(
                "initialize",
                request.to_params(),
                self.core.options.request_timeout,
            )
            .and_then(|raw| {
                InitializeResponse::from_value(raw).map_err(ClientError::InvalidInitialize)
            });
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                self.core.reset_initialize();
                return Err(error);
            }
        };
        self.core.finish_initialize(response.protocol_version);
        if let Err(error) = self.core.notify("initialized", Value::Object(Map::new())) {
            self.core.reset_initialize();
            return Err(error);
        }
        Ok(response)
    }

    /// Create a new ACP session.
    pub fn new_session(
        &self,
        request: NewSessionRequest,
    ) -> Result<NewSessionResponse, ClientError> {
        self.ensure_initialized()?;
        let raw = self.core.request(
            "session/new",
            request.to_params(),
            self.core.options.request_timeout,
        )?;
        let session_id = raw
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ClientError::Protocol("session/new result has no string sessionId".to_string())
            })?
            .to_string();
        Ok(NewSessionResponse {
            session_id,
            config_options: raw.get("configOptions").cloned(),
            modes: raw.get("modes").cloned(),
            meta: raw.get("_meta").cloned(),
            raw,
        })
    }

    /// Load a session and replay its history.
    pub fn load_session(
        &self,
        session_id: &str,
        cwd: impl Into<std::path::PathBuf>,
    ) -> Result<Value, ClientError> {
        self.session_with_cwd("session/load", session_id, cwd.into())
    }

    /// Resume a session without requiring the caller to load it first.
    pub fn resume_session(
        &self,
        session_id: &str,
        cwd: impl Into<std::path::PathBuf>,
    ) -> Result<Value, ClientError> {
        self.session_with_cwd("session/resume", session_id, cwd.into())
    }

    /// Delete a session.
    pub fn delete_session(&self, session_id: &str) -> Result<Value, ClientError> {
        self.ensure_initialized()?;
        self.core.request(
            "session/delete",
            json!({"sessionId": session_id}),
            self.core.options.request_timeout,
        )
    }

    /// Close an active session without deleting its durable history.
    pub fn close_session(&self, session_id: &str) -> Result<Value, ClientError> {
        self.ensure_initialized()?;
        self.core.request(
            "session/close",
            json!({"sessionId": session_id}),
            self.core.options.request_timeout,
        )
    }

    /// List sessions using caller-supplied ACP filters and pagination.
    pub fn list_sessions(&self, params: Value) -> Result<Value, ClientError> {
        self.ensure_initialized()?;
        self.core
            .request("session/list", params, self.core.options.request_timeout)
    }

    /// Send a prompt request.
    ///
    /// ACP v1 normally keeps this request open until the turn terminates. ACP
    /// v2 acknowledges the prompt and publishes progress and completion as
    /// `session/update` notifications.
    pub fn prompt(&self, request: PromptRequest) -> Result<PromptResponse, ClientError> {
        self.ensure_initialized()?;
        let raw = self.core.request(
            "session/prompt",
            request.to_params(),
            self.core.options.request_timeout,
        )?;
        Ok(PromptResponse {
            message_id: raw
                .get("messageId")
                .and_then(Value::as_str)
                .map(str::to_string),
            stop_reason: raw
                .get("stopReason")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage: raw.get("usage").cloned(),
            raw,
        })
    }

    /// Ask the agent to cancel all work in one session.
    pub fn cancel_session(&self, session_id: &str) -> Result<(), ClientError> {
        self.ensure_initialized()?;
        self.core
            .notify("session/cancel", json!({"sessionId": session_id}))
    }

    /// Send an arbitrary ACP request.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        self.core
            .request(method, params, self.core.options.request_timeout)
    }

    /// Send an arbitrary ACP request with an explicit timeout.
    pub fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        self.core.request(method, params, timeout)
    }

    /// Send an arbitrary ACP notification.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), ClientError> {
        self.core.notify(method, params)
    }

    /// Block for the next event.
    pub fn next_event(&self) -> Result<ClientEvent, ClientError> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv()
            .map_err(|_| ClientError::EventStreamClosed)
    }

    /// Wait for the next event for at most `timeout`.
    pub fn next_event_timeout(&self, timeout: Duration) -> Result<ClientEvent, ClientError> {
        match self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv_timeout(timeout)
        {
            Ok(event) => Ok(event),
            Err(RecvTimeoutError::Timeout) => Err(ClientError::EventTimeout { timeout }),
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::EventStreamClosed),
        }
    }

    /// Return one queued event without blocking.
    pub fn try_next_event(&self) -> Result<Option<ClientEvent>, ClientError> {
        match self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_recv()
        {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ClientError::EventStreamClosed),
        }
    }

    /// The most recent stderr lines captured from the agent process.
    #[must_use]
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.snapshot()
    }

    /// Close stdin, wait for the agent to exit, and kill it after the configured
    /// shutdown deadline.
    pub fn close(&mut self) -> Result<(), ClientError> {
        let close_result = self.core.close_transport();
        let mut process = self.process.take();
        if let Some(process) = process.as_mut() {
            let deadline = Instant::now() + self.options.shutdown_timeout;
            loop {
                match process.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) | Err(_) => {
                        let _ = process.child.kill();
                        let _ = process.child.wait();
                        break;
                    }
                }
            }
        }

        let mut threads = self
            .threads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(threads) = threads.as_mut() {
            if let Some(writer) = threads.writer.take() {
                let _ = writer.join();
            }
            if process.is_some()
                && let Some(reader) = threads.reader.take()
            {
                let _ = reader.join();
            }
        }
        if let Some(process) = process.as_mut()
            && let Some(stderr) = process.stderr_thread.take()
        {
            let _ = stderr.join();
        }
        close_result
    }

    fn session_with_cwd(
        &self,
        method: &str,
        session_id: &str,
        cwd: std::path::PathBuf,
    ) -> Result<Value, ClientError> {
        self.ensure_initialized()?;
        self.core.request(
            method,
            json!({
                "sessionId": session_id,
                "cwd": cwd.to_string_lossy(),
            }),
            self.core.options.request_timeout,
        )
    }

    fn ensure_initialized(&self) -> Result<(), ClientError> {
        if self.core.protocol_version().is_some() {
            Ok(())
        } else {
            Err(ClientError::NotInitialized)
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct ProcessState {
    child: Child,
    stderr_thread: Option<JoinHandle<()>>,
}

struct ConnectionThreads {
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
}

struct Connected {
    core: Arc<Core>,
    events: Receiver<ClientEvent>,
    threads: ConnectionThreads,
}

fn start_connection<R, W>(
    reader: R,
    writer: W,
    handler: Arc<dyn ClientHandler>,
    options: ClientOptions,
) -> Result<Connected, ClientError>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    let (event_tx, event_rx) = mpsc::channel();
    let (writer_tx, writer_rx) = mpsc::channel();
    let core = Arc::new(Core {
        next_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        writer: Mutex::new(Some(writer_tx)),
        events: event_tx,
        handler,
        options,
        initialization: Mutex::new(Initialization::Uninitialized),
        closed: AtomicBool::new(false),
    });

    let writer_thread = thread::Builder::new()
        .name("acp-writer".to_string())
        .spawn({
            let writer = writer;
            move || writer_loop(writer, writer_rx)
        })?;
    let reader_core = Arc::clone(&core);
    let reader_thread = thread::Builder::new()
        .name("acp-reader".to_string())
        .spawn(move || reader_loop(reader, reader_core))?;
    Ok(Connected {
        core,
        events: event_rx,
        threads: ConnectionThreads {
            writer: Some(writer_thread),
            reader: Some(reader_thread),
        },
    })
}

enum PendingReply {
    Result(Value),
    Error(RpcError),
    Cancelled,
    Closed(String),
}

enum WriterMessage {
    Frame {
        value: Value,
        result: SyncSender<io::Result<()>>,
    },
    Close {
        result: SyncSender<io::Result<()>>,
    },
}

#[derive(Debug, Clone, Copy)]
enum Initialization {
    Uninitialized,
    InProgress,
    Initialized(ProtocolVersion),
}

struct Core {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Sender<PendingReply>>>,
    writer: Mutex<Option<Sender<WriterMessage>>>,
    events: Sender<ClientEvent>,
    handler: Arc<dyn ClientHandler>,
    options: ClientOptions,
    initialization: Mutex<Initialization>,
    closed: AtomicBool,
}

impl Core {
    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ClientError::ConnectionClosed {
                message: "client is closed".to_string(),
            });
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply_tx, reply_rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, reply_tx);
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.send_frame(frame, self.options.write_timeout) {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            return Err(error);
        }
        match reply_rx.recv_timeout(timeout) {
            Ok(PendingReply::Result(value)) => Ok(value),
            Ok(PendingReply::Error(error)) => Err(ClientError::Rpc(error)),
            Ok(PendingReply::Cancelled) => Err(ClientError::RequestCancelled { id }),
            Ok(PendingReply::Closed(message)) => Err(ClientError::ConnectionClosed { message }),
            Err(RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                Err(ClientError::RequestTimeout {
                    method: method.to_string(),
                    id,
                    timeout,
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::ConnectionClosed {
                message: "request waiter disconnected".to_string(),
            }),
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ClientError::ConnectionClosed {
                message: "client is closed".to_string(),
            });
        }
        self.send_frame(
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
            self.options.write_timeout,
        )
    }

    fn send_frame(&self, value: Value, timeout: Duration) -> Result<(), ClientError> {
        let sender = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| ClientError::ConnectionClosed {
                message: "writer is closed".to_string(),
            })?;
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        sender
            .send(WriterMessage::Frame {
                value,
                result: result_tx,
            })
            .map_err(|_| ClientError::ConnectionClosed {
                message: "writer thread stopped".to_string(),
            })?;
        match result_rx.recv_timeout(timeout) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ClientError::Io(error)),
            Err(RecvTimeoutError::Timeout) => Err(ClientError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out writing an ACP frame",
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "writer thread stopped",
            ))),
        }
    }

    fn close_transport(&self) -> Result<(), ClientError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let result = match self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            Some(sender) => {
                let (result_tx, result_rx) = mpsc::sync_channel(1);
                match sender.send(WriterMessage::Close { result: result_tx }) {
                    Ok(()) => match result_rx.recv_timeout(self.options.write_timeout) {
                        Ok(result) => result.map_err(ClientError::Io),
                        Err(RecvTimeoutError::Timeout) => Err(ClientError::Io(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out closing ACP stdin",
                        ))),
                        Err(RecvTimeoutError::Disconnected) => {
                            Err(ClientError::Io(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "writer thread stopped while closing",
                            )))
                        }
                    },
                    Err(_) => Ok(()),
                }
            }
            None => Ok(()),
        };
        self.fail_pending("client closed the connection");
        let _ = self.events.send(ClientEvent::Closed {
            reason: "client closed the connection".to_string(),
        });
        result
    }

    fn mark_closed(&self, reason: impl Into<String>) {
        let reason = reason.into();
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        drop(
            self.writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
        );
        self.fail_pending(&reason);
        let _ = self.events.send(ClientEvent::Closed { reason });
    }

    fn fail_pending(&self, reason: &str) {
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for (_, waiter) in pending {
            let _ = waiter.send(PendingReply::Closed(reason.to_string()));
        }
    }

    fn begin_initialize(&self) -> Result<(), ClientError> {
        let mut state = self
            .initialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            Initialization::Uninitialized => {
                *state = Initialization::InProgress;
                Ok(())
            }
            Initialization::InProgress => Err(ClientError::InitializationInProgress),
            Initialization::Initialized(_) => Err(ClientError::AlreadyInitialized),
        }
    }

    fn finish_initialize(&self, version: ProtocolVersion) {
        *self
            .initialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Initialization::Initialized(version);
    }

    fn reset_initialize(&self) {
        *self
            .initialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Initialization::Uninitialized;
    }

    fn protocol_version(&self) -> Option<ProtocolVersion> {
        match *self
            .initialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Initialization::Initialized(version) => Some(version),
            Initialization::Uninitialized | Initialization::InProgress => None,
        }
    }

    fn send_response(
        self: &Arc<Self>,
        id: Value,
        outcome: RequestOutcome,
    ) -> Result<(), ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let frame = match outcome {
            RequestOutcome::Result(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
            RequestOutcome::Error(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error,
            }),
        };
        self.send_frame(frame, self.options.write_timeout)
    }

    fn dispatch_request(self: &Arc<Self>, message: &Map<String, Value>) -> Result<(), ClientError> {
        let id = message
            .get("id")
            .cloned()
            .ok_or_else(|| ClientError::Protocol("request is missing id".to_string()))?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| ClientError::Protocol("request has no method".to_string()))?
            .to_string();
        let params = message
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let request = IncomingRequest {
            id: id.clone(),
            method: method.clone(),
            params,
        };
        let core = Arc::clone(self);
        let handler_id = id.clone();
        let spawned = thread::Builder::new()
            .name(format!("acp-handler-{method}"))
            .spawn(move || {
                let context = RequestContext::new(
                    ClientHandle {
                        core: Arc::clone(&core),
                    },
                    core.protocol_version().unwrap_or(ProtocolVersion::Other(0)),
                );
                let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                    core.handler.handle_request(&context, request)
                }));
                match outcome {
                    Ok(outcome) => {
                        let _ = core.send_response(handler_id, outcome);
                    }
                    Err(_) => {
                        let _ = core.events.send(ClientEvent::HandlerPanicked {
                            method: method.clone(),
                        });
                        let _ = core.send_response(
                            handler_id,
                            RequestOutcome::error(
                                -32603,
                                format!("client handler panicked for {method}"),
                            ),
                        );
                    }
                }
            });
        if let Err(error) = spawned {
            self.send_response(
                id,
                RequestOutcome::error(-32603, format!("could not start request handler: {error}")),
            )?;
        }
        Ok(())
    }
}

fn writer_loop<W: Write>(mut writer: W, receiver: Receiver<WriterMessage>) {
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Frame { value, result } => {
                let written = write_frame(&mut writer, &value);
                let failed = written.is_err();
                let _ = result.send(written);
                if failed {
                    break;
                }
            }
            WriterMessage::Close { result } => {
                let _ = writer.flush();
                let _ = result.send(Ok(()));
                break;
            }
        }
    }
}

fn write_frame<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn reader_loop<R: BufRead>(mut reader: R, core: Arc<Core>) {
    loop {
        let frame = match read_frame(&mut reader, core.options.max_frame_bytes) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                core.mark_closed("agent closed stdout");
                return;
            }
            Err(error) => {
                let message = format!("could not read ACP frame: {error}");
                let _ = core.events.send(ClientEvent::ProtocolError {
                    message: message.clone(),
                });
                core.mark_closed(message);
                return;
            }
        };
        if frame.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_slice(&frame) {
            Ok(value) => value,
            Err(error) => {
                let message = format!("invalid JSON-RPC frame: {error}");
                let _ = core.events.send(ClientEvent::ProtocolError {
                    message: message.clone(),
                });
                core.mark_closed(message);
                return;
            }
        };
        let Value::Object(message) = value else {
            let message = "ACP frame must be a JSON object".to_string();
            let _ = core.events.send(ClientEvent::ProtocolError {
                message: message.clone(),
            });
            core.mark_closed(message);
            return;
        };
        if let Err(error) = dispatch_message(&core, &message) {
            let _ = core.events.send(ClientEvent::ProtocolError {
                message: error.to_string(),
            });
            core.mark_closed(error.to_string());
            return;
        }
    }
}

fn dispatch_message(core: &Arc<Core>, message: &Map<String, Value>) -> Result<(), ClientError> {
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        if message.contains_key("id") {
            return core.dispatch_request(message);
        }
        return dispatch_notification(core, method, message);
    }
    if message.contains_key("id") {
        return dispatch_response(core, message);
    }
    Err(ClientError::Protocol(
        "frame has neither a method nor a response id".to_string(),
    ))
}

fn dispatch_response(core: &Arc<Core>, message: &Map<String, Value>) -> Result<(), ClientError> {
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return Ok(());
    };
    let reply = if let Some(error) = message.get("error") {
        PendingReply::Error(
            serde_json::from_value::<RpcError>(error.clone()).map_err(|error| {
                ClientError::Protocol(format!("invalid error response for request {id}: {error}"))
            })?,
        )
    } else if let Some(result) = message.get("result") {
        PendingReply::Result(result.clone())
    } else {
        return Err(ClientError::Protocol(format!(
            "response for request {id} has no result or error"
        )));
    };
    let Some(waiter) = core
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id)
    else {
        return Ok(());
    };
    let _ = waiter.send(reply);
    Ok(())
}

fn dispatch_notification(
    core: &Arc<Core>,
    method: &str,
    message: &Map<String, Value>,
) -> Result<(), ClientError> {
    let params = message
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if method == "$/cancel_request"
        && let Some(id) = params.get("requestId").and_then(Value::as_u64)
        && let Some(waiter) = core
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id)
    {
        let _ = waiter.send(PendingReply::Cancelled);
    }
    if method == "session/update"
        && let Some(session_id) = params.get("sessionId").and_then(Value::as_str)
        && let Some(update) = params.get("update")
    {
        let _ = core.events.send(ClientEvent::SessionUpdate {
            session_id: session_id.to_string(),
            update: update.clone(),
            params,
        });
        return Ok(());
    }
    let _ = core.events.send(ClientEvent::Notification {
        method: method.to_string(),
        params,
    });
    Ok(())
}

fn read_frame<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let (found, consumed, chunk) = {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                return if frame.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(frame))
                };
            }
            match buffer.iter().position(|byte| *byte == b'\n') {
                Some(position) => {
                    let chunk = buffer[..position].to_vec();
                    (true, position + 1, chunk)
                }
                None => (false, buffer.len(), buffer.to_vec()),
            }
        };
        if frame.len().saturating_add(chunk.len()) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ACP frame exceeds {max_bytes} bytes"),
            ));
        }
        frame.extend_from_slice(&chunk);
        reader.consume(consumed);
        if found {
            while matches!(frame.last(), Some(b'\r')) {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::DenyAllHandler;
    use std::io::{Cursor, Read};

    struct ChannelReader {
        receiver: Receiver<Vec<u8>>,
        current: Cursor<Vec<u8>>,
        closed: bool,
    }

    impl ChannelReader {
        fn new(receiver: Receiver<Vec<u8>>) -> Self {
            Self {
                receiver,
                current: Cursor::new(Vec::new()),
                closed: false,
            }
        }

        fn refill(&mut self) -> io::Result<()> {
            if self.closed {
                return Ok(());
            }
            let Ok(mut bytes) = self.receiver.recv() else {
                self.closed = true;
                return Ok(());
            };
            if !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            self.current = Cursor::new(bytes);
            Ok(())
        }
    }

    impl Read for ChannelReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            loop {
                let count = self.current.read(buffer)?;
                if count > 0 {
                    return Ok(count);
                }
                self.refill()?;
                if self.closed {
                    return Ok(0);
                }
            }
        }
    }

    impl BufRead for ChannelReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            loop {
                let available = {
                    let position = self.current.position() as usize;
                    let inner = self.current.get_ref();
                    position < inner.len()
                };
                if available {
                    return self.current.fill_buf();
                }
                self.refill()?;
                if self.closed {
                    return self.current.fill_buf();
                }
            }
        }

        fn consume(&mut self, amount: usize) {
            self.current.consume(amount);
        }
    }

    #[derive(Clone)]
    struct ChannelWriter {
        sender: Sender<Vec<u8>>,
        buffer: Vec<u8>,
    }

    impl Write for ChannelWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.buffer.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if !self.buffer.is_empty() {
                self.sender
                    .send(std::mem::take(&mut self.buffer))
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "peer closed"))?;
            }
            Ok(())
        }
    }

    type TestTransport = (
        Client,
        Receiver<Vec<u8>>,
        Sender<Vec<u8>>,
        Arc<Mutex<Vec<Value>>>,
    );

    fn transport() -> TestTransport {
        let (incoming_tx, incoming_rx) = mpsc::channel();
        let (outgoing_tx, outgoing_rx) = mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler_seen = Arc::clone(&seen);
        let handler = Arc::new(move |_ctx: &RequestContext, request: IncomingRequest| {
            handler_seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(json!({
                    "id": request.id,
                    "method": request.method,
                    "params": request.params,
                }));
            DenyAllHandler.handle_request(_ctx, request)
        });
        let client = Client::connect(
            ChannelReader::new(incoming_rx),
            ChannelWriter {
                sender: outgoing_tx,
                buffer: Vec::new(),
            },
            handler,
            ClientOptions::default().request_timeout(Duration::from_secs(1)),
        )
        .expect("connect");
        (client, outgoing_rx, incoming_tx, seen)
    }

    fn receive_frame(receiver: &Receiver<Vec<u8>>) -> Value {
        let bytes = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("frame");
        serde_json::from_slice(&bytes).expect("json frame")
    }

    fn send_frame(sender: &Sender<Vec<u8>>, value: Value) {
        sender
            .send(serde_json::to_vec(&value).expect("serialize"))
            .expect("send frame");
    }

    #[test]
    fn correlates_responses_and_delivers_events_while_requests_are_pending() {
        let (client, outgoing, incoming, _) = transport();
        let handle = client.handle();
        let request = thread::spawn(move || handle.request("test/slow", json!({"n": 1})));
        let frame = receive_frame(&outgoing);
        assert_eq!(frame["method"], "test/slow");
        assert_eq!(frame["id"], 1);
        send_frame(
            &incoming,
            json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "s-1",
                    "update": {"sessionUpdate": "agent_message_chunk"}
                }
            }),
        );
        let event = client
            .next_event_timeout(Duration::from_secs(1))
            .expect("event");
        assert!(matches!(
            event,
            ClientEvent::SessionUpdate { session_id, .. } if session_id == "s-1"
        ));
        send_frame(
            &incoming,
            json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}),
        );
        assert_eq!(
            request.join().expect("request thread").expect("result")["ok"],
            true
        );
    }

    #[test]
    fn forwards_reverse_requests_to_the_handler() {
        let (client, outgoing, incoming, seen) = transport();
        send_frame(
            &incoming,
            json!({
                "jsonrpc": "2.0",
                "id": "permission-1",
                "method": "session/request_permission",
                "params": {
                    "options": [
                        {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                        {"optionId": "deny", "name": "Deny", "kind": "reject_once"}
                    ]
                }
            }),
        );
        let response = receive_frame(&outgoing);
        assert_eq!(response["id"], "permission-1");
        assert_eq!(response["result"]["outcome"]["optionId"], "deny");
        let seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["method"], "session/request_permission");
        drop(client);
    }

    #[test]
    fn initializes_with_version_specific_capability_fields() {
        let (client, outgoing, incoming, _) = transport();
        let request = thread::spawn(move || client.initialize(InitializeRequest::v2()));
        let frame = receive_frame(&outgoing);
        assert_eq!(frame["params"]["protocolVersion"], 2);
        assert!(frame["params"].get("capabilities").is_some());
        assert!(frame["params"].get("clientCapabilities").is_none());
        assert_eq!(frame["params"]["info"]["name"], "muse-acp-client");
        assert!(frame["params"].get("clientInfo").is_none());
        send_frame(
            &incoming,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": 2,
                    "capabilities": {},
                    "info": {"name": "fake-agent", "version": "1"}
                }
            }),
        );
        let initialized = receive_frame(&outgoing);
        assert_eq!(initialized["method"], "initialized");
        let response = request
            .join()
            .expect("initialize thread")
            .expect("response");
        assert_eq!(response.protocol_version, ProtocolVersion::V2);
    }

    #[test]
    fn request_timeout_does_not_resolve_a_late_response() {
        let (client, outgoing, incoming, _) = transport();
        let error = client
            .request_with_timeout("test/timeout", json!({}), Duration::from_millis(20))
            .expect_err("must time out");
        assert!(matches!(
            error,
            ClientError::RequestTimeout { ref method, id: 1, .. }
                if method == "test/timeout"
        ));
        let frame = receive_frame(&outgoing);
        assert_eq!(frame["id"], 1);
        send_frame(
            &incoming,
            json!({"jsonrpc": "2.0", "id": 1, "result": {"late": true}}),
        );
        let handle = client.handle();
        let next = thread::spawn(move || {
            handle.request_with_timeout("test/next", json!({}), Duration::from_secs(1))
        });
        let next_frame = receive_frame(&outgoing);
        assert_eq!(next_frame["id"], 2);
        send_frame(
            &incoming,
            json!({"jsonrpc": "2.0", "id": 2, "result": {"next": true}}),
        );
        assert_eq!(
            next.join()
                .expect("next request thread")
                .expect("next request")["next"],
            true
        );
    }

    #[test]
    fn eof_fails_pending_requests_and_publishes_closed() {
        let (client, outgoing, incoming, _) = transport();
        let handle = client.handle();
        let request = thread::spawn(move || handle.request("test/pending", json!({})));
        receive_frame(&outgoing);
        drop(incoming);
        let error = request.join().expect("request thread").expect_err("closed");
        assert!(matches!(error, ClientError::ConnectionClosed { .. }));
        let event = client
            .next_event_timeout(Duration::from_secs(1))
            .expect("closed event");
        assert!(matches!(event, ClientEvent::Closed { .. }));
    }

    #[test]
    fn agent_can_cancel_a_client_request() {
        let (client, outgoing, incoming, _) = transport();
        let handle = client.handle();
        let request = thread::spawn(move || handle.request("test/cancel", json!({})));
        let frame = receive_frame(&outgoing);
        assert_eq!(frame["id"], 1);
        send_frame(
            &incoming,
            json!({
                "jsonrpc": "2.0",
                "method": "$/cancel_request",
                "params": {"requestId": 1}
            }),
        );
        let error = request
            .join()
            .expect("request thread")
            .expect_err("cancelled");
        assert!(matches!(error, ClientError::RequestCancelled { id: 1 }));
    }

    #[test]
    fn malformed_frames_report_protocol_failure_and_close() {
        let (client, _outgoing, incoming, _) = transport();
        incoming
            .send(b"not-json".to_vec())
            .expect("send malformed frame");
        let protocol_error = client
            .next_event_timeout(Duration::from_secs(1))
            .expect("protocol error event");
        assert!(matches!(protocol_error, ClientEvent::ProtocolError { .. }));
        let closed = client
            .next_event_timeout(Duration::from_secs(1))
            .expect("closed event");
        assert!(matches!(closed, ClientEvent::Closed { .. }));
    }

    #[test]
    fn a_panicking_handler_returns_one_internal_error() {
        let (incoming_tx, incoming_rx) = mpsc::channel();
        let (outgoing_tx, outgoing_rx) = mpsc::channel();
        let client = Client::connect(
            ChannelReader::new(incoming_rx),
            ChannelWriter {
                sender: outgoing_tx,
                buffer: Vec::new(),
            },
            Arc::new(|_ctx: &RequestContext, _request: IncomingRequest| {
                panic!("handler failure");
            }),
            ClientOptions::default(),
        )
        .expect("connect");
        send_frame(
            &incoming_tx,
            json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "session/request_permission",
                "params": {}
            }),
        );
        let response = receive_frame(&outgoing_rx);
        assert_eq!(response["id"], 11);
        assert_eq!(response["error"]["code"], -32603);
        let event = client
            .next_event_timeout(Duration::from_secs(1))
            .expect("panic event");
        assert!(matches!(event, ClientEvent::HandlerPanicked { .. }));
    }
}
