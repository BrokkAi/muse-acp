# Let Muse read and write files through an ACP v1 editor

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


An ACP v1 editor, such as Zed, can offer the agent its own file system. When the editor's `initialize` request carries `clientCapabilities.fs.readTextFile: true`, the agent may send the editor `fs/read_text_file` and receive the editor's current text for a file, including changes the user has typed but not saved. With `fs.writeTextFile: true` the agent may send `fs/write_text_file`, and the editor writes the file through its buffers, so open tabs update and the editor tracks the change. ACP v2 removed both methods, so this is an ACP v1 feature only. GitHub issue #310 asks for it.

Before this change `muse-acp` never sent either request. Muse (the Muse Code host, `muse serve`) runs its own `read_file`, `write_file`, and `edit_file` tools inside its own process, and Muse's session protocol (MSP) has no way to hand those tools to the MSP client. So the model could only see what was saved on disk.

After this change the adapter gives Muse three extra tools that do go through the editor. The adapter runs a small MCP server inside its own process. MCP (Model Context Protocol) is the standard way to give a model extra tools: a server lists tools, and the model's host calls them. Since MSP 1.3.0, Muse accepts per-session MCP servers on `session/start` and `session/resume` (`config.mcpServers`, behind the `sessionMcp` capability), and `muse-acp` already forwards the editor's own MCP servers that way (`src/mcp.rs`). The new server is named `editor` and is reached over HTTP on 127.0.0.1. Its tools are `read_text_file`, `write_text_file`, and `edit_text_file`, which the model sees as `mcp__editor__read_text_file` and so on. Each call becomes an `fs/read_text_file` or `fs/write_text_file` request to the editor for that session.

To see it working, run `cargo test --locked --test acp_serve editor_bridge` and watch the new black-box tests pass. Then run `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback editor_bridge` against a real Muse host. That live test scripts the model to call `mcp__editor__read_text_file`, answers the adapter's `fs/read_text_file` with text that is not on disk, and checks the text reached the model.


## Progress


- [x] (2026-10-20 19:40Z) Prototype: drove a real Muse 1.4.11, 1.4.8, and 1.3.0 host through the built adapter with a throwaway Python HTTP MCP server registered as an editor MCP server (see Surprises & Discoveries).
- [x] (2026-10-20 19:55Z) Wrote this plan.
- [x] (2026-10-20 20:30Z) Milestone 1: `src/bridge.rs`, with the HTTP listener, MCP request handling, the three tools, path confinement, pending client requests, and 16 unit tests.
- [x] (2026-10-20 20:40Z) Milestone 2: the bridge is wired into `src/main.rs` (capability negotiation, `session_mcp_servers` for new/load/resume/fork, response routing, cancel, and diff cards on `item/completed`), and `src/fold.rs` gives the bridge tools locations.
- [x] (2026-10-20 21:10Z) Milestone 3: the `editor_bridge` fake-host scenario and 7 black-box tests in `tests/acp_serve.rs`, plus the live test `the_editor_file_tools_read_and_edit_through_the_acp_client` in `tests/live_loopback.rs`.
- [x] (2026-10-20 21:25Z) Milestone 4: README, ROADMAP section 7, and CHANGELOG are updated. The full gate is green (fmt, clippy `-D warnings`, 751 workspace tests, MSRV 1.88 build). The full live suite passes on 1.4.11 (50 tests), and the new live test passes on 1.3.0, 1.4.8, and 1.4.11.


## Surprises & Discoveries


- Observation: Muse connects to a streamable-HTTP MCP server on 127.0.0.1 that answers every POST with a plain `application/json` body (no SSE). It sends the configured headers, so a bearer token works for authentication, and it offers the server's tools to the model inside a namespace. The model calls them as `mcp__<server>__<tool>`.
  Evidence (Muse 1.4.11, server registered as `bridge`; the provider log lists the offered tools):

      POST /mcp/tok123 {"method":"initialize",...}  headers: authorization: Bearer sekrit,
           accept: application/json, text/event-stream, mcp-protocol-version: 2025-06-18
      POST notifications/initialized ; POST tools/list ; later POST tools/call
      offered: {"type":"namespace","name":"mcp__bridge","tools":[{"name":"read_text_file",...}]}

