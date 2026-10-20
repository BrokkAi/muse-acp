# Extract a reusable Muse MSP client crate

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This plan follows the repository standard in `.agents/PLANS.md` and must be maintained in accordance with it. It implements GitHub issue #328, "Extract msp.rs into a reusable Muse MSP client crate".

## Purpose / Big Picture

`muse-acp` is a Rust program that sits between an editor and Muse Code. Toward Muse it speaks the Muse Session Protocol (MSP): newline-delimited JSON-RPC 2.0 over the stdin and stdout of one `muse serve` child process. The code that launches that child, negotiates capabilities, matches responses to requests, enforces per-method deadlines, forwards notifications, answers server-initiated requests, classifies how the child exited, and shuts it down lives in `src/msp.rs` as a type named `MspHost`. Today that code can only be used inside the adapter binary, because it reads adapter environment variables, writes adapter log lines, registers with the adapter's shutdown watchdog, and prepares a temporary Muse settings view.

After this change a new workspace crate, `crates/muse-msp-client` (library name `muse_msp_client`), contains the reusable client. Any Rust program can depend on it, launch a configured `muse serve`, complete the MSP handshake, create a session, send commands, read notifications, answer server requests, cancel a pending request, and close the host, without importing or running any Agent Client Protocol (ACP) code. The adapter keeps every editor-specific behavior and drives the crate through an explicit launch configuration, so its observable behavior (log lines, diagnostics, timeouts, Muse discovery, settings overlay) does not change.

To see it working: run `cargo test --locked -p muse-msp-client` for the hermetic suite (no Muse needed), and `MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-msp-client --test live_muse -- --nocapture` on a machine with Muse installed to watch the crate alone drive a real `muse serve` whose model calls go to a local scripted provider.

## Progress

- [x] (2026-10-20 16:40Z) Read `AGENTS.md`, `CONTRIBUTING.md`, `.agents/PLANS.md`, `src/msp.rs`, `src/hosts.rs`, `src/shutdown.rs`, `src/host_config.rs`, `src/json.rs`, `src/compat.rs`, the `crates/muse-acp-client` crate and its plan, CI workflows, and the live loopback harness.
- [x] (2026-10-20 16:45Z) Ran the live loopback suite on unmodified `origin/master` (exported to `/tmp/muse-acp-baseline` so later edits cannot leak into it): 46 passed, 3 failed (`a_fork_starts_from_the_source_history_and_continues_on_its_own`, `a_workflow_child_runs_and_its_result_reaches_the_parent`, `side_chat_reaches_a_real_host_and_attaches_with_provenance`).
- [x] (2026-10-20 17:10Z) Wrote this ExecPlan.
- [x] (2026-10-20 17:20Z) Milestone 1: created the crate, moved the JSON core into it, re-exported it from the adapter, repointed the fuzz paths. Workspace tests unchanged (adapter unit tests 171, crate JSON tests 6).
- [x] (2026-10-20 18:05Z) Milestone 2: implemented the client (transport, correlation, process ownership, handshake, events, cancellation, close) with 33 unit tests over an in-memory transport and 12 process tests against a self-executing fake host; three consecutive runs passed, the process suite takes about 1.5 seconds.
- [x] (2026-10-20 18:40Z) Milestone 3: ported `src/msp.rs`, `src/hosts.rs`, `src/host_config.rs`, and the event patterns in `src/main.rs` onto the crate. All 170 adapter unit tests and 383 `tests/acp_serve.rs` tests pass. A raw-byte comparison against the baseline binary (fake host behind `tee`) showed identical host-bound frames and identical stderr for the `happy`, `approval_req`, and `host_exit` scenarios; `--selftest` and `--support` (with Muse installed and with a missing `MUSE_CLI`) print identical output.
- [x] (2026-10-20 19:10Z) Milestone 4: added `crates/muse-msp-client/tests/live_muse.rs` (passes against Muse 1.4.11 in under a second, three runs), `examples/session.rs` (ran it against the same isolated host: it declined, then approved, a shell call), the crate README, and updates to `README.md`, `CHANGELOG.md`, `ROADMAP.md`, `CONTRIBUTING.md`, `fuzz/README.md`, and the nightly live-host job.
- [x] (2026-10-20 19:30Z) Milestone 5: every gate passes (see Artifacts), including the workspace suite on Rust 1.88.0, the npm smoke and launcher tests; the live loopback suite gives the same 46 passed and the same 3 failures, for the same reasons, as before the change.

