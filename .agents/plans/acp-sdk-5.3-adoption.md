# Adopt ACP Rust SDK 5.3.0: system prompts and replay completion

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


`muse-acp` speaks ACP (the Agent Client Protocol an editor such as Zed or a JetBrains IDE uses to drive a coding agent over stdio) through the official Rust SDK, the `agent-client-protocol` crate, pinned exactly in `Cargo.toml`. It translates ACP into MSP, the JSON-RPC session protocol `muse serve` speaks. The pin was `=5.0.0` (ACP schema 1.15.1); the crate has since published 5.1.0, 5.2.0, and 5.3.0 (schema 1.18.0).

After this change the adapter builds against 5.3.0 and offers what the new schema adds that Muse can honour. An editor can give a session system-level instructions (ACP `systemPrompt`, now stable), and Muse applies them as the session's developer prompt. An ACP v2 editor that resumes a session with history replay learns exactly where the replay ends (`replay_complete`), which the v2 schema now requires; without it the editor must treat the replay as truncated.

To see it working: initialize against a Muse 1.4.18+ host and observe `"systemPrompt":{}` in the session capabilities; create a session with `{"systemPrompt":{"mode":"append","content":[{"type":"text","text":"Answer in French."}]}}` and observe `config.developerPrompt` on the MSP `session/start`; resume a session over ACP v2 with `replayFrom: {"type":"start"}` and observe one `replay_complete` update after the replayed history and before the response.


## Progress


- [x] (2026-11-16 20:30Z) Bumped `agent-client-protocol` to `=5.3.0` (`cargo update -p agent-client-protocol --precise 5.3.0`); it builds with no source change, and the workspace suite and Clippy pass unchanged.
- [x] (2026-11-16 20:45Z) Diffed schema 1.15.1 against 1.18.0 and the 5.0.0 and 5.3.0 crate changelogs (see Context and Orientation).
- [x] (2026-11-16 21:10Z) Probed a live Muse 1.4.20 host for where `developerPrompt` lands and how it behaves across fork, resume of a loaded session, and resume after a restart (see Surprises).
- [x] (2026-11-16 22:00Z) Mapped `systemPrompt` to `config.developerPrompt` on new, load, resume, and fork, with the capability gated on Muse 1.4.18+; seven fake-host tests.
- [x] (2026-11-16 23:30Z) Sent `replay_complete` after a complete ACP v2 replay and withheld it after a downgraded one; two fake-host tests.
- [x] (2026-11-16 23:40Z) README, `docs/acp-extensions.md`, the ROADMAP field-matrix row for `SessionConfig.developerPrompt`, CHANGELOG.
- [x] (2026-11-17 00:10Z) Gates: fmt, Clippy, workspace suite (294 unit, 475 integration), selftest, npm launcher 4/4, Python 41 (6 skips), shell syntax, npm smoke, live loopback 52/52 against 1.4.20-R8619.1.
- [ ] Commit and run the SlopCop static check.


## Surprises & Discoveries


- Observation: The SDK bump is source-compatible for this adapter.
  Evidence: `cargo build` and `cargo test --locked --workspace` pass after the bump with no edit. Schema 1.17 removed the unstable `PlanFile` and 1.16 replaced the `unstable_session_system_prompt` feature with a stable field, but the adapter used neither.

- Observation: Muse places a developer prompt at the head of the developer message and keeps its built-in instructions.
  Evidence: with a loopback model provider dumping request bodies, a session started with `developerPrompt` "ZEBRA-DEV-PROMPT: always answer in French." sent `instructions` of 63835 characters (unchanged from a session without one) and a developer message of 35178 characters beginning with that text, against 35134 without it. A blank `developerPrompt` sent 33937: Muse's explicit clear also drops the agent catalog and memory snapshot.

- Observation: Muse stores the developer prompt with the session, does not carry it into a fork, and applies a new one on a resume of a loaded session.
  Evidence: a fork of a session started with prompt A ran without A; `session/resume` of the loaded fork with A then ran with A; a loaded session resumed with B ran with B; after a host restart, a resume with no prompt ran with the last applied prompt, and a resume with C ran with C.

- Observation: Muse has no counterpart for the other 1.17 and 1.18 additions.
  Evidence: neither the stable nor the experimental 1.4.20 MSP schema defines prompt suggestions or a speed or fast-mode setting.


## Decision Log


- Decision: Advertise `systemPrompt` in `append` mode only, and only on Muse 1.4.18+.
  Rationale: Muse adds the text and keeps its own instructions, which is ACP's `append`. It has nothing that replaces the built-in instructions, so `override` is refused as ACP requires. `developerPrompt` is stable from 1.4.18, and a host that does not serve it ignores it silently, so the capability is gated on the reported version (`Handshake::supports_developer_prompt`).
  Date/Author: 2026-11-16, Claude.

- Decision: Join text blocks with a blank line; refuse non-text blocks, an empty list, and a prompt with no text.
  Rationale: ACP requires agents advertising the capability to accept text and to error on blocks they do not support. Muse reads a blank developer prompt as an explicit clear that also turns off the agent catalog and memory snapshot, which is not what any ACP mode means.
  Date/Author: 2026-11-16, Claude.

