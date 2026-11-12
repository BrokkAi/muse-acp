# Vendored Muse MSP conformance corpus

- **Source:** <https://github.com/meta-models/muse-code-sdk>
- **Upstream revision:** `07e1548c55c268d2bc91966e565a01425d4a9f90`
  ("remirror: 1.4.15 from the internal source tree (tracked internally)",
  2026-11-10).
  This revision publishes the Muse 1.4.15 stable surface, and the vendored
  `stable/` bundle is its own export. The stable 1.4.15-R8339.1 build's
  `muse schema generate-json-schema` export (`manifest.json` and
  `msp.schema.json`) is byte-identical to the vendored bundle. From this
  revision the SDK no longer writes the fingerprint into `manifest.json` or
  the transcript manifests; it derives it from the schema bytes at build.
  The fingerprint recorded in `src/compat.rs`
  (`sha256:ef311d2dbf051ed9114556e14ad3980eb515104baa3409306a6817082a61d8cd`)
  is the one a live 1.4.15 handshake reports and the official 1.4.15-R8339.1
  release manifest publishes as `msp_schema_fingerprint`, and
  `SDK_SCHEMA_SHA256` there pins the bundle bytes it belongs to, so the
  fingerprint classifies as tested. The surface is purely additive over
  1.4.14: new methods, notifications, errors, item kinds, and fields, a new
  reserved capability, and no removed or newly required member.
  Only `schema/msp/stable/` and `schema/msp/transcripts/` were copied, without
  `schema/msp/msp.d.ts` or `schema/msp/experimental/`; the revision's
  `python/`, `clients/`, `rust/`, and `scripts/` trees are not vendored.
  The previous pin was `a45cb67` (Muse 1.4.14, 2026-10-29).
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

Update this directory only by re-copying from a single SDK revision and
recording the new commit hash here. After every update,
`compat::SDK_SCHEMA_SHA256` in `src/compat.rs` must match the SHA-256 of
`stable/msp.schema.json`, and `compat::SDK_MANIFEST_FINGERPRINT` must be the
fingerprint a live host of that release reports (and `stable/manifest.json`'s
`fingerprint`, when the manifest carries one).
