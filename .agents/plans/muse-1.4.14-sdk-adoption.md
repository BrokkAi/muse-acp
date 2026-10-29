# Adopt the Muse 1.4.14 SDK surface and close the remaining version issues

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


Muse Code 1.4.14 is the current stable release (`1.4.14-R7309.1`), and its stable MSP schema differs from the 1.4.13 bundle already vendored in this repository. The installed binary exports fingerprint `sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6`; the matching Muse Code SDK revision is `a45cb67cd2e27c8b6b4b55dcbaae6e6341123ea4`. After this change, the adapter reports that exact host surface as tested, preserves every new stable field through an ACP representation or a documented deliberate omission, and closes the long-running 1.4.13 gaps for ManagedBash work IDs (#396) and monitor replay hints/terminal-row anchors (#397).

An editor user will be able to join a tool card and AIR background task to Muse's registered ManagedBash Work ID, see a monitor row after the host-designated anchor while preserving the open `rowKinds` vocabulary as metadata, see whether a tool call is held for approval or a hook, see monitor wake-source descriptions on the answering agent message, read earlier-history cards from checkpoint-pruned transcripts, see the host's bounded reviewer-escalation reason in permission prompts, and receive useful provider-refusal text. Session provenance from Muse 1.4.14 is forwarded as `_meta.muse`, and an ACP resume may explicitly select a model through `_meta.muse.modelId`.

To verify the work, run the focused fold and adapter tests described below, the full workspace suite, the live loopback suite against the installed stable Muse Code 1.4.14, and the nightly workflow on the branch. The selftest must show `kind=sdk-manifest ... status=tested` for fingerprint `c94c...`, and a live host handshake must classify `1.4.14` as tested.

This plan deliberately does not include the unstable ACP features in #316, #317, or #318, which depend on ACP draft capabilities rather than Muse Code versions. It also does not publish packages to crates.io (#375), which requires a release-channel and credential change. Those remain separate work.


## Progress


- [x] (2026-10-29 20:20Z) Confirmed stable channel and installed Muse Code are both `1.4.14-R7309.1`.
- [x] (2026-10-29 20:25Z) Ran the 52-test adapter live loopback suite and the standalone MSP client live test against `1.4.14`; all 53 tests passed.
- [x] (2026-10-29 20:31Z) Exported the installed host schema and matched its fingerprint to the official 1.4.14 release manifest.
- [x] (2026-10-29 20:34Z) Identified upstream SDK revision `a45cb67`, the eight new stable definitions, sixteen new properties, and the new `earlierHistory` item kind.
- [x] (2026-10-29 20:45Z) Re-copied the stable schema and transcript corpus from SDK `a45cb67`.
- [x] (2026-10-29 20:55Z) Updated compatibility classification, transcript provenance, selftest expectations, the field matrix, and the 1.4.14 nightly pins.
- [x] (2026-10-29 21:10Z) Implemented #396 work-id propagation and #397 replay-hint/anchor behavior with regression tests.
- [x] (2026-10-29 21:25Z) Mapped or explicitly disposed of every 1.4.14 definition, property, and vocabulary addition.
- [x] (2026-10-29 21:35Z) Added fold and fake-host coverage for work IDs, replay hints, held tool calls, earlier history, wake sources, reviewer reasons, session provenance, resume model selection, and request denial.
- [x] (2026-10-29 21:45Z) Updated `README.md`, `docs/acp-extensions.md`, `ROADMAP.md`, `CHANGELOG.md`, and the existing Muse 1.4.x adoption plan.
- [x] (2026-10-29 22:00Z) Ran formatting, Clippy, full workspace tests on 1.99.1 and 1.88.0, selftest, both live loopback suites, npm, Python, shell, and smoke checks.
- [ ] (2026-10-29 22:05Z) Committed, pushed, and opened PR #446. Remaining: review CI and the diff, fix findings, and merge when the required checks are green.


## Surprises & Discoveries


- Observation: Muse 1.4.14 is a substantive stable-surface advance, not a build-only patch.
  Evidence: the installed `muse schema generate-json-schema` export differs from the vendored 1.4.13 bundle and publishes fingerprint `sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6`.

- Observation: The official stable-channel manifest and installed binary agree on the exact release and schema fingerprint.
  Evidence: channel `/muse-code/channels/muse-stable` reports `1.4.14-R7309.1`, and the release manifest reports `msp_schema_fingerprint=sha256:c94c...`.

- Observation: The current live adapter and independent MSP client already tolerate 1.4.14.
  Evidence: `MUSE_ACP_LOOPBACK=1 MUSE_CLI=/home/ryansvihla/.local/bin/muse cargo test -p muse-acp --test live_loopback` passed 52/52, and the `muse-msp-client` live test passed 1/1.

- Observation: Terminal-row anchor support from #397 is already implemented on master, but the issue and ROADMAP still describe it as unsupported.
  Evidence: `src/fold.rs` reads `Item.terminalRowAfterItemId`, defers rows whose anchor is still ahead, and has tests for first-seen, missing, and same-turn anchors.

- Observation: The remaining #397 surface is `Item.replayHint`; it can reuse the same row-deferral mechanism and can be preserved verbatim in card metadata without inventing translations for its open `rowKinds` vocabulary.
  Evidence: `ReplayHint.afterItemId` has the same anchor meaning as `terminalRowAfterItemId`, while `rowKinds` is explicitly open and has no ACP equivalent.

- Observation: Several 1.4.14 fields are only sizing or authority diagnostics on `session/read`, not lifecycle facts the adapter currently exposes.
  Evidence: `Session.driveHolder`, `itemCount`, and `logBytes` are limited to `session/read`; the adapter reads child sessions only for transcript drill-down.

- Observation: Reusing the full live `_meta.muse` snapshot on `session/list` incorrectly exposed fold-only `nameOrigin` and `generatedName`.
  Evidence: the integration suite caught both fields on adapter-owned list rows. `SessionMeta::session_list_meta_json` now filters list rows to branch, attention, `resumedByClient`, and `spawnedBy`.

- Observation: A reviewer escalation reason is host-controlled display text and can contain Markdown/URL syntax.
  Evidence: rendering it directly as text could create active content in Markdown-capable permission surfaces. The reason is now sanitized into a dynamic-backtick code span while the raw bounded value remains in `_meta.muse.reviewerReason`.


## Decision Log


- Decision: Re-pin directly to SDK `a45cb67` and classify the 1.4.14 fingerprint as tested because both the official stable release manifest and the locally executed stable host agree with the SDK's exported schema.
  Rationale: The schema is byte-identical to the installed host export, and all 53 live tests already pass against that build. The nightly matrix will retain an independent pinned download so this status does not depend on one developer machine.
  Date/Author: 2026-10-29, Codex.

- Decision: For #396, publish `Item.workId` verbatim as `_meta.muse.workId` on the tool card and on the AIR `async_task_spawned` update when present, while preserving the existing v1 derivation for AIR task IDs.
  Rationale: This gives editors a stable join key without changing task-stop identity or duplicating/rekeying already seeded tasks. Absence remains unchanged for older hosts and non-ManagedBash calls.
  Date/Author: 2026-10-29, Codex.

- Decision: For #397, use `replayHint.afterItemId` as the monitor row's anchor when `terminalRowAfterItemId` is absent, and preserve the complete `replayHint` object verbatim in `_meta.muse`.
  Rationale: The anchor controls ordering, while `rowKinds` is an open display vocabulary with no direct ACP primitive. Keeping it verbatim avoids inventing wording and preserves unknown future kinds.
  Date/Author: 2026-10-29, Codex.

- Decision: Render `earlierHistory` as one synthetic card with role-labelled stored messages and verbatim facts in `_meta.muse.earlierHistory`.
  Rationale: ACP has no pruned-prefix boundary type. A card preserves the transcript tail without pretending those historical messages are new user or agent turns.
  Date/Author: 2026-10-29, Codex.

- Decision: Map held tool calls to pending ACP status, a visible waiting suffix, and `_meta.muse.held`/`heldFor`; explicitly clear those metadata members when a later snapshot releases the call.
  Rationale: Showing an approval- or hook-held call as already executing is misleading. Explicit false/null is needed because ACP `_meta` updates are merged by clients.
  Date/Author: 2026-10-29, Codex.

- Decision: Carry non-empty `wakeSourceDescriptions` verbatim on the answering agent message's `_meta.muse` rather than altering its text.
  Rationale: The schema defines these as display attribution for the client to scrub and join. Rewriting the model reply would change transcript content and lose the distinction between source metadata and response text.
  Date/Author: 2026-10-29, Codex.

- Decision: Render `ApprovalRequestParams.reviewerReason` as a sanitized `Reviewer concern: …` content line and preserve it verbatim in permission `_meta.muse`.
  Rationale: The reason is explicitly a display hint, not authorization. Showing it helps the user understand why the host escalated the call, while the title and choices remain the decision surface.
  Date/Author: 2026-10-29, Codex.

- Decision: Forward `Session.resumedByClient` and `Session.spawnedBy` as session metadata; explicitly ignore `driveHolder`, `itemCount`, and `logBytes` because they are `session/read` metadata hints the adapter does not expose.
  Rationale: Provenance fields earn a stable `_meta.muse` representation. The three read-only sizing/authority diagnostics would add no adapter behavior and are served only on a path used for child transcript drill-down.
  Date/Author: 2026-10-29, Codex.

- Decision: Support an optional `_meta.muse.modelId` string on ACP `session/resume`/`session/load` and forward it to MSP `session/resume.modelId`; reject explicit null, non-string, or empty values before the host call.
  Rationale: Muse 1.4.14 makes this an explicit recovery and model-binding override. The extension is request-scoped, additive, and maps cleanly without changing standard ACP fields.
  Date/Author: 2026-10-29, Codex.

- Decision: Explicitly ignore `SessionStartParams.previousSessionId`, `SessionStartParams.spawnedBy`, `SessionReadResult.worktree`, and `ViewPageParams.maxBytes`.
  Rationale: The adapter has no `/new` predecessor, worker-session creation, child-worktree display, or caller-supplied view-byte-budget surface. Treating them as ignored is honest and the schema makes all four additive-optional.
  Date/Author: 2026-10-29, Codex.

- Decision: Map `TurnError.requestDenied: true` to one plain provider-refusal sentence when the host supplies no better message.
  Rationale: The schema makes this a live-terminal discriminator, and the user needs to distinguish a provider refusal from a generic failed turn. Absence remains a non-assertion.
  Date/Author: 2026-10-29, Codex.

- Decision: Render `reviewerReason` as literal code-span content rather than raw Markdown.
  Rationale: The host field is a display hint, not trusted markup. A dynamic backtick fence prevents links or other active Markdown while preserving the reason visually.
  Date/Author: 2026-10-29, Codex.

- Decision: Restrict `session/list` `_meta.muse` to branch, attention, `resumedByClient`, and `spawnedBy`.
  Rationale: Live `session_info_update` needs the full metadata snapshot, but list rows are snapshots that must not carry fold-only `nameOrigin` or `generatedName`; the integration tests pin that distinction.
  Date/Author: 2026-10-29, Codex.


## Outcomes & Retrospective


The Muse-version backlog is implemented locally. Work IDs and replay hints/anchors close the remaining #396 and #397 behavior. The adapter maps reviewer reasons, held calls, earlier history, wake attribution, session provenance, resume model selection, and provider-refusal detail; it explicitly ignores the 1.4.14 metadata-read sizing/authority fields, child worktree read facts, session-start graph inputs, and page byte-budget input because it exposes no corresponding ACP surface. Every field is enforced by the ROADMAP matrix test.

Validation passed on Rust 1.99.1 and the 1.88.0 minimum: 262 adapter unit tests, 445 adapter integration tests, all `acp-lite` and `muse-msp-client` suites, 8 field-matrix tests, Clippy with warnings denied, formatting, and selftest. Selftest now prints tested `sdk-manifest` and `host-1.4.14` rows for fingerprint `c94c...`. Both real-host loopback suites pass against `1.4.14-R7309.1` (52 adapter tests and 1 client test), along with 4 Node launcher tests, 43 Python tests with 6 platform skips, shell syntax, and npm smoke.

The branch is committed and pushed as `2013af4`, and [PR #446](https://github.com/BrokkAi/muse-acp/pull/446) tracks review and merge. At the time of this handoff the required GitHub checks are still running.


## Context and Orientation


`muse-acp` is a Rust binary that bridges ACP from an editor to Muse Code's MSP wire protocol. `src/main.rs` handles ACP and MSP routing and lifecycle commands. `src/fold.rs` converts MSP items and turns into ACP session updates. `src/acp.rs` owns ACP session state and generic `_meta.muse` metadata snapshots. `src/compat.rs` classifies the MSP schema fingerprint reported by a live host.

The protocol corpus is copied from the public Muse Code SDK into `tests/protocol/`. The stable schema is `tests/protocol/stable/msp.schema.json`, its fingerprint is in `tests/protocol/stable/manifest.json`, and the golden transcripts live under `tests/protocol/transcripts/`. `tests/protocol/PROVENANCE.md` records the exact source revision. Never hand-edit the stable schema when re-pinning; copy it from the recorded SDK revision.

The field matrix is the prose table in `ROADMAP.md` between `msp-1.4.x-field-matrix` markers. `tests/msp_140_field_matrix.rs` requires exactly one row for every tracked addition and checks that each field or enum remains in the vendored schema. Every disposition must be one of `Mapped to ACP`, `Consumed`, `Internally tracked`, `Intentionally ignored`, or `Unsupported pending protocol decision`. This plan maps or explicitly ignores every 1.4.14 addition, so no unsupported row remains.

The fake MSP host is `tests/fixtures/fake_serve.py`; `tests/acp_serve.rs` drives the real adapter against it. Fold tests in `src/fold.rs` are preferred for item-shape mappings. The live suite in `tests/live_loopback.rs` runs the adapter against a real `muse serve` binary whose model calls use `tests/fixtures/loopback_provider.py`.

The 1.4.14 additions are:

- New definitions `EarlierHistory`, `EarlierHistoryMessage`, `HeldFor`, `RuntimeWorktree`, its three open-enum companions, and `SessionSpawnedBy`.
- `ApprovalRequestParams.reviewerReason`.
- `Item.earlierHistory`, `held`, `heldFor`, and `wakeSourceDescriptions`; plus the open `ItemKind.earlierHistory`.
- `Session.driveHolder`, `itemCount`, `logBytes`, `resumedByClient`, and `spawnedBy`.
- `SessionReadResult.worktree`, `SessionResumeParams.modelId`, `SessionStartParams.previousSessionId`, and `SessionStartParams.spawnedBy`.
- `TurnError.requestDenied` and `ViewPageParams.maxBytes`.


## Plan of Work


First, re-pin the corpus. Copy `schema/msp/stable/*` and `schema/msp/transcripts/*` from `/tmp/muse-code-sdk-latest` at commit `a45cb67` into `tests/protocol/`. Preserve the repository's transcript README and license handling according to `tests/protocol/PROVENANCE.md`. Update provenance to record the revision, date, 1.4.14 host build, fingerprint, and the fact that the installed 1.4.14 export is byte-identical to the SDK bundle.

Second, update version classification. In `src/compat.rs`, add `HOST_1414_FINGERPRINT` with value `sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6`, alias `SDK_MANIFEST_FINGERPRINT` to it, add the tested table and selftest cases, and update historical detail strings that point at the current vendored surface. Add the exact `1.4.14-R7309.1` artifacts to `.github/workflows/nightly.yml`: Linux x64 sha256 `0809aac7cb58d18f5022b6d05e02aa1974683c0ec430e5f04d25b60ed17de2da`, macOS arm64 `1c96114408acbeade9a8fb79e3b4425e5b64d50425938568aef01bdc04ce2e03`, and Windows x64 `20d18c426cc26e9e4b3cc07008ec07ec97e12e466a2be8cd2797b2e0e2018382`. The macOS newest-pin row moves from 1.4.13 to 1.4.14; the Linux and Windows matrices add 1.4.14.

Third, implement #396 and #397 in `src/fold.rs`. Add `workId` and `replayHint` to the tool card's `_meta.muse` fields. Extend `row_anchor` to use `replayHint.afterItemId` when `terminalRowAfterItemId` is absent. Add the work ID to the AIR `async_task_spawned` update when the live item supplied it, without changing the AIR task ID. Add tests for a monitor replay hint with an unknown `rowKinds` entry and for a ManagedBash work ID with and without the field.

Fourth, map the relevant 1.4.14 item state. Extend the fold's host-card handling with an `earlierHistory` arm that renders known `user` and `assistant` messages as a single card and keeps the facts in `_meta`. Extend tool metadata with work ID, replay hint, hold state, and held-for value. While held, force non-terminal ACP status to pending, append a visible waiting suffix to the title, and track previously held item IDs so a release sends explicit `held:false`/`heldFor:null`. Extend completed and replayed agent-message metadata with non-empty `wakeSourceDescriptions`.

Fifth, map approval and terminal diagnostics. In `src/main.rs`, build permission prompt content from both CUA facts and a sanitized `reviewerReason`; merge both raw facts into `_meta.muse`. Append a provider-refused sentence to `friendly_terminal_error` when `TurnError.requestDenied` is exactly true.

Sixth, map resume/session provenance. Add a validator for `_meta.muse.modelId` on `session/resume` and `session/load`; pass it through `resume_session` as an MSP `modelId` field on both the initial resume and MCP-conflict retry. Keep internal re-attach calls at `None`. Extend `adopt_session_projection` to forward non-null `resumedByClient` and `spawnedBy` into `_meta.muse`, retaining a previously known value when absent and clearing it on explicit null. Document the request extension in `docs/acp-extensions.md`.

Seventh, record every disposition. Add all 1.4.14 additions to the ROADMAP field matrix and `tests/msp_140_field_matrix.rs` definitions/property checks. Map the fields above; mark `Session.driveHolder`, `itemCount`, and `logBytes` intentionally ignored because they are metadata-read sizing/authority hints; mark `SessionReadResult.worktree` intentionally ignored because child reads are transcript-only; mark the two session-start graph inputs and `ViewPageParams.maxBytes` intentionally ignored because the adapter exposes no corresponding surface. Update the 1.4.13 rows for replay hint, terminal anchor, and work ID to their implemented behavior.

Eighth, cover the behavior. Add a focused fake-host scenario that emits a live ManagedBash tool item with a work ID under AIR task negotiation. Add a fake-host replay/load scenario with a monitor `replayHint` whose `afterItemId` follows a later item and whose `rowKinds` includes both a known and unknown value. Add integration cases for held/released tool calls, earlier-history cards, wake-source metadata, reviewer reason, resume model override forwarding and validation, session provenance metadata, and request-denied text. Prefer extending existing scenarios where that keeps the corpus understandable.

Ninth, document and validate. Update README only where an editor-visible extension or limitation changed. Update `docs/acp-extensions.md` with the work-id, replay-hint, held, wake-source, earlier-history, reviewer-reason, session-provenance, request-denied, and resume-model metadata. Update `CHANGELOG.md` under Unreleased. Append progress and outcome entries to `.agents/plans/muse-1.4.x-adoptions.md` and this plan. Run the focused tests while iterating, then the full contributor gate and live suite.


## Concrete Steps


Work from `/home/ryansvihla/.codex/worktrees/a003/muse-acp`. The active managed toolchain is Rust 1.99.1; run the MSRV pass with 1.88.0.

Re-pin from the already cloned upstream SDK:

    cp -R /tmp/muse-code-sdk-latest/schema/msp/stable/. tests/protocol/stable/
    cp -R /tmp/muse-code-sdk-latest/schema/msp/transcripts/. tests/protocol/transcripts/

Focused tests while iterating:

    cargo +1.99.1 test --locked --test msp_140_field_matrix
    cargo +1.99.1 test --locked fold
    cargo +1.99.1 test --locked --test acp_serve work_id
    cargo +1.99.1 test --locked --test acp_serve replay_hint
    cargo +1.99.1 test --locked --test acp_serve held
    cargo +1.99.1 test --locked --test acp_serve earlier_history
    cargo +1.99.1 test --locked --test acp_serve resume_model

Full gates:

    cargo +1.99.1 fmt --check
    cargo +1.99.1 clippy --locked --workspace --all-targets -- -D warnings
    cargo +1.99.1 test --locked --workspace
    cargo +1.88.0 test --locked --workspace
    cargo +1.99.1 run --locked -- --selftest
    MUSE_ACP_LOOPBACK=1 MUSE_CLI=/home/ryansvihla/.local/bin/muse MUSE_NO_AUTO_UPDATE=1 cargo +1.99.1 test --locked -p muse-acp --test live_loopback
    MUSE_ACP_LOOPBACK=1 MUSE_CLI=/home/ryansvihla/.local/bin/muse MUSE_NO_AUTO_UPDATE=1 cargo +1.99.1 test --locked -p muse-msp-client --test live_muse
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

The Node launcher and smoke tests must run outside the restricted filesystem sandbox in this environment because the sandbox blocks execution of their copied fixtures with `EPERM`.


## Validation and Acceptance


The re-pin is accepted when `tests/protocol/stable/manifest.json` reports fingerprint `sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6`, its schema SHA-256 is `1a0c2129f5ea35fc736f0f527d202637b86a9420a04ebdefd14ba80a58e2fd2a`, PROVENANCE names `a45cb67`, and selftest emits one tested `sdk-manifest` row for that fingerprint plus a tested `host-1.4.14` row.

For #396, a fake host emits a `toolCall` with `tool: "workspace-shell"`, `background: true`, and a ManagedBash work ID under AIR negotiation. The ACP tool card contains `_meta.muse.workId`, the `async_task_spawned` update contains the same value, and its `asyncTaskId` still follows the existing v1 derivation. Removing `workId` leaves both cards unchanged and produces no fabricated value.

For #397, a loaded/replayed monitor tool card with `replayHint: {afterItemId: "<later>", rowKinds:["reminderCheck","futureRow"]}` is withheld until the later anchor row has emitted, then appears with `_meta.muse.replayHint` containing both row kinds verbatim. A host without a replay hint keeps host-order behavior. The existing `terminalRowAfterItemId` tests continue to pass.

For held calls, the in-progress update has ACP status `pending`, a title suffix naming approval or hook, and `_meta.muse.held:true` plus `heldFor`. The release update restores the ordinary title/status and explicitly sends `held:false` and `heldFor:null`.

For earlier history, a card titled `Earlier conversation` contains only the schema's `user` and `assistant` roles in order and carries the entire `earlierHistory` object in `_meta.muse`. Unknown roles are skipped.

For wake attribution, the answering agent-message completion carries the non-empty source-description array in `_meta.muse.wakeSourceDescriptions` while its text is unchanged.

For a reviewer reason, the permission prompt content includes one sanitized `Reviewer concern: …` line and `_meta.muse.reviewerReason` carries the host string. A request without the field keeps the existing prompt.

For resume model selection, `_meta.muse.modelId: "model-x"` produces MSP `session/resume.modelId: "model-x"` and the returned model selector follows the host result. Missing metadata omits the field. Explicit null, empty, and non-string values fail locally with invalid params before any host call.

For session provenance, a lifecycle or list row carrying `resumedByClient` or `spawnedBy` publishes the raw object in `_meta.muse`; absent fields retain the last known value and explicit null clears it.

For provider refusal, a live failed terminal with `error.requestDenied:true` adds the plain provider-refusal sentence once. Absence or false does not add it.

The full workspace, MSRV, live suites, selftest, npm, Python, shell, and smoke checks must all pass before merge. The PR closes #396 and #397 and states that #316-#318 and #375 are out of scope.


## Idempotence and Recovery


The corpus copy is mechanical and can be repeated from SDK `a45cb67`; if a transcript shape changes unexpectedly, stop and compare against the upstream revision rather than editing fixtures by hand. Compatibility changes are additive. Session metadata forwarding retains prior facts on absence, so reconnects do not erase provenance. Fake-host and live tests use temporary state and do not require a Muse account. No destructive repository operation is required.


## Artifacts and Notes


Stable release evidence:

    installed version: 1.4.14-R7309.1
    stable channel manifest: 1.4.14-R7309.1
    live schema fingerprint: sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6
    upstream SDK revision: a45cb67cd2e27c8b6b4b55dcbaae6e6341123ea4
    adapter live loopback: 52 passed
    MSP client live: 1 passed

Nightly checksums:

    Linux x64:  0809aac7cb58d18f5022b6d05e02aa1974683c0ec430e5f04d25b60ed17de2da
    macOS arm64: 1c96114408acbeade9a8fb79e3b4425e5b64d50425938568aef01bdc04ce2e03
    Windows x64: 20d18c426cc26e9e4b3cc07008ec07ec97e12e466a2be8cd2797b2e0e2018382


## Interfaces and Dependencies


No new runtime dependency is required. The adapter continues to use its dependency-free `J` JSON value and the existing `muse-msp-client`.

In `src/compat.rs`, define:

    pub const HOST_1414_FINGERPRINT: &str = "sha256:c94c...";
    pub const SDK_MANIFEST_FINGERPRINT: &str = HOST_1414_FINGERPRINT;

In `src/main.rs`, extend the internal resume path so the optional model is carried explicitly:

    fn resume_session(
        host: &Arc<Hosts>,
        msp_sid: &str,
        mcp_servers: Option<&str>,
        permission_profile: Option<&str>,
        model_id: Option<&str>,
    ) -> Result<J, J>

The ACP surface is `session/resume`/`session/load` `_meta.muse.modelId`. It must be a non-empty string when present.

In `src/fold.rs`, `replayHint.afterItemId` joins `terminalRowAfterItemId` behind the existing internal row-anchor interface. No new public Rust type is required. The ACP metadata names are `workId`, `replayHint`, `held`, `heldFor`, `wakeSourceDescriptions`, and `earlierHistory` under `_meta.muse`.


## Change Notes


2026-10-29: Created from the current stable release and upstream SDK manifests after confirming the installed 1.4.14 schema, stable channel fingerprint, and complete live suite all agree. Scoped #316-#318 and #375 out as separate ACP draft and release work.

2026-10-29: Completed the implementation and all local gates. Added list-row metadata filtering after integration exposed fold-only fields, and made reviewer-reason content Markdown-safe after review.
