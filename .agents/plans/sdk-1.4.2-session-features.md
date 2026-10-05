# Re-pin the Muse SDK to 1.4.2 and adopt its new session features

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


`muse-acp` lets an editor that speaks the Agent Client Protocol (ACP), such as Zed or a JetBrains IDE, drive Muse Code through Muse's own session protocol (MSP). The adapter keeps a copy of the published MSP schema from the Muse Code SDK (<https://github.com/meta-models/muse-code-sdk>) under `tests/protocol/` and tests itself against it. That copy is pinned at SDK revision `a7c10c5`, which describes the Muse 1.3.0 wire surface. The SDK's newest revision, `bb44be3d36de46d2411bd9eaa4aee99006092546` (2026-09-30, "Re-mirror the SDK closure at tbh@fda770f (1.4.2 lockstep)"), publishes the Muse 1.4.2 surface. Its stable schema fingerprint is `sha256:61afea3112e0906e9dc3a536144278a74cb4b36fc6e20901a91d4432ba3568e2`, byte-identical to what an installed Muse 1.4.2 exports with `muse schema generate-json-schema`.

After this change a user can do five things they could not do before. First, delete a Muse session from the editor's session history: ACP `session/delete` is advertised and backed by MSP `session/delete`, and the deleted session disappears from `session/list`. Second, add extra folders to a session and have Muse's own tools work in them: ACP `additionalDirectories` is passed to Muse as MSP `workspaceRoots` instead of only being enforced by the adapter. Third, see only the reasoning-effort tiers the selected model actually supports, with Muse's descriptions, and have the selector refresh when the model changes. Fourth, see the session cost Muse computes itself, including whether it is partial, instead of the adapter's own list-price estimate. Fifth, send feedback about Muse with a `/feedback` command, with explicit consent for every attachment.

To see it working: run `cargo test --locked` and see the new tests pass; then, with Muse 1.4.2 installed, run `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback` and see the new live tests for delete, workspace roots, and reasoning tiers pass against a real `muse serve`.


## Progress


- [x] (2026-10-05 17:10Z) Read the SDK diff `a7c10c5..bb44be3` in full, the ACP v1 and v2 specs (docs and schemas at agent-client-protocol v1.10.2), and the adapter; probed a live Muse 1.4.2 host (see Surprises & Discoveries).
- [x] (2026-10-05 17:40Z) Wrote this plan.
- [ ] Milestone 1: re-pin `tests/protocol/` to `bb44be3`, update the compatibility table and event matrix, fix `session/closed` handling, record host feature gates.
- [ ] Milestone 2: ACP `session/delete` for v1 and v2, plus the `session/list` fixes it depends on.
- [ ] Milestone 3: ACP `additionalDirectories` sent to Muse as `workspaceRoots`.
- [ ] Milestone 4: per-model reasoning tiers and a spec-conformant `config_option_update`.
- [ ] Milestone 5: host-computed session cost and cache totals in `usage_update`.
- [ ] Milestone 6: `/feedback` backed by MSP `feedback/submit`.
- [ ] Milestone 7: live-host coverage, documentation, full contributor gate, pull request.


## Surprises & Discoveries


- Observation: the live 1.4.2 host only deletes sessions that the same `muse serve` process created with `session/start`. Every other session fails with `reason: "ownershipUnavailable"`: sessions from an earlier host process (even after `session/resume` on the deleting host), sessions created by `session/fork`, sessions made by `muse exec`, and ids that never existed. The schema does not say this; the SDK CHANGELOG only says the TUI `/delete` keeps "sessions with unverified log ownership". Because the adapter starts a new host per editor run, most sessions in an editor's history list cannot be deleted on 1.4.2. The adapter must report this plainly instead of pretending.
  Evidence (wire frames from `muse serve` 1.4.2 with the loopback provider):

      C> {"id":19,"method":"session/delete","params":{"commandId":"01a10c8b-6d84-...","sessionId":"01a10c8b-5a52-..."}}
      S< {"id":19,"result":{"commandId":"01a10c8b-6d84-...","status":"accepted"}}
      S< {"method":"session/deleteCompleted","params":{"commandId":"01a10c8b-6d84-...","outcome":"completed","sessionId":"01a10c8b-5a52-..."}}

      (fresh host, same data directory, session from the earlier process)
      S< {"method":"session/deleteCompleted","params":{"commandId":"...","outcome":"failed","physicalChange":"none","reason":"ownershipUnavailable","sessionId":"01a10c8c-01c9-..."}}

- Observation: an id that never existed is admitted and then fails with the same `ownershipUnavailable` terminal as a real session the host does not own. An already-deleted session sent under a new command id is rejected at admission. A second delete while the first is running is rejected as busy. A session with a running turn fails with `writerBusy` and the turn keeps running. A memory-only host (`--no-session-log`) does not have the method.
  Evidence:

      never existed: ack, then {"outcome":"failed","physicalChange":"none","reason":"ownershipUnavailable",...}
      already deleted: {"error":{"code":-32030,"data":{"kind":"commandRejected","reason":"session_deleted","retryable":false,...}}}
      back-to-back:    {"error":{"code":-32030,"data":{"kind":"commandRejected","reason":"runtime_busy","retryable":true,...}}}
      turn running:    ack, then {"outcome":"failed","physicalChange":"none","reason":"writerBusy",...}
      --no-session-log: {"error":{"code":-32601,"data":{"kind":"methodNotFound"},"message":"method not found"}}
      bad target:      -32602 "Invalid params: invalid session/delete sessionId: ..." / "invalid deletion target" (nil UUID)

- Observation: a completed delete of a loaded session sends only the ack and `session/deleteCompleted`. No `session/closed`, `session/statusChanged`, or `session/listChanged` follows. After it, `session/list` with a `sessionId` filter for that id returns `sessions: []`, `session/read` and `session/resume` return -32020 `sessionNotFound`, and `session/start` with that id is refused with `session_deleted` ("fenced by deletion").