## Surprises & Discoveries

- Observation: on this machine (Muse 1.4.11) the shell-approval live tests now pass; the three baseline failures are host-side limitations (file birth time is unavailable for fork and side chat, and the workflow child card never completes).
  Evidence: `/tmp/muse-acp-live-before.log`, "test result: FAILED. 46 passed; 3 failed".

- Observation: when a write times out, the client kills the host before failing the pending waiters (the original order), so the reader usually sees the killed host's stdout close first and the waiters report `MspError::HostClosed` rather than `Closed("serve stdin write timed out; host terminated")`. The adapter has always had this race; it maps `HostClosed` to the reaped exit message.
  Evidence: the first run of `a_stuck_write_kills_the_host` failed with "unexpected Err(HostClosed)"; the test now accepts either settlement and asserts it happens within a second.

- Observation: with the `malformed_msp` fake-host scenario, the order of the reader thread's "serve parse error" lines relative to the main thread's `host-ready` line differs from run to run, even between two runs of the unmodified baseline binary. It is a pre-existing race between two threads writing to stderr, not a change in behavior.
  Evidence: two baseline runs produced `stderr identical run0 vs run1: False` with only those lines moved.

- Observation: `rustup` did not have Rust 1.88 installed; installing it with `rustup toolchain install 1.88.0 --profile minimal` worked, and `cargo +1.88.0 test --locked --workspace` passes with a separate target directory.
  Evidence: every suite reported `test result: ok` under 1.88.0.

## Decision Log

- Decision: name the crate `muse-msp-client` (library `muse_msp_client`), version 0.1.0, `publish = false`, edition 2024, `rust-version = "1.88"`, like `crates/muse-acp-client`.
  Rationale: the issue proposes the name; matching the sibling crate's manifest keeps CI (MSRV job) and release tooling unchanged.
  Date/Author: 2026-10-20 / Claude

- Decision: move the JSON core (`J`, `parse_json`, `esc`, `j_to_string`, and the accessor methods) from `src/json.rs` into `crates/muse-msp-client/src/json.rs` as the public module `muse_msp_client::json`. The adapter's `src/json.rs` becomes a thin module that re-exports it and keeps the adapter-only helpers `b64` and `mint_id` and the Python differential test.
  Rationale: the shipped binary must stay free of third-party runtime dependencies, so the crate cannot use `serde_json`. A second parser would double the fuzzing and review surface of a security-critical component. Keeping the existing names (`J`, `parse_json`, `j_to_string`, `esc`) means every `crate::json::...` path in the adapter and the fuzz invariants keep compiling unchanged. The moved file stays std-only and free of `crate::` paths so `fuzz/fuzz_targets/json.rs` and `tests/json_fuzz_corpus.rs` can keep including it with `#[path]`. The differential test needs `python3` and `b64`, both already adapter test requirements, so it stays in the adapter and exercises the re-exported parser.
  Date/Author: 2026-10-20 / Claude

- Decision: params are passed as `Params`, a wrapper around JSON text: `Params::raw(text)` sends caller-built JSON verbatim, `From<&J>`/`From<J>` serialize a value, and `Params::empty()` is `{}`.
  Rationale: the adapter builds every MSP params object as a string. Parsing and re-serializing them would change bytes on the wire (and turn an adapter formatting bug into a local error instead of a host rejection). Library users still get a typed path through `J`.
  Date/Author: 2026-10-20 / Claude

- Decision: split launching into `MspClient::spawn(LaunchConfig)` (process and transport only), `MspClient::connect(Transport, ClientOptions)` (custom transport), and `MspClient::initialize()` (handshake); `MspClient::launch(LaunchConfig)` is spawn plus initialize with generic failure handling.
  Rationale: the adapter registers each host with its shutdown watchdog before the handshake and builds its own launch error messages (for example "serve initialize failed: ..." and the reaped exit message). A two-phase API lets it keep that order and wording exactly, while library users get the one-call `launch`.
  Date/Author: 2026-10-20 / Claude

