# Adopt the Muse 1.4.15 SDK surface

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


Muse Code 1.4.15 (`1.4.15-R8339.1`) is the current stable release. Its stable MSP schema (MSP is Muse's session protocol, the JSON-RPC wire protocol `muse serve` speaks on stdio) differs from the 1.4.14 bundle vendored in this repository. After this change the adapter reports the 1.4.15 host as tested, the nightly workflow runs the real 1.4.15 builds, every new stable definition, field, method, notification, and error has an explicit disposition, and the additions that matter to an editor user reach ACP (the Agent Client Protocol an editor such as Zed or a JetBrains IDE speaks to this adapter).

An editor user will see a readable card for Muse's new system notices, peer-session messages, subagent inputs, and consumed subagent results instead of a bare one-line summary or nothing; a Computer Use takeover prompt that names the session currently holding Computer Use instead of the internal tool name `cua_setup`; a clear message when Muse refuses a takeover decision because the lock broker is busy or unavailable; the exit code and reason of a finished background command; the model-retry hint cleared as soon as the retried model attempt starts; and the reason Muse refused a resume's explicit model choice. Session rows carry why Muse's workspace status probe produced no reading.

Muse 1.4.15 also withholds a session's MCP servers in an untrusted workspace. That was fixed separately and merged before this plan in PR #465, which launches the session hosts with `--trust-workspace`; this plan only records the evidence.

To verify, run the focused fold and adapter tests named below, the full workspace suite, `cargo run --locked -- --selftest` (which must print a tested `sdk-manifest` row and a tested `host-1.4.15` row for fingerprint `sha256:ef311d2d...`), and the live loopback suite against an installed Muse 1.4.15.


## Progress


- [x] (2026-11-12 15:30Z) Confirmed the installed Muse is `1.4.15-R8339.1`, its binary matches the official release checksum, and its `muse schema generate-json-schema` export is byte-identical to SDK `07e1548` ("remirror: 1.4.15", 2026-11-10).
- [x] (2026-11-12 15:35Z) Recorded the live handshake fingerprint `sha256:ef311d2dbf051ed9114556e14ad3980eb515104baa3409306a6817082a61d8cd`, which equals the release manifest's `msp_schema_fingerprint`.
- [x] (2026-11-12 16:10Z) Ran the live loopback suite on 1.4.15 with a 1.4.14 control; isolated the untrusted-workspace MCP withholding and the model-catalog change. Workspace trust shipped in PR #465.
- [x] (2026-11-12 18:00Z) Re-pinned `tests/protocol/stable` and `tests/protocol/transcripts` from SDK `07e1548`.
- [x] (2026-11-12 18:10Z) Added `HOST_1415_FINGERPRINT`, the `SDK_SCHEMA_SHA256` byte pin, selftest and table rows in `src/compat.rs`; the corpus test no longer requires a manifest fingerprint.
- [x] (2026-11-12 18:15Z) Added notification rows for `model/completed` and `turn/modelAttemptStarted` to `docs/event-compatibility.md`.
- [x] (2026-11-12 18:20Z) Added the 1.4.15 builds to `.github/workflows/nightly.yml`.
- [x] (2026-11-12 18:25Z) Updated `tests/protocol/PROVENANCE.md`; committed milestone 1 as `635a76c`.
- [x] (2026-11-12 19:10Z) Mapped the new item kinds, takeover approvals, decide errors, background-command exit facts, background target, parent subagent id, model-attempt start, view health, probe-degraded reason, model refusal, and monitor inbox seeding, with fold, main, and fake-host tests.
- [x] (2026-11-12 19:30Z) Recorded every 1.4.15 addition in the ROADMAP tables and `tests/msp_140_field_matrix.rs`.
- [x] (2026-11-12 19:35Z) Fixed `reasoning_selector_matches_the_live_model_catalog` for a catalog without the configured model; it passes on 1.4.14 and 1.4.15.
- [x] (2026-11-12 19:45Z) Updated README, `docs/acp-extensions.md`, CHANGELOG, and the 1.4.x adoption plan.
- [x] (2026-11-12 20:10Z) Ran fmt, Clippy, the workspace suite, selftest, npm, Python, shell, and smoke gates, and the live suite on 1.4.15 and 1.4.14.
- [ ] Commit milestone 2, open the PR, and merge when CI is green.


## Surprises & Discoveries


- Observation: The SDK manifest no longer carries the schema fingerprint.
  Evidence: SDK `07e1548` `schema/msp/stable/manifest.json` is `{"experimental": false, "schemaVersion": 1}`, and the transcript README says the fingerprint "MUST NOT be written"; the SDK derives it from the schema bytes at build. The installed host's own export has the same manifest.

- Observation: Muse 1.4.15 withholds session MCP servers in an untrusted workspace, silently.
  Evidence: the host session log records `MCP startup withheld 1 server: workspace is untrusted.` and the model gets `unknown tool mcp__editor__read_text_file`. The three editor-tool live tests pass on 1.4.14 and fail on 1.4.15, and pass on 1.4.15 with `--trust-workspace` or a saved `trust.json` entry. Trusting a parent folder does not help. Fixed in PR #465.

- Observation: Muse 1.4.15's sessionless `model/list` no longer lists a configured model that is not in the bundled catalog.
  Evidence: with the same loopback config, 1.4.14 returns `rows: ['fake-model']` and 1.4.15 returns `rows: []`, both with `source: bundledCatalog`. The adapter already keeps the current model and falls back to the fixed reasoning tiers; only the live test assumed a row.

- Observation: In this agent environment four live tests always fail for environmental reasons.
  Evidence: `an_approved_shell_command_runs_and_a_rejected_one_does_not`, `a_piped_shell_command_asks_with_every_stage`, `auto_review_decides_a_shell_approval_against_a_real_host`, and `monitor_feed_reaches_a_real_host` fail on 1.4.14 and 1.4.15 alike with `bwrap: Can't mount devpts on /newroot/dev/pts: Permission denied`, because the tool sandbox forbids user namespaces.

- Observation: Two rename live tests fail under full-suite load on 1.4.14 in this environment, before and after this work.
  Evidence: `rename_drives_a_real_host_across_the_capability_registry` and `real_host_without_generated_pair_keeps_it_unpublished` failed with the host's `commandRejected naming_unavailable` (or `CanonicalRefreshMismatch`) in both of two full runs at `3da2491`, before the workspace-trust change, and pass when run alone on both versions. They passed in every 1.4.15 run.

- Observation: The fold already had the turn-scoped clear the model-attempt start needs.
  Evidence: `SessionFold::clear_model_retry(acp_sid, Some(turn), out)` withdraws a hint only for that turn, so `turn/modelAttemptStarted` reuses it and no new fold method was added.

- Observation: No live sample of the four new item kinds was obtainable.
  Evidence: none of `systemNotice`, `peerMessage`, `subagentInput`, or `subagentResult` appears in any live run or in the SDK transcripts, and Muse 1.4.15 exposes no session tool that spawns a subagent directly. Their cards are built only from schema-documented fields, with the host's `fallbackText` as the title fallback.


## Decision Log


- Decision: Re-pin directly to SDK `07e1548` and classify fingerprint `sha256:ef311d2d...` as tested.
  Rationale: The SDK bundle is byte-identical to the installed 1.4.15 export, the release manifest and live handshake agree on the fingerprint, and the nightly matrix gains pinned 1.4.15 builds.
  Date/Author: 2026-11-12, Claude.

- Decision: Replace the manifest-fingerprint assertion with a pin of the vendored schema's SHA-256 bytes (`SDK_SCHEMA_SHA256`), and still check a manifest fingerprint when one is present.
  Rationale: The manifest no longer carries the digest. Pinning the bytes keeps the original guarantee that a re-pin which forgets `src/compat.rs` fails, without porting the SDK's fingerprint normalization; the fingerprint value itself comes from the live handshake and the release manifest.
  Date/Author: 2026-11-12, Claude.

- Decision: Render `systemNotice`, `peerMessage`, `subagentInput`, and `subagentResult` as host cards with their documented facts verbatim in `_meta.muse`.
  Rationale: The host emits them unconditionally. The generic path showed only `fallbackText` (or nothing), losing a peer message's body and the notice severity. Cards match how the adapter already renders `damagedRange`, `scheduledFire`, and `earlierHistory`. An unknown `inputPhase` skips the card, as the schema directs, and an unknown `resultStatus` is treated as absent.
  Date/Author: 2026-11-12, Claude.

- Decision: Title a `computerUseTakeover` approval `Take over Computer Use from <session label>`, add the holder's acquired and last-activity times as content, and carry `holder` verbatim in `_meta.muse`.
  Rationale: Without it the prompt read `cua_setup` with "Take over" and "Keep" buttons and never named whose Computer Use session would be interrupted. The label is host display text, so it is sanitized like other one-line titles.
  Date/Author: 2026-11-12, Claude.

- Decision: Report `approvalResolving` (-32059) and `approvalBrokerUnavailable` (-32058) on `approval/decide` with specific agent text, and log `ApprovalResolvedParams.abortReason`; do not retry.
  Rationale: The decide runs after the ACP prompt already settled, so there is nothing to re-ask; a specific sentence tells the user why their choice did not land. A blind retry could double-decide and the broker condition has no bounded wait.
  Date/Author: 2026-11-12, Claude.

- Decision: Treat `turn/modelAttemptStarted` as the end of a model-retry backoff: clear a showing `_meta.muse.modelRetry` for that turn, and otherwise consume it silently.
  Rationale: The hint describes a wait that is over once the retried attempt starts; leaving it until the turn's terminal misreports the state. Logging every attempt start as "unhandled" was noise.
  Date/Author: 2026-11-12, Claude.

- Decision: Carry `Item.backgroundTarget` on tool cards with an explicit `null` once it disappears, and `Item.parentSubagentId` on subagent cards.
  Rationale: The schema says presence means a live background candidate and absence means none; clients merge `_meta`, so a release needs an explicit null, as with `held`.
  Date/Author: 2026-11-12, Claude.

- Decision: Forward `Session.probeDegradedReason` in `_meta.muse` on lifecycle and list rows, cleared by an omitting streamed row like `branch`.
  Rationale: It is the session index's last derived value, like `branch`, and says why workspace status is unavailable.
  Date/Author: 2026-11-12, Claude.

- Decision: Return the experimental `modelIdRefusal` from `session/resume` as `_meta.muse.modelIdRefusal` on the ACP load/resume response and log it.
  Rationale: The adapter opts into `experimentalApi` and forwards `_meta.muse.modelId`; when Muse keeps the recorded model the editor should learn why. The model selector already follows the host's `session.modelId`.
  Date/Author: 2026-11-12, Claude.

- Decision: Seed the fold's monitor-inbox counts from `SnapshotState.monitorInbox` on resume.
  Rationale: The counts on a monitor card were built only from live inbox notifications, so a resumed session showed none until the next event.
  Date/Author: 2026-11-12, Claude.

- Decision: Intentionally ignore `model/complete`, `model/cancel`, `model/completed`, `session/declareExternalWait`, `session/clearExternalWait`, `subagent/list`, `Session.externalWait`, `SessionReadResult.deliveredInputs`, `SessionReadResult.prunedSubagents`, `SessionResumeResult.prunedSubagents`, `RuntimeWorktree.cleanupReason`, `RuntimeWorktree.cleanupDiagnostic`, and the reserved `mxp` capability.
  Rationale: The adapter has no ACP surface for sessionless completions, external-wait declarations, a roster read, read-only session drill-down facts, or worktree cleanup, and `mxp` is reserved. Each gets a matrix row so a future need is visible.
  Date/Author: 2026-11-12, Claude.


## Outcomes & Retrospective


The adapter now classifies Muse 1.4.15 as tested (`selftest` prints tested `sdk-manifest` and `host-1.4.15` rows for `sha256:ef311d2d...`, and a live 1.4.15 adapter logs `status=tested`), the corpus is SDK `07e1548`, and the nightly matrix carries the 1.4.15 builds. Every 1.4.15 method, notification, error, capability, definition, and field has a disposition, enforced by the notification-matrix test and `tests/msp_140_field_matrix.rs`. Editors get cards for system notices, peer messages, subagent inputs, and consumed subagent results; a takeover prompt that names the holding session; explained takeover decide refusals; an earlier model-retry clear; background-command exit facts; and the new `_meta.muse` facts.

Validation on Rust 1.99.1: 283 adapter unit tests, 464 adapter integration tests, all other workspace suites, 9 field-matrix tests, Clippy with warnings denied, formatting, selftest, 4 npm launcher tests, 43 Python tests (6 platform skips), shell syntax, and npm smoke. Live loopback against 1.4.15-R8339.1: 48 of 52, the four failures being the sandbox-bound shell and monitor tests. Against 1.4.14-R7309.1 the same four fail plus, under load, the two rename tests that fail identically before this work.

What remains: the four new item kinds were mapped from the schema alone, so their cards should be checked against a real 1.4.15 session that produces them; the subagent card still shows its `drained` line beside a `subagentResult` card rather than drawing one or the other.


## Context and Orientation


`muse-acp` is a Rust binary that bridges ACP from an editor to MSP. `src/main.rs` routes ACP requests and MSP notifications and owns approval, resume, and session lifecycle code. `src/fold.rs` turns MSP items (the host's transcript rows: messages, tool calls, subagents, and host cards) into ACP `session/update` notifications; `host_card_parts` builds the title, content, and `_meta.muse` fields for host-side item kinds, and unknown kinds fall to its last arm, which renders only `fallbackText`. `src/compat.rs` classifies a host's schema fingerprint. `src/msp.rs` launches `muse serve`. `src/acp.rs` holds per-session state and the `_meta.muse` snapshot sent in `session_info_update`.

The protocol corpus in `tests/protocol/` is a verbatim copy of the SDK's `schema/msp/stable` and `schema/msp/transcripts`; `tests/protocol/PROVENANCE.md` records the source revision. The field matrix is the table between `msp-1.4.x-field-matrix` markers in `ROADMAP.md`, enforced by `tests/msp_140_field_matrix.rs`; the notification matrix is in `docs/event-compatibility.md`, enforced by a test in `src/compat.rs`. The fake MSP host is `tests/fixtures/fake_serve.py`, driven by `tests/acp_serve.rs`; the live suite is `tests/live_loopback.rs`.

The 1.4.15 stable additions are: methods `model/complete`, `model/cancel`, `session/declareExternalWait`, `session/clearExternalWait`, `subagent/list`; notifications `model/completed`, `turn/modelAttemptStarted`; errors `approvalBrokerUnavailable` (-32058) and `approvalResolving` (-32059); the reserved capability `mxp`; item kinds `peerMessage`, `subagentInput`, `subagentResult`, `systemNotice`; enum values `HistoryNoneReason.projectionRebuildFailed` and `SessionViewHealth.healthy`; and the fields `ApprovalResolvedParams.abortReason`, `ApprovalSubject.holder`, `BackgroundTerminalCompletion.exitCode` and `reason`, `ForkProvenance.isRewind`, `Item.backgroundTarget`, `deliveredBeforeTurnEnd`, `inputPhase`, `noticeKind`, `noticeOrigin`, `noticeSeverity`, `noticeSubject`, `parentSubagentId`, `peerMessage`, `resultStatus`, `RuntimeWorktree.cleanupDiagnostic` and `cleanupReason`, `Session.externalWait` and `probeDegradedReason`, `SessionReadResult.deliveredInputs` and `prunedSubagents`, `SessionResumeResult.prunedSubagents`, and `SnapshotState.monitorInbox`, with 32 new definitions behind them. Open vocabularies also grew: `ApprovalSubject.kind` gained `computerUseTakeover` and `ApprovalChange.kind` gained `resolving`. Nothing was removed or newly required.


## Plan of Work


First, re-pin and classify (done above): copy the SDK corpus, add the fingerprint and byte pin to `src/compat.rs`, add nightly pins, add notification rows, and update `tests/protocol/PROVENANCE.md`.

Second, map behavior. In `src/fold.rs`, add `host_card_parts` arms for the four item kinds; add exit code and reason lines to the `backgroundTerminalCompletion` card; carry `backgroundTarget` (with an explicit clear) and `parentSubagentId`; add a `seed_monitor_inbox` method and a `clear_model_retry` path. In `src/main.rs`, handle `turn/modelAttemptStarted`, split the `session/viewHealthChanged` log by health, build the takeover approval title and content in `approval_title_kind` and the permission content builder, report the two decide errors, log `abortReason`, forward `probeDegradedReason`, add `modelIdRefusal` to the resume response meta, and seed the inbox from the resume snapshot.

Third, record dispositions in `ROADMAP.md` and `tests/msp_140_field_matrix.rs`, fix the live catalog test, and update README, CHANGELOG, and `.agents/plans/muse-1.4.x-adoptions.md`.


## Concrete Steps


Work from the repository root. Re-pin from a clone of the SDK at `07e1548`:

    rm -rf tests/protocol/stable tests/protocol/transcripts
    cp -R <sdk>/schema/msp/stable tests/protocol/stable
    cp -R <sdk>/schema/msp/transcripts tests/protocol/transcripts

Focused tests while iterating:

    cargo test --locked -p muse-acp --bin muse-acp compat
    cargo test --locked -p muse-acp --bin muse-acp fold
    cargo test --locked --test msp_140_field_matrix
    cargo test --locked --test acp_serve

Full gates:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    MUSE_ACP_LOOPBACK=1 MUSE_CLI=<muse 1.4.15> cargo test --locked -p muse-acp --test live_loopback
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Validation and Acceptance


`cargo run --locked -- --selftest` prints `kind=sdk-manifest ... fingerprint=sha256:ef311d2d... status=tested` and a tested `kind=host-1.4.15` row. A live 1.4.15 adapter log shows `schema-compat ... status=tested detail=validated against live host 1.4.15-R8339.1`.

Fold tests show: a `systemNotice` card titled from its text with severity, origin, kind, and subject in `_meta.muse.systemNotice`; a `peerMessage` card `Message from <sender>` whose content is the body; a delivered `subagentInput` card and no card for an unknown phase; a `subagentResult` card with its status; a background-command card with `Exit code 2` and the reason; and a tool card whose `backgroundTarget` is sent and later cleared with `null`. Adapter tests show a takeover prompt titled `Take over Computer Use from Fix login flow` with `_meta.muse.holder`, a decide refused with -32059 producing the resolving sentence, `turn/modelAttemptStarted` clearing a model-retry hint, and the resume response carrying `_meta.muse.modelIdRefusal`.

The live suite against 1.4.15 passes except the four sandbox-bound tests listed in Surprises when run inside a sandbox that forbids user namespaces.


## Idempotence and Recovery


The corpus copy is mechanical and repeatable from SDK `07e1548`; never hand-edit it. All mappings are additive: a host before 1.4.15 sends none of the new fields, kinds, or notifications, and every path is a no-op without them.


## Artifacts and Notes


Release evidence:

    installed: Muse Code 1.4.15 (1.4.15-R8339.1)
    live handshake fingerprint: sha256:ef311d2dbf051ed9114556e14ad3980eb515104baa3409306a6817082a61d8cd
    release manifest msp_schema_fingerprint: (same)
    schema sha256: 66a8840d5768fc2fce6b5ad764aedbdd259da95cd849f587278a206a596537ee
    SDK revision: 07e1548c55c268d2bc91966e565a01425d4a9f90

Nightly checksums from the release manifest:

    Linux x64:   abe0a72878c62b570c6eed6c17c5fc3c520a990ecf040511741c00092312a3d5
    macOS arm64: 76665eda8ff06b586a02642ae981d68b5d23a5263fabbe4aded0b8c90d1e92c9
    Windows x64: fd8693d726474ca7413721d2bcdfc48db343d7dab8203d493524d7535b7a426b


## Interfaces and Dependencies


No new dependency. `src/compat.rs` gains `pub const HOST_1415_FINGERPRINT`, `SDK_MANIFEST_FINGERPRINT` aliases it, and `pub const SDK_SCHEMA_SHA256` pins the bundle bytes. `src/fold.rs` gains `pub fn seed_monitor_inbox(&mut self, entries: Option<&J>)`; `turn/modelAttemptStarted` reuses the existing `clear_model_retry`. `src/main.rs` gains `takeover_holder_text`, `approval_failure_text`, and `resume_result_meta` (which replaces `blocked_prompt_meta`), and `session_info_row` takes the host row and reads `LIST_ROW_FACTS`. The ACP-visible additions are `_meta.muse.systemNotice`, `peerMessage`, `subagentInput`, `subagentResult`, `holder`, `backgroundTarget`, `parentSubagentId`, `probeDegradedReason`, and `modelIdRefusal`.


## Change Notes


2026-11-12: Created after the 1.4.15 review and the workspace-trust fix (PR #465). Records the re-pin already done and the mapping decisions for the remaining additions.