- Observation: `session/closed` means "unloaded from this host", not "deleted". Its schema text: "a loaded session was unloaded from this host ... After it, the session is `notLoaded`; the log remains on disk and `session/resume` reloads it." The adapter today treats it as a list tombstone (`src/main.rs` `cache_session_closed`), which hides stored sessions from ACP `session/list`. On host shutdown the host sends `session/closed {reason:"hostShutdown", sessionId, viewCursor}`, then `session/statusChanged notLoaded`, then `session/listChanged`.

- Observation: `workspaceRoots` validation on the live host (both `session/start` and `turn/start`): a mismatch with `workspaceRoot` is invalid params ("must name the same folder as workspaceRoots[0] (compared on canonical forms ...)"); `null`, a relative path, `[]` ("must be non-empty (omit the field for single-root behavior)"), a duplicate ("duplicate root"), and a missing directory ("not an existing directory") are all invalid params. MSP has no read-back of the root set: `Session` carries only `workspaceRoot`, and `session/resume` and `session/fork` take no `workspaceRoots`.

- Observation: live `model/list` for the loopback test model returns `"defaultReasoningEffort":"high"`, `"reasoningEffortVariants":[]`, and `"variants":["minimal","low","medium","high","xhigh"]`. The SDK's recorded `model-round-trip` transcript has rows with `variants` but no `defaultReasoningEffort`. So the tier list must come from `variants`, descriptions from `reasoningEffortVariants` when present, and the default may be absent.

- Observation: `CumulativeTokenUsage.cost` is absent when no completion is priced. The loopback model has no price, so `session/tokenUsage.cumulative` was `{"outputTokens":1,"promptTokens":1,"totalTokens":2}`. The live shape of `cost` is only known from the schema (`{usd: number, partial: boolean}`, both required).

- Observation: `feedback/submit` without the `feedback` grant fails with -32010 `{"capability":"feedback","kind":"capabilityRequired","retryable":false}` and message "feedback/submit requires the feedback capability". Requesting `feedback` at `initialize` is granted by 1.4.2 (`"grantedCapabilities":["userShell","sessionMcp","sessionListStream","feedback"]`). A real submit was not tried because it uploads to Meta.

- Observation: `session/started` usually arrives before the `session/start` result, but not always (one of sixteen live calls delivered it after the result). Code must not depend on the order.

- Observation: the host's own TUI names the two feedback attachments: "local tracing — selected-session diagnostics, redacted (local-tracing.zip)" and "session record — this whole conversation's replayable trajectory, redacted (session.jsonl)". These map to MSP `withFiles` and `attachSessionRecord`.


## Decision Log


- Decision: scope this work to the re-pin plus every feature the new MSP surface enables (session delete, workspace roots, per-model reasoning tiers, host cost and cache totals, list filters, feedback, typed lifecycle notifications), and fix ACP spec violations only where these features touch the same code: `config_option_update` (ACP requires the full `configOptions` list), extra fields at the root of ACP `Cost` (ACP extensibility forbids custom root fields; they move to `_meta`), `session/closed` handling, and `session/list` paging and errors.
  Rationale: these are the paths the new features change; leaving them non-conformant would ship the new features on broken frames. ACP gaps in unrelated paths (v2 prompt `messageId`, v2 `plan_update`, terminal-auth gating, v2 `auth/*` methods, the `_failed` stop reason, tool-call `name`, `replayFrom: null`) are not part of this SDK update.
  Date/Author: 2026-10-05, Claude.

- Decision: gate features on the host's reported version (`serverInfo.version`), not on the fingerprint table. Delete and workspace roots need Muse 1.4.1 or newer; host cost needs 1.4.2 or newer; delete also needs a durable host (the memory-only profile has no `session/delete`). Feedback is gated on the `feedback` grant. List filtering is gated on `appliedFilter` presence, which the schema names as the probe.
  Rationale: the fingerprint table only knows released builds, while any newer build also has these methods. The adapter's `src/compat.rs` comment already records that 1.4.1 added `session/delete`, `workspaceRoots`, and per-model tiers, and 1.4.2 added feedback, list filters, and cost fields.
  Date/Author: 2026-10-05, Claude.

- Decision: answer ACP `session/delete` when MSP `session/deleteCompleted` arrives, not on the MSP ack.
  Rationale: the ack is "admission only, never an outcome" (`CommandAcceptedResult`), and a delete can still fail after admission (`writerBusy`, `ownershipUnavailable`). Answering early would tell the editor a kept session was deleted. The adapter's main loop is single-threaded and MSP notifications are delivered through it, so the handler cannot block waiting; it records a pending delete keyed by MSP `commandId`, and the `session/deleteCompleted` arm in `handle_msp` answers it. This mirrors how `session/prompt` is answered from `turn/completed`.
  Date/Author: 2026-10-05, Claude.

- Decision: no adapter-side timer on a pending delete. A pending delete is failed when the host it was sent to exits, and settled at adapter shutdown like any other request; an unknown `outcome` keeps it pending.
  Rationale: the schema says "an unknown value leaves the command pending" and the host persists a terminal for every admitted delete. The loop has no timer facility, and a guessed timeout would report failure for a delete that may still complete.
  Date/Author: 2026-10-05, Claude.

- Decision: honor ACP "Deleting an already-deleted session, or a session that never existed, SHOULD succeed silently" as follows. A target that is not a non-nil UUID cannot exist on the host, so answer `{}` without calling the host. A `commandRejected` with reason `session_deleted` and a -32020 `sessionNotFound` are success. Because the host reports a never-existed id with the same `ownershipUnavailable` failure as a real but unowned session, an `ownershipUnavailable` failure is followed by one `session/list {filter:{sessionId:{anyOf:[id]}}}` check: if the host echoes `appliedFilter.sessionId` and returns no rows, the session does not exist and the answer is `{}`; otherwise the failure is reported.
  Rationale: the filter is the host's own existence query (it returned `sessions: []` for a deleted id live), and the `appliedFilter` presence check is how the schema says to detect filter support. A post-check only on this one failure avoids a false "does not exist" for a just-started session that has not been saved yet.
  Date/Author: 2026-10-05, Claude.