- Observation: Muse 1.4.11 sends `params._meta.callId` on `tools/call`, and that id is the model's call id, which the adapter already uses as the ACP `toolCallId`. Muse 1.4.8 and 1.3.0 send only `name` and `arguments`.
  Evidence: 1.4.11 `{"_meta":{"callId":"call_0","idempotencyKey":"tool:call_0","traceparent":...},"arguments":{...},"name":"read_text_file"}`; 1.4.8 `{"arguments":{...},"name":"read_text_file"}`.

- Observation: every MCP tool call needs approval in Muse's default approval mode, even a tool annotated `readOnlyHint: true`. The approval offers "Allow once", "Allow for this session", and "Always allow this MCP tool" (a persistent host rule for that server and tool name).

- Observation: in Read-only and Plan modes Muse still lets an MCP tool that writes run after approval, because `--disable-write --disable-shell` covers only Muse's own tools. The README already says so for editor MCP servers. The bridge must refuse writes itself in those modes.
  Evidence: with mode `readOnly` and `plan`, a call to `mcp__bridge__write_text_file` reached the MCP server after "Allow once".

- Observation: the same prototype works unchanged on Muse 1.4.8 and 1.3.0 (initialize, tools/list, tools/call; the call needs approval), so the bridge does not need a version gate beyond the `sessionMcp` grant.


## Decision Log


- Decision: expose the editor's file system to Muse as an adapter-owned MCP server instead of trying to change Muse's own tools.
  Rationale: MSP has no file-I/O delegation, but it loads per-session MCP servers, and the model can call those tools. This is also how other ACP agents that run their own tool loop reach `fs/*`.
  Date/Author: 2026-10-20 / Claude.

- Decision: serve MCP over HTTP from inside the adapter process (a `TcpListener` on 127.0.0.1, random port) rather than spawning a stdio helper process.
  Rationale: the tool handlers need the adapter's ACP connection (to send `fs/*` requests to the editor and read the answers) and its session table. An in-process server reaches both directly, while a stdio helper would need its own channel back to the adapter. The prototype showed Muse 1.3.0 through 1.4.11 accept JSON-only HTTP responses.
  Date/Author: 2026-10-20 / Claude.

- Decision: authenticate each session's server entry with its own random bearer token in the `Authorization` header, and refuse any request that carries an `Origin` header.
  Rationale: other local processes can reach a loopback port. The token is 256 random bits and maps to exactly one ACP session, so the token also tells the server which session a call belongs to. A browser always sends `Origin`, so refusing it blocks DNS-rebinding pages. Keeping the token out of the URL keeps it out of any URL logging.
  Date/Author: 2026-10-20 / Claude.

- Decision: keep one token per ACP session for the life of the adapter process, and reuse it when the session is loaded again.
  Rationale: a Muse host that already has a session loaded rejects a different MCP set as `session_configuration_conflict`. Reusing the token keeps the set byte-identical on reload.
  Date/Author: 2026-10-20 / Claude.

- Decision: offer the bridge only on ACP v1 connections whose client advertises `fs.readTextFile` or `fs.writeTextFile`, only when the host granted `sessionMcp`, and only when the client did not already supply its own MCP server named `editor`. `MUSE_EDITOR_TOOLS=0` turns it off.
  Rationale: ACP v2 has no `fs/*`, and ACP v1 forbids calling them without the capability. A client server keeps its name, so the adapter never shadows it.
  Date/Author: 2026-10-20 / Claude.

- Decision: `read_text_file` is offered with `fs.readTextFile`, `write_text_file` with `fs.writeTextFile`, and `edit_text_file` only with both.
  Rationale: an edit reads the editor's current text and writes the result back.
  Date/Author: 2026-10-20 / Claude.

- Decision: leave Muse's approval flow untouched. Do not auto-approve the bridge's read tool.
  Rationale: approvals belong to Muse's permission system, and the "Always allow this MCP tool" option already makes repeated reads quiet. The bridge adds its own checks on top: path confinement to the session's roots, and refusing writes in Read-only and Plan modes.
  Date/Author: 2026-10-20 / Claude.

