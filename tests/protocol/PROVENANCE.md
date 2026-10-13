# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `a0c8611b382584eb5091942b477f30e92e1c8d18`
  ("remirror: 1.4.7 from the internal source tree (tracked internally)",
  2026-10-13).
  This revision publishes the Muse 1.4.7 stable surface. Like the `ea88c822`
  (1.4.6) pin before it, no pinned 1.4.7 build has joined the live-host
  matrix yet, so the adapter classifies this fingerprint as degraded rather
  than tested until one does. Only `schema/msp/` was copied, without
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
