# Replay host cards consistently on history replay

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This document must be maintained in accordance with `.agents/PLANS.md` (repository root). Its canonical home in the repo is `.agents/plans/replay-host-cards-on-history-replay.md`; step 1 of the work copies it there.

## Purpose / Big Picture

When an editor reconnects to a Muse session (ACP `session/load`, `session/resume` with `replayFrom: {"type": "start"}`, or `session/fork`), the adapter replays the session history so the transcript looks like it did live. Today that replay silently drops most host-specific cards: monitor feeds, web searches, hook runs, workflow rows, user shells, damaged-log-range notices, scheduled-job fires, background-command completions, and any future unknown card kind. After this change, every card kind that renders live also renders when history is replayed, identically on all three replay paths, while finished background work still does not open terminals or flood the Async Tasks panel. A user sees a complete transcript after reconnect instead of one with holes.

Tracking issue: BrokkAi/muse-acp#393.

## Progress

- [x] (2026-10-27) ExecPlan written and saved to `.agents/plans/replay-host-cards-on-history-replay.md`.
- [x] (2026-10-27) Added `replaying` flag and `replay_history_items` helper to `src/fold.rs`; both `replay_item` catch-all arms now route to the live handlers (`on_item_snapshot` in-flight, `on_item_completed` settled).
- [x] (2026-10-27) Gated live-only side effects in `SessionFold::on_item_completed` on `!self.replaying`: shell terminals (toolCall + userShell) and AIR async-task announcements (backgrounded toolCall, userShell, workflow).
- [x] (2026-10-27) Switched the three replay call sites in `src/main.rs` (`replay_history`, fork, `drill_down_subagent_child`) to the shared helper.
- [x] (2026-10-27) Unit tests in `src/fold.rs` (`mod replay_tests`, 8 tests): live/replay card parity across 11 kinds, terminal suppression (shell + tool call), task suppression, in-flight registration, silent kinds, flag reset, dedupe.
- [x] (2026-10-27) Fake-host scenario `cards_load` in `tests/fixtures/fake_serve.py` plus end-to-end test `load_and_resume_replay_host_cards_without_live_side_effects` in `tests/acp_serve.rs` covering ACP v1 and v2.
- [x] (2026-10-27) Documented the replay policy in `docs/event-compatibility.md`.
- [x] (2026-10-27) Full validation: `cargo fmt --check` and `cargo clippy --locked --workspace --all-targets -- -D warnings` pass clean; `cargo test --locked --workspace --no-fail-fast` is green across all 15 test binaries except the pre-existing environmental failure `terminal_auth_method_runs_muse_acp_login` (adapter `current_exe` resolves to the mbx build cache in this container; fails identically on the pristine tree).
- [x] (2026-10-27) Merged as #406, then reviewed. Five accuracy/test-strength follow-ups fixed: honest wording for the shell content difference, parity test comparing content, shell tests that fail on silence, `fork_history_cards` end-to-end coverage for the third replay path, and save/restore of the `replaying` flag. Recorded under Outcomes & Retrospective.

## Surprises & Discoveries

- Observation: the three replay paths disagree not only on which kinds they forward but on what they assume about them. `drill_down_subagent_child` sends every non-whitelisted kind straight to `on_item_completed`, so an in-flight item of a non-whitelisted kind is settled as if completed.
  Evidence: `src/main.rs:11256-11265` branches on kind before calling either `replay_item` or `on_item_completed`, with no status check.
- Observation: the load/resume whitelist in `replay_history` drops `monitorFeed` even though `replay_item`'s in-flight and completed arms both handle it.
  Evidence: `src/main.rs:8749-8754` matches only `subagent | toolCall | userMessage | agentMessage | compaction`.
- Observation: `settle_background_completion` is already idempotent and a no-op when the task was never announced, so routing `backgroundTerminalCompletion` through `on_item_completed` during replay cannot double-settle; durable settlement stays owned by `reconcile_active_tasks`.
  Evidence: `src/fold.rs:1651-1694` returns early unless the task id is in `announced_tasks` and has no recorded terminal state.
