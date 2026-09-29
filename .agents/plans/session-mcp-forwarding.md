# Forward editor MCP servers to Muse through typed session MCP

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


An editor that speaks the Agent Client Protocol (ACP), such as Zed or a JetBrains IDE, can hand the agent a list of MCP servers when it opens a session. MCP (Model Context Protocol) servers are small helper programs, or HTTP endpoints, that offer extra tools to a model. Zed passes the context servers the user configured in Zed; JetBrains passes its built-in IDE server. Before this change `muse-acp` dropped that list and logged `ignoring client-provided MCP servers`, because the Muse host had no way to accept per-session MCP configuration.

Muse's session protocol (MSP) 1.3.0 added one: `session/start` and `session/resume` accept `config.mcpServers`, a typed map of server name to server description, and a connection must be granted the `sessionMcp` capability to use it. After this change, the editor's stdio and HTTP MCP servers become tools inside the Muse session. The model can call them, every call goes through Muse's normal approval flow, and the editor shows the calls like any other tool call. The adapter advertises the HTTP transport to ACP clients only when the host granted `sessionMcp`, and an older host keeps the previous behavior: the servers are dropped and a log line says why.

To see it working, run `cargo test --locked --test acp_serve mcp` and observe the new session MCP tests pass. With a live Muse 1.4 host, configure a context server in Zed, open a Muse thread, and ask the model to use one of that server's tools. The editor shows a permission prompt for a tool named `mcp__<server>__<tool>`, and after approval the tool's output appears in the thread.


## Progress


- [x] (2026-09-29 09:45Z) Probed a live `muse serve` 1.4.1 for the `sessionMcp` grant, start and resume behavior, failure modes, forks, and the approval flow (see Surprises & Discoveries).
- [x] (2026-09-29 10:05Z) Wrote this plan.
- [x] (2026-09-29 10:15Z) Milestone 1: `MspHost::launch` requests `sessionMcp`; `HandshakeInfo::session_mcp` records the grant; `v1_init`/`v2_init` advertise HTTP MCP only with it; startup logs whether forwarding is available.
- [x] (2026-09-29 10:25Z) Milestone 2: `src/mcp.rs` with `translate` and seven unit tests.
- [x] (2026-09-29 10:40Z) Milestone 3: `client_mcp_servers`, `mcp_config_field`, and `resume_session` in `src/main.rs`; `session/new`, `session/load`/`session/resume`, `session/fork`, and `restart_durable_host` wired; `AcpSession::mcp_servers` added.
- [x] (2026-09-29 10:55Z) Milestone 4: fake host grants `sessionMcp`, refuses ungranted configs, and simulates the conflict; seven MCP integration tests pass (six new, one existing load test).
- [x] (2026-09-29 11:05Z) Milestone 5: README section and feature bullet, ROADMAP §7, CHANGELOG. Full contributor gate green.
- [x] (2026-09-29 11:10Z) Live acceptance through the built adapter against Muse 1.4.1 (see Artifacts and Notes).


## Surprises & Discoveries


- Observation: the live host grants `sessionMcp` whenever it is requested, and without the grant any non-empty `mcpServers` fails `session/start` with `capabilityRequired`. An empty map is accepted without the grant.
  Evidence (Muse 1.4.1, `muse serve`):

      initialize requestedCapabilities ["sessionMcp","sessionListStream"]
        -> grantedCapabilities ["sessionMcp","sessionListStream"]
      session/start config.mcpServers {probe: stdio}, capability not requested
        -> {"code":-32010,"data":{"capability":"sessionMcp","kind":"capabilityRequired"},
            "message":"session MCP configuration requires the sessionMcp capability"}

- Observation: the host starts each stdio server when the session runtime is built (at `session/start`, or at a `session/resume` that loads the session), runs it in the session's workspace root, and merges the supplied `env` into its environment. Tool calls reach the model as `mcp__<server>__<tool>`, go through a normal `approval/request`, and complete as ordinary `toolCall` items.
  Evidence: the probe MCP server logged `initialize`, `notifications/initialized`, and `tools/list` right after `session/start`. A turn asking for the tool produced:

      approval/request subject {"kind":"tool","toolName":"mcp__probe__secret_word"}
        availableChoices: allow_once, allow_session,
          allow_local_mcp_tool (decision approvedPolicyAmendment, scope localPersistent,
            label "Always allow this MCP tool"), abort
      item/completed {"kind":"toolCall","tool":"mcp__probe__secret_word",
        "visibleOutput":"the secret word is pineapple cwd=<workspace root>"}

