# Adopt the Muse 1.4.x MSP additions the adapter still refuses

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture

`muse-acp` is an adapter between two protocols. Editors such as Zed and JetBrains speak ACP (Agent Client Protocol) to the adapter over stdio. The adapter speaks MSP (Muse Session Protocol, JSON-RPC) to a `muse serve` host process. Every host method the adapter does not drive is a feature an editor user cannot reach through the adapter even though Muse itself supports it.

Muse 1.4.3 through 1.4.6 added a list of methods, notifications, capabilities, and vocabulary that the adapter deliberately left unmapped when the vendored schema corpus was re-pinned to SDK revision `ea88c822` (Muse 1.4.6). Those gaps are recorded in `ROADMAP.md` section 9 ("Muse 1.4.x stable-surface additions") with a tracking issue each, filed as GitHub issues #214 through #224. This plan adopts them, one pull request per issue, merging each only when its checks are green. The corpus has since advanced twice more, to SDK `a0c8611` (Muse 1.4.7, 2026-10-13) and then `838d36a` (Muse 1.4.8, 2026-10-14); their additions — 1.4.7's `computerUseSetup/check`, the sandbox-availability facts, `SessionNameOrigin`, and `Item.cutOff` (#240–#243), and 1.4.8's Computer Use app choices, agent-message phase, `monitorFeed`, generated session title, resume permission-profile pick, skill `title`, `turn/modelRetryScheduled`, and `sessionRename` (#253–#260) — have their own tracking issues instead of becoming milestones here.

The user-visible result depends on the issue: an editor user can pause and resume a workflow run (`/workflow pause` and `/workflow resume`), interrupt a pending question while keeping the answers already confirmed (`userInput/interrupt`), ask for a while-you-were-away recap (`/recap`), inspect the session's hooks and background tasks, toggle or import skills, open a side chat, inspect and manage plugins, drive Muse's computer-use setup, attach video (or other large media) to a turn, and see the remaining 1.4.x wire fields mapped or explicitly ignored rather than silently dropped. The last part, issue #224, is as much documentation as code: the goal is that every field-level addition has a recorded disposition. Issue #260 keeps `/rename` working exactly as before while bringing the adapter in step with the 1.4.8 capability registry, so a host that later starts enforcing the `sessionRename` grant gets the typed `capabilityRequired` rather than an unexplained failure.

