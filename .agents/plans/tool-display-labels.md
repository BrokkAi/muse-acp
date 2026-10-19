# Prefer Muse's tool display labels

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

Muse 1.4.9 lets every tool call carry the tool's own presentation text in `Item.displayLabel`. The object contains three one-line strings: `active` for an in-progress call, `done` after a successful call, and `neutral` for failed, rejected, timed-out, cancelled, or otherwise non-successful states. The adapter currently ignores this field and synthesizes every title from the tool name and arguments.

After this change, an editor user sees the title Muse intended for the call and sees it change from the active wording to the done or neutral wording as the item status changes. A pre-1.4.9 host, a missing field, or an empty label keeps today's synthesized title. History replay and background/async-task cards use the same selected label, so they do not disagree with the live card.

To see it working, run the focused fold tests and fake-host integration test in Validation and Acceptance. A live Muse 1.4.9 or later loopback test drives a real tool call and verifies that the ACP card carries the host-selected label rather than the adapter's fallback.


## Progress

- [x] (2026-10-19) Read issue #272, the vendored `ToolDisplayLabel` schema, `SessionFold::tool_title`, snapshot/completion/replay paths, async-task title reuse, and current title tests.
- [x] (2026-10-19) Created branch `issue-272/tool-display-labels` from `origin/master`.
- [x] (2026-10-19) Selected `active`, `done`, or `neutral` from `displayLabel` by item status and preserved the synthesized fallback.
- [x] (2026-10-19) Added unit coverage for all three forms, status transitions, empty/missing labels, replay, and async-task naming.
- [x] (2026-10-19) Added fake-host active/done/neutral coverage and a live Muse 1.4.10 shell-label test.
- [x] (2026-10-19) Updated the ROADMAP field matrix, README, and CHANGELOG.
- [x] (2026-10-19) Ran all local contribution gates successfully.
- [x] (2026-10-19) Opened [PR #284](https://github.com/BrokkAi/muse-acp/pull/284) to close #272.
- [x] (2026-10-19) Completed three review/fix passes: covered missing selected members and absent status, moved the live proof from approval-only behavior to a real `write_todos` fold card, and corrected stale evidence text.
- [ ] Merge only when the final head's required checks are green.


## Surprises & Discoveries

- Observation: all tool-card paths already converge on `SessionFold::tool_title(item)`.
  Evidence: `on_item_snapshot`, `on_item_completed`, `replay_item`, and the async-task announcement path all obtain the same `(title, tool)` pair from that helper.

- Observation: the vendored 1.4.9 stable schema defines the label precisely, but the checked-in golden transcript copies do not all carry the new field.
  Evidence: `tests/protocol/stable/msp.schema.json` defines `ToolDisplayLabel`; searching parsed toolCall items in the vendored transcripts finds no `displayLabel`. The live test therefore supplies evidence from a real 1.4.9+ host.


## Decision Log

- Decision: select the label by MSP item status: `inProgress` uses `active`, `completed` uses `done`, and every other status uses `neutral`.
  Rationale: this is the schema's stated selection rule and avoids implying success on failed, rejected, timed-out, or cancelled calls.
  Date/Author: 2026-10-19, Codex.

- Decision: treat a missing, null, non-string, or whitespace-only selected label as absent and fall back to the existing synthesized title.
  Rationale: the field is additive-optional and the adapter must remain compatible with older hosts. An empty title would make the tool card unusable.
  Date/Author: 2026-10-19, Codex.

- Decision: return only the visible title from the host label; keep the raw tool name separately for ACP `kind` mapping and fallback metadata.
  Rationale: `displayLabel` is presentation text, not a replacement tool identity.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation and all three review passes are complete on PR #284. Tool cards now select host-provided active/done/neutral labels by status, all non-tool-card paths retain the synthesis, and live/replay/async-task paths share the same selection. The final remote-check result will be appended before merge.


## Context and Orientation

`src/fold.rs` turns MSP item snapshots into ACP transcript updates. `ItemKind.toolCall` items carry a stable tool name, raw arguments, a lifecycle status, and optional display facts. `SessionFold::tool_title` currently prefers `commandText`, `displayText`, or `summary`, then a useful argument, and formats `tool: detail`. It returns both the visible title and the raw tool name.

The same helper is used by live snapshots, completions, history replay, and AIR async-task announcements. Updating it therefore fixes all card paths without introducing a second title policy.

The stable schema bundle under `tests/protocol/stable` is pinned from Muse SDK revision `ae46934`. Its `ToolDisplayLabel` definition requires `active`, `done`, and `neutral`. The roadmap field matrix in `ROADMAP.md` records a disposition for each Muse 1.4.x field and is enforced by `tests/msp_140_field_matrix.rs`.

The fake MSP host is `tests/fixtures/fake_serve.py`; integration tests in `tests/acp_serve.rs` inspect actual ACP frames. The live suite in `tests/live_loopback.rs` runs a real Muse binary against a local scripted model provider.


## Plan of Work

Change `SessionFold::tool_title` to first ask a new helper for the host label selected by `item.status`. If the selected member is a non-empty string, return it as the visible title. Otherwise use the current synthesized title unchanged.

Add focused fold tests for:

- `inProgress` selecting `active`.
- `completed` selecting `done`.
- `failed`, `cancelled`, and an unknown status selecting `neutral`.
- A live status transition from active to done producing a new title on the update line.
- Missing, null, malformed, and whitespace-only labels falling back to the existing synthesis.
- Replay using the host label.
- An AIR async-task card naming the host label.

Add a fake-host scenario that emits a tool call with `active` and then `done` labels, asserting the ACP title changes at both statuses. If a second neutral terminal is useful, include a failed/neutral sibling without disturbing the turn terminal.

Add a live 1.4.9+ loopback test using a tool call that the real host labels. Observe the ACP tool card frames and assert that a host-provided label is used. Because exact wording belongs to Muse and may evolve, the test should derive the assertion from observable stable behavior rather than hardcoding a future-specific sentence: use a fake-host test for exact active/done/neutral strings and use the live test to prove a real 1.4.9+ tool item carries a label that wins over the adapter's synthesized `tool: args` fallback.

Update ROADMAP's `Item.displayLabel` field row from unsupported to mapped, describe the status selection and fallback, update the README tool-card section, and add an Unreleased CHANGELOG entry naming #272.


## Concrete Steps

Run all commands from the repository root:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Run focused unit tests while iterating:

    cargo test --locked tool_display_label -- --nocapture

Run the fake-host integration:

    cargo test --locked --test acp_serve tool_display_label -- --nocapture

Run the live test:

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback tool_display_label -- --nocapture

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

An in-progress tool call with `displayLabel.active` renders that active text as the ACP tool-card title. A completed call renders `.done`; failed, rejected, timed-out, cancelled, and unknown statuses render `.neutral`. An updated item replaces the earlier title through a normal `tool_call_update`.

Absent, null, malformed, and empty labels preserve the existing synthesized title. History replay and AIR async-task announcement use the same selected label as the live call. Tool identity and `kind` still come from the raw tool name.

Against a real Muse 1.4.9 or later host, a live tool card uses a host-provided display label rather than the synthesized `tool: args` fallback. The full repository gates remain green.


## Idempotence and Recovery

All changes are additive on a feature branch and every fake/live test uses isolated temporary state. Re-running the suite is safe. If a live host emits no label for a chosen tool, select a tool the host actively labels; do not weaken the fake-host exactness or infer a label that was absent. Do not force-push over review feedback. Rollback is a return to `origin/master` before merge.


## Artifacts and Notes

The schema rule is:

    active  -> status inProgress
    done    -> status completed
    neutral -> every other status

The selected string must be one line of plain text. The adapter still needs the raw tool name for `tool_kind`.

Validation transcripts and review findings will be added as work proceeds.

Local validation on 2026-10-19:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
        unit: 161 passed
        acp_serve: 359 passed
        fuzz corpus: 8 passed
        live_host: 1 passed
        live_loopback: 44 passed (disabled unless MUSE_ACP_LOOPBACK=1)
        msp_130_matrix: 4 passed
        msp_140_field_matrix: 5 passed
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs: 4 passed
    python3 -m unittest discover -s scripts -p 'test_*.py': 36 passed, 6 skipped
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Focused live test against Muse 1.4.10:

    tool_display_label_reaches_a_real_host: passed
    observed active title on a real write_todos card: write_todos

Review/fix commits:

    4c36158 Pin missing display-label fallbacks
    6cc5ecc Exercise display labels on a real tool card
    dd69f7e Align display-label evidence with the tested path


## Interfaces and Dependencies

In `src/fold.rs`, add a private helper equivalent to:

    fn tool_display_label(item: &J) -> Option<&str>

and have `tool_title(item)` consult it before synthesizing. No public protocol methods, crates, or ACP extension fields are added.


Revision note (2026-10-19): Initial ExecPlan for issue #272 after tracing all tool-title paths.
