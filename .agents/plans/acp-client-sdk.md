# Extract a reusable ACP client SDK

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This plan follows the repository standard in `.agents/PLANS.md`.

## Purpose / Big Picture

`muse-acp` currently contains the agent half of the Agent Client Protocol: editors send requests to the binary and the binary sends session updates back. That code proves the wire behavior but is not reusable by programs that need to launch and drive ACP agents. After this change, a Rust application can depend on the new `muse-acp-client` library, launch any ACP stdio agent, initialize either protocol version, create a session, send a prompt, observe streaming events, answer or refuse agent-initiated requests safely, and shut the child process down without writing JSON-RPC transport code itself.

The result is demonstrated by a workspace crate with its own unit tests and by a black-box test that uses the new client to drive the existing `muse-acp` binary against `tests/fixtures/fake_serve.py`. The MSP side remains in `src/msp.rs`; a separate GitHub issue will track its extraction. GitHub issue creation is an external operation and is recorded in Progress rather than implemented in code.

## Progress

- [x] (2026-10-20T16:20Z) Inspected the current ACP agent loop, the official Rust ACP SDK surface cached in the local Cargo registry, and the fake Muse host tests. Established that the missing reusable layer is the client-side connection runtime, not the agent request router.
- [x] (2026-10-20T16:20Z) Created this ExecPlan.
- [x] (2026-10-20T16:35Z) Added the Cargo workspace and `crates/muse-acp-client`, with `muse-acp` kept as a dependency-free binary package.
- [x] (2026-10-20T16:45Z) Implemented the NDJSON JSON-RPC connection core, process transport, request correlation, timeouts, event delivery, safe inbound-request dispatch, and bounded shutdown.
- [x] (2026-10-20T16:50Z) Implemented safe default request handling and common ACP helpers for initialization, sessions, prompts, cancellation, listing, loading, resuming, deleting, and closing.
- [x] (2026-10-20T16:55Z) Added the crate guide and focused tests for response correlation, notifications, reverse requests, version-specific initialization, timeout behavior, EOF, and agent-request cancellation.
- [x] (2026-10-20T17:00Z) Added black-box ACP v1 and v2 tests that launch the existing `muse-acp` binary against `tests/fixtures/fake_serve.py`.
- [x] (2026-10-20T17:05Z) Ran `cargo fmt --check`, workspace clippy with warnings denied, the complete workspace test suite, the adapter self-test, Python release-script tests, and installer shell syntax checks. The Node launcher test was attempted but hung in this container and was interrupted after several minutes.
- [x] (2026-10-20T17:20Z) Created issue #328, "Extract msp.rs into a reusable Muse MSP client crate", after running the GitHub operation outside the restricted sandbox.

## Surprises & Discoveries

- Observation: the current ACP code is already an ACP client for a narrow nested flow. It sends `session/request_permission` and `elicitation/create`, but it is primarily an agent-side request router and cannot launch or drive another ACP agent.
  Evidence: `src/main.rs` parses ACP requests in `handle_acp`, while `src/acp.rs` emits responses and notifications through `StdoutShared`.

- Observation: the official `agent-client-protocol` 3.4.0 crate already provides a general asynchronous client. The new crate should therefore differentiate through a small synchronous API, explicit safe defaults, and a dependency-light stdio process runtime rather than attempting to reproduce the entire official surface.
  Evidence: the cached crate README documents `Client`, `AcpAgent`, typed v1/v2 schemas, process transport, permissions, and conformance tests.

- Observation: ACP v1 keeps the `session/prompt` request open until the turn terminates, while the existing adapter settles v2 through `state_update`. The client must expose the prompt response and streaming events independently so callers can use both versions correctly.
  Evidence: `src/main.rs` sends prompt results for v1 at the turn terminal and sends v2 state updates instead.

