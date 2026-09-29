# Report a missing Muse install and offer to install it from the login flow

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` up to date as work proceeds. It follows `.agents/PLANS.md` and builds on `.agents/plans/terminal-auth-login.md`, which added the `muse-login` terminal auth method and the "host unavailable" mode.

## Purpose / Big Picture

`muse-acp` is an adapter between the Agent Client Protocol (ACP, the JSON-RPC protocol editors such as Zed and JetBrains IDEs use to talk to coding agents) and Muse Code's `muse serve` host. Before this change, someone who installed the adapter from an editor's agent catalog (the ACP Registry) without first installing Muse got a generic `-32603` "Muse host unavailable: Muse CLI not found" error when opening a thread, and nothing in the editor could fix it. After this change the editor gets ACP's auth-required error (`-32000`) with the message "Muse Code is not installed", so it shows its login screen. There the adapter's single auth method is named "Install Muse Code and log in". Choosing it runs `muse-acp login` in a terminal. That command prints the official Muse installer command, asks before running it, installs Muse, then continues with `muse login`. The adapter also finds Muse in the installer's default directory when the editor's `PATH` lacks it, which is the usual case right after an install.

To see it working, run the adapter with no Muse reachable (an empty `PATH`, `HOME` and `LOCALAPPDATA` pointing at an empty directory, `MUSE_CLI` unset). `initialize` lists the method named "Install Muse Code and log in", `session/new` returns `-32000` starting with "Muse Code is not installed", and `muse-acp login` with `n` on stdin prints the installer command and exits 1 without installing.

## Progress

- [x] (2026-09-29) Research: Muse's installers (`https://dev.meta.ai/install.sh`, a bash script, and `https://dev.meta.ai/install.ps1`) are non-interactive. They install to `MUSE_INSTALL_DIR`, else `~/.local/bin/muse`, or `%LOCALAPPDATA%\Programs\muse\muse.cmd` on Windows, and they edit the user's shell profile or user `PATH`.
- [x] (2026-09-29) `src/msp.rs`: `muse_cli()` falls back to the installer directory; new `LaunchError::NotInstalled` for a spawn `NotFound`.
- [x] (2026-09-29) `src/main.rs`: `MUSE_NOT_INSTALLED` flag, install-flavored auth method label, `-32000` "not installed" error in host-unavailable mode, and the install offer in `login()`.
- [x] (2026-09-29) Tests, README, CHANGELOG; contribution gate run on Windows.
- [ ] Manual check of the real install path in a clean VM or container (the installer edits the user's PATH, so it was not run on the development machine).

## Surprises & Discoveries

- Observation: the Muse docs publish `curl -fsSL https://dev.meta.ai/install.sh | sh`, but the script starts with `#!/usr/bin/env bash` and uses `[[ ]]`, so piping it to a POSIX `sh` such as dash would fail. The adapter pipes it to `bash`.
- Observation: on the Windows development machine, `auto_review_override_is_recreated_on_restart_and_used_by_support` and `auto_review_settings_are_cleaned_up_on_forced_shutdown` in `tests/acp_serve.rs` fail on unmodified `master` (a7a36f7) too. They are unrelated to this change.

## Decision Log

- Decision: keep one auth method, `muse-login` with `args: ["login"]`, and only change its name and description when the host launch found no Muse executable.
  Rationale: the ACP Registry check and existing clients already rely on this id. A separate "install" method would appear even when it cannot help, and a client could not tell which one to pick.
  Date/Author: 2026-09-29 / Claude with Ryan Svihla.
- Decision: return `-32000` (auth required), not `-32603`, from every request in host-unavailable mode when Muse is not installed.
  Rationale: Zed shows its auth-method buttons only for the auth-required error, and that button is now the install path.
  Date/Author: 2026-09-29 / Claude.
- Decision: install only after a typed answer in the terminal: Enter, `y`, or `yes`. A closed stdin (EOF) or any other answer declines. Never install when `MUSE_CLI` is set.
  Rationale: the installer downloads and runs remote code and edits shell profiles. Choosing the button shows intent, but the terminal prompt shows the exact command first. Tests and non-interactive runs get EOF and decline. A pinned `MUSE_CLI` means the user manages Muse.
  Date/Author: 2026-09-29 / Claude.
- Decision: run the vendor's installer unchanged and do not download or verify Muse binaries in the adapter.
  Rationale: the installer already checks its launcher's checksum and owns updates. Copying its logic would drift.
  Date/Author: 2026-09-29 / Claude.
- Decision: when `muse` is not on `PATH`, fall back to the installer's directory. On macOS and Linux, a `muse` found on `PATH` still resolves to the bare name.
  Rationale: editors often start agents with a `PATH` that lacks `~/.local/bin`, and `login()` must find the Muse it just installed without a new shell. The bare name keeps Homebrew and cargo symlinks working across upgrades.
  Date/Author: 2026-09-29 / Claude.
- Decision: no automatic relaunch of `muse serve` from host-unavailable mode after an install.
  Rationale: this matches the earlier plan, where terminal-auth clients restart the agent after login. The login message tells users to restart the agent if Muse still shows as missing.
  Date/Author: 2026-09-29 / Claude.

## Outcomes & Retrospective

A registry-style handshake on a machine without Muse now succeeds and points users to an install. The login command installs Muse on request and then logs in. Remaining work: exercise the real installer end to end in a clean environment, and consider relaunching the host after install if editors do not restart the agent.

## Context and Orientation

`src/msp.rs` owns launching `muse serve`: `muse_cli()` picks the executable, `describe_spawn_error()` turns a spawn failure into guidance, and `MspHost::launch()` returns `LaunchError`. `src/main.rs` owns ACP: `main()` launches the host on the first `initialize`, and on failure it stores a reason and routes every later message through `serve_without_host()`. `auth_methods_v1()` and `auth_methods_v2()` build the `authMethods` array for ACP v1 and v2 `initialize` results, and `login()` implements `muse-acp login`. Integration tests in `tests/acp_serve.rs` run the built adapter against a fake MSP host selected with `MUSE_CLI`.

## Plan of Work

In `src/msp.rs`, add `default_install_dir(install_dir, base)`, which returns `MUSE_INSTALL_DIR` if it is set and non-empty, else `<base>/.local/bin`, or `<base>\Programs\muse` on Windows. `base` is `HOME`, or `LOCALAPPDATA` on Windows. Make `muse_cli()` search that directory after `PATH`, and add `LaunchError::NotInstalled` for a spawn `NotFound`. In `src/main.rs`, add `static MUSE_NOT_INSTALLED: AtomicBool` and set it when launch returns `NotInstalled`, before `serve_without_host` answers `initialize`. `auth_method_label()` picks the method name and description from that flag. `serve_without_host` answers `-32000` with the not-installed message while it is set. In `login()`, if spawning `muse login` fails with `NotFound` and `MUSE_CLI` is unset, call `install_muse()`, resolve `muse_cli()` again, and retry once.

## Concrete Steps

From the repository root:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest

## Validation and Acceptance

`missing_cli_still_completes_the_handshake_and_names_the_next_action` expects the install-flavored method name and a `-32000` error containing "Muse Code is not installed". `login_offers_to_install_missing_muse_and_declines_without_an_answer` runs `muse-acp login` with no Muse reachable and a closed stdin, and expects exit 1, the installer URL, the prompt, and the decline message. `default_install_dir_follows_the_muse_installer` covers the path rules. Manually, with Muse installed only in its default directory and an empty `PATH`, `muse-acp --selftest` prints `cli-ready binary=<that directory>/muse...`.

## Idempotence and Recovery

The code changes are additive. Running the Muse installer twice is safe; it replaces an identical launcher and leaves the rest alone. Declining the prompt changes nothing.

## Artifacts and Notes

A no-Muse `session/new` error on Windows:

    {"code":-32000,"message":"Muse Code is not installed. Choose **Install Muse Code and log in** to install it with the official installer, or install it yourself (https://dev.meta.ai/docs/muse-code) and restart the agent. (Muse CLI not found: 'muse'. ...)"}

## Interfaces and Dependencies

No new crates. `msp::LaunchError` gains `NotInstalled(String)`. The installer is reached through `bash` and `curl` on macOS/Linux, and through `powershell` on Windows.
