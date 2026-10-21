//! The agent side of ACP: read a client's requests, answer each one once,
//! stream notifications, and ask the client for things.
//!
//! [`AgentConnection::start`] spawns one reader thread. It parses every frame
//! the client sends and hands the result to your sink as an [`AgentEvent`], on
//! that thread, in arrival order. The [`AgentConnection`] it returns is
//! cheap to clone and writes responses, notifications, and requests from any
//! thread. Writes are synchronous: a client that stops reading blocks the
//! writer, as a pipe would.
//!
//! The connection keeps the protocol bookkeeping an agent otherwise writes
//! by hand:
//!
//! - every request the client sends stays *unanswered* until a response for
//!   its id is written, and [`AgentConnection::fail_unanswered`] answers the
//!   rest, for example at shutdown;
//! - a JSON-RPC batch (an array of messages, used by ACP v2) arrives as one
//!   [`Batch`], and [`AgentConnection::hold`] sends its responses back together
//!   as one array;
//! - requests the agent sends to the client are matched with their answers,
//!   either by a blocking [`PendingRequest`] or as a tagged
//!   [`AgentEvent::Response`] delivered to the sink.
//!
//! Frames that are not JSON are answered with `-32700` and reading goes on.
//! Which methods exist, and whether batches are allowed on the negotiated
//! protocol version, is up to the agent.
//!
//! For simple agents, [`serve`] runs an [`AgentHandler`] on the calling thread.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::frame::read_frame;
use crate::protocol::RpcError;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Limits for one agent connection.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// The longest frame the client may send; a longer one ends the
    /// connection.
    pub max_frame_bytes: usize,
    /// How long [`AgentConnection::request`] and [`PendingRequest::wait`]
    /// wait for the client's answer.
    pub request_timeout: Duration,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            max_frame_bytes: 16 * 1024 * 1024,
            request_timeout: Duration::from_secs(120),
        }
    }
}

impl AgentOptions {
    #[must_use]
    pub fn max_frame_bytes(mut self, bytes: usize) -> Self {
        self.max_frame_bytes = bytes;
        self
    }

    #[must_use]
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

/// A JSON-RPC request id (a string or a number), compared by its JSON text.
#[derive(Debug, Clone)]
pub struct RequestId {
    value: Value,
    key: String,
}

impl RequestId {
    #[must_use]
    pub fn new(value: Value) -> Self {
        let key = value.to_string();
        Self { value, key }
    }

    /// The id as the client sent it.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }
}

impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RequestId {}

impl Hash for RequestId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key)
    }
}

/// A request from the client. Answer it exactly once, with
/// [`AgentConnection::respond`] or [`AgentConnection::respond_error`].
#[derive(Debug, Clone)]
pub struct ClientRequest {
    pub id: RequestId,
    pub method: String,
    /// `Value::Null` when the request had no params.
    pub params: Value,
}

impl ClientRequest {
    /// The params decoded into `T`.
    pub fn params_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        T::deserialize(&self.params)
    }
}

/// A notification from the client.
#[derive(Debug, Clone)]
pub struct ClientNotification {
    pub method: String,
    /// `Value::Null` when the notification had no params.
    pub params: Value,
}

/// The client's answer to a request the agent sent.
#[derive(Debug, Clone)]
pub struct ClientResponse {
    pub id: RequestId,
    /// The tag given to [`AgentConnection::request_routed`], or `None` for an
    /// answer this connection was not waiting for: a request the agent wrote
    /// itself with [`AgentConnection::send_raw_frame`], or one it withdrew.
    pub tag: Option<String>,
    /// The request's method, when the connection sent it.
    pub method: Option<String>,
    pub outcome: Result<Value, RpcError>,
}

/// The messages of one JSON-RPC batch, in order.
#[derive(Debug, Clone, Default)]
pub struct Batch {
    /// Its valid messages. Answers to blocking requests are not included.
    pub entries: Vec<AgentEvent>,
    /// One error per entry that was not a JSON-RPC message.
    pub invalid: Vec<RpcError>,
}

impl Batch {
    /// The ids of the batch's requests.
    pub fn request_ids(&self) -> impl Iterator<Item = &RequestId> {
        self.entries.iter().filter_map(|entry| match entry {
            AgentEvent::Request(request) => Some(&request.id),
            _ => None,
        })
    }
}

/// What the client sent, as the reader thread delivers it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgentEvent {
    Request(ClientRequest),
    Notification(ClientNotification),
    Response(ClientResponse),
    Batch(Batch),
    /// A JSON value that is not a JSON-RPC message, delivered as-is.
    Invalid(Value),
    /// The client closed the connection, or reading failed. Always last.
    Closed {
        reason: String,
    },
}

