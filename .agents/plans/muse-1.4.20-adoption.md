# Adopt the Muse 1.4.20 surface

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


Muse Code 1.4.20 (`1.4.20-R8619.1`) is the current stable release, and the Muse SDK (the `meta-models/muse-code-sdk` repository, which publishes the MSP schema bundles and golden transcripts) published its 1.4.20 revision `649ca21` on 2026-11-13. MSP is Muse's session protocol, the JSON-RPC wire protocol `muse serve` speaks on stdio; this adapter, `muse-acp`, translates it into ACP, the Agent Client Protocol an editor such as Zed or a JetBrains IDE speaks.

Before this change the adapter vendored the 1.4.18 build's own schema export and classified a 1.4.20 host's schema fingerprint as `unknown`: it still ran, but logged a compatibility warning on every start. After this change the vendored stable bundle and transcripts come from SDK `649ca21`, a 1.4.20 host classifies as tested, the nightly workflow runs the real 1.4.20 builds, and the one new stable field has an explicit disposition.

To see it working, run `cargo run --locked -- --selftest` and observe a tested `sdk-manifest` row and a tested `host-1.4.20` row for fingerprint `sha256:283b1b12...`; start the adapter against an installed Muse 1.4.20 and observe `schema-compat ... status=tested detail=validated against live host 1.4.20-R8619.1; the vendored SDK 649ca21 surface` on stderr.


## Progress


- [x] (2026-11-16 18:30Z) Confirmed the channel manifest names `1.4.20-R8619.1`, the installed binary's SHA-256 matches the release manifest's Linux checksum, and the release manifest publishes `msp_schema_fingerprint` `sha256:283b1b125dcc9172928fb5d6711a92a353796d030a7c186988ed527a13cf07f0`.
- [x] (2026-11-16 18:35Z) Confirmed SDK `649ca21`'s `schema/msp/stable/msp.schema.json` and `schema/msp/experimental/msp.schema.json` are byte-identical to the 1.4.20 binary's own exports.
- [x] (2026-11-16 18:45Z) Diffed the stable and experimental exports of 1.4.18-R8587.1 (downloaded at its pinned checksum) and 1.4.20 (see Context and Orientation).
- [x] (2026-11-16 19:00Z) Baseline: unmodified master passes the workspace suite and 52 of 52 live loopback tests against 1.4.20 (the four sandbox-bound tests must run outside a container; see Surprises).
- [x] (2026-11-16 19:30Z) Re-pinned `tests/protocol/stable` and `tests/protocol/transcripts` to SDK `649ca21`; added `HOST_1420_FINGERPRINT`, the byte pin, the table arm, the selftest row, the nightly rows, PROVENANCE, and the fake-host fingerprint tests.
- [x] (2026-11-16 19:40Z) Recorded the `SessionStartParams.clear` disposition in `ROADMAP.md` and `tests/msp_140_field_matrix.rs`; updated README, ROADMAP narrative, CHANGELOG, and the 1.4.x adoption log.
- [x] (2026-11-16 19:50Z) Gates: fmt, Clippy, workspace suite, selftest, live loopback 52/52 against 1.4.20.
- [ ] Open the PR and merge when CI is green; then dispatch the nightly on master.


## Surprises & Discoveries


- Observation: The 1.4.18 to 1.4.20 change is the smallest re-pin so far.
  Evidence: the stable diff is one definition member, `SessionStartParams.clear`. The experimental diff adds only `SessionConfig.allowedBuiltInTools`, `SessionStartResult.appliedBuiltInTools`, and `SessionResumeResult.appliedBuiltInTools` besides `clear`. The SDK's own changes between `07e1548` and `649ca21` are client-library conveniences (an event stream per session, a raw notification listener, host-exit causes on errors, a cookbook recipe), not wire changes.

- Observation: The SDK's transcripts changed only by re-recording.
  Evidence: across the 54 scenarios the only new JSON keys are `schema.experimentalFingerprint` in handshake frames and a `regenerationJustification` object in three manifests; no scenario was added or removed.

- Observation: The four sandbox-bound live tests fail inside a rootless podman container and pass on the host.
  Evidence: `an_approved_shell_command_runs_and_a_rejected_one_does_not`, `a_piped_shell_command_asks_with_every_stage`, `auto_review_decides_a_shell_approval_against_a_real_host`, and `monitor_feed_reaches_a_real_host` fail in the `agent-dev` container (the approved command writes no artifact) and pass when the same test binary runs on the host against the same Muse build.

