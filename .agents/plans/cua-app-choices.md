# Adopt Computer Use app-level choices

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

Muse 1.4.8 exposes the effective per-app Computer Use policy in `ComputerUseSettings.apps`, can include installed and running apps for a picker, and accepts per-app choice patches. The adapter currently reaches only the capability-wide dropdowns and the older blocked-app list.

After this change, an editor user can see each app's effective choice, display name, last-asked time, and lock reason, ask the host for an installed-app picker, and set one or more app choices to `ask`, `alwaysAllow`, or `blocked` in one atomic patch. Locked or malformed app patches keep the host's typed refusal instead of becoming a generic failure.


## Progress

- [x] (2026-10-19) Read issue #253, the vendored Computer Use schemas, the existing extension and `/cua` parser, fake-host behavior, and live test.
- [x] (2026-10-19) Created branch `issue-253/cua-app-choices` from `origin/master` after PR #290 merged.
- [x] Extend `_session/computerUse` reads with the `installed` request member.
- [x] Render app rows and installed-app rows in `/cua` text.
- [x] Add a multi-app `/cua app` patch action.
- [x] Add an installed-app picker command.
- [x] Add fake-host, field-matrix, and live-host coverage for app rows, installed rows, patches, and typed refusals.
- [x] Update README, ROADMAP, event compatibility, CHANGELOG, and this plan.
- [x] Run all contribution gates.
- [ ] Open a pull request, complete three review/fix passes, and merge only after final CI is green.


## Surprises & Discoveries

- Observation: `ComputerUseSettings.apps` is absent only on older hosts; a host that serves the field always returns it.
  Evidence: the vendored `ComputerUseSettings.apps` description states this explicitly. The renderer therefore treats absence as unknown rather than inventing app rows.

- Observation: `apps` is the effective policy view, not merely the user's block list.
  Evidence: each `ComputerUseApp` has a required effective `choice`, and a block wins over a grant. Locked rows are still part of the same list.

- Observation: the app writer has stricter collision rules than the old block/unblock writer.
  Evidence: duplicate bundle IDs inside `apps`, or overlap with `blockApps`/`unblockApps`, are refused by the host. The adapter sends the `apps` array intact so those refusals remain typed.

- Observation: locked apps are always present in the effective `apps` list even when `showBuiltIns` is false; that flag controls only the lower-level `blockedApps` list.
  Evidence: the 1.4.8 schema defines `apps` independently, and the fake-host regression initially exposed an old assertion that treated the two lists as one.


## Decision Log

- Decision: add `/cua app <bundleId> <choice> [<bundleId> <choice> ...]` for one or more per-app choices.
  Rationale: one shape handles a single row and an atomic multi-row patch, while the closed choice vocabulary can still be validated before the host call. Existing `/cua block` and `/cua unblock` remain available for older workflows.
  Date/Author: 2026-10-19, Codex.

- Decision: add `/cua apps` as the human-readable installed-app picker.
  Rationale: `/cua status` should remain a bounded policy summary. `/cua apps` requests `installed: true`, then renders the existing app rows and the installed/running picker source.
  Date/Author: 2026-10-19, Codex.

- Decision: accept `"installed": true` on `_session/computerUse` and forward it to `computerUseSettings/read`.
  Rationale: structured clients need the same picker source as the command. The settings object stays verbatim, so `apps` and `installedApps` reach the client without adapter-side reinterpretation.
  Date/Author: 2026-10-19, Codex.

- Decision: render app rows as `Name [bundleId]: choice`, adding last-asked and lock facts on the same line.
  Rationale: this preserves host order and exposes every field the issue requires without introducing a new structured response for `/cua` command text. The structured extension remains the source for programmatic rendering.
  Date/Author: 2026-10-19, Codex.

- Decision: keep the old `blockedApps` section as an older-host fallback and render `apps` separately when present.
  Rationale: `apps` and `blockedApps` have different shapes and meanings. Absence of `apps` must not hide the older list, and presence of `apps` need not erase the lower-level block facts.
  Date/Author: 2026-10-19, Codex.

- Decision: preserve the host's typed refusal for locked apps, duplicate IDs, and overlapping patch members.
  Rationale: `computerUseSettings/update` applies all-or-nothing and already returns typed errors through the existing host error path. Local parsing should validate only the command vocabulary and shape, not replace host policy refusals with generic text.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation, all contribution gates, and live validation are complete. The opt-in extension now forwards the installed-app request, `/cua status` renders effective app rows, `/cua apps` exposes the picker, and `/cua app` writes one atomic multi-app patch while keeping typed host refusals. PR review passes and merge remain.


## Context and Orientation

`src/main.rs` owns the adapter-local `/cua` parser and execution path and the opt-in `_session/computerUse` extension. The execution path uses the host's `computerUseSettings/read` and `/update` methods. `CuaAction` is the parser's typed action set; `run_cua` gates calls by the client opt-in and the independent per-session/host capabilities; `cua_settings_text` renders command output.

