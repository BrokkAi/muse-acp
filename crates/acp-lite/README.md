# acp-lite

`acp-lite` is a synchronous Rust client for the
[Agent Client Protocol](https://agentclientprotocol.com/) (ACP). It launches
or connects to an ACP stdio agent, performs the v1 or v2 handshake, correlates
JSON-RPC requests, streams session updates, and routes requests from the agent
back to the client.

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

## Current scope

The crate intentionally exposes stable common operations directly and leaves
new or agent-specific methods available through `request` and `notify`. Muse's
`_meta.muse` fields and `_session/*` methods are preserved as raw JSON so the
client does not freeze experimental extension shapes. Rich typed mirrors for
every Muse extension, request-handle cancellation, and an async adapter can be
added without changing the wire model.
