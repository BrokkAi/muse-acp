# Harden the October 2026 security-audit follow-ups

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


The October 2026 security audit found several ways for untrusted content or an unauthenticated local process to influence what a user sees or to consume adapter resources. After this change, auto-review trusts only text the user typed rather than attached file contents, untrusted transcript evidence cannot create prompt headings, permission titles visibly escape characters that can hide an approval's real effect, and the local editor HTTP bridge authenticates a request before buffering its body. The bridge also bounds concurrent and idle connections. Release and fuzz workflows use immutable action commits, while the documentation and startup log make an inherited `allowAll` approval mode explicit.

A user can verify the result by running the focused Rust and integration tests described below and by starting a session whose fake host reports `allowAll`; the adapter logs a warning naming the safer `MUSE_APPROVAL_MODE=promptUnmatched` setting. Reading the README also makes clear that Read-only and Plan constrain Muse's file and shell tools but do not sandbox client-provided MCP tools.

This plan addresses GitHub issues #434, #435, #436, #437, #439, and #440, plus findings L2, L3, L4, L5, L7, and I4 from #441. It does not implement the unrelated ACP protocol feature issues #316, #317, #318, #396, #397, or the crates.io publication issue #375.


## Progress


- [x] (2026-10-29 00:00Z) Read the repository instructions, contribution requirements, open issue descriptions, and referenced fork fixes.
- [x] (2026-10-29 00:00Z) Confirmed the worktree starts clean at release tag `v0.18.1`.
- [x] (2026-10-29 00:00Z) Scoped the batch to the actionable security-audit issues and recorded the existing keep-MCP policy for Read-only and Plan.
- [x] (2026-10-29 00:20Z) Added and tested typed-user-text extraction plus escaped untrusted evidence in `src/reviewer.rs`, and used it when prompts are recorded in `src/main.rs`.
- [x] (2026-10-29 00:35Z) Added and tested visible escaping for control, bidi-format, and zero-width characters in permission and tool-card titles.
- [x] (2026-10-29 00:50Z) Refactored the editor bridge so authorization and size validation happen from headers before the body is read, and capped active connections at 32 with a 60-second idle timeout.
- [x] (2026-10-29 00:52Z) Pinned the remaining mutable action references in `.github/workflows/release.yml` and `.github/workflows/fuzz.yml`.
- [x] (2026-10-29 00:58Z) Clarified Read-only/Plan and client MCP behavior in the human documentation and ACP option description.
- [x] (2026-10-29 01:05Z) Warned when the effective approval mode is `allowAll` and documented the risk and safer setting.
- [x] (2026-10-29 01:20Z) Hardened default Unix CLI resolution, bounded MSP stdout frames at 16 MiB, required explicit deny decisions for fallback cancellation, rejected bare JSON allows, and removed host/reviewer text from automatic stderr logs.
- [x] (2026-10-29 01:45Z) Ran formatting, linting, focused tests, the full workspace suite, selftest, npm, installer, and script checks.
- [x] (2026-10-29 01:50Z) Updated this plan's outcome and prepared a concise handoff naming issues intentionally left open.


## Surprises & Discoveries


- Observation: The current auto-review store records the entire accepted ACP prompt as `user: {acp_content}`. That JSON includes `resource` blocks, so attached file or page text is categorized as trusted user instructions.
  Evidence: `src/main.rs` calls `remember_review_line` with the complete `acp_content` string, while `resource` blocks are part of that array.

- Observation: Untrusted transcript entries are concatenated into the reviewer prompt without an escaping boundary.
  Evidence: `reviewer::build_prompt` currently appends each `transcript` string and a newline, so a newline inside a tool result can begin a fake heading.

- Observation: Permission titles are built from host-controlled command, path, target, and raw argument text without visible escaping.
  Evidence: `approval_title_kind` in `src/main.rs` and `bridge::approval_title` preserve the raw detail. Editor UI commonly renders the title on one line.

- Observation: The editor bridge reads up to 64 MiB before `route` checks `Origin` and the bearer token, and it creates one unbounded thread per accepted socket with a 300-second idle timeout.
  Evidence: `serve_connection` calls `read_request` first, `read_request` reads the body, and `start` spawns an `editor-tools-conn` thread for every incoming connection.

- Observation: Several security-sensitive workflows still reference mutable major-version tags while the publisher job has release and npm-trusted-publishing permissions.
  Evidence: `actions/setup-python@v7`, `actions/upload-artifact@v7`, and `actions/download-artifact@v8` remain in release and fuzz workflows.