- Observation: Muse 1.4.20's native `muse acp` advertises no `/clear` either.
  Evidence: an `initialize` plus `session/new` probe with `--provider echo` lists `compact` and the skill catalog, with no `clear`.


## Decision Log


- Decision: Vendor SDK `649ca21` for both `stable/` and `transcripts/`, and classify fingerprint `sha256:283b1b12...` as tested.
  Rationale: The SDK caught up with the host, so the corpus returns to a single SDK revision; the bundle is byte-identical to the binary's export, and the live handshake, the release manifest, and the live suite agree.
  Date/Author: 2026-11-16, Claude.

- Decision: Do not send `SessionStartParams.clear` and do not add a `/clear` command; record it as intentionally ignored.
  Rationale: An ACP `session/new` is a plain start, which omission keeps. A `/clear` inside an existing ACP session would have to start a new Muse session and rebind the ACP session id to it, but this adapter's ACP session id names one Muse session: `session/load` and `session/list` would then reach the old session under that id, and the editor's transcript would still show the cleared turns. Editors already offer a new thread, and Muse's native ACP server makes the same choice.
  Date/Author: 2026-11-16, Claude.

- Decision: Do not send the experimental `allowedBuiltInTools`.
  Rationale: The schema says the host declares it but does not fill `appliedBuiltInTools` "until the last enforcing piece" lands, so sending it would promise a restriction the host does not enforce.
  Date/Author: 2026-11-16, Claude.


## Outcomes & Retrospective


The adapter classifies Muse 1.4.20 as tested (`--selftest` prints tested `sdk-manifest` and `host-1.4.20` rows for `sha256:283b1b12...`), the stable bundle and the transcripts are SDK `649ca21`'s, and the nightly matrix carries the 1.4.20 builds. 1.4.20 caused no behavior regression: master already passed the live suite against it. The new stable member has a disposition, and the experimental addition is documented in `ROADMAP.md`.

Validation: 295 adapter unit tests, 467 adapter integration tests, 10 field-matrix tests, all other workspace suites, Clippy with warnings denied, formatting, selftest, and live loopback 52 of 52 against 1.4.20-R8619.1.

