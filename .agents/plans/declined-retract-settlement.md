# Keep declined retractions open until the turn terminal

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

`muse-acp` translates Muse's MSP session events into ACP updates for editors. When a user presses Stop, the adapter sends `turn/interrupt` with `retract: true` for a prompt-owned turn. If the turn is interrupted before assistant output is committed, Muse may also durably retract the user's submission. It reports that result as `turn/retracted` with `outcome: "accepted"` or `"declined"`.

Muse 1.4.10 clarifies that a declined retract means the submission stood. The interrupt still stops the turn, but the turn remains live until its ordinary `turn/completed` terminal. The adapter currently treats every retract as final and settles the ACP prompt as cancelled immediately. That can hide the real terminal, clear the active turn too early, and break a second Stop or steering request during the gap.

After this change, an explicit `declined` outcome keeps the prompt, active turn, background-check hint, and retry hint intact until `turn/completed`. The editor receives one ordinary transcript note explaining that Muse declined to retract the submitted message. A later completion settles the prompt normally. Accepted, absent, and unknown outcomes keep the existing final-retract behavior, as required by the schema's open enum and additive-optional rules. A declined retract for a turn the adapter no longer tracks is logged and ignored, matching the SDK fold.

To see it working, run the focused fake-host tests in Validation and Acceptance. They emit both live ordering classes: a declined retract while the prompt is still tracked, and a terminal-first declined retract after committed output. A live loopback test cancels a committed tool-call turn and verifies that Muse 1.4.10 reports the declined outcome after the terminal and leaves the already-settled prompt alone.


## Progress

- [x] (2026-10-19) Read issue #273, the adapter's `turn/retracted` and `session/cancel` paths, the vendored schema, and the upstream Muse SDK fold at revision `ae46934`.
- [x] (2026-10-19) Confirmed the SDK rule: only exact `declined` preserves the prior turn state; absent and unknown outcomes read as accepted.
- [x] (2026-10-19) Created branch `issue-273/declined-retract` from `origin/master`.
- [x] (2026-10-19) Branched `turn/retracted` handling on the exact declined outcome and kept prompt-owned state open.
- [x] (2026-10-19) Sent one user-visible decline notice and logged the standing submission's command id.
- [x] (2026-10-19) Added fake-host coverage for running-first and terminal-first declined ordering, steering, and a second Stop.
- [x] (2026-10-19) Added live 1.4.10 coverage using a committed tool call; the real host emits the terminal first and the adapter ignores the later declined event.
- [x] (2026-10-19) Updated the 1.4.x field matrix, event compatibility matrix, roadmap, README, and changelog.
- [x] (2026-10-19) Ran all local contribution gates and both focused live cancellation tests successfully.
- [ ] Open a pull request that closes #273.
- [ ] Complete three review/fix passes and merge only when the final head's required checks are green.


## Surprises & Discoveries

- Observation: the schema says a rejected retract does not undo the interrupt. The turn therefore remains in the running lifecycle only until the interrupt's normal terminal arrives; it does not mean the stop request failed.
  Evidence: `TurnInterruptParams.retract` in `tests/protocol/stable/msp.schema.json` says "A rejected retract does not undo the interrupt."

- Observation: the SDK fold explicitly preserves a held turn's prior state on `declined`, records the retract's command id, and creates no state for an unseen turn. It treats absent and unknown outcomes as accepted.
  Evidence: `clients/sdk-ts/src/fold/session-fold.ts` at `ae46934`, `#turnRetracted`, and its corresponding tests at `clients/sdk-ts/test/session-fold.test.ts`.

- Observation: the existing fake-host `declined` test models no terminal after the retract. That is not the real decline lifecycle and would leave a fixed adapter waiting forever.
  Evidence: `tests/fixtures/fake_serve.py` scenario `retracted` emits only `turn/retracted`; `tests/acp_serve.rs` currently expects the prompt to settle immediately in `declined_retract_still_settles_the_prompt_cancelled`.

- Observation: real Muse 1.4.10 emits `turn/completed(cancelled)` before the declined `turn/retracted` when a committed tool call prevents rollback. The prompt is already settled at that point, so the adapter must ignore the late decline instead of emitting a hold notice.
  Evidence: `declined_retract_after_a_committed_tool_call_keeps_the_terminal_settlement` passed and the adapter log recorded `outcome=declined` followed by `declined turn/retracted ignored: no tracked prompt`.