- Observation: The repository already chose to keep client MCP servers available in Read-only and Plan, with normal approvals as the boundary.
  Evidence: `.agents/plans/read-only-and-plan-modes.md` records that decision, and README already states that Muse's `--disable-write --disable-shell` flags do not cover MCP tools.

- Observation: The default Unix Muse lookup checked that an absolute PATH entry existed but then returned the bare name `muse`, allowing the eventual process lookup to consult relative PATH entries.
  Evidence: `muse_cli` returned `"muse"` even after finding an absolute candidate.

- Observation: The MSP reader used `read_until` without a frame limit, and automatic exit diagnostics copied raw host stderr into adapter logs.
  Evidence: `reader_loop` buffered an unbounded line, and `ExitClassification::support_lines` emitted every captured stderr line.

- Observation: The deny fallback treated every decision not starting with `approv` as a rejection, including a future host decision named `allow`.
  Evidence: `fallback_deny` selected the first choice whose decision failed the approval prefix test.


## Decision Log


- Decision: Keep client MCP servers available in Read-only and Plan, but document that these modes do not sandbox client MCP tools and should not be treated as a guarantee that no external tool can write or execute.
  Rationale: This is the repository's previously recorded design. Removing editor MCP servers would silently remove user-configured capabilities, and automatically implying sandboxing of external processes would be inaccurate. A visible limitation is preferable to a hidden semantic change.
  Date/Author: 2026-10-29, Codex.

- Decision: Treat only ACP `text` blocks as typed user instructions for auto-review. Name attached resources in the trusted section without copying their contents.
  Rationale: A filename or URI can help the reviewer understand context, but file contents are attacker-controlled workspace data. Their absence from the trusted section prevents a planted attachment from claiming user authorization.
  Date/Author: 2026-10-29, Codex.

- Decision: Render each untrusted transcript entry as one bullet line and show line breaks and control characters as visible escapes.
  Rationale: A one-line bullet cannot create a new Markdown heading. Visible escapes preserve diagnostic meaning without allowing structural injection.
  Date/Author: 2026-10-29, Codex.

- Decision: Show hidden and structural characters in titles as escapes instead of deleting them.
  Rationale: Deletion can make two different commands look identical and can conceal what was approved. Escaping preserves the user's ability to inspect the complete effect.
  Date/Author: 2026-10-29, Codex.

- Decision: Limit the editor bridge to 32 concurrently served connections and reduce the idle read timeout from 300 to 60 seconds, while closing excess connections without spawning a thread for them.
  Rationale: Muse normally needs only a small number of keep-alive connections. A fixed cap prevents a local unauthenticated process from creating unbounded threads and idle sockets, and 60 seconds leaves ample time for normal reuse.
  Date/Author: 2026-10-29, Codex.

- Decision: Parse and validate request headers before reading a body. A request with an `Origin`, an unknown bearer token, or an oversized declared body receives its response from the headers and closes.
  Rationale: Authentication cannot trust content that has not arrived yet. Rejecting before allocation prevents unauthorized memory consumption.
  Date/Author: 2026-10-29, Codex.

- Decision: Warn when the host reports effective mode `allowAll`, but preserve the host-selected mode.
  Rationale: The adapter does not own the host default and changing it implicitly would surprise users. A warning makes the security consequence visible, and `MUSE_APPROVAL_MODE=promptUnmatched` remains an explicit override.
  Date/Author: 2026-10-29, Codex.

- Decision: Defer unrelated protocol features, crates.io publication, and the low findings that require separate redesigns, such as path-confinement TOCTOU and signed release provenance.
  Rationale: This batch should remain reviewable and preserve the ACP and release contracts. The deferred findings remain tracked by #441, #375, and the protocol issues.
  Date/Author: 2026-10-29, Codex.

- Decision: Resolve the Unix Muse executable to a full path under an absolute PATH entry or the installer directory, never return a bare command name for process search.
  Rationale: Full paths preserve symlinked installations while refusing current-directory execution through empty or relative PATH entries.
  Date/Author: 2026-10-29, Codex.

- Decision: Cap host stdout frames at 16 MiB and close the MSP connection when a frame exceeds that bound.
  Rationale: A malformed host must not be able to allocate without limit. Closing is safer than attempting to resynchronize after an oversized newline-free stream.
  Date/Author: 2026-10-29, Codex.

- Decision: Automatic stderr logs include host stderr byte counts, reviewer reply lengths, and stable decision metadata, but not arbitrary host or reviewer text.
  Rationale: Shell diagnostics and model replies can contain credentials or workspace content. The editor transcript still gives the user actionable reviewer failure text, while routine logs remain safe to share.
  Date/Author: 2026-10-29, Codex.


## Outcomes & Retrospective


