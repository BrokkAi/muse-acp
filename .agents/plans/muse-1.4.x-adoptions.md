# Adopt the Muse 1.4.x MSP additions the adapter still refuses

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture

`muse-acp` is an adapter between two protocols. Editors such as Zed and JetBrains speak ACP (Agent Client Protocol) to the adapter over stdio. The adapter speaks MSP (Muse Session Protocol, JSON-RPC) to a `muse serve` host process. Every host method the adapter does not drive is a feature an editor user cannot reach through the adapter even though Muse itself supports it.

Muse 1.4.3 through 1.4.6 added a list of methods, notifications, capabilities, and vocabulary that the adapter deliberately left unmapped when the vendored schema corpus was re-pinned to SDK revision `ea88c822` (Muse 1.4.6). Those gaps are recorded in `ROADMAP.md` section 9 ("Muse 1.4.x stable-surface additions") with a tracking issue each, filed as GitHub issues #214 through #224. This plan adopts them, one pull request per issue, merging each only when its checks are green. The corpus has since advanced to SDK `a0c8611` (Muse 1.4.7, 2026-10-13); the 1.4.7 additions — `computerUseSetup/check` and the sandbox-availability, `SessionNameOrigin`, and `Item.cutOff` fields — have their own tracking issues (#240–#243) instead of becoming milestones here.

The user-visible result depends on the issue: an editor user can pause and resume a workflow run (`/workflow pause` and `/workflow resume`), interrupt a pending question while keeping the answers already confirmed (`userInput/interrupt`), ask for a while-you-were-away recap (`/recap`), inspect the session's hooks and background tasks, toggle or import skills, open a side chat, inspect and manage plugins, drive Muse's computer-use setup, attach video (or other large media) to a turn, and see the remaining 1.4.x wire fields mapped or explicitly ignored rather than silently dropped. The last part, issue #224, is as much documentation as code: the goal is that every field-level addition has a recorded disposition.

To see it working, run the per-issue test command named in each milestone below; each names an integration test in `tests/acp_serve.rs` that drives the real adapter binary against the scripted fake host in `tests/fixtures/fake_serve.py`. The container this plan was written in also has a real Muse 1.4.7 binary, so the strongest proof is `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback`, which runs the adapter against a real `muse serve` whose model calls go to a scripted loopback provider.


## Progress

