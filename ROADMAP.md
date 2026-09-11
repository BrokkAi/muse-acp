# muse-acp roadmap

This is a living roadmap for `muse-acp`. It records the direction that keeps the
adapter close to Muse Session Protocol (MSP), safe around approvals and file
access, and useful in real editor workflows.

- **Last revised:** 2026-09-10
- **Baseline:** `v0.2.5`
- **Protocol sources:** [Muse Code SDK][sdk] and [Muse Code Developer Docs][docs]
- **Comparable adapter used for feature benchmarking:** [`codex-acp`][codex-acp]
- **Reference snapshots used for this revision:** Muse SDK `fbce769`
  (2026-09-02; stable schema version 1, manifest fingerprint
  `sha256:cfd31ee77d78fdada9febc4edccd29b0434ff8f6bf157c7c03fd0ecfcbc29f5a`)
  and `codex-acp` `51d6247` (v1.11.0, 2026-09-10).

## Product principles

1. **Preserve Muse session semantics.** Sessions, approvals, resume behavior,
   queueing, cancellation, and usage should behave like Muse—not like detached
   one-shot `muse exec` calls.
2. **Fail closed.** Ambiguous approvals, host shutdown, unsupported protocol
   methods, and invalid roots must never expand permissions or silently pretend
   to succeed.
3. **Make protocol drift visible.** Muse MSP and the SDK are Developer Preview
   surfaces. Compatibility should be measured continuously and explained in
   actionable diagnostics.
4. **Keep editor onboarding simple.** One native binary, no required Node.js
   runtime, and conservative Zed/JetBrains settings edits.
5. **Do not fabricate data.** Usage and cost should clearly distinguish host
   facts, restored state, replayed history, and client-local estimates.
6. **Prefer protocol-native features.** Optional ACP extensions should map to
   MSP concepts rather than emulate semantics the host cannot guarantee.

## Current strengths

`v0.2.5` already has a solid stateful foundation:

- one long-lived `muse serve` host for all ACP sessions;
- ACP v1 and v2 support;
- durable Muse session IDs across adapter restarts;
- history replay and session load/resume;
- concurrent/queued turns and terminal cancellation behavior;
- exact-turn steering with race-safe idle handling;
- image and embedded-context prompts;
- workspace-confined textual resource links;
- model, approval-mode, and reasoning-effort configuration;
- typed MSP approval subjects and multi-stage requirement handling;
- deny-safe approval and user-input fallback;
- restored context/cumulative usage and replay-safe completion pricing;
- checksummed release installation and comment-preserving editor setup.

The roadmap below protects those strengths while closing the main gaps in
protocol safety, editor onboarding, feature breadth, and long-term maintenance.

## P0: protocol safety and compatibility

These are prerequisites for safely tracking a Developer Preview protocol.

### 1. Runtime schema compatibility diagnostics

Compare the MSP schema version and fingerprint reported by `muse serve` with a
compatibility table maintained in the adapter.

Status: **partially implemented.** Every launch classifies the handshake and
logs a machine-readable `schema-compat` line plus a `host-ready` line;
`--selftest` prints the offline table. Current policy: unknown fingerprints
degrade with a warning, while an unsupported envelope schema version fails
closed. Remaining work: enrich the table as live hosts are validated and feed
the verdict into richer support bundles.

**Work items**

- Seed the table with the inputs already known to differ: the adapter pins
  `sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7`
  (host 1.0.2), while the SDK manifest at `fbce769` publishes
  `sha256:cfd31ee77d78fdada9febc4edccd29b0434ff8f6bf157c7c03fd0ecfcbc29f5a`
  (schema version 1). Transcript fixtures intentionally carry their own
  fingerprint (`sha256:c8d1a2a1866814e220fd396d382a9a75861412feee884b5021b2ee359bd3dc59`)
  and must not be conflated with either surface.
- Record tested schema version/fingerprint pairs.
    - Include the host protocol version, adapter release, and support status.
    - Distinguish tested, warning, and incompatible combinations.
- Add the observed and expected identifiers to startup or self-test logs.
- Produce an actionable mismatch message before unrelated method failures occur.
- Decide separately whether fingerprint mismatch is fatal or degraded.
- Avoid relying only on a single source snapshot: the SDK repository and docs
  can briefly publish different schema fingerprints.
