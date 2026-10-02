# Read-only and plan session modes on a second, read-only Muse host

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It is maintained in accordance with `.agents/PLANS.md` at the repository root.


## Purpose / Big Picture


GitHub issue #159 asks for session modes that guarantee the agent changes nothing: **Read-only** (the agent may read and answer, but cannot write files or run shell commands) and **Plan** (read-only, and the model is told to plan rather than implement; leaving it takes an explicit mode change). Until now the editor's mode selector only chose Muse's approval policy, which never stops writes inside the workspace: Muse allows those without asking.

After this change the editor's **Mode** selector offers Default, Read-only, and Plan. Read-only and Plan sessions run on a second `muse serve` that `muse-acp` launches with `--disable-write --disable-shell`, so Muse itself refuses writes and shell commands. The approval policy moves to its own **Approval Mode** selector. A bare `/plan` switches to Plan without starting a turn. The mode is remembered, so a session loaded later comes back in the same mode.

To see it working, run `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback read_only` with Muse installed: a Plan session's `write_file` fails with Muse's "tool policy denied filesystem write", and after switching back to Default the same request writes the file.


## Progress


- [x] (2026-10-02 14:00Z) Probed live Muse 1.4.2 (see Surprises & Discoveries) and settled the design.
- [x] (2026-10-02 15:00Z) Milestone 1: `src/hosts.rs` router; `main.rs` uses it everywhere it used `MspHost`; events carry their host kind and generation; a crashed read-only host restarts alone. The full suite and the live suite passed unchanged.
- [x] (2026-10-02 15:40Z) Milestone 2: session modes in `src/acp.rs`; `mode` and legacy `modes` carry them; `approval_mode` carries the approval policy, with old ids on `mode` still accepted; `src/modes.rs` persists non-default modes.
- [x] (2026-10-02 16:10Z) Milestone 3: `switch_session_mode` moves sessions between hosts; load, resume, and fork open on the right host; plan turns carry the instruction; bare `/plan` switches.
- [x] (2026-10-02 16:40Z) Milestone 4: five fake-host tests in `tests/acp_serve.rs`, two live tests in `tests/live_loopback.rs` (15/15 pass on 1.3.0, 1.4.1, and 1.4.2), README, CHANGELOG.


## Surprises & Discoveries


- Observation: Muse allows `write_file`, `edit_file`, and read-only shell commands inside the workspace without any approval request, in every approval mode, on 1.3.0, 1.4.1, and 1.4.2. An adapter-side "deny writes" policy therefore never sees the writes it would have to deny.
  Evidence: loopback probes with `write_file src/lib.rs` under `onRequest` and `promptUnmatched` produced no `approval/requested` and wrote the file.

- Observation: `muse serve --disable-write --disable-shell` enforces read-only inside Muse.
  Evidence (1.4.2): `write_file` completed with `tool failed: tool policy denied filesystem write`; `printf a > shell.txt` created nothing; `read_file` worked.

- Observation: a session can be loaded by only one `muse serve` at a time, and Muse releases it only when that host shuts down. A second host's `session/resume` fails with `-32021 sessionInUse`, and neither waiting four minutes nor `view/unsubscribe` released it (no `session/closed` arrived).
  Evidence: `{"code":-32021,"data":{"kind":"sessionInUse",...},"message":"session ... is already in use"}` after 244 s, with and without `view/unsubscribe`.


- Observation: Muse writes a session to disk with its first turn. Moving a session that never ran a turn found nothing to resume on the new host (`-32020 sessionNotFound`) once the old host exited.
  Evidence: live test `read_only_mode_blocks_writes_until_switched_back` failed with "session ... was not found" before the fallback; `session/start` with the same `sessionId` on the new host fixed it.

- Observation: fake-host tests shared the developer's `~/.local/state`, and every fake host uses the session id `msp-sess-1`, so a test that switched to plan made a later test load on the read-only host.
  Evidence: `host_restart_settles_orphaned_turns_of_a_legacy_session_id` failed with "read-only Muse host started" in its log; each test spawn now sets its own `XDG_STATE_HOME`.


## Decision Log


- Decision: two shared hosts (the existing one, and a read-only one launched on first need), not one host per session.
  Rationale: the user's direction, and it keeps one Muse process for all default sessions. The competitor bex-co runs one host per session to switch modes; with two hosts, the cost of releasing a session is a restart of the host that holds it.
  Date/Author: 2026-10-02, Claude.

