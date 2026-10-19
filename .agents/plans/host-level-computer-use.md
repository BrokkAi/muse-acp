# Adopt Muse host-level computer-use setup

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

`muse-acp` is an adapter between ACP, the protocol spoken by editors, and MSP, the JSON-RPC protocol spoken to a `muse serve` host process. Muse 1.4.7 added a host-level computer-use setup path beside the existing per-session setup path. Without this change, an editor user can inspect and start computer use for one session but cannot ask whether the computer itself has the three macOS privacy grants computer use reports, and a sessionless setup-change notice is reported as unhandled. Input Monitoring is reported separately because basic use can be ready without it.

After this change, an editor user who opts into Muse computer use can type `/cua host` to read the host's setup state and `/cua host check` to ask the host to inspect its Screen Recording, Accessibility, and Input Monitoring grants without opening System Settings. Both commands work through an ordinary ACP text prompt and agent message, so a client does not need to implement the `_session/computerUse` custom extension merely to use them. If the host grants only the per-session capability, the host-level command stays dark and explains which grant is missing. If another setup attempt owns the computer-use helper, `/cua host check` returns the host's typed `commandRejected` refusal with reason `runtime_busy`.

The commands still require the ACP client's existing `_meta.muse.computerUse` opt-in. With that opt-in, the adapter asks Muse for both `computerUseSetup` and `computerUseHostSetup`. The existing `/cua`, `/cua status`, and `/cua on` behavior remains session-scoped.

To see the change working, run the integration test named in Validation and Acceptance. It drives the real adapter against the fake MSP host and checks the negotiated capabilities, command output, absent-grant behavior, typed busy refusal, and sessionless change notice. A live loopback test covers a real Muse 1.4.7 or later host, accepting the host's platform-dependent grant and gate postures.


## Progress

- [x] (2026-10-19 12:20Z) Read issue #240, the existing computer-use implementation from #236, the vendored 1.4.7 schema, fake-host support, live-host coverage, and the repository contribution and ExecPlan rules.
- [x] (2026-10-19 12:20Z) Created branch `issue-240/host-computer-use` from current `origin/master`.
- [x] (2026-10-19 12:35Z) Added separate handshake request and grant tracking for `computerUseHostSetup`.
- [x] (2026-10-19 12:42Z) Added `/cua host` and `/cua host check`, preserving existing session targets and fail-closed gates.
- [x] (2026-10-19 12:42Z) Forwarded typed `commandRejected` data as ACP code `-32030`.
- [x] (2026-10-19 12:35Z) Consumed sessionless and session-scoped `computerUseSetup/changed` notices with target-aware diagnostics.
- [x] (2026-10-19 12:50Z) Extended the fake host and added focused integration tests, including schema conformance for both emitted host methods.
- [x] (2026-10-19 12:55Z) Extended the live 1.4.7+ loopback coverage; it passed against installed Muse 1.4.10.
- [x] (2026-10-19 13:00Z) Updated README, command metadata, ROADMAP, event compatibility, and CHANGELOG.
- [x] (2026-10-19 13:05Z) Ran formatting, lint, unit/integration, selftest, npm, script, smoke, and expanded live loopback checks successfully.
- [ ] Open a pull request that closes #240.
- [ ] Complete three review/fix passes and merge only when required checks are green.


## Surprises & Discoveries

- Observation: `computerUseSetup/changed` currently has no explicit notification arm. Every copy, including a 1.4.7 sessionless host-setup copy, reaches the generic `unhandled MSP notification` diagnostic.
  Evidence: `rg -n '"computerUseSetup/changed"' src/main.rs` finds no handler, while `tests/acp_serve.rs` currently waits for the method in stderr rather than asserting it was handled.

- Observation: `send_host_error` forwards structured error data only for the kinds named by `msp::skill_error_data`, and `commandRejected` is not one of them. The helper also uses the caller's fallback code, so the CUA host-check refusal would otherwise lose both `-32030` and its `reason: runtime_busy`.
  Evidence: `src/msp.rs` maps known typed kinds in `acp_error_code` and `skill_error_data`; `src/main.rs` uses both through `send_host_error`.