- Decision: confine every path to the session's approved roots (`cwd` plus `additionalDirectories`), resolving symlinks through the deepest existing ancestor, and ignore `MUSE_ALLOW_UNSCOPED_READS`.
  Rationale: the bridge is a new way for the model to touch files. Keeping it strictly inside the workspace means it can never reach further than Muse's own sandboxed tools.
  Date/Author: 2026-10-20 / Claude.

- Decision: name the opt-out variable `MUSE_EDITOR_TOOLS`, not `MUSE_ACP_EDITOR_TOOLS`.
  Rationale: every other runtime setting the adapter reads is `MUSE_*` (`MUSE_LOG`, `MUSE_SHOW_REMINDERS`, `MUSE_ALLOW_UNSCOPED_READS`). Only the installer uses `MUSE_ACP_*`.
  Date/Author: 2026-10-20 / Claude.

- Decision: write and edit calls show a standard ACP `diff` (`path`, `oldText`, `newText`) on the completed tool card. The bridge records the diff, and the `item/completed` handler attaches it. It matches the card by `_meta.callId` when Muse sends one, and otherwise by session, tool name, and `path` argument, oldest first.
  Rationale: ACP v1 `tool_call_update.content` replaces the card's content, so a diff sent during the call would be lost when the completed update arrives. Attaching the diff to the completed update keeps it. Muse 1.4.8 and older do not send the call id, so they need the fallback match.
  Date/Author: 2026-10-20 / Claude.


## Outcomes & Retrospective


ACP v1 editors that advertise `fs` now get file reads and writes through the editor on Muse 1.3.0 and newer, which closes #310. The live acceptance on three Muse releases shows the model receiving an unsaved buffer that was never on disk, and an edit whose text goes to the editor (`fs/write_text_file`) and not to disk, with a diff on the card. Muse's own file tools remain, and the model chooses between them guided by the tool descriptions and the server's `instructions`. The same bridge can carry ACP v1 terminals (#311) as another tool, since the transport, tokens, client-request plumbing, cancel, and card content are all in place.

What remains open: AIR per-turn file-change reports do not yet include bridge writes, because they come from Muse's native tool items. Forks get the bridge only when Muse loads them again, as with every session MCP server.


## Context and Orientation


`muse-acp` is a Rust program with no external dependencies. It speaks ACP (Agent Client Protocol, JSON-RPC over stdin/stdout) to an editor, and MSP (Muse Session Protocol, JSON-RPC over the stdin/stdout of a `muse serve` child process) to Muse. The relevant files are:

`src/main.rs` holds the ACP request dispatcher `handle_acp` and the `initialize` capability negotiation `negotiate_acp`. Negotiated client capabilities are stored in process-wide atomics such as `NOTICES` and `AUTH_TERMINAL`. `handle_acp` is called on the main thread for every ACP line. A line without `method` is a client response to a request the adapter sent; today those go to `complete_permission`, `complete_feedback`, `complete_elicitation`, and friends, which match the response by its `id`. `client_mcp_servers(host, params, applied)` turns the client's `mcpServers` into MSP `config.mcpServers` JSON text, and `mcp_config_field` wraps it for `session/start` and `session/resume`. Its callers are `session/new`, `session/load` and `session/resume` (one shared handler), and `session/fork`. The resulting text is stored in `AcpSession::mcp_servers`, and every later reattach (host restart, mode switch onto the read-only host) re-sends that stored text. The `"item/completed"` arm of the MSP event handler folds a finished tool call into an ACP `tool_call_update`, and it can attach extra content through `s.fold.patch_diffs` (keyed by MSP item id).

`src/acp.rs` defines `AcpSession` (fields used here: `cwd`, `roots`, `session_mode` which is `default`, `readOnly`, or `plan`, `ver`), `Sessions` (an `Arc<Mutex<HashMap<String, AcpSession>>>` keyed by ACP session id), `StdoutShared` (the shared stdout handle), and `send_raw`, which writes one JSON line to the editor.

`src/fold.rs` turns MSP items into ACP tool cards. `tool_locations` gives Muse's own file tools ACP `locations`. The card's `toolCallId` is the item's MSP `callId` when present.

`src/json.rs` has the hand-written JSON value `J`, `parse_json`, `esc` (JSON string escaping), `j_to_string`, and `mint_id`.

