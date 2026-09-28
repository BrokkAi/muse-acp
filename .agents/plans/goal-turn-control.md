# Make goal turns stoppable and harden the /goal command

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


An editor user who types `/goal <objective>` asks the Muse host to work continuously toward that objective. Before this change the adapter answered that prompt the moment the host accepted the goal, so the editor thought nothing was running. The work the host then started (a "goal turn") could not be stopped: Zed showed no Stop button, and even ACP clients that did show one sent `session/cancel`, which the adapter only applied to turns that belonged to a prompt. A real editor session on 2026-09-28 shows the result: the user tried `/goal edit`, `/goal clear`, `/goal stop`, a new `/goal`, and `/goal pause`, while the original goal turn and its successors kept running and asking for approvals for another fifteen minutes. `/goal stop` was even stored as a new goal whose objective was the word "stop".

After this change, a `/goal` command that starts a goal turn keeps the editor prompt open until that turn ends, so the editor shows it as running and its Stop button interrupts it (the host then pauses the goal on its own). `session/cancel` also interrupts a turn the host started by itself when no prompt owns it. `/goal stop` and similar one-word control words give guidance instead of becoming goals. `/goal` accepts @-mentions. Goal commands work while a permission prompt is open. To see it working, run `cargo test --locked --test acp_serve goal` and observe the new goal tests pass; in Zed, type `/goal <objective>` and observe the Stop button while the goal turn runs.


## Progress


- [x] (2026-09-28 16:45+02:00) Reviewed the `/goal` feature and confirmed the defects against the host session log described above.
- [x] (2026-09-28 16:50+02:00) Wrote this plan.
- [x] (2026-09-28 17:05+02:00) Milestone 1: a goal turn woken by `/goal` is tracked as the prompt's own turn (ACP v1 answer at its terminal; ACP v2 `running` then `idle`).
- [x] (2026-09-28 17:05+02:00) Milestone 2: `session/cancel`, `session/close`, and the fail-closed cancel helper reach an active turn no prompt owns, through `session_stop_targets`.
- [x] (2026-09-28 17:10+02:00) Milestone 3: lone control words fail closed, @-mentions join the command line through `protocol_command_text`, and the pending-approval hold runs after the protocol-command intercept.
- [x] (2026-09-28 17:20+02:00) Milestone 4: fake-host scenarios `goal_wake`, `goal_wake_hang`, and `goal_continuation`; eight integration tests and two parser unit tests; README, ROADMAP, docs/event-compatibility.md, and CHANGELOG updated.
- [x] (2026-09-28 17:30+02:00) ACP v2 fix found in review: a goal command that settles on its ack while a turn still runs re-asserts `running` instead of reporting `idle`.
- [x] (2026-09-28 17:35+02:00) Full contributor gate passed (fmt, clippy, all tests, selftest, npm launcher, scripts, install.sh, npm smoke).


## Surprises & Discoveries


- Observation: the adapter processes ACP requests and MSP host notifications on one loop thread. While the `session/prompt` handler blocks on a host command, host notifications (such as `turn/started` and `turn/completed` for the woken turn) wait in the loop's channel and are handled only after the prompt handler returns.
  Evidence: `main` in `src/main.rs` dispatches both `LoopMsg::Acp` (through `handle_acp`) and `LoopMsg::Msp(MspEvent::Notification)` (through `handle_msp`) from the same `for` loop. So registering the woken turn inside the prompt handler cannot race with its terminal event.

- Observation: the host keeps a goal running by submitting its own follow-up turns ("continuations") after each goal turn completes, with no ACP request attached and no wire signal announcing them in advance.
  Evidence: the host session log shows `runtime.command_intake.received` with `"client_id": "muse-runtime-goal"` and a prompt starting "Continue working toward the active session goal". The MSP schema in `tests/protocol/stable/msp.schema.json` has no notification for a pending continuation; the only signal is the continuation's own `turn/started`.

- Observation: `session/close` had its own copy of the in-flight-only cancel loop, so closing a session also left a goal turn running.
  Evidence: the `"session/close"` arm of `handle_acp` sent `turn/cancel` only for `s.in_flight`. It now calls `cancel_session_turns`.

- Observation: in ACP v2 every protocol command that settled on its ack reported the session `idle`, even when a turn was still running (for example `/goal edit` during a goal turn).
  Evidence: `v2_busy_goal_command_keeps_the_session_running` fails when the new `busy` check is disabled and passes with it.

