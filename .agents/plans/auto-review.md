# Session-scoped client-side auto-review

This ExecPlan is a living document. The sections `Progress`,
`Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must
be kept up to date as work proceeds.

This plan is maintained in accordance with `.agents/PLANS.md` from the
repository root. Read that file before changing this one.

> REVISED 2026-10-02: the deterministic workspace policy described below was
> rejected in review and replaced by an LLM reviewer. The shipped design is a
> second, memory-only, read-only Muse session (`--no-session-log
> --disable-write --disable-shell`) that receives a Codex-style guardian
> policy, the trusted user instructions, bounded recent evidence, and the
> exact approval request, then answers with strict JSON
> (`risk_level`, `user_authorization`, `outcome`, `rationale`). Allow answers
> select a host allowing choice; deny answers select the host reject choice
> with the rationale as feedback; reviewer failures deny. The sections below
> are kept as history of the first attempt.

## Purpose / Big Picture

Today, someone using Muse Code through an editor has two unpleasant choices.
They can leave the approval mode on `promptUnmatched` and click through every
workspace-local file edit the agent proposes, or they can switch to
`allowAll` and give up review entirely. Muse's own `:auto-review` profile is
designed to solve this, but it is a TUI-only profile: `muse serve` rejects it
with `approvalReviewerUnavailable`, and the adapter currently rewrites a saved
`:auto-review` profile to `:ask-me` in `src/host_config.rs` so the host can
start at all.

This change adds the missing middle path. Each editor session gets a
default-off Auto-review selector. When someone selects `workspace`, the
adapter itself answers approval requests for ordinary file access inside the
session's approved workspace roots, and keeps asking the human for everything
else: shell commands, network access, protected files, approvals the host
escalated to its judge, paths outside the workspace, and any request whose
only "allow" choices would create a durable grant. The host is not told about
the selector, Muse's approval mode is not changed, and no environment
variable is introduced.

After this change, a person can start a normal Muse session, choose
Auto-review: workspace, ask the agent to refactor a file, and see the edit
proceed without a permission dialog while a shell command in the same turn
still stops for approval. The adapter writes one stderr line for every
automatic approval so the decision is auditable, and the behavior is
documented in `README.md`, `docs/auto-review.md`, and `CHANGELOG.md` as a
headline capability.

## Progress

- [x] (2026-10-02 14:30Z) Rewrote GitHub issue #153 to the session-selector
  design, removed the environment-variable proposal, recorded that
  auto-review is not an `ApprovalMode` value, and made README/diagram/detailed
  docs first-class acceptance criteria.
- [x] (2026-10-02 14:36Z) Write this ExecPlan.
- [x] (2026-10-02 14:50Z) Add the `auto_review` selector to `src/acp.rs` and thread it through
  every `config_options` call site and `session/set_config_option`.
- [x] (2026-10-02 14:55Z) Add strict workspace-path eligibility and once-scope approval-choice
  selection, with unit tests.
- [x] (2026-10-02 15:05Z) Intercept eligible approvals in `open_approval`, send
  `approval/decide` directly, and add black-box regression tests with the
  fake MSP host.
- [x] (2026-10-02 15:15Z) Add the README headline section with a Mermaid diagram and marketing
  blurb.
- [x] (2026-10-02 15:15Z) Add `docs/auto-review.md` as the detailed human guide and link it from
  the README.
- [x] (2026-10-02 15:18Z) Add the `CHANGELOG.md` Unreleased entry.
- [x] (2026-10-02 15:35Z) Run formatting, clippy, targeted tests, the unit
  suite, the full integration suite, and `--selftest`; record evidence in
  `Artifacts and Notes`.
- [x] (2026-10-02 15:40Z) Complete the `Outcomes & Retrospective` entry and
  mark all remaining progress entries done or explicitly deferred.

## Surprises & Discoveries

- Observation: The vendored MSP schema already endorses the fail-closed rule
  for unknown approval subjects. `tests/protocol/stable/msp.schema.json`
  describes `ApprovalSubject` as an "Open approval subject union. Unknown
  kinds are rendered generically and never auto-approved by clients".
  Evidence: the `$defs.ApprovalSubject.description` string in the vendored
  schema.
- Observation: `confined_path` cannot be reused for eligibility as written,
  because it honors `MUSE_ALLOW_UNSCOPED_READS` and because it requires
  `std::fs::canonicalize` to succeed on the target itself, which fails for a
  file that does not exist yet. Auto-review needs a stricter sibling that
  ignores the environment flag and resolves the canonical parent for creates.
  Evidence: `src/main.rs`, `fn confined_path`.
- Observation: `PermChoice` currently drops the host's `scope` field even
  though `perm_options` parses it, so the adapter cannot tell an `allow once`
  choice from an `allow for this session` or `allow permanently` choice.
  Evidence: `src/acp.rs`, `pub struct PermChoice` and `fn perm_options`.
- Observation: Clippy's `too_many_arguments` limit (7) forced a small
  refactor. `config_options` grew to 8 parameters, so the per-session display
  values now travel in `acp::ConfigOptions`; `try_auto_review` parses the
  approval id and requirement key from its existing `params` argument instead
  of taking two more.
  Evidence: `cargo clippy --locked --all-targets -- -D warnings` reported
  "this function has too many arguments (8/7)" for both functions before the
  refactor, and is clean after it.
- Observation: The Windows sandbox cannot execute the Python fixture that the
  black-box suite uses, so every `tests/acp_serve.rs` test fails at
  `session/new` with "Muse host unavailable" when run sandboxed. Running the
  same tests outside the sandbox passes, including 245 of the 246 integration
  tests.
  Evidence: sandboxed `cargo test --locked --test acp_serve
  approval_preserves_all_choices_with_deny_option` failed with the host
  unavailable message; the same command with sandbox escalation passed.
- Observation: The one failing integration test,
  `auto_review_settings_are_cleaned_up_on_forced_shutdown`, is unrelated to
  this feature and is already documented as failing on unmodified master on
  this Windows machine. It exercises the Muse `:auto-review` settings-profile
  overlay, not the new adapter selector, and this change touches neither
  `src/host_config.rs` nor the shutdown cleanup.
  Evidence: `.agents/plans/muse-install-offer.md` records the same test as a
  pre-existing failure on a7a36f7; the test still fails when run alone, with
  "forced shutdown left temporary settings at ...".
- Observation: The fake host had to resolve the workspace path at approval
  emission time rather than import time, because the test client creates the
  session workspace after the adapter (and therefore the fixture) starts.
  `FAKE_APPROVAL_PATH=workspace` now points a file-access subject at the
  active `session/start` workspace.
  Evidence: `tests/fixtures/fake_serve.py`, `fn approval_params`.
- Observation: The shipped reviewer (`31e35c2`) sent `turn/start` with the
  pre-`input` `prompt` field, which a real Muse 1.4.x host rejects with
  `-32602 invalidParams: invalid turn/start params: missing field \`input\``.
  The reviewer turn therefore always failed, and because a reviewer failure
  denies, every approval was answered with the host's reject choice and Muse
  reported the tool as "approval aborted" on every client. The fake host and
  the black-box suite could not catch this: the fixture never validated
  `turn/start` params, and `tests/live_loopback.rs` had no test for the
  adapter's own reviewer (only for the saved `:auto-review` profile).
  Evidence: `muse serve --provider echo` 1.4.4 rejects
  `turn/start` with `prompt` and accepts it with `input`; the temporary
  revert of the one-word fix fails
  `auto_review_decides_a_shell_approval_against_a_real_host` with
  `auto-review deny …: reviewer turn failed: Invalid params: invalid
  turn/start params: missing field \`input\``.