The delivered change closes the six targeted issues and the directly related low findings from #441. Auto-review now separates typed instructions from attached content, escapes transcript evidence, rejects evidence-free JSON allows, and keeps arbitrary reviewer text out of stderr. Permission and synthetic tool-card titles show hidden characters as escapes. The editor bridge authenticates from headers before body allocation, enforces body and connection bounds, and uses a scoped permit for every connection thread. Release/fuzz actions are pinned, and the inherited `allowAll` posture is visible in logs and documentation. Read-only and Plan retain their existing MCP policy, now with an explicit warning that those external tools are not sandboxed.

The full workspace suite passed, including 442 adapter integration tests and the `muse-msp-client` process tests. Clippy with warnings denied, formatting, selftest, Node launcher tests, 43 Python release-script tests with 6 platform skips, shell syntax, and the npm smoke test also passed. The Node launcher test required running outside the filesystem sandbox because the sandbox blocked execution of its copied Node fixture with `EPERM`; the repository code was unchanged by that environment workaround.

Issue #441 remains open for findings not addressed here: TOCTOU-safe path confinement (L6), signed release/build provenance (I1), TLS 1.3 negotiation in the Windows installer (I2), bearer-token persistence in session logs (I3), and broader architectural review of prompt injection. Issues #316, #317, #318, #396, #397, and #375 remain separate feature/release work.


## Context and Orientation


`muse-acp` is a Rust binary that connects an editor speaking ACP (Agent Client Protocol) to a Muse host process speaking MSP (Muse Session Protocol). ACP messages arrive as newline-delimited JSON-RPC. The adapter launches `muse serve`, forwards user prompts, maps host approvals into editor permission requests, and streams tool and message updates back to the editor.

Auto-review is an optional per-session feature. When enabled, the adapter launches a separate memory-only Muse host and asks a reviewer model to return an `ALLOW` or `DENY` verdict for each permission request. The reviewer receives a fixed policy, user instructions, recent transcript evidence, and the proposed action. `src/reviewer.rs` owns the reviewer prompt and reply parsing. `src/main.rs` stores each session's prompt and evidence history and decides when to call the reviewer.

The editor MCP bridge is an HTTP server on loopback in `src/bridge.rs`. MCP (Model Context Protocol) is the protocol by which the model discovers and calls tools. For ACP v1 editor file and terminal capabilities, the bridge exposes tools such as `read_text_file`, `write_text_file`, and `run_in_terminal` to the Muse model. Each ACP session gets a random bearer token; the HTTP server uses that token to identify the session. `Origin` is rejected because browsers send it while Muse does not.

Permission titles are the short labels editors show above approval controls. `approval_title_kind` in `src/main.rs` converts host approval facts into these labels. `SessionFold::tool_title` in `src/fold.rs` produces equivalent labels for ordinary tool cards. Both can receive model- or workspace-controlled text.

GitHub issue #434 describes prompt injection through attached resource text and raw newlines in transcript evidence. Issue #435 describes hidden characters in permission titles. Issue #436 describes unauthenticated body buffering and unbounded bridge connections. Issue #437 describes mutable GitHub Actions tags. Issue #439 asks whether Read-only and Plan should drop MCP servers. Issue #440 asks for a warning and documentation when the host inherits `allowAll`. Issue #441 bundles lower-severity hardening items; this plan handles its connection-cap item L4.

The test suite uses `tests/fixtures/fake_serve.py` as a scripted Muse host. `tests/acp_serve.rs` launches the real adapter against that fixture and inspects ACP frames, fake-host frames, and stderr. Rust module tests cover prompt construction, title escaping, bridge parsing, and HTTP routing.


## Plan of Work


First, harden auto-review. In `src/reviewer.rs`, add a public function that parses an ACP content array and returns only typed `text` blocks, representing resource and resource-link blocks by URI without copying their contents. Add a private evidence renderer that prefixes each transcript entry with `- ` and escapes carriage returns, newlines, tabs, and other control characters. Use that renderer in `build_prompt`. Add unit tests proving an attached README cannot contribute trusted authorization and a fake `# Trusted user instructions` heading remains inside one escaped bullet. In `src/main.rs`, change the prompt-history call to store `reviewer::trusted_prompt_text(&acp_content)` rather than the raw content array.

Second, make titles visually complete. Add a function in `src/acp.rs` that scans Unicode characters and escapes line breaks, tab, controls, Unicode bidi embeddings and isolates, zero-width format characters, and the byte-order mark. Normal Unicode letters remain unchanged. Add unit tests for commands with hidden newlines, terminal escape sequences, and bidi overrides. Apply it to the final title from `approval_title_kind` and to the final tool-card title from `SessionFold::tool_title`. Keep the machine-readable raw input and `_meta` unchanged.

