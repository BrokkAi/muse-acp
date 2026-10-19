# Publish the ACP extension contract and adopt standard protocol surfaces

This ExecPlan is a living document. The sections `Progress`,
`Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must
be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the
repository root.


## Purpose / Big Picture

ACP client authors currently have to reconstruct Muse extensions from feature
bullets, the MSP event matrix, and source code. After this change, one
consumer-facing document will describe every negotiated extension and `_meta`
contract, with direction, shape, negotiation, fallback, and lifecycle context.

The same change moves two surfaces to protocol-native ACP forms. Context
compaction becomes standard `compaction_update` for ACP v2 and for ACP v1
clients that advertise `session.compaction`; older v1 clients retain the
existing tool-card fallback. MSP `Item.tool` reaches standard ACP
`ToolCall.name`.

ACP v2 treats `session_info_update._meta` as a top-level patch. Muse currently
publishes independent partial objects under one `muse` key, so one update can
erase another fact in a conforming client. The adapter will track the complete
current Muse metadata object and publish it on every session-info update.


## Progress

- [x] (2026-10-19) Read issue #294, the current README extension material, ACP
  v1/v2 schemas, metadata-patch rules, compaction contract, tool-name surface,
  and the adapter's fold/update paths.
- [x] (2026-10-19) Add the consumer-facing extension contract and README links.
- [x] (2026-10-19) Adopt standard ACP compaction with v1 capability fallback.
- [x] (2026-10-19) Map MSP tool identity to standard ACP tool `name`.
- [x] (2026-10-19) Make Muse session-info metadata v2-patch safe.
- [x] (2026-10-19) Add fold, fake-host, and documentation coverage, including
  compaction replay, unknown status, standard tool names, and complete
  metadata patches.
- [x] (2026-10-19) Run all contribution gates, including focused live-host
  coverage for standard tool names and compaction compatibility.
- [x] (2026-10-19) Open PR #295.
- [x] (2026-10-19) Complete four review passes. Passes 1-3 found and fixed
  compaction noop semantics, streamed-row metadata clearing, and duplicate
  lifecycle adoption. Pass 4 found no new problems, ending the review loop.
- [x] (2026-10-19) Merge PR #295 after final CI.
- [x] (2026-10-19) Address post-merge review follow-up: ordinary
  `session/list` pull rows again retain live metadata on omitted fields, and
  the contract documents explicit nested-null clears accurately.


## Surprises & Discoveries

- Observation: current ACP defines `compaction_update` and
  `compaction_summary_chunk`; ACP v1 gates them on
  `clientCapabilities.session.compaction`, while ACP v2 needs no capability.
  Evidence: the current official ACP v1/v2 schema and Session Compaction RFD.

- Observation: current ACP stabilizes `ToolCall.name` for the programmatic
  tool name, while the adapter only derives display titles.
  Evidence: ACP v1/v2 `ToolCall`/`ToolCallUpdate` define optional `name`; the
  adapter's `ToolUpdate` has no corresponding field.

- Observation: ACP v2 `session_info_update._meta` is a top-level-key patch and
  does not deep-merge a nested namespace.
  Evidence: current ACP v2 Extensibility documentation says keys omitted from
  the patch retain stored values, but a concrete key replaces its stored value
  whole. Independent Muse fields currently each send a new `muse` object.

- Observation: the README's Protocol extensions section is useful for
  negotiated `_session/*` methods but not a complete wire contract for
  `_meta`.
  Evidence: `permissionProfile`, approval-reviewing rows, retry hints, message
  marks, provenance, usage metadata, and tool-card facts are documented in
  separate feature bullets.

- Observation: existing clients rely on explicit nested nulls to clear
  approval-reviewing, foreground, model-retry, and subscription facts.
  Evidence: replacing a partial `muse` patch with a complete object initially
  omitted cleared keys and failed eight integration tests. The final snapshot
  includes nested nulls as ordinary data while ACP's top-level patch replaces
  the complete `muse` value.


## Decision Log

- Decision: place the public contract in `docs/acp-extensions.md`.
  Rationale: `README.md` remains the entry point, but a dedicated document can
  own wire shapes, negotiation, fallback, and compatibility without turning
  the README into a protocol reference.
  Date/Author: 2026-10-19, Codex.

- Decision: use standard ACP compaction whenever supported and keep the current
  think-kind tool card only as a fallback.
  Rationale: standard clients can preserve compaction as a timeline entity;
  v1 clients that did not negotiate the capability must not receive an unknown
  standard update.
  Date/Author: 2026-10-19, Codex.

- Decision: publish MSP `Item.tool` as ACP `ToolCall.name` on every actual tool
  call create/update.
  Rationale: `name` is the standard programmatic identity; `title` remains the
  localized/display label and `_meta.muse.itemKind` remains useful only for
  synthetic or future host-card kinds.
  Date/Author: 2026-10-19, Codex.

- Decision: keep one public `_meta.muse` namespace and emit the complete current
  Muse object on each `session_info_update`.
  Rationale: changing every key to a new top-level name would break existing
  consumers. Full snapshots preserve the public shape while satisfying ACP v2
  top-level patch semantics.
  Date/Author: 2026-10-19, Codex.

- Decision: do not migrate persistent Muse hints to ACP `notice`.
  Rationale: notices are fire-and-forget and cannot be cleared; retry,
  foreground, and approval-reviewing rows require observable replacement and
  clearing semantics.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation is complete. `docs/acp-extensions.md` now anchors the public
contract, standard compaction and tool names are on the wire, and session-info
updates publish complete Muse metadata snapshots with explicit nested null
clear markers. Canonical usage annotations now live under the `muse` namespace
with the earlier flat keys retained for compatibility. The full Cargo suite,
clippy, format, focused live-host tests, selftest, npm, Python, installer, and
package smoke gates pass. PR #295 merged after the reviewed CI run.

PR #295 completed four review passes:

- pass 1 preserved `noop` as `_museNoop` instead of falsely claiming
  compaction completed;
- pass 2 made streamed list-row omissions clear canonical metadata;
- pass 3 unified lifecycle and streamed-row adoption and removed duplicate
  projection work;
- pass 4 found no new problems and ended the review loop.

The post-merge follow-up requires its own final CI and merge.

Post-merge review found two real regressions in the first delivery. The pull
path shared the streamed-row clear helper, so routine `session/list` requests
could erase live status, attention, and branch facts. It now uses a separate
additive row path; only `session/listChanged` rows retain replace-and-clear
semantics. The extension contract also changed the reviewer/retry/foreground
clear descriptions from omitted keys to the explicit nested `null` values the
wire actually emits. A fake-host regression test pins the pull-row behavior.


## Context and Orientation

`src/main.rs` negotiates ACP capabilities, handles lifecycle requests, and
routes MSP notifications into the fold in `src/fold.rs`. The fold emits ACP
`session/update` frames, including tool cards and compaction presentation.
`src/acp.rs` owns reusable frame builders and the `AcpSession` state.

ACP v1 client capabilities live under `initialize.params.clientCapabilities`;
ACP v2 capabilities live under `initialize.params.capabilities`. ACP v1
compaction requires `clientCapabilities.session.compaction: {}`. ACP v2 has no
capability gate.

MSP compaction is an `Item` with `kind: "compaction"`. Its `outcome` is
`compacted`, `noop`, `failed`, or `cancelled`, and optional token/reason facts
provide useful provenance. MSP does not expose a user-visible retained summary
on this surface, so the adapter must omit ACP `summary` rather than inventing
one.

ACP v2 metadata patches replace the stored value for each top-level `_meta`
key. `AcpSession` will therefore hold the authoritative current Muse metadata
members. Existing frame builders and fold output will update that map and
serialize the complete `muse` value.


## Milestones

The documentation milestone adds `docs/acp-extensions.md` and a prominent
README link. It is complete when a client author can find the supported
extension methods and every public `_meta` key from the README in two clicks,
and the contract states direction, negotiation, fallback, and ACP version
behavior.

The protocol-native milestone adds ACP compaction capability tracking and the
standard tool `name` field. It is complete when fold and fake-host tests prove
v2 standard compaction, v1 negotiated compaction, v1 fallback, no invented
summary, and tool create/update names.

The metadata-patch milestone centralizes current Muse session-info metadata in
`AcpSession`. It is complete when tests prove that branch, status, attention,
title annotations, provenance, approval-reviewing, foreground, and retry facts
can update independently without erasing previously published Muse fields.

The delivery milestone runs format, clippy, the full Cargo suite, selftest,
launcher, Python, installer, and package smoke gates; opens a PR with
`Fixes #294`; performs review passes until the first clean pass (at most five),
fixing every finding before the next pass; and merges only from a green final
CI head.


## Plan of Work

Create `docs/acp-extensions.md` with sections for negotiated methods, client
opt-ins, request metadata, result metadata, session-info metadata, tool/message
metadata, usage metadata, compatibility-only fields, and standard ACP mappings.
Link it from the README introduction and replace the sentence after the
Protocol extensions list with a direct pointer to the new contract.

Add compaction capability detection in `src/main.rs`. Store it in a
connection-scoped atomic and copy it into each `SessionFold` from
`fresh_fold`. Add `compaction_supported` to `SessionFold`. Change the compaction
handler to emit standard `compaction_update` when supported and the existing
think-kind tool card otherwise. Put host outcome/token/reason provenance under
`_meta.muse.contextCompaction`, but omit ACP `summary`.

Add `name` to `fold::ToolUpdate` and emit it from `tool_line`. Populate it from
the non-empty `Item.tool` in snapshot and completion paths. Keep synthetic host
cards unchanged because they have no tool programmatic identity.

Add current Muse session metadata storage to `AcpSession` in `src/acp.rs`.
Provide helpers to set, remove, and serialize it. Refactor
`send_session_meta`, `send_session_provenance`,
`send_session_status`, `send_approval_reviewing`, and `send_session_title` to
update the stored object before sending. Intercept fold-generated
`session_info_update` frames before output, absorb their `muse` patch, and
replace it with the complete current object. Preserve non-Muse `_meta` keys
such as `jetbrains` and keep goal outside the Muse object.

Update README, ROADMAP, CHANGELOG, and the ACP compatibility documentation.
Add fold unit tests, fake-host integration tests for both ACP versions and
fallbacks, schema conformance fixtures if required, and a metadata accumulation
test that simulates a conforming v2 client applying top-level patches.


## Concrete Steps

Run commands from:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Focused validation:

    cargo fmt
    cargo test --locked compaction -- --nocapture
    cargo test --locked tool_call -- --nocapture
    cargo test --locked session_info -- --nocapture
    cargo test --locked --test acp_serve compact -- --nocapture

All gates:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Validation and Acceptance

ACP v2 receives `compaction_update` with stable `compactionId`, mapped
`in_progress`/`completed`/`failed`/`cancelled` status, optional human-readable
`error`, and complete Muse provenance metadata. ACP v1 receives the same only
when it advertises `clientCapabilities.session.compaction: {}`; without it,
the existing tool card is emitted. No path invents a compaction summary.

Actual MSP tool-call create and update frames carry standard `name` whenever
`Item.tool` is non-empty. Synthetic cards are unchanged.

A conforming ACP v2 client applying top-level `_meta` patches retains all
unrelated Muse session-info fields after branch, status, title annotation,
provenance, approval-reviewing, foreground, and retry updates. Non-Muse
namespaces remain untouched.

`docs/acp-extensions.md` is linked from the README and covers every public
extension key, negotiation, fallback, lifecycle direction, and standard mapping.


## Idempotence and Recovery

All changes are additive at the wire level. Older clients retain the fallback
or ignore new standard fields. Metadata snapshots are rebuilt from session
state and are idempotent. Tests use isolated homes and can be rerun safely.


## Artifacts and Notes

Expected standard compaction fragment:

    {"sessionUpdate":"compaction_update","compactionId":"it-c1","status":"completed",
     "_meta":{"muse":{"contextCompaction":{"version":1,"outcome":"compacted",
     "tokensBefore":12000,"tokensAfter":8000}}}}

Expected tool fragment:

    {"sessionUpdate":"tool_call","toolCallId":"call-1","name":"bash",
     "title":"bash: pwd","kind":"execute","status":"in_progress"}


## Interfaces and Dependencies

`src/fold.rs` gains `SessionFold.compaction_supported`,
`ToolUpdate.name`, and standard compaction framing. `src/main.rs` gains v1
compaction capability detection and fold interception for complete metadata
snapshots. `src/acp.rs` gains the current Muse metadata store and centralized
publishing helpers. No new dependency is added.