- Decision: report host refusals as JSON-RPC errors with code -32603, a plain-language message, and `data: {"reason": <MSP reason>, "physicalChange": <MSP evidence>}`. When `physicalChange` is `possible`, `confirmed`, or unknown, the message says some of the session's data may already be removed. A `failed` terminal missing `reason` or `physicalChange` is malformed and is reported as a failure with unknown evidence (treated as `possible`, as the schema requires for unknown values).
  Rationale: ACP defines no delete error codes and says deleting an active session is implementation-defined; the RFD allows rejecting it. The MSP reason is the only actionable detail the editor can show.
  Date/Author: 2026-10-05, Claude.

- Decision: a second ACP `session/delete` for a session that already has a pending delete joins the pending one and is answered with the same result. A host `runtime_busy` rejection from another client is reported as an error saying Muse is busy with that session.
  Rationale: the live host rejects a second concurrent delete with `runtime_busy`; joining avoids that for the adapter's own duplicates without sleeping in the main loop.
  Date/Author: 2026-10-05, Claude.

- Decision: send the delete to the host that currently holds the session (the existing `Hosts::route` by `sessionId`), or the main host when no host holds it. Do not cancel running work first; the host's own `writerBusy` refusal is reported.
  Rationale: live evidence shows deletion requires the creating host process; routing anywhere else cannot help. Cancelling a user's running turn as a side effect of a delete would be surprising and the host already fails closed.
  Date/Author: 2026-10-05, Claude.

- Decision: `session/closed` no longer hides a session from `session/list`; only a completed delete does. The deleted-id set is consulted whether or not the host granted `sessionListStream`.
  Rationale: schema text above. The stream grant only affects whether the adapter caches streamed rows; deletion must hide the session either way.
  Date/Author: 2026-10-05, Claude.

- Decision: on hosts that support `workspaceRoots`, validate ACP roots up front and send canonical paths. At `session/new`, every `additionalDirectories` entry must canonicalize to an existing directory or the request fails with -32602 naming the entry; canonical duplicates of `cwd` or of an earlier entry are dropped (keeping first-occurrence order, which ACP allows because it does not expand scope); `session/start` carries `workspaceRoots: [canonical cwd, ...canonical extras]` only when there is at least one extra root. After `session/load`, `session/resume`, `session/fork`, and a host restart that re-attaches the session, the next user `turn/start` carries `workspaceRoots: [primary, ...extras]` even when there are no extras, because MSP `turn/start.workspaceRoots` is a sticky replacement ("omitted means unchanged, never reset") and ACP says omitted roots on load or resume activate no additional roots. A steering submit (`ifBusy: "steer"`) never carries it.
  Rationale: the host rejects non-existent, duplicate, and mismatched roots; failing at `session/new` is what ACP requires ("MUST validate the field before creating ... the session") and is clearer than a failed first turn. On older hosts the adapter keeps today's behavior (adapter-side confinement only) and logs that Muse's tools do not see the extra roots.
  Date/Author: 2026-10-05, Claude.

- Decision: the reasoning selector offers the current model's `variants` (in catalog order, with `reasoningEffortVariants[].description` as option descriptions) when the catalog row has a known array; otherwise it keeps today's fixed eight-tier list. A current tier the model does not list stays visible so the selector's current value is always one of its options. AIR `recommendedValue` for reasoning falls back to the row's `defaultReasoningEffort` when the host sent no session-level recommendation. Any host-side change of model, approval mode, reasoning effort, or the adapter's own mode move sends `config_option_update` with the complete `configOptions` list.
  Rationale: ACP `ConfigOptionUpdate` requires `configOptions` ("The full set of configuration options and their current values") in both v1 and v2; the adapter's current `{configId, currentValue}` frame is schema-invalid. Tiers now depend on the model, so a model change must resend the list.
  Date/Author: 2026-10-05, Claude.

- Decision: on hosts that report cost (1.4.2+), ACP `usage_update.cost` comes only from MSP `cumulative.cost` (`amount: usd`, `currency: "USD"`, `_meta.muse: {source: "muse-host", estimate: true, partial}`) and is omitted when the host omits it. On older hosts the adapter's catalog estimate stays, with `source`, `basis`, and `billing` moved from the root of `Cost` into `Cost._meta.muse`. Cumulative `cacheReadTokens` and `cacheWriteTokens` join `_meta.museCumulative`.
  Rationale: the host's figure is "server-computed" and covers every leg it can price; mixing it with the adapter's estimate would double count. ACP `Cost` allows only `amount`, `currency`, and `_meta`.
  Date/Author: 2026-10-05, Claude.

