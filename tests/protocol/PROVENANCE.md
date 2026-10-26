# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `3dce908f556bd8b3dfc9cd7f28f442b0504d74a4`
  ("remirror: 1.4.13 from the internal source tree (tracked internally)",
  2026-10-24).
  This revision publishes the Muse 1.4.13 stable surface, and the vendored
  `stable/` bundle is its own export. No installed Muse 1.4.13 was available
  to diff against (the host in this workspace is Muse 1.4.12-R6909.1), so
  `src/compat.rs` classifies the fingerprint as degraded until a pinned
  1.4.13 build joins the live-host matrix. The surface is purely additive
  over 1.4.12: the method, notification, request, error, and capability
  tables are unchanged, and no required field was added.
  Only `schema/msp/` was copied, without `schema/msp/msp.d.ts`; the
  revision's `python/`, `clients/`, and `scripts/` trees are not vendored.
  The previous pin was `09d67ec` (Muse 1.4.12, 2026-10-22).
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