To see it working, run the per-issue test command named in each milestone below; each names an integration test in `tests/acp_serve.rs` that drives the real adapter binary against the scripted fake host in `tests/fixtures/fake_serve.py`. The container this plan was written in also has a real Muse binary (1.4.8 at the latest re-pin), so the strongest proof is `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback`, which runs the adapter against a real `muse serve` whose model calls go to a scripted loopback provider.


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
- [x] (2026-10-14) Re-pinned the vendored corpus to SDK `838d36a` (Muse 1.4.8): stable manifest and schema bundle, the 52 transcripts, `src/compat.rs`, `tests/protocol/PROVENANCE.md`, `ROADMAP.md` (reference snapshot, §1 compatibility facts, and §9 rows), `docs/event-compatibility.md`, and `CHANGELOG.md`. The new surface is tracked in #253–#260. Documentation-only, matching the 1.4.7 re-pin.
- [x] (2026-10-15) Issue #224 remaining 1.4.x field-level and vocabulary additions: merged in PR #266 (per-field disposition matrix in ROADMAP §9 enforced by `tests/msp_140_field_matrix.rs`, `TurnError.quota` and `mcpLinkError` folded into the turn-failure text, the `turn/retryScheduled`/`turn/retracted` 1.4.6 detail fields added to diagnostics, a `webSearch` generic-card fold test, the fake-host quota/retry/declined-retract scenarios, and a deterministic wait for the reasoning-stream test). The issue is closed.
- [x] (2026-10-15) Issue #260 `sessionRename`: merged in PR #267 (the capability requested unconditionally at `initialize` and recorded on the handshake, `/rename` kept advertised with a typed `capabilityRequired` refusal when a 1.4.8+ host withholds the grant, the `capabilityRequired` error kind forwarded as ACP `-32010` with its typed data, fake-host withheld/pre-registry scenarios, and a live test across the registry entry). The issue is closed.
- [x] (2026-10-15) Issue #243 `Item.cutOff`: merged in PR #268 (a cut-off reply is marked once with `_meta.muse.cutOff` and the host `itemId` from `item/updated`, `item/completed`, a gap refill, or history replay; the mark rides the resent or replayed text, a late mark names only the latest settled reply and waits out a newer streaming reply on ACP v1, the field joined the ROADMAP §9 field matrix, and a fake-host scenario plus a live 1.4.7+ loopback test cover it). The issue is closed.
- [x] (2026-10-19) Re-pinned the vendored corpus to SDK `ae46934` (Muse 1.4.10, carrying the 1.4.9 stable surface): stable manifest and schema bundle (byte-identical to an installed Muse 1.4.10's own export), the 52 transcripts, `src/compat.rs` (new fingerprint degraded pending a pinned CI build), `tests/protocol/PROVENANCE.md`, `ROADMAP.md` (reference snapshot, §1 compatibility facts, §2, and §9 rows), `docs/event-compatibility.md` (new `approval/reviewing` row), the `Item.displayLabel` field-matrix row enforced by `tests/msp_140_field_matrix.rs`, and `CHANGELOG.md`. The new surface is tracked in #271–#273. Documentation-only, matching the 1.4.7/1.4.8 re-pins.
- [ ] (2026-10-19) Issue #241 sandbox-availability facts: implemented on branch `issue-241/sandbox-availability`, PR pending (`HandshakeInfo` records `sandboxAvailability`/`sandboxUnavailableReason`, a `host-sandbox` startup line plus a warning on `unavailable`, a `support-host-sandbox` handshake probe in `--support`, an offline `unknown` row in `--selftest`, fake-host variants, a live test that asserts a real posture on 1.4.7+ and `unknown` before it, and both fields in the ROADMAP §9 field matrix).


## Surprises & Discoveries


- Observation: the ACP elicitation contract cannot carry partial answers. `elicitation/create` is answered exactly once, and only the `accept` action carries any content; `decline` and `cancel` carry none. Therefore the adapter can only hold "confirmed partial answers" if it collects answers one question at a time itself. This shapes the #223 milestone.
  Evidence: the ACP schema at `agentclientprotocol/agent-client-protocol` defines `ElicitationAction` as `Accept(ElicitationAcceptAction)`, `Decline`, `Cancel`, with the action flattened and content only on the accept arm.

- Observation: Muse 1.4.6 reuses a workflow's run id for the resumed generation. A paused run named `workflow-run-model-tool-call_1` resumed as an item with both `workflowRunId` and `resumeFromRunId` equal to `workflow-run-model-tool-call_1`, a fresh item id, `triggerSource: "guidanceAuto"`, and a restarted revision sequence. The fold therefore treats a same-id `resumeFromRunId` as a generation boundary (bypassing the revision guard) instead of a supersession to a different run.
  Evidence: a live 1.4.6 run paused via `/workflow pause`, resumed via `/workflow resume`, and the resumed card frame carried both ids equal; the host's session record shows the resumed launch as a new `workflow_run_launched` event under the same run id.
- Observation (1.4.8 re-pin): the new `sessionRename` capability is registered and grantable but not enforced by the installed Muse 1.4.8. A connection that never requests it still gets an accepted `session/rename`, so requesting the grant is forward-compatibility, not a fix for a live failure. Tracked in [#260](https://github.com/BrokkAi/muse-acp/issues/260).
  Evidence: `muse serve` 1.4.8 handshakes with `requestedCapabilities: ["sessionListStream","sessionMcp","feedback"]` and grants exactly those three; a `session/rename` on that connection returned `{"commandId": "...", "name": "probe-name", "status": "accepted"}`. The same handshake plus `"sessionRename"` granted all four, and rename again returned `status: "accepted"`.

- Observation (#260): an absent `sessionRename` grant does not mean the same thing on every host. Muse 1.4.3 through 1.4.7 ignore the unknown requested name and withhold it, yet still serve `session/rename`; only 1.4.8+ has the name in its grantable registry, so only there does an absent grant mean the host refused the capability. Gating `/rename` on the raw grant would therefore have regressed every pre-1.4.8 pin in the live matrix.
  Evidence: a direct 1.4.7 handshake requesting `["sessionListStream","sessionMcp","feedback","sessionRename"]` answered `grantedCapabilities: ["sessionMcp","sessionListStream","feedback"]` (probed in the container that wrote this plan), and `HandshakeInfo::allows_session_rename` plus the `rename_still_works_on_a_host_that_predates_the_capability` fake-host test pin that reading.



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

- Decision: treat the Muse 1.4.8 re-pin as documentation-only, with no adapter feature code, matching the 1.4.7 re-pin. The vendored corpus advances to SDK `838d36a`, `src/compat.rs` classifies the new fingerprint as degraded until a pinned 1.4.8 build joins the live-host matrix, and each new surface the adapter does not drive gets one tracking issue (#253–#260) instead of a milestone here.
  Rationale: the re-pin is a maintenance step that keeps the vendored schema and the event matrices honest, and every new 1.4.8 surface is additive, so the adapter tolerates it unchanged. Filing issues rather than implementing keeps each adoption reviewable on its own, which is how the 1.4.3–1.4.7 additions were handled.
  Date/Author: 2026-10-14, Codex.

- Decision: for #224, map the fields that earn an ACP representation and record an explicit disposition for the rest. `TurnError.quota` maps because the schema tells clients to branch on its presence and never on the message (the failure text now names the limit kind and the reset wait), `TurnErrorKind.mcpLinkError` maps to an added MCP-link remedy, the `webSearch` item kind maps through the existing unknown-kind generic card path, and the newly required-but-nullable params (`branch`, `providerId`, `modelId`, `goal`, `sideFrom`) map to the metadata/totals they already feed. The `turn/retryScheduled` (`errorKind`/`httpStatus`/`system`) and `turn/retracted` (`outcome`) fields are `Consumed` diagnostics, and `SessionStartParams.permissionProfile` plus `TurnStartParams.userShell` are `Intentionally ignored` because the adapter drives those surfaces through `approvalMode` and `session/userShell`.
  Rationale: this satisfies the issue's suggested scope (a per-field decision recorded in ROADMAP §9 and `docs/event-compatibility.md`) without inventing ACP primitives. `tests/msp_140_field_matrix.rs` mirrors the §9 rows and asserts each addition is still present — and, where relevant, still null-tolerant — in the vendored bundle, so a future re-pin cannot drop one silently. `declined` retracts settle the prompt like `accepted` ones because the interrupt that produced the retract already cancelled the turn; the outcome only changes whether the user message rolls back, which ACP has no primitive for.
  Date/Author: 2026-10-15, Codex.

- Decision: for #260, record the `sessionRename` grant on the handshake and gate `/rename` on it only for hosts whose registry contains the name (`at_least(1, 4, 8)`). An absent grant on an older host is "unknown name ignored", not a refusal, because those hosts still serve `session/rename`.
  Rationale: gating on the raw `sessionRename` grant alone would have broken `/rename` on Muse 1.4.3 through 1.4.7, all of which are pinned in the live-host matrix. The adapter requests the capability unconditionally so a future enforcing host grants it, records the grant so the posture is observable, and refuses only where the registry makes the absent grant meaningful - answering with the host's own typed `capabilityRequired` shape, which is what the issue's acceptance criteria ask for. The alternative, relying only on the host to refuse an ungranted call, would not hold on the live 1.4.8 host, which accepts `session/rename` without the grant; the local gate is what makes the refusal deterministic. Forwarding the `capabilityRequired` kind as `-32010` with its typed data also keeps a host-enforced refusal typed, which it was not before (it fell back to a generic `-32603`).
  Date/Author: 2026-10-15, Codex.

- Decision: for #241, keep the sandbox facts as diagnostics only (disposition `Consumed`), classify them through a closed adapter-side vocabulary, and have `--support` run one real `initialize` handshake to report them.
  Rationale: the schema calls the facts host properties that are never negotiated and tells clients to read absent as unknown and never infer a reason, so the `host-sandbox` line prints only `available`, `unavailable`, or `unknown`, and a reason only beside `unavailable` (an unrecognized one reads as `unknown`). Host strings are never echoed, so nothing unsanitized reaches the log. Nothing is gated on the facts, as the issue asks; `unavailable` adds a warning line. The support bundle previously never handshook, so without the probe it could not carry the facts; the probe is bounded by the 30-second `initialize` budget and reports `status=unavailable` when the host cannot start, so the bundle still succeeds. `--selftest` stays offline and prints `unknown`. The live test asserts `unknown` before 1.4.7 rather than skipping, because those hosts provably never send the field.
  Date/Author: 2026-10-19, Claude.


## Outcomes & Retrospective

The 0.13.0 and 0.14.0 releases landed most of the 1.4.x surface; #224 is the
last milestone and closes with a documented, test-enforced disposition for
every remaining 1.4.6 field and vocabulary addition rather than new protocol
primitives. The per-field matrix in ROADMAP §9 is now the single place to look
when a re-pin adds a field: if it is not in the table, `tests/msp_140_field_matrix.rs`
cannot see it, and a dropped member fails the same test.


## Context and Orientation

The adapter's entry point is `src/main.rs`. It runs one loop that reads ACP messages from the editor's stdin, MSP messages from the host process, and dispatches both. `handle_acp` handles editor requests; `handle_msp` handles host notifications. Host methods are sent through `MspHost::command`, with `commandId`-based idempotency where MSP requires it (`mint_cmd`). ACP session state lives in `AcpSession` (`src/acp.rs`), including the fold (`src/fold.rs`) that turns MSP item and turn events into ACP updates, pending permissions, pending user-input forms, and in-flight prompts.

There are two ways an editor reaches a host feature. Either ACP core carries it (a new session, a prompt, a cancel), or the adapter teaches the editor a slash command (like `/goal`, `/rename`, `/workflow-child`, `/feedback`, `/compact`) and advertises it through `available_commands_json` in `src/acp.rs`. Slash commands have four moving parts, all in `src/main.rs`: a parser beside `parse_goal_command`, an entry in `PROTOCOL_COMMANDS`, an intercept in the `session/prompt` handler that sends the host method and settles the ACP prompt, and a fake-host acknowledgement plus an integration test in `tests/fixtures/fake_serve.py` and `tests/acp_serve.rs`. The AI Review extension (`AIR`) is the negotiated extension surface for observing and stopping background work; its methods are `_session/steering`, `_session/readOutput`, `_session/userShell`, and `_session/async_task/stop`.

Protocol conformance inputs live in `tests/protocol/`: `stable/msp.schema.json` is the vendored stable schema bundle for SDK revision `838d36a` (Muse 1.4.8), and `stable/manifest.json` pins its fingerprint. `src/compat.rs` classifies the fingerprint a live host reports; the 1.4.8 fingerprint classifies as degraded until a pinned 1.4.8 build joins CI. The vendored `transcripts/` corpus has hand-authored golden transcripts, including `userinput-interrupt-round-trip`, which shows the intended `userInput/interrupt` wire flow. Two unit tests enforce documentation: `every_schema_notification_has_an_explicit_matrix_disposition` in `src/compat.rs` requires `docs/event-compatibility.md` to classify every and only published schema notification, and `tests/msp_130_matrix.rs` requires exactly one disposition row per Muse 1.3.0 addition in `ROADMAP.md`, with an issue link for unsupported rows.

The contributing gates are, from the repository root: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `cargo run --locked -- --selftest`, `node --test npm/test/launcher.test.cjs`, `python3 -m unittest discover -s scripts -p 'test_*.py'`, `sh -n install.sh`, and `node scripts/smoke_npm.cjs target/debug/muse-acp`. Pull requests to `master` require the `live-host` check, which runs the loopback suite against pinned Muse builds 1.3.0, 1.4.1, 1.4.2, 1.4.3, 1.4.4, and macOS 1.4.4. Feature tests that need a newer host call `require_release` in `tests/live_loopback.rs` so older legs report a skip instead of a failure. This container has Muse 1.4.8 installed, so live tests can also be run locally against the exact surface the plan targets.


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

For #260, request `sessionRename` unconditionally in `MspHost::launch` and record the grant as `HandshakeInfo::session_rename`. The 1.4.8 registry makes the name grantable but does not enforce it, so a decision is needed about what an absent grant means: Muse 1.4.3 through 1.4.7 predate the entry, ignore the unknown requested name, and still serve `session/rename`, which the adapter has driven since `/rename` was added. `HandshakeInfo::allows_session_rename` therefore treats an absent grant as a refusal only on a host that knows the name (`at_least(1, 4, 8)`, the release whose registry added it). `handle_acp` answers `/rename` on such a host with the host's own typed `capabilityRequired` shape (`msp::capability_required_error`: code `-32010`, `data.kind` `capabilityRequired`, `data.capability` `sessionRename`) instead of sending the call. `msp::acp_error_code` maps the kind to `-32010` and `msp::skill_error_data` forwards its typed data, so a host that enforces the grant and refuses also stays typed rather than degrading to the generic `-32603`. The fake host withholds the grant under `FAKE_NO_SESSION_RENAME` but still answers `session/rename`, modelling the live 1.4.8 host, and `FAKE_SERVER_VERSION` selects whether the adapter's gate applies, so one scenario proves the refusal and another proves the pre-registry fallback.


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

For #260: start the adapter against a host that reports itself as Muse 1.4.8 and withholds the `sessionRename` grant (`FAKE_SERVER_VERSION=1.4.8` plus `FAKE_NO_SESSION_RENAME=1`), then send `/rename <name>`. The prompt fails with error code `-32010`, `data.kind` `capabilityRequired`, and `data.capability` `sessionRename`, and no `session/rename` reaches the host. Repeat with `FAKE_SERVER_VERSION=1.4.7`: the same withheld grant is not a refusal there, `/rename` reaches the host, and the ACP title updates. Both are pinned by `rename_is_refused_with_the_typed_capability_error_when_the_grant_is_withheld` and `rename_still_works_on_a_host_that_predates_the_capability` in `tests/acp_serve.rs`, and `rename_drives_a_real_host_across_the_capability_registry` in `tests/live_loopback.rs` proves the same split against whatever real build the live-host matrix pins, reading the adapter's own `session rename: granted=… available=…` startup line.


## Idempotence and Recovery

Each milestone is an additive change on its own branch; rerunning the gates and tests is safe. If a milestone turns out to be wrong at review time, the branch can be corrected with additional commits or abandoned without touching `master`, because nothing is merged until its checks are green. Schema, ROADMAP, and documentation edits are repeated per milestone but always in different rows, so concurrent branches would still merge cleanly one at a time.


## Artifacts and Notes

The relevant pinned surface is the Muse 1.4.8 schema bundle at `tests/protocol/stable/msp.schema.json`, fingerprint `sha256:4e3200c3c9dc0cd7919d11022630d43f9bc1c5cdda375381e90b0183b5665069`. The `userinput-interrupt-round-trip` transcript under `tests/protocol/transcripts/` shows the intended wire sequence: the prompt opens, the client engages, then sends `userInput/interrupt` with one confirmed answer, and the host settles `interrupted` and completes the turn `cancelled`.


## Interfaces and Dependencies

No new Rust dependencies. The adapter stays dependency-free at runtime; tests keep using `serde_json` and the standard library. New MSP calls go through `MspHost::command` in `src/msp.rs`. New slash commands must be registered in `PROTOCOL_COMMANDS` and advertised by `available_commands_json` in `src/acp.rs`. New ACP extension methods use the `_session/` prefix; new host methods must have their params match the vendored schema bundle exactly, because `emitted_frames_conform_to_the_vendored_schema` in `tests/acp_serve.rs` validates every adapter-to-host frame against it.
