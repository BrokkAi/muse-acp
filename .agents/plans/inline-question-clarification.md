# Put "Explain instead" inside each Muse question form

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This plan follows `.agents/PLANS.md`. It implements GitHub issue #323 in the repository `BrokkAi/muse-acp`.

## Purpose / Big Picture

Today, when Muse asks a question with selectable options, the editor first receives a separate form containing only the choices "Answer questions" and "Explain instead". The user must submit that form before seeing the actual question. This change puts "Explain instead" and an optional clarification text box into the question form itself. After the change, answering a normal single-choice question takes one form submission instead of two, while a user can still choose "Explain instead", type text, and send the same MSP `userInput/clarify` command as before.

The behavior is visible in an editor or in the integration test that launches the real adapter with the checked-in fake MSP host. An option-bearing Muse question must produce one `elicitation/create` request whose schema contains the question property, the host's choices, an "Explain instead" choice, and a `clarification` field. Choosing a real option must still produce `userInput/answer`; choosing "Explain instead" with valid text must produce exactly one `userInput/clarify`.

## Progress

- [x] (2026-10-20) Reviewed issue #323, the current pending-UI state machine, question schemas, fake-host scenarios, integration tests, live loopback tests, and user documentation.
- [x] (2026-10-20) Created the topic branch `fix/issue-323-inline-explain-choice`.
- [x] (2026-10-20) Removed the route and separate clarification stages from the pending-question model.
- [x] (2026-10-20) Emitted one question form with a collision-safe explain choice and optional clarification field.
- [x] (2026-10-20) Routed explain selections through the existing `userInput/clarify` validation and host command.
- [x] (2026-10-20) Updated fake-host and live loopback tests for the one-form flow, including single-choice, multiple-choice, staged questions, resume, validation, and host rejection.
- [x] (2026-10-20) Updated human-facing documentation.
- [x] (2026-10-20) Ran formatting, focused tests, the full contributing gates, and recorded final evidence.

## Surprises & Discoveries

- Observation: MSP `UserInputClarifyParams` has no answers member.
  Evidence: the vendored schema definition contains only `commandId`, `sessionId`, `userInputId`, and `clarification`. Therefore an inline explain selection must replace rather than augment any answers already held from earlier staged questions.

- Observation: making "Explain instead" available on a multiple-selection question can conflict with the original `minSelections` value.
  Evidence: selecting only the explain sentinel is one array item, so a host minimum greater than one would make the explain path impossible if the client schema retained that minimum. The adapter must allow the explain-only branch and separately enforce the original minimum when the user selects real answers.

- Observation: `_explain` is a good default sentinel but is not guaranteed to avoid an arbitrary Muse label.
  Evidence: MSP labels are free-form strings. The adapter must choose a sentinel not already present in the question's deduplicated display labels and remember it on the pending question.

- Observation: the npm launcher tests and smoke test need to run outside the execution sandbox.
  Evidence: inside the sandbox, Node reported `EPERM` from nested `spawnSync`, lost inherited output, and the smoke test timed out with status `null`. With scoped escalation, all four launcher tests and the native-binary smoke test passed.

- Observation: `master` advanced with the ACP initialization fix from #320 while the #323 PR was open.
  Evidence: the rebase conflicted only in the new `Unreleased` changelog section. Both entries were retained, and the full Rust suite passed again after rebasing onto merge commit `26982fd`.

## Decision Log

- Decision: Remove `UiStage` and all route/clarification stage state rather than leaving an unused one-value state machine.
  Rationale: every question form now has the complete answer and clarification surface. The pending record only needs its current question index, confirmed previous answers, and current ACP request id.
  Date/Author: 2026-10-20 / Codex

- Decision: Use a collision-safe sentinel beginning with `_explain`, adding trailing underscores until it is absent from that question's display labels.
  Rationale: it gives a stable internal value while avoiding rewriting a real host label.
  Date/Author: 2026-10-20 / Codex

- Decision: The inline explain path reuses the existing validation and `userInput/clarify` command behavior rather than introducing a second host protocol.
  Rationale: MSP already defines clarification as the "let me explain" path, with text limited to 500 characters.
  Date/Author: 2026-10-20 / Codex

- Decision: For multiple-selection questions, the explain sentinel must be the only selected array entry. If it appears with real answers, the same form is reissued with an explanatory error.
  Rationale: combining an explanation with structured answers would be ambiguous, and `userInput/clarify` cannot carry both.
  Date/Author: 2026-10-20 / Codex

- Decision: For multiple-selection questions whose original minimum is greater than one, the JSON schema tells generic clients only that at least one item is needed; the adapter enforces the original minimum for real-answer submissions.
  Rationale: standard JSON Schema cannot express "one explain sentinel or at least N real labels" without conditional complexity that generic ACP form clients may not support. Adapter validation preserves the host contract while keeping the explain branch usable.
  Date/Author: 2026-10-20 / Codex

- Decision: If "Explain instead" is chosen on a later staged question, confirmed earlier answers are not included in the clarification.
  Rationale: MSP's clarification command has no answers field. The explanation replaces the structured answer flow for that user-input request.
  Date/Author: 2026-10-20 / Codex