What remains: ACP `systemPrompt` on top of the stable `developerPrompt` (#318) is planned in `.agents/plans/acp-sdk-5.3-adoption.md`, since ACP schema 1.16 made `systemPrompt` stable. The built-in tool restriction can be revisited once a host fills `appliedBuiltInTools`.


## Context and Orientation


`muse-acp` is a Rust binary that bridges ACP from an editor to MSP. `src/compat.rs` classifies the schema fingerprint a host reports in its `initialize` result: each known fingerprint is a `HOST_*_FINGERPRINT` constant with a row in `classify`'s table and in `selftest_lines`, `SDK_MANIFEST_FINGERPRINT` aliases the fingerprint of the vendored bundle, and `SDK_SCHEMA_SHA256` pins the vendored bundle's bytes so a re-pin that forgets the fingerprint fails.

The protocol corpus in `tests/protocol/` holds the stable schema bundle (`stable/`) and the SDK's golden transcripts (`transcripts/`); `tests/protocol/PROVENANCE.md` records the source. The field matrix is the table between `msp-1.4.x-field-matrix` markers in `ROADMAP.md`, enforced by `tests/msp_140_field_matrix.rs` (every listed addition needs exactly one row and must still be in the bundle). The fake MSP host is `tests/fixtures/fake_serve.py`, driven by `tests/acp_serve.rs`; the live suite is `tests/live_loopback.rs`, which runs a real `muse serve` against a scripted loopback model provider. `.github/workflows/nightly.yml` downloads pinned Muse builds by version, file, and SHA-256 and runs the live suite against each.

The 1.4.20 stable addition over 1.4.18 is `SessionStartParams.clear` (optional boolean: `true` starts the new session as a `/clear`, so its `SessionStart` hooks get source `clear` instead of `startup`; explicit `null` is invalid params). The experimental additions are `SessionConfig.allowedBuiltInTools` (a per-session restriction of the built-in tools on `session/start` and `session/resume`) and its echo `appliedBuiltInTools` on both results, declared but not yet enforced by the host. Nothing was removed or newly required, and the method, notification, request, error, and capability tables are unchanged.


## Plan of Work


Milestone 1 re-pins and classifies. Copy SDK `649ca21`'s `schema/msp/stable/` over `tests/protocol/stable` and its `schema/msp/transcripts/` over `tests/protocol/transcripts`; add `HOST_1420_FINGERPRINT` in `src/compat.rs`, point `SDK_MANIFEST_FINGERPRINT` at it, update `SDK_SCHEMA_SHA256`, add a table arm and a selftest row (and raise the selftest row count in `log_and_selftest_lines_are_machine_readable`), and reword the 1.4.18 entry as previously vendored; replace `sdk_manifest_fingerprint_is_the_1_4_18_surface` in `tests/acp_serve.rs` with a 1.4.20 version and keep a 1.4.18 test; add 1.4.20 rows to `.github/workflows/nightly.yml` (Linux, macOS arm64 replacing 1.4.18 as newest, Windows); update `tests/protocol/PROVENANCE.md`.

Milestone 2 records the disposition and documentation: the `SessionStartParams.clear` row in `ROADMAP.md` and in `ADDITIONS` and the still-in-bundle list of `tests/msp_140_field_matrix.rs`; README's compatibility paragraph; ROADMAP's reference snapshot and narrative; CHANGELOG; the 1.4.x adoption log.

Milestone 3 runs the gates and opens the PR.


## Concrete Steps


From the repository root, with the SDK cloned at `$SDK`:

    curl -sS https://api.meta.ai/muse-code/channels/muse-stable
    git -C $SDK show 649ca21:schema/msp/stable/msp.schema.json > tests/protocol/stable/msp.schema.json
    git -C $SDK show 649ca21:schema/msp/stable/manifest.json > tests/protocol/stable/manifest.json
    rm -rf tests/protocol/transcripts && mkdir tests/protocol/transcripts
    git -C $SDK archive 649ca21 schema/msp/transcripts | tar x --strip-components=3 -C tests/protocol/transcripts
    sha256sum tests/protocol/stable/msp.schema.json    # 70c91759...

Full gates:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2

The live suite's sandbox-bound tests need a host where Muse's `bwrap` sandbox works; inside a rootless container, build the test binary there and run it on the host with `MUSE_CLI` and `MUSE_ACP_LOOPBACK=1` set.


## Validation and Acceptance


`cargo run --locked -- --selftest` prints `kind=sdk-manifest ... fingerprint=sha256:283b1b12... status=tested` and tested `kind=host-1.4.18` and `kind=host-1.4.20` rows. A live 1.4.20 adapter log shows `schema-compat adapter=... host=muse/1.4.20 ... status=tested detail=validated against live host 1.4.20-R8619.1; the vendored SDK 649ca21 surface` and `schema-compat-experimental host=muse/1.4.20 experimental_fingerprint=sha256:c2f9255c...`. The fake-host tests `sdk_manifest_fingerprint_is_the_1_4_20_surface` and `the_1_4_18_fingerprint_stays_tested` pass, and the field-matrix test fails if the `clear` row is removed.


## Idempotence and Recovery


The corpus copy is mechanical and repeatable from the SDK revision; never hand-edit it. No adapter behavior changed, so reverting the commit restores the 1.4.18 classification exactly.


## Artifacts and Notes


Release evidence:

    channel: {"channel":"muse-stable","version":"1.4.20-R8619.1",...}
    installed: Muse Code 1.4.20 (1.4.20-R8619.1)
    release manifest msp_schema_fingerprint: sha256:283b1b125dcc9172928fb5d6711a92a353796d030a7c186988ed527a13cf07f0
    live handshake experimentalFingerprint: sha256:c2f9255c3e027d8c167a64f638429655f2121d5b28d7b5f6b5da437a99dbe57a
    stable schema sha256: 70c917598a6df13762572ad15ab15d3380b11de93432ea16bcdcb6019dcaa110
    SDK revision: 649ca21ee15979c960afc27ad5051156cdb0c87c

Nightly checksums from the release manifest:

    Linux x64:   3685b023d45a1a07c8c559da00088bdf59564920b13a85adad11e13ad6f4d181
    macOS arm64: 89964f68f7f020896fc75679dc2f3975e5e15a6efe8537d91512a1b45ffb37aa
    Windows x64: 2d223f7f992f02c313225ed47adf3dda24e7361b6b8389f2c19f7c12f026618a


## Interfaces and Dependencies


No new dependency. `src/compat.rs` gains `pub const HOST_1420_FINGERPRINT`; `SDK_MANIFEST_FINGERPRINT` aliases it. No ACP-visible change.


## Change Notes


2026-11-16: Created after diffing the 1.4.18 and 1.4.20 stable and experimental exports, and completed in the same session.
