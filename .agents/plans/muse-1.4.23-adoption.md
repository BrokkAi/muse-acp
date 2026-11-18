# Adopt the Muse 1.4.23 surface

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


Muse Code 1.4.23 (`1.4.23-R8977.1`) is the current stable release, and the Muse SDK (the `meta-models/muse-code-sdk` repository, which publishes the MSP schema bundles and golden transcripts) published its 1.4.23 revision `5fc95e5` on 2026-11-17. MSP is Muse's session protocol, the JSON-RPC wire protocol `muse serve` speaks on stdio; this adapter, `muse-acp`, translates it into ACP, the Agent Client Protocol an editor such as Zed or a JetBrains IDE speaks. The ACP Rust SDK (`agent-client-protocol`) is still at 5.3.0, which the adapter already uses, so this change is about the Muse side only.

Before this change the adapter vendored the 1.4.20 surface and classified a 1.4.23 host's schema fingerprint as `unknown`: it ran, but logged a compatibility warning on every start. After this change the vendored stable bundle and transcripts come from SDK `5fc95e5`, a 1.4.23 host classifies as tested, and the nightly workflow runs the 1.4.23 builds. The three new stable members each do something visible:

1. The handshake's compatibility epoch tells an additive future surface from a declared breaking one. A later Muse whose fingerprint the adapter has not recorded, at the vendored epoch, logs `status=degraded` ("every stable difference is additive") instead of `unknown`.
2. A Computer Use permission prompt for a browser names the browser and words the host's first-use and window-consent notices.
3. History replay puts a background terminal's Finished row in front of the item the host names, instead of where the log recorded it.

To see it working: run `cargo run --locked -- --selftest` and observe `compatibility_epoch=0` in the header and tested `sdk-manifest` and `host-1.4.23` rows for `sha256:b7fb9137...`; start the adapter against Muse 1.4.23 and observe `schema-compat ... host=muse/1.4.23 schema_version=1 compatibility_epoch=0 fingerprint=sha256:b7fb9137... status=tested detail=validated against live host 1.4.23-R8977.1; the vendored SDK 5fc95e5 surface` on stderr.


## Progress


- [x] (2026-11-18 08:30Z) Confirmed the channel manifest names `1.4.23-R8977.1`, the installed binary's SHA-256 matches the release manifest's Linux checksum (`7ca9e166...`), and the release manifest publishes `msp_schema_fingerprint` `sha256:b7fb9137...`. Confirmed crates.io's newest `agent-client-protocol` is still 5.3.0.
- [x] (2026-11-18 08:35Z) Confirmed SDK `5fc95e5`'s stable `manifest.json` and `msp.schema.json` and its experimental `msp.schema.json` are byte-identical to the 1.4.23 binary's own exports.
- [x] (2026-11-18 08:45Z) Diffed the stable and experimental bundles of SDK `649ca21` (1.4.20) and `5fc95e5` (see Context and Orientation) and the SDK's client-library changes.
- [x] (2026-11-18 09:00Z) Baseline: unmodified master passes the workspace suite (295 unit, 476 integration) and 52 of 52 live loopback tests against 1.4.23.
- [x] (2026-11-18 09:20Z) Probed a raw 1.4.23 handshake (`compatibilityEpoch: 0`), the experimental skill provider (refused), native `muse acp` (unchanged), and a backgrounded bash call (see Surprises).
- [x] (2026-11-18 09:40Z) Re-pinned `tests/protocol/stable` and `tests/protocol/transcripts` to SDK `5fc95e5`; added `HOST_1423_FINGERPRINT`, `SDK_COMPATIBILITY_EPOCH`, the byte pin, the table arm, the selftest row, the nightly rows, PROVENANCE, and the fake-host fingerprint tests.
- [x] (2026-11-18 10:00Z) Read `compatibilityEpoch` in the MSP client crate's `Handshake` and in `compat::classify_served`; unit, fake-host, and live tests.
- [x] (2026-11-18 10:15Z) Worded `ApprovalCuaCardFacts.browser` in the permission prompt; unit and fake-host tests.
- [x] (2026-11-18 10:30Z) Honored `Item.terminalRowBeforeItemId` on history replay; unit tests, including one that fails when a moved row is allowed to end a turn.
- [x] (2026-11-18 10:45Z) Field-matrix rows, ROADMAP narrative, README, `docs/acp-extensions.md`, CHANGELOG, and the 1.4.x adoption log.
- [x] (2026-11-18 11:00Z) Gates: fmt, Clippy, workspace suite (306 unit, 481 integration), selftest.
- [x] (2026-11-18 11:30Z) Live loopback 53 of 53 against 1.4.23-R8977.1 (the new epoch test sees `compatibility_epoch=0`) and 53 of 53 against 1.4.20-R8619.1 (it sees `absent`); npm launcher, Python (41, 6 skips), shell syntax, npm smoke, and an MSRV 1.88 check all pass.
- [ ] Open the PR and merge when CI is green; then dispatch the nightly on master.