- Decision: adapter-specific behavior is injected through `ClientOptions` hooks rather than compiled into the crate: a diagnostics sink receives typed `Diagnostic` values (malformed frames, unsupported server requests, unknown response ids, notification method tracing, shutdown timeouts, captured stderr size); a watchdog hook is armed around the client's own cleanup steps ("host shutdown", "child reap and stderr drain", "host reader disconnect"); and a handshake check decides whether a negotiated handshake is acceptable.
  Rationale: the issue requires separating the engine from adapter diagnostics and `shutdown` registration while keeping adapter output byte-for-byte identical. With no hooks installed the crate is silent, has no watchdog, and refuses only an unsupported MSP envelope schema version.
  Date/Author: 2026-10-20 / Claude

- Decision: server-initiated requests are routed by a per-method table in `ClientOptions`: `ServerRequestMode::Acknowledge` replies `{}` before forwarding (the MSP pattern for `approval/request` and `userInput/request`, whose outcome travels as a later command), `ServerRequestMode::Respond` forwards the request with its id so the caller answers with `respond`/`respond_error`, and any other method is answered with the typed MSP `methodNotFound` error. The default table is empty.
  Rationale: preserves the adapter's ack-then-forward ordering exactly, keeps unknown methods fail-closed, and lets library users answer requests themselves.
  Date/Author: 2026-10-20 / Claude

- Decision: request cancellation is local. `MspClient::start_request` returns a `PendingRequest`; its cloneable `RequestHandle::cancel` removes the waiter and wakes it with `MspError::Cancelled`. A response that arrives later is dropped and reported as an unknown-id diagnostic.
  Rationale: MSP has no JSON-RPC request-cancellation message (the schema only has work-level commands such as `turn/cancel`), and the issue forbids inventing protocol behavior. The command already written cannot be withdrawn; callers stop work with the MSP command meant for it.
  Date/Author: 2026-10-20 / Claude

- Decision: keep in the adapter: `muse_cli()` and Windows launcher discovery, every environment read (`MUSE_CLI`, `MUSE_SERVE_ARGS`, `MUSE_COMMAND_TIMEOUT_MS`, `MUSE_LOG`, `MUSE_INSTALL_DIR`), the settings overlay in `src/host_config.rs`, `HandshakeInfo` with its feature gates and log lines, the compatibility table in `src/compat.rs`, authentication diagnostics, ACP error-code mapping, editor-facing exit messages, and shutdown registration. Move to the crate: the transport, correlation, the method timeout table, UUIDv7 command id minting, exit classification (`ExitKind`, `ExitClassification`, `classify_exit_parts`, support lines), bounded stderr capture, child termination (including the Windows process-tree kill), the stdin-closed exit probe, and handshake parsing.
  Rationale: the issue's split between reusable MSP mechanics and editor-specific mapping. Editor-facing exit messages mention restarting `muse-acp` and `MUSE_SERVE_ARGS`, so they stay with the adapter as an extension trait on the crate's `ExitClassification`.
  Date/Author: 2026-10-20 / Claude

- Decision: `close()` repeats the same bounded sequence on every call (fail pending requests, close stdin, wait up to the grace period, kill, wait up to the timeout, drain stderr) and emits the same diagnostics each time; each step is a no-op once done, so calling it again is harmless. Dropping an `MspClient` that was never closed starts a silent background close.
  Rationale: the adapter calls `shutdown` more than once on the same host and logs "serve stderr captured N bytes" each time; keeping that is part of identical behavior. The background close on drop fixes the old leak (the reader thread kept the host alive) without adding log lines, because every adapter path that drops a host either shut it down first or saw it exit.
  Date/Author: 2026-10-20 / Claude

- Decision: process-level hermetic tests live in `crates/muse-msp-client/tests/process.rs` as a `harness = false` test whose `main` re-executes the test binary as a scripted fake host when `MUSE_MSP_FAKE_HOST` is set.
  Rationale: the tests must run on Linux, macOS, and Windows (the nightly workflow runs the workspace tests on all three) without Python or Muse. A self-executing Rust binary gives real pipes, real exit codes, and a real kill without a test-only binary target in the library.
  Date/Author: 2026-10-20 / Claude

- Decision: the adapter passes typed `HostFlags` (`READ_ONLY_FLAGS`, `REVIEWER_FLAGS` in `src/hosts.rs`) instead of string slices, and `host_config::configure` takes the `LaunchConfig`. `LaunchConfig::get_args` and `get_envs` mirror `std::process::Command` so tests can inspect a configuration.
  Rationale: the issue asks for host-lifetime flags in the launch configuration; the flags emit in the same order as the old constants, so the command line is unchanged.
  Date/Author: 2026-10-20 / Claude

