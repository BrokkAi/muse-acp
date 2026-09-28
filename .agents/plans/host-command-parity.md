# Close the remaining Muse 1.3.0 host-command gaps


This ExecPlan follows `.agents/PLANS.md` and is maintained during implementation.


## Purpose / Big Picture


`muse-acp` is an adapter. Editors such as Zed and JetBrains speak ACP (the Agent Client Protocol) to it over stdio, and it speaks MSP (the Muse Session Protocol, JSON-RPC) to a `muse serve` host process. Muse 1.3.0 added a set of host methods and notifications. Most of them are now wired through, but the documentation still describes several as unsupported, and three host commands still have no editor path.

After this plan, an editor user can rename the current session by typing `/rename <name>` in the prompt box. They can also skip or retry one child of a running workflow with a slash command, without leaving the editor. The ROADMAP's Muse 1.3.0 disposition table will describe what the adapter actually does today. `task/background` stays deliberately unbuilt, and the reason is recorded where the next contributor will find it.

To see it working, run the integration tests named in Validation and Acceptance. They drive the real adapter binary against the scripted fake host in `tests/fixtures/fake_serve.py`. Then, against a live `muse serve`, type `/rename My session` in an ACP editor and watch the session title change.


## Progress


- [x] (2026-09-28) Step 0, SDK re-pin: vendored `muse-code-sdk` revision `a7c10c5` into `tests/protocol/`. Opened as PR #139 on branch `brb/sdk-repin-a7c10c5`; not yet merged.
- [x] (2026-09-28) Adopted the ExecPlan convention (`AGENTS.md`, `.agents/PLANS.md`, this file) on branch `brb/adopt-execplans`.
- [ ] Step 1, roadmap reconcile (its own PR, after #139 merges).
- [ ] Step 2, `/rename` mapped to `session/rename` (feature PR).
- [ ] Step 3a, retain workflow children per run in fold state.
- [ ] Step 3b, make child ids discoverable from the editor.
- [ ] Step 3c, the workflow child-control slash command.
- [ ] Step 3d, remove the hardcoded `childControlUnavailableReason`.
- [ ] Step 4, record the `task/background` rationale on a tracking issue.


## Surprises & Discoveries


Every tracking issue the ROADMAP links for the still-unbuilt surfaces is closed: #38 (`task/background`), #41 (`session/rename`), and #46 (`workflow/childControl`). Check with `gh issue view 41 --json state`. The test `tests/msp_130_matrix.rs` requires every row whose disposition is "Unsupported pending protocol decision" to link a `https://github.com/BrokkAi/muse-acp/issues/` URL, and it passes with closed issues. Linking a closed issue is misleading, though.

The re-pinned schema (`tests/protocol/stable/msp.schema.json` after PR #139) defines both `SessionRenameParams` and `WorkflowChildControlParams`. The integration tests validate every adapter-to-host request frame against the bundle's required fields and property types, so the new commands must match the schema exactly. They are not skipped the way `goal/*` was under the old pin.

The workflow item itself carries `children[]`. Each child has required fields `childId`, `attempt` (an integer, at least 1), and `status`, and optional `label`, `phase`, `durationMs`, `resultRef`, `terminal`, and `usage`. The fold in `src/fold.rs` already renders `label: status (phase)` lines from this array but throws away `childId` and `attempt`. Step 3 therefore needs to retain data the adapter already receives, not invent new event handling.

An earlier scratch plan listed `session/delete` as an undecided row to keep. No such row exists in the ROADMAP matrix, and `tests/msp_130_matrix.rs` does not list it. Ignore it.


## Decision Log


Decision: ship as separate pull requests, in the order step 0, step 1, then steps 2 and 3. Rationale: the re-pin is the audit-heavy change and deserves its own reviewable diff, and a stall in step 3 must not block the documentation or `/rename`. Date: 2026-09-28.

Decision: once the SDK manifest fingerprint equals the live-validated 1.3.0-R3401.1 host fingerprint, `compat::SDK_MANIFEST_FINGERPRINT` aliases `HOST_130_R3401_1_FINGERPRINT` and classifies as tested. Rationale: the surface is identical, and two consts with the same value in one `match` are an unreachable pattern that fails clippy with `-D warnings`. Landed in PR #139. Date: 2026-09-28.

Decision: the workflow child-control command never takes `attempt` from the user. It reads `attempt` from the adapter's latest copy of the workflow item. Rationale: the schema says a stale attempt is rejected with `stale_attempt` and must never be guessed. Surface that rejection verbatim rather than retrying. Date: 2026-09-28.

Decision: do not build `task/background`. Rationale: see the step 4 section of Plan of Work. Date: 2026-09-28.

Decision: rows that stay "Unsupported pending protocol decision" must link an open issue. Reopen #38, #41, and #46, or file replacements; either needs the maintainer's approval because it is visible on GitHub. Date: 2026-09-28.


## Outcomes & Retrospective


Step 0 is complete in PR #139. The corpus diff was purely additive, and all CONTRIBUTING gates passed locally. Nothing else has landed yet.


## Context and Orientation


Protocol conformance inputs live in `tests/protocol/`: the stable manifest, the JSON schema bundle `stable/msp.schema.json`, and golden transcripts. They are copied verbatim from the upstream `muse-code-sdk` repository at a single revision recorded in `tests/protocol/PROVENANCE.md`. `src/compat.rs` classifies the fingerprint a host reports during `initialize` as tested, degraded, fixture, unknown, or incompatible. It only affects log lines and `--selftest` output.

Two documents are test-enforced. `docs/event-compatibility.md` has a table between the markers `<!-- schema-notifications:start -->` and `<!-- schema-notifications:end -->`. A unit test in `src/compat.rs` requires that table to list every and only the notifications in the schema bundle's `notifications` index. `ROADMAP.md` section 9 has a table between `<!-- msp-1.3.0-matrix:start -->` and `<!-- msp-1.3.0-matrix:end -->`. `tests/msp_130_matrix.rs` requires exactly one row for each name in its `METHODS`, `NOTIFICATIONS`, `ERRORS`, and `REQUESTS` lists, with a valid disposition, and an issue link for unsupported rows.

The `/goal` slash command is the pattern to copy for new commands. It has four parts. First, `parse_goal_command` in `src/main.rs` turns prompt text into a host method plus arguments, or a usage error. It returns `None` when the text isn't a `/goal` command, and a leading space escapes any slash command. Second, an intercept block in the `session/prompt` handler of `src/main.rs` (search for `goal_attempt`) runs before the prompt would become a turn. It sends the host command with a freshly minted `commandId` (`host.mint_cmd("cmd-")`) and `sessionId`, then settles the ACP prompt immediately. ACP v2 gets an empty result, an echoed user message, and `idle`; ACP v1 gets a `user_message_chunk` and `end_turn`. A parse error is JSON-RPC `-32602`. Third, `available_commands_json` in `src/acp.rs` advertises the command to the editor with an input hint, and filters any host skill of the same name. Fourth, `tests/fixtures/fake_serve.py` acknowledges the `goal/*` methods (search `goal/set`), and `goal_slash_commands_map_to_host_goal_methods` in `tests/acp_serve.rs` tests the whole path.

`workflow/cancel` is already exposed through the AIR async-task stop path in `src/main.rs` (search for `"workflow/cancel"`). It sends `commandId`, `sessionId`, and `workflowRunId`. AIR is the adapter's negotiated ACP extension for observing and stopping background work. Its extensions are `async_task/stop`, `readOutput`, `steering`, and `userShell`.


## Plan of Work


Step 1 is a documentation-only reconcile of the ROADMAP section 9 table, done after PR #139 merges. The prose above the table ("The pinned schema bundle predates the Muse 1.3.0 additions…") is stale once the bundle is re-pinned, so rewrite it. Rewrite the disposition and behavior cells for surfaces that have landed: `goal/*`, `skill/list`, `skill/changed`, `task/stop`, `task/stopAll`, `usage/read`, `usage/changed`, `view/subscribe`, `workflow/cancel`, `session/setReasoningEffort`, `session/reasoningEffortChanged`, `session/statusChanged`, `session/nameChanged`, `session/viewHealthChanged`, `item/readOutput`, `skillNotFound`, and `outputUnavailable`. For each, read the code path (search `src/` for the method name) and describe the behavior; `docs/event-compatibility.md` already has accurate wording for the notifications. Keep `task/background` and `session/modelRouteUnserved` (intentionally ignored) as they are. Leave the `session/rename` and `workflow/childControl` rows for steps 2 and 3. Relink every row that stays unsupported to an open issue, per the Decision Log.

Step 2 adds `/rename <name>`, which maps to `session/rename` with params `commandId`, `name`, and `sessionId`, all required strings. The result carries `commandId`, `status`, and an optional normalized `name`. Add `parse_rename_command` beside `parse_goal_command`. A bare `/rename` or whitespace-only name is a usage error. Intercept it in the prompt handler the same way `/goal` is intercepted, and settle the prompt the same way. Advertise `rename` in `available_commands_json` with the hint `<name>`, and filter any host skill named `rename`. Make `fake_serve.py` acknowledge `session/rename`, and emit `session/nameChanged` so the test can observe the title update the adapter already publishes through `session_info_update`. Add `rename_slash_command_maps_to_host_method` to `tests/acp_serve.rs`. It covers the mapping, the settle, the bare-name usage error, and the leading-space escape. Then update the README command list, the ROADMAP row for `session/rename`, and `docs/event-compatibility.md` if it lists commands.

Step 3 adds workflow child control. It is the largest step and has four parts.

In 3a, keep the latest `children[]` of each workflow item in fold state, keyed by `workflowRunId`, next to the existing workflow async-task ids in `src/fold.rs`. Replace it whenever a newer item snapshot arrives. Add fold unit tests, including one where a retry increments `attempt`.

In 3b, make children discoverable. Include each child's `childId` in the rendered content line, as `label [childId]: status (phase)`. A bare command, or one naming an unknown child, returns a usage error that lists the current run's children with id, status, and attempt.

In 3c, add the command. Choose a name, such as `/workflow skip <childId>` and `/workflow retry <childId>`, and record the choice in the Decision Log. It maps to `workflow/childControl` with `action` (`skip` or `retry`), `attempt` taken from 3a's state, `childId`, `commandId`, `sessionId`, and `workflowRunId`. The ack is admission-only: the prompt settles on the ack, and the child's new state arrives later as ordinary workflow item events. If more than one workflow run is active, find the child id across all runs; reject an id that appears in more than one. Surface host errors, including `stale_attempt`, with their typed message.

In 3d, remove the hardcoded `childControlUnavailableReason` meta entry from the workflow arm of the fold. The same four deliverables as step 2 apply: parser, intercept, advertising, and a fake-host ack with an integration test. Also update the ROADMAP row for `workflow/childControl`.

Step 4 builds nothing. `task/background` asks the host to send a running foreground tool task to the background (`sessionId`, `commandId`, and `taskId`, which is the tool call's `itemId`). AIR lets an editor observe and stop background work, but no ACP or AIR affordance lets an editor start backgrounding. The host can still background its own tools, and that path is unaffected. Building this would mean inventing an extension no client implements, or an ambiguous `/background` command with no host TUI syntax to mirror. Copy this rationale onto the open tracking issue for `task/background` so it outlives this plan. Revisit only if AIR gains an affordance or the host documents a TUI equivalent.


## Concrete Steps


Run everything from the repository root. Before each pull request, run the CONTRIBUTING gates:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

To run one integration test while iterating:

    cargo test --locked --test acp_serve rename_slash_command_maps_to_host_method

Before merging, `gh pr checks <number>` must show every check passing. Committing, pushing, and merging each need the maintainer's explicit approval.


## Validation and Acceptance


Each new test must fail before its fix and pass after it. Confirm this by running it once before implementing. `cargo test --locked` must be fully green, including the matrix-versus-schema unit test in `src/compat.rs`, `tests/msp_130_matrix.rs`, and the request-frame schema validation.

Step 2 is accepted when typing `/rename Release prep` in an ACP client sends exactly one `session/rename` with `name` `"Release prep"`, settles the prompt without starting a turn, and the resulting `session/nameChanged` updates the session title. `/rename` with no name returns `-32602` with a usage message. ` /rename x`, with a leading space, is sent to the host as ordinary prompt text.

Step 3 is accepted when, in a session with a running workflow, the editor shows each child's id and the child-control command sends `workflow/childControl` with the child's current attempt, matching the fake host's recorded `children[]`. A deliberately stale attempt in the fake host produces a visible `stale_attempt` error, not a silent retry.


## Idempotence and Recovery


All steps are additive code and documentation edits on a feature branch, so they can be repeated safely. If a matrix test fails after a docs edit, the failure names the missing or duplicate row. Fix that row rather than editing the test. To redo the SDK re-pin, delete `tests/protocol/stable` and `tests/protocol/transcripts` and copy `schema/msp/stable` and `schema/msp/transcripts` from a checkout of the recorded upstream revision.


## Artifacts and Notes


The `a7c10c5` bundle compared with the earlier `fbce769` bundle has 185 → 234 `$defs`, none removed, no new required fields, and 23 → 31 notifications. The manifest fingerprint is `sha256:7469c9e352e67def4a59df7e439984d7194fa351e1c8b7abb34060fd977ced81`.


## Interfaces and Dependencies


No new crates are needed; the binary stays dependency-free. In `src/main.rs`, define:

    fn parse_rename_command(text: &str) -> Option<Result<String, String>>
    fn parse_workflow_child_command(text: &str) -> Option<Result<(String, String), String>>

The workflow parser returns `(action, childId)`. Fold state gains a map from `workflowRunId` to the last-seen children, where each entry keeps at least `childId`, `attempt`, `status`, and `label`.


Revision note (2026-09-28): converted from the scratch `plans.md` into ExecPlan form. Recorded step 0 as done in PR #139. Added the closed-issue, new-schema, and `children[]` findings, and dropped the nonexistent `session/delete` row.