- Observation: session MCP configuration is not durable. A cold `session/resume` (the session is not loaded in this host process) without `config` starts no MCP servers; the same resume with `config` starts them.
  Evidence: in a fresh host process, `session/resume` of a session originally started with a server spawned nothing; adding `config` spawned the server once.

- Observation: once a session is loaded, its MCP set is fixed. Resuming it with a different non-empty map fails with `commandRejected` reason `session_configuration_conflict`; resuming with an identical map, an empty map, or no `config` succeeds and keeps the running servers.
  Evidence:

      {"code":-32030,"data":{"kind":"commandRejected","reason":"session_configuration_conflict"},
       "message":"command ... rejected: session MCP configuration conflicts with the loaded session runtime"}

  This matters for the adapter: ACP `session/close` only drops the adapter's local state (MSP has no unload command), so an editor that closes a thread and later reloads it with changed MCP settings would hit this rejection on the same host.

- Observation: `session/fork` takes no `config`, and the forked session is loaded immediately with no MCP servers. Resuming the fork with any non-empty map, even the parent's, is rejected as a configuration conflict until the host process restarts.
  Evidence: `resume fork with parent config` returned `session_configuration_conflict`; the probe server was not spawned by the fork.

- Observation: a `required` server that cannot start does not fail `session/start`; it fails every turn instead. An `optional` server that cannot start is skipped silently and turns run normally.
  Evidence:

      turn/completed {"terminal":"failed","error":{"kind":"configError","message":
        "invalid run configuration: Required MCP server `bad` failed during startup:
         the configured command is unavailable."}}

- Observation: the host rejects the whole `session/start` with `invalidParams` for an empty server name or an unknown `transport`. It accepts names with spaces, dots, and slashes, relative commands, and extra unknown fields.
  Evidence: `empty-name` and `unknown-transport` returned `invalid session/start config: mcpServers does not match the supported shape`; the other probes returned a session.


## Decision Log


- Decision: forward only when the host granted `sessionMcp`, and otherwise keep the old drop-and-log behavior with a message that names the missing grant.
  Rationale: the grant is fixed for the MSP connection, and without it a non-empty map fails `session/start` outright. Older hosts (1.2.x) do not know the capability, so they will not grant it.
  Date/Author: 2026-09-29, Claude.

- Decision: send every client server with `"mode":"optional"`.
  Rationale: ACP has no notion of required servers, and editors pass every configured server, including ones the user never uses with Muse. With the host default (`required`), one broken server makes every turn fail with `configError`, which would disable the agent for a reason unrelated to the user's request. With `optional`, the working servers still load and the agent keeps working.
  Date/Author: 2026-09-29, Claude.

- Decision: translate ACP `stdio` entries (v1 untagged or `"type":"stdio"`) to MSP `transport: "stdio"` and ACP `"type":"http"` to MSP `transport: "streamableHttp"`. Drop `sse` and unknown types with a log line. Drop malformed entries (missing name, command, or URL; non-string arguments, environment variables, or headers) with a log line instead of failing the session. The first entry with a given name wins.
  Rationale: MSP has no SSE transport, and the host rejects the whole session for one malformed entry. Failing the editor's session because of one bad server entry would be worse than running without that server. MSP keys servers by name, so duplicates cannot both be sent.
  Date/Author: 2026-09-29, Claude.

- Decision: never log `command`, `args`, `env`, `url`, or `headers` values. Log only server names, transports, and counts.
  Rationale: editors put tokens in environment variables and headers. The host's own diagnostics avoid echoing commands and URLs for the same reason.
  Date/Author: 2026-09-29, Claude.

- Decision: when `session/resume` with `config` fails with `session_configuration_conflict`, retry the resume once without `config` and log that the loaded session keeps the MCP servers it was loaded with until muse-acp restarts.
  Rationale: the conflict only happens when this adapter's host already has the session loaded. That happens after an ACP `session/close` followed by a reload with changed settings, or for a fork. Failing the reload would lock the user out of their thread; attaching to the running session keeps it usable, and the log explains why the server set did not change.
  Date/Author: 2026-09-29, Claude.

- Decision: store the translated config on the adapter's session record (`AcpSession::mcp_servers`) and re-send it when the adapter re-attaches sessions to a restarted host.
  Rationale: session MCP configuration is not durable (see Surprises), so a host restart would otherwise remove the tools without any sign.
  Date/Author: 2026-09-29, Claude.

- Decision (revised during Milestone 3): the record stores the client's latest requested set, even after a conflict kept the running set or when the session is a fork. The plan originally kept the previously loaded set on a conflict.
  Rationale: the stored set is only used when a restarted host loads the session cold, where no conflict is possible. The editor's current servers are then the right ones to load. This also lets a fork get its servers after a restart, and it avoids tracking host-loaded state for sessions the adapter closed.
  Date/Author: 2026-09-29, Claude.

- Decision: do not attempt to give forks MCP servers in the same host process. Log it when the client supplied servers with `session/fork`.
  Rationale: MSP `session/fork` has no `config`, and a later resume with `config` is rejected (see Surprises). The fork gains the servers the next time it is loaded after a restart, through the normal `session/load` path.
  Date/Author: 2026-09-29, Claude.

- Decision: keep offering the host's "Always allow this MCP tool" choice unchanged.
  Rationale: the adapter already shows every host choice with its rule preview and scope, for example `Always allow this MCP tool (host rule preview: tool mcp__probe__secret_word; host scope: localPersistent)`. The user makes the choice explicitly, and the host applies the same policy to MCP servers configured in Muse itself. The README notes that the rule is keyed by the server name and tool name.
  Date/Author: 2026-09-29, Claude.


## Outcomes & Retrospective


Editor MCP servers now reach Muse sessions on hosts that grant `sessionMcp`, and the live end-to-end run proved the purpose: an ACP client attached a stdio server, the editor received a permission prompt for `mcp__probe__secret_word`, and the tool's output reached the thread. Hosts without the grant keep the previous behavior. The remaining gap is the host's: a loaded session or a fork cannot change its MCP set until the host process restarts, so the adapter logs this case and documents it rather than failing. The live Muse 1.4.1 schema fingerprint (`sha256:e0e163db...`) is not yet recorded in `src/compat.rs`, so the adapter reported it as `status=unknown`; a follow-up change recorded it in the compatibility table as tested.


## Context and Orientation


`muse-acp` is a single Rust binary with no runtime dependencies. An editor launches it and talks ACP over stdin and stdout as newline-delimited JSON-RPC 2.0. The adapter launches `muse serve` as a child process (the "host") and talks MSP to it over the child's stdin and stdout. Most request handling is in `src/main.rs`; the MSP connection is `src/msp.rs`; ACP helpers and the per-session record `AcpSession` are in `src/acp.rs`; a small JSON value type `J` with `get`, `as_str`, and friends is in `src/json.rs` (use `crate::json::esc` to quote a string as JSON).

The startup order matters here. The adapter waits for the editor's ACP `initialize`, derives the client posture from it (`negotiate_acp` in `src/main.rs`), launches the host with `MspHost::launch` in `src/msp.rs` (which sends MSP `initialize` with `requestedCapabilities` and records the response in `HandshakeInfo`), and only then answers ACP `initialize` through `send_initialize`. The host grant is therefore known when the ACP capabilities are written. If the host cannot start, `serve_without_host` answers `initialize` without a host.

ACP session requests map to MSP as follows. `session/new` becomes MSP `session/start`. `session/load` and `session/resume` both become MSP `session/resume`; they share one arm in `handle_acp`. `session/fork` becomes MSP `session/fork`. `session/close` drops local state only. When the host process dies, `restart_durable_host` launches a new one and sends `session/resume` for every known session.

The ACP `mcpServers` field is an array on `session/new`, `session/load`, `session/resume`, and `session/fork`. In ACP v1 a stdio entry has no `type` field and looks like `{"name":"x","command":"/abs/bin","args":["a"],"env":[{"name":"K","value":"V"}]}`. HTTP and SSE entries carry `"type":"http"` or `"type":"sse"` with `name`, `url`, and `headers` (an array of `{name, value}`). In ACP v2 every entry is tagged, `"type":"stdio"` or `"type":"http"`. The agent advertises transports in `initialize`. In v1 this is `agentCapabilities.mcpCapabilities` with booleans `http` and `sse`; stdio is always required. In v2 it is `capabilities.session.mcp` with objects `stdio: {}` and `http: {}`; a transport is supported only if advertised.

The MSP side is `config.mcpServers` on `session/start` and `session/resume`: an object keyed by server name whose values are either `{"transport":"stdio","command":..,"args":[..],"env":{..},"mode":..}` or `{"transport":"streamableHttp","url":..,"headers":{..},"mode":..}`. `mode` is `required` (default) or `optional`. The vendored schema is `tests/protocol/stable/msp.schema.json` (`$defs.SessionConfig`, `$defs.SessionMcpServerConfig`).

Tests: `tests/acp_serve.rs` drives the real adapter binary against `tests/fixtures/fake_serve.py`, a scripted fake host. The fake appends every adapter-to-host frame to `<FAKE_LOG>.frames`, which tests read to check what crossed the MSP boundary. `Client::spawn(scenario, env)` starts a test, and `c.req`, `c.wait_for`, `c.wait_stderr`, and `c.wait_log` drive it. Its MSP `initialize` answer grants capabilities per scenario in `result_for`.


## Plan of Work


Milestone 1 (handshake and ACP capabilities). In `src/msp.rs`, add `"sessionMcp"` to `requestedCapabilities` in `MspHost::launch`, and add `pub session_mcp: bool` to `HandshakeInfo`, set from `grantedCapabilities`. In `src/main.rs`, change `v1_init` and `v2_init` to take `session_mcp: bool`. v1 writes `"mcpCapabilities":{"http":<session_mcp>,"sse":false}`. v2 adds `"mcp":{"stdio":{},"http":{}}` inside `capabilities.session` when granted and omits it otherwise. `send_initialize` takes the flag; `handle_acp` passes `host.handshake().session_mcp`, and `serve_without_host` passes `false`. Log once at startup whether session MCP forwarding is available.

Milestone 2 (translation). Create `src/mcp.rs` with:

    pub struct Translation {
        /// JSON text of the MSP `mcpServers` object, or None when nothing is forwarded.
        pub servers: Option<String>,
        /// Names of the servers that were translated, in order.
        pub forwarded: Vec<String>,
        /// One human-readable line per dropped entry (never contains values).
        pub dropped: Vec<String>,
    }
    pub fn translate(params: Option<&J>) -> Translation

`translate` reads `params.mcpServers`. When it is missing, not an array, or empty, it returns an empty translation. Each entry follows the translation decision above. The output object keeps input order, and each value gets `"mode":"optional"`. Add unit tests in the same file covering: a v1 untagged stdio entry with args and env; a v2 tagged stdio entry; http with headers; sse dropped; unknown type dropped; missing name, command, or URL dropped; a non-string arg dropping the entry; duplicate names; empty input; and a check that no value (env value, header value, URL, command) appears in any `dropped` line. Register the module in `src/main.rs` with `mod mcp;`.

Milestone 3 (lifecycle). In `src/acp.rs`, add `pub mcp_servers: Option<String>` to `AcpSession` and initialize it everywhere an `AcpSession` is built (`rg "AcpSession \{" src`). In `src/main.rs`, replace `ignore_client_mcp_servers` with `client_mcp_config(host, params, context) -> Option<String>`. It runs `translate`, logs each dropped line, and returns `servers` only when the host granted `sessionMcp`. If the host did not grant it and the client sent servers, it logs `ignoring client-provided MCP servers: this Muse host did not grant sessionMcp`. When it forwards, it logs `forwarding N client MCP server(s) to Muse: a, b`.

`validate_session_roots` stops calling the MCP helper; the `session/new` arm computes the config after validation and adds `,"config":{"mcpServers":...}` to the `session/start` params when present. The new session record stores it.

The `session/resume`/`session/load` arm does the same for MSP `session/resume`. If the command fails with `commandRejected` whose `data.reason` is `session_configuration_conflict`, it logs the conflict and re-sends the same resume once without `config`, with a fresh command id. The conflict retry lives in `resume_session(host, msp_sid, mcp_servers) -> Result<J, J>`, which `restart_durable_host` also uses. The record stores the config the client just requested, even after a conflict (see the revised decision in the Decision Log). An existing record's `mcp_servers` is updated in place (the arm uses `entry().or_insert_with`, so set the field after the insert).

The `session/fork` arm logs `not applying N client MCP server(s) to the forked session until Muse loads it again (MSP session/fork takes no configuration)` when the client supplied servers, and the fork's record stores the requested set so a later load or restart applies it.

`restart_durable_host` includes each session's stored `mcp_servers` in its re-attach `session/resume`, if the new host granted `sessionMcp`. If the new host did not grant it, it logs that the session lost its client MCP servers.

To detect the conflict, add a helper in `src/msp.rs` or `src/main.rs` that inspects the error `J` returned by `host.command` for `data.kind == "commandRejected"` and `data.reason == "session_configuration_conflict"`. Check how `err_message` and `msp::acp_error_code` read that error value.

Milestone 4 (fake host and integration tests). In `tests/fixtures/fake_serve.py`, grant `sessionMcp` when requested unless `FAKE_NO_SESSION_MCP=1`. When `FAKE_MCP_CONFLICT=1`, answer the first `session/resume` that carries a non-empty `config.mcpServers` with the live conflict error. Replace `v1_jetbrains_mcp_attachment_is_logged_and_not_forwarded` in `tests/acp_serve.rs` with tests that:

- a v1 JetBrains-style stdio entry reaches `session/start` as `config.mcpServers.intellij` with `transport: "stdio"`, `command`, `args`, and `mode: "optional"`, and the v1 initialize advertises `"http":true`;
- a v2 client sees `"mcp":{"stdio":{},"http":{}}` and its tagged http entry reaches the host as `streamableHttp` with a headers object;
- with `FAKE_NO_SESSION_MCP=1`, v1 advertises `"http":false`, v2 omits `mcp`, nothing crosses the boundary, and stderr says the grant is missing;
- `session/load` forwards the config on `session/resume`, and a conflict retries without config and still loads;
- sse and malformed entries are dropped with stderr lines while valid entries still go through, and no env value appears in stderr;
- after a host restart (the `host_exit` scenario with `FAKE_RESTART_MARKER`), the re-attach `session/resume` carries the same config.

Also send one MCP server in `emitted_frames_conform_to_the_vendored_schema`'s session so the emitted `session/start` includes `config`, and verify the emitted `config.mcpServers` values against `$defs.SessionMcpServerConfig` (required fields and the `transport` const of the matching arm).

Milestone 5 (documentation). Rewrite the README section "Client-provided MCP servers" to describe forwarding, the transports, `optional` mode, approvals, the persistent "Always allow this MCP tool" choice, reload and fork limits, and the old-host fallback. Update ROADMAP §7 status and work items. Add a CHANGELOG `Unreleased` entry. Run the full contributor gate from `CONTRIBUTING.md`.


## Concrete Steps


Work from the repository root (the worktree directory). Build and test:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Focused runs while iterating:

    cargo test --locked mcp
    cargo test --locked --test acp_serve mcp


## Validation and Acceptance


Unit tests in `src/mcp.rs` prove the translation, including the rule that no secret value appears in a log line. Integration tests in `tests/acp_serve.rs` prove the wire behavior against the fake host: the initialize capabilities follow the grant, the config reaches `session/start` and `session/resume` in the MSP shape, the conflict fallback keeps load working, a restarted host gets the config again, and an ungranted host gets nothing. `emitted_frames_conform_to_the_vendored_schema` proves the emitted config matches the vendored MSP schema.

Live acceptance (optional, needs Muse 1.4 and a login): run the adapter from Zed or JetBrains with a context server configured, ask the model to use one of its tools, and observe a permission prompt naming `mcp__<server>__<tool>`, then the tool's output in the thread. `MUSE_LOG=debug` shows `forwarding 1 client MCP server(s) to Muse: <name>` in the adapter log.


## Idempotence and Recovery


All steps are additive source edits and tests; rerunning them is safe. No persistent state is written by the adapter. The live probes used a private copy of the Muse settings directory with `:ask-me` in place of `:auto-review` and did not change the user's saved settings.


## Artifacts and Notes


Live probe scripts are not part of the repository. The essential transcripts are quoted in Surprises & Discoveries.

Live acceptance through `target/debug/muse-acp` (ACP v1 client, Muse 1.4.1), a stdio server named `probe` whose tool returns an environment value:

    INIT mcpCapabilities {"http": true, "sse": false}
    PERMISSION mcp__probe__secret_word ['allow_once:allow_once', 'allow_session:allow_always',
      'allow_local_mcp_tool:allow_always', 'abort:reject_once']
    TOOL tool_call_update mcp__probe__secret_word completed [... "the secret word is mango ..."]
    AGENT mango
    [muse-acp] forwarding 1 client MCP server(s) to Muse: "probe"

Close and reload with a changed server set on the same adapter, then a cold load in a new adapter:

    reload with changed servers: True spawns +0
    [muse-acp] session 01a0eca3-... is already loaded with a different MCP server set; it keeps that set until muse-acp restarts
    cold load in a new adapter: True spawns +1


## Interfaces and Dependencies


No new dependencies. New module `src/mcp.rs` exposes `pub struct Translation` and `pub fn translate(params: Option<&J>) -> Translation` as described in Plan of Work. `HandshakeInfo` in `src/msp.rs` gains `pub session_mcp: bool`. `AcpSession` in `src/acp.rs` gains `pub mcp_servers: Option<String>`.

Revision note (2026-09-29): recorded milestone completion, the revised decision on which MCP set the session record stores, live acceptance evidence, and the outcome.
