# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Stable bundle (`stable/`):** <https://github.com/meta-models/muse-code-sdk>
  at `5fc95e5bbbedc7944b1729297ca79bcef9627416`'s `schema/msp/stable/`
  (`manifest.json` and `msp.schema.json`, copied byte for byte), taken
  2026-11-18. The bundle is byte-identical to the stable 1.4.23-R8977.1
  build's own `muse schema generate-json-schema --out DIR` export. The
  fingerprint recorded in `src/compat.rs`
  (`sha256:b7fb9137267aa35c92424df37a5ac56287006a0e546b197bf0930d593fecbdb1`)
  is the one a live 1.4.23 handshake reports and the official
  1.4.23-R8977.1 release manifest publishes as `msp_schema_fingerprint`, and
  `SDK_SCHEMA_SHA256` there pins the bundle bytes it belongs to, so the
  fingerprint classifies as tested. The bundle records its compatibility
  epoch (`0`) at its root, which `SDK_COMPATIBILITY_EPOCH` pins. Over 1.4.20
  the changes are `ApprovalCuaCardFacts.browser` with its
  `ApprovalCuaBrowser` definition, `Item.terminalRowBeforeItemId`, and
  `SchemaInfo.compatibilityEpoch`; the `stable/manifest.json` bytes are
  unchanged. The previous stable bundle was SDK `649ca21`'s (Muse 1.4.20),
  and before it the 1.4.18-R8587.1 build's own export.
- **Transcripts (`transcripts/`):** <https://github.com/meta-models/muse-code-sdk>
  at `5fc95e5bbbedc7944b1729297ca79bcef9627416`
  ("remirror: 1.4.23 from the internal source tree (tracked internally)",
  2026-11-17), the same revision as the stable bundle. Only
  `schema/msp/transcripts/` was copied, without `schema/msp/msp.d.ts` or
  `schema/msp/experimental/`; the revision's `python/`, `clients/`, `rust/`,
  and `scripts/` trees are not vendored. Over `649ca21` four scenarios were
  added (`approval-choice-invalid`, `approval-requirement-stale`,
  `turn-interrupt-queued-successor`, and
  `turn-interrupt-queued-successor-burst`), for 58 in all, and the
  re-recorded handshakes carry `schema.compatibilityEpoch`. Since `07e1548`
  the SDK no longer writes the fingerprint into the transcript manifests; it
  derives it from the schema bytes at build.
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
`stable/msp.schema.json`, `compat::SDK_MANIFEST_FINGERPRINT` must be the
fingerprint a live host of that release reports (and `stable/manifest.json`'s
`fingerprint`, when the manifest carries one), and
`compat::SDK_COMPATIBILITY_EPOCH` must be the bundle's root
`compatibilityEpoch`.