/// A failure to write to, or wait on, the client.
#[derive(Debug)]
#[non_exhaustive]
pub enum AgentError {
    /// Writing to the client failed; the connection takes no more writes.
    Write(io::Error),
    /// The connection is closed, or was sealed by
    /// [`AgentConnection::fail_unanswered`].
    Closed,
    /// A value could not be encoded as JSON.
    Json(serde_json::Error),
    /// The client answered the request with an error.
    Rpc(RpcError),
    /// The client did not answer in time. The request is still outstanding;
    /// [`AgentConnection::request`] withdraws it before returning this.
    Timeout { method: String, timeout: Duration },
    /// The request was withdrawn with [`AgentConnection::cancel_request`].
    Withdrawn,
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Write(error) => write!(f, "could not write to the client: {error}"),
            AgentError::Closed => f.write_str("the connection is closed"),
            AgentError::Json(error) => write!(f, "could not encode JSON: {error}"),
            AgentError::Rpc(error) => write!(f, "the client answered with an error: {error}"),
            AgentError::Timeout { method, timeout } => write!(
                f,
                "the client did not answer {method} within {} s",
                timeout.as_secs_f32()
            ),
            AgentError::Withdrawn => f.write_str("the request was withdrawn"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AgentError::Write(error) => Some(error),
            AgentError::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for AgentError {
    fn from(error: serde_json::Error) -> Self {
        AgentError::Json(error)
    }
}

/// JSON text checked to be valid, for the `_raw` methods.
fn raw(value: &Value) -> Result<Box<RawValue>, AgentError> {
    Ok(RawValue::from_string(serde_json::to_string(value)?)?)
}

enum Reply {
    Answer(Result<Value, RpcError>),
    Withdrawn,
    Closed,
}

enum Waiter {
    Blocking(Sender<Reply>),
    Routed { tag: String, method: String },
}

/// Responses held for the batch being answered.
struct HeldBatch {
    ids: HashSet<RequestId>,
    responses: Vec<String>,
    answered: Vec<RequestId>,
}

struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    options: AgentOptions,
    /// No more writes: a write failed, or `fail_unanswered` sealed it.
    sealed: AtomicBool,
    /// The reader thread has ended.
    closed: AtomicBool,
    next_id: AtomicU64,
    pending: Mutex<HashMap<RequestId, Waiter>>,
    unanswered: Mutex<HashSet<RequestId>>,
    batch: Mutex<Option<HeldBatch>>,
}

impl Shared {
    fn write_line(&self, line: &str) -> Result<(), AgentError> {
        let mut writer = lock(&self.writer);
        if self.sealed.load(Ordering::SeqCst) {
            return Err(AgentError::Closed);
        }
        let written = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
        written.map_err(|error| {
            self.sealed.store(true, Ordering::SeqCst);
            AgentError::Write(error)
        })
    }

    /// Write a response line, or hold it for the batch it answers.
    fn write_response(&self, id: Option<&RequestId>, line: String) -> Result<(), AgentError> {
        if let Some(id) = id {
            let mut batch = lock(&self.batch);
            if let Some(held) = batch.as_mut()
                && held.ids.remove(id)
            {
                held.responses.push(line);
                held.answered.push(id.clone());
                return Ok(());
            }
        }
        self.write_line(&line)?;
        if let Some(id) = id {
            lock(&self.unanswered).remove(id);
        }
        Ok(())
    }

    fn error_line(id: Option<&RequestId>, error: &RpcError) -> Result<String, AgentError> {
        Ok(format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{}}}",
            id.map_or("null", |id| id.key.as_str()),
            serde_json::to_string(error)?
        ))
    }

    /// Answer a frame the agent never sees (bad JSON) with an error.
    fn reject(&self, code: i64, message: &str) {
        if let Ok(line) = Self::error_line(None, &RpcError::new(code, message)) {
            let _ = self.write_line(&line);
        }
    }

    fn finish_batch(&self) -> Result<(), AgentError> {
        let Some(held) = lock(&self.batch).take() else {
            return Ok(());
        };
        // JSON-RPC forbids an empty array reply: a batch of notifications and
        // late answers gets none.
        if held.responses.is_empty() {
            return Ok(());
        }
        self.write_line(&format!("[{}]", held.responses.join(",")))?;
        let mut unanswered = lock(&self.unanswered);
        for id in &held.answered {
            unanswered.remove(id);
        }
        Ok(())
    }

    fn close_waiters(&self) {
        let waiters: Vec<Waiter> = lock(&self.pending).drain().map(|(_, w)| w).collect();
        for waiter in waiters {
            if let Waiter::Blocking(reply) = waiter {
                let _ = reply.send(Reply::Closed);
            }
        }
    }
}

/// The writing half of an agent's connection to its client. Clone it freely;
/// every clone writes to the same client.
#[derive(Clone)]
pub struct AgentConnection {
    shared: Arc<Shared>,
}

