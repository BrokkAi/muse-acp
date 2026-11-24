# Complete the ACP plugin lifecycle

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture

After this change, an editor user can inspect installed Muse plugins and remove one without leaving ACP. The slash-command surface gains `/plugin list` and `/plugin remove`; the latter can optionally remove the plugin's data with an explicit `--delete-data` flag. Muse 1.4.24 added the underlying `plugin/remove` command and `plugin/removed` notification, and the adapter already supports plugin inventory, installation, and marketplace management. Completing removal makes that lifecycle usable.

The change also makes host-gate refusals understandable. An experimental method rejected because its host gate is off carries `ErrorData.gate`; the adapter will preserve that typed data and say which gate is disabled instead of returning an unexplained `methodNotFound`.

The working tree already contains the independent Muse 1.4.25 SDK re-pin. This plan builds on that work and must not revert or rewrite it.


## Progress

- [x] (2026-11-24) Defined `/plugin list`, `/plugin remove <id>`, and explicit `--delete-data` syntax.
- [x] (2026-11-24) Implemented `/plugin list` with deterministic rendering and host error forwarding.
- [x] (2026-11-24) Implemented `/plugin remove`, with data deletion only behind `--delete-data`.
- [x] (2026-11-24) Consumed `plugin/removed` explicitly and refreshed plugin-dependent skill and hook state.
- [x] (2026-11-24) Preserved `ErrorData.gate` in typed ACP errors and made plugin refusal text actionable.
- [x] (2026-11-24) Added fake-host coverage, parser tests, and a live 1.4.25 inventory/refusal test.
- [x] (2026-11-24) Updated README, event compatibility, ROADMAP, CHANGELOG, and the SDK adoption plan.
- [x] (2026-11-24) Passed formatting, clippy, the full non-live workspace suite, scripts, selftest, npm smoke, and focused live validation.


## Surprises & Discoveries

- Observation: `plugin/remove` is a client method in the experimental methods index, while `plugin/removed` is its notification. The params require `commandId` and `id`; `sessionId` targets the session host, and `deleteData` defaults to false.
  Evidence: SDK `e6ef4a6` experimental schema definitions `PluginRemoveParams`, `PluginRemoveResult`, and `PluginRemovedParams`.

- Observation: the adapter already has a generic protocol-command path, but it sends a `commandId` and settles without rendering the host result. `plugin/list` takes only `sessionId`, and both list and remove need meaningful result text, so they need a dedicated command path.
  Evidence: `parse_plugin_command` and the generic protocol dispatch in `src/main.rs`; `PluginListResult` exposes rows under `plugins`.

- Observation: the checksummed 1.4.25 Linux host accepts the 1.4.25 schema and serves `/plugin list`, but its current runtime reports `plugin/remove is not available on this host`. The adapter therefore validates the requested verb live and preserves the host's refusal; successful removal semantics are covered by the fake host until a host with the plugin-mutation gate enabled is available.
  Evidence: focused live test `plugin_inventory_and_typed_refusals_reach_a_real_host` passed with a `-32603` "not available on this host" refusal.


## Decision Log

- Decision: add `/plugin list`, `/plugin remove <id>`, and `/plugin remove <id> --delete-data`.
  Rationale: the existing `/plugin` family is the natural discovery point, and removal needs the plugin id returned by the inventory. Data deletion remains explicit because it is irreversible from the editor.
  Date/Author: 2026-11-24, Codex.

- Decision: keep existing install and marketplace commands on the generic command path; add a dedicated lifecycle path only for list and remove.
  Rationale: list takes a different request shape and both commands need result rendering. Reusing the generic path for them would send an invalid extra `commandId` to list or silently discard useful remove warnings.
  Date/Author: 2026-11-24, Codex.

- Decision: handle `plugin/removed` explicitly and refresh skill and hook state instead of leaving it on the generic unhandled-notification path.
  Rationale: plugin packages can provide skills and hooks. The schema tells clients to re-read `plugin/list`, `hook/list`, and `skill/list`; the adapter already knows how to refresh and publish its skill command catalog, and can refresh folded hook labels when the hooks capability is negotiated.
  Date/Author: 2026-11-24, Codex.

- Decision: forward errors with `data.gate` as typed ACP data and explain the gate in plugin command text.
  Rationale: an old host also returns `methodNotFound`; only `gate` distinguishes an intentionally disabled experimental feature. Preserving the structured field lets clients branch, while the human sentence prevents an unexplained failure.
  Date/Author: 2026-11-24, Codex.


## Outcomes & Retrospective

The slash-command plugin lifecycle is complete. Users can inspect the host's installed inventory without implementing `_session/plugins`, remove a plugin without deleting its data, or explicitly request data deletion. Removal notifications refresh plugin-dependent skill and hook state. Experimental gate refusals now retain their structured `gate` fact and produce a message that names the disabled gate.

The implementation is validated by parser tests, ACP integration tests for listing, successful removal, warnings, destructive opt-in, skill-catalog refresh, hook refresh, and both gate and `pluginNotFound` refusals. Formatting, clippy, the full non-live workspace suite, scripts, selftest, and npm smoke pass. The real 1.4.25 host validates inventory and its own removal refusal; successful live removal remains host-gated rather than a code-path gap.


## Context and Orientation

The adapter is a Rust process that speaks ACP to an editor and MSP to a `muse serve` child. Slash commands are recognized in `protocol_command_text` in `src/main.rs`, parsed before normal turn submission, and sent through `MspHost::command`. The existing `parse_plugin_command` handles plugin installation and marketplace writes. The opt-in `_session/plugins` ACP extension reads `plugin/list` verbatim for clients that render a native inventory.

