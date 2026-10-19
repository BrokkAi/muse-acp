# Adopt generated session short titles

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agents/PLANS.md` from the repository root.


## Purpose / Big Picture

Muse 1.4.8 can serve a generated short title separately from a session's canonical `name`. An editor connected through `muse-acp` currently receives only the canonical title and provenance metadata. After this change, a client can display the host-generated short title from `session_info_update._meta.muse.generatedName` without the adapter confusing it with, or fabricating it from, the canonical name.

The behavior is observable with the fake MSP host and with a real Muse host whose snapshot or `session/nameChanged` frame carries `generatedName`. A client that understands the metadata receives the latest host-authored generated value; a client that ignores it sees the same canonical title behavior as before.


## Progress

- [x] (2026-10-19) Read issue #256, the vendored `Session.generatedName` and `SessionNameChangedParams.generatedName` schemas, and the existing title/origin paths.
- [x] (2026-10-19) Created branch `issue-256/generated-name` from `origin/master` at `63244b7`.
- [x] Add optional generated-title state to `HostTitleFacts` without changing canonical title selection.
- [x] Carry snapshot generated names through start, resume, load, fork, and reattach.
- [x] Carry live generated names through `session/nameChanged`, including replacement and explicit withdrawal.
- [x] Keep `generatedName` out of `session/list` rows and preserve it across list-row replacements.
- [x] Add unit and fake-host regression coverage.
- [x] Add real-host coverage for the non-fabrication/compatibility half on Muse 1.4.8+.
- [x] Update README, ROADMAP, event compatibility, CHANGELOG, and this plan.
- [x] Run all contribution gates and the focused live test.
- [x] Open [PR #289](https://github.com/BrokkAi/muse-acp/pull/289).
- [x] Complete three review/fix passes.
- [ ] Merge only after final CI is green.


## Surprises & Discoveries

- Observation: the generated title is not another candidate in the canonical title fallback chain and must never be inferred from `Session.name`.
  Evidence: the vendored schema says `name` stays canonical and absence of `generatedName` reads as unpublished.

- Observation: `Session.generatedName` is fold-only, while `session/list` rows never carry it.
  Evidence: both field descriptions in `tests/protocol/stable/msp.schema.json` explicitly exclude list rows.

- Observation: `SessionNameChangedParams` is replace-wholesale, but `generatedName` remains additive-optional.
  Evidence: the schema description says an already-published generated pair is carried again at settlement and omissions mean no pair was ever published.

- Observation: an MSP-created or ordinary loopback `muse` session on the installed Muse 1.4.10 does not publish a generated pair, even after several completed turns.
  Evidence: the live adapter and a TUI/headless Muse session produced canonical first-prompt titles, but no `generatedName` in any lifecycle snapshot or `session/nameChanged`; the native loopback provider logged no session-description call. The focused test also passes against the checksum-pinned Muse 1.4.8 binary. The live test therefore pins non-fabrication while fake-host tests pin adoption.


## Decision Log

- Decision: store the generated short title as `HostTitleFacts::generated_name` and expose it independently of `HostTitleFacts::selected`.
  Rationale: the generated title is metadata, not an ACP title fallback. Keeping it separate prevents canonical-title behavior from changing.
  Date/Author: 2026-10-19, Codex.

- Decision: publish a present generated title as `session_info_update._meta.muse.generatedName` on the same update as the canonical title and name origin.
  Rationale: one session-info update gives clients a consistent title snapshot and avoids a second metadata notification. Existing clients can ignore the namespaced key.
  Date/Author: 2026-10-19, Codex.

- Decision: treat lifecycle snapshots and `session/nameChanged` as authoritative replacements for the generated-name fact, but treat `session/listChanged` rows as non-authoritative.
  Rationale: snapshots and name-change frames fold the durable fact, while list rows explicitly never carry it. A list-row omission must not erase a value learned elsewhere.
  Date/Author: 2026-10-19, Codex.

- Decision: an authoritative replacement that removes a previously learned generated title publishes `generatedName: null`; an initial absence omits the key entirely.
  Rationale: null withdraws stale client metadata if a host serves a later fold without a pair, while omission preserves the schema's "never published" representation and prevents clients from treating every unnamed session as explicitly cleared.
  Date/Author: 2026-10-19, Codex.

- Decision: only accept non-empty string generated names, matching the adapter's existing host-title fact policy.
  Rationale: empty or malformed metadata is not a usable host-authored short title and must not become invented UI text.
  Date/Author: 2026-10-19, Codex.


## Outcomes & Retrospective

Implementation is complete, all contribution gates pass, PR #289 is open, and all three review/fix passes are complete. Fake-host coverage proves snapshot restoration, live replacement, withdrawal, list-row preservation, and no fabrication. The real-host test proves the compatibility half on checksum-pinned Muse 1.4.8 and installed 1.4.10: a host that publishes no generated pair does not acquire metadata from its canonical title or a custom rename. Final CI remains.


## Context and Orientation

`src/acp.rs` defines `HostTitleFacts`, the small collection of host-authored values used to choose an ACP session title, and `send_session_title`, which emits a `session_info_update`. The canonical ACP `title` is selected from `Session.name`, then `Session.title`, then `Session.firstUserPrompt`. `Session.nameOrigin` describes only `Session.name`.

`src/main.rs` receives ACP lifecycle requests and MSP notifications. The important paths are `session/start`, `session/fork`, `session/load`, `session/resume`, the reconnect helper `refresh_resumed_session`, the live `session/nameChanged` arm, and `session/listChanged`. Lifecycle results and name-change frames can prove the generated-title fact. `session/list` and `session/listChanged` rows cannot.

`tests/fixtures/fake_serve.py` is a scripted MSP host launched by integration tests. `tests/acp_serve.rs` drives the real adapter process against it and inspects the ACP frames. `tests/live_loopback.rs` drives a real Muse binary with a deterministic loopback model provider. Tests in `src/main.rs` cover the title-fact folding helpers in isolation.

`ROADMAP.md` section 9 catalogs the stable Muse API surface. `docs/event-compatibility.md` records notification dispositions. `README.md` documents session names for users, and `CHANGELOG.md` records externally visible additions.


## Plan of Work

First, extend `HostTitleFacts` in `src/acp.rs` with an optional `generated_name`. Add a helper that returns whether all publishable title metadata changed, comparing canonical title, known name origin, and generated name. Extend `send_session_title` so a caller may omit the generated-name key, publish a string, or publish null. Build a single `_meta.muse` object containing both `nameOrigin` and `generatedName` when both are present, rather than emitting duplicate metadata objects.

Second, update title-fact folding in `src/main.rs`. Read only a non-empty string `generatedName` from `Session` and `SessionNameChangedParams`. Keep ordinary partial updates such as list rows from clearing it. Add an explicit replacement helper for lifecycle snapshots and name-change frames so an absent field becomes `None`; when that removes an existing value, emit an explicit null withdrawal.

Third, carry the metadata through every canonical-title emission: initial start, fork, load/resume, resumed-session refresh, live name changes, and list-change title updates. Make sure a generated-only change still emits the session-info update. Do not add generated names to `session_info_row`, `owned_session_row`, or any `session/list` response.

Fourth, extend the fake host with scenarios for a generated name in the initial snapshot, a live generated pair with replacement, a later authoritative omission, and a list-row update that omits generated metadata. Add unit tests proving no inference from `name` and proving list-row preservation. Add integration assertions that ACP receives `_meta.muse.generatedName`, that generated-only changes publish, that withdrawal emits null, and that list rows never contain the key.

Probe the live host path and scope its assertion to what that path can prove. The native loopback host publishes canonical first-prompt and rename titles without a generated pair, so the live test must assert that neither path fabricates `generatedName`. The fake host supplies the positive snapshot, replacement, and withdrawal evidence because the host-native path does not expose a generated pair.

Finally, update user and compatibility documentation, run every repository gate, open a pull request that closes #256, review the actual diff three times, fix real findings, and merge only after the final required checks pass.


## Concrete Steps

Run commands from:

    /workspace/ada1c13475327f13220f278a3a763540/muse-acp

Format and run focused tests while iterating:

    cargo fmt
    cargo test --locked generated_name -- --nocapture
    cargo test --locked --test acp_serve generated_name -- --nocapture

Run the focused real-host test:

    MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback real_host_without_generated_pair_keeps_it_unpublished -- --nocapture

Run all contribution gates before opening or updating the pull request:

    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Publish the branch and open the pull request:

    git push -u origin issue-256/generated-name
    gh pr create --base master --head issue-256/generated-name

After each review pass, commit and push fixes. Merge only after the latest head's required checks are green:

    gh pr checks <number>
    gh pr merge <number> --merge


## Validation and Acceptance

A fake host session started with `generatedName: "Generated short"` emits a `session_info_update` whose canonical title is the host's `name` and whose `_meta.muse.generatedName` is `"Generated short"`. A session without `generatedName` emits no generated-name key and does not substitute its `name` or title fallback.

A live `session/nameChanged` carrying a new `generatedName` updates the metadata even when the canonical name is unchanged. A later authoritative name-change frame without one withdraws a previously learned value with `generatedName: null`; an initial name-change frame without one publishes nothing.

`session/list` rows and list-change frames never expose or infer `generatedName`, and a list-change omission does not erase a generated value already held for the session.

All unit, integration, live, and repository gates are green. The live host proves that canonical titles and custom renames remain unpublished when the host supplies no generated pair; the fake host proves positive adoption and withdrawal. The pull request closes #256, receives three documented review/fix passes, and is merged only at a green final head.


## Idempotence and Recovery

Repeated fake-host and live tests are safe. The implementation keeps no persistent file or external state. If a test fails, inspect the emitted ACP frame and the fake-host log before changing behavior. Re-running `cargo fmt` and the tests is idempotent. A branch push updates only the issue branch until the pull request is merged.


## Artifacts and Notes

The key output shape is expected to resemble:

    {"sessionUpdate":"session_info_update","title":"Canonical name",
     "_meta":{"muse":{"nameOrigin":"automatic",
                      "generatedName":"Generated short"}}}

An initial update with no generated title omits `generatedName`. A withdrawal after a previously published generated title uses:

    "generatedName":null


## Interfaces and Dependencies

At completion, `src/acp.rs` defines `HostTitleFacts` with a public `generated_name: Option<String>` member. `send_session_title` accepts enough information to distinguish omission, a string generated title, and explicit null, and emits at most one `_meta.muse` object containing the publishable name-origin and generated-name facts.

At completion, `src/main.rs` provides a helper that snapshots the canonical title, name origin, and generated name for change detection; an authoritative generated-name replacement helper used by lifecycle snapshots and `SessionNameChangedParams`; and non-authoritative folding used by list rows. No new dependency is required.

Change note (2026-10-19): review pass 1 corrected the live-test description and progress to match the implemented and locally validated behavior.

Change note (2026-10-19): review pass 2 strengthened the fake-host list test so a canonical list-row change must republish the previously learned generated title, proving that the fold-only value survives a list read rather than merely checking that the response omits it.

Change note (2026-10-19): review pass 3 corrected the lifecycle helper's absent/malformed/clear contract, narrowed the live-host claim to the checksum-pinned 1.4.8 and installed 1.4.10 evidence, removed a 1.4.8 title-event race from the live test, and recorded completion of all three review passes.