- Record host-versus-schema discrepancies as compatibility facts. Known
  example: hosts emit a `session/started` notification after `session/start`,
  but the published `MspNotification` union omits it.

**Acceptance criteria**

- A host with an unknown fingerprint still yields a clear compatibility warning.
- Every supported branch has a machine-readable compatibility result.
- Self-test output includes adapter version, host version, schema version, and
  schema fingerprint.

### 2. Continuous conformance against the Muse schema corpus

The SDK publishes generated MSP types, a schema bundle, and recorded transcripts.
The Rust adapter should consume those artifacts as conformance inputs.

Status: **corpus vendored and replaying.** `tests/protocol/` pins SDK revision
`fbce769` (stable manifest, JSON schema bundle, 50 golden transcripts) with
provenance and license. CI now replays every server-side item event through
the notification fold (unknown kinds must tolerate), validates every emitted
ACP frame with the adapter's own parser, and fails if the vendored manifest
fingerprint drifts from the compatibility table. Remaining work: schema-bundle
validation of full adapter payloads, permission-path transcript replays, and
the optional live-host smoke gate.

**Work items**

- Pin the SDK revision (currently `fbce769`, 2026-09-02) and vendor
  `schema/msp/stable/manifest.json`, `schema/msp/stable/msp.schema.json`, and
  the `schema/msp/transcripts/` corpus under `tests/protocol/`, or fetch that
  locked revision in CI. The corpus already covers approvals, cancellation,
  compaction, cursor/gap recovery, goals, handshakes, models, pending-command
  reconciliation, resume, subagents, user input, user shell, workflows, and
  unknown-kind/state/stream tolerance.
- Validate JSON payloads emitted by the adapter against the schema bundle.
- Replay recorded MSP transcripts through the notification fold and permission
  paths.
- Add compatibility tests for typed MSP errors, especially stale approval
  requirements and invalid choices.
- Add an optional live-host smoke test gated by an environment variable and
  authenticated Muse installation.

**Acceptance criteria**

- CI fails when a schema change alters a method, result, notification, error, or
  field consumed by the adapter.
- Transcript replays do not depend on an authenticated Meta account.
- Live-host tests are optional and clearly distinguish skipped from failed.

### 3. Pending approval and user-input reconciliation

Use MSP `approval/listPending` to reconcile approvals and user-input prompts
after reconnect, resume, load, and view attachment.

Status: **implemented for resume/load.** Every successful attach now pulls
`approval/listPending` and presents approvals and user input that were not
already displayed, deduplicated by id against displayed, queued, and settled
requests. Concurrent approvals queue behind the displayed permission instead
of overwriting it. Remaining work: queued-turn re-association from the resume
snapshot's pending-command set.

**Work items**

- Call the pull endpoint after session resume/load and view attachment.
- Compare pending records against already displayed requests.
- Preserve current deduplication for replayed user-input forms.
- Define reconciliation behavior when a notification and pull result disagree.
- Ensure no pending request can be lost because a replay was missed.
- Extend the same reconciliation to queued turns: the resume snapshot carries
  `queuedTurns`, `pendingApprovals`, and `pendingUserInputs` (the SDK's SS4.13
  pending-command set). Use them to re-associate in-flight ACP prompts with
  admitted-but-not-launched turns, and reuse the original `commandId` when
  retrying an admitted command.

**Acceptance criteria**

- Dropping an approval/user-input notification still leaves the request visible
  after reconciliation.
- Replayed and pulled copies of the same request are displayed once.
- Stale requirements cannot satisfy a later approval stage.
- After reconnect/resume, in-flight ACP prompts correspond one-to-one with the
  snapshot's queued/active turns or settle with an explicit classification.

### 4. Method-specific handling of server-initiated requests

Do not return a generic `{}` result for every server-initiated MSP request.

