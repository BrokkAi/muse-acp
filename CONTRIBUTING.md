# Contributing

Thank you for helping improve `muse-acp`.

## Development setup

Install Rust 1.88 or newer and Python 3. The integration suite uses the
checked-in fake MSP host, so it does not require a live Muse session.

Before submitting a pull request, run:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --locked -- --selftest
```

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