## Decision Log

- Decision: Auto-review is a separate `configOptions` selector named
  `auto_review`, not a fifth value in the `approval_mode` selector and not a
  process environment variable.
  Rationale: `approval_mode` exists to mirror the host's closed `ApprovalMode`
  enum, and the host can change it underneath the adapter through
  `session/setApprovalMode` and `session/approvalModeChanged`. Client-side
  policy in that list would make the selector ambiguous and would mix adapter
  vocabulary into a host enum. Conversely, a per-session selector is visible,
  default-off, and does not require restarting the editor or the host.
  Date/Author: 2026-10-02, root agent, after discussion with the user.
- Decision: The selector values are `off` and `workspace`, with `off` the
  default for every new, loaded, resumed, and forked session.
  Rationale: Two values keep the selector compatible with the clients the
  adapter already supports, all of which render `select` config options.
  A boolean config option type was not verified against those clients.
  Date/Author: 2026-10-02, root agent.
- Decision: Auto-review never selects a durable choice. It chooses the first
  approving choice whose scope is `once`; if none exists it prompts.
  Rationale: Selecting `session` or `localPersistent` would silently create
  standing grants, which is a larger security decision than approving the
  action in front of the user. The issue's safety story depends on this.
  Date/Author: 2026-10-02, root agent.
