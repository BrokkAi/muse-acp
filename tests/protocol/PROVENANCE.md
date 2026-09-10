# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `fbce769ccb75ab971d00e01a00fe076de4c773fc`
  ("Re-mirror SDK source closure at the docs cohort head", 2026-09-02)
- **License:** MIT — see `LICENSE.muse-code-sdk`
- **Contents:**
  - `stable/manifest.json` — schema version + stable-surface fingerprint
  - `stable/msp.schema.json` — the stable v1 JSON schema bundle
  - `transcripts/` — the recorded golden-transcript corpus

Update this directory only by re-copying from a single SDK revision and
recording the new commit hash here. `compat::SDK_MANIFEST_FINGERPRINT` in
`src/compat.rs` must match `stable/manifest.json` after every update.