- Observation: `turn/interrupt` with `"retract": true` can be refused for the retract part (`assistant_output_committed`) while the interrupt itself still happens.
  Evidence: a live host session recorded the retract as rejected at 16:23:43 and the host trace log shows `event="runtime.run.interrupt" ... outcome="accepted"` and the run ending `outcome="cancelled"`. The schema says "A rejected retract does not undo the interrupt."


## Decision Log


- Decision: keep the `/goal` prompt open only for a turn the goal command itself started, identified as a `turnId` in the host's acknowledgement that is neither the session's current active turn nor a turn owned by another prompt.
  Rationale: the MSP goal acknowledgement carries `turnId` in two cases. On an idle session it names the fresh goal turn the command woke; on a busy session it names the turn that was already running (a routing fact). A busy acknowledgement must not attach the `/goal` prompt to someone else's turn. Because host notifications are processed after the prompt handler returns (see Surprises), the adapter's view of the active turn at acknowledgement time is the view from before the command was sent. If a host continuation launched just before admission and its `turn/started` has not been processed yet, the prompt attaches to that continuation; this is harmless, since the prompt then ends with that turn and Stop interrupts it.
  Date/Author: 2026-09-28, Claude.

- Decision: do not hold the `/goal` prompt open across host continuation turns.
  Rationale: nothing on the wire says whether a continuation will follow a completed goal turn, so holding the prompt would risk a prompt that never ends. Continuations are instead covered by `session/cancel` (Milestone 2). ACP v2 clients see them as running through `session/statusChanged`, which the adapter already maps to the ACP v2 `running` state. In ACP v1 (Zed), `/goal pause` stops further continuations, and pressing Stop on any later prompt also interrupts the running continuation.
  Date/Author: 2026-09-28, Claude.

- Decision: interrupt an unowned active turn with a plain `turn/interrupt` (no `retract`).
  Rationale: retract exists so a client can restore the text of a prompt that produced nothing. An unowned turn has no editor prompt to restore, and retracting a host continuation's submission is not something the user asked for.
  Date/Author: 2026-09-28, Claude.

- Decision: `/goal` followed by exactly one of the words stop, cancel, abort, end, quit, exit, off, done, status, show, or help (any letter case) is a usage error that names the real controls. Longer objectives that merely start with these words are still goals.
  Rationale: the host would store the word as a new objective and start working on it, which is what happened with `/goal stop`. A one-word objective made of a control word is almost certainly a control attempt. The guard only rejects input; it never sends a different host method, so it cannot change host semantics.
  Date/Author: 2026-09-28, Claude.

- Decision: a protocol command (`/goal`, `/rename`, `/workflow-child`) whose first block is text may be followed by text blocks and @-mention blocks (`resource_link` and embedded `resource`). The blocks are joined into one command line; a mention becomes `[@name](uri)` (the same link text Zed shows for a mention). Any other block (an image or audio) makes the command a usage error.
  Rationale: Zed sends each @-mention as its own content block, so a `/goal` with a mention used to fall through and run as an ordinary prompt starting with "/goal". Only a reference is used, never embedded file contents, so an objective stays small and the model can open the file itself.
  Date/Author: 2026-09-28, Claude.

- Decision: `session/close` reuses `cancel_session_turns` instead of its own in-flight loop, and a v2 command that settles on its ack sends `running` rather than `idle` while any turn is active.
  Rationale: both were the same "only prompt-owned turns count" defect found while implementing Milestone 2; fixing them keeps the session state and cleanup consistent with Stop.
  Date/Author: 2026-09-28, Claude.

- Decision: the pending-approval hold in the `session/prompt` handler moves below the protocol-command intercept, so `/goal`, `/rename`, and `/workflow-child` reach the host while a permission prompt is open. `/compact` and ordinary prompts stay held.
  Rationale: the hold exists because a new `turn/start` against an unresolved approval is rejected by the host as an unrecorded human resolution. Goal, rename, and workflow-child commands are control commands, not human turn input; a goal command on a busy session never starts a turn. Pausing or clearing a goal while the goal turn waits for approval is exactly when a user needs it. `/compact` is left held to keep this change narrow.
  Date/Author: 2026-09-28, Claude.


## Outcomes & Retrospective


All four milestones are done. The seven behavior tests named in Validation and Acceptance, plus `v2_busy_goal_command_keeps_the_session_running`, fail against the previous `src/main.rs` and pass now; the full suite passes (234 integration tests, 96 unit tests). In an editor, `/goal <objective>` on an idle session now shows a running prompt whose Stop button interrupts the goal turn, after which the host pauses the goal.