- Decision: First-cut eligibility is limited to `fileAccess` subjects whose
  path (and target, when present) canonicalizes inside the session roots, with
  `judgeEscalated`, `protectedWrite`, and `subagentOrigin` requests excluded.
  Rationale: Shell and process commands can leave the workspace, network and
  Unix-socket subjects are not path-scoped, and the host's judge or protected
  write flags are explicit signals that the request is not routine. The
  vendored schema independently says unknown kinds are never auto-approved.
  Date/Author: 2026-10-02, root agent.
- Decision: An ineligible or ambiguous request falls back to the ordinary
  editor prompt instead of being denied.
  Rationale: The feature is a convenience layer for a human operator, not a
  replacement security reviewer. Downgrading an ineligible request to a
  silent denial would change Muse's behavior in a way the user did not ask
  for. This is also a deliberate difference from Codex's `auto_review`, whose
  reviewer failure mode is fail-closed denial.
  Date/Author: 2026-10-02, root agent.
- Decision: Auto-review does not call `session/setApprovalMode`, does not send
  `auto_review` to the host, and does not honor `MUSE_ALLOW_UNSCOPED_READS` in
  its path check.
  Rationale: The host mode still decides which approvals surface. Reusing the
  unscoped-read escape hatch would turn a read-only compatibility flag into a
  write-approval bypass.
  Date/Author: 2026-10-02, root agent.
- Decision: The per-session display values passed to `acp::config_options`
  travel in an `acp::ConfigOptions` struct.
  Rationale: Clippy rejects functions with more than seven arguments, and a
  named struct is clearer than a tuple or an allow attribute as the selector
  set grows. `try_auto_review` parses the approval id and requirement key it
  needs from `params` instead of taking them as extra arguments.
  Date/Author: 2026-10-02, root agent.
- Decision: Documentation ships in the same change: a prominent README
  section with a Mermaid diagram, `docs/auto-review.md`, and a CHANGELOG
  entry.
  Rationale: The user asked for Auto-review to be a headline capability and
  made README text, an entry point, and a diagram acceptance criteria for the
  issue. A feature users cannot discover or understand does not satisfy the
  intent.
  Date/Author: 2026-10-02, root agent.

## Outcomes & Retrospective

Shipped. Issue #153 now describes the session-scoped selector design and the
documentation deliverables, and the working tree implements it:

- A default-off `auto_review` config option with values `off` and `workspace`
  appears in ACP v1 and v2 `configOptions` and is settable through
  `session/set_config_option` without touching the host.
- Eligible approvals (workspace-local `fileAccess`, no protected write, no
  host-judge escalation, no subagent origin, with a once-scoped allow choice)
  are answered directly with `approval/decide` and never open
  `session/request_permission`. Everything else prompts as before.
- Path eligibility canonicalizes both sides, resolves creates through their
  canonical parent, refuses dangling symlinks and unresolvable entries, and
  ignores `MUSE_ALLOW_UNSCOPED_READS`.
- The README has a headline Auto-review section with a Mermaid flow diagram;
  `docs/auto-review.md` documents eligibility, prompt fallback, audit output,
  and the differences from Codex auto-review and Muse's TUI profile;
  `CHANGELOG.md` records the feature.

Verification evidence: `cargo fmt --check` is clean; `cargo clippy --locked
--all-targets -- -D warnings` is clean; 119 unit tests pass; 245 of 246
integration tests pass; `cargo run --locked -- --selftest` passes. The single
integration failure is the pre-existing, unrelated
`auto_review_settings_are_cleaned_up_on_forced_shutdown` documented above and
in `.agents/plans/muse-install-offer.md`.