## Decision Log

- Decision: exact `outcome: "declined"` preserves the tracked prompt and active turn; everything else follows the accepted/final-retract path.
  Rationale: this matches the vendored schema's open-enum rule and the SDK fold. Keep the branches narrow so an unknown future outcome does not accidentally create a stuck prompt.
  Date/Author: 2026-10-19, Codex.

- Decision: keep the ACP prompt open through a declined retract and settle it only from `turn/completed` (or another existing terminal/reconciliation path).
  Rationale: the host's interrupt still ends the turn, but only its terminal is authoritative. Settling at the retract would clear steering and Stop targeting while the host turn still exists.
  Date/Author: 2026-10-19, Codex.

- Decision: send one visible transcript note per in-flight turn when a decline keeps the prompt open.
  Rationale: the user pressed Stop and expects the prompt to close. The note explains that Muse retained the message and that the host terminal will settle the running prompt. A per-in-flight flag suppresses duplicate notices from replayed frames.
  Date/Author: 2026-10-19, Codex.

- Decision: do not invent a new ACP extension or state-update shape for retractions.
  Rationale: ACP has no retract primitive. The prompt lifecycle already represents the important outcome, and an adapter-authored agent message follows existing diagnostic-note behavior.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation is in progress. The completed outcome will be recorded after the three review passes and merge.


## Context and Orientation

The adapter entry point is `src/main.rs`. It owns the ACP-to-MSP event loop. `session/cancel` is an editor notification rather than a request. For each foreground turn it sends `turn/interrupt`, adding `retract: true` when an ACP prompt owns the turn. The host later emits `turn/retracted` for the retract result and `turn/completed` for the turn terminal.

`AcpSession` in `src/acp.rs` holds `active_turn` and `in_flight` prompts. `InFlight` maps a Muse turn id to the ACP request id and per-turn metadata. The adapter uses `active_turn` for exact-turn steering and cancellation. Removing an `InFlight` entry settles the editor's prompt; ACP v1 gets a result with a stop reason, while ACP v2 gets an idle state update.

`turn/retracted` is handled in `src/main.rs`. Today it always clears the active turn, closes background and retry hints, removes the matching in-flight prompt, and settles it cancelled. `turn/completed` performs the same cleanup for real terminals and computes the stop reason from the terminal.

The fake host is `tests/fixtures/fake_serve.py`. Its scenario selected by `FAKE_SCENARIO` scripts notifications after `turn/start`. Integration tests in `tests/acp_serve.rs` drive the real adapter binary against this fixture. `tests/live_loopback.rs` drives a real `muse serve` with a scripted local model provider; a reply can stream text and then use `hold_ms` to leave the turn running long enough for a cancel.

`ROADMAP.md` section 9 contains the field matrix enforced by `tests/msp_140_field_matrix.rs`. `docs/event-compatibility.md` classifies every published schema notification and is test-enforced. `README.md` documents editor-visible turn behavior, and `CHANGELOG.md` records the unreleased change.


## Plan of Work

Add one boolean to `InFlight` recording whether the adapter has already reported a declined retract for that prompt. Initialize it false at every construction site. The flag is presentation state only; it does not represent the host turn state.

Split the `turn/retracted` arm in `src/main.rs` after parsing `outcome`. Treat only the exact string `declined` as declined. Log the turn, session, standing submission command id, and outcome. For declined:

- Find the matching in-flight prompt. If it is absent, log that the stale declined retract was ignored and return without touching active state.
- If present, leave `active_turn`, `in_flight`, background-check state, and retry state unchanged.
- If the in-flight flag is not already set, set it and send one agent text note: `Muse declined to retract the submitted message; the turn is still stopping and its terminal will settle this prompt.`
- Return without invalidating approvals or settling the prompt.

For accepted, absent, and unknown outcomes, retain the existing final-retract cleanup and cancellation settlement. Update comments and diagnostics to describe the actual accepted semantics rather than the previous assumption that both outcomes were terminal.

Add two fake-host scenarios. `declined_retract_then_completed` emits `turn/retracted` with outcome `declined`, then an agent message, then `turn/completed(completed)`. `declined_retract_quiet` emits only the declined retract so a test can steer and request a second Stop before the terminal. Update the fixture's scenario documentation and replace the obsolete no-terminal declined test.