What remains: in ACP v1 a host continuation turn that starts after the woken goal turn ended has no open prompt, so Zed shows no Stop button for it. `/goal pause` prevents further continuations, and Stop on any later prompt now also interrupts the running continuation. Closing that gap needs a host signal that a continuation is coming, which MSP does not provide today.

Lesson: the original feature was tested only against a fake host whose goal acknowledgement never named a turn, so the woken-turn path documented in the ROADMAP had never run in a test. Fake-host scenarios should cover every result case the schema describes.


## Context and Orientation


The adapter (`muse-acp`) sits between an editor that speaks ACP (Agent Client Protocol: JSON-RPC over stdin/stdout, used by Zed and JetBrains AIR) and a Muse host process that speaks MSP (Muse Session Protocol, also JSON-RPC). ACP has two versions in this adapter. In ACP v1, a `session/prompt` request stays open while the agent works and is answered with a `stopReason` when the work ends; the editor shows a Stop button only while such a request is open, and Stop sends the `session/cancel` notification. In ACP v2, `session/prompt` is answered immediately with `{}` and the adapter reports progress with `session/update` state changes (`running`, `idle`, `requires_action`).

The MSP host runs work as "turns". The adapter starts one with the `turn/start` host method and learns about it through the `turn/started` and `turn/completed` host notifications. The adapter records each prompt-owned turn as an `InFlight` value (struct `InFlight` in `src/acp.rs`: the MSP turn id `msp_turn`, the ACP request id `req_id`, a `queued` flag, and an optional file report). Each ACP session is an `AcpSession` with `in_flight: Vec<InFlight>` and `active_turn: Option<String>`, the turn the host last reported as running. The `turn/completed` handler in `handle_msp` (`src/main.rs`) removes the matching `InFlight` and answers its prompt (v1) or reports `idle` (v2).

The MSP goal methods are `goal/set` and `goal/edit` (both take `objective`), and `goal/pause`, `goal/resume`, and `goal/clear` (no objective). All answer with `{commandId, status, turnId?}`. `turnId` is present when an idle session was woken (the fresh goal turn) or when the session was busy and the verb was set, edit, or resume (the turn already running). Pause and clear never carry it. When a goal turn is interrupted, the host pauses the goal itself (transcript `tests/protocol/transcripts/goal-interrupt-auto-pause`).

The editor reaches these methods through adapter-local slash commands handled in the `"session/prompt"` arm of `handle_acp` in `src/main.rs`. `parse_goal_command`, `parse_rename_command`, and `parse_workflow_child_command` turn prompt text into a host method plus fields, and `parse_protocol_command` tries them in order. The intercept (search for `let command_attempt =`) requires the prompt to be exactly one text block, sends the host method, and answers the prompt immediately. Before that intercept, a hold (search for `if let Some(approval_id) = pending_approval`) refuses any prompt while a tool approval is unanswered. `session/cancel` (search for `"session/cancel" =>`) sends `turn/interrupt` with `retract: true` for every `InFlight` turn, and `cancel_session_turns` sends `turn/cancel` for the same set when a permission cannot be shown to the user.

Tests: `tests/acp_serve.rs` drives the built adapter against a fake host, `tests/fixtures/fake_serve.py`, selected by the `FAKE_SCENARIO` environment variable. The fake host appends each received method name to a log file (read with `Client::wait_log`) and each request's params to an input log (read with `Client::wait_input`). Notifications the fake sends while handling a request go out before that request's response.


## Plan of Work


Milestone 1 tracks the goal turn. In the protocol-command intercept of `"session/prompt"` in `src/main.rs`, after a successful host acknowledgement, read `turnId`. When it is present and is neither the session's `active_turn` nor the `msp_turn` of any `InFlight`, the command woke a fresh goal turn: push an `InFlight` for it (with `queued: false` and the prompt's file report request), set `active_turn`, and answer the prompt the way an accepted `turn/start` is answered: in ACP v2 send `{}`, the user-message echo, and `running`; in ACP v1 echo the user message and leave the request open. The existing `turn/completed`, `turn/retracted`, and host-failure paths then answer it. Otherwise keep today's immediate answer.

Milestone 2 makes Stop reach every foreground turn. Add a helper `session_stop_targets(s: &AcpSession) -> Vec<(String, String, bool)>` returning `(msp_session_id, turn_id, owned)` for every `InFlight` turn (`owned = true`) plus `active_turn` when no `InFlight` owns it (`owned = false`). Use it in `session/cancel` (owned turns keep `retract: true`; unowned turns get a plain interrupt) and in `cancel_session_turns`.

