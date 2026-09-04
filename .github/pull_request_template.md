## Summary

<!-- What changes, and why? -->

## Verification

<!-- List the checks you ran. -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --locked --all-targets -- -D warnings`
- [ ] `cargo test --locked`
- [ ] `cargo run --locked -- --selftest`

## Security and compatibility

<!-- Note permission, filesystem, process, ACP/MSP, or compatibility impact. -->

- [ ] I removed credentials, private logs, customer data, and unrelated
      personal information from this change.
