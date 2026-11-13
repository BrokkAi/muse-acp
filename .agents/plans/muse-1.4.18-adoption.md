# Adopt the Muse 1.4.18 surface

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


Muse Code 1.4.18 (`1.4.18-R8587.1`) is the current stable release, published on 2026-11-13, one day after this repository adopted 1.4.15. Its stable MSP schema (MSP is Muse's session protocol, the JSON-RPC wire protocol `muse serve` speaks on stdio) differs from the 1.4.15 bundle vendored in this repository, and its experimental surface (which this adapter opts into with `experimentalApi`) changed how a waiting session reports its status. Today the adapter classifies a 1.4.18 host's fingerprint as `unknown` and labels every session that waits for an approval or a question with the status `unknown`.

After this change the adapter reports 1.4.18 as tested, the nightly workflow runs the real 1.4.18 builds, every new stable definition, field, and enum value has an explicit disposition, and the additions that matter to an editor user reach ACP (the Agent Client Protocol an editor such as Zed or a JetBrains IDE speaks to this adapter). An editor user sees a session waiting on them as `needsInput` with the reason (approval or question) instead of `unknown`, and an ACP v2 client sees it as `requires_action`; a shell command card titled with the short description the model gave the command (the "Bash Title") instead of the raw command line; a card when a backgrounded call's result is delivered back to the agent; a subagent card that says when its shown result was superseded by a newer attempt; specific advice when a turn fails because the conversation overflowed the model's context window or the provider refused to replay earlier reasoning; and the reason Muse refused to stop a workflow.

To verify, run the focused tests named below, the full workspace suite, `cargo run --locked -- --selftest` (which must print a tested `sdk-manifest` row and a tested `host-1.4.18` row for fingerprint `sha256:77dda185...`), and the live loopback suite against an installed Muse 1.4.18.


## Progress


- [x] (2026-11-13 09:50Z) Confirmed the channel manifest (`https://api.meta.ai/muse-code/channels/muse-stable`) names `1.4.18-R8587.1`, the installed binary matches the release manifest's Linux checksum, and the release manifest publishes `msp_schema_fingerprint` `sha256:77dda18550adef14e257b015fe4e6490b722723f7d526df5ba4867939fff68a3`.
- [x] (2026-11-13 09:55Z) Confirmed the public SDK has no revision past `07e1548` (1.4.15) and npm `@muse-code/sdk` is still 1.4.15, so the 1.4.18 bundle comes from the binary's own `muse schema generate-json-schema` export.
- [x] (2026-11-13 10:00Z) Diffed the 1.4.15 and 1.4.18 stable and experimental exports (see Context and Orientation).
- [x] (2026-11-13 10:20Z) Ran the live loopback suite against 1.4.18 with the current adapter: 52 of 52 pass. Captured the raw MSP stream and confirmed `status: "needsInput"`, `needsInputReasons`, `bashTitle`, and `schema.experimentalFingerprint` on the wire.
- [ ] Milestone 1: re-pin `tests/protocol/stable` to the 1.4.18 export, add `HOST_1418_FINGERPRINT` and the byte pin, nightly rows, PROVENANCE.
- [ ] Milestone 2: map `needsInput` and `needsInputReasons`.
- [ ] Milestone 3: map `bashTitle`, `bodyCompletion`, `SubagentResult.superseded`, the two turn error kinds, `stopRefusal`, `experimentalFingerprint`, and log the experimental load warnings and activation changes.
- [ ] Milestone 4: dispositions in `ROADMAP.md` and `tests/msp_140_field_matrix.rs`; README, `docs/acp-extensions.md`, `docs/event-compatibility.md`, CHANGELOG, and the 1.4.x adoption log.
- [ ] Milestone 5: full gates, live suite on 1.4.18 and a 1.4.15 control, PR.


## Surprises & Discoveries


- Observation: Muse 1.4.18 ships its own ACP server, `muse acp`, which 1.4.15 does not have.
  Evidence: `muse --help` on 1.4.18 lists `acp  Serve the Agent Client Protocol to an editor over stdio`; 1.4.15's help has no such command. An ACP `initialize` probe with `--provider echo` returns protocol version 1, `loadSession`, `sessionCapabilities.list`, `resume`, and `additionalDirectories`, http MCP only, image and embedded-context prompts, a single `muse-login` auth method, and approval modes as the session mode selector. It has no ACP v2, no fork or side chat, no model or effort selector on an echo session, and no editor fs/terminal bridge. The comparison is recorded in `.agents/docs/native-muse-acp.md`; it does not change this plan.

- Observation: The SDK has not published 1.4.16, 1.4.17, or 1.4.18.
  Evidence: `gh api repos/meta-models/muse-code-sdk/commits` newest is `07e1548` (2026-11-10, "remirror: 1.4.15"); npm `@muse-code/sdk` is 1.4.15. The transcripts corpus therefore stays at `07e1548`.

- Observation: The R-numbers of 1.4.16 and 1.4.17 are not discoverable.
  Evidence: the channel manifest only names the current build, the launcher deleted the earlier binaries on update, and no local state or session history records them. Their fingerprints stay unclassified (`unknown`), which degrades loudly but never blocks.

- Observation: The stable change from 1.4.15 to 1.4.18 is small and purely additive; the experimental change is larger.
  Evidence: only `$defs` differs in the stable bundle (methods, notifications, requests, errors, capabilities, and reserved names are identical). The experimental surface adds seven methods (`session/rewind`, `session/rewindAttach`, `session/rewindPoints`, `session/rewindReadInput`, `subagent/workStop`, `mcpServer/readStartupNotices`, `pluginData/read`) and one notification (`session/activationChanged`).

- Observation: All four sandbox-bound live tests now pass in this environment.
  Evidence: `an_approved_shell_command_runs_and_a_rejected_one_does_not`, `a_piped_shell_command_asks_with_every_stage`, `auto_review_decides_a_shell_approval_against_a_real_host`, and `monitor_feed_reaches_a_real_host` passed against 1.4.18 on 2026-11-13, so the earlier `bwrap` limitation no longer applies here.

- Observation: The three open "Nightly failure" issues (#472, #473, #474) are not caused by Muse.
  Evidence: every failed job fails compiling `muse-acp` with `duplicated attribute`, which `99ad6fa` ("Remove duplicate Clippy allowance") fixed on master.


## Decision Log


- Decision: Vendor the 1.4.18 binary's own stable export as `tests/protocol/stable`, keep the transcripts at SDK `07e1548`, and classify fingerprint `sha256:77dda185...` as tested.
  Rationale: The SDK has not published 1.4.18. The binary's export is exact for that binary (the help text says the bundles are precomputed at build), the release manifest and live handshake agree on the fingerprint, and the nightly matrix gains pinned 1.4.18 builds. Waiting for the SDK would leave the current stable host unclassified.
  Date/Author: 2026-11-13, Claude.

- Decision: Accept `needsInput` as a known session status, carry it verbatim in `_meta.muse.status`, treat it as ACP v2 `requires_action`, and carry `needsInputReasons` in `_meta.muse.needsInputReasons`.
  Rationale: Muse 1.4.18 sends it to a client that opted into `experimentalApi`, which this adapter does. It means a loaded session is waiting for its user, exactly what `requires_action` means, and projecting it as `unknown` lost that.
  Date/Author: 2026-11-13, Claude.

- Decision: Title a shell tool card with `Item.bashTitle` when the host sends it and sends no `displayLabel`, and keep the command in `rawInput`, in the ACP v2 terminal, and in the approval prompt.
  Rationale: The host serves the Bash Title only while its own `bash_titles` gate is on, which is Muse's own decision to title Bash rows by their description. The host's `displayLabel` still wins, as it does today.
  Date/Author: 2026-11-13, Claude.

- Decision: Render `bodyCompletion` as a host card titled by the terminal status, carry the facts verbatim in `_meta.muse.bodyCompletion` beside `taskId`.
  Rationale: It is the receipt row the host writes when a backgrounded call's result reaches the agent. `originCallId` is the provider call id, which is the ACP tool call id this adapter already uses for that call, so the editor can join it without the adapter guessing.
  Date/Author: 2026-11-13, Claude.

- Decision: When `result.superseded` is true, add a line to the subagent card and never settle the child's state from that result.
  Rationale: The schema says a superseded result is from an earlier attempt and the child's status comes from `controlStatus`; settling a `resultReady` child from a stale result would end it early.
  Date/Author: 2026-11-13, Claude.

- Decision: Log `schema.experimentalFingerprint`, `session/start` and `session/resume` `warnings`, and `session/activationChanged`; do not map the experimental rewind, work-stop, MCP startup notice, plugin data, node, or journal additions.
  Rationale: The adapter has no ACP surface for those yet. Logging keeps them diagnosable and stops `session/activationChanged` from taking the unhandled-notification path. Each is listed in Outcomes as a follow-up.
  Date/Author: 2026-11-13, Claude.

- Decision: Leave `SessionConfig.developerPrompt` (now stable) as `Unsupported pending protocol decision` linked to #318 in this plan, and implement ACP `systemPrompt` on top of it as a separate change.
  Rationale: Mapping ACP's `append` and `override` modes onto Muse's "replace the settings-derived developer instructions" needs its own design and tests; keeping it out keeps this adoption reviewable.
  Date/Author: 2026-11-13, Claude.


## Outcomes & Retrospective


(To be written at completion.)


## Context and Orientation


`muse-acp` is a Rust binary that bridges ACP from an editor to MSP. `src/main.rs` routes ACP requests and MSP notifications and owns approval, resume, session-status, and lifecycle code. `src/fold.rs` turns MSP items (the host's transcript rows: messages, tool calls, subagents, and host cards) into ACP `session/update` notifications; `host_card_parts` builds the title, content, and `_meta.muse` fields for host-side item kinds, `tool_title` titles tool cards, and `subagent_state` derives a child's ACP state. `src/compat.rs` classifies a host's schema fingerprint. `src/msp.rs` launches `muse serve` and logs the handshake. `src/acp.rs` holds per-session state and the `_meta.muse` snapshot sent in `session_info_update`.

The protocol corpus in `tests/protocol/` holds the stable schema bundle (`stable/`) and the SDK's golden transcripts (`transcripts/`); `tests/protocol/PROVENANCE.md` records the source. The field matrix is the table between `msp-1.4.x-field-matrix` markers in `ROADMAP.md`, enforced by `tests/msp_140_field_matrix.rs`; the notification matrix is in `docs/event-compatibility.md`, enforced by a test in `src/compat.rs`. The fake MSP host is `tests/fixtures/fake_serve.py`, driven by `tests/acp_serve.rs`; the live suite is `tests/live_loopback.rs`.

The 1.4.18 stable additions over 1.4.15 are: item kind `bodyCompletion` with definitions `BodyCompletionMembers` (`status`, `resultSummary`, `correlation`, `receipt`, optional `originCallId`) and `BodyCompletionCorrelation` (`resolved`, `unresolved`); fields `Item.bodyCompletion`, `Item.bashTitle`, `ErrorData.stopRefusal`, `SchemaInfo.experimentalFingerprint`, `SessionConfig.developerPrompt`, `SessionReadResult.modelTurnRunning`, and `SubagentResult.superseded`; enum values `NoticeOrigin.continuedChildTools`, `TurnErrorKind.contextOverflow`, and `TurnErrorKind.reasoningReplayRefused`. `Item.taskId` is now also set on an unresolved `bodyCompletion`, and `Session.activeTurnId` stays set while a turn is parked under `needsInput`. Nothing was removed or newly required.

The experimental additions this plan touches are `SessionStatus.needsInput`, `Session.needsInputReasons` and `SessionStatusChangedParams.needsInputReasons` (open values `approvalPending`, `inputPending`), `SessionStartResult.warnings` and `SessionResumeResult.warnings`, and the `session/activationChanged` notification.


## Plan of Work


Milestone 1 re-pins and classifies. Copy the 1.4.18 export over `tests/protocol/stable`; add `HOST_1418_FINGERPRINT` in `src/compat.rs`, point `SDK_MANIFEST_FINGERPRINT` at it, update `SDK_SCHEMA_SHA256`, add a table arm and a selftest row, and reword the 1.4.15 entry as previously vendored; add 1.4.18 rows to `.github/workflows/nightly.yml` (Linux, macOS arm64 replacing 1.4.15 as newest, Windows); update `tests/protocol/PROVENANCE.md`.

Milestone 2 maps `needsInput`. In `src/main.rs`, `session_status_projection` keeps `needsInput`; `current_state_snapshot` and `send_session_projection` treat it as `requires_action`; `adopt_session_projection`, `adopt_streamed_session_row`, and `adopt_status_changed` carry `needsInputReasons` (a status event or streamed row that omits it clears it; a lifecycle row that omits it keeps it); `LIST_ROW_FACTS` gains it.

Milestone 3 maps the rest. In `src/fold.rs`, `tool_title` uses `bashTitle` after `displayLabel`, and `tool_muse_fields` carries it; `host_card_parts` gains a `bodyCompletion` arm; the subagent card adds a superseded line and `superseded` meta, and `subagent_state` skips a superseded `resultReady` result. In `src/main.rs`, the turn failure text adds sentences for `contextOverflow` and `reasoningReplayRefused`, the workflow stop error names `stopRefusal`, and the start/resume paths log `warnings`; `session/activationChanged` is logged. In `src/msp.rs`, `compatibility_check` logs the experimental fingerprint.

Milestone 4 records dispositions and documentation, and milestone 5 runs the gates and opens the PR.


## Concrete Steps


From the repository root:

    curl -sS https://api.meta.ai/muse-code/channels/muse-stable
    ~/.local/bin/muse-bin-1.4.18-R8587.1 schema generate-json-schema --out /tmp/m118/stable
    cp /tmp/m118/stable/manifest.json /tmp/m118/stable/msp.schema.json tests/protocol/stable/
    sha256sum tests/protocol/stable/msp.schema.json

Focused tests while iterating:

    cargo test --locked -p muse-acp --bin muse-acp compat
    cargo test --locked -p muse-acp --bin muse-acp fold
    cargo test --locked --test msp_140_field_matrix
    cargo test --locked --test acp_serve

Full gates:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2
    MUSE_CLI=/tmp/muse-bins/1.4.15-R8339.1/muse MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp


## Validation and Acceptance


`cargo run --locked -- --selftest` prints `kind=sdk-manifest ... fingerprint=sha256:77dda185... status=tested` and a tested `kind=host-1.4.18` row. A live 1.4.18 adapter log shows `schema-compat ... status=tested detail=validated against live host 1.4.18-R8587.1` and an `experimental_fingerprint=sha256:43a76702...` line.

Unit and fake-host tests show: a `session/statusChanged` with `needsInput` and `["approvalPending"]` producing `_meta.muse.status: "needsInput"`, `_meta.muse.needsInputReasons`, and on ACP v2 a `requires_action` state, and a following `running` event clearing the reasons; a bash tool card titled `Write the piped artifact`; a `bodyCompletion` card titled `Background task failed` with its summary and facts; a superseded subagent card with its line and no premature settle; a `contextOverflow` failure suggesting `/compact`; and a workflow stop error naming `stale_revision`.


## Idempotence and Recovery


The corpus copy is mechanical and repeatable from the 1.4.18 binary; never hand-edit it. Every mapping is additive: a host before 1.4.18 sends none of the new fields, kinds, statuses, or notifications, and every path is a no-op without them.


## Artifacts and Notes


Release evidence:

    channel: {"channel":"muse-stable","version":"1.4.18-R8587.1",...}
    installed: Muse Code 1.4.18 (1.4.18-R8587.1)
    release manifest msp_schema_fingerprint: sha256:77dda18550adef14e257b015fe4e6490b722723f7d526df5ba4867939fff68a3
    live handshake fingerprint: (same)
    live handshake experimentalFingerprint: sha256:43a76702a69cdfe355db9edac2c7efdc8f75ae40daaae28789ad1d6ec68c3603
    stable schema sha256: b0b823c56e7b955a456cd5300b27eb1792f425da33918631b6aa08fa0dc71924

Nightly checksums from the release manifest:

    Linux x64:   57732db89e25a58196c923b3790aa3e2a85b8d1ad5094492384792260e65b538
    macOS arm64: 3f6da0d865ceb97dfe3002372ff1551b6a187dccc740866456a546510b1412e7
    Windows x64: f9550f99c31934ed6728ab50b4758ac8052c8f2899948a798ed73c1f1106d263

Live MSP capture (52-test run, tee'd stream): `"status":"needsInput"` 8 times with `needsInputReasons` `["approvalPending"]` or `["inputPending"]`; `"bashTitle":"Write the piped artifact"`.


## Interfaces and Dependencies


No new dependency. `src/compat.rs` gains `pub const HOST_1418_FINGERPRINT`; `SDK_MANIFEST_FINGERPRINT` aliases it. The ACP-visible additions are `_meta.muse.needsInputReasons` (session info and list rows), `_meta.muse.bashTitle` on tool cards, `_meta.muse.bodyCompletion`, and `_meta.muse.subagent.superseded`.


## Change Notes


2026-11-13: Created after diffing the 1.4.18 stable and experimental exports against the vendored 1.4.15 bundle and running the live suite on 1.4.18.
