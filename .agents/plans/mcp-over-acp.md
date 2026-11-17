# Let Muse use MCP servers an ACP client hosts over the ACP connection (issue #317)

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.

Status: research complete, implementation stopped before the first code change. The ACP binding and the Muse host speak two different, incompatible eras of MCP, and every way around that needs a maintainer decision. The Decision Log and the Plan of Work describe the options and what each would take.


## Purpose / Big Picture


An editor that speaks the Agent Client Protocol (ACP, the JSON-RPC protocol an editor such as Zed uses to drive a coding agent over stdin and stdout) can offer the agent extra tools through MCP (Model Context Protocol) servers. Today `muse-acp` forwards an editor's stdio and HTTP MCP servers to Muse. ACP's unstable "MCP-over-ACP" feature adds a third kind: an MCP server that lives inside the editor process itself and is reached over the ACP connection the editor already has, with no extra process and no network port. The editor declares it in a session's `mcpServers` list as `{"type":"acp","name":...,"serverId":...}`, and the agent sends each MCP request to the editor wrapped in an ACP `mcp/message` request.

After this work, a client that hosts such a server would see Muse list and call its tools: the model would get `mcp__<name>__<tool>` tools, each call would go through Muse's normal approval, and the call would reach the editor as `mcp/message`. GitHub issue #317 asks for exactly that.

The work stopped before implementation because of what the research found: the ACP binding carries only the newest, "modern" MCP revision (2026-07-28, no `initialize` handshake, every request self-describing), while Muse's MCP client is a "legacy" client (revision 2025-06-18, `initialize` first). The upstream ACP design document says an HTTP adapter like the one this feature needs must not add a fallback for legacy clients. See Surprises & Discoveries for the evidence and the Decision Log for the options.


## Progress