impl fmt::Debug for AgentConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentConnection")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl AgentConnection {
    /// Start reading the client from `reader` and writing to it on `writer`.
    /// `sink` receives every [`AgentEvent`] on the reader thread, whose handle
    /// is returned with the connection.
    pub fn start<R, W, S>(
        reader: R,
        writer: W,
        options: AgentOptions,
        sink: S,
    ) -> io::Result<(AgentConnection, JoinHandle<()>)>
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
        S: FnMut(AgentEvent) + Send + 'static,
    {
        let shared = Arc::new(Shared {
            writer: Mutex::new(Box::new(writer)),
            options,
            sealed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            unanswered: Mutex::new(HashSet::new()),
            batch: Mutex::new(None),
        });
        let reader_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("acp-agent-reader".to_string())
            .spawn(move || reader_loop(reader, &reader_shared, sink))?;
        Ok((AgentConnection { shared }, thread))
    }

    /// [`AgentConnection::start`] on this process's stdin and stdout.
    pub fn stdio<S>(options: AgentOptions, sink: S) -> io::Result<(AgentConnection, JoinHandle<()>)>
    where
        S: FnMut(AgentEvent) + Send + 'static,
    {
        Self::start(BufReader::new(io::stdin()), io::stdout(), options, sink)
    }

    /// [`AgentConnection::start`] with the events delivered to a channel.
    pub fn channel<R, W>(
        reader: R,
        writer: W,
        options: AgentOptions,
    ) -> io::Result<(AgentConnection, Receiver<AgentEvent>)>
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
    {
        let (events, receiver) = mpsc::channel();
        let (connection, _thread) = Self::start(reader, writer, options, move |event| {
            let _ = events.send(event);
        })?;
        Ok((connection, receiver))
    }

    /// Whether the client has closed the connection.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Answer a request with a result.
    pub fn respond(&self, id: &RequestId, result: Value) -> Result<(), AgentError> {
        self.respond_raw(id, &raw(&result)?)
    }

    /// Answer a request with a result that is already JSON text.
    pub fn respond_raw(&self, id: &RequestId, result: &RawValue) -> Result<(), AgentError> {
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{}}}",
            id.key,
            result.get()
        );
        self.shared.write_response(Some(id), line)
    }

    /// Answer a request with an error. `None` answers with `"id": null`, for
    /// a message whose id could not be read.
    pub fn respond_error(&self, id: Option<&RequestId>, error: RpcError) -> Result<(), AgentError> {
        let line = Shared::error_line(id, &error)?;
        self.shared.write_response(id, line)
    }

    /// Send a notification.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), AgentError> {
        self.notify_raw(method, &raw(&params)?)
    }

    /// Send a notification whose params are already JSON text.
    pub fn notify_raw(&self, method: &str, params: &RawValue) -> Result<(), AgentError> {
        self.shared.write_line(&format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":{},\"params\":{}}}",
            serde_json::to_string(method)?,
            params.get()
        ))
    }

    /// Write one JSON-RPC message as given. The connection does not track it:
    /// an answer to a request sent this way arrives as an untagged
    /// [`AgentEvent::Response`].
    pub fn send_raw_frame(&self, frame: &RawValue) -> Result<(), AgentError> {
        self.shared.write_line(frame.get())
    }

    fn mint_id(&self) -> RequestId {
        let n = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        RequestId::new(Value::String(format!("acp-lite-{n}")))
    }

    fn send(
        &self,
        method: &str,
        params: &RawValue,
        waiter: Waiter,
    ) -> Result<RequestId, AgentError> {
        let id = self.mint_id();
        lock(&self.shared.pending).insert(id.clone(), waiter);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":{},\"params\":{}}}",
            id.key,
            serde_json::to_string(method)?,
            params.get()
        );
        if let Err(error) = self.shared.write_line(&line) {
            lock(&self.shared.pending).remove(&id);
            return Err(error);
        }
        Ok(id)
    }

    /// Send a request to the client and get a handle to wait for its answer.
    pub fn send_request(&self, method: &str, params: Value) -> Result<PendingRequest, AgentError> {
        self.send_request_raw(method, &raw(&params)?)
    }

    /// [`AgentConnection::send_request`] with params that are already JSON
    /// text.
    pub fn send_request_raw(
        &self,
        method: &str,
        params: &RawValue,
    ) -> Result<PendingRequest, AgentError> {
        let (reply, answer) = mpsc::channel();
        let id = self.send(method, params, Waiter::Blocking(reply))?;
        Ok(PendingRequest {
            connection: self.clone(),
            id,
            method: method.to_string(),
            answer,
            done: false,
        })
    }

    /// Send a request and wait for its answer, up to the connection's request
    /// timeout. A request that times out is withdrawn.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, AgentError> {
        self.send_request(method, params)?.wait()
    }

    /// Send a request whose answer arrives later as an
    /// [`AgentEvent::Response`] carrying `tag`. Returns the request's id, for
    /// [`AgentConnection::cancel_request`].
    pub fn request_routed(
        &self,
        method: &str,
        params: Value,
        tag: impl Into<String>,
    ) -> Result<RequestId, AgentError> {
        self.request_routed_raw(method, &raw(&params)?, tag)
    }

    /// [`AgentConnection::request_routed`] with params that are already JSON
    /// text.
    pub fn request_routed_raw(
        &self,
        method: &str,
        params: &RawValue,
        tag: impl Into<String>,
    ) -> Result<RequestId, AgentError> {
        let waiter = Waiter::Routed {
            tag: tag.into(),
            method: method.to_string(),
        };
        self.send(method, params, waiter)
    }

    /// Withdraw a request sent to the client with ACP's `$/cancel_request`.
    /// A blocked waiter gets [`AgentError::Withdrawn`], and a late answer
    /// arrives as an untagged [`AgentEvent::Response`].
    pub fn cancel_request(&self, id: &RequestId) -> Result<(), AgentError> {
        if let Some(Waiter::Blocking(reply)) = lock(&self.shared.pending).remove(id) {
            let _ = reply.send(Reply::Withdrawn);
        }
        self.shared.write_line(&format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"$/cancel_request\",\"params\":{{\"requestId\":{}}}}}",
            id.key
        ))
    }

    /// Hold the responses to `batch`'s requests and send them together, with
    /// an error for each invalid entry, when the guard is finished or dropped.
    /// Responses written after that leave alone, as for a prompt that ends
    /// later. Holding a second batch sends the first.
    #[must_use = "the batch's responses are sent when the guard is finished or dropped"]
    pub fn hold(&self, batch: &Batch) -> BatchGuard {
        let _ = self.shared.finish_batch();
        let mut responses = Vec::new();
        for error in &batch.invalid {
            if let Ok(line) = Shared::error_line(None, error) {
                responses.push(line);
            }
        }
        *lock(&self.shared.batch) = Some(HeldBatch {
            ids: batch.request_ids().cloned().collect(),
            responses,
            answered: Vec::new(),
        });
        BatchGuard {
            connection: self.clone(),
            finished: false,
        }
    }

    /// The requests the client sent that have no response yet.
    #[must_use]
    pub fn unanswered(&self) -> Vec<RequestId> {
        lock(&self.shared.unanswered).iter().cloned().collect()
    }

    /// Answer every unanswered request with this error, then take no more
    /// writes. For shutdown: the client gets an answer for every request,
    /// even ones the agent never got to.
    pub fn fail_unanswered(&self, code: i64, message: &str) {
        let _ = self.shared.finish_batch();
        let ids: Vec<RequestId> = lock(&self.shared.unanswered).drain().collect();
        let error = RpcError::new(code, message);
        for id in &ids {
            if let Ok(line) = Shared::error_line(Some(id), &error)
                && self.shared.write_line(&line).is_err()
            {
                break;
            }
        }
        self.shared.sealed.store(true, Ordering::SeqCst);
    }
}

