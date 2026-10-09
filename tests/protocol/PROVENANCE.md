# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `537cc8dc72cf1347c16063fc13dcb93f9cde7f6e`
  ("remirror: 1.4.4 from the internal source tree (tracked internally)",
  2026-10-08).
  This revision publishes the Muse 1.4.4 stable surface: its manifest
  fingerprint equals what a live Muse 1.4.4 host reports, and the binary's
  own `muse schema` export is identical to the vendored bundle. Only `schema/msp/`
  was copied, without `schema/msp/msp.d.ts`; the revision's `python/`,
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
`tests/fixtures/fake_serve.py`. Host behavior the schema does not state is
documented in [the event matrix](../../docs/event-compatibility.md) and
[the roadmap](../../ROADMAP.md).

Update this directory only by re-copying from a single SDK revision and
recording the new commit hash here. `compat::SDK_MANIFEST_FINGERPRINT` in
`src/compat.rs` must match `stable/manifest.json` after every update.
