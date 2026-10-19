# Surface native reviewer approval holds

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

Muse 1.4.9 can report `approval/reviewing` when the host's native reviewer is still deciding whether an approval should be presented. This is activity, not a request: the approval is absent from the host's pending set, no editor decision is possible yet, and the eventual release arrives as `approval/requested` or `approval/resolved` for the same approval id.

The adapter currently logs that event as unhandled. After this change, an editor can show that an approval is under native review without opening a permission prompt or implying that the user has something to decide. The hint is reversible: it clears when the approval becomes actionable or resolves without a prompt. Existing editors that ignore `_meta` continue to behave exactly as before.

This does not change the adapter-side Auto-review setting. Native host review happens before approval presentation; Auto-review remains a separate adapter policy that can evaluate an approval only after `approval/requested` arrives.


## Progress

- [x] (2026-10-19) Read issue #271, the vendored `approval/reviewing` schema, the upstream SDK fold, the adapter approval paths, Auto-review queue, and existing metadata-update helpers.
- [x] (2026-10-19) Created branch `issue-271/approval-reviewing` from current `origin/master`.
- [x] (2026-10-19) Tracked native-reviewing approvals per ACP session and approval identity.
- [x] (2026-10-19) Published reversible `_meta.muse.approvalReviewing` metadata without opening a permission request.
- [x] (2026-10-19) Cleared the hint on `approval/requested`, `approval/resolved`, and the owning turn terminal.
- [x] (2026-10-19) Added fake-host tests for release, resolution, replay, child-session identity, terminal cleanup, and Auto-review independence.
- [x] (2026-10-19) Added live Muse 1.4.10 coverage.
- [x] (2026-10-19) Updated the README, ROADMAP, event matrix, CHANGELOG, and Auto-review documentation.
- [x] (2026-10-19) Ran all contribution gates successfully.
- [ ] Open a pull request that closes #271.
- [ ] Complete three review/fix passes and merge only when the final head's required checks are green.


## Surprises & Discoveries

- Observation: `approval/reviewing` uses the full `ApprovalRequestParams`, but the SDK fold turns it into only `{kind: "approvalReviewing", approvalId}` and never inserts a pending approval.
  Evidence: `clients/sdk-ts/src/fold/session-fold.ts` at revision `ae46934`, `#approvalReviewing`.

- Observation: the real Muse 1.4.10 shell-approval path currently emits `approval/reviewing`, then `approval/requested`, and the adapter logs the first as unhandled.
  Evidence: the `tool_display_label_reaches_a_real_host` investigation and existing live approval logs both showed `unhandled MSP notification: approval/reviewing`.

- Observation: approval IDs are only unique per MSP session; child streams can reuse a parent's id.
  Evidence: `AcpSession::approval_seen` and `approval_resolved` already key approvals by `(msp session id, approval id)`.


## Decision Log

- Decision: publish one `_meta.muse.approvalReviewing` array per ACP session, containing the approvals currently held by a native reviewer.
  Rationale: an array handles concurrent reviews and permits an unambiguous clear with `null` when the last review ends. It is structured metadata, so no permission card is created.
  Date/Author: 2026-10-19, Codex.

- Decision: include `sourceSessionId`, `approvalId`, `title`, and `kind` in each metadata row.
  Rationale: the pair of source session and approval id is the actual identity, and title/kind let a client render useful activity without exposing the complete approval subject.
  Date/Author: 2026-10-19, Codex.

- Decision: clear the hint immediately before `approval/requested` enters normal approval handling and on `approval/resolved`.
  Rationale: requested means the host released the decision to this client; resolved means no decision will arrive. Both end native-review activity.
  Date/Author: 2026-10-19, Codex.

- Decision: clear all held reviews for a turn when its `turn/completed` terminal arrives.
  Rationale: a missing release/resolution frame must not leave a stale session hint after the work has ended.
  Date/Author: 2026-10-19, Codex.

- Decision: do not treat native reviewing as adapter Auto-review activity and do not add it to `approval_seen`.
  Rationale: the host has not presented a decision request yet. Suppressing the later `approval/requested` would break the normal flow, and adapter Auto-review must remain independent from host-native review.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation and focused/live validation are complete. The reviewed outcome will be recorded after the three review passes and merge.


## Context and Orientation

`src/main.rs` handles both editor ACP requests and host MSP notifications. `approval/requested` passes through `open_approval`, which may hand the request to adapter Auto-review or display an ACP `session/request_permission`. `approval/resolved` withdraws a displayed or queued decision and records the durable terminal. The new `approval/reviewing` arm must remain beside those paths but must not call `open_approval`.

`src/acp.rs` owns `AcpSession`, including pending permissions, queued approvals, and the sets used to deduplicate requests and remember resolved terminals. It also contains helpers that emit `session_info_update` frames with namespaced `_meta.muse` facts.

The fake MSP host is `tests/fixtures/fake_serve.py`; `tests/acp_serve.rs` drives the real adapter binary and inspects emitted ACP frames. `tests/live_loopback.rs` runs a real Muse binary against a scripted loopback model provider. A shell tool that requires approval reaches the real native-reviewing path on Muse 1.4.9+.

`ROADMAP.md` section 9 and `docs/event-compatibility.md` currently classify `approval/reviewing` as intentionally ignored. `docs/auto-review.md` explains adapter-side Auto-review. `README.md` describes the user-visible approval surface.