- Observation: GitHub writes need access outside the restricted sandbox even though the public repository can be read from inside it. The sandboxed `gh auth status` did not expose a usable credential, while the explicitly approved unsandboxed branch push, PR creation, and issue creation all succeeded.
  Evidence: the branch produced PR #327 and the MSP follow-up produced issue #328.

- Observation: the existing Node launcher test did not reach its first test in this container. A 45-second timeout cancelled the file while Node's event loop contained only a pending promise; this occurs before any launcher assertion and is independent of the new Rust crate.
  Evidence: `timeout 45s node --test npm/test/launcher.test.cjs` exited 124 with `tests 1`, `pass 0`, `cancelled 1`, and no child diagnostics.

## Decision Log

- Decision: implement a synchronous client with background reader and writer threads.
  Rationale: the adapter and repository tests are synchronous and dependency-light; callers get blocking request methods and a thread-safe event receiver without adopting an async runtime. The public handle will still be cloneable and safe for concurrent requests.
  Date/Author: 2026-10-20 / Codex

- Decision: do not depend on `agent-client-protocol` for the extracted crate.
  Rationale: the stated goal is an owned client SDK that can compete with the official implementation. Keeping the transport and public API independent makes that comparison meaningful, while preserving raw JSON values keeps unknown ACP extensions forward-compatible.
  Date/Author: 2026-10-20 / Codex

- Decision: use `serde` and `serde_json` in the library while leaving the adapter binary dependency-free.
  Rationale: a reusable public SDK should deserialize into normal Rust types instead of copying the adapter's private `J` parser and formatting each wire frame by hand. A workspace-local dependency does not change the shipped binary unless the binary opts into the crate.
  Date/Author: 2026-10-20 / Codex

- Decision: every agent-initiated request is handled on a separate thread through a `ClientHandler`.
  Rationale: a handler may issue another ACP request while answering an agent request. Handling it on the reader thread would deadlock because the response could not be read until the handler returned.
  Date/Author: 2026-10-20 / Codex

- Decision: the default handler refuses permissions, declines elicitation, and returns JSON-RPC method-not-found for filesystem and terminal requests.
  Rationale: clients must not silently grant access or claim capabilities they do not implement. Applications can replace the handler when they want interactive behavior.
  Date/Author: 2026-10-20 / Codex

- Decision: keep the MSP extraction out of this change and track it separately.
  Rationale: the user explicitly requested the ACP client extraction now and an issue for the MSP client. Mixing the two increases review and regression risk.
  Date/Author: 2026-10-20 / Codex

- Decision: track the MSP extraction in issue #328 rather than mixing it into the ACP client PR.
  Rationale: focused changes are reviewable and independently releasable. The issue records the required transport extraction, coupling removals, public API, failure behavior, tests, and non-goals.
  Date/Author: 2026-10-20 / Codex

## Outcomes & Retrospective

The ACP client extraction is implemented. `muse-acp-client` is a synchronous,
runtime-independent client with configurable process and custom transports,
ACP v1/v2 initialization, session and prompt helpers, request correlation and
timeouts, streaming events, reverse-request dispatch on independent threads,
agent-request cancellation, bounded stderr capture, and graceful process
shutdown. Eight crate tests cover the connection core and safe request
handling, and two black-box tests drive the existing adapter over ACP v1 and
v2. The complete workspace suite, clippy, formatting, adapter self-test,
Python release-script tests, and shell syntax check pass.

The deliberate initial boundaries are rich typed types for every Muse
extension, a public pending-request handle, and an async adapter. Unknown
extension data remains lossless through `serde_json::Value`, and callers can
still issue any method through `request`.

The MSP extraction is tracked in issue #328. The implementation PR is #327.

## Context and Orientation

The repository root is a Rust binary package named `muse-acp`. `src/main.rs` contains the editor-facing ACP agent loop. `src/acp.rs` contains session state and outbound ACP frame helpers. `src/msp.rs` contains the separate Muse Session Protocol client. `tests/acp_serve.rs` launches the binary against `tests/fixtures/fake_serve.py`, which behaves enough like `muse serve` to exercise adapter behavior without credentials.