The stable vendored schema in `tests/protocol/stable/msp.schema.json` does not contain experimental methods. Muse's experimental schema at SDK `e6ef4a6` defines `plugin/remove` and `plugin/removed`. The adapter already negotiates `experimentalApi`, so it can call the method while still tolerating older hosts that return `methodNotFound`.

The fake MSP host is `tests/fixtures/fake_serve.py`. The `plugin_control` scenario already serves installed rows and install/marketplace receipts. ACP integration tests live in `tests/acp_serve.rs`; the parser and prompt handler are in `src/main.rs`; folded hook inventory is in `src/fold.rs`; host-error shaping is in `src/msp.rs`.


## Plan of Work

Add a small lifecycle command enum and parser in `src/main.rs` for `list` and `remove`. Intercept those commands before the generic protocol-command dispatch so `plugin/list` receives only `sessionId`, while `plugin/remove` receives `commandId`, `sessionId`, `id`, and optional `deleteData: true`. Render a stable inventory sorted by plugin id, including enabled state, version, source, and origin when present. Render removal status and any host warning; never say data was deleted unless `--delete-data` was requested and the host accepted removal.

In `src/msp.rs`, classify an error carrying `data.gate` as typed error data even when its `kind` is not in the existing allowlist. Add a small accessor for the gate name. In the plugin error path, print that the verb is unavailable because the named gate is disabled while forwarding the full typed data.

Handle `plugin/removed` in the MSP notification dispatcher in `src/main.rs`. Log the removed plugin and session, refresh the session's skill catalog and ACP available commands, and refresh `hook/list` inventory when the hooks capability is opted in. The plugin view itself has no cache and needs no invalidation.

Extend `tests/fixtures/fake_serve.py` with remove success, warning, delete-data, typed removal refusal, and gate-disabled method-not-found behavior. Add parser unit tests and ACP integration tests for list rendering, removal params, destructive-flag handling, notification-driven skill refresh, hook refresh, and gate text/data. Add a live 1.4.25 test if a temporary local plugin can be installed and removed without touching user state.

Document the slash commands in `README.md`, the experimental ACP behavior in `docs/acp-extensions.md` if it affects the extension contract, the notification disposition in `docs/event-compatibility.md`, the method row in `ROADMAP.md`, and the user-visible change under `## Unreleased` in `CHANGELOG.md`.


## Concrete Steps

All commands run from the repository root, `/workspace/81b5233a67c34c66a580808155ad38ab/muse-acp`.

Implement and iterate with focused tests:

    cargo test --locked -p muse-acp --bin muse-acp plugin
    cargo test --locked -p muse-acp --test acp_serve plugin
    cargo test --locked -p muse-acp --test msp_140_field_matrix

Run the contribution gates from `CONTRIBUTING.md`:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

For live validation, use the checksummed Muse 1.4.25 Linux artifact already downloaded under `$HOME` and set `MUSE_CLI`:

    MUSE_ACP_LOOPBACK=1 MUSE_CLI="$HOME/.cache/muse-muse-acp/1.4.25-R9361.1/muse" \
      cargo test --locked -p muse-acp --test live_loopback plugin -- --nocapture


## Validation and Acceptance

`/plugin list` returns a readable inventory and does not mutate host state. With the fake inventory it lists `threejs` as enabled and `local-tools` as disabled, including version and source.

`/plugin remove threejs` sends `plugin/remove` with the session id and plugin id, omits `deleteData`, and reports successful removal plus any host warning. `/plugin remove threejs --delete-data` additionally sends `"deleteData":true`. Invalid arities never reach the host.

A host `plugin/removed` notification refreshes the ACP skill command catalog and, when negotiated, hook labels. A gate-disabled refusal reaches the editor with the host's `data.gate` intact and a message naming the disabled gate.

Formatting, clippy, the full non-live workspace suite, scripts, selftest, and npm smoke tests pass. Any local live-test limitation caused by the container's bubblewrap restriction is recorded rather than misreported as success.


## Idempotence and Recovery

Parser and rendering changes are deterministic and safe to rerun. Removal commands use a fresh UUIDv7 `commandId`, so retrying the same slash command after an uncertain failure creates a new operation only when the user sends it again; the adapter does not retry destructive removal automatically. The fake-host state is process-local. No migration or rollback is required; reverting the source changes removes the new slash commands.


## Artifacts and Notes

Useful SDK shapes from `e6ef4a6`:

    PluginRemoveParams:
      commandId: string (required)
      id: string (required)
      sessionId?: string
      deleteData?: boolean

    PluginRemoveResult:
      commandId, id, status (required)
      warning?: string

    PluginRemovedParams:
      id (required)
      sessionId?: string

    ErrorData.gate?: string, on methodNotFound refusals for gated verbs


## Interfaces and Dependencies

No new Rust dependency. Add an adapter-local enum in `src/main.rs` for lifecycle actions, a parser returning `Option<Result<PluginAction, String>>`, and a runner accepting the existing `Hosts`, `StdoutShared`, `Sessions`, ACP session id, protocol version, prompt request id, and echoed prompt content. Extend `src/msp.rs` with a gate accessor and broaden typed-error forwarding to errors carrying `data.gate`; keep the existing error code mapping and full host data. Extend the fake host's `plugin_control` scenario without changing stable schema conformance expectations.