Deferred deliberately: subagent-origin approvals, read-only or finer-grained
selector variants, and automatic selection of session or persistent grants.
Those are recorded as open questions in issue #153. If the feature is taken
further, the first follow-ups should be a live-host scenario against a real
Muse build and a decision about whether subagent approvals deserve the same
workspace-local treatment.

## Context and Orientation

`muse-acp` is a single Rust binary that sits between an editor and Muse. The
editor speaks the Agent Client Protocol (ACP) over standard input and output;
Muse is launched as `muse serve` and speaks the Muse Session Protocol (MSP)
over line-delimited JSON-RPC. The adapter translates between the two.

The files this plan touches are:

- `src/acp.rs` builds the ACP JSON shapes. `config_options` is the function
  that emits the editor's selectors: Mode, Approval Mode, Model, and
  Reasoning Effort today. `perm_options` converts MSP `availableChoices` into
  ACP permission options and the adapter-side `PermChoice` records used to map
  the editor's answer back to an MSP `approval/decide` call.
- `src/main.rs` owns the runtime. `AcpSession` is the per-editor-session
  state. `open_approval` is the single function through which every approval
  arrives, whether from an `approval/requested` notification, a reissued
  `approval/request` server request, or an `approval/updated` refresh. It
  deduplicates by approval id and requirement, queues approvals behind the
  one currently displayed, and otherwise sends `session/request_permission`
  to the editor. `send_permission_decision` sends the eventual
  `approval/decide` command. `confined_path` is the existing
  workspace-confinement helper for resource links.
- `tests/acp_serve.rs` is the black-box integration suite. It spawns the real
  adapter against `tests/fixtures/fake_serve.py`, a scripted MSP host.
  `Client::spawn` selects the scenario and passes environment variables to the
  fixture.
- `README.md` is the human landing page. `docs/` holds longer human
  documentation. `CHANGELOG.md` tracks user-visible changes.

Terms used in this plan:

- An approval is "eligible" when every condition in the first-cut eligibility
  list below holds. Only eligible approvals are answered automatically.
- A "once-scoped" choice is an MSP `availableChoices` entry with
  `decision` starting with `approved` (case-insensitive) and `scope` equal to
  `once`. It authorizes this one action and creates no standing rule.
- "Canonicalize" means calling `std::fs::canonicalize`, which resolves
  symbolic links and returns an absolute path. Two paths are compared after
  both the candidate and each root have been canonicalized.
- "Fail closed to the human" means the request is shown in the editor exactly
  as it is today. It does not mean the request is denied.

The current selector plumbing works like this. `acp::config_options` returns
one JSON array. Every call site passes the session's current values in.
`session/set_config_option` validates a requested value, forwards the
corresponding MSP command when the host owns the setting, updates the
`AcpSession` field, and returns the refreshed selector array.

## Plan of Work

The work proceeds in four milestones. Each milestone leaves the tree
compiling and the existing tests passing.

Milestone 1 adds the selector. In `src/acp.rs`, add public constants for the
two option values and a resolver, add an `auto_review` parameter to
`config_options`, and emit a fifth selector after Approval Mode. In
`src/main.rs`, add an `auto_review: bool` field to `AcpSession`, initialize it
to `false` at every construction site, pass it to every `config_options` call,
and handle `config_key == "auto_review"` in `session/set_config_option` by
validating the value, updating the session field, and returning the refreshed
selector set without contacting the host. Update the unknown-config-id error
text. Add unit tests in `src/acp.rs` for the new selector JSON and the
resolver.

Milestone 2 adds eligibility and choice selection. Extend `PermChoice` in
`src/acp.rs` with `scope: String`, populate it from the MSP choice in
`perm_options`, and add a public helper that returns the first
once-scoped approving choice id. In `src/main.rs`, add
`auto_review_path_allowed` (strict, canonicalizing, no environment override)
and `auto_review_choice` (subject kind, path rules, judge/protected/subagent
exclusions, then the once-scope helper). Add unit tests in the `src/main.rs`
test module covering inside/outside roots, creates whose parent exists,
relative paths, missing paths, shell and unknown subjects, protected and
judge-escalated flags, subagent origin, and durable-only choices.

