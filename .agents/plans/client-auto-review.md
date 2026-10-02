# Approve routine workspace file work on the user's behalf (auto-review)

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


Muse asks for permission before most tool actions. Through `muse-acp`, every such request becomes an editor permission prompt (`session/request_permission`), so a session that edits ten files asks ten times. The only way to stop the prompts used to be the `allowAll` approval mode, which also lets shell commands, network access, and MCP tools through without asking. GitHub issue #153 asked for an opt-in middle ground: the adapter answers the routine requests itself and still asks about everything else.

After this change, the editor's approval-mode selector has a fifth entry, **Auto-review**. With it selected, Muse runs in its `promptUnmatched` mode, and `muse-acp` approves, without asking, each request that only reads or edits an ordinary file inside the session's workspace roots. Shell commands, network access, MCP and other tools, deletes and moves, files outside the workspace, files under hidden folders such as `.git`, and anything the adapter does not recognize still reach the editor. `MUSE_APPROVAL_MODE=autoReview` starts every session in this mode.

To see it working, run `cargo test --locked --test acp_serve auto_review`. The test `auto_review_approves_workspace_edits_once_without_asking` shows two workspace edits approved with the host's allow-once choice and no editor prompt; `auto_review_still_asks_about_commands_and_files_outside_the_workspace` shows a shell command and an outside file still prompting.


## Progress


- [x] (2026-10-02 09:00Z) Read the approval path (`open_approval` in `src/main.rs`) and the MSP approval schema in `tests/protocol/stable/msp.schema.json`.
- [x] (2026-10-02 09:20Z) Added the `autoReview` selector mode and its host mapping in `src/acp.rs`, and threaded it through session new, load, resume, fork, host restart, `session/set_config_option`, `session/set_mode`, and `session/approvalModeChanged` in `src/main.rs`.
- [x] (2026-10-02 09:40Z) Added `src/auto_review.rs` (eligibility) with unit tests, and hooked it into `open_approval`.
- [x] (2026-10-02 10:10Z) Fake host knob `FAKE_APPROVAL_SUBJECT=workspace-file` and three integration tests in `tests/acp_serve.rs`.
- [x] (2026-10-02 10:20Z) README headline and section, `MUSE_APPROVAL_MODE` row, ROADMAP guardrail wording, CHANGELOG.


## Surprises & Discoveries


- Observation: the MSP schema already anticipates client auto-approval. `ApprovalSubject` says "Unknown kinds are rendered generically and never auto-approved by clients", and every approval request carries required `protectedWrite` and `judgeEscalated` booleans.
  Evidence: `tests/protocol/stable/msp.schema.json`, definitions `ApprovalSubject` and `ApprovalRequestParams`.

- Observation: a real file-write approval looks like `{"kind":"fileAccess","toolName":"write_file","path":"/home/me/src/proj/Cargo.toml","access":"write"}` with choices `allow_once` (`approved`, scope `once`), `allow_session` (`approvedForSession`), and `abort`.
  Evidence: `tests/protocol/transcripts/approval-round-trip/transcript.ndjson`.

- Observation: mapping an unknown future host mode through `mode_from_msp` yields `promptUnmatched`, so a naive fold kept auto-review on under a host posture the adapter does not understand. The unit test caught it; `fold_mode` now compares the raw host mode.
  Evidence: `acp::tests::auto_review_runs_on_prompt_unmatched_and_only_the_adapter_reports_it` failed with `futureMode` before the fix.


## Decision Log


- Decision: expose auto-review as a fifth value of the existing approval-mode selector (`mode` config option and legacy `modes`), with id `autoReview`, rather than as an environment flag or a separate toggle.
  Rationale: the selector is where users already choose between prompting and `allowAll`, it renders in every ACP client the adapter supports, and choosing it there is an explicit per-session opt-in. A second toggle would need a config option type editors may not render, and could be combined with `allowAll`, where it means nothing.
  Date/Author: 2026-10-02, Claude.

- Decision: auto-review runs on the host's `promptUnmatched` mode, and the selector shows `autoReview` only while the host reports exactly `promptUnmatched`. Any other host mode, including one this adapter does not know, ends it.
  Rationale: `promptUnmatched` is the mode in which Muse asks about unmatched actions, so it is the one whose prompts auto-review can answer. Ending it on any other report keeps the selector honest when another client changes the mode, and fails closed for unknown modes.
  Date/Author: 2026-10-02, Claude.

- Decision: approve only `subject.kind == "fileAccess"` with access `read`, `list`, `stat`, `search`, `write`, `create`, `append`, `edit`, or `modify`; require `protectedWrite` and `judgeEscalated` to be present and `false`; refuse requests with approval stages.
  Rationale: these are the routine actions the issue targets. Deletes and moves are harder to undo, shell and network effects cannot be confined to a path, and MCP tools are opaque. Requiring the flags to be present means an older or unusual host fails closed.
  Date/Author: 2026-10-02, Claude.

- Decision: the path must resolve inside a canonical workspace root, and no component below the root may start with a dot. A path that does not exist resolves through its nearest existing ancestor, but only if each missing name truly does not exist (`symlink_metadata` reports `NotFound`). Paths that are relative or contain `..` are refused.
  Rationale: canonicalizing follows symbolic links, so a link inside the workspace cannot lead a write outside it. A dangling link would otherwise resolve to its own path and be approved, after which Muse would write through it. Hidden folders hold version control internals (`.git/hooks` runs code), CI definitions (`.github/workflows` runs with secrets), editor tasks, and environment files.
  Date/Author: 2026-10-02, Claude.