ACP is a JSON-RPC 2.0 protocol carried as one JSON object per newline over an agent's stdin and stdout. A client sends requests with an `id` and receives matching responses. Notifications have no `id`. Agents may also send requests back to clients, for example to ask permission to run a tool. The client must keep reading while a request is pending so notifications and reverse requests continue to flow.

The new library lives at `crates/muse-acp-client/`. The root `Cargo.toml` becomes the root package of a workspace containing that crate. The binary remains at the repository root and is not rewritten to use the new client, because it is the agent rather than the client. A root integration test uses the library as a client against the binary.

## Plan of Work

First, add the workspace declaration and crate manifest. The crate exposes modules for protocol values, inbound request handling, process configuration, and the connection runtime. Its public names use ordinary asynchronous-free Rust: `Client`, `ClientHandle`, `ClientHandler`, `ClientEvent`, `AgentProcess`, and `ClientError`.

Second, implement the transport. A writer thread serializes JSON values and writes one newline-terminated frame at a time. A reader thread parses incoming lines, matches responses to pending request IDs, forwards notifications to the caller, and starts an independent handler thread for each agent-initiated request. Request IDs are monotonically increasing unsigned integers. A request timeout removes its waiter, returns a typed timeout error, and ignores any late response. EOF fails all pending requests and publishes a closed event. A malformed inbound frame closes the connection with a protocol error rather than being silently ignored.

Third, implement process ownership. `AgentProcess` stores the executable, arguments, environment additions, and working directory. `Client::spawn` starts the child with piped stdin and stdout, drains stderr into a bounded tail, and keeps the child handle for graceful shutdown. Dropping the client closes stdin, waits for the configured shutdown deadline, and kills the child if it remains alive. A generic `Client::connect` constructor over an arbitrary reader and writer keeps the transport testable.

Fourth, implement protocol helpers. `Client::initialize` sends the correct v1 or v2 initialization field names, validates the negotiated version, sends the `initialized` notification, and stores the negotiated version for request handlers. Helpers cover new, load, resume, list, delete, close, prompt, and cancel. Less common or future methods remain available through `request`, `request_with_timeout`, and `notify`, preserving arbitrary JSON values.

Fifth, add tests. Unit tests use an in-memory pipe or small fake peer to prove request correlation, notifications, reverse requests, safe defaults, timeouts, malformed frames, and EOF settlement. A root integration test uses `Client::spawn` to launch the real adapter with `MUSE_CLI` pointing at `tests/fixtures/fake_serve.py`, initializes, creates a session, prompts, observes the expected update, and closes cleanly. The integration test covers both ACP v1 and v2 because the adapter shapes differ.

Finally, run all contribution gates. If GitHub remains unauthenticated, preserve the prepared MSP issue title and body in the final handoff and report that external issue creation was blocked, rather than silently claiming it was created.

## Concrete Steps

Run all commands from the repository root:

    /home/ryansvihla/code/muse-acp/.mj/clones/91df41ae79c3b2dfc03744de20759225

After adding the crate:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo test --locked --test acp_client_sdk
    cargo run --locked -- --selftest

Before completion, also run the existing repository gates:

    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    sh -n install.ps1 2>/dev/null || true

The last PowerShell check may be skipped when `pwsh` is unavailable. The expected successful result is zero failed Rust tests in the workspace, one passing `acp_client_sdk` integration test, a successful self-test, and unchanged release-script tests.

## Validation and Acceptance

A caller can construct `AgentProcess::new("muse-acp")`, add arguments and environment variables, and pass it to `Client::spawn`. It can call `initialize` with ACP v1 or v2 and receive a response whose negotiated protocol version is available through `protocol_version`. It can create a session, send a text prompt, read `session/update` notifications from the event receiver, send `session/cancel`, and call `close`.