Milestone 3 wires the interception into `open_approval`. After `open_approval`
computes the approval id and requirement key, it calls a new
`try_auto_review` helper. The helper returns false when the session is off or
the request is ineligible, in which case the existing display path runs
unchanged. When it returns true it deduplicates against `approval_seen`,
writes the audit line, sends the approving decision through
`send_permission_decision`, and returns so no editor request is opened. Add
black-box tests plus a fixture knob to point the fake host's file-write
approval at a chosen path. Tests cover the automatic path in both ACP
protocol versions, the default-off state, and prompt fallback for shell and
out-of-root paths.

Milestone 4 is documentation and verification. Add the README section with
the Mermaid diagram and marketing blurb, add `docs/auto-review.md`, add the
CHANGELOG entry, run the full gate from `CONTRIBUTING.md`, and record the
outcomes here.

## Concrete Steps

All commands run from the repository root,
`C:\Users\ryan\.codex\worktrees\c43d\muse-acp` on the machine where this plan
was written. On macOS and Linux the same commands work from the checkout root.

Milestone 1:

1. Edit `src/acp.rs`: add `AUTO_REVIEW_OFF`, `AUTO_REVIEW_WORKSPACE`, and
   `resolve_auto_review`; add the selector to `config_options`; update the
   function's documentation comment; extend the existing unit tests.
2. Edit `src/main.rs`: add the `AcpSession` field and initialize it at the
   three `AcpSession { … }` sites (new session, load/resume, fork); pass it to
   the seven `acp::config_options` calls; add the `auto_review` arm to
   `session/set_config_option`.
3. Run `cargo test --locked acp::tests` and expect the new selector tests to
   pass.

Milestone 2:

1. Edit `src/acp.rs`: add `scope` to `PermChoice`, populate it in
   `perm_options`, add `approve_once_choice`.
2. Edit `src/main.rs`: add `auto_review_path_allowed`,
   `auto_review_choice`, and unit tests.
3. Run `cargo test --locked auto_review` and expect the new unit tests to
   pass.

Milestone 3:

1. Edit `src/main.rs`: add `try_auto_review` and call it from
   `open_approval` before the display/queue block.
2. Edit `tests/fixtures/fake_serve.py`: honor `FAKE_APPROVAL_PATH` for the
   `file-write` subject.
3. Edit `tests/acp_serve.rs`: add the automatic-approval and fallback tests.
4. Run `cargo test --locked --test acp_serve auto_review` and expect all
   auto-review integration tests to pass.

Milestone 4:

1. Edit `README.md`, add `docs/auto-review.md`, and edit `CHANGELOG.md`.
2. Run the full gate:

       cargo fmt --check
       cargo clippy --locked --all-targets -- -D warnings
       cargo test --locked
       cargo run --locked -- --selftest

   Expect formatting and clippy to be clean, all tests to pass, and
   `--selftest` to print the offline compatibility table without errors.

## Validation and Acceptance

The behavior is accepted when all of the following are observable.

First, the selector exists and defaults to off. A client that creates a
session and inspects the `session/new` result sees a fifth selector with
`"id"` (ACP v1) or `"configId"` (ACP v2) equal to `auto_review` and
`"currentValue":"off"`. Sending
`session/set_config_option` with `configId: "auto_review"` and
`value: "workspace"` returns the refreshed selector with
`"currentValue":"workspace"`, and the fake host log shows no corresponding
MSP method.

Second, an eligible approval is automatic. With the fake host configured to
emit a `fileAccess` approval whose path is inside the session workspace, the
client enables `auto_review=workspace`, asks for a turn, and then observes no
`session/request_permission` frame. The fake host's captured input contains an
`approval/decide` command whose `choiceId` is the host's once-scoped allowing
choice, and the adapter stderr contains a line beginning with `auto-review`.

Third, ineligible approvals still prompt. With the default shell subject, or
with a `fileAccess` path outside the session roots, the client receives
`session/request_permission` exactly as before. When the only allowing choice
has `scope` `session` or `localPersistent`, the client also receives the
prompt.

Fourth, the host contract is unchanged. No new MSP method is sent, no
`session/setApprovalMode` command is triggered by selecting `auto_review`, and
`MUSE_APPROVAL_MODE` behavior is untouched.

