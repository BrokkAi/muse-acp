# Replace muse-acp's hand-rolled ACP layer with the official agent-client-protocol SDK

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture

ACP (Agent Client Protocol) is the JSON-RPC protocol an editor (the *client*, for example Zed or a JetBrains IDE) uses to drive a coding agent over the agent's stdin and stdout. `muse-acp` is such an agent: it translates ACP into the Muse Session Protocol (MSP) spoken by `muse serve`.

Before this work, `muse-acp` spoke ACP through `acp-lite`, a crate in this repository (`crates/acp-lite`) that hand-rolled line framing, JSON-RPC request and response matching, batches, `$/cancel_request`, and parse errors, while `src/main.rs`, `src/acp.rs`, `src/fold.rs` and `src/bridge.rs` hand-built every ACP payload as JSON text and parsed every request by hand. Version negotiation (`protocolVersion` 1 or 2) and the rule that `initialize` comes first were also written by hand.

After this work the official Rust SDK, the `agent-client-protocol` crate version 5.0.0 published by the ACP project, does all of that. The SDK owns the transport, JSON-RPC, version negotiation through its `AgentProtocolRouter`, the initialize-first rule, request parsing into its typed schema structs, response and notification serialization from those structs, outgoing requests and their `$/cancel_request` withdrawal, and malformed-frame errors. `acp-lite` is deleted. The adapter keeps only Muse logic, plus extension payloads that no ACP schema defines (JetBrains AIR updates such as `subagent_spawned`, Muse `_meta` members, and Muse extension methods such as `_session/steering`), which travel through the SDK's `UntypedMessage` or the schema's own extension variants.

Observable result: an editor sees the same ACP behavior as before, except where the SDK defines it differently (exact error messages for malformed frames, JSON-RPC batches now refused with `-32600`, as ACP v2 forbids them). The black-box suite `tests/acp_serve.rs` and the live loopback suite pass against the SDK-backed adapter.


## Progress

- [x] (2026-11-02) Baseline: `cargo test --locked --workspace` passes (451 `acp_serve` tests, 265 unit tests).
- [x] (2026-11-02) Prototype in a scratch crate: router with v1 and v2 builders over `Lines`, typed enum dispatch forwarded to a synchronous thread, SDK framing errors, batch refusal.
- [x] (2026-11-02) Milestone 1: transport and JSON-RPC on the SDK; `acp-lite` deleted; batches dropped (commit 8c8bb32).
- [x] (2026-11-02) Milestone 2: typed inbound requests (SDK schema structs) for every standard method, read into one view per method in `src/request.rs` (c90ddaf).
- [x] (2026-11-02) Milestone 3: typed outbound values. Transcript updates (c1d2663); initialize, session setup and config option responses (06bffd8); every other session update (9f9deff); every other standard response (12dd005); permission prompts and forms with typed answers (e024f1c); editor file and terminal requests (8b50ee0); the subagents RFD's `subagent_update` (ca2279e).
- [x] (2026-11-02) Milestone 4: README, `docs/acp-extensions.md`, CHANGELOG, dead-code removal, full gate run.


## Surprises & Discoveries

- Observation: the SDK's typed request enums (`v1::schema::ClientRequest`, `v2::schema::ClientRequest`) claim every method name. Methods outside the schema that do not start with `_` are answered `-32601` by the enum handler before any later handler runs, so handlers for non-standard methods (`session/set_model`, `subagent/*`, `shutdown`, `exit`) must be registered before the enum handler.
  Evidence: in the prototype, `session/set_model` got `{"code":-32601,"message":"Method not found"}` even with an untyped fallback handler registered after the enum handler.

- Observation: the SDK has no API to answer every still-outstanding incoming request from another thread, and a `Responder` dropped while the connection shuts down is not answered. The adapter's shutdown deadline must answer every request even when the main loop is stuck, so the adapter keeps its outstanding `Responder`s in one shared table that the deadline thread can drain.
  Evidence: `ResponderDropGuard::drop` in the SDK's `src/jsonrpc.rs` returns early when `connection_state.is_shutting_down()`.

