# acp-lite

`acp-lite` is a synchronous Rust library for both sides of the
[Agent Client Protocol](https://agentclientprotocol.com/) (ACP).

- **Client side** (`Client`): launch or connect to an ACP stdio agent, perform
  the v1 or v2 handshake, correlate JSON-RPC requests, stream session updates,
  and route the agent's requests back to a handler.
- **Agent side** (`AgentConnection`, or `serve` for simple agents): read a
  client's requests, answer each exactly once, answer JSON-RPC batches
  together, stream notifications, ask the client for permissions, forms,
  files, or terminals, and fail whatever is unanswered at shutdown. See
  [Agent side](#agent-side).

The crate is independent of the `muse-acp` binary and of
`agent-client-protocol`. It uses background threads instead of an async
runtime, which makes it suitable for small tools, tests, editor integrations,
and applications that already own a synchronous event loop.

## Quick start

This checkout has not published the crate to crates.io. Use the workspace path
from another package in this repository:

```toml
[dependencies]
acp-lite = { path = "crates/acp-lite" }
```

```rust
use acp_lite::{
    AgentProcess, Client, ClientOptions, ContentBlock, DenyAllHandler,
    InitializeRequest, NewSessionRequest, PromptRequest,
};
use std::sync::Arc;

let mut process = AgentProcess::new("muse-acp");
let client = Client::spawn(
    process,
    Arc::new(DenyAllHandler),
    ClientOptions::default(),
)?;

let initialized = client.initialize(InitializeRequest::v2())?;
let session = client.new_session(NewSessionRequest::new(std::env::current_dir()?))?;
let prompt = PromptRequest::new(
    session.session_id,
    vec![ContentBlock::text("Explain the test failure")],
);
let _accepted = client.prompt(prompt)?;

while let Ok(event) = client.next_event() {
    println!("{event:?}");
}
```

The example above never exits the event loop. A real client stops after the
ACP v2 `state_update` reaches `idle`, after the v1 prompt response returns, or
when it calls `Client::close`.

## Protocol versions

`InitializeRequest::v1()` sends `clientInfo` and `clientCapabilities`;
`InitializeRequest::v2()` sends `info` and `capabilities`. The client sends the
standard `initialized` notification after the agent returns a valid
`protocolVersion`, including when an agent negotiates back from a requested v2
to v1.

ACP v1 generally keeps `session/prompt` open until the turn finishes. ACP v2
acknowledges prompt submission and reports progress and completion through
`session/update` notifications. `ClientEvent::SessionUpdate` exposes either
shape without hiding unknown fields.

## Agent-initiated requests

Agents can send requests to the client for permissions, elicitation,
filesystem access, and terminals. Implement `ClientHandler` and return a
`RequestOutcome`:

```rust
use acp_lite::{
    ClientHandler, IncomingRequest, RequestContext, RequestOutcome,
};
use serde_json::json;

struct MyHandler;

impl ClientHandler for MyHandler {
    fn handle_request(
        &self,
        _context: &RequestContext,
        request: IncomingRequest,
    ) -> RequestOutcome {
        match request.method.as_str() {
            "session/request_permission" => {
                RequestOutcome::result(json!({
                    "outcome": {"outcome": "cancelled"}
                }))
            }
            _ => RequestOutcome::error(-32601, "unsupported client method"),
        }
    }
}
```

Handlers run on dedicated threads, so they may issue another `request` through
`context.client()`. Return only capabilities that the handler actually
implements in `InitializeRequest::capabilities`. `DenyAllHandler` is the safe
default: it selects an offered reject permission option, cancels permissions
that offer no reject option, declines elicitation, and returns method-not-found
for filesystem and terminal requests.

## Transport and lifecycle

`Client::spawn` owns the child process, drains stderr into a bounded tail, and
sends EOF to stdin on close. It waits for the configured shutdown timeout and
then kills a child that has not exited. `Client::connect` accepts an arbitrary
`BufRead` and `Write`, which is useful for in-process agents and tests.

Requests use monotonic numeric IDs and a per-request deadline. A late response
after a timeout is ignored. Agent cancellation through `$/cancel_request`
settles the matching local request as `RequestCancelled`. EOF fails every
pending request and publishes `ClientEvent::Closed`.

## Agent side

An agent reads the client's messages on one reader thread and writes from any
thread. The smallest agent implements `AgentHandler` and calls `serve_stdio`;
[`examples/echo_agent.rs`](examples/echo_agent.rs) is a complete one that
streams each prompt back:

```rust
use acp_lite::{AgentConnection, AgentHandler, AgentOptions, ClientRequest, RpcError, serve_stdio};
use serde_json::{Value, json};

struct Echo;

impl AgentHandler for Echo {
    fn handle_request(&mut self, connection: &AgentConnection, request: ClientRequest)
        -> Result<Value, RpcError> {
        match request.method.as_str() {
            "initialize" => Ok(json!({"protocolVersion": 1, "agentCapabilities": {}, "authMethods": []})),
            "session/new" => Ok(json!({"sessionId": "s1"})),
            "session/prompt" => {
                connection.notify("session/update", json!({"sessionId": "s1", "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "hello"},
                }})).map_err(|e| RpcError::new(-32603, e.to_string()))?;
                Ok(json!({"stopReason": "end_turn"}))
            }
            other => Err(RpcError::new(-32601, format!("method not found: {other}"))),
        }
    }
}

fn main() -> std::io::Result<()> {
    serve_stdio(AgentOptions::default(), Echo)
}
```

`serve` handles one message at a time. An agent that must stay responsive
during a long prompt, for example to handle `session/cancel`, drives the
connection itself:

```rust
use acp_lite::{AgentConnection, AgentEvent, AgentOptions};

let (connection, _reader) = AgentConnection::stdio(AgentOptions::default(), move |event| {
    // Runs on the reader thread, in arrival order. Forward the event to your
    // own loop, for example through a channel.
    let _ = event;
})?;
```

What the connection does for you:

- **Requests are answered once.** Each `AgentEvent::Request` stays unanswered
  until `respond` or `respond_error` writes its id. `unanswered()` lists the
  rest, and `fail_unanswered(code, message)` answers them all and then refuses
  further writes. Use it at shutdown, so the client never waits forever.
- **Batches are answered together.** A JSON-RPC array (used by ACP v2)
  arrives as one `AgentEvent::Batch`. `hold(&batch)` buffers the responses to
  its requests, plus a `-32600` for each invalid entry, and sends them as one
  array when the guard is finished or dropped. Whether batches are allowed on
  the negotiated version is your choice.
- **Bad frames don't stop the agent.** Invalid JSON and invalid UTF-8 get a
  `-32700` with `id: null`, an empty array gets `-32600`, and reading goes on.
  Other non-messages arrive as `AgentEvent::Invalid`.
- **Requests to the client are matched with their answers.**
  `send_request(method, params)` returns a `PendingRequest` to `wait`,
  `wait_timeout` (in a loop that also checks for cancellation), or `cancel`.
  `request` waits up to `AgentOptions::request_timeout` and withdraws the
  request if it runs out. `request_routed(method, params, tag)` delivers the
  answer later as `AgentEvent::Response` carrying your tag, which suits an
  agent whose own event loop handles the answer. `cancel_request(id)` withdraws
  either kind with ACP's `$/cancel_request`, and a late answer arrives as an
  untagged `AgentEvent::Response`.
- **Text in, text out.** `respond_raw`, `notify_raw`, `send_request_raw`, and
  `send_raw_frame` take `serde_json::value::RawValue` for agents that already
  build JSON as text. It is checked once, and never re-parsed into a tree.

Writes are synchronous under one lock, so a client that stops reading blocks
the writer, as a pipe would. A failed write refuses further writes with
`AgentError::Closed`.

## Current scope

The crate intentionally exposes stable common operations directly and leaves
new or agent-specific methods available through `request` and `notify`. Muse's
`_meta.muse` fields and `_session/*` methods are preserved as raw JSON so the
client does not freeze experimental extension shapes. Rich typed mirrors for
every Muse extension, request-handle cancellation, and an async adapter can be
added without changing the wire model.