- Decision: the adapter logs crate diagnostics with their `Display` text, and the crate pins every line in `diagnostic_lines_are_stable`.
  Rationale: the strings were ported from the adapter's log lines; keeping one copy avoids drift, and the test makes any wording change a deliberate change to the adapter's diagnostic contract. Notification diagnostics go to the adapter's `MUSE_LOG=debug` tracing instead.
  Date/Author: 2026-10-20 / Claude

- Decision: run the crate's live test in the nightly live-host job right after the adapter's suite (not on the Windows legs, which run only the workspace-root flows).
  Rationale: it proves the acceptance flow against every pinned Muse build without credentials, and like the rest of that job it is not a pull-request gate.
  Date/Author: 2026-10-20 / Claude

## Outcomes & Retrospective

The extraction is complete. `crates/muse-msp-client` is a dependency-free,
synchronous MSP client: explicit launch configuration, a two-phase or
one-call launch, typed handshake facts, per-method deadlines, cancellable
pending requests, mode-based server-request routing, events for
notifications, requests, stderr lines, and closure, exit classification, a
stdin-closed probe, and bounded close. The adapter drives all three of its
hosts through it and kept every behavior the issue lists: the host-bound
bytes and logs match the old binary, and every adapter test, the live
loopback suite, `--selftest`, and `--support` give the same results.

The acceptance flow from the issue runs in `tests/live_muse.rs` with only the
crate: launch, handshake, session, command, notifications, an answered server
request and decided approval, one cancelled request, and a clean close.

Remaining gaps are deliberate: no typed mirrors of MSP methods, no reconnect
or restart logic (the adapter keeps its own durable-host restart), and
cancellation stays local because MSP has no request-cancellation message. The
two-phase API turned out to be the key design choice: it let the adapter keep
its exact launch ordering and error wording without the crate knowing about
either.

## Context and Orientation

The repository root is a Cargo workspace. The root package `muse-acp` (manifest `Cargo.toml`) builds one binary from `src/main.rs`; its `[dependencies]` table is empty and must stay free of third-party crates. `crates/muse-acp-client` is an unrelated ACP client SDK that uses `serde`; it is a workspace member listed in the root `[workspace] members`.

Terms used in this plan:

MSP (Muse Session Protocol): JSON-RPC 2.0 messages, one JSON object per line, exchanged over the stdin and stdout of `muse serve`. A request carries an `id` and gets a response with the same `id`; a notification has no `id`. The host can also send requests to the client (server-initiated requests), such as `approval/request`. The vendored protocol schema is `tests/protocol/stable/msp.schema.json`, and recorded example conversations are under `tests/protocol/transcripts/`.