## Surprises & Discoveries


- Observation: The stable 1.4.20 to 1.4.23 change touches only definitions, all optional.
  Evidence: the structural diff adds `ApprovalCuaBrowser` and `ApprovalCuaCardFacts.browser`, `Item.terminalRowBeforeItemId` (and rewords `terminalRowAfterItemId` to cover `backgroundTerminalCompletion` and Finished-row tool calls), `SchemaInfo.compatibilityEpoch`, and a root `compatibilityEpoch: 0`; `SessionSideChatResult.history` is only reworded (a side opens with empty items). The method, notification, request, error, and capability tables are unchanged, and `stable/manifest.json` is byte-identical.

- Observation: The experimental additions are a client-supplied skill catalog, a plugin-data change feed, and smaller members.
  Evidence: `ClientCapabilities.skillProvider` and `InitializeResult.skillProvider`, the `skillProvider/list`, `skillProvider/get`, and `skillProvider/readResource` server requests and the `skillProvider/updated` method (SEP-2640's `skills/*` and `resources/read` shapes), `SkillSource.skillProvider`; `pluginData/subscribe`, `pluginData/unsubscribe`, and the `pluginData/changed` notification; `SessionResumeParams.worktree`; `ErrorData.ceilingKind` with `LimitsCeilingKind`; `PluginCatalogEntry.origin`; journal paging cursors and checkpoint markers.

- Observation: Stock 1.4.23 refuses the skill provider even with the experimental API.
  Evidence: `initialize` with `capabilities: {experimentalApi: true, skillProvider: {version: 1}}` answers `-32602` `{"kind":"experimentalRequired","descriptor":"ClientCapabilities.skillProvider"}`; the schema says the member is honored only when the host's `skill_provider` gate is open as well.

- Observation: A live 1.4.23 handshake reports `compatibilityEpoch: 0` and experimental fingerprint `sha256:6b10316b...`.
  Evidence: raw `muse serve` probe; the stable fingerprint equals the release manifest's.

- Observation: A backgrounded model `bash` call does not produce `terminalRowBeforeItemId` or a `backgroundTerminalCompletion` item on 1.4.23.
  Evidence: a raw MSP probe with the loopback provider ran `sleep 3; echo bg-finished` with `yield_time_ms: 300`, both finishing while idle and finishing during a later held turn. The late `toolCall` completion carried no anchor, and `session/read` history had none. The schema says `backgroundTerminalCompletion` is read from the inbox terminal record behind a host gate, so the before-anchor is implemented from the schema's semantics and covered by unit tests rather than a live test.

- Observation: Native `muse acp` 1.4.23 is unchanged from 1.4.20: ACP v1 only, the same capabilities and commands.
  Evidence: `initialize` and `session/new` probe with `--provider echo`.


## Decision Log


- Decision: Vendor SDK `5fc95e5` for both `stable/` and `transcripts/`, and classify fingerprint `sha256:b7fb9137...` as tested.
  Rationale: The bundle is byte-identical to the binary's export, and the live handshake, the release manifest, and the live suite agree.
  Date/Author: 2026-11-18, Claude.

- Decision: An unrecorded fingerprint at the vendored compatibility epoch is `degraded`; at another epoch it stays `unknown` with a detail naming the declared non-additive advance; neither is fatal.
  Rationale: The schema defines an equal epoch as "every difference is additive", which matches `degraded` ("known shape, not validated"). The Muse SDK's own default for a differing epoch is a warning, with refusal opt-in, and this adapter's rule is to degrade loudly and let only the envelope version block. A recorded fingerprint keeps its verdict whatever the epoch, as the SDK skips the check for equal fingerprints. A malformed epoch reads as absent, as the schema requires.
  Date/Author: 2026-11-18, Claude.

- Decision: Pin the vendored epoch as `compat::SDK_COMPATIBILITY_EPOCH` and test it against the bundle's root `compatibilityEpoch`.
  Rationale: A re-pin across a declared breaking advance must move the epoch with the fingerprint, or later hosts would be read against the wrong epoch.
  Date/Author: 2026-11-18, Claude.

- Decision: Word the browser notices in the adapter, lead the prompt with them, and quote the browser name as a code span.
  Rationale: The schema sends flags and says the client words them. The first-use notice says Computer Use will act signed in as the person, which is consent-relevant, so it comes first. The name is host display text, treated like the takeover holder's label.
  Date/Author: 2026-11-18, Claude.

- Decision: Honor `terminalRowBeforeItemId` only on history replay, by moving the row in front of its target's card; a moved row neither opens nor closes a turn; an after-anchor still wins.
  Rationale: ACP can only append, so a live row cannot be placed in front of a card already shown, and one whose target has not rendered already lands in front of it. In a replay batch the whole order is known. A moved row that opened a turn would flush rows held for a missing after-anchor in front of itself (a test covers that). When a row has both anchors and they conflict, keeping the existing after-anchor deferral matches the live path. The TUI's "coalesced region" has no ACP counterpart; each item is its own card.
  Date/Author: 2026-11-18, Claude.

- Decision: Do not adopt the experimental skill provider, plugin-data feed, resume worktree, limits ceiling kind, or journal changes.
  Rationale: ACP editors do not offer an agent a skill catalog (ACP's unstable agent skills advertise the agent's skills to the editor, the other direction), and stock 1.4.23 refuses the capability anyway. The adapter starts no worktrees, sends no `limits`, is not a journal store, and has no ACP surface for a projection change feed. Plugin origins need no work: `_session/plugins` returns `plugin/list` verbatim.
  Date/Author: 2026-11-18, Claude.


## Outcomes & Retrospective


The adapter classifies Muse 1.4.23 as tested, the stable bundle and the transcripts are SDK `5fc95e5`'s, and the nightly matrix carries the 1.4.23 builds. Every new stable member has a disposition and a behavior: the compatibility epoch feeds the verdict and the logs, browser notices reach the permission prompt, and history replay honors the before-anchor.

Validation: 306 adapter unit tests, 481 adapter integration tests, 10 field-matrix tests, all other workspace suites, Clippy with warnings denied, formatting, selftest, an MSRV 1.88 check, and live loopback 53 of 53 against both 1.4.23-R8977.1 and 1.4.20-R8619.1.

What remains: the ACP SDK 5.3 surfaces deferred in `.agents/plans/acp-sdk-5.3-adoption.md` (`session/options`, agent skills, `SessionReference`). The experimental skill provider can be revisited if ACP grows a client-supplied skill catalog and a stock host opens the gate.


## Context and Orientation


`muse-acp` is a Rust binary that bridges ACP from an editor to MSP. `src/compat.rs` classifies the schema fingerprint a host reports in its `initialize` result: each known fingerprint is a `HOST_*_FINGERPRINT` constant with a row in `table_entry` and in `selftest_lines`, `SDK_MANIFEST_FINGERPRINT` aliases the fingerprint of the vendored bundle, `SDK_SCHEMA_SHA256` pins the vendored bundle's bytes, and now `SDK_COMPATIBILITY_EPOCH` pins its root epoch. `classify` reads a fingerprint alone; `classify_served` adds the host's epoch and is what `src/msp.rs` (`compatibility`) uses. The handshake facts come from the workspace crate `crates/muse-msp-client` (`Handshake::from_result`, now with `compatibility_epoch`).

`src/main.rs` builds a computer-use permission prompt's text in `cua_card_text`; `cua_browser_lines` words the browser notices. `src/fold.rs` turns MSP items into ACP updates; `replay_history_items` replays a history batch, deferring rows whose `terminalRowAfterItemId` anchor is still ahead (`awaiting_anchor`), and now walks the batch in `before_anchor_order`.

The protocol corpus in `tests/protocol/` holds the stable schema bundle (`stable/`) and the SDK's golden transcripts (`transcripts/`); `tests/protocol/PROVENANCE.md` records the source. The field matrix is the table between `msp-1.4.x-field-matrix` markers in `ROADMAP.md`, enforced by `tests/msp_140_field_matrix.rs`. The fake MSP host is `tests/fixtures/fake_serve.py` (new knobs `FAKE_COMPATIBILITY_EPOCH` and `FAKE_APPROVAL_SUBJECT=computer-use-browser`), driven by `tests/acp_serve.rs`; the live suite is `tests/live_loopback.rs`. `.github/workflows/nightly.yml` downloads pinned Muse builds by version, file, and SHA-256.


## Plan of Work


Milestone 1 re-pins and classifies, as in the 1.4.20 plan. Milestone 2 adds the compatibility epoch to the handshake, the verdict, the log line, and the selftest header. Milestone 3 words the browser notices. Milestone 4 adds the replay ordering. Milestone 5 records dispositions and documentation and runs the gates.


## Concrete Steps


From the repository root, with the SDK cloned at `$SDK`:

    curl -sS https://api.meta.ai/muse-code/channels/muse-stable
    git -C $SDK show 5fc95e5:schema/msp/stable/msp.schema.json > tests/protocol/stable/msp.schema.json
    git -C $SDK show 5fc95e5:schema/msp/stable/manifest.json > tests/protocol/stable/manifest.json
    rm -rf tests/protocol/transcripts && mkdir tests/protocol/transcripts
    git -C $SDK archive 5fc95e5 schema/msp/transcripts | tar x --strip-components=3 -C tests/protocol/transcripts
    sha256sum tests/protocol/stable/msp.schema.json    # b72d5a3f...

Full gates:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    MUSE_CLI=$HOME/muse-bins/muse-1.4.23-R8977.1 MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Validation and Acceptance


`--selftest` prints `compatibility_epoch=0` in its header and tested `sdk-manifest`, `host-1.4.20`, and `host-1.4.23` rows. The fake-host tests `sdk_manifest_fingerprint_is_the_1_4_23_surface`, `the_1_4_20_fingerprint_stays_tested`, `an_unknown_fingerprint_at_the_vendored_epoch_is_additive`, `an_unknown_fingerprint_past_a_declared_breaking_advance_is_named`, `a_malformed_compatibility_epoch_reads_as_absent`, and `computer_use_browser_permission_words_the_hosts_notices` pass. The fold tests `a_replayed_row_renders_before_the_earlier_item_the_host_names`, `before_anchors_keep_batch_order_and_travel_with_moved_rows`, `a_deep_before_anchor_chain_does_not_recurse`, and `a_moved_row_does_not_end_the_turn_it_is_moved_into` pass. The live test `the_host_compatibility_epoch_is_recorded_from_a_real_handshake` sees `compatibility_epoch=0` on 1.4.23 and `absent` on 1.4.20, and the field-matrix test fails if any of the three new rows is removed.


## Idempotence and Recovery


The corpus copy is mechanical and repeatable from the SDK revision; never hand-edit it. Every behavior change is additive: a host that sends no epoch, no browser facts, and no before-anchor sees the same adapter behavior as before, except the extra `compatibility_epoch=absent` field in the `schema-compat` log line. Reverting the commit restores the 1.4.20 classification.


## Artifacts and Notes


Release evidence:

    channel: {"channel":"muse-stable","version":"1.4.23-R8977.1",...}
    installed: Muse Code 1.4.23 (1.4.23-R8977.1)
    release manifest msp_schema_fingerprint: sha256:b7fb9137267aa35c92424df37a5ac56287006a0e546b197bf0930d593fecbdb1
    live handshake schema: {"compatibilityEpoch":0,"experimentalFingerprint":"sha256:6b10316bb7da4b152ae90d076720323576cee999c60b44a55442a6c7bf45195d","fingerprint":"sha256:b7fb9137...","version":1}
    stable schema sha256: b72d5a3f6fd6201affcd45e4e13453a9bfeb10246649ceb6492fb6fe26c8c127
    SDK revision: 5fc95e5bbbedc7944b1729297ca79bcef9627416

Nightly checksums from the release manifest:

    Linux x64:   7ca9e16659f759eab3ae6c4b61c6f1e70f5f3fea1ca961ebd49044a1146956f4
    macOS arm64: 215a6eaac150f5855edc5a1df3b14ddfa504eb03690a4d65449887b396522fa8
    Windows x64: 9ff96e49e2c623f35ea002a4aab08c29b08878636cbe218be388f8e0d5f33de1

Probe scripts (raw MSP handshake, skill provider, background bash) were kept outside the repository; their findings are in Surprises.


## Interfaces and Dependencies


No new dependency. `crates/muse-msp-client`: `Handshake.compatibility_epoch: Option<u64>`. `src/compat.rs`: `HOST_1423_FINGERPRINT`, `SDK_COMPATIBILITY_EPOCH`, `Compatibility.compatibility_epoch`, `classify_served`. `src/main.rs`: `cua_browser_lines`. `src/fold.rs`: `SessionFold::row_before_anchor`, `SessionFold::before_anchor_order`. The `schema-compat` log line gains `compatibility_epoch=` between `schema_version=` and `fingerprint=`, and the selftest header gains `compatibility_epoch=`.


## Change Notes


2026-11-18: Created after diffing the 1.4.20 and 1.4.23 stable and experimental bundles and probing a live 1.4.23 host, and completed in the same session.