Add focused tests in `tests/acp_serve.rs`. The first drives both ACP v1 and v2 through declined-then-completed and requires `end_turn`, not `cancelled`, plus the visible decline notice. The second uses ACP v2 with steering negotiated, verifies no idle state arrives at the decline, sends `_session/steering` and checks it targets the original turn, then sends `session/cancel` and verifies the later terminal settles the original prompt cancelled. Keep existing accepted, unknown/absent, and late-completion tests green.

Extend `cancel_stops_a_streaming_turn` in `tests/live_loopback.rs`. Resolve the host release, stream committed assistant text, send Stop, and for Muse 1.4.6 or later assert the adapter log recorded `outcome=declined` and the visible decline note. The prompt must still settle cancelled from the real terminal within the existing timeout.

Update `TurnRetractedParams.outcome` in the ROADMAP field matrix from a diagnostic-only disposition to an ACP-visible branch. Update the `turn/retracted` row in the event matrix and the retraction narrative in ROADMAP section 15. Add a short README clarification under turns/cancellation and an Unreleased CHANGELOG entry for #273.


## Concrete Steps

Run all commands from the repository root:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Run the focused integration tests while iterating:

    cargo test --locked --test acp_serve declined_retract -- --nocapture

Run the live cancellation test against the installed Muse build:

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback cancel_stops_a_streaming_turn -- --nocapture

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

A declined retract followed immediately by `turn/completed(completed)` must produce a v1 prompt result with `stopReason: "end_turn"` and a v2 idle state with `end_turn`, never an early cancelled settlement. The transcript contains exactly one message saying Muse declined to retract the submission, and stderr records the standing submission command id plus `outcome=declined`.

While a declined retract has no terminal, the ACP prompt remains open and no idle state is emitted. `_session/steering` still sends `turn/steer` with `expectedTurnId` equal to the original turn. A second `session/cancel` still targets that turn, and the resulting terminal settles the original ACP prompt exactly once as cancelled.

A declined retract for a turn with no matching in-flight prompt changes no active state and emits no transcript note. Accepted, absent, and unrecognized outcomes still settle immediately as cancelled. Existing retract-then-completed race tests remain green.

Against real Muse 1.4.10, canceling after assistant text has streamed must log the declined retract outcome and settle the ACP prompt cancelled from the host terminal. The test must not accept a generic or premature failure.


## Idempotence and Recovery

All changes are additive on a feature branch. Fake-host runs use isolated temporary directories. Re-running tests is safe. If the live test does not observe a declined retract on a host version that should support it, inspect the adapter log and provider stream log before changing the assertion; the fallback for pre-1.4.6 hosts must remain valid. Do not force-push over review feedback. Rollback is a return to `origin/master` before merge.


## Artifacts and Notes

The SDK's declined branch is:

    if (params.outcome === "declined") {
      const held = turns.get(params.turnId);
      if (held === undefined) return ignoredStaleFrame;
      held.commandId = params.commandId;
      return { kind: "turn", turnId, state: held.state };
    }

The schema's interrupt contract is:

    retract: if the turn is cancelled before any assistant output committed,
    the submission is durably retracted; a rejected retract does not undo
    the interrupt.

Validation transcripts and review findings will be added as work proceeds.

Local validation on 2026-10-19:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
        unit: 156 passed
        acp_serve: 357 passed
        fuzz corpus: 8 passed
        live_host: 1 passed
        live_loopback: 43 passed (disabled unless MUSE_ACP_LOOPBACK=1)
        msp_130_matrix: 4 passed
        msp_140_field_matrix: 5 passed
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs: 4 passed
    python3 -m unittest discover -s scripts -p 'test_*.py': 36 passed, 6 skipped
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Focused live tests against Muse 1.4.10:

    cancel_stops_a_streaming_turn: passed
    declined_retract_after_a_committed_tool_call_keeps_the_terminal_settlement: passed


## Interfaces and Dependencies

In `src/acp.rs`, extend:

    pub struct InFlight {
        pub msp_turn: String,
        pub req_id: J,
        pub queued: bool,
        pub file_report: Option<FileChangeReport>,
        pub declined_retract_notified: bool,
    }

In `src/main.rs`, the `turn/retracted` branch distinguishes:

    outcome == "declined"  -> hold tracked prompt; notify once; wait for terminal
    any other outcome      -> current final-retract cancellation path

No new crates or protocol methods are required.


Revision note (2026-10-19): Initial ExecPlan for issue #273 after reading the upstream 1.4.10 SDK fold.