`tests/fixtures/fake_serve.py` simulates the host settings and can inject typed host failures. `tests/acp_serve.rs` drives the real adapter binary and checks the frames. `tests/live_loopback.rs` exercises a real Muse host but must accept the platform's existing Computer Use gate/refusal posture.

`tests/protocol/stable/msp.schema.json` defines `ComputerUseSettings`, `ComputerUseSettingsReadParams`, `ComputerUseSettingsUpdateParams`, `ComputerUseApp`, `ComputerUseAppPatch`, and `ComputerUseInstalledApp`. `ROADMAP.md` section 9 and `docs/event-compatibility.md` document stable-surface dispositions. `README.md` is the user-facing Computer Use guide.


## Plan of Work

Extend `CuaAction` with an installed-app read and a multi-app patch. Parser rules are:

- `/cua apps` reads settings with `installed: true`.
- `/cua app <bundleId> <choice> [<bundleId> <choice> ...]` requires at least one pair, validates each choice against `ask|alwaysAllow|blocked`, and sends `apps` as an ordered array of objects.

Keep the existing status, host, setup, dropdown, block, and unblock actions unchanged. Update the usage string and the ACP command hint.

In `_session/computerUse`, read an optional boolean `installed` request member and include it in the `computerUseSettings/read` params alongside `showBuiltIns`. Do not mutate the returned settings.

Extend `cua_settings_text` to render `apps` rows in host order with the host name, bundle ID, effective choice, optional `lastAskedAt`, and `locked`/`lockedBy`. Render `installedApps` under a separate heading when supplied, and retain `blockedApps` for older hosts. Empty or malformed arrays are tolerated without fabricated rows.

Update the fake host to return representative app and installed-app rows, apply app patches to returned settings, and return typed `commandRejected` errors for locked or duplicate app targets. Add integration tests for structured reads with and without `installed`, `/cua status`, `/cua apps`, multi-app updates, typed refusals, and parser usage errors. Extend the live 1.4.8 test opportunistically: when the platform gate succeeds, assert app/installed arrays and a safe app patch; when the platform returns its existing typed CUA refusal, keep that behavior as the proof.

Update README, ROADMAP section 9 and the field matrix, `docs/event-compatibility.md`, CHANGELOG, and this plan. Run every contribution gate, open a PR with `Fixes #253`, complete three review/fix passes, and merge only after final CI is green.


## Concrete Steps

Run commands from:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Run focused tests while iterating:

    cargo fmt
    cargo test --locked cua -- --nocapture
    cargo test --locked --test acp_serve cua -- --nocapture
    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback computer_use_reads_reach_a_real_host_when_negotiated -- --nocapture

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

The `_session/computerUse` response with `installed: true` carries the host's `settings.apps` and `settings.installedApps` fields verbatim when the host grants Computer Use. Without the request member, it does not ask for installed apps. `/cua status` text includes each app's host name or bundle ID, effective choice, optional last-asked time, and a visible lock reason.

`/cua apps` sends `installed: true` and renders the installed/running app source. `/cua app com.example.One alwaysAllow com.example.Two blocked` sends one `apps` array with both patches and renders the returned settings. A locked target and a duplicate bundle ID preserve the host's typed `commandRejected` error and cause no local state change.

All existing Computer Use tests remain green, including opt-in, independent grants, typed `computerUseUnavailable`, host setup, and runtime-busy behavior. The full repository gates and focused live test are green. The PR closes #253, receives three documented review/fix passes, and is merged only at a green final head.


## Idempotence and Recovery

Reads have no persistent adapter state. The app writer is one all-or-nothing host patch; a failed call leaves host policy unchanged. The fake host models this by applying valid patches only to the returned settings. Tests use disposable homes and settings, so live validation never changes the developer's policy. Re-running tests is safe.


## Artifacts and Notes

The expected command shapes are:

    /cua status
    /cua apps
    /cua app com.example.One alwaysAllow com.example.Two blocked

The expected app row text resembles:

    Apps:
    Acme Editor [com.acme.Editor]: alwaysAllow (last asked 2026-10-01T12:00:00Z)
    Finder [com.apple.finder]: blocked (locked by builtIn)


## Interfaces and Dependencies

`CuaAction` gains an installed-app read variant and an app-patch variant carrying ordered `(bundleId, choice)` pairs. `parse_cua_command` validates only closed choices and pair shape. `run_cua` forwards reads and writes through the existing typed host-call helper. `cua_settings_text` renders the new host fields without changing existing output for hosts that omit them. No dependency is added.

Change note (2026-10-19): review pass 1 corrected the live app-patch assertion so it requires the unique probe bundle id in the returned settings text instead of matching an earlier generic settings frame.
