# Advertise ACP terminal authentication backed by `muse-acp login`

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.

## Purpose / Big Picture

`muse-acp` is a bridge. Editors speak the Agent Client Protocol (ACP, JSON-RPC over stdio) to it, and it speaks the Muse Session Protocol (MSP) to a child process, `muse serve`, from the Muse Code CLI. Muse keeps its own login state. The user creates it by running `muse login`, which shows a device code for them to approve in a browser.

Today the adapter's ACP `initialize` response advertises `"authMethods": []`, so an editor has no way to help a user who is not logged in. The ACP Registry (the catalog that editors such as Zed and JetBrains use to offer one-click agent installs) only lists agents that advertise at least one auth method of type `agent` or `terminal`. Its CI launches the agent in a fresh sandbox (an empty HOME, and almost certainly no Muse installed), sends one `initialize` request with `protocolVersion: 1`, and fails the entry if the response is an error, if `authMethods` is empty, or if no method has type `agent` or `terminal`. That CI also advertises the client capability `_meta: {"terminal-auth": true}`, which is the older Zed convention for terminal login.

After this change:

1. `muse-acp login` exists. It runs the configured Muse executable's `login` subcommand in the foreground, attached to the user's terminal, and exits with its status. It also works through npm (`npx @brokkai/muse-acp login`).
2. `initialize` advertises one terminal auth method, `muse-login`. In ACP v1 it includes both the typed form (`"type": "terminal", "args": ["login"]`) and the legacy `_meta."terminal-auth"` form with an absolute `command`. In ACP v2 it uses the typed form keyed by `methodId`.
3. `initialize` succeeds even when `muse serve` cannot be started (Muse not installed, or the host exits during startup). The adapter stays alive in a "host unavailable" state. Every later request gets an actionable error, and `shutdown`/`exit` still work. Before this change, the adapter answered `initialize` with `-32603` and exited, which would fail the registry check.
4. `authenticate` with `methodId: "muse-login"` returns success, because the login itself happens out of band in the terminal. Any other method id gets `-32602`. The legacy `auth/login`, `auth/logout`, and `logout` names keep today's guidance error.

To see it working, run `cargo run -- login` (it launches `muse login`), then pipe an `initialize` request into the binary with `MUSE_CLI=/nonexistent` and observe a successful response that contains `"authMethods":[{"id":"muse-login",...,"type":"terminal","args":["login"],...}]`.

An unauthenticated prompt already returns ACP error `-32000` ("auth required") with `muse login` guidance. That is the signal ACP clients use to offer the advertised auth methods. It is unchanged here.

## Progress

- [x] (2026-09-28 00:00Z) Research: registry requirements, ACP terminal-auth RFD, and the adapter's current behavior when Muse is missing or unauthenticated.
- [x] (2026-09-28) Milestone 1: `muse-acp login` subcommand and usage text. Manually verified: exit status propagates (3 -> 3), a missing binary exits 1 with guidance, and `login extra` exits 2.
- [x] (2026-09-28) Milestone 2: advertise `muse-login` in v1/v2 `initialize`, and make `authenticate` accept it.
- [x] (2026-09-28) Milestone 3: degraded "host unavailable" mode so `initialize` succeeds without Muse. A registry-validator-shaped handshake with `MUSE_CLI=/nonexistent-muse` returns the auth methods, `session/new` gets the install guidance, and `shutdown` exits 0.
- [x] (2026-09-28) Milestone 4: tests, README/ROADMAP/CHANGELOG. `cargo fmt --check`, clippy, `cargo test --locked` (227 integration tests), selftest, `sh -n install.sh`, and the npm smoke test pass.
- [ ] Not verified: a live, interactive `muse login` through an editor's terminal-auth UI (needs a human and a browser); whether an already-running `muse serve` picks up new credentials without a restart.

## Surprises & Discoveries

- Observation: an unauthenticated `muse serve` (Muse 1.4.0, empty HOME) starts normally, and `session/new` succeeds. The failure only appears at `session/prompt`, which the adapter already maps to `-32000`.
  Evidence (prompt reply): `{"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"Muse is not authenticated. Run \`muse login\` ..."}}`