`tests/acp_serve.rs` runs the built adapter against `tests/fixtures/fake_serve.py`, a fake `muse serve` whose behavior is picked by `FAKE_SCENARIO`. `tests/live_loopback.rs` runs the adapter against a real Muse with a scripted model (`tests/fixtures/loopback_provider.py`); it runs only with `MUSE_ACP_LOOPBACK=1`.

ACP v1 file system methods, as the adapter will use them: `fs/read_text_file` takes `{sessionId, path, line?, limit?}` (absolute `path`, 1-based `line`, `limit` in lines) and returns `{content}`. `fs/write_text_file` takes `{sessionId, path, content}` and returns `null` or `{}`. A tool card's `content` may hold `{"type":"diff","path":...,"oldText":...,"newText":...}`, where `oldText` is null for a new file.

MCP over streamable HTTP, as the bridge implements it: the host POSTs one JSON-RPC message per request to the server URL. The server answers a request with `200` and an `application/json` body that holds the response, and answers a notification or a response with `202` and no body. It may answer `GET` (an optional server-to-client stream) with `405`. The bridge implements `initialize` (it echoes a supported `protocolVersion` and declares `capabilities.tools`), `ping`, `tools/list`, and `tools/call`. It accepts the notifications `notifications/initialized` and `notifications/cancelled` (`params.requestId` names the call to abort). A tool failure is a normal result with `isError: true` and a text explanation, so the model can read it.


## Plan of Work


Milestone 1 adds `src/bridge.rs` and declares it in `src/main.rs` (`mod bridge;`). It owns the negotiated capabilities (`READ`, `WRITE` atomics, set by `bridge::negotiate(ver, capabilities)`), the token table (token to ACP session id), the pending client request table (request id to a channel sender), the pending card content table, and the per-session cancel generations. `bridge::start(stdout, sessions)` binds `127.0.0.1:0` once and spawns an accept thread. Each connection runs on its own thread and reads HTTP/1.1 requests in a loop: a request line, headers up to 64 KiB, and a body by `Content-Length` or chunked encoding, up to 64 MiB. It writes one response per request and honors `Connection: close`. Routing checks in order: an `Origin` header gives 403, a path other than `/mcp` gives 404, a missing or unknown bearer token gives 401, `GET` and `DELETE` give 405, and a `POST` body that is not one JSON object gives 400. A request with a `method` and an `id` gets a JSON-RPC response; anything else gets 202.

The tool logic is separated from the transport. `call_tool(ctx, client, name, arguments, call_id)` takes a `ToolContext` (ACP session id, cwd, roots, session mode, and an abort check) and a `client` closure that sends one ACP request and returns its result. The production closure writes the request with `acp::send_raw` and waits on a channel, polling the abort check every 200 ms. It gives up after 120 seconds and then sends `$/cancel_request`. Unit tests pass a fake closure. `resolve_path(raw, cwd, roots)` makes the path absolute against `cwd`, removes `.` and `..` lexically, finds the deepest ancestor that exists, canonicalizes it, and requires the result to start with a canonicalized root. It returns the lexical absolute path, which is the path the editor knows.

Milestone 2 wires it up. `negotiate_acp` calls `bridge::negotiate`. A new `fn session_mcp_servers(host, stdout, sessions, params, applied, acp_sid) -> (Option<String>, Option<String>)` in `src/main.rs` wraps `client_mcp_servers`. When `bridge::offered()` holds, the host granted `sessionMcp`, and no client server is named `editor`, it starts the bridge, takes the session's token (`bridge::session_token(acp_sid)`, which reuses a bound token), and adds `"editor":{"transport":"streamableHttp","url":"http://127.0.0.1:PORT/mcp","headers":{"Authorization":"Bearer TOKEN"},"mode":"optional"}` to the map. `session/new` and `session/fork` bind the token to the new ACP session id once it exists (`bridge::bind`). Load and resume pass the known id, so the token is bound at once. In `handle_acp`, a client response first goes to `bridge::complete(&id, msg)`, and when that returns true nothing else sees it. `session/cancel` calls `bridge::cancel_session(&sid)`. The `item/completed` handler asks `bridge::take_card_content(acp_sid, item)` for a recorded diff and puts it in `patch_diffs` for that item when no stored-patch diff exists. In `src/fold.rs`, `tool_locations` maps `mcp__editor__read_text_file`, `mcp__editor__write_text_file`, and `mcp__editor__edit_text_file` to their `path` argument.

