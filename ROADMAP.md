# muse-acp roadmap

This is a living roadmap for `muse-acp`. It records the direction that keeps the
adapter close to Muse Session Protocol (MSP), safe around approvals and file
access, and useful in real editor workflows.

- **Last revised:** 2026-09-10
- **Baseline:** `v0.2.5`
- **Protocol sources:** [Muse Code SDK][sdk] and [Muse Code Developer Docs][docs]
- **Comparable adapter used for feature benchmarking:** [`codex-acp`][codex-acp]

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

**Work items**

- Record tested schema version/fingerprint pairs.
    - Include the host protocol version, adapter release, and support status.
    - Distinguish tested, warning, and incompatible combinations.
- Add the observed and expected identifiers to startup or self-test logs.
- Produce an actionable mismatch message before unrelated method failures occur.
- Decide separately whether fingerprint mismatch is fatal or degraded.
- Avoid relying only on a single source snapshot: the SDK repository and docs
  can briefly publish different schema fingerprints.

**Acceptance criteria**

- A host with an unknown fingerprint still yields a clear compatibility warning.
- Every supported branch has a machine-readable compatibility result.
- Self-test output includes adapter version, host version, schema version, and
  schema fingerprint.

### 2. Continuous conformance against the Muse schema corpus

The SDK publishes generated MSP types, a schema bundle, and recorded transcripts.
The Rust adapter should consume those artifacts as conformance inputs.

**Work items**

- Vendor a pinned SDK/schema revision under `tests/protocol/` or fetch a locked
  revision in CI.
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

**Work items**

- Call the pull endpoint after session resume/load and view attachment.
- Compare pending records against already displayed requests.
- Preserve current deduplication for replayed user-input forms.
- Define reconciliation behavior when a notification and pull result disagree.
- Ensure no pending request can be lost because a replay was missed.

**Acceptance criteria**

- Dropping an approval/user-input notification still leaves the request visible
  after reconciliation.
- Replayed and pulled copies of the same request are displayed once.
- Stale requirements cannot satisfy a later approval stage.

### 4. Method-specific handling of server-initiated requests

Do not return a generic `{}` result for every server-initiated MSP request.

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

- `session/todoListChanged`
- `session/goalChanged`
- `session/branchChanged`
- `session/modelChanged`
- `session/approvalModeChanged`
- `turn/started`
- `turn/unqueued`
- `turn/retryScheduled`
- `turn/retracted`
- `turn/cancel`
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

Each row should state whether the event is consumed, mapped to ACP, internally
tracked, intentionally ignored, or unsupported pending a protocol decision.

**Acceptance criteria**

- Every schema notification has a documented disposition.
- CI checks that new schema notifications require an explicit matrix decision.

### 10. Truncation and large-output policy

Tool output is currently bounded for editor usability. Make truncation explicit
and configurable.

**Work items**

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

Map Muse todo or plan notifications to the best supported ACP representation.

**Work items**

- Inventory all MSP todo/plan item shapes.
- Choose native ACP plan support, a compatible extension, or a conservative
  synthetic item representation.
- Preserve state on resume.
- Test transitions: created, updated, completed, failed, retried, and retracted.

**Acceptance criteria**
- Users can see long-running task progress without relying only on tool output.

### 13. Per-turn file-change report

Provide an editor-friendly summary of files changed during a turn.

**Work items**

- Identify the authoritative MSP source for changed paths.
- Represent adds, edits, deletions, renames, and binary changes safely.
- Consider an ACP extension only after client capability negotiation.
- Ensure resumed sessions do not duplicate reports.

**Acceptance criteria**
- A user can see which files were touched in a completed turn.
- The report never infers a write that the host did not report.

### 14. Reasoning and status visibility

Expose model reasoning/status only when MSP provides an appropriate signal and
the client can represent it.

**Work items**

- Identify MSP item/notification fields that indicate planning, reasoning, web
  search, terminal work, retries, or waiting.
- Map to supported ACP events where available.
- Use non-misleading generic status when detailed content is unavailable.
- Respect host privacy/redaction behavior.

**Acceptance criteria**
- The editor remains responsive during long turns.
- Generic status is not labeled as model reasoning.

### 15. Branch, goal, retry, and retraction state

Surface session-level state without corrupting prompt settlement.

**Work items**
- Represent branch changes in metadata/status.
- Define goal semantics or explicitly defer them.
- Ensure retry/retract notifications settle or supersede affected queued ACP
  prompts.
- Add race tests against terminal `turn/completed` events.

**Acceptance criteria**
- No queued ACP request hangs or falsely reports success because a turn was
  retracted or retried.

### 16. Background tasks

Investigate whether MSP can represent long-running/background tasks.

**Work items**

- Inventory task/item lifecycle and stop/cancel semantics.
- Add native ACP extension support only after bilateral capability negotiation.
- Ensure root-routed permissions apply to background work.
- Define host-shutdown and adapter-restart behavior.

**Acceptance criteria**
- Users can observe and stop qualifying long-running work without losing turn
  settlement.

### 17. Subagents

Do not emulate subagents until MSP exposes native semantics.

**Work items**

- Track the ACP subagent proposal.
- Identify MSP concepts for separate histories, permissions, routing, and
  lifecycle.
- Add bilateral capability negotiation before enabling native behavior.
- Keep a conservative tool-call representation only if it does not obscure
  permission boundaries.

**Acceptance criteria**
- A child agent cannot inherit or widen root/permission scope implicitly.

## P3: engineering hardening and maintenance

These reduce long-term maintenance cost as the protocol and test matrix grow.

### 18. JSON robustness and fuzzing

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

### 19. Shutdown and lock robustness

Handle host death and internal lock failure deterministically.

**Work items**
- Audit mutex poisoning and convert it into bounded, explicit adapter errors.
- Test reader-thread exit while commands are pending.
- Test writes to a closed editor stdout and closed host stdin.
- Reap the child process and drain/capture stderr without deadlock.
- Add timeout protection around all shutdown paths.

**Acceptance criteria**
- Host or client disconnect settles all open ACP requests.
- Shutdown cannot block indefinitely on I/O or lock ownership.

### 20. Single-source version metadata

Use Cargo package metadata for all adapter version strings.

**Work items**

- Replace literal `0.2.5` strings in initialization payloads with
  `env!("CARGO_PKG_VERSION")`.
- Add a test that prevents version literals from drifting.
- Include adapter and host version in diagnostics.

**Acceptance criteria**
- A release changes the reported version in exactly one source.

### 21. Installer safety and platform coverage

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

### 22. Diagnostic levels and log contract

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
- Event compatibility matrix.
- Explicit truncation metadata.

### `v0.4.0` — Richer editor integration

- Plan/todo visibility.
- Per-turn file-change report.
- Branch/retry/retract status settlement.
- Reasoning/status mapping where safely supported.
- Diagnostic levels and redacted support bundles.

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
