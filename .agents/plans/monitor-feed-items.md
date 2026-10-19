# Render Muse monitor feed items

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

Muse 1.4.8 can publish background monitor observations as `monitorFeed` transcript items. Today the adapter tolerates the kind but usually drops it or presents only a generic fallback, so an editor user cannot see what a monitor observed. After this change, a monitor opens one stable ACP tool card, streamed feed text updates that card, and a completed feed item settles it with the host's truncation fact intact.

The card title is derived from the monitor's arm-time description after removing terminal control characters and collapsing it to one bounded line. The card metadata carries the monitor task id, allowing a client to correlate it with the monitor tool call whose MSP `itemId` has the same value. Unknown future item kinds keep using the existing generic fallback path.


## Progress

- [x] (2026-10-19) Read issue #255, the vendored `monitorFeed`, `monitorId`, `monitorDescription`, `truncated`, and nullable `turnId` schema, and the upstream SDK revision.
- [x] (2026-10-19) Created branch `issue-255/monitor-feed` from `origin/master` after PR #289 merged.
- [x] Add a monitor-feed role to the session fold with stable per-monitor ACP card identity.
- [x] Render started, updated, delta, completed, gap-refill, and history-replay monitor items.
- [x] Sanitize and bound the model-controlled monitor description before it reaches a title or metadata.
- [x] Honor the host `truncated` flag on monitor text.
- [x] Add unit, fake-host, matrix, and live-host coverage on Muse 1.4.8 and 1.4.10.
- [x] Update README, ROADMAP, event compatibility, CHANGELOG, and this plan.
- [x] Run all contribution gates.
- [x] Open [PR #290](https://github.com/BrokkAi/muse-acp/pull/290).
- [x] Complete three review/fix passes.
- [ ] Merge only after final CI is green.


## Surprises & Discoveries

- Observation: `monitorId` is not the feed item's identity.
  Evidence: the schema says it is the monitor task id and equals the monitor `toolCall` item's `itemId`. A feed item still has its own distinct `itemId`.

- Observation: the schema requires clients to render feed deltas through the ordinary `item/delta` field `text`, but ACP tool-card content is a replacement snapshot.
  Evidence: MSP deltas append to a field, while `ToolCallUpdate.content` replaces the card content. The fold therefore must accumulate feed text and send the current accumulated value on each update.

- Observation: `monitorDescription` is model-controlled and allowed to saturate a 64 KiB surface.
  Evidence: the schema explicitly says to sanitize before render and includes `monitorDescription` among the surfaces `Item.truncated` covers.

- Observation: `monitorFeed.turnId` is always null because the feed can arrive outside a turn.
  Evidence: the schema names `monitorFeed` beside `userShell` and session-boundary `hookRun` as turnless kinds.

- Observation: the real 1.4.8 monitor only becomes an item feed when the monitor tool is armed with `show_lines: true`; otherwise its events arrive only as model-delivered task notifications.
  Evidence: the same shell monitor produced no `monitorFeed` frames without `show_lines`, but produced feed cards and matching `monitorId` values with `show_lines: true` on both 1.4.8 and 1.4.10.


## Decision Log

- Decision: use one stable ACP card id per `monitorId`, in the form `monitor-<monitorId>`.
  Rationale: all feed items from one monitor then update one transcript row, matching the issue's request to key presentation by the monitor identity and preventing one row per event batch. The host feed item id remains in `_meta.muse.itemId`.
  Date/Author: 2026-10-19, Codex.

- Decision: expose `_meta.muse.monitorId`, sanitized `_meta.muse.monitorDescription`, and `_meta.muse.itemKind: "monitorFeed"` on every feed card update.
  Rationale: `monitorId` lets a client correlate the feed with the monitor tool call, whose existing item metadata carries the same value as `_meta.muse.itemId`. The item kind and feed item id preserve provenance without interpreting the monitor description again.
  Date/Author: 2026-10-19, Codex.

- Decision: title the card `Monitor: <description>` and use the feed `text` as content.
  Rationale: the description is the host's arm-time label and the text is the observation. This keeps event output in expandable card content instead of using an unbounded message or title.
  Date/Author: 2026-10-19, Codex.

- Decision: replace C0/C1 controls, bidi formatting controls, and line breaks in the description with spaces, collapse whitespace, and apply the existing display truncation helper.
  Rationale: the field is model-controlled and may contain terminal escape or directionality sequences. Collapsing it into a bounded single line makes it safe for common editor title renderers.
  Date/Author: 2026-10-19, Codex.

- Decision: accumulate `item/delta.field == "text"` for a monitor and send the accumulated text as replacement card content.
  Rationale: MSP deltas append, but ACP tool card content is a full replacement. The completion snapshot remains authoritative and settles the card.
  Date/Author: 2026-10-19, Codex.

- Decision: route monitor items through dedicated fold handling, while leaving the generic unknown-kind fallback unchanged.
  Rationale: monitor items need streaming, stable per-monitor identity, and truncation metadata that generic cards do not provide. Future unrecognized kinds must retain their current tolerant behavior.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation, all contribution gates, and all three review/fix passes are complete. One stable ACP card per monitor carries safe description labeling, accumulated feed text, terminal settlement, and host truncation as `_meta.muse.truncated.source = "host"`. The real host path passes on Muse 1.4.8 and 1.4.10 when the monitor is armed with `show_lines: true`. Final CI and merge remain.


## Context and Orientation

`src/fold.rs` converts MSP item notifications into ACP `session/update` objects. `SessionFold::on_item_snapshot` handles `item/started` and `item/updated`; `on_item_delta` handles streamed field appends; `on_item_completed` handles the terminal item; and `replay_item` seeds the same state from history. `ItemRole` tracks per-item presentation state. Existing host-side cards use `card_line` to produce an ACP tool call with title, kind, status, content, `_meta.muse`, and truncation facts.

`tests/fixtures/fake_serve.py` is the scripted MSP host. `tests/acp_serve.rs` drives the real adapter binary against it. `tests/live_loopback.rs` drives a real Muse binary with a scripted model provider. `tests/protocol/stable/msp.schema.json` is the vendored protocol contract.

`ROADMAP.md` section 9 records stable-surface dispositions and the field matrix. `docs/event-compatibility.md` records notification behavior. `README.md` describes editor-visible capabilities, and `CHANGELOG.md` records user-visible changes.


## Plan of Work

Extend `ItemRole` with a monitor variant carrying the ACP card id, first-announcement state, sanitized title, base metadata, accumulated text, and host truncation fact. Add a persistent set of monitor card ids so later feed items for the same monitor update an existing card instead of creating one per event batch.

Add a `monitorFeed` branch to `host_card_parts`. It derives the description from `monitorDescription`, falling back to `fallbackText` only when the description is absent. It strips a leading durable `Monitor event: ` prefix if present, sanitizes controls and directionality characters, collapses whitespace, and truncates the resulting title using the adapter's existing bounded display policy. It returns the feed text as content and a metadata object containing `monitorId`, the sanitized description, and `itemKind`.

Handle `monitorFeed` in item snapshots, deltas, completions, and replay. The first card is a `tool_call` on ACP v1 and a `tool_call_update` on ACP v2, following existing card behavior. Later updates use the stable monitor card id and include accumulated text. Completion uses the authoritative item text and status, then releases the feed item role. The host `truncated` flag sets `_meta.muse.truncated.source = "host"`.

Add fold unit tests for sanitization, title bounding, and stable monitor card accumulation. Add a fake-host scenario that opens a feed, streams a delta, completes with host truncation, and sends a second feed item for the same monitor. Assert one stable card id, the monitor metadata, streamed content replacement, completed state, and truncation. If the live Muse tool can be scripted on 1.4.8 without credentials, add a loopback test that opens a real monitor, observes its feed card, and settles it; otherwise document the host limitation and prove the non-fabrication/compatibility behavior that remains possible.

Update README, ROADMAP section 9 and its field matrix, `docs/event-compatibility.md`, CHANGELOG, and this plan. Run all gates, open a PR with `Fixes #255`, complete three review/fix passes, and merge only after final CI is green.


## Concrete Steps

Run commands from:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Run focused tests while iterating:

    cargo fmt
    cargo test --locked monitor_feed -- --nocapture
    cargo test --locked --test acp_serve monitor_feed -- --nocapture

Probe the live host:

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback monitor_feed -- --nocapture

Run all contribution gates:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Then push, open the pull request, review the diff three times, fix real findings, and merge at a green final head.


## Validation and Acceptance

A fake-host `monitorFeed` item with description `"Build\twatcher\u001b[31m"` opens a card whose title contains `"Monitor: Build watcher"` and no control characters. The card metadata contains the monitor id and item kind. A subsequent `item/delta` appends text and the ACP card update carries the accumulated text. Completion settles the card as completed with the authoritative text. A completed feed whose item carries `truncated: true` includes `_meta.muse.truncated.source = "host"`.

A second feed item with the same `monitorId` reuses the same ACP tool call id and does not mint a second card. The `turnId: null` shape is accepted without requiring a turn correlation. Unknown item kinds still render only through their existing generic fallback path.

All unit, fake-host, live, and repository gates are green. The pull request closes #255, receives three documented review/fix passes, and is merged only at a green final head.


## Idempotence and Recovery

The fold is in-memory and disappears with the adapter session. Replayed or duplicated feed snapshots are keyed by the feed `itemId` and terminal ids are tracked in `done`, so a replay can settle without creating duplicate cards. Re-running tests is safe. A failed live-monitor probe keeps its disposable host logs for diagnosis.


## Artifacts and Notes

The expected card shape on ACP v1 is:

    {"sessionUpdate":"tool_call","toolCallId":"monitor-<monitorId>",
     "title":"Monitor: build watcher","kind":"other","status":"in_progress",
     "content":[...],"_meta":{"muse":{"monitorId":"<monitorId>",
     "monitorDescription":"build watcher","itemKind":"monitorFeed",
     "itemId":"<feedItemId>"}}}

Update and completion frames use `tool_call_update` and the same card id.


## Interfaces and Dependencies

`SessionFold::ItemRole` gains an internal monitor role. `SessionFold::new` initializes the set of monitor card ids. `host_card_parts` and a private description sanitizer produce the monitor title, content, and metadata. Public fold method signatures do not change. No dependency is added.

Change note (2026-10-19): review pass 1 corrected the history-replay test so it replays a terminal monitor item rather than an in-progress snapshot.

Change note (2026-10-19): review pass 2 added an explicit duplicate-terminal assertion for gap-refill idempotence.

Change note (2026-10-19): review pass 3 added direct coverage for fallback description stripping and the bounded long-description title.