Milestone 3 adds the `editor_bridge` scenario to `tests/fixtures/fake_serve.py`. The fake host records the `editor` entry from `session/start` or `session/resume`. On `turn/start` it starts a thread that plays Muse's MCP client against the bridge, using `FAKE_BRIDGE_CALLS` (a JSON list of `{name, arguments}`). The thread emits `item/started` and `item/completed` for each call, using the MCP result text, and then completes the turn. It appends every MCP exchange to `FAKE_BRIDGE_LOG`. The black-box tests answer `fs/read_text_file` and `fs/write_text_file` from the test, as an editor would. They cover a read returning the editor's text, a write and an edit producing a diff card, a path outside the roots refused without any `fs/*` request, no bridge entry for clients without `fs` and for ACP v2, the bridge's HTTP authentication, and cancel. The live loopback test uses a real Muse to show a scripted `mcp__editor__read_text_file` call returning editor text that is not on disk.

Milestone 4 documents the feature in `README.md` (a new "Editor file access (ACP v1)" subsection beside "Client-provided MCP servers", plus the feature list), `docs/acp-extensions.md` if it lists client methods, `ROADMAP.md`, and `CHANGELOG.md`. It then runs the full gate (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`) and the live acceptance.


## Concrete Steps


All commands run from the repository root.

    cargo build --locked
    cargo test --locked -p muse-acp bridge::          # unit tests
    cargo test --locked --test acp_serve editor_bridge
    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback editor_bridge
    MUSE_ACP_LOOPBACK=1 MUSE_CLI=/tmp/muse-bins/1.4.8-R6256.1/muse cargo test --locked --test live_loopback editor_bridge
    cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked


## Validation and Acceptance


With an ACP v1 client that advertises `fs.readTextFile` and `fs.writeTextFile`, a session started on Muse 1.3.0 or newer offers the model `mcp__editor__read_text_file`, `mcp__editor__write_text_file`, and `mcp__editor__edit_text_file`. When the model calls the read tool and the user approves, the editor receives `fs/read_text_file` with the session id and the absolute path, and whatever text the editor returns (for example an unsaved buffer) is the tool's output. When the model calls the write or edit tool, the editor receives `fs/write_text_file` with the full new text, and the completed tool card carries a `diff` from the old text to the new text. A path outside the session's roots, or a write in Read-only or Plan mode, fails the tool call with an explanation and sends nothing to the editor. ACP v2 clients, and v1 clients without `fs`, see no change.


## Idempotence and Recovery


Everything is additive. If the listener cannot bind, the adapter logs `editor file tools unavailable: <error>` and sessions start without the bridge. `MUSE_EDITOR_TOOLS=0` turns the bridge off without a rebuild.


## Artifacts and Notes


Prototype transcript (Muse 1.4.11): the permission request offered `allow_once`, `allow_session`, `allow_local_mcp_tool`, and `abort`. After `allow_once`, the MCP server received `tools/call` with `name: read_text_file`, and the model received the server's text as the function call output.


## Interfaces and Dependencies


No new crates. In `src/bridge.rs`:

    pub const SERVER_NAME: &str = "editor";
    pub fn negotiate(ver: u8, capabilities: Option<&J>);
    pub fn offered() -> bool;
    pub fn start(stdout: &StdoutShared, sessions: &Sessions) -> Option<u16>;
    pub fn session_token(acp_sid: Option<&str>) -> String;
    pub fn bind(token: &str, acp_sid: &str);
    pub fn server_entry(port: u16, token: &str) -> String;
    pub fn complete(id: &Option<J>, msg: &J) -> bool;
    pub fn cancel_session(acp_sid: &str);
    pub fn take_card_content(acp_sid: &str, item: &J) -> Option<String>;

Revision note (2026-10-20): recorded progress through Milestone 4, the opt-out variable's rename to `MUSE_EDITOR_TOOLS`, and the outcomes.