- [x] (2026-10-12) Surveyed the open issues #214 through #224, the vendored schema, `ROADMAP.md` section 9, and the existing extension and slash-command patterns.
- [x] (2026-10-12) Issue #223 `userInput/interrupt`: merged in PR #228 (staged multi-question forms, interrupt with confirmed answers, fake-host and live-1.4.6 tests, docs).
- [x] (2026-10-12) Issue #218 `workflow/pause` and `workflow/resume`: merged in PR #229 (fold pause state and resume generation, `/workflow pause|resume [<runId>]`, fake-host plus live 1.4.4/1.4.6 tests, docs).
- [x] (2026-10-12) Issue #220 `hook/list`: merged in PR #230 (opt-in `_session/hooks`, hook inventory in the fold, `hookRun` cards, fake-host plus live 1.4.4/1.4.6 tests, docs).
- [x] (2026-10-12) Issue #215 `recap/request` and `recap/cancel`: merged in PR #231 (`/recap` holding the prompt until `recap/completed`, `recap/cancel` on stop, failure reporting, fake-host plus live-1.4.6 tests, docs).
- [x] (2026-10-12) Issue #217 `skill/setActivation` and `skill/import`: merged in PR #232 (`/skill-activation`, `/skill-import`, typed error passthrough on protocol commands, fake-host plus live 1.4.4/1.4.6 tests, docs).
- [x] (2026-10-12) Issue #222 `session/sideChat` and `SideProvenance`: merged in PR #233 (opt-in `_session/sideChat`, client-side attach through resume/load, provenance in `session_info_update`, fake-host plus live 1.4.4/1.4.6 tests, docs).
- [x] (2026-10-12) Issue #219 `task/list`: merged in PR #234 (resume/re-attach seeding of AIR async tasks, seeded-id reuse for later item events, fake-host plus live-1.4.6 tests, docs).
- [x] (2026-10-12) Issue #214 plugin and marketplace management: merged in PR #235 (opt-in `_session/plugins`, `/plugin install|marketplace` commands, typed refusals, consumed change notices, fake-host plus live 1.4.4/1.4.6 tests, docs).
- [x] (2026-10-12) Issue #216 computer-use settings and setup: merged in PR #236 (opt-in `_session/computerUse`, `/cua` command family, capability request/grant plumbing, typed `computerUseUnavailable`, consumed setup notices, fake-host plus live-1.4.6 tests, docs).
- [ ] (2026-10-12) Issue #221 `media/upload`: implemented on `issue-221/media-upload` (resource-link/embedded-blob video entry, 1 MiB aligned chunks with retries and UUIDv7 uploadId, injected uploader callback, video turn part, fake-host plus live-1.4.6 tests, docs), PR and merge pending.
- [x] (2026-10-13) Re-pinned the vendored corpus to SDK `a0c8611` (Muse 1.4.7): stable manifest and schema bundle, 51 transcripts, `src/compat.rs` compatibility table, `tests/protocol/PROVENANCE.md`, `ROADMAP.md`, and `CHANGELOG.md` (PR #244). The new surface is tracked in #240–#243.
- [ ] Issue #224 remaining 1.4.x field-level and vocabulary additions.


## Surprises & Discoveries


- Observation: the ACP elicitation contract cannot carry partial answers. `elicitation/create` is answered exactly once, and only the `accept` action carries any content; `decline` and `cancel` carry none. Therefore the adapter can only hold "confirmed partial answers" if it collects answers one question at a time itself. This shapes the #223 milestone.
  Evidence: the ACP schema at `agentclientprotocol/agent-client-protocol` defines `ElicitationAction` as `Accept(ElicitationAcceptAction)`, `Decline`, `Cancel`, with the action flattened and content only on the accept arm.

- Observation: Muse 1.4.6 reuses a workflow's run id for the resumed generation. A paused run named `workflow-run-model-tool-call_1` resumed as an item with both `workflowRunId` and `resumeFromRunId` equal to `workflow-run-model-tool-call_1`, a fresh item id, `triggerSource: "guidanceAuto"`, and a restarted revision sequence. The fold therefore treats a same-id `resumeFromRunId` as a generation boundary (bypassing the revision guard) instead of a supersession to a different run.
  Evidence: a live 1.4.6 run paused via `/workflow pause`, resumed via `/workflow resume`, and the resumed card frame carried both ids equal; the host's session record shows the resumed launch as a new `workflow_run_launched` event under the same run id.


## Decision Log


- Decision: ship one pull request per issue, in the order listed in Progress, and merge each when its required check (the `live-host` aggregate) is green.
  Rationale: the maintainer asked for a PR at a time with a review and fix cycle between merges. The order starts with the smallest, best-understood issues so the early PRs can land while the larger protocol questions (media upload, computer use) are still being designed.
  Date/Author: 2026-10-12, Codex.

- Decision: for #223, stage multi-question prompts one question per `elicitation/create` round and hold each confirmed answer, so `session/cancel` can settle the open prompt with `userInput/interrupt` carrying those answers.
  Rationale: ACP forms are atomic, so no partial answer can exist unless the adapter keeps it across rounds. The alternative - sending `userInput/interrupt` with no answers - would not change behavior the host cares about, and the issue exists precisely to stop losing confirmed answers. Single-question prompts keep their current two-round flow (route, then the answer form), so the common case is unchanged; a prompt with no confirmed answer still stops through `turn/interrupt` exactly as before, and declining a form still sends `userInput/cancel`.
  Date/Author: 2026-10-12, Codex.

- Decision: each milestone updates the ROADMAP section 9 row for its surfaces from "Unsupported pending protocol decision" to the behavior actually implemented, adds a `CHANGELOG.md` Unreleased entry, and updates `docs/event-compatibility.md` when a notification's disposition changes.
  Rationale: the repository's contribution rules require documentation to match behavior, and existing tests enforce that the notification matrix classifies every published schema notification exactly once.
  Date/Author: 2026-10-12, Codex.

- Decision: name the run-control command `/workflow pause|resume [<runId>]`, following issue #218's suggested spelling.
  Rationale: a host skill named `workflow` is shadowed, which is the same tradeoff already accepted for `/goal`, `/rename`, and `/compact`. The alternative `/workflow-child`-style hyphenated name would contradict the issue's explicit suggestion, and the adapter's help text lists the live runs so the command is discoverable.
  Date/Author: 2026-10-12, Codex.


## Outcomes & Retrospective

To be written as milestones land.


## Context and Orientation

The adapter's entry point is `src/main.rs`. It runs one loop that reads ACP messages from the editor's stdin, MSP messages from the host process, and dispatches both. `handle_acp` handles editor requests; `handle_msp` handles host notifications. Host methods are sent through `MspHost::command`, with `commandId`-based idempotency where MSP requires it (`mint_cmd`). ACP session state lives in `AcpSession` (`src/acp.rs`), including the fold (`src/fold.rs`) that turns MSP item and turn events into ACP updates, pending permissions, pending user-input forms, and in-flight prompts.

There are two ways an editor reaches a host feature. Either ACP core carries it (a new session, a prompt, a cancel), or the adapter teaches the editor a slash command (like `/goal`, `/rename`, `/workflow-child`, `/feedback`, `/compact`) and advertises it through `available_commands_json` in `src/acp.rs`. Slash commands have four moving parts, all in `src/main.rs`: a parser beside `parse_goal_command`, an entry in `PROTOCOL_COMMANDS`, an intercept in the `session/prompt` handler that sends the host method and settles the ACP prompt, and a fake-host acknowledgement plus an integration test in `tests/fixtures/fake_serve.py` and `tests/acp_serve.rs`. The AI Review extension (`AIR`) is the negotiated extension surface for observing and stopping background work; its methods are `_session/steering`, `_session/readOutput`, `_session/userShell`, and `_session/async_task/stop`.

Protocol conformance inputs live in `tests/protocol/`: `stable/msp.schema.json` is the vendored stable schema bundle for SDK revision `a0c8611` (Muse 1.4.7), and `stable/manifest.json` pins its fingerprint. `src/compat.rs` classifies the fingerprint a live host reports; the 1.4.7 fingerprint classifies as degraded until a pinned 1.4.7 build joins CI. The vendored `transcripts/` corpus has hand-authored golden transcripts, including `userinput-interrupt-round-trip`, which shows the intended `userInput/interrupt` wire flow. Two unit tests enforce documentation: `every_schema_notification_has_an_explicit_matrix_disposition` in `src/compat.rs` requires `docs/event-compatibility.md` to classify every and only published schema notification, and `tests/msp_130_matrix.rs` requires exactly one disposition row per Muse 1.3.0 addition in `ROADMAP.md`, with an issue link for unsupported rows.

The contributing gates are, from the repository root: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `cargo run --locked -- --selftest`, `node --test npm/test/launcher.test.cjs`, `python3 -m unittest discover -s scripts -p 'test_*.py'`, `sh -n install.sh`, and `node scripts/smoke_npm.cjs target/debug/muse-acp`. Pull requests to `master` require the `live-host` check, which runs the loopback suite against pinned Muse builds 1.3.0, 1.4.1, 1.4.2, 1.4.3, 1.4.4, and macOS 1.4.4. Feature tests that need a newer host call `require_release` in `tests/live_loopback.rs` so older legs report a skip instead of a failure. This container has Muse 1.4.7 installed, so live tests can also be run locally against the exact surface the plan targets.


## Plan of Work

Every milestone follows the same shape. Create a branch named `issue-<n>/<short-name>` from current `master`. Implement the mapping in `src/main.rs`, `src/acp.rs`, and `src/fold.rs` as needed, add fake-host support in `tests/fixtures/fake_serve.py` and integration tests in `tests/acp_serve.rs`, and extend `tests/live_loopback.rs` with a live test gated at Muse 1.4.3 or 1.4.4 where the feature is old enough for CI to run it and it can be scripted through the loopback provider. Update `README.md`, `ROADMAP.md`, `CHANGELOG.md`, and `docs/event-compatibility.md`. Run the contributing gates, push, open a pull request that says `Fixes #<n>`, review the diff, fix what the review finds, and merge when the required check is green.

For #223, `PendingUi` in `src/acp.rs` gains the owning `turn_id` and a `confirmed` list of MSP `UserInputAnswer` objects. `bridge_user_input` in `src/main.rs` asks one question per round for prompts with more than one question, reusing the existing route step. `complete_elicitation` validates and holds each answer, reissuing the next question, and sends the single `userInput/answer` only after the last one; a host rejection reissues the rejected question with the error text. The `session/cancel` handler sends `userInput/interrupt` with the held answers for any open prompt that has them, before the generic `turn/interrupt` targets are computed, and skips that prompt's turn so it is not interrupted twice.

For #218, the workflow fold in `src/fold.rs` already retains the latest workflow item per run. Add `/workflow pause|resume <workflowRunId>` (or reuse the child listing to resolve a unique run) mapping onto `workflow/pause` and `workflow/resume`, advertised in `available_commands_json`, with the admission-only ack settling the prompt while the card follows the item events. Honor the existing invariant that an in-flight `workflow/cancel` keeps the run's card until its terminal frame.

For #220, expose `hook/list` through a read-only `_session/hooks` extension and label `hookRun` items with their handler identity from the folded hook inventory.

For #215, add a `/recap` slash command that sends `recap/request` and holds the ACP prompt open until `recap/completed` arrives (the host enforces a 45-second timeout), publishing the recap text as an agent message and settling the prompt; stopping it sends `recap/cancel`, and `idle` is an accepted no-op.

For #217, expose `skill/setActivation` as an ACP configuration write and `skill/import` as a slash action, refreshing the catalog through the existing `skill/changed` path instead of a local cache.

For #222, decide the side-chat mapping and implement it: either a `/side` slash command that opens a side chat and attaches it as a new ACP session with a parent link, or a `_session/sideChat` extension on the parent, plus `SideProvenance` in `session_info_update._meta.muse`.

For #219, call `task/list` on resume and reattach to seed the adapter's async-task inventory, then keep following item events, so a restarted adapter can reconcile background work it did not observe start.

For #214, decide the plugin surface, then implement `plugin/list` reads and `plugin/install` plus the `plugin/marketplace*` writes behind it, with typed error passthrough for the four new error kinds and notification-driven refresh.

For #216, request the `computerUseSetup` capability only when a client asks for it, expose setup and settings through the adapter's ACP surface, and keep the CUA gate's refusals typed (`computerUseUnavailable`).

For #221, add the chunked `media/upload` path. Muse's video store takes one `media/upload` call per chunk (`chunkIndex`/`chunkCount`/`totalBytes`/`chunkBase64`/`mediaType`/`uploadId`), and a turn consumes the upload with a `{"type":"video","uploadId":...}` input part. Design decided here: ACP clients supply video as a `resource_link` to a local file whose `mimeType` (or extension) is a supported video type (`video/mp4`, `video/quicktime`), or as an embedded `resource` blob with a `video/*` mime type; both are currently rejected or degraded to a text reference. The prompt path reads the bytes (resource links go through the existing approved-roots local-file reader), mints one UUIDv7 `uploadId`, and uploads in fixed-size chunks (1 MiB raw, so each base64 frame stays well under the host's 10 MiB frame budget) with a fresh `commandId` per call and bounded per-chunk retries; a replayed chunk answers the recorded receipt. The receipt becomes the `video` turn part, and the upload happens before `turn/start` so a failure surfaces as a prompt error instead of a half-submitted turn. `extract_prompt_parts` gains an injected uploader callback so the prompt and steering paths can upload while pure callers pass a refusing stub that keeps the current error text.

For #224, close the remaining field-level gaps: map or explicitly ignore `webSearch` items, `mcpLinkError`, `TurnError.quota`, the new `turn/retryScheduled` and `turn/retracted` detail fields, `SessionStartParams.permissionProfile`, `TurnStartParams.userShell`, `Session.sideFrom`, and the newly required nullable fields, recording each decision in `docs/event-compatibility.md` and ROADMAP section 9. The 1.4.7 re-pin is tracked separately: `computerUseSetup/check` in #240, the `InitializeResult` sandbox-availability facts (`sandboxAvailability`, `sandboxUnavailableReason`) in #241, `SessionNameOrigin` (`Session.nameOrigin`, `SessionNameChangedParams.nameOrigin`) in #242, and `Item.cutOff` in #243.


## Concrete Steps

All commands run from the repository root, `/workspace/ce9e407e2053b7cf8057690875a06e80/muse-acp`.

Create a branch and implement one milestone:

    git checkout master
    git pull --ff-only
    git checkout -b issue-223/userinput-interrupt

Run the contributing gates and the focused tests while iterating:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo test --locked --test acp_serve <test-name>
    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback -- --test-threads 2
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Then push and open the pull request:

    git push -u origin HEAD
    gh pr create --repo BrokkAi/muse-acp --base master --title '<title>' --body-file <file>

After the review and fixes, the required `live-host` check gates the merge:

    gh pr checks <n> --repo BrokkAi/muse-acp --watch
    gh pr merge <n> --repo BrokkAi/muse-acp --merge
    git checkout master
    git pull --ff-only


## Validation and Acceptance

Each milestone's acceptance is behavior a person can verify. For #223: with a client that advertises form elicitation, answer the first of two questions, then send `session/cancel`; the host receives `userInput/interrupt` carrying that answer (not `turn/interrupt`), the form is withdrawn, and the ACP prompt settles as cancelled. A prompt with no confirmed answer still receives `turn/interrupt`, and declining a form still sends `userInput/cancel`. The integration test `partial_user_input_interrupts_with_confirmed_answers` in `tests/acp_serve.rs` proves this on the fake host, and a `live_loopback` test proves the same wire flow against real Muse 1.4.6 (older CI legs skip it through `require_release`). The full contributing gates and the PR's `live-host` check must pass before merge.


## Idempotence and Recovery

Each milestone is an additive change on its own branch; rerunning the gates and tests is safe. If a milestone turns out to be wrong at review time, the branch can be corrected with additional commits or abandoned without touching `master`, because nothing is merged until its checks are green. Schema, ROADMAP, and documentation edits are repeated per milestone but always in different rows, so concurrent branches would still merge cleanly one at a time.


## Artifacts and Notes

The relevant pinned surface is the Muse 1.4.7 schema bundle at `tests/protocol/stable/msp.schema.json`, fingerprint `sha256:6de32db08bbbe0deb478d2bfcaba001e9fb490986bc80db3aceb4d996abf7df9`. The `userinput-interrupt-round-trip` transcript under `tests/protocol/transcripts/` shows the intended wire sequence: the prompt opens, the client engages, then sends `userInput/interrupt` with one confirmed answer, and the host settles `interrupted` and completes the turn `cancelled`.


## Interfaces and Dependencies

No new Rust dependencies. The adapter stays dependency-free at runtime; tests keep using `serde_json` and the standard library. New MSP calls go through `MspHost::command` in `src/msp.rs`. New slash commands must be registered in `PROTOCOL_COMMANDS` and advertised by `available_commands_json` in `src/acp.rs`. New ACP extension methods use the `_session/` prefix; new host methods must have their params match the vendored schema bundle exactly, because `emitted_frames_conform_to_the_vendored_schema` in `tests/acp_serve.rs` validates every adapter-to-host frame against it.