Handshake: the client sends `initialize` with `clientInfo` and `capabilities` (`experimentalApi`, `userInputDialogs`, `requestedCapabilities`), the host answers with `serverInfo`, `schema` (`version` is the envelope schema version, currently 1; `fingerprint` is a hash of the host's stable protocol surface), `grantedCapabilities`, optional `sessionDurability` (absent means durable), optional `sandboxAvailability` and `sandboxUnavailableReason`, then the client sends the `initialized` notification. No session or turn command is accepted before that notification.

Command id: MSP commands carry a caller-minted `commandId`, a UUID version 7. Retrying a timed-out command must reuse the same id so the host recognizes the duplicate; the client never mints or rewrites ids inside params.

Admission timeout: an MSP response only acknowledges that a command was admitted; the outcome arrives later as notifications. Each method has a deadline for that acknowledgement (`src/msp.rs`, `method_timeout_ms`).

Today's code paths: `src/msp.rs` defines `MspHost` (launch, `command`, `notify`, `reap`, `shutdown`, `force_stop`, `mint_cmd`, `logged_out`, `refresh_config`), the reader and writer threads, `ExitClassification`, `HandshakeInfo`, and many error helpers used by `src/main.rs`. `src/hosts.rs` owns up to three `MspHost` processes (main, read-only, reviewer) and routes commands. `src/shutdown.rs` keeps weak references to every host so an expired shutdown deadline can kill them, and provides `deadline(context)` guards. `src/host_config.rs` prepares a temporary `XDG_CONFIG_HOME` view when the saved Muse permission profile is `:auto-review` and sets that variable on the child command. `src/compat.rs` classifies the handshake's schema fingerprint against a table of tested Muse builds. The JSON parser and serializer are in `src/json.rs`; `fuzz/fuzz_targets/json.rs` and `tests/json_fuzz_corpus.rs` include that file by path.

The adapter's tests: `cargo test --locked --workspace` runs unit tests and `tests/acp_serve.rs`, which drives the binary against the fake host `tests/fixtures/fake_serve.py`. The live suite `tests/live_loopback.rs` runs only with `MUSE_ACP_LOOPBACK=1`; it starts `tests/fixtures/loopback_provider.py`, writes a throwaway Muse config whose `endpoint_transport.base_url` points at it plus a dummy `auth.json`, clears the environment, and points `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`, and `HOME` at temporary directories, so no credentials are needed.

## Plan of Work

Milestone 1 creates `crates/muse-msp-client` with `Cargo.toml` (no dependencies), adds it to the root workspace `members` and as a path dependency of `muse-acp`, and moves the JSON core. `git mv src/json.rs crates/muse-msp-client/src/json.rs`, then remove `b64`, `mint_id`, and the `differential_tests` module from the moved file and put them in a new `src/json.rs` that starts with `pub use muse_msp_client::json::*;`. Update the `#[path]` attributes in `fuzz/fuzz_targets/json.rs` and `tests/json_fuzz_corpus.rs` to `crates/muse-msp-client/src/json.rs`. At the end the workspace builds and every test passes with no behavior change.

Milestone 2 writes the client. `src/config.rs` holds `LaunchConfig`, `HostFlags`, `ShutdownPolicy`, `ClientOptions`, `ClientInfo`, `Capabilities`, `ServerRequestMode`, and `Transport`. `src/client.rs` holds `MspClient`, `PendingRequest`, `RequestHandle`, `MspEvent`, `Params`, the reader and writer loops, and `default_timeout`. `src/process.rs` holds the child process state, bounded stderr capture, termination, and `probe_exit`. `src/exit.rs` holds the exit classification. `src/handshake.rs` holds `Handshake`, initialize-param serialization, and release-version parsing. `src/error.rs` holds `MspError`, `LaunchError`, `HandshakeError`, and `Diagnostic`. The logic is ported line by line from `src/msp.rs` so failure behavior is preserved: the pending waiter is inserted before the frame is written; a write that does not finish within the write timeout (5 seconds by default) takes the writer away, kills the child (or calls the custom transport's terminate hook), and fails every pending request with "serve stdin write timed out; host terminated"; stdout EOF takes the writer and drops every waiter, which then reports `MspError::HostClosed`; unknown server requests get `{"code":-32601,"message":"method not found: <m>","data":{"kind":"methodNotFound","retryable":false}}`; stderr is kept in a tail of at most 100 lines and 8 KiB and is never parsed. Unit tests in `src/client.rs` use an in-memory transport made of channels; `tests/process.rs` covers real child processes.

Milestone 3 rewrites `src/msp.rs` as an adapter layer. `MspHost` becomes a wrapper holding an `MspClient`, the adapter's `HandshakeInfo`, and the optional settings view. `MspHost::launch` keeps its signature; it resolves `muse_cli()`, builds a `LaunchConfig` (`serve`, then `MUSE_SERVE_ARGS`, then the host flags of the host kind), lets `host_config::configure` add `XDG_CONFIG_HOME`, installs the diagnostics sink (`log`, and `trace_method` for notifications), the shutdown-deadline watchdog, the compatibility check that logs the `schema-compat` line, and the acknowledged server requests, spawns, registers with `shutdown::register_host`, and initializes, mapping every crate error to the same `LaunchError` text as before. `MspHost::command` calls `request_with_timeout` with `command_timeout(MUSE_COMMAND_TIMEOUT_MS, method)` and maps `MspError` back to the same JSON error objects and messages. `src/hosts.rs` changes only its imports; `src/main.rs` changes only the event pattern names.

Milestone 4 adds `crates/muse-msp-client/README.md`, `crates/muse-msp-client/examples/session.rs`, and the env-gated `crates/muse-msp-client/tests/live_muse.rs`, which reuses `tests/fixtures/loopback_provider.py` to run the acceptance flow against a real `muse serve`. It updates `README.md`, `CHANGELOG.md` (Unreleased), `ROADMAP.md`, `CONTRIBUTING.md`, and the nightly live-host job.

Milestone 5 runs every gate listed in Validation and Acceptance and records results here.

## Concrete Steps

All commands run from the repository root (the worktree directory).

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo test --locked -p muse-msp-client
    cargo run --locked -- --selftest
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-msp-client --test live_muse -- --nocapture
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2

The first `cargo build` after adding the crate rewrites `Cargo.lock` with a `muse-msp-client` entry; commit it, because CI passes `--locked`.

## Validation and Acceptance

The hermetic crate suite must show: a response with id 2 resolves request 2 while request 1 stays pending, and notifications arrive meanwhile; concurrent requests from several threads each get their own response and every frame on the wire is a complete line; a request that times out reports `MspError::Timeout` and its late response is ignored with an unknown-id diagnostic; a cancelled request reports `MspError::Cancelled` and its late response is ignored; malformed and non-UTF-8 frames are skipped with a diagnostic and the next frame still works; stdout EOF fails every pending request with `MspError::HostClosed` and emits `MspEvent::Closed`; an acknowledged server request gets `{}` before the event is delivered, a respond-mode request is answered by the caller, and an unknown one gets `methodNotFound`; a blocked write terminates the transport and fails pending requests; exit codes 0 to 5 and a signal map to the documented exit kinds; `close` stops a host that exits on stdin EOF without killing it, kills one that ignores EOF after the grace period, and is safe to call twice; stderr capture stays within its bounds and never reaches the event stream unless enabled.

The acceptance test `crates/muse-msp-client/tests/live_muse.rs` (with `MUSE_ACP_LOOPBACK=1`) must launch the installed Muse with an isolated configuration, complete the handshake (server name `muse`, schema version 1), start a session, start a turn whose scripted reply asks to run a shell command, receive `approval/request`, answer it, decide the approval, observe `turn/completed`, cancel one pending request and see `MspError::Cancelled`, and close the host with a clean exit classification. The adapter's complete test suite and its live loopback suite must give the same results as before the change.

## Idempotence and Recovery

All steps are additive edits plus one file move; rerunning the commands is safe. If a milestone leaves the build broken, `git stash` is not safe in this shared checkout; commit work in progress on the branch instead and continue. Tests create temporary directories under the system temp directory and remove them on success; a failing live test keeps its directory and prints its path.

## Artifacts and Notes

Baseline live loopback result on unmodified `origin/master`, and the same
suite after the change (per-test results identical; each failure has the same
message, "file birth time is unavailable" for fork and side chat, and "the
card follows the child to completion: nullnull" for the workflow child):

    before: test result: FAILED. 46 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 17.90s
    after:  test result: FAILED. 46 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 17.98s
    failures (both):
        a_fork_starts_from_the_source_history_and_continues_on_its_own
        a_workflow_child_runs_and_its_result_reaches_the_parent
        side_chat_reaches_a_real_host_and_attaches_with_provenance

Gates at completion:

    cargo fmt --check                                              ok
    cargo clippy --locked --workspace --all-targets -- -D warnings ok
    cargo test --locked --workspace                                ok (adapter 170 + 383 + others; crate 35 unit, 12 process, 1 doc)
    cargo +1.88.0 test --locked --workspace                        ok
    cargo run --locked -- --selftest                               ok, output identical to the baseline
    python3 -m unittest discover -s scripts -p 'test_*.py'         OK (skipped=6)
    sh -n install.sh                                               ok
    node scripts/smoke_npm.cjs target/debug/muse-acp               ok
    node --test npm/test/launcher.test.cjs                         4 passed
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-msp-client --test live_muse   1 passed (0.85s)

The live acceptance test prints the host it reached:

    host 1.4.11 fingerprint sha256:f282ad16e21520d0cda68cb190149b86e2b062eef1dd282749360c05aa3d1082 durable=true
    test the_client_alone_drives_a_real_host_through_a_session ... ok

The example against the same isolated host:

    $ session <muse> <ws> 'write the artifact [[script:shell]]'
    connected to muse 1.4.11 (schema Some(1), sha256:f282ad16...)
    approval for bash: abort
    muse: done
    turn completed
    muse serve shut down cleanly (exit code 0)
    $ session <muse> <ws> 'write the artifact [[script:shell]]' --approve
    approval for bash: allow_once
    ...
    approved.txt: approved

## Interfaces and Dependencies

The crate has no dependencies and no dev-dependencies. Its public surface, in `crates/muse-msp-client/src/lib.rs`:

    pub mod json;   // J, parse_json, esc, j_to_string
    pub use client::{MspClient, MspEvent, Params, PendingRequest, RequestHandle, default_timeout};
    // MspEvent: Notification { method, params }, Request { id, method, params },
    //           Stderr { line }, Closed { reason }; #[non_exhaustive].
    pub use config::{Capabilities, ClientInfo, ClientOptions, HostFlags, LaunchConfig, ServerRequestMode, ShutdownPolicy, Transport};
    pub use error::{Diagnostic, HandshakeError, LaunchError, MspError};
    pub use exit::{ExitClassification, ExitKind, classify_exit_parts};
    pub use handshake::{ENVELOPE_SCHEMA_VERSION, Handshake, SessionDurability, parse_release};
    pub use process::probe_exit;

The central methods:

    impl MspClient {
        pub fn launch(config: LaunchConfig) -> Result<(MspClient, Receiver<MspEvent>), LaunchError>;
        pub fn spawn(config: LaunchConfig) -> Result<(MspClient, Receiver<MspEvent>), LaunchError>;
        pub fn connect(transport: Transport, options: ClientOptions) -> (MspClient, Receiver<MspEvent>);
        pub fn initialize(&self) -> Result<Handshake, HandshakeError>;
        pub fn handshake(&self) -> Option<Handshake>;
        pub fn request<'a>(&self, method: &str, params: impl Into<Params<'a>>) -> Result<J, MspError>;
        pub fn request_with_timeout<'a>(&self, method: &str, params: impl Into<Params<'a>>, timeout: Duration) -> Result<J, MspError>;
        pub fn start_request<'a>(&self, method: &str, params: impl Into<Params<'a>>, timeout: Duration) -> Result<PendingRequest, MspError>;
        pub fn notify<'a>(&self, method: &str, params: impl Into<Params<'a>>) -> Result<(), MspError>;
        pub fn respond<'a>(&self, id: &J, result: impl Into<Params<'a>>) -> Result<(), MspError>;
        pub fn respond_error(&self, id: &J, error: &J) -> Result<(), MspError>;
        pub fn mint_command_id(&self) -> String;
        pub fn stderr_tail(&self) -> String;
        pub fn reap(&self) -> Option<ExitClassification>;
        pub fn close(&self) -> Option<ExitClassification>;
        pub fn force_stop(&self) -> bool;
    }
    impl PendingRequest { pub fn id(&self) -> u64; pub fn handle(&self) -> RequestHandle; pub fn wait(self) -> Result<J, MspError>; }
    impl RequestHandle { pub fn id(&self) -> u64; pub fn cancel(&self) -> bool; }