- [x] (2026-11-17 09:20Z) Read issue #317, `AGENTS.md`, `CONTRIBUTING.md`, `.agents/PLANS.md`, and the related plans `session-mcp-forwarding.md`, `editor-fs-bridge.md`, `acp-sdk-5.3-adoption.md`, and `security-audit-2026-10-followups.md`. Created branch `feature/mcp-over-acp` from master `d1ea6e4`.
- [x] (2026-11-17 09:30Z) Read the MCP-over-ACP types in the schema crate `agent-client-protocol-schema` 1.18.0 (`src/v1/mcp.rs`, `src/v2/mcp.rs`, `src/v1/agent.rs`, `src/v1/client.rs`, `src/v2/agent.rs`, `src/v2/client.rs`) and the SDK crate `agent-client-protocol` 5.3.0 (`src/mcp_server/`, `src/schema/mcp.rs`, `src/jsonrpc/incoming_actor.rs`, `src/session.rs`).
- [x] (2026-11-17 09:35Z) Downloaded and read the upstream unstable schemas (v1 and v2 `schema.unstable.json`), the docs index `llms.txt`, the RFD "MCP-over-ACP: Stateless MCP Requests over ACP", the upstream SDK's `md/mcp-bridge.md` and `agent-client-protocol-polyfill` README, and the MCP 2026-07-28 "Versioning and Compatibility" page.
- [x] (2026-11-17 09:40Z) Confirmed the adapter compiles with the SDK's `unstable_mcp_over_acp` feature turned on (`cargo check --locked -p muse-acp --features agent-client-protocol/unstable_mcp_over_acp`), so the SDK can carry the ACP side.
- [x] (2026-11-17 09:45Z) Probed the local Muse (1.4.23-R8977.1; it self-updated from 1.4.18-R8587.1 earlier the same morning) through the built adapter with a scratch HTTP MCP server, once answering as a legacy server and once as a modern-only server.
- [x] (2026-11-17 10:10Z) Wrote this plan and stopped. No source, test, or documentation file other than this plan was changed.
- [x] (2026-11-17) Re-checked against the newest releases: `agent-client-protocol` 5.3.0, schema 1.18.0, and polyfill 5.3.0 are the latest on crates.io; the RFD file `docs/rfds/mcp-over-acp.mdx` last changed on 2026-09-30 (#2265); Muse 1.4.23-R8977.1 is the newest build. Ran two more Muse probes (see Surprises).
- [x] (2026-11-17) Maintainer decision: stay on the ACP design (option B). #317 stays open and parked until Muse's MCP client speaks MCP 2026-07-28 (modern or dual-era).
- [ ] Milestones 1 to 5 of the Plan of Work, without the legacy layer, once a Muse build passes the modern-only probe.


## Surprises & Discoveries


- Observation: the ACP binding has exactly one method, `mcp/message`, used in two directions, and no connect, disconnect, or initialize exchange. The agent sends a request to the client; the client may send notifications for a request that is still open. The ACP request's own JSON-RPC id is separate from the logical MCP request id `requestId`, which the agent generates.
  Evidence: schema 1.18.0 `src/v1/client.rs` lists `AgentRequest::MessageMcpRequest` with response `ClientResponse::MessageMcpResponse` (method `mcp/message`, agent to client); `src/v1/agent.rs` lists `ClientNotification::MessageMcpNotification` (client to agent). `src/v2/client.rs` and `src/v2/agent.rs` mirror them. The RFD says: "Earlier drafts used server `id`/`acpId`, an MCP connection ID, connect/disconnect methods, and reverse requests ... This revision replaces them with server-addressed requests and request-scoped notifications targeting MCP 2026-07-28 only."

- Observation: the wire shapes, identical in v1 and v2. A declaration is `{"type":"acp","name":"project-tools","serverId":"project-tools:7a72"}` (`McpServerAcp`, field `serverId`, an opaque string the client generates and must not reuse for another server on the same connection). A request's params are `{"serverId","requestId","method","params"?,"_meta"?}`. A successful response carries exactly one inner outcome, `{"result": <any JSON, even null>}` or `{"error": {"code","message","data"?}}`; an inner MCP error is still a successful ACP response. A notification's params have the same five members.
  Evidence: schema 1.18.0 `src/v1/mcp.rs` (`MessageMcpRequest`, `MessageMcpResponse`, `MessageMcpNotification`, `McpError`, `McpRequestId`), `src/v1/agent.rs` (`McpServer::Acp`, `McpServerAcp`, `McpServerAcpId`), and the test `test_mcp_server_acp_serialization`.

- Observation: the capability lives at `agentCapabilities.mcpCapabilities.acp` (boolean, default false) in ACP v1 and at `capabilities.session.mcp.acp` (an object; `{}` means supported, omitted or null means not) in ACP v2. Both are behind the SDK feature `unstable_mcp_over_acp`, which this repository does not enable today.
  Evidence: schema 1.18.0 `src/v1/agent.rs` `McpCapabilities::acp`, `src/v2/agent.rs` `McpCapabilities::acp` and `McpAcpCapabilities`; upstream `schema/v1/schema.unstable.json` `$defs.McpCapabilities.properties.acp` and `schema/v2/schema.unstable.json` `$defs.McpAcpCapabilities`; `Cargo.toml` features list.

- Observation: outer ACP errors on `mcp/message` are reserved for the binding itself: `-32602` malformed envelope or duplicate active `(serverId, requestId)`, `-32800` cancelled, `-33000` binding resource limit, `-33001` server registration unavailable, `-33002` backend failed without a valid MCP outcome. Cancellation of one operation is ACP's ordinary `$/cancel_request` for the outer request id.
  Evidence: RFD tables "Binding failures" and "Cancellation and lifetime"; SDK 5.3.0 `src/mcp_server/mod.rs` constants `MCP_RESOURCE_LIMIT_EXCEEDED`, `MCP_SERVER_UNAVAILABLE`, `MCP_BACKEND_FAILURE`; `src/jsonrpc/incoming_actor.rs` answers an unclaimed `mcp/message` request with `-33001`.

- Observation (the blocker, part one): the binding carries only modern MCP, revision 2026-07-28. There is no `initialize`. Every inner request must carry `params._meta["io.modelcontextprotocol/protocolVersion"] = "2026-07-28"` and an `io.modelcontextprotocol/clientCapabilities` object. The SDK's own provider side, which a Rust client uses to host a server (`SessionBuilder::with_mcp_server`, v1 and v2), enforces this.
  Evidence: the RFD: "This draft targets **MCP 2026-07-28 only**. It does not emulate older MCP revisions ... there is no MCP initialization handshake"; "a caller without a mutual version surfaces that error rather than falling back to `initialize` or an earlier revision". SDK 5.3.0 `src/mcp_server/active_session.rs`:

      const MCP_VERSION: &str = "2026-07-28";
      fn validate_modern_request(method, params) {
          if method == "initialize" { method_not_found().data("native MCP requests do not use initialize") }
          ... requires _meta["io.modelcontextprotocol/protocolVersion"] == MCP_VERSION (else -32022)
          ... requires _meta["io.modelcontextprotocol/clientCapabilities"] to be an object (else -32602)
      }

  and `handle_request` returns that failure as the inner outcome. `src/mcp_server/server.rs` wraps both the v1 and the v2 attachment in this handler.

- Observation (the blocker, part two): the ACP design explicitly forbids the kind of HTTP adapter this feature would need from serving legacy MCP clients, and the upstream SDK's reference adapter follows that.
  Evidence: the RFD, section "Proxying and HTTP adaptation": "An optional adapter may expose a native server to a **modern MCP HTTP client**. HTTP capability alone does not prove that an agent supports this MCP revision. The adapter must not add a legacy fallback." The upstream `agent-client-protocol-polyfill` README (crate 5.3.0 on crates.io): "`McpOverAcpPolyfill::http()` re-exports native MCP-over-ACP servers through loopback HTTP for a final agent that supports **MCP 2026-07-28** over HTTP ... It does not provide a fallback to older MCP revisions." Its guide `md/mcp-bridge.md`: "The adapter does not accept batches, client responses, MCP initialize/session headers, or standalone GET/DELETE streams."

- Observation (the blocker, part three): Muse's MCP client is a legacy 2025-06-18 client. It opens with `initialize`, sends no per-request version metadata, never tries the modern `server/discover`, and quietly drops an optional server that refuses `initialize`.
  Evidence: the built adapter (`target/debug/muse-acp`) driving Muse 1.4.23-R8977.1 with an ACP v1 `session/new` carrying one `{"type":"http"}` server that points at a scratch logging server. Against a legacy-style server:

      POST /mcp  mcp-protocol-version: 2025-06-18
        {"id":1,"method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"tbh","version":"0.1.0"},"protocolVersion":"2025-06-18"}}
      POST /mcp  {"method":"notifications/initialized","params":{}}
      POST /mcp  {"id":2,"method":"tools/list","params":{}}
      POST /mcp  {"id":3,"method":"tools/call","params":{"_meta":{"callId":"call_0","idempotencyKey":"tool:call_0","traceparent":"00-..."},"arguments":{"message":"hi"},"name":"echo"}}
      PERMISSION mcp__probe__echo ; tool_call_update completed "echo: hi"

  Against a modern-only server that answers `initialize` with `-32601` and would answer `server/discover`:

      POST /mcp  {"id":1,"method":"initialize",...,"protocolVersion":"2025-06-18"}
      (nothing else; the turn ran without the tool)

  The MCP 2026-07-28 "Versioning and Compatibility" page states the same outcome in general: "Legacy | Modern | Fails ... Legacy clients have no fall-forward mechanism", while a "dual-era" server that also answers `initialize` "MAY" serve both.

- Observation: MSP (Muse's session protocol) gives the adapter no way to pick Muse's MCP revision or to hand Muse a server reached any other way. `SessionMcpServerConfig` is a closed union of `stdio` (`command`, `args`, `env`, `framing`, `mode`) and `streamableHttp` (`url`, `headers`, `mode`).
  Evidence: `tests/protocol/stable/msp.schema.json` (Muse 1.4.20) `$defs.SessionMcpServerConfig`, whose description says a future transport "arrives as an explicit schema addition".

- Observation: the SDK is not the obstacle. With `unstable_mcp_over_acp` turned on the adapter still compiles, and the typed requests fit the existing outgoing-request path: `Editor::request_blocking` in `src/editor.rs` sends any `sdk::JsonRpcRequest` (the SDK implements it for both `v1::schema::MessageMcpRequest` and `v2::schema::MessageMcpRequest` in `src/schema/mcp.rs` and `src/schema/v2_impls.rs`) and withdraws it with `$/cancel_request`. Client notifications arrive as typed `ClientNotification::MessageMcpNotification`.
  Evidence: `cargo check --locked -p muse-acp --features agent-client-protocol/unstable_mcp_over_acp` finished without errors or warnings.

- Observation: the Muse binary on this machine is 1.4.23-R8977.1, not the 1.4.18 the task assumed; it self-updated at 10:15 local time on 2026-11-17 (`~/.local/bin/.muse-update-notice` reads `1.4.18-R8587.1 1.4.23-R8977.1`). Its MSP schema fingerprint is not in `src/compat.rs`. The binary contains MCP 2026-07-28 type names (`server/discover`, `io.modelcontextprotocol/protocolVersion`, `input_required`), but the session-MCP client the probe exercised still behaves as a 2025-06-18 client.

- Observation: the strictness comes from the ACP binding, not from MCP. MCP 2026-07-28 itself allows a "dual-era" server that answers a legacy `initialize` and serves modern per-request calls on the same endpoint ("Legacy | Dual-era | Works"), and it tells dual-era clients how to detect a server's era over Streamable HTTP. The ACP RFD deliberately narrows its own binding to 2026-07-28 and says the HTTP re-export "must not add a legacy fallback"; its "Native-tool re-export" section describes the same loopback, bearer-per-server endpoint this adapter would build.
  Evidence: https://modelcontextprotocol.io/specification/2026-07-28/basic/lifecycle, section "Backward Compatibility with Initialization-Based Versions" and its compatibility matrix; the RFD on `main` (`docs/rfds/mcp-over-acp.mdx`), sections "Proxying and HTTP adaptation" and "Native-tool re-export".

- Observation: no Muse setting switches its MCP client to the modern revision, and Muse's own ACP agent does not offer MCP-over-ACP either.
  Evidence: with an MCP server configured in Muse's `settings.json` as `{"type":"http","url":...,"protocolVersion":"2026-07-28"}` (no ACP-supplied server), Muse 1.4.23-R8977.1 still sent only `initialize` with `mcp-protocol-version: 2025-06-18` to a modern-only server and ran the turn without the tool. `muse mcp add-json` accepts any unknown key (a `bogusKey` entry was saved too), so the key being accepted proves nothing. `muse acp` answers ACP v1 `initialize` with `"mcpCapabilities":{"http":true,"sse":false}`, with no `acp` member.


## Decision Log


- Decision: stop before implementation and ask the maintainer to choose an option.
  Rationale: the issue and the task describe relaying each Muse MCP message, `initialize` included, through `mcp/message`. That cannot work with a conforming provider: the binding has no `initialize`, and the SDK's own provider answers it with an inner `-32601` and answers Muse's unversioned `tools/list` with an inner `-32602`, after which Muse drops the server. The only design that would work with today's Muse is a gateway that speaks legacy MCP to Muse and modern MCP to the editor, which is the "legacy fallback" the ACP design says such an adapter must not add. Choosing to deviate from the protocol design, or to ship nothing usable, is the maintainer's call, and the task's ground rules say to stop and report rather than narrow the scope or hand-roll around a protocol gap.
  Date/Author: 2026-11-17, Claude.

- Decision: reject the verbatim relay (forward every HTTP JSON-RPC message from Muse unchanged as `mcp/message`, including `initialize`).
  Rationale: it advertises a capability that fails with every conforming provider, including the SDK's reference provider, and the RFD says an intermediary advertises the capability only when its chain can consume the transport. A test that only shows `initialize` round-tripping through a scripted client would prove a behavior the protocol forbids.
  Date/Author: 2026-11-17, Claude.

- Decision: do not advertise `mcpCapabilities.acp` (v1) or `session.mcp.acp` (v2) until one of the options below is approved and implemented.
  Rationale: an agent that advertises the capability must accept `acp` declarations, and a client may start calling into its server as soon as it publishes the declaration. Without a working path the adapter would accept a server that Muse can never use.
  Date/Author: 2026-11-17, Claude.

- Decision (proposed, needs approval): the options, in the order recommended.

  Option B, conforming re-export once Muse can consume it. Ask the Muse team for a session MCP client that speaks MCP 2026-07-28 over streamable HTTP (or is dual-era: tries a modern request and falls back), or for a native MSP session-MCP transport the adapter can feed with `mcp/message` traffic. Then implement the plan below without the legacy layer, exactly as the RFD and the upstream polyfill describe, gated on the Muse version or MSP grant that brings it. Nothing ships for #317 until then; the issue stays open and is labelled blocked upstream. This is the only option that follows the ACP design to the letter.

  Option A, legacy gateway now, as a recorded deviation. The adapter's loopback bridge acts as a dual-era MCP server toward Muse: it answers `initialize`, `notifications/initialized`, and `ping` itself, and turns each other request into one stateless `mcp/message` with a fresh `requestId` and the modern `_meta` filled in (`protocolVersion` "2026-07-28", `clientCapabilities` `{}`, `clientInfo` copied from Muse's `initialize`). Results are passed back with the modern-only parts projected away; an `input_required` result (the modern multi-round-trip pattern, which a legacy client cannot follow) becomes an MCP tool error the model can read. The ACP wire stays conforming; the deviation is inside the agent. It works with every Muse that grants `sessionMcp` (1.3.0 and later), but it contradicts "the adapter must not add a legacy fallback", it relies on how revision 2026-07-28 results differ from 2025-06-18 ones (which this plan would have to specify method by method), and the binding is unstable and may change under it. It needs the maintainer's explicit approval and a paragraph in `docs/acp-extensions.md`.

  Option C, close or park #317 as not applicable to a legacy MCP host, with this plan as the record.

  Rationale: see the Surprises above. Option B costs nothing now and avoids building on a deviation; option A delivers the feature at the price of a documented divergence from the protocol design.
  Date/Author: 2026-11-17, Claude.

- Decision: option B. Stay on the ACP design; ship nothing for #317 until Muse's session MCP client speaks MCP 2026-07-28 (modern, or dual-era with era detection). Do not build the legacy gateway.
  Rationale: the maintainer chose to stay on spec after the re-check against the newest SDK, RFD, and Muse releases. The trigger to resume is a Muse build whose session MCP client, pointed at a modern-only server, sends a request with `params._meta["io.modelcontextprotocol/protocolVersion"]` (or `server/discover`) instead of `initialize`.
  Date/Author: 2026-11-17, maintainer decision recorded by Claude.


## Outcomes & Retrospective


Nothing has shipped. The research establishes the full MCP-over-ACP wire contract for ACP v1 and v2, shows that SDK 5.3.0 can carry it, and shows why the issue's relay design cannot meet its acceptance against a conforming client with the Muse MCP client as it is today. The lesson for similar issues: a gap check against the ACP schema alone misses constraints that live in the RFD and in the SDK's reference implementation, so both should be read before a design is proposed.


## Context and Orientation


`muse-acp` is a Rust binary (crate root `src/main.rs`) that an editor launches. It speaks ACP to the editor through the official Rust SDK (`agent-client-protocol` pinned `=5.3.0` in `Cargo.toml`, schema crate `agent-client-protocol-schema` 1.18.0), and MSP, a JSON-RPC protocol, to a `muse serve` child process (the "host"). The SDK features enabled today are listed in `Cargo.toml`; `unstable_mcp_over_acp` is not among them.

How client MCP servers reach Muse today: `src/request.rs` turns the SDK's typed `McpServer` (v1 by `v1_mcp`, v2 by `v2_mcp`) into the adapter's own `request::McpServer` enum, with an `Other` arm for transports the adapter does not know. With the SDK feature turned on, the catch-all arm of each mapper would turn an `acp` entry into `Other`, which `translate` drops with a log line. `src/mcp.rs` `translate` turns stdio and HTTP entries into MSP `config.mcpServers` JSON with `"mode":"optional"` and drops everything else with a log line that never contains commands, URLs, environment values, or headers. `client_mcp_servers` and `session_mcp_servers` in `src/main.rs` apply it on `session/new`, `session/load`, `session/resume`, and `session/fork`, only when the host granted the MSP capability `sessionMcp`; `session_config_field` writes the `config` member. Session MCP configuration is not durable in Muse, so the adapter stores it on `AcpSession::mcp_servers` (in `src/acp.rs`) and sends it again when it reattaches sessions to a restarted host. A loaded Muse session refuses a different MCP set (`session_configuration_conflict`), and the adapter then retries the resume without `config`. `.agents/plans/session-mcp-forwarding.md` records all of that.

The capabilities the adapter advertises are built by `v1_init` and `v2_init` in `src/main.rs`. Both take `session_mcp: bool` (the host's grant): v1 writes `mcpCapabilities.http` from it, v2 adds `session.mcp` with `stdio` and `http` only when it is true.

The loopback MCP server: `src/bridge.rs` runs an HTTP/1.1 server on 127.0.0.1 inside the adapter process, started on first use by `bridge::start`. Muse reaches it through a `streamableHttp` session MCP entry named `editor` whose `Authorization: Bearer <token>` header carries a 256-bit random token per ACP session (`bridge::session_token`, `bridge::bind`). It refuses any request with an `Origin` header (browsers send one, Muse does not), checks the route and token from the headers before reading a body, caps headers at 64 KiB and bodies at 64 MiB, serves at most 32 connections with a 60-second idle timeout, answers `GET` and `DELETE` with 405, and answers each POSTed JSON-RPC request with a plain `application/json` body. It implements `initialize` (echoing one of `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05`), `ping`, `tools/list`, and `tools/call` for the editor's file and terminal tools, which call the editor with `fs/*` and `terminal/*` requests through `Editor::request_blocking`.

The editor connection: `src/editor.rs` runs the SDK connection on its own thread. `Editor::request_blocking(request, timeout, abort)` sends any typed request, waits for the answer, and withdraws it with `$/cancel_request` on timeout or when `abort` returns true. Incoming client requests and notifications reach the main loop as `Incoming::V1` or `Incoming::V2`.

Terms. MCP "legacy" revisions (2025-11-25 and earlier) start with an `initialize` request that fixes the protocol version for a connection or HTTP session. The "modern" revision 2026-07-28 has no handshake: each request names its revision and the caller's capabilities in `params._meta`, and an HTTP request also sends the `MCP-Protocol-Version` header. A "dual-era" server answers both kinds. "MRTR" (multi round-trip request) is the modern pattern in which a tool answers `resultType: "input_required"` and the caller retries the call with the requested input; legacy clients cannot do this.


## Plan of Work


This section describes the implementation for option B, and notes where option A differs. Nothing here should be started before the maintainer picks an option.

Milestone 1 turns on the SDK feature and parses declarations. Add `"unstable_mcp_over_acp"` to the `agent-client-protocol` features in `Cargo.toml`. In `src/request.rs`, map `v1::schema::McpServer::Acp` and `v2::schema::McpServer::Acp` to a new arm `request::McpServer::Acp { name, server_id }`, and teach `McpServer::name` about it. Unit tests in `src/request.rs` parse `{"type":"acp","name":"t","serverId":"s"}` under both versions.

Milestone 2 adds the relay route to the bridge. In `src/bridge.rs`, add a route per declared server, `/mcp/acp/<route key>`, where the route key is a non-secret encoding of the `serverId` (hex of its UTF-8 bytes keeps it URL-safe), authenticated by the same per-session bearer token as the `editor` route, so the token also names the ACP session. The existing admission checks apply unchanged and run before any body is read. A route whose server the session did not declare answers 404. Each admitted POST that holds a JSON-RPC request becomes one `mcp/message` request through `Editor::request_blocking`, built as `v1::schema::MessageMcpRequest` or `v2::schema::MessageMcpRequest` for the connection's ACP version, with a fresh UUID-like `requestId`, the inner `method`, and the inner `params`. Under option B the route requires the modern headers and `_meta` and rejects `initialize`, batches, and client-sent responses, as the polyfill guide describes. Under option A the route instead answers `initialize`, `notifications/initialized`, and `ping` locally, adds the modern `_meta` keys to every other request, and projects modern-only result fields away. The inner outcome becomes the HTTP JSON-RPC response with Muse's own request id: `{"result":...}` passes through, `{"error":...}` becomes a JSON-RPC error with the same code, message, and data. Outer ACP errors map to a JSON-RPC error that says the editor's server failed (`-33001` unavailable, `-33002` backend failure, `-33000` resource limit, `-32800` cancelled), never reusing the inner code space. While a request is open, `mcp/message` notifications from the client with the same `serverId` and `requestId` are collected by a new `bridge::notify` that the main loop calls for `ClientNotification::MessageMcpNotification`; when any arrive, the response is sent as a `text/event-stream` with the notifications first and the result last, and late or unknown notifications are dropped. If Muse closes the connection before the answer, the request is withdrawn with `$/cancel_request`; `session/cancel` withdraws every open relay request of that session through `bridge::cancel_session`. Bodies and notifications are bounded like the existing bridge (64 MiB request, a fixed number of buffered notifications per request) and an overflow cancels only that request.

Milestone 3 wires the session lifecycle. `src/mcp.rs` `translate` stops dropping `Acp` entries when the bridge can serve them and instead emits `{"transport":"streamableHttp","url":"http://127.0.0.1:PORT/mcp/acp/<key>","headers":{"Authorization":"Bearer TOKEN"},"mode":"optional"}` under the declared name; the URL never carries the token, and log lines name only the server. `session_mcp_servers` in `src/main.rs` starts the bridge when any `acp` entry is present and records each session's declared `serverId` set next to its token, for `session/new`, `session/load`, `session/resume`, and `session/fork`, and for reattachment after a host restart through the stored `AcpSession::mcp_servers`. Because the token is reused for a session, the entry text stays byte-identical on reload, which avoids Muse's configuration conflict.

Milestone 4 advertises the capability. `v1_init` sets `mcpCapabilities.acp` and `v2_init` adds `session.mcp.acp: {}` only when the host granted `sessionMcp`, the bridge can bind, and (option B only) the host's MCP client is modern. An `acp` declaration that arrives without the capability is refused as ACP requires (`-32602` for v2 transports the agent does not support) or dropped with a log line in v1, matching the existing handling of unknown transports.

Milestone 5 adds tests and documentation. In `tests/fixtures/fake_serve.py`, a scenario plays Muse's MCP client against the bridge route and logs the exchanges; in `tests/acp_serve.rs`, a scripted ACP client answers `mcp/message` as a conforming provider (it rejects `initialize` and unversioned requests exactly as `validate_modern_request` does), so the tests prove that `tools/list` and `tools/call` round-trip and that the capability is absent without `sessionMcp`. Rejection tests cover a missing or wrong token, an `Origin` header, an undeclared server route, an outer `-33001`, an inner MCP error, cancellation on `session/cancel`, and a v2 client. A live test in `tests/live_loopback.rs` (option A only, or option B once a modern Muse exists) scripts the model to call `mcp__<name>__<tool>` and answers the resulting `mcp/message` from the test. README ("Client-provided MCP servers"), `docs/acp-extensions.md`, and ROADMAP section 7 describe the transport, the gating, and, for option A, the deviation.


## Concrete Steps


From the repository root (the worktree directory). The research steps, which can be repeated safely:

    gh issue view 317 --repo BrokkAi/muse-acp
    cargo check --locked -p muse-acp --features agent-client-protocol/unstable_mcp_over_acp
    curl -sSfL -o /tmp/acp-spec/rfd.md https://agentclientprotocol.com/rfds/mcp-over-acp.md
    curl -sSfL -o /tmp/acp-spec/v1.unstable.json https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/main/schema/v1/schema.unstable.json
    curl -sSfL -o /tmp/acp-spec/v2.unstable.json https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/main/schema/v2/schema.unstable.json
    curl -sSfL -o /tmp/acp-spec/lifecycle.md https://modelcontextprotocol.io/specification/2026-07-28/basic/lifecycle.md

The Muse probe: build the adapter (`cargo build --locked`), start a scratch HTTP server on 127.0.0.1 that appends every POST body and its headers (minus `Authorization`) to a log and answers as an MCP server, then drive `target/debug/muse-acp` the way `tests/live_loopback.rs` does (throwaway `XDG_CONFIG_HOME` with a loopback model provider from `tests/fixtures/loopback_provider.py`, `MUSE_NO_AUTO_UPDATE=1`) with `session/new` carrying `{"type":"http","name":"probe","url":"http://127.0.0.1:PORT/mcp","headers":[]}` and a scripted call to `mcp__probe__echo`. Expect the transcripts in Surprises & Discoveries.

Once an option is chosen, the gates are the repository's usual ones:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2


## Validation and Acceptance


The acceptance for #317 is behavioural: a client that hosts an `acp` MCP server sees Muse list and call its tools. Concretely, a fake-host test starts a session whose `mcpServers` holds `{"type":"acp","name":"proj","serverId":"proj:1"}`, the fake Muse lists and calls the server's tool through the bridge, and the scripted client receives `mcp/message` requests with `serverId` "proj:1", a fresh `requestId` each, method `tools/list` and then `tools/call`, and modern `_meta`; its answers reach the fake Muse unchanged, and the call's text appears on the ACP tool card. The same test with `FAKE_NO_SESSION_MCP=1` sees no `acp` capability and no relay. Under option B this acceptance also requires a Muse build whose session MCP client is modern; none exists as of 1.4.23.


## Idempotence and Recovery


The research steps write only to `/tmp` and to the throwaway probe directories, which can be deleted. No repository file other than this plan has been changed. Every proposed milestone is additive and can be reverted by reverting its commit.


## Artifacts and Notes


The probe scripts were kept outside the repository. The essential transcripts are in Surprises & Discoveries. The RFD's request example, for reference:

    {"jsonrpc":"2.0","id":20,"method":"mcp/message","params":{"serverId":"project-tools:7a72",
      "requestId":"mcp-request:a11f","method":"tools/call","params":{"name":"echo",
      "arguments":{"message":"hello"},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
      "io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"example-agent","version":"1"},
      "progressToken":"progress-1"}}}}
    {"jsonrpc":"2.0","id":20,"result":{"result":{"resultType":"complete","content":[{"type":"text","text":"hello"}],"isError":false}}}


## Interfaces and Dependencies


No new crates. The SDK feature `unstable_mcp_over_acp` on `agent-client-protocol` `=5.3.0` provides `v1::schema::{McpServerAcp, McpServerAcpId, MessageMcpRequest, MessageMcpResponse, MessageMcpNotification, McpError, McpRequestId}`, the v2 equivalents, `v1::schema::McpCapabilities::acp(bool)`, and `v2::schema::McpCapabilities::acp(McpAcpCapabilities)`. The adapter would add `request::McpServer::Acp { name: String, server_id: String }`, `bridge::notify(acp_sid: &str, notification: &J)`, and a relay route in `src/bridge.rs`; the existing `bridge::cancel_session` and `Editor::request_blocking` are reused.


Revision note (2026-11-17): created after the spec research and the Muse probe; records why implementation stopped and the options for the maintainer.
