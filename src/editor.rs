//! The editor connection, on the official ACP SDK (`agent-client-protocol`).
//!
//! The SDK owns everything about the wire: newline framing, JSON-RPC parsing
//! and its errors (`-32700` for a bad line, `-32600` for a batch or a value
//! that is no message), protocol-version selection between ACP v1 and v2
//! through [`AgentProtocolRouter`], the rule that `initialize` comes first,
//! typed request parsing (a request whose params do not fit the schema gets
//! `-32602`), ids for the requests the adapter sends, and `$/cancel_request`.
//!
//! The connection runs on its own thread under the `futures` executor. Its
//! handlers forward every incoming message to the adapter's synchronous main
//! loop through the sink given to [`Editor::start`], in arrival order.
//!
//! One thing the SDK leaves to the application is answering every request
//! that is still open when the adapter has to stop, including from the
//! shutdown deadline thread while the main loop is stuck: the SDK neither
//! offers that nor answers a `Responder` dropped during shutdown. So each
//! request's `Responder` waits in [`Editor`]'s table, where the main loop
//! answers it by id and [`Editor::fail_unanswered`] can answer the rest.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use agent_client_protocol::{
    self as sdk, Agent, AgentProtocolRouter, Client, ConnectTo, Dispatch, DynConnectTo, Handled,
    Lines, RequestCancellationHandle, Responder, UntypedMessage, v1, v2,
};
use futures::{AsyncBufReadExt, AsyncWriteExt};
use serde_json::Value;

/// A request or notification of ACP v1, parsed into the SDK's schema types.
pub type V1Dispatch = v1::Dispatch<v1::schema::ClientRequest, v1::schema::ClientNotification>;
/// A request or notification of ACP v2, parsed into the SDK's schema types.
pub type V2Dispatch = v2::Dispatch<v2::schema::ClientRequest, v2::schema::ClientNotification>;

/// The SDK's error type, which every response error is built from.
pub use agent_client_protocol::Error;

/// Methods the adapter serves that no ACP schema defines. The SDK's typed
/// request enums answer an unknown method that does not start with `_`
/// with method-not-found, so these are claimed first, as untyped messages.
const NON_STANDARD_METHODS: &[&str] = &[
    // The legacy unstable `SetSessionModelRequest`; see docs/acp-extensions.md.
    "session/set_model",
    // Muse native-subagent controls, passed through to MSP.
    "subagent/sendMessage",
    "subagent/followupTask",
    "subagent/interrupt",
    "subagent/stop",
    "subagent/resume",
    "subagent/reopen",
    "subagent/close",
    "subagent/readResult",
    // Process lifecycle requests some clients send.
    "shutdown",
    "exit",
];

/// Methods one protocol version defines that clients of the other version
/// also call. They are served under both versions, as before the SDK.
const V1_ONLY_METHODS: &[&str] = &["session/load", "session/set_mode", "authenticate", "logout"];
const V2_ONLY_METHODS: &[&str] = &["auth/login", "auth/logout"];

/// Standard methods whose requests carry an extension member at the top
/// level of their params, which the schema type would drop. ACP v2
/// `session/fork` takes JetBrains AIR's `replayFrom`, as `session/resume`
/// does, to replay the fork's history.
const V2_EXTENDED_METHODS: &[&str] = &["session/fork"];

/// What the connection hands the main loop.
pub enum Incoming {
    /// A typed ACP v1 request (its `Responder` waits in the editor's table)
    /// or notification.
    V1(V1Message),
    /// A typed ACP v2 request or notification.
    V2(V2Message),
    /// A method no schema of the negotiated version types. `ver` is the
    /// negotiated version.
    Untyped {
        ver: u8,
        id: Option<RequestId>,
        message: UntypedMessage,
    },
    /// The client asked, with `$/cancel_request`, to cancel one of its
    /// requests. The SDK has already marked that request's cancellation.
    CancelRequest { id: RequestId },
    /// The client answered a request the adapter sent with
    /// [`Editor::request`].
    Answer {
        tag: String,
        id: RequestId,
        result: Result<Value, Error>,
    },
    /// The client closed stdin. Answer what is owed, then call
    /// [`Editor::close`].
    Closed,
    /// The connection is gone: the transport failed, or it ended.
    Ended { reason: String },
}

/// A v1 request with its id, or a v1 notification.
pub enum V1Message {
    Request(RequestId, Box<v1::schema::ClientRequest>),
    Notification(v1::schema::ClientNotification),
}

