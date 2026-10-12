# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `ea88c8221d381de7cb28281be8bc6b8b1507ba72`
  ("remirror: 1.4.6 from the internal source tree (tracked internally)",
  2026-10-11).
  This revision publishes the Muse 1.4.6 stable surface. Unlike the `bb44be3`
  (1.4.2) and `537cc8d` (1.4.4) pins, its manifest fingerprint has not been
  compared against a live `muse serve` binary from this project yet, so the
  adapter classifies it as degraded rather than tested until a pinned 1.4.6
  build joins the live-host matrix. Only `schema/msp/` was copied, without
  `schema/msp/msp.d.ts`; the revision's `python/`, `clients/`, and `scripts/`
  trees are not vendored.
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
`tests/fixtures/fake_serve.py`. Host behavior the schema does not state is
documented in [the event matrix](../../docs/event-compatibility.md) and
[the roadmap](../../ROADMAP.md).

Update this directory only by re-copying from a single SDK revision and
recording the new commit hash here. `compat::SDK_MANIFEST_FINGERPRINT` in
`src/compat.rs` must match `stable/manifest.json` after every update.
