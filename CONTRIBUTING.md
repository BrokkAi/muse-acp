# Contributing

Thank you for helping improve `muse-acp`.

## Development setup

Install Rust 1.88 or newer, Python 3, and Node.js 22 or newer. The integration suite uses the
checked-in fake MSP host, so it does not require a live Muse session.

The live-host suite runs the adapter against a real `muse serve` whose model
calls go to a scripted loopback provider (`tests/fixtures/loopback_provider.py`).
It needs Muse installed but no Muse account, and it never reads your Muse
settings:

```sh
MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback
```

The same switch runs the MSP client crate's own live test, which drives
`muse serve` through `crates/muse-msp-client` without the adapter:

```sh
MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-msp-client --test live_muse
```

Set `MUSE_CLI` to test a specific Muse build. The nightly workflow
(`.github/workflows/nightly.yml`) runs this suite against each pinned build it
lists; run it on a branch with `gh workflow run nightly --ref <branch>` when a
change needs live-host coverage before merge. Each failed nightly job opens an issue
labeled `nightly-failure`, or comments on the open one for that job.

Before submitting a pull request, run:

```sh
cargo fmt --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo run --locked -- --selftest
node --test npm/test/launcher.test.cjs
python3 -m unittest discover -s scripts -p 'test_*.py'
sh -n install.sh
node scripts/smoke_npm.cjs target/debug/muse-acp
```

The script tests check that the npm launcher, `install.ps1`, `scripts/release.py`,
and the release workflow list the same targets. When `pwsh` or `powershell` is
installed, they also run the installer's architecture selection against a stubbed
registry.

CI also parses `install.ps1` with PowerShell, tests the minimum Rust version,
and runs the npm launcher tests on Linux. The nightly workflow runs clippy, the
Rust tests, and the selftest on macOS and Windows x64 and arm64, and the npm
launcher tests on macOS and both Windows architectures.

Keep protocol changes compatible with the ACP versions advertised by the
adapter. Add regression coverage for behavior changes, especially permission,
filesystem, cancellation, and concurrency paths.

## Pull requests

- Keep changes focused and explain externally visible behavior.
- Update README and protocol notes when configuration or compatibility changes.
- Never commit credentials, private logs, customer data, or local environment
  files. Redact diagnostics before attaching them.
- Report security issues according to [SECURITY.md](SECURITY.md), not in a
  public issue.

Unless explicitly stated otherwise, contributions intentionally submitted for
inclusion are licensed under the Apache License, Version 2.0, as described in
section 5 of [LICENSE](LICENSE).

## Maintainer releases

See [RELEASING.md](RELEASING.md) for preparation, exact-commit checks, publication
and recovery. Preflight branch pushes and manual dispatches do not publish.

Coverage-guided JSON fuzzing uses a separate development package and optional
manual CI workflow; see [fuzz/README.md](fuzz/README.md). `cargo test --locked --workspace`
also runs its checked-in seed corpus without requiring nightly or libFuzzer.