/// A v2 request with its id, or a v2 notification.
pub enum V2Message {
    Request(RequestId, Box<v2::schema::ClientRequest>),
    Notification(v2::schema::ClientNotification),
}

/// A JSON-RPC request id, as the client sent it (a string or a number), or
/// as the SDK minted it for a request the adapter sent.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    fn of(id: &sdk::RawJsonRpcId) -> RequestId {
        RequestId(serde_json::to_string(id).unwrap_or_else(|_| "null".to_string()))
    }

    pub fn of_value<T: serde::Serialize>(id: &T) -> RequestId {
        RequestId(serde_json::to_string(id).unwrap_or_else(|_| "null".to_string()))
    }

    /// The id as a JSON value.
    pub fn value(&self) -> Value {
        serde_json::from_str(&self.0).unwrap_or(Value::Null)
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The negotiated connection, by protocol version.
#[derive(Clone)]
enum Connection {
    V1(v1::ConnectionTo<Client>),
    V2(v2::ConnectionTo<Client>),
}

type Sink = Arc<dyn Fn(Incoming) + Send + Sync>;

struct Shared {
    connection: Mutex<Option<Connection>>,
    /// Each open request's `Responder`, by id.
    responders: Mutex<HashMap<RequestId, Responder>>,
    /// Requests the adapter sent that the client has not answered.
    outgoing: Mutex<HashMap<RequestId, RequestCancellationHandle>>,
    /// No more writes: the connection failed, or `fail_unanswered` sealed it.
    sealed: AtomicBool,
    stop: Mutex<Option<futures::channel::oneshot::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    sink: Sink,
}

/// The adapter's handle on its client. Clone it freely.
#[derive(Clone)]
pub struct Editor {
    shared: Arc<Shared>,
}

/// A failure to write to the client.
#[derive(Debug)]
pub struct Gone;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serve the connection's version-typed builder until the main loop calls
/// [`Editor::close`]. `connect_with`, unlike `connect_to`, keeps the
/// connection writable after stdin closes, so requests still open then get
/// their answers before the output is drained.
struct ServeUntilClosed<F>(F);

impl<F, Fut> ConnectTo<Client> for ServeUntilClosed<F>
where
    F: FnOnce(DynConnectTo<Agent>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), Error>> + Send,
{
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> Result<(), Error> {
        (self.0)(DynConnectTo::new(client)).await
    }
}

impl Editor {
    /// Start serving ACP on stdin and stdout. `sink` receives every message,
    /// on the connection's thread, in arrival order.
    pub fn start(sink: impl Fn(Incoming) + Send + Sync + 'static) -> std::io::Result<Editor> {
        let (stop_tx, stop_rx) = futures::channel::oneshot::channel::<()>();
        let shared = Arc::new(Shared {
            connection: Mutex::new(None),
            responders: Mutex::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            sealed: AtomicBool::new(false),
            stop: Mutex::new(Some(stop_tx)),
            thread: Mutex::new(None),
            sink: Arc::new(sink),
        });
        let editor = Editor {
            shared: Arc::clone(&shared),
        };
        let serving = editor.clone();
        let thread = std::thread::Builder::new()
            .name("acp-connection".to_string())
            .spawn(move || {
                let reason = match futures::executor::block_on(serving.clone().serve(stop_rx)) {
                    Ok(()) => "the ACP connection ended".to_string(),
                    Err(error) => format!("the ACP connection failed: {error}"),
                };
                serving.shared.sealed.store(true, Ordering::SeqCst);
                (serving.shared.sink)(Incoming::Ended { reason });
            })?;
        *lock(&shared.thread) = Some(thread);
        Ok(editor)
    }

    async fn serve(self, stop: futures::channel::oneshot::Receiver<()>) -> Result<(), Error> {
        // Whichever version the router selects gets the stop signal.
        let stop = Arc::new(Mutex::new(Some(stop)));
        let v1 = Self::v1_builder(&self);
        let v1_stop = Arc::clone(&stop);
        let v1 = ServeUntilClosed(move |client: DynConnectTo<Agent>| {
            v1.connect_with(client, async move |_| {
                wait_for_stop(&v1_stop).await;
                Ok(())
            })
        });
        let v2 = Self::v2_builder(&self);
        let v2_stop = Arc::clone(&stop);
        let v2 = ServeUntilClosed(move |client: DynConnectTo<Agent>| {
            v2.connect_with(client, async move |_| {
                wait_for_stop(&v2_stop).await;
                Ok(())
            })
        });
        let router = AgentProtocolRouter::new().with_v1(v1).with_v2(v2);
        router.connect_to(stdio_lines()).await
    }

    #[allow(clippy::type_complexity)]
    fn v1_builder(
        editor: &Editor,
    ) -> v1::Builder<
        Agent,
        impl sdk::HandleDispatchFrom<Client> + use<>,
        impl sdk::RunWithConnectionTo<Client> + use<>,
        impl sdk::HandleConnectionClose<Client> + use<>,
    > {
        let untyped = editor.clone();
        let cancels = editor.clone();
        let typed = editor.clone();
        let closed = editor.clone();
        Agent
            .v1()
            .name("muse-acp")
            .on_receive_notification(
                async move |cancel: v1::schema::CancelRequestNotification, _cx| {
                    (cancels.shared.sink)(Incoming::CancelRequest {
                        id: RequestId::of_value(&cancel.request_id),
                    });
                    Ok(())
                },
                sdk::on_receive_notification!(),
            )
            .on_receive_dispatch(
                async move |dispatch: v1::Dispatch, cx: v1::ConnectionTo<Client>| {
                    untyped.connected(Connection::V1(cx));
                    Ok(untyped
                        .claim_untyped(1, dispatch.into_raw(), V2_ONLY_METHODS)
                        .map(|dispatch| dispatch.into_protocol())
                        .transpose()?
                        .map_or(Handled::Yes, |message| Handled::No {
                            message,
                            retry: false,
                        }))
                },
                sdk::on_receive_dispatch!(),
            )
            .on_receive_dispatch(
                async move |dispatch: V1Dispatch, cx: v1::ConnectionTo<Client>| {
                    typed.connected(Connection::V1(cx));
                    match dispatch {
                        Dispatch::Request(request, responder) => {
                            let id = typed.hold(responder.into_raw());
                            (typed.shared.sink)(Incoming::V1(V1Message::Request(
                                id,
                                Box::new(request),
                            )));
                        }
                        Dispatch::Notification(notification) => {
                            (typed.shared.sink)(Incoming::V1(V1Message::Notification(
                                notification,
                            )));
                        }
                        // Answers to the adapter's own requests reach their
                        // callbacks through the SDK's default routing.
                        response @ Dispatch::Response(..) => {
                            return Ok(Handled::No {
                                message: response,
                                retry: false,
                            });
                        }
                    }
                    Ok(Handled::Yes)
                },
                sdk::on_receive_dispatch!(),
            )
            .on_close(async move |_| {
                (closed.shared.sink)(Incoming::Closed);
                Ok(())
            })
    }

    #[allow(clippy::type_complexity)]
    fn v2_builder(
        editor: &Editor,
    ) -> v2::Builder<
        Agent,
        impl sdk::HandleDispatchFrom<Client> + use<>,
        impl sdk::RunWithConnectionTo<Client> + use<>,
        impl sdk::HandleConnectionClose<Client> + use<>,
    > {
        let untyped = editor.clone();
        let cancels = editor.clone();
        let typed = editor.clone();
        let closed = editor.clone();
        Agent
            .v2()
            .name("muse-acp")
            .on_receive_notification(
                async move |cancel: v2::schema::CancelRequestNotification, _cx| {
                    (cancels.shared.sink)(Incoming::CancelRequest {
                        id: RequestId::of_value(&cancel.request_id),
                    });
                    Ok(())
                },
                sdk::on_receive_notification!(),
            )
            .on_receive_dispatch(
                async move |dispatch: v2::Dispatch, cx: v2::ConnectionTo<Client>| {
                    untyped.connected(Connection::V2(cx));
                    Ok(untyped
                        .claim_untyped(
                            2,
                            dispatch.into_raw(),
                            &[V1_ONLY_METHODS, V2_EXTENDED_METHODS].concat(),
                        )
                        .map(|dispatch| dispatch.into_protocol())
                        .transpose()?
                        .map_or(Handled::Yes, |message| Handled::No {
                            message,
                            retry: false,
                        }))
                },
                sdk::on_receive_dispatch!(),
            )
            .on_receive_dispatch(
                async move |dispatch: V2Dispatch, cx: v2::ConnectionTo<Client>| {
                    typed.connected(Connection::V2(cx));
                    match dispatch {
                        Dispatch::Request(request, responder) => {
                            let id = typed.hold(responder.into_raw());
                            (typed.shared.sink)(Incoming::V2(V2Message::Request(
                                id,
                                Box::new(request),
                            )));
                        }
                        Dispatch::Notification(notification) => {
                            (typed.shared.sink)(Incoming::V2(V2Message::Notification(
                                notification,
                            )));
                        }
                        // Answers to the adapter's own requests reach their
                        // callbacks through the SDK's default routing.
                        response @ Dispatch::Response(..) => {
                            return Ok(Handled::No {
                                message: response,
                                retry: false,
                            });
                        }
                    }
                    Ok(Handled::Yes)
                },
                sdk::on_receive_dispatch!(),
            )
            .on_close(async move |_| {
                (closed.shared.sink)(Incoming::Closed);
                Ok(())
            })
    }

    fn connected(&self, connection: Connection) {
        let mut current = lock(&self.shared.connection);
        if current.is_none() {
            *current = Some(connection);
        }
    }

    /// Forward a message whose method the adapter serves without a schema
    /// type for this version, and give back every other message.
    fn claim_untyped(
        &self,
        ver: u8,
        dispatch: Dispatch,
        other_version: &[&str],
    ) -> Option<Dispatch> {
        let method = dispatch.method().to_string();
        let claimed = NON_STANDARD_METHODS.contains(&method.as_str())
            || other_version.contains(&method.as_str());
        if !claimed {
            return Some(dispatch);
        }
        match dispatch {
            Dispatch::Request(message, responder) => {
                let id = self.hold(responder);
                (self.shared.sink)(Incoming::Untyped {
                    ver,
                    id: Some(id),
                    message,
                });
                None
            }
            Dispatch::Notification(message) => {
                (self.shared.sink)(Incoming::Untyped {
                    ver,
                    id: None,
                    message,
                });
                None
            }
            response @ Dispatch::Response(..) => Some(response),
        }
    }

    /// Keep a request's `Responder` until the adapter answers it.
    fn hold(&self, responder: Responder) -> RequestId {
        let id = RequestId::of(responder.raw_id());
        lock(&self.shared.responders).insert(id.clone(), responder);
        id
    }

    fn connection(&self) -> Result<Connection, Gone> {
        if self.shared.sealed.load(Ordering::SeqCst) {
            return Err(Gone);
        }
        lock(&self.shared.connection).clone().ok_or(Gone)
    }

    fn take_responder(&self, id: &RequestId) -> Option<Responder> {
        if self.shared.sealed.load(Ordering::SeqCst) {
            return None;
        }
        lock(&self.shared.responders).remove(id)
    }

    /// Answer request `id` with `result`. An id that is not open (already
    /// answered, or never received) is ignored.
    pub fn respond(&self, id: &RequestId, result: Value) -> Result<(), Gone> {
        match self.take_responder(id) {
            Some(responder) => responder.respond(result).map_err(|_| Gone),
            None => Ok(()),
        }
    }

    /// Answer request `id` with `error`.
    pub fn respond_error(&self, id: &RequestId, error: Error) -> Result<(), Gone> {
        match self.take_responder(id) {
            Some(responder) => responder.respond_with_error(error).map_err(|_| Gone),
            None => Ok(()),
        }
    }

    /// Send a notification.
    pub fn notify(&self, message: UntypedMessage) -> Result<(), Gone> {
        match self.connection()? {
            Connection::V1(cx) => cx.send_notification(message),
            Connection::V2(cx) => cx.send_notification(message),
        }
        .map_err(|_| Gone)
    }

    /// Send a request; the client's answer reaches the sink as
    /// [`Incoming::Answer`] carrying `tag`. Returns the request's id.
    pub fn request(&self, message: UntypedMessage, tag: &str) -> Result<RequestId, Gone> {
        let tag = tag.to_string();
        let shared = Arc::clone(&self.shared);
        self.send_request(message, move |id, result| {
            (shared.sink)(Incoming::Answer { tag, id, result });
        })
    }

    /// Send a request and block until the client answers it, the request is
    /// withdrawn, or `abort` says to stop waiting. A request that is not
    /// answered in `timeout` is withdrawn with `$/cancel_request`.
    pub fn request_blocking(
        &self,
        message: UntypedMessage,
        timeout: Duration,
        abort: &dyn Fn() -> bool,
    ) -> Result<Result<Value, Error>, Blocked> {
        let (answer_tx, answer_rx) = std::sync::mpsc::channel();
        let id = self
            .send_request(message, move |_, result| {
                let _ = answer_tx.send(result);
            })
            .map_err(|_| Blocked::Gone)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match answer_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(result) => return Ok(result),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(Blocked::Gone),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            let stop = if abort() {
                Blocked::Aborted
            } else if std::time::Instant::now() >= deadline {
                Blocked::TimedOut
            } else {
                continue;
            };
            self.cancel(&id);
            return Err(stop);
        }
    }

    fn send_request(
        &self,
        message: UntypedMessage,
        answered: impl FnOnce(RequestId, Result<Value, Error>) + Send + 'static,
    ) -> Result<RequestId, Gone> {
        let shared = Arc::clone(&self.shared);
        let register = move |id: RequestId, handle: RequestCancellationHandle| {
            lock(&shared.outgoing).insert(id.clone(), handle);
            let shared = Arc::clone(&shared);
            move |result: Result<Value, Error>| {
                lock(&shared.outgoing).remove(&id);
                answered(id, result);
            }
        };
        match self.connection()? {
            Connection::V1(cx) => {
                let request = cx.prepare_request(message);
                let id = RequestId::of_value(request.id());
                let done = register(id.clone(), request.cancellation_handle());
                request
                    .on_receiving_result(move |result| async move {
                        done(result);
                        Ok(())
                    })
                    .map_err(|_| Gone)?;
                Ok(id)
            }
            Connection::V2(cx) => {
                let request = cx.prepare_request(message);
                let id = RequestId::of_value(request.id());
                let done = register(id.clone(), request.cancellation_handle());
                request
                    .on_receiving_result(move |result| async move {
                        done(result);
                        Ok(())
                    })
                    .map_err(|_| Gone)?;
                Ok(id)
            }
        }
    }

    /// Withdraw a request the adapter sent, with `$/cancel_request`. The
    /// client may still answer it; that answer still arrives.
    pub fn cancel(&self, id: &RequestId) {
        let handle = lock(&self.shared.outgoing).get(id).cloned();
        if let Some(handle) = handle {
            let _ = handle.cancel();
        }
    }

    /// Answer every request that is still open with `message`, then take no
    /// more writes. For shutdown: the client gets an answer for every request,
    /// even ones the main loop never reached.
    pub fn fail_unanswered(&self, code: i32, message: &str) {
        if self.shared.sealed.swap(true, Ordering::SeqCst) {
            return;
        }
        let open: Vec<Responder> = lock(&self.shared.responders)
            .drain()
            .map(|(_, responder)| responder)
            .collect();
        for responder in open {
            let _ = responder.respond_with_error(Error::new(code, message));
        }
    }

    /// End the connection gracefully: everything already sent is written,
    /// then the connection thread ends. Waits at most `timeout`.
    pub fn close(&self, timeout: Duration) {
        if let Some(stop) = lock(&self.shared.stop).take() {
            let _ = stop.send(());
        }
        let Some(thread) = lock(&self.shared.thread).take() else {
            return;
        };
        let deadline = std::time::Instant::now() + timeout;
        while !thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Why [`Editor::request_blocking`] returned without an answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The connection is gone.
    Gone,
    /// The request was not answered in time; it was withdrawn.
    TimedOut,
    /// `abort` asked to stop waiting; the request was withdrawn.
    Aborted,
}

async fn wait_for_stop(stop: &Mutex<Option<futures::channel::oneshot::Receiver<()>>>) {
    let receiver = lock(stop).take();
    match receiver {
        // A dropped sender also stops the connection.
        Some(receiver) => {
            let _ = receiver.await;
        }
        None => futures::future::pending::<()>().await,
    }
}

/// Stdin and stdout as the SDK's `Lines` transport. JSON text must be
/// UTF-8, so a line that is not reaches the SDK's parser marked as such and
/// never as JSON: the SDK answers it with `-32700` and keeps reading. The
/// SDK's `Stdio` transport would end the connection instead.
fn stdio_lines() -> impl ConnectTo<Agent> {
    let stdin = futures::io::BufReader::new(blocking::Unblock::new(std::io::stdin()));
    let incoming = futures::stream::unfold(stdin, async |mut reader| {
        let mut bytes = Vec::new();
        match reader.read_until(b'\n', &mut bytes).await {
            Ok(0) => None,
            Ok(_) => {
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                    if bytes.last() == Some(&b'\r') {
                        bytes.pop();
                    }
                }
                let line = match String::from_utf8(bytes) {
                    Ok(line) => line,
                    Err(error) => format!(
                        "invalid UTF-8: {}",
                        String::from_utf8_lossy(error.as_bytes())
                    ),
                };
                Some((Ok(line), reader))
            }
            Err(error) => Some((Err(error), reader)),
        }
    });
    let stdout = blocking::Unblock::new(std::io::stdout());
    let outgoing = futures::sink::unfold(stdout, async |mut stdout, line: String| {
        stdout.write_all(line.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
        Ok::<_, std::io::Error>(stdout)
    });
    Lines::new(Box::pin(outgoing), Box::pin(incoming))
}