Third, restructure editor bridge request admission. In `src/bridge.rs`, split request parsing into two phases: parse the request line and headers, then decide whether to read a body. Reuse one authorization helper from the HTTP router so `Origin`, route, bearer token, method, and header size checks cannot disagree. A rejected request has an empty body and closes the connection; `route` still emits the appropriate 401, 403, or 405 status. For an admitted request, enforce the existing 64 MiB body cap before allocation for `Content-Length` and during chunk reads. Add an active-connection counter guarded by a scoped permit object. Set the maximum to 32 and idle timeout to 60 seconds. If the listener cannot acquire a permit, close the newly accepted stream immediately without spawning another thread. Add tests that an unadmitted head does not read its body, valid and invalid token/origin combinations are distinguished, the declared oversize body is rejected before allocation, and a local counter with a small limit refuses extra permits and releases permits on drop.

Fourth, pin actions. In `.github/workflows/fuzz.yml` and `.github/workflows/release.yml`, replace `actions/setup-python@v7`, `actions/upload-artifact@v7`, and `actions/download-artifact@v8` with the full commit SHAs used by the audited fix, preserving version comments. Confirm no mutable `uses: name@vN` references remain in those two files.

Fifth, document the restrictive-mode boundary. In `README.md`, make the Read-only and Plan section explicit that `--disable-write --disable-shell` covers Muse's own file and shell tools, while client-supplied MCP servers remain available and may perform writes or execution under the host approval policy. In `src/acp.rs`, change the Mode option description so it does not imply all MCP tools are disabled. Include guidance to use `MUSE_APPROVAL_MODE=promptUnmatched` or `denyUnmatched` for harness deployments that need an approval boundary.

Sixth, expose inherited approval risk. In `src/main.rs`, add a small function that logs a warning when an effective host approval mode is `allowAll`, naming the session and the safer `MUSE_APPROVAL_MODE=promptUnmatched` override. Call it for new and resumed sessions after the effective host mode is known. In `README.md`, add an approval-safety paragraph explaining that `allowAll` bypasses both editor permission prompts and auto-review, and should not be used for untrusted harnesses unless full host access is intended. Extend the existing `host_default_mode_is_reflected` black-box test to require the warning.

Seventh, close the directly related low findings. In `src/msp.rs`, resolve the Unix `muse` executable to a full path from an absolute PATH entry or the installer directory, and never return a bare command name that would trigger a relative process search. In `crates/muse-msp-client/src/client.rs`, read each host stdout line through a 16 MiB bound and close the connection with a typed diagnostic when it is exceeded. In `src/acp.rs`, make `fallback_deny` select only explicitly denied, rejected, aborted, cancelled, or timed-out decisions, returning `None` for unknown vocabulary. In `src/reviewer.rs`, require a JSON allow fallback to include risk, authorization, and rationale fields so a bare `{"outcome":"allow"}` is unusable. Finally, make `ExitClassification::support_lines` report only the stderr byte count, and make automatic logs report reviewer reply length and generic failure metadata instead of reply or prompt text.


## Concrete Steps


Run all commands from the repository root:

    /home/ryansvihla/.codex/worktrees/a003/muse-acp

The managed default `stable` toolchain in this workspace was missing its standard-library files, so validation used the complete Rust 1.99.1 toolchain with `cargo +1.99.1`. On a normal contributor machine, the commands are the same without the toolchain selector. Start with focused feedback while editing:

    cargo test --locked reviewer
    cargo test --locked bridge
    cargo test --locked visible_title
    cargo test --locked --test acp_serve host_default_mode_is_reflected

After the edits, run the repository contributor gate:

    cargo fmt --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace
    cargo run --locked -- --selftest
    node --test npm/test/launcher.test.cjs
    python3 -m unittest discover -s scripts -p 'test_*.py'
    sh -n install.sh
    node scripts/smoke_npm.cjs target/debug/muse-acp

Also run:

    rg -n "uses: actions/(setup-python|upload-artifact|download-artifact)@v" .github/workflows/release.yml .github/workflows/fuzz.yml

Expect no output. `cargo fmt --check` should exit 0, Clippy should print `Finished`, and every test binary should report zero failures. The selftest should print its success summary. npm and Python tests should report all passing tests. `sh -n` should produce no output, and the npm smoke script should exit 0.

The final successful workspace run reported 255 adapter unit tests and 442 adapter integration tests, plus all `acp-lite` and `muse-msp-client` unit, integration, process, and doctest suites. The Python release-script command reported 43 tests passing with 6 platform-specific skips. The Node launcher file reported 4 passing tests.