The crate's connection test must show that a response with id 2 resolves request 2 while request 1 remains blocked, and that a notification is delivered during that interval. A reverse request must invoke the configured handler and receive exactly one response. The default handler must select an offered reject permission option when one exists and otherwise return a cancelled outcome. It must decline elicitation and return method-not-found for filesystem and terminal requests.

The black-box test must fail if the client sends malformed initialization fields, misses the `initialized` notification, mishandles newline framing, loses streaming events, or leaves the child process running. For both v1 and v2, the test must observe the fake host's agent message and terminal state.

## Idempotence and Recovery

The workspace and crate additions are additive. Repeated `cargo` commands update `Cargo.lock` deterministically. Tests use unique temporary directories and terminate spawned children on both success and failure. If a test leaves a child behind because the test process is killed, remove only the temporary directory reported by the test and rerun the targeted test; no repository state needs repair.

If GitHub authentication is unavailable, do not attempt to modify credentials in the repository or print tokens. The MSP issue can be recreated later from its saved title and body. The ACP extraction does not depend on the issue existing.

## Artifacts and Notes

The GitHub issue requested for the MSP extraction is expected to be titled:

    Extract msp.rs into a reusable Muse MSP client crate

Its body should describe the current `MspHost` transport and correlation behavior, list the coupling to `host_config`, environment variables, diagnostics, shutdown registration, and adapter state, and define acceptance as a reusable library that can launch a configurable Muse host, initialize it, send requests and notifications, expose server requests and events, time out requests, restart or reattach when configured, and close without depending on the ACP adapter.

## Interfaces and Dependencies

The crate manifest uses `serde` with derive support and `serde_json`. It does not depend on `muse-acp` or `agent-client-protocol`.

In `crates/muse-acp-client/src/lib.rs`, expose:

    pub struct AgentProcess;
    pub struct Client;
    pub struct ClientHandle;
    pub struct ClientOptions;
    pub enum ClientError;
    pub enum ClientEvent;
    pub enum ProtocolVersion;
    pub trait ClientHandler;
    pub struct IncomingRequest;
    pub struct RequestContext;
    pub enum RequestOutcome;
    pub struct InitializeRequest;
    pub struct InitializeResponse;
    pub struct NewSessionRequest;
    pub struct PromptRequest;

The central public methods are:

    impl AgentProcess {
        pub fn new(program: impl Into<std::ffi::OsString>) -> Self;
        pub fn arg(&mut self, arg: impl Into<std::ffi::OsString>) -> &mut Self;
        pub fn env(&mut self, key: impl Into<std::ffi::OsString>, value: impl Into<std::ffi::OsString>) -> &mut Self;
        pub fn current_dir(&mut self, dir: impl Into<std::path::PathBuf>) -> &mut Self;
    }

    impl Client {
        pub fn spawn(agent: AgentProcess, handler: std::sync::Arc<dyn ClientHandler>, options: ClientOptions) -> Result<Self, ClientError>;
        pub fn connect<R, W>(reader: R, writer: W, handler: std::sync::Arc<dyn ClientHandler>, options: ClientOptions) -> Result<Self, ClientError>
        where R: std::io::BufRead + Send + 'static, W: std::io::Write + Send + 'static;
        pub fn initialize(&self, request: InitializeRequest) -> Result<InitializeResponse, ClientError>;
        pub fn new_session(&self, request: NewSessionRequest) -> Result<serde_json::Value, ClientError>;
        pub fn prompt(&self, request: PromptRequest) -> Result<serde_json::Value, ClientError>;
        pub fn cancel_session(&self, session_id: &str) -> Result<(), ClientError>;
        pub fn request(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, ClientError>;
        pub fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), ClientError>;
        pub fn next_event(&self) -> Result<ClientEvent, ClientError>;
        pub fn close(&mut self) -> Result<(), ClientError>;
    }

`ClientHandle` exposes the same request and notification operations without owning the child process. `ClientHandler` must be `Send + Sync + 'static`. `RequestOutcome` contains either a JSON result or an `RpcError`.