- Decision: switching an open session's mode restarts the host that holds it, re-attaching that host's other sessions through the existing durable-restart path, then resumes the session on the other host. The switch is refused while a turn is running on the source host, in this session or another.
  Rationale: host shutdown is the only way Muse releases a session (see Surprises). Requiring an idle host means the restart interrupts nothing; re-attaching is the same code that recovers from a crash.
  Date/Author: 2026-10-02, Claude.

- Decision: route MSP commands by their `sessionId` in a `Hosts` type with the same method names `main.rs` already calls, learning which process owns a session from the events each process emits and from the session it starts, resumes, or forks.
  Rationale: about 60 command call sites keep working unchanged, and subagent child sessions, which only appear in events, route correctly.
  Date/Author: 2026-10-02, Claude.

- Decision: the `mode` selector (category `mode`) and legacy `modes` carry `default`, `readOnly`, and `plan`; the approval policy becomes the `approval_mode` selector. `session/set_mode` and `set_config_option` for `mode` still accept the old approval ids and apply them to the approval policy.
  Rationale: issue #159 asks for the session modes on the ACP mode surface, as bex-co does. Accepting the old ids keeps editors that remember a previous approval selection working.
  Date/Author: 2026-10-02, Claude.

- Decision: Plan is the read-only host plus a planning instruction prepended to each turn; only an explicit mode change leaves it. A bare `/plan` switches to Plan and ends without a turn; `/plan <text>` keeps running Muse's plan skill in the current mode.
  Rationale: the issue asks only for a bare `/plan` to switch. Making `/plan <text>` switch too, as bex-co does, would change an existing command and could fail whenever another thread on the main host is mid-turn.
  Date/Author: 2026-10-02, Claude.

- Decision: when the moved session cannot be resumed on the new host because Muse never saved it (`sessionNotFound`), start it there under the same `sessionId`, workspace, approval mode, and MCP servers, and follow its view from the new head.
  Rationale: a session with no turns has no history to lose, and `session/start` accepts an explicit id that the old host no longer holds.
  Date/Author: 2026-10-02, Claude.

- Decision: persist each session's mode in `$XDG_STATE_HOME/muse-acp/session-modes.json` (Windows: `%LOCALAPPDATA%\muse-acp\session-modes.json`), keyed by Muse session id, and open loaded and resumed sessions on the matching host. A fork keeps its source's mode.
  Rationale: the issue requires the mode to survive load and resume or the README to say it does not; a read-only guarantee that silently lapses on reload would be worse than none. A fork is created on the host that holds its source, so it starts in that mode.
  Date/Author: 2026-10-02, Claude.

- Decision: client MCP servers still load in read-only sessions; the README states that read-only covers Muse's file and shell tools, while MCP tools still go through approvals.
  Rationale: `--disable-write` does not constrain external tools, and the adapter cannot either; dropping the editor's servers silently would surprise users more than an explicit limit.
  Date/Author: 2026-10-02, Claude.


## Outcomes & Retrospective


Read-only and Plan work as designed on every pinned Muse build: a write in either mode is refused by Muse and nothing reaches the workspace, switching back allows it, and the mode survives an adapter restart. The router kept the change to `main.rs` small: most of its command call sites did not change. The cost of the two-host design is visible only on a mode change: the host being left restarts, and the change waits for its turns to finish. Not covered: MCP tools in read-only sessions (still approval-gated), and modes are not shared with other Muse clients.


## Context and Orientation


`muse-acp` is one Rust binary. An editor launches it and speaks ACP (Agent Client Protocol, newline-delimited JSON-RPC) on stdin and stdout. The adapter launches `muse serve`, the "host", and speaks MSP (Muse Session Protocol) to it. `src/msp.rs` defines `MspHost`, one host process and its connection: `MspHost::launch` starts it, `command` sends a request and waits for the result, events arrive on a channel as `MspEvent` (`Notification`, `Request`, `Eof`). `src/main.rs` holds the event loop (`main`), the ACP request handler `handle_acp`, the MSP event handler `handle_msp`, and `restart_durable_host`, which relaunches a crashed host and re-attaches every session with `resume_session` and `reattach_view`. Sessions live in `AcpSession` records (`src/acp.rs`) keyed by ACP session id; `msp_sid` is the Muse session id, `mode_value` the approval selector's value. `acp::config_options` and `acp::session_modes` render the selectors. Host launch flags come from `MUSE_SERVE_ARGS` in `serve_command` (`src/msp.rs`).