- Observation: with `MUSE_CLI` pointing at a missing binary, `initialize` gets `-32603` and the process exits 1 (test `missing_cli_failure_names_the_next_action` in `tests/acp_serve.rs`). The registry sandbox would hit this path.
- Observation: the registry validator infers a method's type from `type`, or else from `_meta` keys `terminal-auth`/`agent-auth`, and otherwise defaults to `agent`. It requires `id` and `name`.
- Observation: three existing tests (`unsupported_envelope_schema_version_fails_closed`, `command_timeout_reports_method_id_and_configured_duration`, `authentication_initialize_failure_has_external_login_guidance`) asserted that the adapter exits on a failed launch. They now assert the degraded contract through the helper `assert_host_unavailable`. The schema case still fails closed, because no request reaches a host. The MSP-initialize auth failure now yields `-32000` on `session/new`, which is exactly the point where terminal-auth clients offer login.
- Observation (pre-existing, fixed here): `describe_spawn_error` in `src/msp.rs` had runs of spaces inside its message ("Install Muse Code              (https://..."), because a string continuation lacked a trailing backslash. The unit test `spawn_errors_name_the_next_user_action` now rejects double spaces.

## Decision Log

- Decision: use ACP terminal auth backed by `muse login`, not in-protocol `agent` auth through MSP's experimental `account/loginStart`.
  Rationale: the adapter never handles credentials, and it doesn't opt into MSP's experimental API. `muse login` is already a browser device-code flow, so terminal auth gives the full UX with no new trust surface.
  Date/Author: 2026-09-28 / Claude with Ryan Svihla.
- Decision: name the subcommand `login` (no dashes), matching the existing `install`/`uninstall` subcommands, and advertise `args: ["login"]`.
  Rationale: consistent CLI. The registry replaces the agent's default args with the advertised args, and the adapter's default is no args.
  Date/Author: 2026-09-28 / Claude.