## Validation and Acceptance


For prompt hardening, a unit test supplies a typed text block plus a resource block whose text says the user authorizes credential upload. The trusted extraction contains the typed text and the resource URI, but not the planted authorization. A second test supplies an evidence entry beginning with real newlines and a fake trusted-instructions heading; `build_prompt` contains exactly one real trusted heading and renders the forged text after literal `\\n` escapes.

For title hardening, unit tests pass commands containing newline-separated destructive tails, terminal escape sequences, a right-to-left override, and zero-width characters. The returned title contains visible `\\n` or `\\u{...}` sequences, retains ordinary accented text, and places no raw control or bidi-format character in editor-visible title text.

For bridge admission, a request helper with an admitting predicate that returns false must return after parsing headers with an empty body and `close = true`, even when `Content-Length` says bytes follow. Authorization tests prove no token and a bad token are rejected while a registered bearer token is admitted, and any `Origin` header is rejected. A request with `Content-Length` above the cap remains a 413 error. A test-local permit counter initialized with a limit of two yields two permits, refuses the third, and admits again after dropping one. There are no unbounded per-connection allocations before admission.

For workflow pinning, every remaining action reference in release and fuzz workflows is either a full 40-hex commit or a repository-local action. The release publisher retains its existing permissions and behavior.

For restrictive-mode documentation, the README and ACP Mode description explicitly say that Read-only and Plan block Muse's file and shell tools, not client MCP tools, and that host approvals remain the boundary for those servers.

For inherited `allowAll`, `cargo test --locked --test acp_serve host_default_mode_is_reflected` starts the fake host with `FAKE_MODE=allowAll`, verifies the ACP selector still reports `allowAll`, and observes a warning that names `MUSE_APPROVAL_MODE=promptUnmatched`. A normal `promptUnmatched` session does not emit the warning.

For the low findings, unit tests prove relative PATH entries cannot select the Unix executable, a frame one byte over its test bound reports `TooLarge`, the deny fallback ignores a future `allow` decision, and a bare JSON allow has no assessment. Exit and black-box tests prove automatic logs and the support bundle report `stderr-bytes` with `contents=omitted`, never the fake host's diagnostic text, while the editor transcript still carries the bounded reviewer reason.


## Idempotence and Recovery


All source, test, workflow, and documentation edits are additive or replace equivalent behavior; rerunning the commands is safe. Tests use temporary directories and fake hosts and do not modify the developer's Muse settings. If a focused test fails, fix the implementation and rerun only that test before restarting the full gate. No destructive migration or external publication is part of this plan.


## Artifacts and Notes


The referenced fork commits supplied the intended behavior:

    3a7dae1  harden auto-review prompt and permission titles
    5f9f89a  read no request body before auth checks
    c5974f3  pin remaining actions to commit SHAs

The local implementation additionally adds a bridge connection cap, tool-card title escaping, the Read-only MCP limitation, and the `allowAll` warning requested by the newer issues.

Final validation also covered the #441 hardening added during implementation: no relative PATH lookup for the default Unix Muse command, a 16 MiB MSP stdout frame bound, explicit reject-only permission fallback, strict JSON allow evidence, and metadata-only automatic diagnostics.


## Interfaces and Dependencies


In `src/reviewer.rs`, expose:

    pub fn trusted_prompt_text(acp_content: &str) -> String

It accepts the JSON text of an ACP content-block array and returns only typed user text plus non-content resource markers. It does not fail the prompt when the array is malformed; it returns an empty trusted string so the reviewer fails more cautiously.

In `src/acp.rs`, expose:

    pub fn visible_title(text: &str) -> String

It returns display-safe one-line text where control and hidden format characters are literal escape sequences. It does not truncate or alter ordinary printable Unicode.

In `src/bridge.rs`, keep the existing public bridge API unchanged. Internally introduce header-based authorization and a scoped connection permit. The cap is a compile-time constant; the permit decrements its counter on every return path through `Drop`.

No new external crate is required. The implementation uses the existing standard-library networking, atomic, Unicode, test, and JSON facilities and the existing fake MSP host.


## Change Notes


2026-10-29: Created the plan after reading all open issue bodies and the referenced fork fixes. Scoped the batch to the actionable security issues and recorded why the existing client-MCP policy is retained with clearer documentation.

2026-10-29: Added low findings L2, L3, L5, L7, and I4 from #441 because their fixes stayed within the same security boundaries. Recorded the choice to keep raw stderr and unusable reviewer text out of automatic logs, and documented the managed-toolchain test workaround.
