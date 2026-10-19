# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `ae469342dfe6ccd5142947af5f7dcdd5d50ccc18`
  ("remirror: 1.4.10 from the internal source tree (tracked internally)",
  2026-10-16).
  This revision publishes the Muse 1.4.9 stable surface, which Muse 1.4.10
  carries unchanged (the 1.4.10 remirror touches no stable-schema file). The
  vendored `stable/` bundle is byte-identical to what an installed Muse
  1.4.10 exports with `muse schema generate-json-schema`, which reports this
  fingerprint byte-for-byte. No pinned 1.4.9 or 1.4.10 build is in the
  live-host matrix yet, so the adapter classifies this surface as degraded.
  Only `schema/msp/` was copied, without `schema/msp/msp.d.ts`; the
  revision's `python/`, `clients/`, and `scripts/` trees are not vendored.
  The previous pin was `838d36a` (Muse 1.4.8, 2026-10-14).
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
