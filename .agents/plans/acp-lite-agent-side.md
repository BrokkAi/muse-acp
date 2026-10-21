# Give acp-lite an agent side and move muse-acp onto it

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.

It builds on `.agents/plans/acp-client-sdk.md`, which created the synchronous ACP client crate now named `acp-lite` (`crates/acp-lite`).


## Purpose / Big Picture


ACP (Agent Client Protocol) has two sides. An editor is the *client*: it starts an agent process and sends it requests. The agent answers, streams `session/update` notifications back, and sometimes asks the client for things, such as permission, a form, or a file. `acp-lite` today has only the client side. It is a small synchronous library: background threads instead of an async runtime, and `serde_json` values for anything without a typed struct.

`muse-acp` is an ACP agent, and it hand-rolls its whole agent side inside `src/main.rs`, `src/acp.rs`, `src/shutdown.rs`, and `src/bridge.rs`. That covers:

- line framing of stdin;
- JSON-RPC batches;
- answering each request exactly once;
- failing still-unanswered requests at shutdown;
- matching the client's answers to the requests the agent sent.

That last part is done four separate times with four separate id maps: permissions, permission feedback, forms, and the editor-tool bridge.

After this work, `acp-lite` has an agent side (`acp_lite::agent`) that any Rust program can use to be an ACP agent, and `muse-acp` uses it for all of the above. A user can write a small ACP agent with `acp-lite` alone. `muse-acp` loses its duplicated plumbing and keeps only Muse logic.

The work lands in three pull requests, one per milestone. The behavior of `muse-acp` must not change: the `tests/acp_serve.rs` suite (more than 430 black-box tests), the live loopback suite, and the ACP TCK v1/v2 runs are the guard.


## Progress


- [x] (2026-10-21 12:25Z) Read the current transport (`main` stdin pump and loop, `dispatch_batch`, `acp::send_*`, the batch capture, `shutdown::register_request`/`completed`/`settle`) and the `acp-lite` client core (`client.rs` reader and writer threads, the pending map, `read_frame`).
- [x] (2026-10-21 12:35Z) Wrote this plan.
- [x] (2026-10-21 13:05Z) Milestone 1: `acp_lite::agent` (`crates/acp-lite/src/agent.rs`, with `read_frame` moved to `src/frame.rs`), 10 unit tests, the round-trip test `crates/acp-lite/tests/agent_round_trip.rs`, `examples/echo_agent.rs`, and README docs. PR 1.
- [ ] Milestone 2: `muse-acp` reads and writes ACP through `acp_lite::agent`. That replaces the stdin pump, framing, batches, the `acp::send_*` write path, and the shutdown request registry. `serde`/`serde_json` become dependencies of the binary. PR 2.
- [ ] Milestone 3: `muse-acp`'s requests to the client (permission, permission feedback, feedback form, elicitation, device-login form, editor bridge) use the agent connection's correlation, and the chain of `complete_*` id matchers is removed. PR 3.


## Surprises & Discoveries


- Observation: `acp-lite` already exported `IncomingRequest` for requests an agent sends to a client, so the agent side names its types from the agent's point of view: `ClientRequest`, `ClientNotification`, `ClientResponse`.
  Evidence: `crates/acp-lite/src/handler.rs` `pub struct IncomingRequest`.

- Observation: `muse-acp` will still write some requests to the client as raw text until milestone 3, so answers the connection did not send must reach the application. They arrive as `AgentEvent::Response` with `tag: None`, and so do late answers to withdrawn requests, instead of being dropped.


## Decision Log


- Decision: `muse-acp` adopts the agent side, in stages, rather than `acp-lite` gaining it standalone.
  Rationale: the user chose this. A real agent proves the API, and `muse-acp` loses duplicated plumbing.
  Date/Author: 2026-10-21 / user, recorded by Claude.

- Decision: use `serde_json` throughout, so the `muse-acp` binary gains `serde` and `serde_json` as dependencies. `muse-msp-client` keeps its own JSON parser, and `muse-acp` keeps its `J` value type internally, converting at the ACP boundary.
  Rationale: the user chose serde. SDK users expect serde types. The "no third-party code in the binary" idea was never a project rule. Converting `J` to and from `Value` at one boundary keeps milestone 2 mechanical: `handle_acp` and everything below it stay on `J`.
  Date/Author: 2026-10-21 / user, recorded by Claude.