- Observation: a host-level command must use the main host explicitly. `Hosts::command` routes params carrying a `sessionId` to that session's owner, while host setup has no session and is a property of the main `muse serve` process.
  Evidence: `src/hosts.rs` documents and implements session-based routing in `Hosts::route`; `main_host()` is already used for host-wide facts such as the handshake.


## Decision Log

- Decision: use `/cua host` for the host-level read and `/cua host check` for the non-prompting grant check. Keep `/cua`, `/cua status`, and `/cua on` session-scoped.
  Rationale: this gives every client that can send a normal prompt a usable path without requiring the custom `_session/computerUse` extension, while keeping the existing command meanings stable and making the target explicit. The issue suggests a host target for status/check and distinguishes it from per-session `/cua on`.
  Date/Author: 2026-10-19, Codex.

- Decision: do not expose host-level `computerUseSetup/start` as `/cua host on`.
  Rationale: starting host setup is an interactive human setup flow, while the requested host target is specifically a non-prompting status/check surface. The existing `/cua on` starts the current session's setup and remains the correct action for a coding session.
  Date/Author: 2026-10-19, Codex.

- Decision: request both computer-use capabilities whenever the ACP client opts in, but track and gate their grants independently.
  Rationale: a host may grant the per-session setup capability without granting host-wide setup. The session surface must keep working when only `computerUseSetup` is granted, and the host surface must remain dark when only `computerUseHostSetup` is absent.
  Date/Author: 2026-10-19, Codex.

- Decision: add `commandRejected` to the adapter's mapped typed error data and map it to ACP `-32030`.
  Rationale: issue #240 explicitly requires the busy host to surface the typed refusal. This mapping preserves the host's `kind`, `reason`, and optional `commandId`/`retryable` members rather than reducing the failure to a generic internal error.
  Date/Author: 2026-10-19, Codex.

- Decision: keep the `_session/computerUse` request contract session-scoped.
  Rationale: the standard slash-command path satisfies basic consumer access without custom client integration. Adding a second, target-parameterized JSON shape would expand an existing extension API without being needed by issue #240's acceptance criteria.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation is in progress. The completed outcome will be recorded here after the pull request is reviewed, fixed, and merged.


## Context and Orientation

The adapter entry point is `src/main.rs`. It reads ACP JSON-RPC frames from the editor and MSP frames from one or more `muse serve` children. A normal editor prompt arrives as `session/prompt`. Adapter-local slash commands are intercepted before a model turn starts, send host methods directly, and settle the held ACP prompt with ordinary agent text. `src/acp.rs` advertises those commands through `available_commands_update`.

`src/msp.rs` launches `muse serve`, performs the MSP `initialize` handshake, records grantable capabilities in `HandshakeInfo`, and sends commands with JSON-RPC. `src/hosts.rs` owns the main, read-only, and reviewer host processes and routes session commands to the session owner. Host-wide setup belongs to the main process.

The vendored MSP schema is `tests/protocol/stable/msp.schema.json`. In this corpus, `computerUseSetup/read` accepts an optional `sessionId`; omitting it reads the host's own setup and needs `computerUseHostSetup`. `computerUseSetup/start` has the same optionality, but this plan only exposes host read/check. `computerUseSetup/check` takes no params, reads the three macOS grants without prompting, and may answer `commandRejected` with reason `runtime_busy`. `computerUseSetup/changed` carries `state`, with `sessionId` absent for the host's own setup.

The fake protocol host is `tests/fixtures/fake_serve.py`. Tests in `tests/acp_serve.rs` exercise the real adapter binary against that fixture and can inspect logs of host methods and every emitted request frame. The live-host suite is `tests/live_loopback.rs`; features newer than older pinned hosts use `require_release`.

The consumer documents are `README.md`, which describes editor-visible computer-use behavior and protocol extensions; `ROADMAP.md` section 9, which records one disposition for every stable MSP addition; and `docs/event-compatibility.md`, which records one disposition for every schema notification. `CHANGELOG.md` records user-visible unreleased behavior.


## Plan of Work

First, extend `src/msp.rs`. Add `computer_use_host_setup: bool` to `HandshakeInfo`, initialize it in every test constructor through the existing struct-update patterns, and populate it from the handshake only when the ACP opt-in caused the capability to be requested. When the opt-in is enabled, add both `"computerUseSetup"` and `"computerUseHostSetup"` to `requestedCapabilities`. Record the two grants independently.