- Decision: `/feedback` is advertised only when the main host granted `feedback`. With form elicitation it opens a form (category, note, two default-off consent checkboxes using the host's own descriptions of the attachments); without forms it accepts `/feedback <bug|bad|good|other> <note>` and sends no attachments. Nothing is sent until the user submits; the outcome is shown as an agent message.
  Rationale: `feedback/submit` says "The call itself is the explicit confirm", so the adapter must collect explicit consent first; attachments must never be sent by default.
  Date/Author: 2026-10-05, Claude.


## Outcomes & Retrospective


(To be filled at milestone completion.)


## Context and Orientation


The repository is a Rust binary with no runtime dependencies (`Cargo.toml` has only a test-only `serde_json`). All JSON is built with `format!` and parsed with the in-tree parser in `src/json.rs` (type `J`, with `get`, `as_str`, and so on). Words used below:

- ACP: the editor-facing protocol. The adapter speaks ACP v1 and v2 over stdin/stdout. The negotiated version is per connection; `AcpSession::ver` stores it.
- MSP: the Muse-facing protocol. The adapter runs `muse serve` as a child process and speaks MSP to it over pipes. A "command" is an MSP request carrying a UUIDv7 `commandId` minted by `MspHost::mint_cmd` in `src/msp.rs`; the host first answers with an ack and may report the outcome later as a notification.
- Host: one `muse serve` process. `src/hosts.rs` keeps up to three: `Main`, `ReadOnly` (started with `--disable-write --disable-shell`, used by the Read-only and Plan modes), and `Reviewer` (memory-only, used by auto-review). `Hosts::route` sends a command to the host that owns the `sessionId` in its params; `note_owner` and `learn_owner` record ownership.
- Handshake: the MSP `initialize` exchange in `MspHost::launch` (`src/msp.rs`). Its result is kept in `HandshakeInfo` (server name and version, schema version and fingerprint, durability, and capability grants such as `session_list_stream`, `user_shell`, `session_mcp`).
- Main loop: `fn main` in `src/main.rs`. One thread reads ACP lines and MSP events from a channel and calls `handle_acp` (ACP requests) or `handle_msp` (MSP notifications) one at a time. A handler may call `host.command(...)`, which blocks until the MSP response arrives (responses are routed by the reader thread, not the main loop), but a handler must never wait for an MSP notification, because notifications are processed by the same loop.
- Session table: `Sessions` (`src/acp.rs`), a map from ACP session id to `AcpSession`. For sessions created by this adapter the ACP id equals the MSP id; legacy `sess-*` ids resolve through `_meta.mspSessionId`.
- List cache: `SessionListCache` in `src/main.rs`, rows streamed by `session/listChanged` when the host grants `sessionListStream`, plus a tombstone set.
- Fake host: `tests/fixtures/fake_serve.py`, a Python MSP host driven by environment variables (`FAKE_SCENARIO`, `FAKE_LOG`, `FAKE_FRAMES`, and knobs listed in its docstring). `result_for` has one arm per method; notifications sent inside `result_for` go out before the ack, notifications sent in `main()` after `send(...)` go out after it.
- Integration tests: `tests/acp_serve.rs` spawns the adapter against the fake host (`Client::spawn`, `req`, `wait_for`, `wait_log`, `wait_frame_contains`, `host_requests`). The test `emitted_frames_conform_to_the_vendored_schema` checks every adapter-to-host request frame against the vendored schema's `required` lists and scalar types.
- Live tests: `tests/live_loopback.rs`, enabled by `MUSE_ACP_LOOPBACK=1`, run a real `muse serve` against `tests/fixtures/loopback_provider.py` with an isolated home. CI runs them against Muse 1.3.0-R3401.1, 1.4.1-R4503.1, and 1.4.2-R4684.1 (`.github/workflows/ci.yml`), so new live tests must check the host version and skip with a message where a feature is absent.
- Compatibility table: `src/compat.rs`. `SDK_MANIFEST_FINGERPRINT` must equal `tests/protocol/stable/manifest.json`; the test `every_schema_notification_has_an_explicit_matrix_disposition` requires `docs/event-compatibility.md` (between `<!-- schema-notifications:start -->` and `<!-- schema-notifications:end -->`) to list exactly the schema's notifications with one of the dispositions "Mapped to ACP", "Internally tracked", "Consumed", "Intentionally ignored", "Unsupported pending protocol decision".

Primary sources used to write this plan (re-read them rather than trusting a summary): the SDK at `bb44be3` (`schema/msp/stable/msp.schema.json`, `schema/msp/msp.d.ts`, `CHANGELOG.md`, `clients/sdk-ts/src/fold/session-fold.ts`), and the ACP spec repository at v1.10.2 (`docs/protocol/v1/session-delete.mdx`, `docs/protocol/v2/session-delete.mdx`, `docs/protocol/v{1,2}/session-list.mdx`, `docs/protocol/v{1,2}/session-config-options.mdx`, `docs/protocol/v1/session-setup.mdx` "Additional Workspace Roots", `docs/rfds/additional-directories.mdx`, `docs/rfds/session-delete.mdx`, and `schema/v{1,2}/schema.json`). Clone them with `git clone https://github.com/meta-models/muse-code-sdk` and `git clone https://github.com/agentclientprotocol/agent-client-protocol`.

The ACP rules this plan relies on, in short: `session/delete` takes `{sessionId}` and returns `{}`; it is advertised as `agentCapabilities.sessionCapabilities.delete: {}` in v1 and `capabilities.session.delete: {}` in v2; clients must not call it unless advertised; deleted sessions must not appear in later `session/list` results; deleting a missing session should succeed silently; deleting an active session is implementation-defined. `config_option_update` must carry the full `configOptions` array. `Cost` has `amount`, `currency` (v2: three upper-case letters), and `_meta` only. `additionalDirectories` entries must be absolute; omitted or empty on load or resume activates no additional roots; agents must validate them before creating or resuming the session and must not silently drop roots they cannot grant.

The MSP rules this plan relies on: `session/delete {commandId, sessionId}` returns `{commandId, status:"accepted"}` and later `session/deleteCompleted {commandId, sessionId, outcome, reason?, physicalChange?}` where `outcome` is `completed` or `failed` (open enum; unknown means still pending), a `failed` terminal carries both `reason` and `physicalChange`, `reason` is one of `ownershipUnavailable`, `sharedSource`, `writerBusy`, `unsafeSource`, `sourceChanged`, `quiescenceFailed`, `cancelled`, `storageFailure`, `cleanupIncomplete`, `unsupportedLayout` (open), and an unknown `physicalChange` counts as `possible`. `session/start.workspaceRoots` is the initial ordered root set (first entry is the primary root and must match `workspaceRoot`); `turn/start.workspaceRoots` is a sticky replacement that may not ride `ifBusy: "steer"`. `session/list` accepts `filter: {sessionId?: {anyOf: [uuid...]}, branch?: {anyOf: [...]}, text?: {fields: ["name"|"title"], allOf?, anyOf?}}` and echoes `appliedFilter` exactly when `filter` was sent. `model/list` rows may carry `variants` (array of tiers, or the string `"unknown"`), `reasoningEffortVariants` (`[{tier, description?}]`), and `defaultReasoningEffort`. `session/tokenUsage.cumulative` may carry `cacheReadTokens`, `cacheWriteTokens`, and `cost: {usd, partial}` (absent when nothing is priced). `feedback/submit {classification: bug|badResult|goodResult|other, note, sessionId, withFiles, attachSessionRecord?, clientArtifactsPath?}` returns `{bundlePath, outcome, sessionRecordAttached, sessionRecordTruncated, cause?, localTracingNote?, retryAfterMs?, sessionNote?, taskId?, uploadId?}`; `attachSessionRecord: true` is valid only with `withFiles: true` and `bug` or `badResult`, and `note` must be non-empty for `bug`. `session/started {session}` and `session/closed {reason: idle|hostShutdown, sessionId, viewCursor: string|null}` are broadcast to every connection.


## Plan of Work


### Milestone 1: re-pin the vendored SDK and fix lifecycle handling

Copy `schema/msp/stable/manifest.json`, `schema/msp/stable/msp.schema.json`, and the whole `schema/msp/transcripts/` directory from the SDK at `bb44be3d36de46d2411bd9eaa4aee99006092546` over `tests/protocol/stable/` and `tests/protocol/transcripts/`, replacing the old files (remove transcript directories that no longer exist upstream; none are expected to). Keep `tests/protocol/LICENSE.muse-code-sdk` (re-copy `LICENSE` if it changed). Rewrite `tests/protocol/PROVENANCE.md` for the new revision, date, and title.

In `src/compat.rs`, alias `SDK_MANIFEST_FINGERPRINT` to `HOST_142_FINGERPRINT`, rewrite the test that asserted equality with the R3401.1 fingerprint so it asserts the SDK pin is the 1.4.2 surface, and drop the 1.4.1 comment sentence "No published SDK carries this surface yet." In `tests/acp_serve.rs`, point `sdk_manifest_fingerprint_is_tested` at the new fingerprint.

In `src/msp.rs`, add `HandshakeInfo::at_least(&self, major: u64, minor: u64, patch: u64) -> bool`, which parses the leading `MAJOR.MINOR.PATCH` of `server_version` (ignoring any suffix) and returns false when it cannot. Add `supports_session_delete()` (durable host and at least 1.4.1; read how `durability` is spelled in the handshake and in `restartable()`), `supports_workspace_roots()` (at least 1.4.1), and `reports_session_cost()` (at least 1.4.2). Unit-test the parser with `"1.4.2"`, `"1.4.1"`, `"1.3.0"`, `"1.10.0"`, `"1.4.2-R4684.1"`, `""`, and `"garbage"`.

Fix `session/closed`: it must no longer tombstone. Rename `SessionListCache::closed` to `deleted` (it will be filled by Milestone 2), stop calling the tombstone helper from the `session/closed` arm, and make that arm log the unload with its `reason`. Update the test that pinned the old behavior (`session_list_stream_updates_titles_filters_rows_and_unloads_sessions`) so an unloaded session stays listed. Update the fake host to send schema-shaped `session/closed {reason, sessionId, viewCursor}` frames.

Add an explicit `handle_msp` arm for the experimental `mcpServer/oauthLoginCompleted` notification (the adapter connects with `experimentalApi: true`, so it can arrive) that logs and ignores it, instead of falling to "unhandled MSP notification".

In `docs/event-compatibility.md`, move `session/started` and `session/closed` into the schema matrix (they are now in the published notification index) with accurate dispositions, add `session/deleteCompleted` (disposition "Unsupported pending protocol decision" in this milestone; Milestone 2 changes it to "Mapped to ACP"), and remove the now-wrong "Host-emitted extensions" rows. Fix `ROADMAP.md` statements that `session/started` is absent from the published index and that no published SDK carries the 1.4 surface, and update the reference-snapshot header and §2 pin text.

Acceptance: `cargo test --locked` passes, including `vendored_sdk_manifest_matches_the_compatibility_table`, `every_schema_notification_has_an_explicit_matrix_disposition`, and the transcript replay tests in `src/fold.rs`; `cargo run --locked -- --selftest` prints `sdk-manifest` as tested.

### Milestone 2: ACP `session/delete`

Advertise delete only when `host.handshake().supports_session_delete()`: extend `v1_init` and `v2_init` in `src/main.rs` with a `session_delete: bool` argument (update `send_initialize` and `selftest`, which parses all four variants) and insert `"delete":{}` into `sessionCapabilities` (v1) or `capabilities.session` (v2). The initialize response is built after the host handshake, so the grant is known.

Add a `"session/delete"` arm to `handle_acp`. It reads `params.sessionId` (string, else -32602), resolves the MSP id the same way `session/resume` does (live session, then `_meta.mspSessionId`, then the raw id), and answers `{}` immediately when the MSP id is not a non-nil UUID (write `fn is_session_uuid(s: &str) -> bool`; accept any version, as the schema says "legacy-valid"). If a delete for the same MSP id is already pending, it adds this ACP request id to that pending entry and returns. Otherwise it mints a command id, records a `PendingDelete { waiters: Vec<J>, msp_sid: String, host_kind: HostKind }` under that command id in a new static map (follow the `REVIEW_STATE` pattern), and sends `session/delete {commandId, sessionId}` through `host.route`. If the command fails, remove the pending entry and answer: `commandRejected` with reason `session_deleted` or a -32020 `sessionNotFound` error means success (clean up and answer `{}`); reason `runtime_busy` is an error "Muse is busy with this session; try again"; -32601 is an error "This Muse host cannot delete sessions"; any other error is passed through with its message.

Add a `"session/deleteCompleted"` arm to `handle_msp`. Look up the pending entry by `commandId` (a terminal for an unknown command, for example from another client, is logged; a `completed` one still tombstones the id). If `outcome` is `completed` and neither `reason` nor `physicalChange` is present, clean up and answer every waiter `{}`. If `outcome` is `failed` with `reason` `ownershipUnavailable`, run the existence check (below); if the host says the session does not exist, clean up and answer `{}`. Any other `failed` (or a malformed one) answers every waiter with a -32603 error built by `fn delete_failure_message(reason: Option<&str>, physical_change: Option<&str>) -> String` and `data: {"reason", "physicalChange"}`. Use these messages, taken from the host's own TUI wording where it exists: `ownershipUnavailable` "Muse kept this session because it cannot prove it owns all of the session's logs. Muse deletes only sessions started by the Muse host that is running now, so sessions from an earlier editor run cannot be deleted here."; `writerBusy` and `quiescenceFailed` "Muse kept this session because work is still running in it. Stop it and try again."; `sharedSource` "Muse kept this session because some of its logs are shared with another session."; any other reason "Muse could not delete this session (<reason>)."; and append " Some of its data may already be removed." when `physicalChange` is anything but `none`. An unknown `outcome` leaves the entry pending and logs.

Cleanup after a successful delete (`fn forget_deleted_session`): remove the `AcpSession` if this adapter holds it, settling anything still open exactly as `session/close` does (reuse its code; normally nothing is open because the host refuses a busy session); add the MSP id to `SessionListCache::deleted` and drop any cached row; forget the stored mode with `modes::save(msp_sid, DEFAULT_MODE)` or a new `modes::forget`; and forget the host owner (add `Hosts::forget_owner`). `learn_event_owner` must not re-learn an owner from a `session/deleteCompleted` frame.

Existence check (`fn session_absent_on_host(host, msp_sid) -> bool`): send `session/list {limit: 1, filter: {sessionId: {anyOf: [msp_sid]}}}` to the main host; return true only when the result has `appliedFilter.sessionId` and `sessions` is empty. Any error, or a missing `appliedFilter.sessionId`, returns false (the failure is then reported).

When a host exits, every pending delete sent to that host kind is answered with -32603 "Muse exited before it confirmed the deletion. List sessions to see whether it was removed." (hook the same places that fail in-flight prompts for that host: the main-host EOF path, `recover_read_only_host`, and `fail_all_with_message`).

`session/list` fixes in the same milestone: skip ids in `deleted` whether or not the stream is granted; append adapter-held sessions that the host page did not include only on the first page (no `cursor`), and on later pages drop host rows for adapter-held sessions so each session appears once; when the request carried a `cursor` and the host rejects it, answer -32602 "invalid cursor" instead of the silent fallback; skip host rows whose `workspaceRoot` is null or empty (ACP requires an absolute `cwd`) and log how many were skipped.

`session/load` and `session/resume` of a session the host reports as `sessionNotFound` (-32020) answer -32002 with "session not found (it may have been deleted)". Check the existing error path first and keep the auth (-32000) mapping intact.

Fake host: add `session/delete` (ack `{commandId, status:"accepted"}`, then `session/deleteCompleted` after the ack) with knobs to produce `completed`, `failed` with a chosen reason and physical change, a `commandRejected` `session_deleted`, `runtime_busy`, and a `session/list` `filter` echo (`appliedFilter`) with `sessions` filtered by id. Make its `initialize` report `serverInfo.version` from a `FAKE_SERVER_VERSION` knob so tests can exercise the version gates (default it to `1.4.2` only if every existing test still passes; otherwise keep the current default and set the knob in the new tests).

Tests (all in `tests/acp_serve.rs`): delete advertised in v1 and v2 on a 1.4.2 host and absent on a 1.3.0 host and on an ephemeral host; delete of a live idle session answers `{}` only after `deleteCompleted`, removes it from `session/list`, and later prompts get "unknown sessionId"; non-UUID and `session_deleted` and never-existed (`ownershipUnavailable` plus empty filtered list) answer `{}`; `ownershipUnavailable` for an existing session, `writerBusy`, and a failure with `physicalChange: possible` answer -32603 with the right message and data; two concurrent deletes for one session are both answered from one host command; host exit with a pending delete answers it; list paging no longer duplicates live sessions; invalid cursor errors; null-root rows are skipped; the emitted `session/delete` frame passes the schema gate. Update `docs/event-compatibility.md` (`session/deleteCompleted` becomes "Mapped to ACP").

### Milestone 3: `additionalDirectories` as MSP `workspaceRoots`

Add to `AcpSession` a `host_roots_pending: bool` (true when the next user `turn/start` must carry `workspaceRoots`) and keep `roots` as the ACP-ordered list. Write `fn host_workspace_roots(cwd: &str, extras: &[String]) -> Result<Vec<String>, String>` that canonicalizes `cwd` and each extra with `std::fs::canonicalize`, requires each to be a directory, drops canonical duplicates (keeping first occurrence), and returns the canonical strings; on Windows, strip a leading `\\?\` when the rest is a plain drive path (`C:\...`) so the host sees a normal absolute path. Errors name the offending entry.

When `host.handshake().supports_workspace_roots()`: in `session/new`, call it before `session/start`, fail with -32602 on error, and add `"workspaceRoots":[...]` to `session/start` when there is at least one extra root. In `restart_unsaved_session` (which re-sends `session/start`), do the same from the stored roots. In `session/load`, `session/resume`, and `session/fork`, validate the request's roots the same way (fail with -32602 before contacting the host) and set `host_roots_pending = true`; also set it for every session re-attached by `restart_durable_host` and by mode moves. In the `session/prompt` path that sends `turn/start`, when `host_roots_pending` is true, add `"workspaceRoots":[primary, ...extras]` where `primary` is the canonical form of the session's `cwd`; clear the flag only after the host accepts the turn. Never add it to the steering `turn/start` (`ifBusy: "steer"`) or to `turn/steer`. For resume and load, compare the canonical request `cwd` with the host session's `workspaceRoot` from the resume result; if they differ, fail the request with -32602 saying the `cwd` does not match the session's folder (ACP requires the load/resume `cwd` to match the session's `cwd`), unless the existing code already handles this case differently, in which case keep that behavior and record it here.

On older hosts, keep the current behavior and log once per session with extra roots: "this Muse host does not support workspaceRoots (needs 1.4.1); Muse's own tools only see <cwd>".

Fake host: record `workspaceRoots` from `session/start` and `turn/start` in the input log and validate them like the live host (non-empty, absolute, no duplicates, first equals `workspaceRoot`) so tests catch mistakes. Tests: new session with two extra directories sends canonical `workspaceRoots`; a missing directory fails `session/new` with -32602 naming it; a symlinked or duplicate root is canonicalized and deduplicated; after `session/load` with no extras the first prompt's `turn/start` carries `[cwd]` and the second prompt's does not; after resume with extras the first prompt carries them; steering never carries it; on a 1.3.0-version fake host nothing is sent. Update README ("Workspace roots" text that says MSP has no additional-root field) and ROADMAP §8.

### Milestone 4: per-model reasoning tiers and full `config_option_update`

Replace the catalog tuple `(String, String, bool)` with a struct `CatalogModel { id, label, is_default, variants: Option<Vec<String>>, tier_descriptions: Vec<(String, Option<String>)>, default_effort: Option<String> }` in `src/main.rs` (or `src/acp.rs` if that reads better), filled by `catalog()` from `model/list`. `variants` is `None` when absent or `"unknown"`; unknown tier strings outside the closed `ReasoningEffort` set are skipped with a log.

Change `acp::config_options` to build the reasoning options from the current model's row: the "Muse default" option when offered, then each tier in `variants` order with today's display names and the tier's description when present; when `variants` is `None`, today's fixed list. If the session's current tier is not in the list, keep it in the list. `recommended_reasoning` falls back to the current model's `default_effort` when the session has no host recommendation (still only when AIR `recommendedValue` was negotiated, and only if the value is among the options).

Replace `acp::send_config_option_update(stdout, acp_sid, config_id, value, recommended)` with `send_config_options_update(stdout, acp_sid, config_options_json)` that sends `{"sessionUpdate":"config_option_update","configOptions":[...]}` built by the same code as `config_options_result`. Call it from the mode switch, `session/reasoningEffortChanged`, `session/modelChanged`, and `session/approvalModeChanged`. When the model changes (from the editor or the host) and the adapter holds a per-turn tier override that the new model's known `variants` do not include, reset the override to "Muse default" and log it, so the adapter never sends a tier the model does not serve.

Fake host: give `model/list` rows `variants`, `reasoningEffortVariants` with a description, and `defaultReasoningEffort`, plus a second model with a different tier set, and send `session/modelChanged` on `session/setModel`. Tests: the selector offers exactly the model's tiers with descriptions; switching model resends the full list with the new tiers; a host `reasoningEffortChanged` sends the full list; a model with `"variants":"unknown"` gets the fixed list; the AIR recommendation falls back to `defaultReasoningEffort`. Update the two existing tests that pinned the old `config_option_update` shape.

### Milestone 5: host session cost and cache totals

Extend `adopt_cumulative` to read `cacheReadTokens`, `cacheWriteTokens`, and `cost {usd, partial}` into new `AcpSession` fields (`cum_cache_read`, `cum_cache_write`, `host_cost: Option<(f64, bool)>`); `cost` replaces the previous value whenever a cumulative object arrives (it may go down), and a cumulative object without `cost` clears it. In `acp::send_usage`, when the host reports cost (`reports_session_cost()`; pass a flag in), emit `cost` only from `host_cost` as `{"amount":usd,"currency":"USD","_meta":{"muse":{"source":"muse-host","estimate":true,"partial":bool}}}`, and skip the adapter's catalog estimate entirely (do not compute it). Otherwise keep the estimate with `"_meta":{"muse":{"source":"adapter-estimate","basis":"catalog-list-price","billing":false}}` and no extra root keys. Add `cacheReadTokens` and `cacheWriteTokens` to `_meta.museCumulative` when known. Guard non-finite numbers as today. Fix the stale `cost_amount` comment about cached input.

Fake host: a scenario whose `session/tokenUsage.cumulative` carries cache splits and `cost`, one with `partial: true`, and one without `cost`. Tests: host cost is forwarded with `partial`; no cost is sent when the host omits it on a 1.4.2 host; a 1.3.0 host still gets the estimate with metadata inside `_meta`; the existing `usage` test is updated for the `_meta` move. Update README "Usage" and ROADMAP §11.

### Milestone 6: `/feedback`

Request `feedback` in `requestedCapabilities` in `MspHost::launch` and record the grant in `HandshakeInfo::feedback`. Advertise a `feedback` command in `available_commands_json` only when the main host granted it, with description "Send feedback about Muse" and input hint "[bug|bad|good|other] <note>". Handle `/feedback` in the prompt command path next to `/rename` and `/compact`.

With form elicitation negotiated, send an `elicitation/create` form for the session with these properties: `classification` (enum `bug`, `badResult`, `goodResult`, `other` with titles "Bug", "Bad result", "Good result", "Other"; default from the command argument when given), `note` (string, default from the argument), `withFiles` (boolean, default false, title "Include local tracing", description "Selected-session diagnostics, redacted"), and `attachSessionRecord` (boolean, default false, title "Attach the session record", description "This whole conversation's replayable trajectory, redacted. Only for Bug or Bad result, and only with local tracing."). Required: `classification` and `note`. Route the form answer back to the feedback flow (add a pending-feedback state keyed by the form request id, or a `PendingUi` variant; read how user-input forms are tracked first). On accept, check the rules (`note` non-empty for `bug`; `attachSessionRecord` only with `withFiles` and `bug` or `badResult`) and send `feedback/submit {classification, note, sessionId, withFiles, attachSessionRecord}` to the main host. On decline or cancel, end the turn with "Feedback not sent." Without forms, parse `/feedback <bug|bad|good|other> <note>` (accept `badResult`/`goodResult` too) and send with both consents false; with missing parts, end the turn with the usage line.

Show the result as an agent message and end the turn normally: `uploaded` "Feedback sent (id <uploadId>)."; `recorded` "Feedback recorded."; `rateLimited` "Feedback was not sent: rate limited, try again in <n> seconds."; `noCredential`, `authRejected`, `disabled`, `dark`, `failed`, and any unknown outcome "Feedback was not sent (<outcome>[: <cause>])."; `acceptedWithoutReceipt`, `trackingFailed`, `trackingUncertain` "Feedback was sent, but Muse could not confirm the receipt (<outcome>)."; then "A local copy is at <bundlePath>." and any `sessionNote` and `localTracingNote` on their own lines. A host error is shown as "Feedback was not sent: <message>." Never retry automatically (the method has no idempotency key).

Fake host: grant `feedback` when requested (knob to withhold it), implement `feedback/submit` with selectable outcomes and validation errors. Tests: command advertised only with the grant; form shown with defaults off; accepted form sends exactly the chosen fields; declined form sends nothing; invalid consent combination is refused locally; no-form syntax; each outcome message. No live test submits feedback (it would upload to Meta); record that in this plan.

### Milestone 7: live coverage, documentation, and the gate

Add live tests to `tests/live_loopback.rs`, each reading the host version from the adapter log line `host-ready server=muse/<version>` or from the initialize capabilities, and skipping with an explicit message where the feature is absent: delete of a session started in this run answers `{}` and the session leaves `session/list` and cannot be loaded; delete of a session from an earlier adapter run answers an error with `data.reason == "ownershipUnavailable"` (this pins observed 1.4.x behavior; a host that lifts the restriction makes the test fail loudly, which is the intended compatibility signal); delete of a random UUID answers `{}`; a session with an extra directory lets the scripted model read a file in it (read the loopback provider to see how tool calls are scripted); the reasoning selector offers exactly the live `model/list` variants. Run the suite locally against the installed 1.4.2 and against 1.4.1 and 1.3.0 downloaded with the URLs and checksums in `.github/workflows/ci.yml` (set `MUSE_CLI`).

Update `README.md` (session features, delete limits, workspace roots, reasoning tiers, cost source, `/feedback`), `ROADMAP.md` (§1, §2, §8, §11, §14, §20 status text; the reference snapshot header), `CHANGELOG.md` (`## Unreleased` entries in the existing style), `docs/event-compatibility.md`, and this plan's living sections. Then run the full contributor gate and open the pull request.


## Concrete Steps


All commands run from the repository root (the worktree). Clone sources once into a scratch directory outside the repository:

    git clone https://github.com/meta-models/muse-code-sdk "$SCRATCH/muse-code-sdk"
    git -C "$SCRATCH/muse-code-sdk" checkout bb44be3d36de46d2411bd9eaa4aee99006092546
    git clone https://github.com/agentclientprotocol/agent-client-protocol "$SCRATCH/agent-client-protocol"

Milestone 1 copy:

    rm -rf tests/protocol/stable tests/protocol/transcripts
    cp -r "$SCRATCH/muse-code-sdk/schema/msp/stable" tests/protocol/stable
    cp -r "$SCRATCH/muse-code-sdk/schema/msp/transcripts" tests/protocol/transcripts
    cat tests/protocol/stable/manifest.json
      {"experimental": false, "fingerprint": "sha256:61afea3112e0906e9dc3a536144278a74cb4b36fc6e20901a91d4432ba3568e2", "schemaVersion": 1}

Contributor gate (run after every milestone; all must pass):

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Live suite (Muse installed; set `MUSE_CLI=/path/to/muse` to pick a build):

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback

Commit after each milestone with a message that names the milestone's user-visible change.


## Validation and Acceptance


The work is accepted when all of the following hold. `cargo test --locked` passes with the new tests described in each milestone; each new test fails if its feature code is removed. The selftest prints the SDK manifest row as tested at the 1.4.2 fingerprint. Against a live Muse 1.4.2 with the loopback provider, an ACP client can: see `"delete":{}` in the initialize result; delete a session started in this run and get `{}`, after which `session/list` no longer returns it and `session/load` fails with -32002; delete a session from an earlier run and get a -32603 error whose `data.reason` is `ownershipUnavailable` and whose message explains why; delete a random UUID and get `{}`; open a session with an extra directory and have Muse read a file there; and see a reasoning selector listing `minimal, low, medium, high, xhigh` (the live catalog's variants for the test model) plus "Muse default". Against Muse 1.3.0 the live suite still passes, with the new live tests reporting that they skipped.


## Idempotence and Recovery


Every step is additive and can be repeated: the schema copy replaces files wholesale, tests use temporary directories, and the live tests isolate `HOME` and the XDG directories. Do not run `muse exec` with a real home during testing: an earlier probe found that `muse exec` writes a session-name claim into `~/.local/share/muse/session-name-authority/` even when `HOME` and `XDG_*` point elsewhere. The live suite uses `muse serve` only, which did not write there. If a milestone breaks the gate, fix it before starting the next one; each milestone is a separate commit, so `git revert` of one milestone is possible.


## Artifacts and Notes


Live `model/list` row for the loopback model (Muse 1.4.2):

    {"defaultReasoningEffort":"high","reasoningEffortVariants":[],"variants":["minimal","low","medium","high","xhigh"],"source":"bundledCatalog", ...}

ACP v1 delete capability placement (from `docs/protocol/v1/session-delete.mdx`):

    "agentCapabilities": { "sessionCapabilities": { "delete": {} } }

ACP v2 placement (from `docs/protocol/v2/session-delete.mdx`):

    "capabilities": { "session": { "delete": {} } }


## Interfaces and Dependencies


No new dependencies. At the end of the work these exist:

In `src/msp.rs`: `impl HandshakeInfo { pub fn at_least(&self, major: u64, minor: u64, patch: u64) -> bool; pub fn supports_session_delete(&self) -> bool; pub fn supports_workspace_roots(&self) -> bool; pub fn reports_session_cost(&self) -> bool; }` and `pub feedback: bool` on `HandshakeInfo`.

In `src/hosts.rs`: `impl Hosts { pub fn forget_owner(&self, msp_sid: &str); }`.

In `src/main.rs`: `fn v1_init(session_mcp: bool, session_delete: bool) -> String`, `fn v2_init(session_mcp: bool, session_delete: bool) -> String`, `fn is_session_uuid(s: &str) -> bool`, `fn delete_failure_message(reason: Option<&str>, physical_change: Option<&str>) -> String`, `fn session_absent_on_host(host: &Arc<Hosts>, msp_sid: &str) -> bool`, `fn forget_deleted_session(...)`, `fn host_workspace_roots(cwd: &str, extras: &[String]) -> Result<Vec<String>, String>`, a pending-delete map keyed by MSP command id, and the `CatalogModel` struct.

In `src/acp.rs`: `pub fn send_config_options_update(stdout: &StdoutShared, acp_sid: &str, config_options_json: &str)`; `AcpSession` fields `host_roots_pending: bool`, `cum_cache_read: Option<u64>`, `cum_cache_write: Option<u64>`, `host_cost: Option<(f64, bool)>`.

Names may change during implementation if the surrounding code suggests better ones; record any change in the Decision Log.