/// Sends a held batch's responses when finished or dropped.
pub struct BatchGuard {
    connection: AgentConnection,
    finished: bool,
}

impl BatchGuard {
    /// Send the batch's responses now.
    pub fn finish(mut self) -> Result<(), AgentError> {
        self.finished = true;
        self.connection.shared.finish_batch()
    }
}

impl Drop for BatchGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.connection.shared.finish_batch();
        }
    }
}

/// A request sent to the client, waiting for its answer. Dropping it stops
/// waiting without withdrawing the request.
pub struct PendingRequest {
    connection: AgentConnection,
    id: RequestId,
    method: String,
    answer: Receiver<Reply>,
    done: bool,
}

impl fmt::Debug for PendingRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingRequest")
            .field("id", &self.id)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl PendingRequest {
    #[must_use]
    pub fn id(&self) -> &RequestId {
        &self.id
    }

    /// Wait up to the connection's request timeout; withdraw the request if
    /// it runs out.
    pub fn wait(mut self) -> Result<Value, AgentError> {
        let timeout = self.connection.shared.options.request_timeout;
        match self.wait_timeout(timeout) {
            Some(outcome) => outcome,
            None => {
                let _ = self.connection.cancel_request(&self.id);
                self.done = true;
                Err(AgentError::Timeout {
                    method: self.method.clone(),
                    timeout,
                })
            }
        }
    }

    /// Wait up to `timeout`. `None` means no answer yet; the request stays
    /// outstanding, so this can be called in a loop that also checks for
    /// cancellation.
    pub fn wait_timeout(&mut self, timeout: Duration) -> Option<Result<Value, AgentError>> {
        if self.done {
            return Some(Err(AgentError::Closed));
        }
        let reply = match self.answer.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => Reply::Closed,
        };
        self.done = true;
        Some(match reply {
            Reply::Answer(Ok(result)) => Ok(result),
            Reply::Answer(Err(error)) => Err(AgentError::Rpc(error)),
            Reply::Withdrawn => Err(AgentError::Withdrawn),
            Reply::Closed => Err(AgentError::Closed),
        })
    }

    /// Withdraw the request with `$/cancel_request`.
    pub fn cancel(mut self) -> Result<(), AgentError> {
        self.done = true;
        self.connection.cancel_request(&self.id)
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if !self.done {
            lock(&self.connection.shared.pending).remove(&self.id);
        }
    }
}

