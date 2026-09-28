# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `a7c10c5dd3f66be412077d29f9d11111af70317b`
  ("Merge pull request #25 from meta-models/python-tree-audience-gate",
  2026-09-21). Only `schema/msp/` was copied; the revision's `python/`,
  `clients/`, and `scripts/` trees are not vendored.
- **License:** MIT — see `LICENSE.muse-code-sdk`
- **Contents:**
  - `stable/manifest.json` — schema version + stable-surface fingerprint
  - `stable/msp.schema.json` — the stable v1 JSON schema bundle
  - `transcripts/` — the recorded golden-transcript corpus

The transcript README is upstream documentation. Its `schema/msp/` paths,
`tbh-conformance` package, and regeneration commands refer to the Muse SDK's
source repository, not this adapter checkout. Here the corpus lives under
`tests/protocol/` and is checked by `cargo test --locked` (including the
`compat` tests in `src/compat.rs`). The local adapter integration tests use
`tests/fixtures/fake_serve.py`. Host additions beyond this pinned SDK revision
are documented in [the event matrix](../../docs/event-compatibility.md) and
[the roadmap](../../ROADMAP.md).

Update this directory only by re-copying from a single SDK revision and
recording the new commit hash here. `compat::SDK_MANIFEST_FINGERPRINT` in
`src/compat.rs` must match `stable/manifest.json` after every update.