- Decision: the agent side is event driven. A reader thread parses each frame and calls an application-supplied sink (`FnMut(AgentEvent) + Send`) on that thread. A cloneable `AgentConnection` writes responses, notifications, and requests. A `channel` convenience returns a `Receiver<AgentEvent>`, and a small `serve` helper runs a handler trait for simple agents.
  Rationale: `muse-acp` already owns a single-threaded loop that merges ACP input with Muse host events (`LoopMsg`). A sink lets it forward `AgentEvent`s into that loop and do its reader-thread work (noticing `shutdown`/`exit` at once) without the library knowing about it.
  Date/Author: 2026-10-21 / Claude.

- Decision: writes are synchronous under one mutex, not queued to a writer thread.
  Rationale: this keeps the backpressure and failure behavior `muse-acp` has today. A slow editor pipe blocks the writer, and the shutdown deadline (in `src/shutdown.rs`) bounds that. An unbounded queue would grow memory instead.
  Date/Author: 2026-10-21 / Claude.

- Decision: the connection accepts results and params either as `serde_json::Value` or as pre-serialized `&RawValue` (the `raw_value` feature of `serde_json`).
  Rationale: `muse-acp` builds much of its JSON as text, including high-volume session updates. `RawValue::from_string` checks that the text is valid JSON without building a tree, so the library never writes invalid JSON and `muse-acp` avoids a parse-and-serialize round trip.
  Date/Author: 2026-10-21 / Claude.

- Decision: requests to the client come in two forms:
  - **Blocking**: `send_request` returns a `PendingRequest` with `wait_timeout` and `cancel`. Used by the editor-tool bridge threads, which poll for cancellation.
  - **Routed**: `request_routed(method, params, tag)` returns the request id, and the client's answer arrives as `AgentEvent::Response { tag, .. }` on the event sink.

  `cancel_request(id)` withdraws either kind with `$/cancel_request`, and late answers to withdrawn requests are dropped.
  Rationale: `muse-acp` uses both patterns today. Bridge threads block; permission and form flows are answered in the main loop.
  Date/Author: 2026-10-21 / Claude.

- Decision: the library tracks every incoming request id from the moment it is read until a response for it is written. `fail_unanswered(code, message)` answers the rest.
  Rationale: this is `shutdown::register_request`/`completed`/`settle` in `muse-acp`, which must answer every request even when the main loop is stuck. Registering on the reader thread, before the sink runs, preserves that.
  Date/Author: 2026-10-21 / Claude.

- Decision: batches (JSON-RPC arrays, used by ACP v2) arrive as one `AgentEvent::Batch` holding the valid entries in order and an error for each invalid entry. `AgentConnection::hold(&batch)` returns a guard that buffers responses to the batch's request ids and writes them, with the invalid-entry errors, as one array when finished or dropped. Whether batches are allowed (`muse-acp` refuses them on ACP v1) is left to the application.
  Rationale: this matches `muse-acp`'s `dispatch_batch` and `acp::begin_batch`/`finish_batch` exactly, and keeps protocol-version policy out of the transport.
  Date/Author: 2026-10-21 / Claude.


## Outcomes & Retrospective


(To be written at completion.)


## Context and Orientation


`crates/acp-lite` is the workspace's synchronous ACP client. Its files:

- `src/client.rs`: `Client`, plus a private `Core` with a reader thread, a writer thread, and a pending-reply map. It also has the `read_frame` function, which reads one newline-terminated frame with a byte limit.
- `src/handler.rs`: `ClientHandler`, for requests from the agent.
- `src/protocol.rs`: `ProtocolVersion`, `RpcError` (`{code, message, data}`), `ContentBlock`, and the client's request structs.
- `src/error.rs`: `ClientError`.
- `src/process.rs`: spawns an agent.

It depends on `serde` and `serde_json`.

`muse-acp`'s agent side today:

- **Input.** `fn main` in `src/main.rs` spawns a stdin thread that:
  - reads lines, sending `LoopMsg::AcpInvalidUtf8` for non-UTF-8 and `LoopMsg::AcpLine` for the rest;
  - registers every request id with `shutdown::register_request`;
  - calls `shutdown::disconnect` on `shutdown`/`exit` methods and at EOF.
- **Main loop.**
  - It parses each line with the `J` parser and answers a parse failure with `-32700`.
  - A JSON array goes to `dispatch_batch`, which refuses batches unless ACP v2 is negotiated and validates entries with `valid_batch_entry`. It calls `acp::begin_batch`, dispatches each entry to `handle_acp`, and calls `acp::finish_batch`.
  - A single object goes to `handle_acp`, after the first-request-must-be-`initialize` check and host launch.