Second, extend the typed MSP error bridge in `src/msp.rs`. Rename `skill_error_data` to `typed_error_data` because it now covers command rejections as well as skill, plugin, capability, and computer-use gate failures. Include `commandRejected` in the forwarded kinds and map it to `-32030` in `acp_error_code`. Update `send_host_error` in `src/main.rs` and add unit coverage for a runtime-busy-shaped error.

Third, extend the `/cua` parser and dispatcher in `src/main.rs`. Keep `CuaAction` explicit with session and host variants. Parse a bare `/cua` or `/cua status` as session status; parse `/cua host` and `/cua host status` as host read; parse `/cua host check` as the host grant check. Reject all other host suffixes with the usage text. The existing `/cua on`, dropdown patch, and app block commands remain unchanged.

In `run_cua`, require the ACP opt-in for every target. For host actions, resolve `host.main_host()`, require its `computer_use_host_setup` grant, and send `computerUseSetup/read` with `{}` or `computerUseSetup/check` with `{}`. For session actions, preserve the current `computerUseSetup` gate and session-id lookup. Render host output with an explicit `Host computer use setup:` or `Host computer use check:` prefix so users can distinguish it from session state. Keep every command settled through the existing helper and never start a model turn.

Add an explicit `computerUseSetup/changed` compatibility arm in `handle_msp` in `src/main.rs`. If `sessionId` is present, log the session target; otherwise log the host target. Include a bounded state name when present. Do not cache state: both command paths re-read the host, so the notice remains a diagnostic.

Fourth, update `tests/fixtures/fake_serve.py`. Track the host grant separately from the setup grant. Grant `computerUseHostSetup` when requested unless `FAKE_NO_COMPUTER_USE_HOST=1`; keep `FAKE_NO_COMPUTER_USE=1` for the per-session grant. Answer host-level read/check frames and log their params. Send a sessionless `computerUseSetup/changed` after a successful check. When `FAKE_CUA_HOST_BUSY=1`, answer check with `-32030`, `data.kind=commandRejected`, `data.reason=runtime_busy`, and `retryable=false`.

Fifth, extend `tests/acp_serve.rs`. Add one focused integration test with four legs: no client opt-in; opt-in with the host grant withheld; both grants with host status and check output; and a busy host. Assert that both capabilities are requested with the opt-in, the host methods are never sent when their gate is closed, host read uses `{}` rather than a session id, check output names all three grants, the sessionless change notice is consumed rather than unhandled, the busy refusal keeps `-32030` plus typed data, and no command starts a turn. Update the existing session command test only if shared stderr assertions need to distinguish the now-handled notice.

Sixth, extend `tests/live_loopback.rs`. In the existing computer-use test or a sibling test, use release 1.4.7 as the minimum and invoke `/cua host` and `/cua host check` through an ordinary prompt. Accept the expected platform outcomes: a successful host state, the adapter's `-32601` withheld-grant refusal, the CUA gate's typed `-32033`, or the host's typed `-32030` runtime-busy refusal. Reject generic failures and assert that any host check response is a settled prompt rather than a model turn.

Seventh, update consumer documentation. In `README.md`, expand the computer-use bullet and protocol-extension section with the opt-in, the two grants, `/cua host`, `/cua host check`, and the no-custom-extension slash-command path. Update the command input hint and description in `src/acp.rs`. Update ROADMAP section 9 rows for `computerUseSetup/check`, host-level `computerUseSetup/read`, the sessionless change notice, and both capabilities. Update the `computerUseSetup/changed` row in `docs/event-compatibility.md`. Add an Unreleased CHANGELOG entry naming issue #240.


## Concrete Steps

Run all commands from the repository root:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

While iterating, run the focused fake-host test:

    cargo test --locked --test acp_serve cua_host_status_and_check_require_the_opt_in_and_grant

Run the focused typed-error unit test:

    cargo test --locked command_rejected_keeps_the_host_refusal_shape

Run the live computer-use test when a Muse binary is available:

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback computer_use

Before opening the pull request, run every contribution gate:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Push the branch and open a draft or ready pull request only after the local gates pass. Use a body that links `Fixes #240`, summarizes the user-visible commands and capability behavior, lists validation evidence, and records the platform limitation for the true macOS grant-success path.


