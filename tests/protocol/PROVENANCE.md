# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `838d36aa71f0c7fc610adef5e23ab6141fc0b318`
  ("remirror: 1.4.8 from the internal source tree (tracked internally)",
  2026-10-14).
  This revision publishes the Muse 1.4.8 stable surface. The vendored
  `stable/` bundle is byte-identical to what an installed Muse 1.4.8 exports
  with `muse schema generate-json-schema`, and the pinned 1.4.8 build in the
  live-host matrix reports this fingerprint byte-for-byte, so the adapter
  classifies this surface as tested. Only `schema/msp/` was copied, without
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
