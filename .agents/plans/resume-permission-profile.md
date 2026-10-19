# Adopt the resume permission-profile pick

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

Muse 1.4.8 lets `session/resume` apply a launch permission-profile pick after restoring the session's retained authority. Editors using `muse-acp` currently have no way to request that pick. After this change, an ACP client can set `_meta.muse.permissionProfile` on `session/load` or `session/resume`; the adapter forwards the selected profile only on the underlying MSP `session/resume`, reports the resumed session, and preserves the host's typed refusal for unknown or unavailable profiles.

`session/fork` remains profile-free as the schema requires. Omitting the metadata preserves current behavior exactly: the host restores retained authority without an override.


## Progress

- [x] (2026-10-19) Read issue #257, the vendored resume/start permission-profile schemas, the ACP resume/load and fork paths, and existing profile diagnostics.
- [x] (2026-10-19) Created branch `issue-257/resume-permission-profile` from `origin/master` after PR #291 merged.
- [x] (2026-10-19) Add strict `_meta.muse.permissionProfile` request parsing.
- [x] (2026-10-19) Build the MSP resume request member only for normal load/resume.
- [x] (2026-10-19) Preserve typed host profile refusals.
- [x] (2026-10-19) Prove fork never carries the member.
- [x] (2026-10-19) Add fake-host, field-matrix, and live 1.4.8 coverage.
- [x] (2026-10-19) Update README, ROADMAP, CHANGELOG, and this plan. No event-compatibility edit was needed because this is a lifecycle request member, not a notification disposition change.
- [x] (2026-10-19) Run all contribution gates.
- [x] (2026-10-19) Open PR #292 with `Fixes #257`.
- [x] (2026-10-19) Complete three review/fix passes.
- [ ] Merge only after final CI is green.


## Surprises & Discoveries

- Observation: the permission-profile pick belongs only to the initial load/resume request, not to internal re-attach or usage-refresh calls.
  Evidence: the schema applies it during the host's pre-marker restore-and-select step. Reapplying it on a later adapter reattach would change the meaning from a client launch choice to an adapter policy.

- Observation: profile admission failures use the typed `commandRejected` error (`-32030`) with `permission_profile_unknown` or `permission_profile_unavailable`.
  Evidence: both `SessionStartParams.permissionProfile` and `SessionResumeParams.permissionProfile` descriptions define that refusal shape.

- Observation: the existing resume error path flattened host errors through `send_error`.
  Evidence: `session/resume` / `session/load` constructed a fallback message and called `acp::send_error`, which could not preserve the host's typed profile data.


## Decision Log

- Decision: expose the pick as `_meta.muse.permissionProfile` on ACP `session/load` and `session/resume`.
  Rationale: ACP has no core permission-profile field. The established namespaced metadata surface lets a client make a request-scoped choice without changing ordinary resume behavior for clients that do not know the extension.
  Date/Author: 2026-10-19, Codex.

- Decision: require a non-empty string. Absent means no override; explicit null, empty, or non-string values fail locally with `-32602`.
  Rationale: MSP rejects explicit null and selects only catalog ids. Failing before the host call gives deterministic validation and preserves the existing omitted behavior.
  Date/Author: 2026-10-19, Codex.

- Decision: add the pick to `resume_session` as an optional value, but pass `None` from adapter-owned reattach and recovery paths.
  Rationale: only the client's original load/resume request chose that profile. Reattaching an existing session should restore retained authority without reinterpreting an earlier UI choice.
  Date/Author: 2026-10-19, Codex.

- Decision: preserve the profile on the MCP-configuration conflict retry.
  Rationale: the retry exists only to omit conflicting MCP configuration. The host still needs the client's profile choice in the same pre-marker resume attempt.
  Date/Author: 2026-10-19, Codex.

- Decision: use `send_host_error` for generic resume failures and leave the special session-deleted translation local.
  Rationale: typed `commandRejected` data must survive, while a missing session has a more useful ACP-specific error.
  Date/Author: 2026-10-19, Codex.

- Decision: ignore profile metadata on `session/fork`; do not attempt to translate or reject the fork itself.
  Rationale: the issue's hard invariant is that the MSP fork request never carries the member. A client may reuse the same metadata object across lifecycle calls, and fork still needs to work.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation and local validation are complete. A client-selected resume
profile is forwarded only through ACP load/resume, malformed metadata fails
before any host call, typed host refusals retain their `commandRejected` data,
and fork never carries the member. The full Cargo suite, focused fake-host and
live tests, format, clippy, selftest, npm, Python, packaging, and shell gates
pass. PR #292 completed three review/fix passes:

- The first pass added crash/reattach coverage proving adapter recovery does
  not repeat a client's launch profile.
