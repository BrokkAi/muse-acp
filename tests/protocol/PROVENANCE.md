# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Stable bundle (`stable/`):** the stable 1.4.18-R8587.1 build's own
  `muse schema generate-json-schema --out DIR` export (`manifest.json` and
  `msp.schema.json`, copied byte for byte), taken 2026-11-13. The Muse SDK
  has published no revision past 1.4.15, and the binary's help states its
  bundles are precomputed at build, so the export is exact for that build.
  The fingerprint recorded in `src/compat.rs`
  (`sha256:77dda18550adef14e257b015fe4e6490b722723f7d526df5ba4867939fff68a3`)
  is the one a live 1.4.18 handshake reports and the official
  1.4.18-R8587.1 release manifest publishes as `msp_schema_fingerprint`, and
  `SDK_SCHEMA_SHA256` there pins the bundle bytes it belongs to, so the
  fingerprint classifies as tested. Over 1.4.15 the surface is purely
  additive and changes only definitions: a new item kind, fields, and enum
  values, with the method, notification, request, error, and capability
  tables unchanged and no removed or newly required member. The previous
  stable bundle was SDK `07e1548`'s (Muse 1.4.15).
- **Transcripts (`transcripts/`):** <https://github.com/meta-models/muse-code-sdk>
  at `07e1548c55c268d2bc91966e565a01425d4a9f90`
  ("remirror: 1.4.15 from the internal source tree (tracked internally)",
  2026-11-10), the newest SDK revision when the stable bundle moved to
  1.4.18. Only `schema/msp/transcripts/` was copied, without
  `schema/msp/msp.d.ts` or `schema/msp/experimental/`; the revision's
  `python/`, `clients/`, `rust/`, and `scripts/` trees are not vendored.
  From this revision the SDK no longer writes the fingerprint into the
  transcript manifests; it derives it from the schema bytes at build.
- **License:** MIT — see `LICENSE.muse-code-sdk`
- **Contents:**
  - `stable/manifest.json` — schema version (and, before `07e1548`, the
    stable-surface fingerprint)
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

Update `stable/` only by re-copying a single source, either an SDK
revision's `schema/msp/stable/` or a pinned Muse build's own export, and
`transcripts/` only from a single SDK revision, and record each source here.
After every update, `compat::SDK_SCHEMA_SHA256` in `src/compat.rs` must match the SHA-256 of
`stable/msp.schema.json`, and `compat::SDK_MANIFEST_FINGERPRINT` must be the
fingerprint a live host of that release reports (and `stable/manifest.json`'s
`fingerprint`, when the manifest carries one).
