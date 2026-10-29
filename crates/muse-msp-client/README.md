# muse-msp-client

`muse-msp-client` is a synchronous Rust client for the Muse Session Protocol
(MSP), the newline-delimited JSON-RPC protocol that `muse serve` speaks on its
stdin and stdout. It launches one configured host (or connects over any pair
of streams), performs the `initialize` handshake, correlates responses with
per-method deadlines, serializes concurrent writes, delivers notifications
and server-initiated requests as events, classifies how the host exited, and
shuts it down within a bounded time.

It is the MSP engine inside `muse-acp`, extracted so tools that need Muse
sessions can use it without speaking ACP. The crate has no dependencies (it
carries its own small JSON type in `muse_msp_client::json`) and no async
runtime, and it never reads the environment: the executable, arguments,
variables, working directory, and capabilities are all explicit.

## Quick start

This checkout has not published the crate to crates.io. Use the workspace path
from another package in this repository:

```toml
[dependencies]
muse-msp-client = { path = "crates/muse-msp-client" }
```

```rust
use muse_msp_client::json::{J, esc};
use muse_msp_client::{
    ClientOptions, LaunchConfig, MspClient, MspEvent, Params, ServerRequestMode,
};

let mut config = LaunchConfig::new("/home/me/.local/bin/muse");
config.current_dir("/home/me/project").options(
    ClientOptions::default().server_request("approval/request", ServerRequestMode::Acknowledge),
);
let (client, events) = MspClient::launch(config)?;

let started = client.request(
    "session/start",
    Params::raw(format!(
        r#"{{"commandId":{},"workspaceRoot":{}}}"#,
        esc(&client.mint_command_id()),
        esc("/home/me/project")
    )),
)?;
let session = started
    .get("session")
    .and_then(|session| session.get("sessionId"))
    .and_then(J::as_str);

client.request(
    "turn/start",
    Params::raw(format!(
        r#"{{"commandId":{},"sessionId":{},"input":[{{"type":"text","text":"Explain this repository"}}]}}"#,
        esc(&client.mint_command_id()),
        esc(session.unwrap_or_default())
    )),
)?;

for event in &events {
    match event {
        MspEvent::Notification { method, .. } if method == "turn/completed" => break,
        MspEvent::Notification { method, .. } => println!("{method}"),
        MspEvent::Request { method, params, .. } => {
            // Already acknowledged; decide with `approval/decide`.
            println!("{method}: {params:?}");
        }
        MspEvent::Closed { reason } => {
            println!("host closed: {reason}");
            break;
        }
        _ => {}
    }
}
client.close();
```

`examples/session.rs` is a complete program that runs one prompt, prints the
agent's messages, and declines or approves tool calls:

```sh
cargo run -p muse-msp-client --example session -- ~/.local/bin/muse ~/project "Explain this repository"
```

## Launch configuration

`LaunchConfig::new(program)` runs `<program> serve`, then the arguments added
with `arg`/`args`, then the typed `HostFlags` (`--no-session-log`,
`--disable-write`, `--disable-shell`). Like `std::process::Command`, the
child inherits this process's environment and working directory unless
`env_clear`, `env`, `env_remove`, or `current_dir` say otherwise. `ShutdownPolicy`
sets how long `close` waits after closing stdin before it kills the host
(5 seconds) and how long it waits in total (7 seconds).

`ClientOptions` holds what the handshake sends and how the connection
behaves: `ClientInfo` (`name` must match `[a-z0-9_]+`), `Capabilities`
(`experimentalApi`, `userInputDialogs`, `requestedCapabilities`,
`optOutNotificationMethods`; unset members are omitted), the server-request
table, the write timeout (5 seconds), and an optional single request timeout
that replaces the per-method table.

## The handshake

`MspClient::launch` is `MspClient::spawn` (process and pipes only) followed by
`MspClient::initialize`. Use the two steps separately when something must
happen between them, such as registering the client with a watchdog.
`initialize` sends `initialize`, checks the result, and then sends
`initialized`; MSP accepts no session or turn command before that
notification. The default check refuses an envelope schema version other than
`ENVELOPE_SCHEMA_VERSION` (1); `ClientOptions::handshake_check` replaces it,
for example to compare the schema fingerprint against builds you have tested.

`Handshake` exposes the server name and version, the schema version and
fingerprint, the granted capabilities (`granted("sessionMcp")`), the session
durability (`session_durability()`: absent reads as durable, an unknown value
is not durable), the sandbox facts, `release()`/`at_least()` for version
gates, and the complete `initialize` result for anything else.

## Requests, cancellation, and retries

`request` waits for the method's default deadline (`default_timeout`: 30
seconds for the handshake, queries, and control decisions, 180 seconds for
history-bearing lifecycle work and plugin installs, 60 seconds otherwise).
`request_with_timeout` sets one explicitly. Both return the JSON result or an
`MspError`: `Rpc` carries the host's error object verbatim (`code()`,
`message()`, `kind()` read it); `Timeout`, `Cancelled`, `HostClosed`, `Closed`,
and `Write` are local outcomes.