- **Output.** In `src/acp.rs`:
  - `send_raw` writes a line under the `StdoutShared` mutex and calls `shutdown::disconnect` on a write error.
  - `send_result`/`send_error`/`send_error_with_data` go through `send_response`. That holds the line if it answers a request of the active batch, skips writing while `shutdown::expiring()`, and calls `shutdown::completed(id)` after a successful write.
  - `send_cancel_request` writes `$/cancel_request`.
- **Answers to the agent's own requests.** In `handle_acp`, a message without `method` is offered in turn to:
  - `bridge::complete` (`src/bridge.rs`);
  - `complete_permission`;
  - `complete_permission_feedback`;
  - `complete_feedback`;
  - `complete_elicitation`;
  - `complete_device_login_elicitation`.

  Each compares the id with ids it stored itself.


## Plan of Work


Milestone 1 (PR 1): the agent side in `acp-lite`.

1. Move `read_frame` from `client.rs` into a new `src/frame.rs`, used by both sides.
2. Add `src/agent.rs`, re-exported from `lib.rs`, with these types:
   - `AgentOptions`: maximum frame bytes (default 16 MiB, as for the client) and the default timeout for blocking requests (120 s).
   - `RequestId`: wraps the id's `Value`, compared and hashed by its JSON text.
   - `ClientRequest { id, method, params }`, with a `params_as::<T>()` helper.
   - `ClientNotification { method, params }`.
   - `ClientResponse { id, tag: Option<String>, method: Option<String>, outcome: Result<Value, RpcError> }`. The tag and method are `None` for an answer the connection did not send.
   - `Batch { entries: Vec<AgentEvent>, invalid: Vec<RpcError> }`.
   - `AgentEvent::{Request, Notification, Response, Batch, Invalid(Value), Closed { reason }}`.
   - `AgentConnection` (`Clone`), with `start(reader, writer, options, sink)`, `stdio(options, sink)`, and `channel(reader, writer, options)`.
   - `AgentError` for write failures and a closed connection.
   - `PendingRequest`.
   - `BatchGuard`.
   - `AgentHandler` and `serve`, for simple agents.
3. The reader thread classifies each frame:
   - invalid UTF-8 or invalid JSON → write `-32700` with `id: null`, and continue;
   - neither an object nor an array → `-32600`;
   - an empty array → `-32600`;
   - an array → `Batch`;
   - an object with a string `method` and an `id` → `Request`, registered as unanswered before the sink sees it;
   - `method` without `id` → `Notification`;
   - no `method`, but an `id` and a `result` or `error` → a response, which goes to a blocking waiter, or becomes `AgentEvent::Response` for a routed request, or is dropped when unknown;
   - anything else → `Invalid`.

   EOF or a read error → `Closed`, then the thread ends.
4. `AgentConnection` methods:
   - `respond` / `respond_raw` / `respond_error(Option<&RequestId>, RpcError)`;
   - `notify` / `notify_raw`;
   - `send_request` / `send_request_raw`, which return a `PendingRequest` with `id`, `wait`, `wait_timeout`, and `cancel`;
   - `request` (blocking with the default timeout);
   - `request_routed(method, params, tag)` and `request_routed_raw`;
   - `cancel_request`;
   - `hold(&Batch) -> BatchGuard`;
   - `unanswered()` and `fail_unanswered(code, message)`;
   - `is_closed()`.

   Outgoing request ids are strings `"acp-lite-<n>"`, so they never collide with an application's own ids. Every write goes through one mutex. A failed write marks the connection broken and returns `AgentError::Write`, and the application decides what to do.
5. Tests:
   - unit tests in `agent.rs` for each classification, batch holding with invalid entries, unanswered tracking, routed and blocking requests, cancel and late answers, and write failure;
   - `crates/acp-lite/tests/agent_round_trip.rs`, where the crate's own `Client` talks to an `AgentConnection` over two `std::io::pipe()` pairs (stable since Rust 1.87; MSRV is 1.88): initialize, new session, prompt with a streamed `session/update`, a permission request answered by the client's handler, and cancel;
   - `crates/acp-lite/examples/echo_agent.rs`.
6. Docs: README "Agent side" section; the crate description and the root README mention both sides.

Milestone 2 (PR 2): `muse-acp` transport.

1. Add `serde_json` (with `raw_value`) and `acp-lite` to `[dependencies]`.
2. Add `json::to_value(&J) -> Value` and `json::from_value(&Value) -> J` in `src/json.rs`, preserving number text with the `arbitrary_precision` feature, which the tests already enable.
3. Replace the stdin pump with `AgentConnection::stdio`. Its sink:
   - calls `shutdown::disconnect` on `shutdown`/`exit` and on `Closed`;
   - forwards events as `LoopMsg::Acp(AgentEvent)`.