- The second pass clarified that consumers must select an existing
  host-catalog id and that malformed metadata fails before the host call.
- The third pass extended typed-refusal coverage to the documented ephemeral
  host reason.

Final CI and merge remain.


## Context and Orientation

`src/main.rs` handles ACP `session/load`, `session/resume`, and `session/fork`. ACP load/resume call `resume_session`, which sends MSP `session/resume` with a command id, session id, history preference, and optional MCP configuration. The helper also retries once without MCP configuration when a loaded session rejects a conflicting server set. `reattach_session` calls the same helper for adapter-owned recovery and must not carry a launch profile.

The host's permission profile is not the adapter's `approval_mode` selector. It is a named host policy chosen at session start or resume. The existing adapter supports the host launch default and `:auto-review` compatibility, but does not expose the host profile catalog. This change uses profile ids supplied by the client.

`tests/fixtures/fake_serve.py` scripts MSP lifecycle responses and can inject typed errors. `tests/acp_serve.rs` drives the adapter binary. `tests/live_loopback.rs` uses an isolated Muse 1.4.8+ host. `tests/msp_140_field_matrix.rs`, `ROADMAP.md`, and `docs/event-compatibility.md` keep the stable-surface disposition documented.


## Plan of Work

Add a request helper that reads `_meta.muse.permissionProfile`. Treat no member as `Ok(None)`, a non-empty string as `Ok(Some(id))`, and null/empty/non-string as an invalid-params message. Invoke it early in the ACP load/resume handler before the host call.

Extend `resume_session` with `permission_profile: Option<&str>` and emit `,"permissionProfile":...` only when supplied. Pass the client value from the ACP load/resume path and `None` from `reattach_session`. The MCP-conflict retry must pass the same profile. Internal `backfill_usage` and other direct resume reads already omit it.

Change the generic resume error path to `send_host_error`, retaining `-32030` and its `data` for profile admission refusals. Do not add the saved-default-profile settings hint when a client explicitly selected a profile; the host refusal already names the requested id and reason. Keep the session-not-found translation.

Update the fake host to log the member, accept a known test profile, and return typed `commandRejected` errors for `:missing` and `:unavailable`. Add integration tests for ACP v1 load and v2 resume with metadata, absent metadata, invalid values, typed unknown/unavailable refusals, MCP-conflict retry preservation, and fork omission. Add live 1.4.8 coverage that probes the host catalog behavior without requiring a particular named profile: if the host offers a known id through the existing isolated settings/test profile, resume with it; otherwise verify the typed unknown-profile refusal from an obviously absent id.

Update README, ROADMAP section 9 and the field matrix, `docs/event-compatibility.md` if lifecycle wording changes, CHANGELOG, and this plan. Run all gates, open a PR with `Fixes #257`, complete three review/fix passes, and merge after final CI is green.


## Concrete Steps

Run commands from:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Run focused tests:

    cargo fmt
    cargo test --locked resume_permission_profile -- --nocapture
    cargo test --locked --test acp_serve resume_permission_profile -- --nocapture
    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback resume_permission_profile -- --nocapture

Run all contribution gates:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Validation and Acceptance

An ACP `session/resume` with `_meta.muse.permissionProfile: "team-safe"` reaches MSP as `session/resume.params.permissionProfile = "team-safe"` and returns the resumed ACP session. ACP v1 `session/load` does the same. Omitting the metadata sends no member and keeps the current resume behavior.

Null, empty, and non-string values fail with `-32602` before an MSP resume call. The fake host's `:missing` and `:unavailable` refusals reach the client as `-32030` with `data.kind = "commandRejected"` and the host's reason. The same profile survives the MCP-conflict retry.

`session/fork` never sends `permissionProfile`, even when the ACP request carries the metadata. Existing session/start and fork behavior is unchanged. The live 1.4.8 test records whether a chosen id was accepted or refused with the typed profile error. All repository gates are green. The PR closes #257, receives three documented review/fix passes, and merges at a green final head.


## Idempotence and Recovery

The profile pick is request-scoped and the adapter stores no local profile state. A refused resume performs no durable host write by contract; retrying with the same command id is unnecessary because the adapter mints a fresh command id per request. Tests use isolated homes and can be rerun safely.


## Artifacts and Notes

The expected ACP request fragment is:

    {"sessionId":"...","_meta":{"muse":{"permissionProfile":"team-safe"}}}

The expected MSP request fragment is:

    {"sessionId":"...","history":"inline","permissionProfile":"team-safe"}

A fork request carrying the metadata still sends no `permissionProfile`.


## Interfaces and Dependencies

`src/main.rs` gains a strict request-metadata parser returning `Option<String>`. `resume_session` gains an optional profile parameter and preserves it on its configuration-conflict retry. `reattach_session` passes `None`. No new dependency is added.