`LaunchConfig` (by `&mut self`, like `std::process::Command`): `new(program)`, `arg`, `args`, `host_flags(HostFlags)`, `env`, `env_remove`, `env_clear`, `current_dir`, `shutdown_policy(ShutdownPolicy)`, `options(ClientOptions)`, `program`, `get_args`, `get_envs`. `ClientOptions` (by value): `client_info`, `capabilities`, `server_request(method, ServerRequestMode)`, `request_timeout`, `write_timeout`, `stderr_events`, `diagnostics(Fn(&Diagnostic))`, `watchdog(Fn(&'static str) -> guard)`, `handshake_check(Fn(&Handshake) -> Result<(), String>)`. `Transport::new(reader, writer).on_terminate(f)`.

`muse-acp` gains `muse-msp-client = { path = "crates/muse-msp-client" }` under `[dependencies]`; this adds no third-party code to the binary. In the adapter, `src/msp.rs` re-exports `ExitClassification`, `ExitKind`, `HostFlags`, and `MspEvent`, adds the `ExitMessage` trait (`editor_message`), and keeps `MspHost::launch(user_input_dialogs, user_shell, computer_use, flags: HostFlags)`, `command`, `reap`, `shutdown`, `force_stop`, `mint_cmd`, `logged_out`, `refresh_config`, and `handshake` with their old behavior.

Revision note (2026-10-20, completion): filled in Progress, Surprises, the last three decisions, Outcomes, Artifacts, and the final interface details after implementing and verifying every milestone.