- Decision: in v1, include `_meta."terminal-auth"` with `command` set to `std::env::current_exe()`, `args: ["login"]`, and `label: "Muse login"`. Omit `_meta` if the current executable path can't be resolved.
  Rationale: clients that predate typed terminal auth (and the registry validator's capability flags) use this convention, and it needs an explicit executable. The npm launcher runs the native binary, so `current_exe` is that binary and runs directly.
  Date/Author: 2026-09-28 / Claude.
- Decision: `authenticate` for `muse-login` returns `{}`.
  Rationale: terminal-auth clients run the login outside ACP, then re-initialize or retry. Muse checks credentials on the next turn, and a still-unauthenticated prompt returns `-32000` again, so success here can't hide a failure.
  Date/Author: 2026-09-28 / Claude.
- Decision: when `muse serve` fails to launch, answer `initialize` normally and hold a "host unavailable" state, instead of exiting. Later requests with an id get the launch diagnostic as an error. The code is `-32000` if the text is an auth failure, otherwise `-32603`. There is no automatic relaunch.
  Rationale: the registry handshake must pass on machines without Muse, and editors show a request error better than a dead agent. Relaunching is left to the editor restarting the agent, which terminal-auth clients already do.
  Date/Author: 2026-09-28 / Claude.

## Outcomes & Retrospective

The adapter now meets the ACP Registry auth requirement. `initialize` (v1 and v2) advertises one `terminal` method, `muse-login`, and the handshake succeeds even without Muse installed. The login itself is `muse-acp login`, which delegates to `muse login`, so the adapter still never touches credentials. What remains is a manual editor check (Zed or JetBrains) of the terminal-auth UI with a real Muse account, plus submitting the registry entry (an `agent.json` using the npm distribution `@brokkai/muse-acp@<version>`). The lesson: the registry's CI exercises the no-Muse path, so handshake robustness mattered as much as the auth method itself.

## Context and Orientation

All code is Rust in `src/`. There are no third-party crates. JSON is handled by the in-repo parser in `src/json.rs` (`parse_json`, `esc` for string escaping, and the `J` value enum).

`src/main.rs` holds `fn main()`. It handles the `--selftest` and `--support` flags first, then calls `zed::dispatch(&args)` for the installer subcommands and help/version. Otherwise it runs the ACP loop: a stdin thread forwards lines, and the loop lazily launches the MSP host on the first request, which must be `initialize`. The launch is `MspHost::launch` in `src/msp.rs`, which spawns `$MUSE_CLI serve` (default `muse`). On launch failure it currently sends `-32603` and calls `shutdown::exit(1)`. After launch, every message goes to `handle_acp`. Its `"initialize"` arm sends `v1_init()` or `v2_init()`, static JSON literals that `selftest()` validates. Its `"authenticate" | "auth/login" | "auth/logout" | "logout"` arm returns `-32601` with guidance.

`src/zed.rs` holds CLI parsing (`parse_args`, `usage()`) for the installer subcommands.

`src/msp.rs` holds `describe_spawn_error` (the missing-binary guidance), `auth_failure`, `auth_diagnostic`, and `acp_error_code`. The last one maps auth-looking errors to `-32000`.

Tests: `tests/acp_serve.rs` drives the real adapter binary against the Python fake host `tests/fixtures/fake_serve.py`, through a `Client` helper (`Client::spawn(scenario, env)`, `req`, `wait_for`, `finish`). The test `authentication_remains_external_until_experimental_account_surface_is_adopted` asserts the old empty `authMethods`, so it must be rewritten.

## Plan of Work

Milestone 1. In `src/main.rs`, add `fn login() -> i32`. It resolves the binary as `MUSE_CLI` or `muse`, prints a one-line note to stderr naming the binary, runs `Command::new(bin).arg("login")` with inherited stdio, and returns the child's exit code. It returns 1 if the child was killed by a signal. If the spawn fails, it prints `msp::describe_spawn_error` (made `pub` if needed) and returns 1. Call it from `main()` when `args == ["login"]`, before `zed::dispatch`. Add a `login` line to `usage()` in `src/zed.rs`. So `muse-acp login --foo` gets rejected, add an explicit `"login"` arm in `parse_args` that returns an error for extra arguments.

Milestone 2. Replace the `"authMethods":[]` substring in `v1_init()`/`v2_init()` with a placeholder filled by `auth_methods_v1()`/`auth_methods_v2()`. v1:

    [{"id":"muse-login","name":"Log in with Muse","description":"Run `muse login` in a terminal and approve the code in your browser","type":"terminal","args":["login"],"_meta":{"terminal-auth":{"label":"Muse login","command":"<abs exe>","args":["login"]}}}]

v2:

    [{"methodId":"muse-login","name":"Log in with Muse","description":"...","type":"terminal","args":["login"]}]

Change the `authenticate` arm to read `params.methodId`: `"muse-login"` returns `{}`, and anything else returns `-32602` naming the supported id. `auth/login`, `auth/logout`, and `logout` keep the existing guidance text.

Milestone 3. In `main()`, keep `host_unavailable: Option<String>`. When `MspHost::launch` fails: log the error and any support lines, set the state, answer the `initialize` request with the negotiated literal (factor out `send_initialize(stdout, &id)`), and continue. While unavailable: a repeated `initialize` is answered normally. `shutdown` replies `null` and exits 0, and `exit` exits 0. Client responses (no method) and notifications are ignored. Every other request gets an error with the stored message and code `-32000` if `msp::auth_failure(msg)` matches, else `-32603`.

Milestone 4. Tests in `tests/acp_serve.rs`:
- Rewrite the authentication test: both versions advertise `muse-login` with type `terminal` and args `["login"]`. v1 includes `_meta` `terminal-auth` with a `command`. `authenticate` `muse-login` succeeds, and an unknown id gets `-32602`.
- Change `missing_cli_failure_names_the_next_action`: `initialize` succeeds and advertises auth methods, `session/new` gets an error containing the missing-binary guidance, and the process stays alive until `shutdown`.
- New `login_subcommand_runs_muse_login`: point `MUSE_CLI` at a tiny script that writes its args to a file and exits 3. Assert `muse-acp login` exits 3 and the script received `login`. Also cover a missing binary (exit 1 with guidance). Skip the script test on Windows.

Update the README "Authentication and remote environments" section, ROADMAP §6 status, and CHANGELOG Unreleased.

## Concrete Steps

From the worktree root:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Registry-style handshake without Muse:

    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"_meta":{"terminal-auth":true}}}}' | MUSE_CLI=/nonexistent timeout 5 target/debug/muse-acp

Expected: one `result` frame whose `authMethods` contains `"type":"terminal"`, and no `error`.

## Validation and Acceptance

Acceptance: (a) the registry-style handshake above passes both with and without Muse installed; (b) `muse-acp login` launches `muse login` and propagates its exit code; (c) an unauthenticated prompt still returns `-32000`; (d) the full contribution gate passes.

## Idempotence and Recovery

All changes are additive code and docs. The commands can be re-run freely. No credentials are read, written, or logged.

## Artifacts and Notes

(Filled as work proceeds.)

## Interfaces and Dependencies

In `src/main.rs`: `fn login() -> i32`, `fn auth_methods_v1() -> String`, `fn auth_methods_v2() -> String`, `fn send_initialize(stdout: &StdoutShared, id: &Option<J>)`. In `src/msp.rs`: `pub fn describe_spawn_error` (visibility only).