/// How one message of a frame was handled.
enum Parsed {
    Event(AgentEvent),
    /// An answer delivered to a blocking waiter.
    Consumed,
    /// Not a JSON-RPC message.
    Invalid(Value),
}

fn reader_loop<R, S>(mut reader: R, shared: &Shared, mut sink: S)
where
    R: BufRead,
    S: FnMut(AgentEvent),
{
    let reason = loop {
        let frame = match read_frame(&mut reader, shared.options.max_frame_bytes) {
            Ok(Some(frame)) => frame,
            Ok(None) => break "the client closed the connection".to_string(),
            Err(error) => break format!("could not read an ACP frame: {error}"),
        };
        let Ok(text) = std::str::from_utf8(&frame) else {
            shared.reject(-32700, "parse error: invalid UTF-8 frame");
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                shared.reject(-32700, &format!("parse error: {error}"));
                continue;
            }
        };
        match value {
            Value::Array(entries) if entries.is_empty() => {
                shared.reject(-32600, "invalid request: empty batch");
            }
            Value::Array(entries) => {
                let mut batch = Batch::default();
                for entry in entries {
                    match read_message(shared, entry) {
                        Parsed::Event(event) => batch.entries.push(event),
                        Parsed::Consumed => {}
                        Parsed::Invalid(_) => batch.invalid.push(RpcError::new(
                            -32600,
                            "invalid request: batch entry is not a JSON-RPC message",
                        )),
                    }
                }
                sink(AgentEvent::Batch(batch));
            }
            value => match read_message(shared, value) {
                Parsed::Event(event) => sink(event),
                Parsed::Consumed => {}
                Parsed::Invalid(value) => sink(AgentEvent::Invalid(value)),
            },
        }
    };
    shared.closed.store(true, Ordering::SeqCst);
    shared.close_waiters();
    sink(AgentEvent::Closed { reason });
}

fn read_message(shared: &Shared, value: Value) -> Parsed {
    let Value::Object(mut message) = value else {
        return Parsed::Invalid(value);
    };
    let params = message.remove("params").unwrap_or(Value::Null);
    match message.get("method") {
        Some(Value::String(method)) => {
            let method = method.clone();
            match message.remove("id") {
                Some(id) => {
                    let id = RequestId::new(id);
                    lock(&shared.unanswered).insert(id.clone());
                    Parsed::Event(AgentEvent::Request(ClientRequest { id, method, params }))
                }
                None => Parsed::Event(AgentEvent::Notification(ClientNotification {
                    method,
                    params,
                })),
            }
        }
        Some(_) => {
            message.insert("params".to_string(), params);
            Parsed::Invalid(Value::Object(message))
        }
        None => {
            let (Some(id), true) = (
                message.get("id").cloned(),
                message.contains_key("result") || message.contains_key("error"),
            ) else {
                message.insert("params".to_string(), params);
                return Parsed::Invalid(Value::Object(message));
            };
            let id = RequestId::new(id);
            let outcome = match message.remove("error") {
                Some(error) => Err(serde_json::from_value::<RpcError>(error)
                    .unwrap_or_else(|_| RpcError::new(-32603, "malformed error response"))),
                None => Ok(message.remove("result").unwrap_or(Value::Null)),
            };
            match lock(&shared.pending).remove(&id) {
                Some(Waiter::Blocking(reply)) => {
                    let _ = reply.send(Reply::Answer(outcome));
                    Parsed::Consumed
                }
                Some(Waiter::Routed { tag, method }) => {
                    Parsed::Event(AgentEvent::Response(ClientResponse {
                        id,
                        tag: Some(tag),
                        method: Some(method),
                        outcome,
                    }))
                }
                None => Parsed::Event(AgentEvent::Response(ClientResponse {
                    id,
                    tag: None,
                    method: None,
                    outcome,
                })),
            }
        }
    }
}

/// A simple agent: [`serve`] calls it for each message, one at a time, on
/// the thread that called `serve`.
pub trait AgentHandler {
    /// Answer one request. The connection can stream notifications and send
    /// blocking requests to the client before returning.
    fn handle_request(
        &mut self,
        connection: &AgentConnection,
        request: ClientRequest,
    ) -> Result<Value, RpcError>;