- Observation: `Builder::connect_to` returns at clean stdin EOF after draining output already queued; responses produced later are lost. An `on_close` callback runs before that drain and may wait, so the adapter's close callback waits until the main loop has answered what it owes.

- Observation: the test client sent requests that do not fit the ACP schema (no `mcpServers` on v1 `session/new`, no `cwd` on `session/load`, uri-only images, `session/cancel` as a request, stdio MCP entries without `args`/`env`). The SDK refuses them with `-32602` (or `-32601` for the request form of a notification). The tests now send conforming params, and the cases that tested non-conforming input assert the SDK's refusal.

- Observation: ACP v2 has no `capabilities.subagents`; the earlier tests advertised one. The schema crate (feature `unstable_subagents`) defines the current subagents RFD: ACP v1 `clientCapabilities.subagents` and the `subagent_update` session update in both versions, which v2 clients must understand without a capability. The adapter had been sending an older draft's `subagent_spawned`/`subagent_state_update` to standard clients.
  Evidence: `SubagentUpdate` in `agent-client-protocol-schema` 1.15.1 `src/v1/client.rs` and `src/v2/client.rs`; the v2 schema text "No Client capability is required."

- Observation: a live side-chat resume failed with `workspace_not_composed` once every `session/resume` carried a `cwd`: the adapter marked host roots pending even when the host reported no workspace. It now marks them only when the host has one.

- Observation: the live test `rename_drives_a_real_host_across_the_capability_registry` failed with `naming_unavailable` on the untouched base commit too (3 of 3 runs) during this work; it depends on the installed Muse host's naming service, not on this change, and passed in the final run.

- Observation: neither schema's `session/load` or `session/resume` result has a `sessionId`; the old adapter repeated the client's id there. Two live tests read it and now read `_meta.mspSessionId`.


- Observation (review, 2026-11-02): the test harness added `mcpServers` to every `session/load`, `session/resume` and `session/fork`, which hid that ACP v2 `session/load` was read with the v1 schema. The harness now adds it for v1 only, as v2 makes it optional. On the RFD path, a settled child that ran again was never shown running; fixed with a regression test. `_`-prefixed extension notifications were dropped; they run again.


## Decision Log

- Decision: drop JSON-RPC batch support.
  Rationale: SDK 5.0.0 refuses batches with `-32600` and the ACP v2 transport rules forbid them; the user confirmed on 2026-11-02 that the adapter drops them as the SDK does.
  Date/Author: 2026-11-02 / agent, confirmed by the user.

- Decision: delete `crates/acp-lite` entirely, including its client side and its tests.
  Rationale: the SDK has a client side too; nothing in the product uses the `acp-lite` client.
  Date/Author: 2026-11-02 / user decision.

- Decision: drop the `arbitrary_precision` feature of `serde_json`.
  Rationale: the SDK's untagged message enum rejects integers wider than 64 bits when it is on.
  Date/Author: 2026-11-02 / user decision.

- Decision: read stdin through the SDK's `Lines` transport with a byte-line reader that decodes each line lossily, instead of the SDK's `Stdio` transport.
  Rationale: `Stdio` ends the connection on an invalid UTF-8 line; with `Lines` such a line reaches the SDK's parser and gets its `-32700` reply, and reading goes on.
  Date/Author: 2026-11-02 / user decision.

- Decision: send standard clients the SDK's typed `subagent_update` (ACP v1 with `clientCapabilities.subagents`, and every ACP v2 client), and keep AIR's own updates for clients that negotiate AIR `nativeSubagentSessions` without the standard v1 capability.
  Rationale: the user asked that nothing the SDK does stays hand-rolled; the SDK defines the subagent update, and AIR's updates are an extension no schema defines.
  Date/Author: 2026-11-02 / agent.

- Decision: build the adapter's outgoing requests (`session/request_permission`, `elicitation/create`, `fs/*`, `terminal/*`) from the SDK's request types and read the answers as the SDK's response types (`request::Answer`), instead of reading raw JSON.
  Rationale: the SDK parses and validates the client's answer; a malformed answer arrives as an error, which every flow already fails closed on.
  Date/Author: 2026-11-02 / agent.