`start_request` writes the request and returns a `PendingRequest`. Its
`handle()` is a cloneable `RequestHandle`; `cancel()` wakes the waiter with
`MspError::Cancelled` from any thread. Cancellation is local: MSP has no
request-cancellation message, so the host may still act on a frame it
already received. Stop work with the MSP command meant for it, such as
`turn/cancel`. A response that arrives after a timeout or cancellation is
ignored.

MSP commands carry a caller-minted `commandId` (`mint_command_id` makes a
UUIDv7, the only shape the host accepts). The client never mints or rewrites
ids inside params. After a timeout, retry a command with the same
`commandId`: the host recognizes the duplicate. Never mint a fresh id for a
retry.

Params are `Params::raw(json_text)` (sent verbatim, not validated),
`Params::from(&value)` for a `json::J`, or `Params::empty()` for `{}`.

## Events and server requests

The receiver returned with the client yields `MspEvent::Notification`,
`MspEvent::Request` (server-initiated requests), `MspEvent::Stderr` (only with
`ClientOptions::stderr_events(true)`), and one `MspEvent::Closed` when the
host's stdout ends. Dropping the receiver closes the connection, because
nothing would answer the host's requests.

Each server-request method needs a `ServerRequestMode`. `Acknowledge` replies
`{}` before delivering the event; that is MSP's contract for
`approval/request` and `userInput/request`, whose decision travels later as
`approval/decide` or `userInput/answer`. `Respond` delivers the request with
its id and leaves the answer to `respond` or `respond_error`. Any method
without a mode gets the typed MSP `methodNotFound` error, never a synthetic
success.

## Failure behavior

- The waiter is registered before the frame is written, and every request
  settles exactly once.
- A frame that does not finish writing within the write timeout cannot be
  withdrawn from the pipe, so the client kills the host (or calls the custom
  transport's `on_terminate` hook) and fails every pending request. An
  undelivered command can never run later.
- When the host closes stdout, every pending request fails immediately with
  `HostClosed` instead of waiting for its deadline.
- Malformed or non-UTF-8 frames are skipped and reported as diagnostics; the
  connection keeps working.
- A stdout frame larger than 16 MiB closes the connection with a typed
  diagnostic instead of growing the reader buffer without bound.
- Host stderr is kept in a tail of at most 100 lines and 8 KiB
  (`stderr_tail()`), attached to the exit classification, and never parsed
  as protocol input.

## Shutdown and exit classification

`close` fails pending requests, closes stdin, waits for the host to exit
within the grace period, kills it if needed, and returns the
`ExitClassification`. Every step is bounded, and calling it again is harmless.
`reap` classifies an exit that already happened (it waits at most 250ms).
`force_stop` kills without waiting for locks, for use from a watchdog.
Dropping a client that was never closed closes it in the background.

`ExitClassification` follows MSP §2.11: exit code 0 is a clean shutdown, 1 an
unhandled error, 2 a usage error, 3 a configuration error, 4 an unavailable
session lease, 5 a disabled SDK surface, and anything else (or a signal) a
crash. `retryable()`, `retry_advice()`, and `support_lines()` turn it into a
decision or a diagnostic. `probe_exit` runs the configured host with stdin
closed and classifies how it ends, without sending any frame. `support_lines()`
reports the captured stderr byte count but omits its contents; a caller that
deliberately handles potentially sensitive host output can read `stderr_tail`
directly.

## Embedding hooks

`ClientOptions::diagnostics` receives typed `Diagnostic` values (skipped
frames, unsupported server requests, late responses, notification method
names, shutdown timeouts, captured stderr size). They never include payloads
or stderr text. `ClientOptions::watchdog` arms a caller-owned guard around the
client's own cleanup steps, so a process that must never hang can bound them.
Without hooks the client is silent.

`MspClient::connect` runs the same engine over any `Read` and `Write` pair
wrapped in a `Transport`, for tests and in-process hosts.

## Tests

`cargo test -p muse-msp-client` needs neither Muse nor credentials. Unit tests
drive an in-memory transport; `tests/process.rs` re-executes its own test
binary as a scripted fake `muse serve` to cover exit codes, kills, stderr, and
probes on every platform. `tests/live_muse.rs` runs the whole flow against a
real `muse serve` whose model calls go to the repository's loopback provider:

```sh
MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-msp-client --test live_muse
```

## Current scope

The crate covers the transport and the handshake. It has no typed mirrors of
MSP methods or notifications, no session state, and no reconnect or restart
logic: whether a host may be relaunched depends on its durability profile and
the caller's own session bookkeeping. `muse-acp` keeps its ACP translation,
editor session state, approval prompts, and Muse discovery to itself.