## Outcomes & Retrospective

Implemented the complete #323 outcome. The pending-question state machine now has no route or separate clarification stage. Every option-bearing question includes a titled, collision-safe inline explanation choice and clarification text box. Normal answers preserve their existing MSP mapping; explain submissions use the existing `userInput/clarify` command; invalid text and ambiguous multiple-choice combinations reissue the same form without a host mutation.

The fake-host suite covers single-choice, multiple-choice, strict minima, sentinel collision, later staged questions, explain validation, host rejection, duplicate delivery, resume, and interruption. Both live loopback question scenarios passed against the installed Muse 1.4.11 host. The full Rust test suite, Clippy, formatting, Python tests, shell syntax, npm launcher tests, selftest, and npm native-binary smoke test all passed.

The main lesson is that inline explanation is a second branch within each staged question, not a separate UI stage. Keeping the host's real-answer minimum in the adapter was necessary because the JSON schema minimum has to permit the one-item explain branch.

## Context and Orientation

The adapter is a Rust executable. `src/main.rs` contains the ACP loop, MSP bridging, pending-question state transitions, and the form schema builders. `src/acp.rs` contains shared pending-session data types. The checked-in Python file `tests/fixtures/fake_serve.py` imitates `muse serve`; `tests/acp_serve.rs` launches the adapter against that fake host and inspects JSON-RPC traffic. `tests/live_loopback.rs` exercises the same paths against a real Muse host when live validation is enabled.

An ACP "elicitation form" is a client-initiated user-input display represented by an `elicitation/create` request. Its `requestedSchema` is a JSON Schema object describing fields. The client replies with an `accept` result and a `content` object, a `decline` result, or a cancellation/error. The adapter must map accepted content back to Muse's MSP protocol and must withdraw forms when Muse settles the question elsewhere.

The current two-step flow is:

1. `bridge_user_input` stores a pending UI in `UiStage::Route` and sends a form whose only property is `route`.
2. For "Answer questions", `complete_elicitation` stores `UiStage::Answers` and sends another form containing the first actual question.
3. For "Explain instead", it sends a third-style clarification-only form before calling `userInput/clarify`.

Multi-question prompts are already staged one question at a time so the adapter can hold confirmed answers and settle through `userInput/interrupt` if the user stops. That staging remains.

## Plan of Work

In `src/acp.rs`, remove the `UiStage` enum and remove `stage`, `answer_schema`, and `answer_message` from `PendingUi`. Add an `explain_value` field to `UiQuestion` so the exact collision-safe sentinel travels with the question. Keep `confirmed`, `question_index`, and the existing ownership and request fields because those still drive staged answering, resume, cancellation, and duplicate handling.

In `src/main.rs`, update the pending-form helpers so showing or reissuing a form only changes the current request id, question index, confirmed answers, message, and schema. Remove all route-specific constants and stage arguments.

Change `bridge_user_input` so construction of a pending question never creates a route form. After parsing every `UserInputQuestion`, assign its collision-safe explain sentinel. Store the pending request and immediately send the first question form using `ui_answer_message(0, ...)` and `ui_answer_schema(0, ...)`. This applies to both option-bearing and free-text first questions.

Build option schemas that use titled JSON Schema choices. Keep the existing deduplicated display label as each real option's `const` and use that display label as its `title`. Append the explain option with the pending question's sentinel as `const`, `title` equal to "Explain instead", and a short description explaining that the user should supply text instead of choosing a structured answer. Single-choice properties remain strings and use `oneOf`. Multiple-choice properties remain arrays, use `anyOf` for their item choices, remain required when the host question is required, and retain the existing maximum when present.

Add an optional `clarification` string property with `maxLength` 500 to option-bearing forms. Do not add it to free-text questions because they already answer with text. The title and description should make clear that it is required only when the explain choice is selected.

Add a helper that classifies the `q<index>` value as a normal answer, an explain request, or an invalid ambiguous explain request. For a single-choice string, the explain request is exactly the stored sentinel. For a multiple-choice array, the explain request is a one-element array containing the sentinel. A sentinel mixed with other selections is invalid and reissues the same form.

Refactor `complete_elicitation` to process the current question directly. If the explain request is valid, validate `clarification` with the same trim, non-empty, and 500-character rules used today. Missing or invalid text reissues the same question form with guidance and sends no host command. Valid text calls `userInput/clarify` exactly as the current clarification stage does and removes the pending UI on success. A host rejection reissues the same question form with Muse's error and keeps the prompt answerable.

If the result is a normal answer, preserve `ui_answer_json` and the existing single-choice, multiple-choice, and free-text mappings. Add adapter-side validation of the host's original minimum and maximum for multiple-selection real-answer submissions because the schema minimum may be relaxed to permit explain alone. On a later staged question, a valid explain request replaces the normal answer path and sends clarification without the previously confirmed answers.

Update `tests/acp_serve.rs` so integration tests expect the first elicitation immediately, without a route request. Cover all existing normal mappings and staged flows, plus these issue acceptance behaviors: a real option emits one `userInput/answer`; explain with text emits exactly one `userInput/clarify` and no answer or extra turn start; blank or over-limit clarification reissues the same form without a host command; host clarification rejection is visible and correctable; explain mixed with other multiple-choice values is rejected; resume and duplicate handling still show one form per pending ask.