- Decision: run the SDK connection on its own thread under `futures::executor::block_on`, and keep the adapter's synchronous main loop. Handlers registered on the SDK builders forward each incoming message, with its `Responder`, to the main loop over the existing `mpsc` channel.
  Rationale: the adapter's 17,000-line main loop is synchronous and blocks on MSP commands; the SDK is runtime-neutral and needs no tokio. Forwarding keeps message order and lets the SDK keep reading while the main loop is busy.
  Date/Author: 2026-11-02 / agent.

- Decision: follow JetBrains AIR's specification as strictly as ACP's. The written contract is `docs/air-extensions.md` of `agentclientprotocol/claude-agent-acp` (fetched at d2ef490, 2026-10-30); the reference implementations are `@agentclientprotocol/claude-agent-acp` 0.99.0 and `@brokkai/codex-acp` 1.15.0 (the only AIR v2 reference). AIR keys go to AIR clients only, with the agent side in the initialize response's top-level `_meta`. AIR clients get AIR's subagent updates for either subagent signal. ACP v2 uses AIR's `_`-prefixed tags. The fork point is `_meta.jetbrains.air.fork`. Forks replay nothing. `_session/async_task/stop` answers `{stopped}`. File reports carry `unavailable` reasons. `recommendedValue` replaces the default reasoning row.
  Rationale: the user stated that AIR is a spec the adapter adheres to, like ACP.
  Date/Author: 2026-11-02 / user decision.

- Decision: conform to the negotiated schema everywhere Muse owns the surface. Each version serves only its own schema's methods (no v1 `session/load`, `session/set_mode`, `authenticate`, `logout` on v2; no v2 `auth/login`, `auth/logout` on v1). Muse's own methods start with `_`: the subagent controls become `_subagent/*`, and `session/set_model`, `shutdown` and `exit` are removed. A method outside the schema gets `-32601`, and its notification is ignored. JetBrains AIR's shapes stay as AIR defines them: `replayFrom` on v2 `session/fork`, AIR's updates, and the `subagents` initialize member.
  Rationale: the user asked that anything not conforming to the ACP spec change; AIR's shapes are AIR's to define and changing them would break AIR.
  Date/Author: 2026-11-02 / user decision.


## Outcomes & Retrospective

(2026-11-02) Done. `acp-lite` and every hand-built ACP frame are gone. The SDK reads every request into its typed schema, and every response, notification and request the adapter sends is an SDK schema value. What stays the adapter's own JSON is what no ACP schema defines: `_meta` members, the results of extension methods (`_session/*`, `_subagent/*`), JetBrains AIR updates (sent as the v2 `Other` update or, in v1, an `UntypedMessage`), and AIR's `subagents` member of the initialize response. JSON-RPC batches are refused as the SDK refuses them.

Behavior that changed for clients is listed in `CHANGELOG.md` under Unreleased. All workspace tests pass (`acp_serve` 451, unit 270), and the live loopback suite passes against Muse 1.4.14 (52 of 52).

Not done yet, AIR: a settled AIR child that runs again is not reopened as a new generation (`<id>:generation:<n>` with a new `subagent_spawned`), and updates addressed to a settled AIR child are not dropped. Only its later state updates are. Child routing is spread across the MSP event handlers, so this needs its own change.

Not done here because the adapter never did it, by hand or otherwise: the unstable ACP methods the SDK can parse but the adapter does not serve (`session/set_title`, `session/archive`, `session/status`, `auth/status`, `providers/*`) and the v2 `state_update.background` work list.


## Context and Orientation

The repository root holds the `muse-acp` binary crate. Its source is in `src/`:

`src/main.rs` holds the main loop (`fn main`), the ACP request dispatcher (`fn handle_acp`), the MSP event dispatcher (`fn handle_msp`), and most flows (permissions, forms, prompts, session lifecycle). `src/acp.rs` holds the per-session state (`AcpSession`) and the helpers that write ACP frames. `src/fold.rs` turns MSP transcript items into ACP `session/update` payloads. `src/bridge.rs` lets Muse call editor tools (file reads and writes, terminals) by sending ACP requests to the client. `src/shutdown.rs` bounds shutdown with a deadline. `src/json.rs` is the adapter's own JSON value type `J`, used for MSP data.