    /// Handle one notification. Ignores it by default.
    fn handle_notification(
        &mut self,
        _connection: &AgentConnection,
        _notification: ClientNotification,
    ) {
    }
}

/// Run `handler` until the client closes the connection. Each request is
/// answered with the handler's result; batches are answered together.
/// Requests run one at a time, so a long request delays the next message:
/// agents that must stay responsive during a prompt should drive
/// [`AgentConnection::start`] themselves.
pub fn serve<R, W, H>(reader: R, writer: W, options: AgentOptions, mut handler: H) -> io::Result<()>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
    H: AgentHandler,
{
    let (connection, events) = AgentConnection::channel(reader, writer, options)?;
    for event in events {
        match event {
            AgentEvent::Batch(batch) => {
                let guard = connection.hold(&batch);
                for entry in batch.entries {
                    dispatch(&connection, &mut handler, entry);
                }
                let _ = guard.finish();
            }
            AgentEvent::Closed { .. } => break,
            event => dispatch(&connection, &mut handler, event),
        }
    }
    Ok(())
}

/// [`serve`] on this process's stdin and stdout.
pub fn serve_stdio<H: AgentHandler>(options: AgentOptions, handler: H) -> io::Result<()> {
    serve(BufReader::new(io::stdin()), io::stdout(), options, handler)
}