## Plan of Work


Milestone 1 adds `src/hosts.rs` with `pub enum HostKind { Main, ReadOnly }` and `pub struct Hosts`. `Hosts` holds the main `Arc<MspHost>`, an optional read-only one, the launch settings, a sender for tagged events, and a map from Muse session id to `HostKind`. It exposes the methods `main.rs` uses on a host (`command`, `notify`, `mint_cmd`, `handshake`, `logged_out`, `refresh_config`, `shutdown`, `force_stop`), where `command` and `notify` read `sessionId` from the params and send to the owner (the main host when unknown), plus `command_on(kind, ...)`, `note_owner`, `owner`, `read_only_host()` (launching it on first use with `--disable-write --disable-shell` appended), and `replace(kind, host)`. `MspHost::launch` gains an `extra_args` parameter. `LoopMsg::Msp` carries the `HostKind`; the loop records the owner of every event's `sessionId` before handling it, and on `Eof` restarts only that process, re-attaching only the sessions it owned. `main.rs` changes `Arc<MspHost>` to `Arc<Hosts>` in its signatures.

Milestone 2 adds `SESSION_MODES` (`default` "Default", `readOnly` "Read-only", `plan` "Plan") to `src/acp.rs`, a `session_mode` field on `AcpSession`, renders the `mode` selector and legacy `modes` from it, and renders the approval selector as `approval_mode`. `set_config_option` and `set_mode` dispatch accordingly. A small `src/modes.rs` reads and writes the persisted map.

Milestone 3 adds `switch_mode` in `main.rs`: validate idleness, move hosts if the target needs a different host (restart the source with the session excluded, resume on the target, note the owner, re-attach the view), set the mode, persist, and send `current_mode_update` and `config_option_update`. `session/new` starts on the main host; `session/load` and `session/resume` read the persisted mode and resume on the matching host. Prompts in Plan get the planning instruction as a leading text part. A bare `/plan` calls `switch_mode` and ends the prompt.

Milestone 4 extends `tests/fixtures/fake_serve.py` to record its own launch arguments per process and to refuse a resume of a session another fake process holds, adds integration tests in `tests/acp_serve.rs`, adds live tests in `tests/live_loopback.rs`, and documents the modes.


## Concrete Steps


From the repository root: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo clippy --locked --all-targets --target x86_64-pc-windows-msvc -- -D warnings`, `cargo test --locked`, and `MUSE_ACP_LOOPBACK=1 cargo test --locked --test live_loopback`.


## Validation and Acceptance


In Plan or Read-only, a prompt that makes the model call `write_file` or a shell command changes nothing in the workspace, and the tool result says Muse refused it. Switching back to Default lets the same request write. A session reloaded after an adapter restart comes back in its mode. A mode switch while another thread's turn is running is refused with a message naming the reason. These are covered by the live tests named above and by fake-host tests for routing, restart, and refusal.


## Idempotence and Recovery


Mode switches are idempotent: switching to the current mode does nothing. A failed resume on the target host leaves the session on no host; the switch then reports the error and the next prompt's existing re-attach logic applies. The persisted mode file is rewritten whole on each change and ignored if unreadable.


## Artifacts and Notes


    write_file in a read-only session (Muse 1.4.2):
    item/completed toolCall visibleOutput "tool failed: tool policy denied filesystem write"

    second host resuming a loaded session:
    {"code":-32021,"data":{"kind":"sessionInUse"},"message":"session ... is already in use"}


## Interfaces and Dependencies


In `src/hosts.rs`:

    pub enum HostKind { Main, ReadOnly }
    pub struct Hosts { ... }
    impl Hosts {
        pub fn command(&self, method: &str, params_json: &str) -> Result<J, J>;
        pub fn command_on(&self, kind: HostKind, method: &str, params_json: &str) -> Result<J, J>;
        pub fn note_owner(&self, msp_sid: &str, kind: HostKind);
        pub fn owner(&self, msp_sid: &str) -> HostKind;
    }

No new dependencies.
