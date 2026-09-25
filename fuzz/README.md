# JSON fuzzing

The `json` target compiles the production parser and serializer directly. Its
libFuzzer and serde_json dependencies belong to this separate test package;
the shipped adapter still has no runtime dependencies.

Install [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz/setup.html) and
a nightly toolchain (without changing your default):

```sh
cargo install cargo-fuzz --locked --version 0.13.2
rustup toolchain install nightly --profile minimal
mkdir -p fuzz/corpus/json
cargo +nightly fuzz run json fuzz/corpus/json fuzz/seeds/json -- -max_total_time=120 -timeout=5 -max_len=65536
```

Checked-in seeds live in `fuzz/seeds/json`; generated coverage inputs go to
`fuzz/corpus/json` and failures to `fuzz/artifacts/json`. Commit minimized
regressions to the seed directory. Replay a failure with
`cargo +nightly fuzz run json fuzz/artifacts/json/<artifact>`; minimize with
`cargo +nightly fuzz tmin json fuzz/artifacts/json/<artifact>`.

The target checks parse/serialize stability, string escaping, a strict independent
parser oracle, truncated frames, and parsing a healthy frame after each input.
Panics, sanitizer failures, and libFuzzer's per-input timeout fail the run. Seeds
cover malformed numbers, NaN/Infinity, lone surrogates, escapes, and depth limits.
`cargo test --locked` replays the seeds deterministically and retains the stdio
recovery tests for malformed notifications and requests. Fuzzing supplements
these tests; a finite run cannot prove the absence of all crashes or hangs.

The optional **json-fuzz** GitHub Actions workflow runs a bounded campaign on
manual dispatch. Normal CI requires neither nightly nor libFuzzer.