fn dispatch<H: AgentHandler>(connection: &AgentConnection, handler: &mut H, event: AgentEvent) {
    match event {
        AgentEvent::Request(request) => {
            let id = request.id.clone();
            let _ = match handler.handle_request(connection, request) {
                Ok(result) => connection.respond(&id, result),
                Err(error) => connection.respond_error(Some(&id), error),
            };
        }
        AgentEvent::Notification(notification) => {
            handler.handle_notification(connection, notification);
        }
        AgentEvent::Invalid(_) => {
            let _ = connection.respond_error(None, RpcError::new(-32600, "invalid request"));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::PipeWriter;
    use std::time::Instant;

    /// What the agent wrote, shared with the test.
    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);

    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            lock(&self.0).extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Output {
        fn lines(&self) -> Vec<Value> {
            String::from_utf8(lock(&self.0).clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }

        /// Wait until `count` lines have been written.
        fn wait_lines(&self, count: usize) -> Vec<Value> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let lines = self.lines();
                if lines.len() >= count {
                    return lines;
                }
                assert!(Instant::now() < deadline, "only {lines:?} written");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    struct Harness {
        connection: AgentConnection,
        input: Option<PipeWriter>,
        events: Receiver<AgentEvent>,
        output: Output,
    }

    impl Harness {
        fn new() -> Harness {
            let (reader, input) = io::pipe().unwrap();
            let output = Output::default();
            let (connection, events) = AgentConnection::channel(
                BufReader::new(reader),
                output.clone(),
                AgentOptions::default(),
            )
            .unwrap();
            Harness {
                connection,
                input: Some(input),
                events,
                output,
            }
        }

        fn send(&mut self, bytes: &[u8]) {
            let input = self.input.as_mut().unwrap();
            input.write_all(bytes).unwrap();
            input.write_all(b"\n").unwrap();
        }

        fn next(&self) -> AgentEvent {
            self.events
                .recv_timeout(Duration::from_secs(5))
                .expect("an event")
        }

        fn request(&mut self, frame: &str) -> ClientRequest {
            self.send(frame.as_bytes());
            match self.next() {
                AgentEvent::Request(request) => request,
                other => panic!("expected a request, got {other:?}"),
            }
        }
    }

    #[test]
    fn requests_notifications_and_other_values_are_told_apart() {
        let mut h = Harness::new();
        let request = h.request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
        );
        assert_eq!(request.method, "initialize");
        assert_eq!(request.id.value(), &json!(1));
        assert_eq!(request.params["protocolVersion"], 1);
        assert_eq!(h.connection.unanswered(), std::slice::from_ref(&request.id));

        h.send(br#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s"}}"#);
        match h.next() {
            AgentEvent::Notification(note) => {
                assert_eq!(
                    (note.method.as_str(), &note.params["sessionId"]),
                    ("session/cancel", &json!("s"))
                );
            }
            other => panic!("{other:?}"),
        }
        h.send(b"42");
        assert!(matches!(h.next(), AgentEvent::Invalid(value) if value == json!(42)));
        h.send(br#"{"jsonrpc":"2.0","method":7}"#);
        assert!(matches!(h.next(), AgentEvent::Invalid(_)));

        h.connection
            .respond(&request.id, json!({"protocolVersion": 1}))
            .unwrap();
        assert_eq!(
            h.output.wait_lines(1)[0],
            json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 1}})
        );
        assert!(h.connection.unanswered().is_empty());
    }

    #[test]
    fn bad_frames_are_answered_and_reading_goes_on() {
        let mut h = Harness::new();
        h.send(b"not json");
        h.send(b"\xff\xfe");
        h.send(b"[]");
        h.send(b"   ");
        let request = h.request(r#"{"jsonrpc":"2.0","id":"x","method":"ping"}"#);
        assert_eq!(request.method, "ping");
        let lines = h.output.wait_lines(3);
        let errors: Vec<(i64, bool)> = lines
            .iter()
            .map(|line| {
                (
                    line["error"]["code"].as_i64().unwrap(),
                    line["id"].is_null(),
                )
            })
            .collect();
        assert_eq!(errors, [(-32700, true), (-32700, true), (-32600, true)]);
        assert!(
            lines[1]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("UTF-8")
        );
        assert!(
            lines[2]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("empty batch")
        );
    }

    #[test]
    fn a_batch_is_answered_together_with_its_invalid_entries() {
        let mut h = Harness::new();
        h.send(br#"[{"jsonrpc":"2.0","id":"a","method":"x"},{"jsonrpc":"2.0","method":"note"},5,{"jsonrpc":"2.0","id":"b","method":"y"}]"#);
        let AgentEvent::Batch(batch) = h.next() else {
            panic!("expected a batch")
        };
        assert_eq!(batch.entries.len(), 3);
        assert_eq!(batch.invalid.len(), 1);
        let ids: Vec<RequestId> = batch.request_ids().cloned().collect();
        assert_eq!(
            ids,
            [RequestId::new(json!("a")), RequestId::new(json!("b"))]
        );
        let guard = h.connection.hold(&batch);
        h.connection.respond(&ids[1], json!("second")).unwrap();
        h.connection.respond(&ids[0], json!("first")).unwrap();
        assert!(
            h.output.lines().is_empty(),
            "held until the batch is finished"
        );
        guard.finish().unwrap();
        let lines = h.output.wait_lines(1);
        let replies = lines[0].as_array().expect("one array reply");
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["error"]["code"], -32600);
        assert_eq!(
            (&replies[1]["id"], &replies[1]["result"]),
            (&json!("b"), &json!("second"))
        );
        assert_eq!(
            (&replies[2]["id"], &replies[2]["result"]),
            (&json!("a"), &json!("first"))
        );
        assert!(h.connection.unanswered().is_empty());

        // A batch with nothing to answer writes nothing; a response after
        // the batch ends leaves on its own.
        h.send(br#"[{"jsonrpc":"2.0","method":"note"},{"jsonrpc":"2.0","id":"c","method":"z"}]"#);
        let AgentEvent::Batch(batch) = h.next() else {
            panic!("expected a batch")
        };
        drop(h.connection.hold(&batch));
        assert_eq!(h.output.lines().len(), 1);
        h.connection
            .respond(&RequestId::new(json!("c")), json!(null))
            .unwrap();
        assert_eq!(h.output.wait_lines(2)[1]["id"], "c");
    }

    #[test]
    fn unanswered_requests_fail_at_shutdown_and_the_connection_is_sealed() {
        let mut h = Harness::new();
        let first = h.request(r#"{"jsonrpc":"2.0","id":1,"method":"a"}"#);
        let second = h.request(r#"{"jsonrpc":"2.0","id":2,"method":"b"}"#);
        h.connection.respond(&first.id, json!({})).unwrap();
        h.connection.fail_unanswered(-32603, "shutting down");
        let lines = h.output.wait_lines(2);
        assert_eq!(
            lines[1],
            json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32603, "message": "shutting down"}})
        );
        assert!(matches!(
            h.connection.respond(&second.id, json!({})),
            Err(AgentError::Closed)
        ));
        assert!(matches!(
            h.connection.notify("x", json!({})),
            Err(AgentError::Closed)
        ));
    }

    #[test]
    fn a_blocking_request_gets_its_answer() {
        let mut h = Harness::new();
        let pending = h
            .connection
            .send_request("fs/read_text_file", json!({"path": "/a"}))
            .unwrap();
        assert_eq!(pending.id().value(), &json!("acp-lite-1"));
        let sent = &h.output.wait_lines(1)[0];
        assert_eq!(
            (&sent["method"], &sent["params"]["path"]),
            (&json!("fs/read_text_file"), &json!("/a"))
        );
        h.send(br#"{"jsonrpc":"2.0","id":"acp-lite-1","result":{"content":"text"}}"#);
        assert_eq!(pending.wait().unwrap(), json!({"content": "text"}));

        let pending = h
            .connection
            .send_request("fs/read_text_file", json!({}))
            .unwrap();
        h.send(
            br#"{"jsonrpc":"2.0","id":"acp-lite-2","error":{"code":-32002,"message":"not found"}}"#,
        );
        match pending.wait() {
            Err(AgentError::Rpc(error)) => {
                assert_eq!((error.code, error.message.as_str()), (-32002, "not found"))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_withdrawn_request_is_cancelled_and_its_late_answer_is_untagged() {
        let mut h = Harness::new();
        let mut pending = h
            .connection
            .send_request("terminal/wait_for_exit", json!({}))
            .unwrap();
        assert!(
            pending.wait_timeout(Duration::from_millis(20)).is_none(),
            "still outstanding"
        );
        pending.cancel().unwrap();
        let lines = h.output.wait_lines(2);
        assert_eq!(
            lines[1],
            json!({"jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": "acp-lite-1"}})
        );
        h.send(br#"{"jsonrpc":"2.0","id":"acp-lite-1","result":{}}"#);
        match h.next() {
            AgentEvent::Response(response) => {
                assert_eq!((response.tag, response.method), (None, None));
                assert_eq!(response.id, RequestId::new(json!("acp-lite-1")));
            }
            other => panic!("{other:?}"),
        }

        // A blocked waiter learns its request was withdrawn.
        let pending = h.connection.send_request("x", json!({})).unwrap();
        let id = pending.id().clone();
        let waiter = thread::spawn(move || pending.wait());
        thread::sleep(Duration::from_millis(20));
        h.connection.cancel_request(&id).unwrap();
        assert!(matches!(waiter.join().unwrap(), Err(AgentError::Withdrawn)));
    }

    #[test]
    fn routed_answers_arrive_as_tagged_events() {
        let mut h = Harness::new();
        let id = h
            .connection
            .request_routed(
                "session/request_permission",
                json!({"sessionId": "s1"}),
                "permission:s1",
            )
            .unwrap();
        h.send(
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"result":{{"outcome":{{"outcome":"cancelled"}}}}}}"#
            )
            .as_bytes(),
        );
        match h.next() {
            AgentEvent::Response(response) => {
                assert_eq!(response.tag.as_deref(), Some("permission:s1"));
                assert_eq!(
                    response.method.as_deref(),
                    Some("session/request_permission")
                );
                assert_eq!(response.outcome.unwrap()["outcome"]["outcome"], "cancelled");
            }
            other => panic!("{other:?}"),
        }
        let id = h
            .connection
            .request_routed("elicitation/create", json!({}), "form")
            .unwrap();
        h.send(
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32800,"message":"cancelled"}}}}"#
            )
            .as_bytes(),
        );
        match h.next() {
            AgentEvent::Response(response) => {
                assert_eq!(response.tag.as_deref(), Some("form"));
                assert_eq!(response.outcome.unwrap_err().code, -32800);
            }
            other => panic!("{other:?}"),
        }
    }

    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_write_seals_the_connection() {
        let (reader, _input) = io::pipe().unwrap();
        let (connection, _events) =
            AgentConnection::channel(BufReader::new(reader), Broken, AgentOptions::default())
                .unwrap();
        assert!(matches!(
            connection.notify("x", json!({})),
            Err(AgentError::Write(_))
        ));
        assert!(matches!(
            connection.notify("x", json!({})),
            Err(AgentError::Closed)
        ));
    }

    #[test]
    fn closing_the_input_ends_with_closed_and_frees_waiters() {
        let mut h = Harness::new();
        let pending = h.connection.send_request("x", json!({})).unwrap();
        drop(h.input.take());
        match h.next() {
            AgentEvent::Closed { reason } => assert!(reason.contains("closed"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(h.connection.is_closed());
        assert!(matches!(pending.wait(), Err(AgentError::Closed)));
    }

    #[test]
    fn serve_answers_each_request_with_the_handler() {
        struct Echo;
        impl AgentHandler for Echo {
            fn handle_request(
                &mut self,
                _: &AgentConnection,
                request: ClientRequest,
            ) -> Result<Value, RpcError> {
                match request.method.as_str() {
                    "echo" => Ok(request.params),
                    other => Err(RpcError::new(-32601, format!("no {other}"))),
                }
            }
        }
        let (reader, mut input) = io::pipe().unwrap();
        let output = Output::default();
        let served = output.clone();
        let server = thread::spawn(move || {
            serve(
                BufReader::new(reader),
                served,
                AgentOptions::default(),
                Echo,
            )
        });
        input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"echo\",\"params\":[1,2]}\n")
            .unwrap();
        input.write_all(b"[{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"echo\",\"params\":\"b\"},{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"nope\"}]\n").unwrap();
        let lines = output.wait_lines(2);
        drop(input);
        server.join().unwrap().unwrap();
        assert_eq!(lines[0]["result"], json!([1, 2]));
        let batch = lines[1].as_array().unwrap();
        assert_eq!(batch[0]["result"], "b");
        assert_eq!(batch[1]["error"]["code"], -32601);
    }
}
