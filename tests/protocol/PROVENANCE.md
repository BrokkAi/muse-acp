# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `bb44be3d36de46d2411bd9eaa4aee99006092546`
  ("Merge pull request #67 from meta-models/sdk-1.4.2-remirror", 2026-09-30;
  its change is "Re-mirror the SDK closure at tbh@fda770f (1.4.2 lockstep)").
  This revision publishes the Muse 1.4.2 stable surface: its manifest
  fingerprint equals what a live Muse 1.4.2 host reports. Only `schema/msp/`
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