- Observation: in-flight replay already announces active AIR tasks via `on_item_snapshot`, and `reconcile_active_tasks` skips items the fold already tracks (`!replayed || !has_active_item`), so extending replay to in-flight user shells/tool calls cannot duplicate task announcements.
  Evidence: `src/fold.rs:2921-2999` (snapshot announce), `src/main.rs:8820-8823` (reconcile guard).
- Observation: ACP v1 initialize reads client extensions from `clientCapabilities`, while ACP v2 reads `capabilities`; a fake-host test that negotiates AIR async tasks must use the per-version field name.
  Evidence: `src/main.rs:4970-4974`.
- Observation: in ACP v2 every card create is a `tool_call_update` (the `tool_call` sessionUpdate exists only for ACP v1 creates); test helpers matching card frames must accept both.
  Evidence: `src/fold.rs:942-946` (`card_line`).
- Observation: `terminal_auth_method_runs_muse_acp_login` fails in this container on the pristine tree too: the adapter reports its `current_exe`, which resolves to the mbx build cache (`/home/ryansvihla/.cache/mbx/targets/...`) instead of `target/debug/muse-acp`. Pre-existing environment artifact, unrelated to this change.
  Evidence: fails identically with the change stashed.

## Decision Log

- Decision: one shared replay helper on `SessionFold` (`replay_history_items`) becomes the single funnel for all three paths, and it sets a `replaying` flag that `on_item_completed` consults to suppress live-only side effects.
  Rationale: the issue's acceptance demands one policy so a new kind cannot be added to one path and forgotten in the others; a flag on the fold keeps the per-kind rendering logic in exactly one place (the live handlers) instead of forking it.
  Date/Author: 2026-10-27, Kimi Code CLI.
- Decision: during replay, finished user shells and shell tool calls render their card with text content instead of opening an agent-owned terminal.
  Rationale: the issue forbids opening or closing terminals for commands that ended long ago; a text card preserves title, content, and `_meta.muse` parity with everything except the terminal reference, which has no faithful replay meaning.
  Date/Author: 2026-10-27, Kimi Code CLI.