Status: **implemented for the current schema.** Only `approval/request` and
`userInput/request` are acked `{}` and forwarded; every other server-initiated
request receives the typed `methodNotFound` error (matching the reference SDK
client's shape) and is logged with its id. New methods require a deliberate
table entry plus a disposition in the event matrix.

**Work items**

- Explicitly support known request methods such as `approval/request` and
  `userInput/request`.
- Return a protocol-appropriate unsupported response for unknown methods when a
  response is required.
- Log unknown methods with enough detail to add support safely.
- Keep unsupported methods fail-closed.

**Acceptance criteria**
- A future MSP request cannot accidentally receive an invalid success result.
- Unsupported requests are observable in diagnostics rather than silently lost.

### 5. Method-aware command timeout policy

Replace the single 60-second command timeout with policy appropriate to MSP
method semantics.

Status: **implemented.** Timeouts now follow a per-method table (30s for the
handshake/queries/control decisions, 180s for history-bearing lifecycle work,
60s default) with a global `MUSE_COMMAND_TIMEOUT_MS` override. Timeout errors
carry the method, request id, duration, and session when present. Retry
policy: reissue only with the original `commandId` (admission is idempotent);
never mint a fresh handle for a retry. Remaining work: profile the table
against slow real hosts and adjust.

**Work items**

- Profile normal and slow behavior for startup, `model/list`, resume, view
  paging, and approval decisions.
- Set method-specific defaults and allow an environment override.
- Include MSP method, request ID, timeout duration, and session ID in errors.
- Define which timed-out commands may be retried and how idempotency handles are
  reused.
- Add shutdown tests for commands waiting when the host exits.

**Acceptance criteria**

- Slow, legitimate resume operations do not become indistinguishable protocol
  failures.
- Retry cannot accidentally duplicate a non-idempotent operation.

## P1: editor onboarding, transparency, and parity

These items improve day-to-day reliability for existing users.

### 6. Authentication and host-readiness diagnostics

Current setup assumes `muse` is installed and authenticated. Make failures
obvious before the first prompt.

**Work items**

- Distinguish missing executable, unauthenticated host, expired session,
  unsupported host version, and host startup failure.
- Include the executable path and host version where available.
- Provide the next user action for each failure.
- Consider advertising a safe ACP auth flow if/when Muse exposes an
  authentication method compatible with ACP.
- Add browserless/remote-environment guidance.

**Acceptance criteria**

- A new user can tell whether to install Muse, log in, set `PATH`, upgrade
  `muse-acp`, or report a protocol incompatibility.
- Authentication errors do not appear as generic JSON-RPC internal errors.

### 7. Explicit client MCP policy

Client MCP configuration is currently tolerated but not forwarded. This should
be an explicit product decision.

**Work items**

- Document why client-provided MCP is ignored.
- Emit a diagnostic or capability signal rather than silently dropping it.
- If forwarding is added:
    - support stdio command and HTTP client servers where possible;
    - route tool calls through Muse approvals;
    - preserve session-root confinement;
    - define lifecycle and failure behavior for client-owned servers;
    - avoid extending host permissions merely because a client supplied a tool.

**Acceptance criteria**

- Users can discover from logs/docs why MCP tools are unavailable.
- Any future forwarding path cannot bypass approval or workspace policy.

### 8. Workspace roots and resource-link hardening

Clarify and test the adapter's multi-root behavior and continue tightening
local-read confinement.

**Work items**

- Document how ACP `cwd`, additional workspace directories, and MSP session
   roots map to one another.
- Add tests for nested roots, repeated roots, symlinked roots, and unrelated
   roots.
- Audit textual resource links for symlink, path normalization, hard-link,
   case-normalization, and `file://` percent-encoding edge cases.
- Keep binary/blob resources rejected unless a safe typed representation is
   specified.
- Keep `MUSE_ALLOW_UNSCOPED_READS` opt-in and explicit.

**Acceptance criteria**

- No resource expansion can read outside all approved roots without the explicit
  unsafe-read override.
- Root behavior is identical across Zed and JetBrains fixtures.

### 9. Richer event compatibility matrix

Publish a full MSP-to-ACP event matrix so ignored and unsupported notifications
are intentional.

**Initial matrix rows**

- `initialized`
- `view/gap`
- `session/started` (host-emitted; absent from the published notification index)
- `session/todoListChanged`
- `session/goalChanged`
- `session/branchChanged`
- `session/modelChanged`
- `session/approvalModeChanged`
- `turn/started`
- `turn/unqueued`
- `turn/retryScheduled`
- `turn/retracted`
- `turn/completed`
- `item/delta`
- `item/started`
- `item/updated`
- `item/completed`
- `approval/requested`
- `approval/request`
- `approval/updated`
- `approval/resolved`
- `userInput/requested`
- `userInput/request`
- `userInput/settled`
- `session/contextUsage`
- `session/tokenUsage`

`approval/request` and `userInput/request` are server-initiated *requests*, not
notifications; list them in a separate section with their response policy.
`turn/cancel` is a client command, not an event.

Each row should state whether the event is consumed, mapped to ACP, internally
tracked, intentionally ignored, or unsupported pending a protocol decision.

**Acceptance criteria**

- Every schema notification has a documented disposition.
- CI checks that new schema notifications require an explicit matrix decision.
- The matrix distinguishes published notifications, host-emitted extras, and
  server-initiated requests.

### 10. Truncation and large-output policy

Tool output is currently bounded for editor usability. Make truncation explicit
and configurable.

Status: **implemented.** The output bound is configurable through
`MUSE_TOOL_OUTPUT_LIMIT` (clamped to a 200-character floor), adapter cuts emit
both the human `…[truncated]` marker and machine-readable
`_meta.muse.truncated` with `source`, `originalChars`, and `retainedChars`,
and a host-saturated surface reports `source: "host"` without claiming an
adapter cut. Remaining work: head+tail retention and `item/readOutput`
fetch-through for `outputRef`.

**Work items**

- Consume the host's own truncation facts rather than only local bounds:
  `item.truncated` marks a saturated streamed surface (`agentMessage.text`,
  `reasoning.summary[*]`, `toolCall/userShell.visibleOutput`), and
  `outputRef` names stored output that can be fetched via `item/readOutput`.
- Add visible truncation metadata and original/retained length.
- Make the limit configurable.
- Consider preserving both head and tail for logs and errors.
- Never truncate approval subjects, decision-critical text, IDs, or requirement
  references.
- Add tests for Unicode boundaries and nested JSON content.

**Acceptance criteria**

- Editors can show that output was shortened.
- No truncation changes the apparent success or permission semantics of a tool
  call.

### 11. Usage and cost semantics

Keep usage forwarding accurate and make client-local estimates harder to
misinterpret.

Status: **implemented.** Host usage facts and client-local cost remain
separate: the `cost` object carries `source: adapter-estimate`,
`basis: catalog-list-price`, and `billing: false`, replay-once accounting and
rate-refresh replacement are covered by tests, and historic/unpriceable
completions are excluded. Remaining work: cached-input rate separation if the
host ever exposes cached-token counts per completion.

**Work items**

- Keep host-provided context/cumulative usage separate from derived values.
- Mark cost explicitly as a local list-price estimate in metadata.
- Omit numeric cost unless all required rate fields are available.
- Preserve replay-once accounting.
- Handle model catalog updates without resurrecting stale rates.
- Document exclusions: historic completions, unavailable rates, cached-input
  differences, taxes/discounts, regional pricing, and actual billing.

**Acceptance criteria**
- Users can distinguish host usage facts from client estimates.
- Refilling a history gap or reconnecting cannot duplicate cost.

## P2: feature breadth

Add features only where MSP can provide authoritative behavior.

### 12. Plan/todo visibility

MSP v1 already provides the authoritative signal. `session/todoListChanged`
carries the full list wholesale (`TodoItem { text, status, activeForm? }`,
status `pending|inProgress|completed|cancelled` plus open values), the resume
snapshot serves `state.todoList`, and an empty `items` array is a cleared
list. Map it to ACP `plan`/`plan_update`, matching `codex-acp`'s plan
presentation.

Status: **implemented.** `session/todoListChanged` and the resume snapshot's
`state.todoList` map to ACP `plan` updates (whole-list replacement, empty list
clears the plan, `cancelled` and unknown statuses stay `pending`). Remaining
work: none for the v1 wire shape; revisit if ACP adopts a distinct todo
surface.

**Work items**

- Fold `session/todoListChanged` by replacing the whole list on every event;
  order by `viewCursor` (the `revision` field is diagnostics-only).
- Emit ACP `plan` once and `plan_update` thereafter, mapping
  `inProgress` to an in-progress entry and unknown statuses to pending.
- Restore the plan from the resume snapshot (`state.todoList`).
- Test transitions: created, updated, completed, cancelled, cleared, and
  unknown open statuses.

**Acceptance criteria**
- Users can see long-running task progress without relying only on tool output.

### 13. Per-turn file-change report

Provide an editor-friendly summary of files changed during a turn.

**Work items**

- Identify the authoritative MSP source for changed paths. No dedicated
  file-change event exists in MSP v1; writes surface as `toolCall` items.
  `codex-acp` computes its report from a hidden read-only fork; MSP's
  `session/fork` (see item 19) is the analogous primitive if a host-derived
  report is ever built.
- Represent adds, edits, deletions, renames, and binary changes safely.
- Consider an ACP extension only after client capability negotiation.
- Ensure resumed sessions do not duplicate reports.

**Acceptance criteria**
- A user can see which files were touched in a completed turn.
- The report never infers a write that the host did not report.

### 14. Reasoning and status visibility

MSP v1 provides a native `reasoning` item kind: `summary[]` streams part-wise
via `item/delta` field `summary.<n>`, committed raw reasoning rides `text`,
and `truncated` marks server-side saturation. `codex-acp` maps its equivalent
signal to ACP `agent_thought_chunk`; do the same rather than dropping it.

Status: **reasoning implemented.** Summary parts stream as
`agent_thought_chunk` with a section break on part transitions; a completion
with no observed deltas emits the committed summary (or raw text) exactly
once, and host-side truncation is logged rather than presented as complete.
Unknown future item kinds render generically from `fallbackText` (with the
source kind in `_meta.muse.itemKind`) and stay invisible when the host
supplies no summary.

**Work items**

- Fold `reasoning` items and stream summary parts as `agent_thought_chunk`.
- Emit the committed summary (or raw text when no summary exists) exactly once
  at completion when deltas were missed.
- Respect `item.truncated`; never present saturated text as complete.
- Use non-misleading generic status when detailed content is unavailable.
- Render unknown item kinds generically from `fallbackText` rather than
  silently hiding them.
- Respect host privacy/redaction behavior.

**Acceptance criteria**
- The editor remains responsive during long turns.
- Generic status is not labeled as model reasoning.

### 15. Branch, goal, retry, and retraction state

Surface session-level state without corrupting prompt settlement.

Status: **goal and branch display implemented.** `session/goalChanged`
(including explicit `null` clears) publishes the provider-neutral `_meta.goal`
presentation on `session_info_update`; `session/branchChanged` publishes a
namespaced branch observation; both are restored from the resume snapshot.
Goal control stays deferred (experimental upstream). Remaining work:
retry/retract settlement race tests.

**Work items**
- Represent branch changes (`BranchState { branch, vcs, workspaceRoot }` from
  `session/branchChanged`; branch may be `null` on detached HEAD) in
  metadata/status.
- Publish goal display state from `session/goalChanged` (`Goal { objective,
  status, percentComplete, currentWork?, nextWork? }`) and the resume snapshot
  (`state.goal`) via `session_info_update`, mirroring the provider-neutral
  goal presentation `codex-acp` uses. Pass through out-of-contract statuses
  and >100 percentages without clamping.
- Defer goal *control* (`goal/set|pause|resume|clear`): those methods are
  absent from the stable v1 method index and masked in the transcript corpus,
  i.e. experimental; revisit only when the adapter deliberately opts into
  `experimentalApi`.
- Ensure retry/retract notifications settle or supersede affected queued ACP
  prompts.
- Add race tests against terminal `turn/completed` events.

**Acceptance criteria**
- No queued ACP request hangs or falsely reports success because a turn was
  retracted or retried.

### 16. Background tasks and the user shell

MSP v1 can represent this work: `session/userShell` (gated on the
`userShell` initialize capability) runs shell commands outside any turn;
`userShell` items carry `commandText`, `exitCode`/`exitSignal`,
`visibleOutput`, and a null `turnId`; and a `toolCall` may be durably
backgrounded (`background: true`, `backgroundInitiator: user|timeout`).

Status: **AIR async tasks implemented (display-only).** After bilateral AIR
negotiation, backgrounded tool calls mark their command card
(`_meta.jetbrains.air.asyncTasks.backgrounded`) and emit
`async_task_spawned`/`async_task_state_update`; user-shell items map to their
own shell tasks with exit facts settled from code/signal. `canStop` is
honestly false and `_session/async_task/stop` fails explicitly because MSP v1
publishes no stop primitive. Remaining work: the `userShell` host capability
request, active-task reconciliation, and stop once MSP exposes one.

**Work items**

- Map backgrounded `toolCall`/`userShell` items to the AIR async-tasks
  extension (spawned/state updates plus targeted stop) only after bilateral
  capability negotiation, as `codex-acp` does.
- Request the `userShell` host capability only when an editor feature needs it,
  and never request it by default.
- Surface `userShell` exit facts verbatim (code vs signal number); do not
  invent signal names.
- Ensure root-routed permissions apply to background work.
- Define host-shutdown and adapter-restart behavior.

**Acceptance criteria**
- Users can observe and stop qualifying long-running work without losing turn
  settlement.

### 17. Subagents

The earlier premise — wait for MSP to expose native semantics — is resolved.
MSP v1 publishes `subagent/sendMessage`, `subagent/followupTask`,
`subagent/interrupt`, `subagent/stop`, `subagent/resume`, `subagent/reopen`,
`subagent/close`, and `subagent/readResult`; `subagent`, `workflow`, and
`reminderChild` item kinds carry `subagentId`, `childSessionId`,
`controlStatus`, `result`, and transitive `usage`; and the transcript corpus
covers nested lifecycles, steering replay/rejection, and close round-trips.

Status: **native sessions implemented (spawn/state/child replay).** After
bilateral negotiation (canonical `subagents` capability or AIR's
`nativeSubagentSessions`), the adapter advertises the capability in both
protocol versions, emits idempotent `subagent_spawned` announcements with MSP
provenance and `subagent_state_update` terminals (`completed`/`failed`/
`cancelled`/`disconnected` for recovery states), and replays the child
transcript onto the child session id through one `session/read` drill-down.
Without negotiation the legacy tool cards remain. Legacy visibility:
as synthetic tool cards (`agent: objective` titles, result summaries, child
state lines) carrying `subagentId`, `childSessionId`, `controlStatus`, and
run provenance in `_meta.muse`, so no child work is silently dropped. Native
ACP subagent sessions and `subagent/*` controls remain the open work.

**Work items**

- Implement the draft ACP subagent RFD on top of MSP children after bilateral
  capability negotiation (`subagent_spawned`/`subagent_state_update`), keeping
  a tool-call-shaped representation for non-negotiating clients.
- Route child output through `childSessionId` using `session/read`/
  `view/page` drill-down rather than inventing a second protocol.
- Map `controlStatus` transitions (`accepted`, `starting`, `running`,
  `resultReady`, `closing`, `closed`, `recoveryPending`,
  `manualReconciliation`) to ACP child states; the generic item `status`
  remains the terminal authority.
- Route child approvals/user input fail-closed through the owner session;
  a child must never inherit or widen root/permission scope.
- Reconstruct the child tree on `session/load`; an unproven outcome stays
  unknown rather than being reported as success or failure.

**Acceptance criteria**
- A child agent cannot inherit or widen root/permission scope implicitly.

### 18. Context compaction visibility

MSP provides `session/compact` (with `CompactionOutcome`
`compacted|noop|failed|cancelled`), `compaction` items (`tokensBefore`,
`tokensAfter`, `outcome`, `reason`, `strategyId`, `summarizedThrough`), and
`ContextPressureLevel` (`normal|warning|blocked`) on context usage.

Status: **items and command implemented.** Compaction items surface as
think-kind tool calls with `contextCompaction` v1 provenance metadata and
token facts; a bare `/compact` prompt maps to `session/compact` and settles
the ACP prompt honestly for both `accepted` and `noop`. Remaining work: carry
the context pressure level alongside `usage_update` metadata.

**Work items**

- Surface `compaction` items as a visible think-kind tool call with `_meta`
  provenance (matching `codex-acp`'s compaction presentation) instead of
  dropping them.
- Add a `/compact` slash command mapped to `session/compact`; report `noop`
  and failure outcomes honestly.
- Carry the pressure level alongside `usage_update` where the client accepts
  metadata.
- Treat `summarizedThrough` as opaque provenance: display only, never parse it
  or use it as a cursor.

**Acceptance criteria**
- A user can see that compaction happened and whether it succeeded.
- Compaction visibility never changes turn settlement or permissions.

### 19. ACP session fork

ACP defines `session/fork` (including AIR fork-point metadata) and
`codex-acp` advertises the capability. MSP provides `session/fork` with a
`ForkCutPoint { lastTurnId }` cutting through a completed turn, plus durable
`ForkProvenance` on the new session.

Status: **implemented for message-id cut points.** Both protocol versions
advertise the fork capability; an omitted cut point maps to "all completed
turns", an AIR `messageId` point resolves through `session/read` to the
owning turn, and unresolvable or fingerprint-only points fail closed with an
explicit invalid-params error instead of silently copying extra history. The
new session is registered immediately so the fork result envelope's view
notifications are never orphaned. Remaining work: fingerprint-based cut
points (needs a hash implementation), and history replay on request.

**Work items**

- Advertise the ACP `fork` session capability and map `session/fork` to the
  MSP command with a fresh UUIDv7 `commandId`.
- Resolve AIR fork points (`messageId`, `messageFingerprint`, occurrence) by
  reading history (`session/read`) to find the owning completed turn; map
  unresolved points to an explicit invalid-params error rather than a silent
  full copy.
- Default an omitted cut point to "all completed turns".
- Rebuild the new session's view exactly like `session/resume`, and preserve
  `forkedFrom` provenance for diagnostics.

**Acceptance criteria**
- A fork reproduces the source history through the requested turn and no
  further.
- Fork failures never leave an ACP session half-registered.

### 20. Recommended values and session presentation

Small editor-facing parity items from the `codex-acp` comparison.

**Work items**

- Implement the AIR `recommendedValue` extension for the model selector from
  the catalog's `isDefault` row, after client capability negotiation; emit a
  recommendation only when the value is present among advertised options.
- Consider reasoning-effort recommendations if the host ever publishes a
  default; do not fabricate one.
- Consider lightweight session titles for `session/list` if clients render
  them; derive only from host-provided facts, never from prompt text mining.

**Acceptance criteria**
- Recommended metadata never overrides the user's current selection.
- No presentation data is inferred from content the host did not provide.

## P3: engineering hardening and maintenance

These reduce long-term maintenance cost as the protocol and test matrix grow.

### 21. JSON robustness and fuzzing

The dependency-free parser is a security- and reliability-critical component.

**Work items**

- Add parser/serializer fuzzing.
- Add differential tests against a known-good JSON implementation in CI without
  adding a runtime dependency.
- Continue testing strict numbers, Unicode surrogate pairs, depth limits, and
  malformed input.
- Add malformed-notification and malformed-server-request recovery tests.

**Acceptance criteria**
- Fuzz failures cannot panic or deadlock the adapter.
- Invalid JSON never terminates an otherwise healthy stdio connection.

### 22. Shutdown and lock robustness

Handle host death and internal lock failure deterministically.

Status: **durable host restart implemented.** A crashed host whose handshake
reported `durable` (or omitted durability, which the schema reads as durable)
is relaunched up to three times with backoff, every known session is
re-attached via `session/resume`, and pending requests are reconciled —
in-flight ACP prompts stay open because durable terminals arrive on resume.
Ephemeral or unknown profiles still fail closed, and exhausted restarts settle
all requests before exiting. Remaining work: mutex-poisoning audit and
closed-stdio tests.

**Work items**
- Audit mutex poisoning and convert it into bounded, explicit adapter errors.
- Test reader-thread exit while commands are pending.
- Test writes to a closed editor stdout and closed host stdin.
- Reap the child process and drain/capture stderr without deadlock.
- Add timeout protection around all shutdown paths.
- Add deterministic host-restart handling for durable sessions: on abnormal
  host death, respawn `muse serve`, re-run the initialize handshake, and
  re-attach known sessions via `session/resume` instead of failing every
  in-flight turn and exiting. `codex-acp` restarts its provider this way, and
  the Muse SDK defines the durability/host-death obligations the adapter must
  discharge.
- Classify live turn-waits by the handshake's durability profile: durable
  sessions recover terminals on resume; unrecognized or ephemeral profiles
  mark in-progress items terminal-unknown and refuse `commandId` replay.
- Bound the restart loop (for example, N attempts with backoff) and surface
  the classification in diagnostics.

**Acceptance criteria**
- Host or client disconnect settles all open ACP requests.
- Shutdown cannot block indefinitely on I/O or lock ownership.
- A transient host crash recovers durable sessions without duplicate turns
  or double-settled prompts; an unrecoverable crash degrades explicitly.

### 23. Single-source version metadata

Use Cargo package metadata for all adapter version strings.

**Work items**

- Replace literal `0.2.5` strings in initialization payloads with
  `env!("CARGO_PKG_VERSION")`.
- Add a test that prevents version literals from drifting.
- Include adapter and host version in diagnostics.

**Acceptance criteria**
- A release changes the reported version in exactly one source.

### 24. Installer safety and platform coverage

Keep editor installation conservative while expanding supported environments.

**Work items**

- Preserve comments, sibling settings, and rollback behavior.
- Validate replacement settings before writing.
- Add atomic write/rollback for all editor config paths.
- Add automated install/uninstall tests for more existing-file shapes.
- Formalize Windows installation and path behavior where Muse supports it.
- Document Linux arm64 limitations and provide a compatibility matrix.

**Acceptance criteria**
- A failed install never leaves partially replaced editor configuration.
- Supported OS/architecture/runtime combinations are explicit.

### 25. Diagnostic levels and log contract

Make support reports reproducible without exposing secrets or workspace data.

**Work items**

- Define concise default logs and richer opt-in diagnostics.
- Redact tokens, environment secrets, approval feedback, and file contents.
- Include protocol fingerprints, versions, method names, and event IDs.
- Document a log bundle format for bug reports.

**Acceptance criteria**
- Users can collect useful diagnostics without pasting source or credentials.

## Explicit near-term non-goals

- **Do not replace MSP with repeated `muse exec` calls.** One-shot wrappers can
  remain useful elsewhere, but they are not this adapter's architecture.
- **Do not fabricate ACP extensions** for host concepts MSP cannot authorize or
  restore.
- **Do not weaken approvals to improve automation convenience.**
- **Do not silently expand filesystem scope.**
- **Do not present local price estimates as Muse billing.**
- **Do not add runtime dependencies solely for convenience.**

## Success metrics

Track these alongside each release:

- number of schema notifications/methods with explicit compatibility decisions;
- CI coverage from the pinned MSP schema/transcript corpus;
- unresolved protocol mismatches at release;
- live-host smoke-test pass rate when a host is available;
- failed/failing/hung ACP requests after host disconnect;
- durable sessions recovered without duplicate turns after a host restart;
- duplicate approval/user-input presentations after resume;
- missed pending approvals found by reconciliation;
- resource-link escape attempts blocked;
- editor install/uninstall tests covering preserved existing settings;
- count of manually duplicated adapter version strings;
- startup diagnostics that identify the next user action.

## Suggested release checkpoints

### `v0.3.0` — Compatibility-safe MSP adapter

- Runtime schema compatibility table and diagnostics.
- Pinned schema/transcript CI corpus.
- Pending approval/user-input reconciliation.
- Method-specific server request handling.
- Method-aware command timeout policy.
- Single-source version metadata.

### `v0.3.x` — Operational reliability

- Authentication/readiness diagnostics.
- MCP policy warning and documentation.
- Root/resource-link hardening.
- Shutdown and lock robustness.
- Durable-session host restart and pending-set reconciliation.
- Event compatibility matrix.
- Explicit truncation metadata.

### `v0.4.0` — Richer editor integration

- Plan/todo visibility.
- Reasoning thought-stream visibility.
- Context compaction visibility.
- Per-turn file-change report.
- Branch/retry/retract status settlement.
- Diagnostic levels and redacted support bundles.

### `v0.5.0` — MSP-native breadth

- Native subagent sessions mapped from MSP `subagent/*` and child items.
- Background tasks and the user shell via the AIR async-tasks extension.
- ACP session fork mapped to MSP `session/fork`.
- Recommended model values from the catalog default.

### `v1.0.0` — Stable adapter

- Muse schema compatibility process proven across multiple host releases.
- Full event disposition matrix.
- Optional live-host smoke suite.
- Documented OS/runtime matrix.
- Security review of root confinement, approvals, MCP policy, and installer.
- Clear support policy for Developer Preview protocol changes.

## Maintenance policy

Update this file whenever:

- a Muse SDK schema version changes;
- a compatibility result changes;
- an intentional event disposition changes;
- an optional ACP extension is adopted or rejected;
- a security boundary changes;
- a release checkpoint is completed or redefined.

[sdk]: https://github.com/meta-models/muse-code-sdk
[docs]: https://meta-models.github.io/muse-code-sdk/
[codex-acp]: https://github.com/agentclientprotocol/codex-acp