- Decision: always choose the host's `approved` + `once` choice, never a session or persistent rule. If the host offers none, ask the editor.
  Rationale: a saved rule would keep approving after the user turns auto-review off, and would apply to requests auto-review never examined.
  Date/Author: 2026-10-02, Claude.

- Decision: decide eligible requests immediately, even while another permission prompt is displayed, and if the host rejects the decision, show the request to the editor instead.
  Rationale: queueing an approvable request behind an unrelated prompt would stall the turn for no reason. A rejected decision (for example a stale requirement) must not leave the host waiting.
  Date/Author: 2026-10-02, Claude.

- Decision: log each approval (`auto-review approved <access> <path> once`) and each request left to the editor, with its reason, to the adapter's stderr log.
  Rationale: the issue asked for auditability, and the reason line explains an unexpected prompt.
  Date/Author: 2026-10-02, Claude.

- Decision: Muse cannot store auto-review with the session, so a loaded or resumed session starts in `promptUnmatched` unless `MUSE_APPROVAL_MODE=autoReview` is set, a fork inherits its source's selector value, and a host restart keeps it.
  Rationale: losing the mode on reload errs toward more prompts. The environment variable is the operator's standing choice and applies wherever the adapter would otherwise guess.
  Date/Author: 2026-10-02, Claude.


## Outcomes & Retrospective


Auto-review shipped as described. The integration tests show workspace edits approved with the allow-once choice and no editor prompt, and shell commands and outside files still prompting. The unit tests cover eligibility, symbolic links, and the mode fold. Remaining gaps: the read-only and plan modes of issue #159 should be designed alongside this selector, and there is no live-host test yet (issue #157).


## Context and Orientation


`muse-acp` is a Rust binary that an editor launches. It talks ACP (Agent Client Protocol, newline-delimited JSON-RPC) with the editor on stdin and stdout, and MSP (Muse Session Protocol) with a `muse serve` child process, called the host. When a Muse tool needs permission, the host sends `approval/requested` (and sometimes reissues it as an `approval/request` server request) with an `approvalId`, a `subject` describing the action, `availableChoices`, and `currentRequirementId`. The adapter answers with the MSP command `approval/decide`, naming one `choiceId`.

`src/main.rs` holds request handling. `open_approval` deduplicates approval events per session (`AcpSession::approval_seen`), queues a second approval while one is displayed (`perm_queue`), and otherwise sends `session/request_permission` to the editor. `complete_permission` turns the editor's answer into `approval/decide`. Each session record (`AcpSession` in `src/acp.rs`) has `mode_value`, the value the approval-mode selector shows, and `roots`, the session's workspace roots (the ACP `cwd` plus `additionalDirectories`).

`src/acp.rs` defines `APPROVAL_MODES`, the selector entries, and `config_options` and `session_modes`, which render them. `tests/fixtures/fake_serve.py` is a scripted fake host used by `tests/acp_serve.rs`.


## Plan of Work


In `src/acp.rs`, add `AUTO_REVIEW` (`"autoReview"`) to `APPROVAL_MODES`, plus `host_approval_mode` (the MSP mode to send for a selector value) and `fold_mode(current, host_mode)` (the selector value after the host reports a mode). Keep `mode_from_msp` unable to return `autoReview`.

In `src/main.rs`, send `host_approval_mode(..)` wherever a selector value goes to the host (`MUSE_APPROVAL_MODE` at `session/start`, `session/set_config_option`, `session/set_mode`), and use `fold_mode` wherever a host-reported mode updates `mode_value` (session new, load and resume, fork, host restart, `session/approvalModeChanged`). `loaded_mode_seed` returns `autoReview` when `MUSE_APPROVAL_MODE=autoReview`, for loaded sessions.

Create `src/auto_review.rs` with `review(params, roots) -> Result<String, &'static str>`, returning the choice id to send or the reason to ask the editor. In `open_approval` (now `show_approval` with an `auto_review` flag), call `review_on_behalf` before taking the session lock, mark an eligible approval seen instead of queueing it, and send the decision with `approve_on_behalf`. If the host rejects it, unmark it and show it to the editor.


## Concrete Steps


From the repository root:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo test --locked --test acp_serve auto_review

Expect all tests to pass, including the three `auto_review_*` integration tests added here and the unit tests `auto_review::tests::*` and `acp::tests::auto_review_runs_on_prompt_unmatched_and_only_the_adapter_reports_it`.


## Validation and Acceptance


In Zed or a JetBrains IDE with this build, open a Muse thread, choose **Auto-review** in the mode selector, and ask Muse to edit a file in the project. No permission prompt appears, and the agent log shows `auto-review approved write <path> once`. Ask it to run a shell command: a permission prompt appears, and the log shows `auto-review: asking the editor about approval ... because it is not a file access`. Ask it to edit `.github/workflows/ci.yml`: a prompt appears.


## Idempotence and Recovery


The change is code-only and has no persistent state. Turning auto-review off by choosing another mode takes effect on the next request, because no rules are saved.


## Artifacts and Notes


An approval log line from the integration test:

    [muse-acp] auto-review approved write /tmp/acp-fake-.../workspace/src/lib.rs once (approval ap-1)


## Interfaces and Dependencies


In `src/acp.rs`:

    pub const AUTO_REVIEW: &str = "autoReview";
    pub fn host_approval_mode(mode: &str) -> &str;
    pub fn fold_mode(current: &str, host_mode: &str) -> &'static str;

In `src/auto_review.rs`:

    pub fn review(params: &J, roots: &[String]) -> Result<String, &'static str>;

No new dependencies.