Update `tests/live_loopback.rs` so its single-question and staged-question scenarios answer the first `q0` form directly. Update `README.md` under "Questions" to explain that option-bearing forms include "Explain instead" and the clarification field inline.

## Concrete Steps

Run all commands from the repository root:

    /home/ryansvihla/code/muse-acp/.mj/clones/c5b2648d03234ada7cd8c457cb356652

After the state-model and schema edits, compile and run the focused integration tests:

    cargo test --locked --test acp_serve user_input
    cargo test --locked --test acp_serve explanation
    cargo test --locked --test acp_serve clarification
    cargo test --locked --test acp_serve structured_question
    cargo test --locked --test acp_serve partial_user_input

The expected result is that all selected tests pass. Before the implementation, the existing tests expect a `route` property and therefore demonstrate the old behavior. Afterward, their updated form-count assertions should prove that only question forms are emitted.

Run the formatting and full contributor gates before handoff:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

The test environment may not have a real Muse installation or live-host credentials. The fake-host integration suite must still pass. If the environment supports `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback`, run the question-related live tests as an additional check and record any release-based skips.

## Validation and Acceptance

Use the fake-host scenario `questions` with a form-capable client. After `session/prompt`, the first `elicitation/create` must already contain a `q0` property with `Alpha` and `Beta`, the collision-safe explain sentinel, and an optional `clarification` property. There must be no `route` property and no separate pre-question form.

Submit `q0: "Alpha"` to the one form. The fake host must receive one `userInput/answer` whose selected label is `"Alpha"`; it must receive no `userInput/clarify`.

Submit `q0` equal to the explain sentinel and `clarification: "The premise does not fit my situation."` The fake host must receive exactly one `userInput/clarify` for the original `userInputId`, with that text and format `text`. It must receive no `userInput/answer` for that submission and no additional `turn/start`. The pending form must not be reissued.

Submit the explain sentinel with blank text, whitespace-only text, or more than 500 characters. No host command may be sent for that submission. A replacement question form must appear with an actionable validation message and the same answer and clarification fields.

For a multiple-choice question, submit `[ "_explain", "Red" ]` (where the first value is the actual stored sentinel). The adapter must reject the ambiguous combination by reissuing the form without a host command. Submit the sentinel alone with valid clarification and expect one `userInput/clarify`. Submit only real choices still respecting the host's original minimum.

For a two-question staged prompt, answer the first question and stop at the second. The existing `userInput/interrupt` must still carry the confirmed first answer. Answering a real second option must still send one combined `userInput/answer`. Duplicate host delivery of the same pending question must still produce only one form.

## Idempotence and Recovery

The source edits and test commands are safe to repeat. If compilation fails mid-refactor, preserve the existing `ui_answer_json` mapping and finish removing stage references before adjusting tests. If a test fails because it still sends a `route` value, update that test to answer the question form directly rather than reintroducing a compatibility route.

Do not delete or rewrite unrelated worktree changes. This branch was created from a clean `master`. If validation creates build artifacts, they are ignored by Git. No credentials or real Muse state are required for the fake-host test suite.

## Artifacts and Notes

The key observable shape for a single-choice question should resemble this fragment inside `params.requestedSchema`:

    "properties": {
      "q0": {
        "type": "string",
        "oneOf": [
          {"const": "Alpha", "title": "Alpha"},
          {"const": "Beta", "title": "Beta"},
          {"const": "_explain", "title": "Explain instead",
           "description": "Explain what you meant instead of choosing an answer."}
        ]
      },
      "clarification": {"type": "string", "maxLength": 500}
    },
    "required": ["q0"]

The exact explanatory prose may differ, but the sentinel must match the value stored on the pending question and must not collide with a real display label.

## Interfaces and Dependencies

In `src/acp.rs`, the resulting shared types should have these essential fields:

    pub struct UiQuestion {
        pub qid: String,
        pub labels: Vec<String>,
        pub display: Vec<String>,
        pub message: String,
        pub multiple: bool,
        pub min: u64,
        pub max: Option<u64>,
        pub explain_value: String,
    }

    pub struct PendingUi {
        pub req_id: J,
        pub user_input_id: String,
        pub owner_msp_sid: String,
        pub turn_id: String,
        pub questions: Vec<UiQuestion>,
        pub tool_call_id: String,
        pub question_index: usize,
        pub confirmed: Vec<String>,
    }

No new crate dependency is needed. The implementation continues to construct schema JSON with the repository's existing `esc` and JSON helper functions. The host commands remain `userInput/answer`, `userInput/clarify`, `userInput/cancel`, and `userInput/interrupt` as defined in `tests/protocol/stable/msp.schema.json`.

Plan revision note (2026-10-20): marked the plan complete after implementation and validation, recorded the sandbox-only npm runner failure, added the collision and multi-select decisions that emerged from implementation, and recorded the clean rebase over the subsequently merged #320 initialization change.