## Validation and Acceptance

Acceptance is behavioral. Start the adapter against the fake host with an ACP client that opts into `_meta.muse.computerUse`. The MSP initialize frame must request both computer-use capabilities. Typing `/cua host` must send `computerUseSetup/read` with `{}` and render a host-prefixed state with Screen Recording, Accessibility, and Input Monitoring. Typing `/cua host check` must send `computerUseSetup/check` with `{}`, render its state, and consume the sessionless `computerUseSetup/changed` notice without an unhandled-notification diagnostic. Neither command may send `turn/start`.

With `FAKE_NO_COMPUTER_USE_HOST=1`, `/cua host` must answer a local `-32601` naming the missing `computerUseHostSetup` grant and must never send a host-level computer-use method. Without the ACP opt-in, both host and session computer-use commands must fail closed before any host method. An independently granted host capability may continue to serve `/cua host` even when the per-session capability is withheld.

With `FAKE_CUA_HOST_BUSY=1`, `/cua host check` must settle the ACP prompt with code `-32030`, `error.data.kind` equal to `commandRejected`, `error.data.reason` equal to `runtime_busy`, and `retryable=false`. It must not degrade to `-32603` or omit the typed data.

The new test must fail before the implementation because host read/check are parsed as usage errors and no host grant is requested. After implementation it must pass, and the full `cargo test --locked` suite must remain green. The live test must pass against the installed Muse build or correctly skip on builds before 1.4.7; it must not turn a real host refusal into a generic failure.


## Idempotence and Recovery

All changes are additive on a feature branch. Re-running tests is safe. The fake host uses per-test temporary directories, so its logs and notifications do not leak between runs. If a schema-frame assertion fails, inspect the recorded frame in the fake log and correct the adapter's emitted params rather than weakening the test.

If the pull request has failing checks, keep the branch mergeable with `origin/master`, fix the failure on the same branch, rerun the focused test and the complete gates, and push again. Do not force-push over review feedback. If a live-host failure is environment-specific, capture the adapter's typed response and exact test name; do not mark a generic host failure as acceptable. Rollback consists of returning to `origin/master` and deleting the feature branch before merge.


## Artifacts and Notes

The key contract facts from the vendored bundle are:

    computerUseSetup/check: no params; result ComputerUseSetupState;
    commandRejected runtime_busy while a setup attempt holds the host.

    computerUseSetup/read: optional sessionId; omitted = host setup;
    needs computerUseHostSetup.

    computerUseSetup/changed: required state; optional sessionId;
    absent sessionId = host setup, delivered only to host-grant owners.

Validation transcripts and the final review findings will be added here as the work progresses.

Local validation on 2026-10-19:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
        unit: 156 passed
        acp_serve: 354 passed
        fuzz corpus: 8 passed
        live_host: 1 passed
        live_loopback: 42 passed under MUSE_ACP_LOOPBACK=1
        msp_130_matrix: 4 passed
        msp_140_field_matrix: 5 passed
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs: 4 passed
    python3 -m unittest discover -s scripts -p 'test_*.py': 36 passed, 6 skipped
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Interfaces and Dependencies

In `src/msp.rs`, `HandshakeInfo` gains:

    pub computer_use_host_setup: bool

The `initialize` request includes both names under the existing client opt-in:

    "computerUseSetup", "computerUseHostSetup"

The typed error bridge becomes:

    pub fn typed_error_data(error: &J) -> Option<String>

and maps `commandRejected` to ACP `-32030`.

In `src/main.rs`, extend:

    enum CuaAction {
        SessionStatus,
        SessionStart,
        HostStatus,
        HostCheck,
        Set { key: &'static str, value: &'static str },
        Block { bundle: String, block: bool },
    }

The parser contract is:

    /cua [status]                 session status
    /cua host [status]            host setup read
    /cua host check               host grant check
    /cua on                       session setup start
    /cua set ...                  host-global settings patch
    /cua block|unblock <bundle>   host-global settings patch

No new Rust crates are required.


Revision note (2026-10-19): Initial ExecPlan for issue #240, created after surveying the current adapter and before implementation.