The integration test names to watch for are
`auto_review_approves_workspace_file_access_without_prompting` and
`auto_review_still_prompts_for_ineligible_requests`. The unit tests to watch
for are `auto_review_selector_defaults_off` in `src/acp.rs` and
`auto_review_eligibility_is_workspace_strict` in `src/main.rs`. Each fails
before the corresponding milestone and passes after it.

## Idempotence and Recovery

Every edit is additive or a parameter threading change, so rerunning the steps
is safe. The tests create their own temporary directories under the system
temporary directory and remove nothing outside them. If a test run leaves a
stale adapter or fake host process, it exits when its standard input closes;
the test client also waits for child exit and fails loudly if it does not.

If the feature misbehaves in a live session, the recovery is to select
`auto_review=off` (or start a new session, which defaults off). No host state
is written by the selector, so turning it off is always sufficient.

The issue body draft used to update GitHub is kept at
`.agents/docs/issue-153-auto-review.md`. It is a record, not an input to the
build; deleting it does not affect the implementation.

## Artifacts and Notes

The issue update this plan implements is
<https://github.com/BrokkAi/muse-acp/issues/153>. Its acceptance criteria and
documentation requirements are repeated in this plan so a future contributor
does not need GitHub access.

Expected audit line shape:

    [muse-acp] auto-review approved ap-1 (workspace-files, fileAccess) with c-allow

## Interfaces and Dependencies

In `src/acp.rs`, the following must exist at the end of Milestone 2:

    pub const AUTO_REVIEW_OFF: &str = "off";
    pub const AUTO_REVIEW_WORKSPACE: &str = "workspace";
    pub const AUTO_REVIEW_HELP: &str = "off|workspace";

    pub fn resolve_auto_review(value: &str) -> Option<&'static str>;

    pub struct PermChoice {
        pub id: String,
        pub decision: String,
        pub scope: String,
        pub accepts_feedback: bool,
    }

    /// First approving choice scoped to this one action, in host order.
    pub fn approve_once_choice(choices: &[PermChoice]) -> Option<String>;

`acp::config_options` takes a new `ConfigOptions` struct and emits the new
selector regardless of protocol version using the same `id`/`configId` key
selection as the other selectors:

    pub struct ConfigOptions<'a> {
        pub session_mode: &'a str,
        pub approval_mode: &'a str,
        pub model: &'a str,
        pub reasoning_effort: &'a str,
        pub offer_muse_default: bool,
        pub auto_review: bool,
        pub recommendations: (Option<&'a str>, Option<&'a str>),
    }

In `src/main.rs`, the following must exist at the end of Milestone 3:

    // AcpSession field
    pub auto_review: bool,

    /// Strict workspace check for auto-review. Unlike `confined_path`, this
    /// ignores `MUSE_ALLOW_UNSCOPED_READS` and accepts a not-yet-created file
    /// whose canonical parent is inside a root.
    fn auto_review_path_allowed(path: &str, roots: &[String]) -> bool;

    /// The choice id to approve automatically, or None to prompt.
    fn auto_review_choice(params: &J, roots: &[String]) -> Option<String>;

    /// Returns true when the approval was answered automatically.
    fn try_auto_review(
        host: &Arc<Hosts>,
        stdout: &StdoutShared,
        sessions: &Sessions,
        acp_sid: &str,
        owner_msp_sid: &str,
        params: &J,
    ) -> bool;

No new crate dependency is required. The implementation uses `std::fs` for
canonicalization, the existing JSON helpers for parsing, and the existing
`send_permission_decision` path for the MSP command. `auto_review_choice`
calls `acp::perm_options` once to get the `PermChoice` records; when the
request is ineligible the ordinary display path calls it again, which is
harmless because it only rebuilds the same JSON and records.

## Revision Notes

- 2026-10-02 14:36Z: Created. Records the session-selector decision, the
  eligibility boundary, the once-scope rule, the prompt-fallback rule, and the
  README/diagram/detailed-docs deliverables agreed with the user.
- 2026-10-02 15:40Z: Implementation and documentation completed. Recorded the
  `ConfigOptions` refactor, the sandbox/Python test constraint, the one
  pre-existing integration failure, the fixture workspace-path knob, and the
  verification evidence. No scope changed from the original plan.