- Decision: Apply a fork's prompt with a follow-up `session/resume` (`history: "snapshot"`) of the loaded fork.
  Rationale: MSP `session/fork` takes no configuration and Muse does not copy the source's prompt, while a resume of a loaded session applies one. Snapshot history is the lightest rung that is always served.
  Date/Author: 2026-11-16, Claude.

- Decision: Keep the session's last prompt in `AcpSession.developer_prompt` and resend it when a restarted host restarts or reattaches the session.
  Rationale: Muse saves a session only with its first turn, so a session that never ran is started again under its id and would otherwise lose the prompt.
  Date/Author: 2026-11-16, Claude.

- Decision: Send `replay_complete` only when the resume served `history.mode: "inline"`.
  Rationale: The schema says the agent sends it after the last replayed update and does not send it when it cannot finish the replay. The adapter asks for inline history; a downgraded or unreadable history replays only part.
  Date/Author: 2026-11-16, Claude.

- Decision: Do not adopt `SessionReference` tool-call content, `session/options`, agent skills, prompt suggestions, or the speed config category in this change.
  Rationale: MSP gives no link from a spawning tool call to the child session it created, and child sessions are already announced with `subagent_update`, so there is no card to reference from without guessing. Prompt suggestions and speed have no Muse source. `session/options` and agent skills are unstable surfaces that each need their own design.
  Date/Author: 2026-11-16, Claude.


## Outcomes & Retrospective


The adapter builds against `agent-client-protocol` 5.3.0, offers stable ACP system prompts on Muse 1.4.18+ (closing #318), and marks the end of ACP v2 history replays. All gates pass, including the live suite against Muse 1.4.20 (52/52).

What remains: `session/options` and initial config options (unstable), agent skills as `skill` command entries with `skill` prompt blocks (unstable, v2), and `SessionReference` content if MSP ever links a spawning call to its child.


## Context and Orientation


The pin and features live in `Cargo.toml`. `src/request.rs` maps each protocol version's typed request onto one view per method; the lifecycle view is `Setup`, now with `system_prompt`. `src/main.rs` handles requests: `v1_init` and `v2_init` build the `initialize` responses, the `Request::NewSession`, `Request::ResumeSession | Request::LoadSession`, and `Request::ForkSession` arms drive MSP, `session_config_field` builds the MSP `config` member, `developer_prompt` and `setup_developer_prompt` validate a `systemPrompt`, `resume_session`, `reattach_session`, and `restart_unsaved_session` carry the prompt, `apply_developer_prompt` gives a fork its prompt, and `replay_history` reports whether a replay was complete. `src/out.rs` builds ACP notifications (`replay_complete`). `src/msp.rs` holds the host handshake and its version gates. `tests/acp_serve.rs` drives the adapter against the fake MSP host `tests/fixtures/fake_serve.py`; `host_requests` reads the MSP frames the adapter sent.

Schema changes between 1.15.1 and 1.18.0 that matter here: `systemPrompt` (`{mode, content}`, modes `append`, `override`, or custom) on new, load, resume, and fork requests with the agent capability `systemPrompt` (1.16, stable); the v2 `replay_complete` session update (sent once, after a requested replay and before the response); v2 `SessionReference` tool-call content; and the unstable `session/options` with initial `configOptions` (1.17), agent skills, prompt suggestions, and the `speed` config category (1.18). The unstable `PlanFile` was removed.


## Plan of Work


Milestone 1 bumps the pin and confirms the suite passes unchanged. Milestone 2 maps `systemPrompt` as described in the Decision Log and adds the fake-host tests named below. Milestone 3 adds `replay_complete`. Milestone 4 updates the documentation and runs the gates.


## Concrete Steps


From the repository root:

    cargo update -p agent-client-protocol --precise 5.3.0
    cargo test --locked --test acp_serve system_prompt
    cargo test --locked --test acp_serve -- replay_marker end_of_a_requested_replay

Full gates:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    MUSE_ACP_LOOPBACK=1 cargo test --locked -p muse-acp --test live_loopback -- --test-threads 2


## Validation and Acceptance


The fake-host tests show: `systemPrompt: {}` advertised on 1.4.18 and 1.4.20 and not on 1.4.15, for v1 and v2; two text blocks reaching `session/start` as `config.developerPrompt` "Answer in French.\n\nBe brief."; no `config` without a prompt; `override`, a custom mode, an empty list, blank text, and a resource link each refused with `-32602` and no `session/start`; a prompt on a 1.4.15 host refused; load and resume carrying the prompt, and a later resume without one sending no `config`; a fork with a prompt followed by a `session/resume` of the fork with that prompt and `history: "snapshot"`; exactly one `replay_complete` after the replayed history and before the response for `replayFrom: {"type":"start"}`, none without a cursor, and none when the host served an `anchoredSnapshot`.


## Idempotence and Recovery


Every change is additive: a client that sends no `systemPrompt` and no `replayFrom` sees the same traffic as before. Reverting the commit restores the 5.0.0 pin and behaviour.


## Artifacts and Notes


Probe scripts and request dumps were kept outside the repository; the measurements are recorded in Surprises.


## Interfaces and Dependencies


`agent-client-protocol` `=5.3.0` (schema crate 1.18.0); no new dependency. New: `request::SystemPrompt`, `Setup.system_prompt`, `AcpSession.developer_prompt`, `Handshake::supports_developer_prompt`, `out::replay_complete`.


## Change Notes


2026-11-16: Created during the upgrade, after the schema diff and the live developer-prompt probe.