4. In the loop, convert a `Request` or `Notification` to `J` and call `handle_acp` exactly as today. A `Batch` keeps the v2 check and uses `hold`.
5. `acp::send_*` write through the connection: raw text becomes `RawValue`, and the `expiring()` skip is kept. `shutdown::settle` uses `fail_unanswered`, and `register_request`/`completed` go away.
6. Remove `LoopMsg::AcpLine` and `AcpInvalidUtf8`.
7. Acceptance: every existing test passes unchanged, and the TCK v1/v2 results match master.

Milestone 3 (PR 3): requests to the client.

1. Permission, permission-feedback, feedback-form, elicitation, and device-login requests use `request_routed` with tags naming the flow and session. The loop dispatches `AgentEvent::Response` by tag to the existing completion logic, keyed by the request id it already stores.
2. `bridge.rs` uses `send_request` and `PendingRequest::wait_timeout`/`cancel` in place of its own `PENDING` map.
3. `$/cancel_request` withdrawals use `cancel_request`.
4. The `complete_*` id-matching chain and the per-flow id maps that only did correlation are deleted.
5. Acceptance: the same suites pass unchanged.


## Concrete Steps


From the repository root:

    cargo test --locked -p acp-lite
    cargo run --locked -p acp-lite --example echo_agent   # with a client attached; see README
    cargo test --locked --workspace
    cargo clippy --locked --workspace --all-targets -- -D warnings
    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback      # milestones 2 and 3
    /tmp/tck/run.sh target/debug/muse-acp 1 report-v1.json            # TCK, milestones 2 and 3


## Validation and Acceptance


Milestone 1: `cargo test -p acp-lite` passes. In the new round-trip test, an `acp-lite` `Client` initializes against an `acp-lite` agent, opens a session, sends a prompt, receives a streamed `session/update`, answers the agent's `session/request_permission` through its handler, and gets the prompt's `stopReason`. Milestones 2 and 3: the workspace tests, the live loopback suite, and the TCK all give the same results as before each change, and the shipped binary's behavior is unchanged.


## Idempotence and Recovery


Each milestone is one PR that leaves everything working. If milestone 2 or 3 regresses, revert that PR alone; milestone 1 stands on its own as a library feature.


## Artifacts and Notes


(To be filled in as milestones land.)


## Interfaces and Dependencies


`crates/acp-lite` gains `serde_json`'s `raw_value` feature. Public items, all in `acp_lite` (re-exported from `agent`):

    pub struct AgentOptions { pub max_frame_bytes: usize, pub request_timeout: Duration }
    pub struct RequestId(/* Value */);
    pub enum AgentEvent { Request(ClientRequest), Notification(ClientNotification),
                          Response(ClientResponse), Batch(Batch), Invalid(Value), Closed { reason: String } }
    pub struct AgentConnection;  // Clone + Send + Sync
    impl AgentConnection {
        pub fn start<R, W, S>(reader: R, writer: W, options: AgentOptions, sink: S) -> io::Result<(AgentConnection, JoinHandle<()>)>
            where R: BufRead + Send + 'static, W: Write + Send + 'static, S: FnMut(AgentEvent) + Send + 'static;
        pub fn respond(&self, id: &RequestId, result: Value) -> Result<(), AgentError>;
        pub fn respond_raw(&self, id: &RequestId, result: &RawValue) -> Result<(), AgentError>;
        pub fn respond_error(&self, id: Option<&RequestId>, error: RpcError) -> Result<(), AgentError>;
        pub fn notify(&self, method: &str, params: Value) -> Result<(), AgentError>;
        pub fn send_request(&self, method: &str, params: Value) -> Result<PendingRequest, AgentError>;
        pub fn request_routed(&self, method: &str, params: Value, tag: impl Into<String>) -> Result<RequestId, AgentError>;
        pub fn cancel_request(&self, id: &RequestId) -> Result<(), AgentError>;
        pub fn hold(&self, batch: &Batch) -> BatchGuard;
        pub fn fail_unanswered(&self, code: i64, message: &str);
    }

Revision note (2026-10-21): milestone 1 is done. The agent-side types are named `ClientRequest`/`ClientNotification`/`ClientResponse` to avoid the client side's `IncomingRequest`, and answers the connection did not send surface with `tag: None` for milestone 2's sake.