- Decision: subagent spawn lines keep rendering during replay (unchanged from today's whitelist), and `reminderChild` stays silent during replay when reminders are off, because both match live behavior for those kinds.
  Rationale: the acceptance criteria require silent-live-stays-silent and impose no change on kinds that already replay correctly.
  Date/Author: 2026-10-27, Kimi Code CLI.

## Outcomes & Retrospective

Delivered 2026-10-27. All three replay paths (load/resume, fork, subagent drill-down) now call the single `SessionFold::replay_history_items` helper, which sets `replaying` while it feeds items through `replay_item`; both of `replay_item`'s catch-all arms route to the live handlers, so every host card kind (webSearch, hookRun, workflow, userShell, monitorFeed, damagedRange, scheduledFire, backgroundTerminalCompletion, overdueNotice, sideChat, unknown kinds with `fallbackText`) renders on replay with the same title and `_meta.muse` as live. `on_item_completed` gates terminal open/close and AIR async-task announcements on `!self.replaying`, so finished shells render as text cards and finished background work produces no Async Tasks entries, while `reconcile_active_tasks` still restores still-active work exactly once (in-flight replay registers items as active, and the reconcile guard skips them). Validation: 8 new fold unit tests, a new `cards_load` fake-host scenario, and an ACP v1+v2 end-to-end test all pass; fmt/clippy clean; the full workspace suite is green except one pre-existing environmental failure unrelated to this change. The ExecPlan copy in the session plans directory is superseded by `.agents/plans/replay-host-cards-on-history-replay.md`, which carries the full decision log and discoveries.

### Review follow-ups (2026-10-27)

Post-merge review of #406 found no blocking defects but five accuracy and test-strength gaps, all fixed in the follow-up commit:

- The "byte-identical"/"renders identically" wording was wrong for finished shells, which are the one deliberate content difference (terminal live, text card on replay). Both the `replay_item` comment and `docs/event-compatibility.md` now name that exception instead of overclaiming.
- The parity unit test only compared `(title, _meta)`, so it could not have caught a content regression — including its own false claim. It now compares content too, skipping exactly the shell card, and a probe showed `host_card_items` does contain a `userShell`, which is what made the stricter assertion fail and forced the honest split.
- Both shell tests asserted only the absence of `terminal_update`, which silence would satisfy. They now also require the card to render, keep its live title, and carry text content.
- `session/fork` had no end-to-end coverage even though #393's acceptance names all three paths; its items arrive through `collect_history`, not `replay_items`. Added the `fork_history_cards` fake-host scenario and `fork_replays_host_cards_without_live_side_effects`, and confirmed it fails when the settled catch-all is restored to `_ => {}`.
- `replaying` was cleared rather than restored, so a future nested batch (a drill-down inside a replay) would have re-enabled live side effects for the rest of the outer batch. It now saves and restores the previous value, with `a_nested_batch_does_not_reenable_live_side_effects` pinning it.

Also recorded: the drill-down path previously sent non-whitelisted kinds to the live handler, which could announce a child's finished tasks into the Async Tasks panel. That behavior change went into `CHANGELOG.md` since it reaches beyond the replay paths #393 describes.

Remaining: the working tree is uncommitted; commit (or PR) is the owner's next step. Live-host verification against a real pinned Muse build (rather than the fake host) can follow on the nightly matrix.

## Context and Orientation

This repository builds `muse-acp`, an adapter binary that speaks ACP (the Agent Client Protocol) to editors on one side and MSP (Muse's session protocol) to the Muse host on the other. All transcript rendering decisions live in `SessionFold` (`src/fold.rs`), a per-session state machine that turns MSP item events into ACP session-update lines (cards, messages, terminal updates, async-task announcements). `src/main.rs` is the ACP JSON-RPC front end: it dispatches client methods, talks to the MSP host, and owns three code paths that replay history into a fold:

1. Load/resume: `replay_history` at `src/main.rs:8741`, called after `session/load` and `session/resume` with `replayFrom: {"type": "start"}`. It whitelists five item kinds and drops everything else before calling `SessionFold::replay_item`.
2. Fork: `src/main.rs:6249-6251` replays the fork's collected history through `replay_item` with no whitelist, but `replay_item` itself whitelists.
3. Subagent drill-down: `drill_down_subagent_child` at `src/main.rs:11198` replays a child transcript, again with its own kind whitelist and a fall-through that sends everything else directly to `on_item_completed`.

`SessionFold::replay_item` (`src/fold.rs:2513`) is the per-item replay entry point. It reads the item's `status` field: items whose status is `pending`, `inProgress`, `in_progress`, `running`, or `started` (`replay_is_in_flight`, `src/fold.rs:2502`) take the "snapshot" arm (the fold registers the work as active and may render a start card via `on_item_snapshot`); all other items take the "completed" arm (the fold settles the item via `on_item_completed`). Both arms currently whitelist kinds and silently drop the rest. The live event handlers `on_item_snapshot` (`src/fold.rs:2749`) and `on_item_completed` (`src/fold.rs:3170`) already handle every host card kind — hookRun, workflow, userShell, monitorFeed, reminderChild, unknown kinds with `fallbackText` — because they serve the live event stream. `on_item_completed` also performs side effects that are only correct for live traffic: it opens and closes agent-owned terminals for shell items (`note_shell`/`finish_terminal`, `src/fold.rs:2191-2312`) and announces AIR async tasks for backgrounded tool calls, user shells, and workflows. After replay, `reconcile_active_tasks` (`src/main.rs:8767`) restores the async-task inventory for work the host still reports active, calling `on_item_snapshot` only for items the fold does not already track (`has_active_item`, `src/fold.rs:731`), so replay and reconciliation compose without duplicates as long as in-flight replay registers items the same way it does today.

Terms used in this plan: "card" is an ACP `tool_call` update with a title; "AIR async task" is the ACP async-tasks extension's panel entry for background work; "host card kinds" are the MSP item kinds beyond plain messages and tool calls that the host emits (monitorFeed, webSearch, hookRun, workflow, userShell, damagedRange, scheduledFire, backgroundTerminalCompletion, overdueNotice, sideChat, reminderChild, and unknown kinds carrying `fallbackText`).

Tests: unit tests for the fold live in `src/fold.rs` under `mod corpus_tests` and siblings. Black-box tests that run the real adapter binary against a fake MSP host (`tests/fixtures/fake_serve.py`, driven by `tests/acp_serve.rs`) assert on the ACP frames the adapter emits; each test picks a fake-host scenario (for example `load` at `tests/fixtures/fake_serve.py:2190` serves inline history for `session/load`). Contribution gates are `cargo fmt --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo test --locked --workspace` (see `CONTRIBUTING.md:35-37`).

## Plan of Work

All edits are in three files plus docs and tests.

In `src/fold.rs`: add a private `replaying: bool` field to `SessionFold` (near `src/fold.rs:284`), initialized to `false` in `SessionFold::new` (`src/fold.rs:425`). Add a public method `replay_history_items(&mut self, acp_sid: &str, ver: u8, items: &[J], out: &mut Vec<String>)` next to `replay_item` that sets `self.replaying = true`, loops over `items` calling `self.replay_item(acp_sid, ver, item, out)`, then sets `self.replaying = false`. This method is the single replay policy every call site will use.

In `SessionFold::replay_item` (`src/fold.rs:2513`): in the in-flight arm, replace the catch-all `_ => {}` (at `src/fold.rs:2555`) with a call to `self.on_item_snapshot(acp_sid, ver, item, out)`, so an in-flight item of any kind is registered and start-rendered exactly as a live `item/started` would render it. In the completed arm, replace the catch-all `_ => {}` (at `src/fold.rs:2586`) with the same wrap-and-settle the explicit kinds use: build `let wrap = J::Obj(vec![("item".to_string(), item.clone())]);` and call `self.on_item_completed(acp_sid, ver, &wrap, out)`. Leave the explicit arms and the message handling untouched.

In `SessionFold::on_item_completed` (`src/fold.rs:3170`), gate the live-only side effects on `!self.replaying`, leaving card emission untouched. In the `toolCall` arm: wrap `self.note_shell(ver, &item_id, &tc_id, item)` and the `let terminal = self.finish_terminal(...)...` chain so neither runs during replay (compute `terminal` as `None` when replaying, which also makes the existing `let content = if text.is_empty() || terminal.is_some() { None } else { Some(text) }` fall back to text content), and wrap the `if backgrounded && self.air_async_tasks { ... }` block (the `backgrounded_tool_line`, `announce_async_task`, and `emit_async_task_state` calls at `src/fold.rs:3274-3288`). In the `"subagent" | "workflow" | "userShell" | "hookRun"` arm: compute the terminal only when `!self.replaying` for `userShell` (the `note_shell`/`finish_terminal` pair at `src/fold.rs:3407-3413` and the terminal follow-up update at `src/fold.rs:3430-3438`), and wrap the `userShell` AIR announcement block (`src/fold.rs:3439-3450`) and the `workflow` AIR announcement block (`src/fold.rs:3451-3475`) in `!self.replaying`. Do not gate the subagent spawn logic, the card lines themselves, `settle_background_completion`, or the `known()`/done bookkeeping.

In `src/main.rs`, switch all three call sites to the shared helper. In `replay_history` (`src/main.rs:8741`): delete the kind match and call `sess.fold.replay_history_items(&sess.acp_sid, sess.ver, &items, &mut out)` directly. Borrow checker note: `sess.fold.replay_history_items(&sess.acp_sid, sess.ver, ...)` borrows `sess` mutably and immutably at once; assign `let acp_sid = sess.acp_sid.clone();` and `let ver = sess.ver;` first, as neighboring code already does. At the fork site (`src/main.rs:6249-6251`) replace the loop with `fork_fold.replay_history_items(&new_msp, ver, &history_items, &mut replay_lines);`. In `drill_down_subagent_child` (`src/main.rs:11255-11265`) replace the kind if/else with `fold.replay_history_items(&child, ver, &items, &mut out);`. Verify with a search for `replay_item` afterwards that only `replay_history_items` and tests call it.

Tests. In `src/fold.rs`, add a `mod replay_tests` next to the existing test modules. Write focused tests, each constructing items with `parse_json` as the existing tests do: a live-vs-replay parity test that runs each host-card kind (completed `monitorFeed`, `webSearch`, `hookRun`, `workflow`, `damagedRange`, `scheduledFire`, `backgroundTerminalCompletion`, `overdueNotice`, `sideChat`, an unknown kind carrying `fallbackText`, and `userShell`) through `on_item_completed` on one fold and through `replay_history_items` on a fresh fold with `air_async_tasks` enabled, asserting the card line (the `tool_call` update) from replay carries the same title and `_meta.muse` as the live one; a terminal-suppression test asserting replay of a completed ACP v2 `userShell` and a shell `toolCall` emits no `terminal_update` frame while the live path does; a task-suppression test asserting replay of a completed backgrounded `toolCall`, a completed `userShell`, and a completed `workflow` with `air_async_tasks` enabled emits no `async_task_spawned` frame; an in-flight registration test asserting replay of an in-flight `userShell` and backgrounded `toolCall` leaves `has_active_item` true (so `reconcile_active_tasks` would skip them) and emits their start cards; a silent-kind test asserting replay of `reminderChild` (completed and in-flight, `show_reminders` false) emits no card; a flag-reset test asserting a live `on_item_completed` for a backgrounded call after a replay still announces its async task (proving `replaying` was cleared); and a dedupe test asserting replaying the same item list into the same fold twice emits nothing the second time.

Fake-host end-to-end tests. In `tests/fixtures/fake_serve.py`, add a scenario `cards_load` modeled on the `load` scenario (`tests/fixtures/fake_serve.py:2190`) whose `session/resume` result serves `{"mode": "inline", "snapshot": None, "items": [...]}` containing, in order: a completed `webSearch` item with a title-bearing shape (mirror how existing scenarios build items, using the fields `host_card_parts` reads — `query`/`results` or `fallbackText`), a completed `hookRun`, a completed `workflow` with a `workflowRunId`, a completed `userShell` with `visibleOutput` and `exitCode`, a completed unknown kind (for example `futureCard`) carrying `fallbackText`, a completed `backgroundTerminalCompletion` whose `backgroundTerminalCompletion.toolCallId` names a backgrounded call that also appears earlier in the same history as a completed backgrounded `toolCall` with `"background": true`, and one in-flight backgrounded `toolCall` (`"status": "inProgress"`, `"background": true`) so reconciliation has active work. Also include an active `userShell` (`status: "inProgress"`) if the `async_resume` scenario shows the accepted shape for one; otherwise cover the active shell only in the unit tests. In `tests/acp_serve.rs`, write a test modeled on `session_load_replays_history` (`tests/acp_serve.rs:4454`) that runs the scenario for both ACP v1 (`session/load`) and v2 (`session/resume` with `replayFrom: {"type": "start"}`), asserting: each host card's title appears in the frames after the load result; the cards carry `_meta.muse` with the right `itemKind`; no `terminal_update` frame appears for the finished `userShell`; no `async_task_spawned` frame appears for the finished backgrounded call or the finished workflow; and exactly one `async_task_spawned` frame appears for the in-flight backgrounded call (guarding against duplicates from `reconcile_active_tasks`). Reuse the existing `Client` test helper and its frame-log accessors.

Docs: in `docs/event-compatibility.md`, add a short "History replay policy" section stating that replay funnels every history item through the same fold handlers as live events, that finished shell work replays as text cards without opening terminals, that finished background work does not announce Async Tasks entries (reconciliation owns the active inventory), and that kinds silent live (reminderChild with reminders off) stay silent on replay.

Finally, copy this ExecPlan into `.agents/plans/replay-host-cards-on-history-replay.md` (single fenced-free Markdown document per `.agents/PLANS.md`) and commit it with the first code change, keeping the `Progress` section current at every stopping point.

## Concrete Steps

Working directory is the repository root throughout.

1. Copy this plan to `.agents/plans/replay-host-cards-on-history-replay.md`.

2. Implement the fold changes:

    edit src/fold.rs — add `replaying` field, `replay_history_items`, the two catch-all changes, and the `on_item_completed` gating.

3. Implement the call-site changes:

    edit src/main.rs — `replay_history` (line ~8741), fork site (line ~6250), `drill_down_subagent_child` (line ~11256).

    then verify no stray callers remain:

    grep -n "replay_item" src/main.rs src/fold.rs

    expected: only the definition and internal call in `replay_history_items`, plus test call sites.

4. Add unit tests in `src/fold.rs` (`mod replay_tests`) and run them:

    cargo test --locked -p muse-acp replay_tests

    expected: all new tests pass; run them against the pre-change code where feasible (the parity and suppression tests fail before the change because the kinds are dropped).

5. Add the `cards_load` scenario to `tests/fixtures/fake_serve.py` and the end-to-end test to `tests/acp_serve.rs`, then run:

    cargo test --locked -p muse-acp --test acp_serve cards

6. Update `docs/event-compatibility.md`.

7. Run the contribution gates:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace

## Validation and Acceptance

The behavior to verify: a session whose live transcript showed host cards shows the same cards after `session/load`, after `session/resume` with `replayFrom: {"type": "start"}`, and in a fork, with identical card titles and `_meta.muse` payloads; finished user shells and shell tool calls appear as text cards without any `terminal_update`; finished background work produces no Async Tasks entries while still-active work appears exactly once.

Concretely: `cargo test --locked -p muse-acp replay_tests` passes with at least the seven tests listed above; `cargo test --locked -p muse-acp --test acp_serve cards` passes for both ACP versions; and the three contribution gates in Concrete Steps pass clean. Before the change, the new parity test fails because replayed `webSearch`/`hookRun`/`workflow`/`userShell`/unknown-kind items emit no card at all — that failure-then-pass is the proof the fix works. Existing replay tests (`session_load_replays_history`, `resume_reattaches_at_the_retained_cursor_without_duplicate_replay`, `gap_refill_does_not_price_a_replayed_completion_twice`) must still pass unchanged, proving the five previously whitelisted kinds did not change behavior.

Downgraded history (`history.mode != "inline"`) needs no code change: `replay_items` returns `None` when items are absent, replay simply does not run, and the existing log line "resume: history downgraded to {mode}; replay may be partial" stays accurate.

## Idempotence and Recovery

Every step is additive or a narrowing of a whitelist; no state outside the process changes. Tests are repeatable. If the fake-host scenario misbehaves, delete the scenario key and the test returns to the previous behavior; the pre-existing scenarios are untouched. Rolling back is `git revert` of the commits. The `replaying` flag is strictly internal to `SessionFold` and defaults to `false`, so a half-applied change (flag without gating, or gating without the helper) only ever behaves like today's code — no new side effects can leak into live traffic.

## Artifacts and Notes

Key code excerpts the implementer will touch (current state):

    src/main.rs:8749  match kind {
    src/main.rs:8750      "subagent" | "toolCall" | "userMessage" | "agentMessage" | "compaction" => {
    src/main.rs:8751          sess.fold.replay_item(&sess.acp_sid, sess.ver, it, &mut out)
    src/main.rs:8752      }
    src/main.rs:8753      _ => {}
    src/main.rs:8754  }

    src/fold.rs:2555  (in-flight catch-all)  _ => {}
    src/fold.rs:2586  (completed catch-all)  _ => {}

New method signature to add:

    pub fn replay_history_items(&mut self, acp_sid: &str, ver: u8, items: &[J], out: &mut Vec<String>)

## Interfaces and Dependencies

No new dependencies, crates, or protocol surfaces. The only public API addition is `SessionFold::replay_history_items` in `src/fold.rs`; `SessionFold::replay_item` stays public (tests call it) but becomes an implementation detail of the helper. No changes to `acp-lite` or `muse-msp-client`. The fake-host contract (`tests/fixtures/fake_serve.py` scenario keys) gains one additive scenario, `cards_load`.