`crates/acp-lite` is the hand-rolled ACP library being removed. `crates/muse-msp-client` is the MSP client; it is unrelated to ACP and stays.

Tests: `tests/acp_serve.rs` runs the real adapter binary against a scripted fake MSP host (`tests/fixtures/fake_serve.py`) and matches substrings of the ACP frames the adapter writes. `tests/live_loopback.rs` runs against a real `muse serve` when `MUSE_ACP_LOOPBACK=1`.

The SDK: `agent-client-protocol` 5.0.0, with features `unstable_protocol_v2` (ACP v2 types and the `AgentProtocolRouter` that picks v1 or v2 from `initialize.protocolVersion`), `unstable_session_fork` (`session/fork`), and the other unstable features the adapter's surface needs. Its schema types live in `agent_client_protocol::v1::schema` and `agent_client_protocol::v2::schema`. `UntypedMessage` carries any method with raw JSON params; `Responder` answers one incoming request; `ConnectionTo` sends notifications and requests.


## Plan of Work

Milestone 1 moves the transport. A new module `src/editor.rs` owns the SDK connection: it builds `Agent.v1()` and `Agent.v2()` builders, each with a handler for the adapter's non-standard methods first and a handler for the version's typed `ClientRequest`/`ClientNotification` enums second, wraps them in `AgentProtocolRouter`, and drives it over `Lines` on a dedicated thread. Every dispatch is forwarded to the main loop. The `Editor` handle replaces `acp_lite::AgentConnection` (`StdoutShared`): it sends responses through the stored `Responder`s, notifications through `ConnectionTo::send_notification`, and requests through `ConnectionTo::send_request` with `on_receiving_result` posting the answer back to the main loop; cancellation uses the SDK's `RequestCancellationHandle`. The hand-written initialize-first check, version negotiation errors, and batch dispatch are deleted. `crates/acp-lite`, `tests/acp_lite.rs` and the workspace member are removed. Tests that pinned hand-written error text are updated to the SDK's replies.

Milestone 2 types the inbound side: `handle_acp` matches on the SDK's typed request enums and each handler reads typed fields (`NewSessionRequest.cwd`, `PromptRequest.prompt`, `meta`) instead of looking keys up in `J`.

Milestone 3 types the outbound side: responses are SDK response structs, `session/update` payloads are `SessionNotification`/`UpdateSessionNotification` with typed `SessionUpdate` variants (AIR and Muse-only updates use `UntypedMessage` or the v2 `Other` variant), and permission, elicitation, file and terminal requests are the SDK's request structs.

Milestone 4 updates `README.md`, `docs/acp-extensions.md`, `CHANGELOG.md`, removes now-dead helpers, and runs the full contribution gate.


## Concrete Steps

Run from the repository root:

    cargo build --locked
    cargo test --locked --workspace
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo fmt --check
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback


## Validation and Acceptance

All workspace tests pass. A v1 and a v2 `initialize` each get the adapter's capabilities; a malformed line gets `{"jsonrpc":"2.0","id":null,"error":{"code":-32700,...}}` and the adapter keeps reading; a batch gets `-32600`. The live loopback suite passes with Muse installed.


## Idempotence and Recovery

Every step is an ordinary source edit under git; `git diff` shows progress and `git checkout -- <file>` undoes a step.


## Artifacts and Notes

Gate run on 2026-11-02: `cargo fmt --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `cargo run -- --selftest`, `node --test npm/test/launcher.test.cjs`, `python3 -m unittest discover -s scripts -p 'test_*.py'`, `sh -n install.sh`, `node scripts/smoke_npm.cjs target/debug/muse-acp`, and `MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback`.


## Interfaces and Dependencies

`Cargo.toml` depends on `agent-client-protocol = { version = "=5.0.0", features = [...] }` and `futures`. `crate::editor::Editor` is the cloneable handle the whole adapter uses to talk to the client.