## Plan of Work

Add a `ReviewingApproval` value to `src/acp.rs` with source MSP session id, approval id, title, kind, and owning turn id. Add a vector of these values to `AcpSession`, initialized empty at every construction site.

Add an ACP helper that serializes the current vector as `session_info_update._meta.muse.approvalReviewing`, or `null` when empty. Rows should be stable in first-observed order and carry `sourceSessionId`, `approvalId`, `title`, and `kind`; the owning turn id stays internal for terminal cleanup.

Extract the existing approval subject title/kind projection from `open_approval` into a helper so `approval/reviewing` renders the same human title but does not duplicate or diverge from permission presentation. Do not include choices, raw arguments, or stage evidence in the reviewing metadata.

Handle `approval/reviewing` in `handle_msp`. Route the source MSP session through `owner_for_msp_session`, ignore already-resolved approvals, insert or replace the review row keyed by `(source session, approval id)`, log the target, and emit the updated metadata only when the vector changes. Never call `open_approval`, `enqueue_review`, send `requires_action`, or create `PendingPerm`.

Clear a matching review row at the start of `open_approval`, after routing and validating the approval id, before duplicate or Auto-review returns. Clear a matching row in `approval/resolved` after routing. In `turn/completed`, remove rows whose source session and turn id match the terminal and emit the updated metadata. In all cases, emit only when state changes.

Add fake-host scenarios for reviewing followed by requested, reviewing followed by resolved without a request, duplicate/replayed reviewing, and a child session reusing a parent approval id. Assert that reviewing creates no permission request; requested still opens normally; resolved settles without a card; reclaim release clears metadata; duplicate frames do not duplicate rows. Add an Auto-review independence test showing that a host-reviewing notice does not consume the later request and that adapter Auto-review still receives the released approval.

Add a live 1.4.9+ test using the shell approval flow. Wait for the native-reviewing metadata, verify no permission request exists before release, then wait for the normal permission prompt or typed gate outcome and verify the metadata clears. Use the existing Auto-review live coverage to show the adapter-side path remains separate.

Update README approvals, ROADMAP's 1.4.9 disposition paragraph and `approval/reviewing` row, `docs/event-compatibility.md`, `docs/auto-review.md`, and CHANGELOG.


## Concrete Steps

Run all commands from the repository root:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Run focused integration tests while iterating:

    cargo test --locked --test acp_serve approval_reviewing -- --nocapture

Run the live native-review test:

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback native_approval_reviewing -- --nocapture

Before opening the pull request, run every contribution gate:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Validation and Acceptance

On `approval/reviewing`, the ACP stream receives a `session_info_update` whose `_meta.muse.approvalReviewing` contains the source session id, approval id, title, and kind. No `session/request_permission` appears, no pending permission is stored, and no adapter Auto-review job starts.

A later `approval/requested` with the same identity clears that row, then follows today's behavior exactly: Auto-review may claim it, or the editor receives the permission request. A later `approval/resolved` clears the row and records the terminal without opening a prompt. A turn terminal clears any remaining rows for that turn. Duplicate reviewing frames do not duplicate rows; a child and parent approval with the same approval id remain distinct.

Against Muse 1.4.9 or later, a real shell approval exposes the native-reviewing metadata before the permission release, then clears it. The full repository gates remain green.


## Idempotence and Recovery

All changes are additive on a feature branch, and every test has isolated host state. Replayed reviewing frames are idempotent by `(source session, approval id)`. If a live host does not emit native reviewing for a particular approval mode, use a shell approval that does and verify the adapter's own Auto-review separately; do not infer reviewing from timing. Do not force-push over review feedback. Rollback is a return to `origin/master` before merge.


## Artifacts and Notes

The metadata shape is:

    "_meta": {
      "muse": {
        "approvalReviewing": [
          {
            "sourceSessionId": "<MSP session>",
            "approvalId": "<id>",
            "title": "<human title>",
            "kind": "<ACP tool kind>"
          }
        ]
      }
    }

When no native reviews remain, the array member is explicitly `null`.

Validation transcripts and review findings will be added as work proceeds.

Local validation on 2026-10-19:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
        unit: 166 passed
        acp_serve: 366 passed
        fuzz corpus: 8 passed
        live_host: 1 passed
        live_loopback: 46 passed (disabled unless MUSE_ACP_LOOPBACK=1)
        msp_130_matrix: 4 passed
        msp_140_field_matrix: 5 passed
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs: 4 passed
    python3 -m unittest discover -s scripts -p 'test_*.py': 36 passed, 6 skipped
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Focused live test against Muse 1.4.10:

    native_approval_reviewing_reaches_a_real_host_and_clears: passed


## Interfaces and Dependencies

In `src/acp.rs`, add a private or crate-visible `ReviewingApproval` value and:

    pub fn send_approval_reviewing(
        stdout: &StdoutShared,
        acp_sid: &str,
        reviews: &[ReviewingApproval],
    )

Extend `AcpSession` with a `Vec<ReviewingApproval>` keyed uniquely by `(source_session_id, approval_id)`.

In `src/main.rs`, add helpers to:

    observe_approval_reviewing(...)
    clear_approval_reviewing(...)
    clear_turn_approval_reviewing(...)

No new crates or MSP methods are required.


Revision note (2026-10-19): Initial ExecPlan for issue #271 after tracing the native-review and Auto-review paths.