Milestone 3 changes parsing. In `parse_goal_command`, reject a lone reserved control word with a message that names Stop, `/goal pause`, and `/goal clear`. Replace the single-block check in the intercept with a helper `protocol_command_text(blocks: &[J]) -> Option<Result<String, String>>`: `None` unless the first block is a text block whose first word (after `/`) is `goal`, `rename`, or `workflow-child`; otherwise the joined command line, or an error naming the command when a block cannot be expressed as text. Move the pending-approval hold so it runs after the protocol-command intercept and before `/compact`.

Milestone 4 adds tests and documentation. In `tests/fixtures/fake_serve.py`, add scenarios `goal_wake` (the woken turn produces a message and completes), `goal_wake_hang` (the woken turn runs until interrupted; a later set, edit, or resume names it as the busy turn), and `goal_continuation` (the woken turn completes and the host starts an unowned follow-up turn that runs until interrupted). The existing `turn/interrupt` handler already reports `turn/completed` with terminal `cancelled`. Add integration tests in `tests/acp_serve.rs` for each behavior and unit tests for the parsers. Update README.md (Goals bullet), ROADMAP.md (the five `goal/*` rows and section 15), docs/event-compatibility.md (the goal paragraph), and CHANGELOG.md.


## Concrete Steps


Run everything from the repository root (the worktree directory).

    cargo build
    cargo test --locked --test acp_serve goal
    cargo test --locked --bin muse-acp goal

Then the full contributor gate from CONTRIBUTING.md:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest


## Validation and Acceptance


These tests must fail before the change and pass after it. `goal_wake_keeps_the_prompt_open_until_the_goal_turn_ends` (ACP v1): the answer to `/goal Green the suite` arrives only after the goal turn's agent message, with `stopReason` `end_turn`. `goal_wake_reports_running_in_v2` (ACP v2): the prompt is answered with `{}`, then `running`, then `idle` with `end_turn`. `session_cancel_interrupts_a_goal_turn`: after `/goal`, `session/cancel` sends `turn/interrupt` naming the goal turn and the `/goal` prompt ends with `cancelled`, while a `/goal edit` sent in between is answered at once. `session_cancel_interrupts_an_unowned_host_turn`: with a host continuation running and no prompt open, `session/cancel` sends `turn/interrupt` naming the continuation without `retract`. `goal_control_words_fail_closed`: `/goal stop` is answered with error `-32602` whose message names `/goal pause`, and the host never sees `goal/set`. `goal_objective_keeps_mentions`: `/goal port ` plus a `resource_link` sends `goal/set` whose objective contains `[@lib.rs](file:///tmp/lib.rs)`, and an image block yields `-32602`. `goal_commands_pass_the_pending_approval_hold`: with an approval open, `/goal pause` reaches the host as `goal/pause` while an ordinary prompt is still held.

Existing tests must keep passing, in particular `goal_slash_commands_map_to_host_goal_methods`, `session_cancel_interrupts_exact_turn_with_retract`, and `prompt_while_approval_pending_is_held_locally`.


## Idempotence and Recovery


All steps are source edits and test runs; repeating them is safe. The fake host writes only to per-test temporary directories. If a step leaves the tree broken, `git diff` shows exactly what changed and `git checkout -- <file>` restores a file.


## Artifacts and Notes


The sequence from the reporting session's host log (objectives replaced by placeholders; times are local, +02:00):

    16:10:44 goal_control_applied set   "<objective A>"    (goal turn starts)
    16:18:38 goal_control_applied edit  "<objective B>"    (turn keeps running)
    16:18:52 goal_control_applied clear
    16:19:05 goal_control_applied set   "stop"             (a new goal named "stop")
    16:19:27 command_intake client_id=muse-runtime-goal "Continue working toward the active session goal ... stop"
    16:19:49 goal_control_applied set   "<objective C>"
    16:20:36 goal_control_applied pause
    16:22-16:25 tool approvals keep arriving


## Interfaces and Dependencies


No new dependencies. In `src/main.rs`, at the end of the milestones, these exist:

    /// Every foreground turn Stop should reach: (msp session id, turn id, owned by a prompt).
    fn session_stop_targets(s: &AcpSession) -> Vec<(String, String, bool)>

    /// The command line of a protocol-command prompt, or None when the prompt is not one.
    fn protocol_command_text(blocks: &[J]) -> Option<Result<String, String>>

`parse_goal_command` keeps its signature `fn parse_goal_command(text: &str) -> Option<Result<(String, Option<String>), String>>`.


Revision note (2026-09-28): updated Progress, Surprises & Discoveries, Decision Log, and Outcomes after implementation; added the `session/close` and ACP v2 busy-state fixes found during the work.
