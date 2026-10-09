# Changelog

## 0.11.1

- Fix Auto-review against a real Muse host. The reviewer session started its
  turn with the pre-`input` `prompt` field, which Muse 1.4.x rejects with
  "invalid turn/start params: missing field `input`". Every approval then
  failed closed as `reviewer turn failed`, which Muse surfaced to the editor
  as "approval aborted", on every approval and in every client. The reviewer
  now submits `turn/start` with `input`, the shape the rest of the adapter
  already used and the MSP schema requires. The fake MSP host now rejects a
  `turn/start` without `input`, and the live-host suite covers the adapter's
  own reviewer against a real `muse serve`.
- Consume `turn/foregroundCompleted` instead of falling through to the
  unhandled-notification diagnostic. Muse's newer hosts emit it when a turn's
  foreground work is done but named background reminder checks still hold the
  turn open; it is explicitly non-terminal, so the ACP prompt stays open until
  `turn/completed` / `turn/unqueued`. The adapter now logs the turn, session,
  and blocking reminder-agent ids.

## 0.11.0

- Deny a hung auto-review past a deadline. A reviewer turn that never ends no
  longer stalls the approval indefinitely: after 90 seconds (Codex's guardian
  review timeout, overridable with `MUSE_REVIEW_TIMEOUT_MS`) the adapter logs
  the timeout, denies with a reviewer-unavailable rationale, resets the
  reviewer session, and starts the next queued review.

- Name reminder-child cards after their agent. A `reminderChild` item used to
  render as a generic card showing only "Reminder child session", so parallel
  reminders were indistinguishable. The card title now names the reminder
  agent and generation, falling back to the task id and then the server
  summary as before, with the task, child session, and log path in the card
  body.
- Support exact-turn steering on ACP v1. `_session/steering` now works on both
  protocol versions: the v1 `initialize` response advertises
  `steering.supported`, v1 connections are accepted instead of failing with
  `-32601`, and the synthetic user echo uses the v1 `user_message_chunk` shape
  without a `state_update`. Previously steering was reachable only from ACP v2
  clients, so ACP v1 clients such as Mjolnir could not steer a Muse turn.
- Fix Windows sessions with extra workspace roots. Muse 1.4.3 requires
  `turn/start workspaceRoots` entries in the verbatim `\\?\C:\...` canonical
  form, so the adapter keeps that form instead of stripping it back to
  `C:\...`, and `session/start` now names the same folder in `workspaceRoot`.
  Previously a session that re-attached after a mode change, resume, or fork
  could fail its next turn with "expected a canonical path".
- Classify Muse 1.4.4 as a tested host. A live `muse serve --provider echo`
  1.4.4 handshake reports the new stable fingerprint, and the binary's own
  `muse schema` export is identical to the vendored bundle, so startup logs
  `status=tested` instead of `status=unknown`. `session/delete` stays
  removed; the version-gated features are unchanged (no delete from 1.4.3
  on, workspace roots and host-computed cost stay on). The live-host suite
  now runs against 1.4.4 on Linux, and the newest macOS arm64 pin moves to
  1.4.4.
- Re-pin the vendored Muse SDK conformance corpus to `537cc8d`. It publishes
  the Muse 1.4.4 stable surface, adds the `userinput-interrupt-round-trip`
  transcript (51 scenarios), and drops `session/delete` and
  `session/deleteCompleted` from the schema; the event-compatibility matrix
  classifies the new `turn/foregroundCompleted` and `userInput/engaged`
  notifications as intentionally ignored.
- Run the live-host suite against Muse 1.4.3 (`1.4.3-R5018.1`) on Linux and
  macOS arm64, and classify its schema fingerprint as tested. On 1.4.3,
  Read-only and Plan sessions no longer get write or shell tools at all,
  instead of having the call denied; writes stay blocked either way.
- Stop offering session delete on Muse 1.4.3. That release removed
  `session/delete` and answers it with "method not found", so the adapter
  no longer advertises delete there and refuses the request itself, keeping
  the session. Delete still works on Muse 1.4.1 and 1.4.2.
- Publish a native Windows arm64 build (`aarch64-pc-windows-msvc`). Releases
  now carry its ZIP and checksum, the npm package bundles it for
  `win32/arm64`, and `install.ps1` installs it on arm64 machines, including
  from an emulated x64 PowerShell, which reports `AMD64` in its environment.
  Previously the npm launcher and the installer refused Windows arm64.

## 0.10.0

- Add session-scoped **Auto-review**. A new default-off `auto_review`
  selector sends every permission request to a reviewer agent instead of the
  editor. The reviewer runs on a memory-only, read-only Muse host, follows a
  Codex-style safety policy, and answers with a risk level, user-authorization
  score, allow/deny outcome, and rationale; failures deny. The selector never
  changes the host's approval mode, and each decision is logged with its
  rationale. See the README Auto-review section and
  [docs/auto-review.md](docs/auto-review.md).
- Add **Read-only** and **Plan** session modes. The editor's Mode selector
  now offers Default, Read-only, and Plan. Read-only and Plan sessions run on
  a second `muse serve` started with `--disable-write --disable-shell`, so
  Muse itself refuses file writes and shell commands. Plan also tells Muse to
  plan rather than implement, and a bare `/plan` switches to it without
  starting a turn. Changing an open session's mode moves it between the two
  hosts and is refused while a turn or background work runs on the host it
  leaves. The mode is remembered, so a reloaded session comes back in it.
- When the Muse host restarts, a session that had not run a turn yet starts
  again under its own id. Muse saves a session only with its first turn, so
  such a session previously failed to reconnect.
- The approval policy moves to its own **Approval Mode** selector
  (`approval_mode`). Approval mode ids sent to `mode` or `session/set_mode`
  still set it.
- When Muse declines `/compact`, for example on a session too short to
  compact, the prompt now ends normally with a note naming Muse's reason.
  Previously a `compaction_unavailable` or `missing_run` answer surfaced as an
  internal `-32603` error, and a no-op compaction ended without a word. On
  ACP v2 the session stays running if another turn still is.
- Classify Muse 1.4.2 as a tested host. Its stable schema only adds to
  1.4.1's, and the live-host suite passes against it, so startup logs now
  report `status=tested` instead of `status=unknown`.
- Re-pin the vendored Muse SDK conformance corpus to `bb44be3`. It publishes
  the Muse 1.4.2 stable surface, including `session/delete`, workspace roots,
  session-list filters, per-model reasoning tiers, cost totals, and feedback.
  Startup now logs a `host-features` line saying whether the host's version
  offers session delete, workspace roots, and host-computed session cost.
- ACP `session/delete` is advertised and backed by MSP `session/delete` on
  Muse 1.4.1+ hosts with durable session logs. The editor's answer arrives
  with the host's terminal event, a completed delete disappears from
  `session/list`, and a refusal keeps the session and explains why. Muse only
  deletes sessions it can prove the running host owns, so a session from an
  earlier editor run may be kept; the host's own reason and physical-change
  evidence are shown either way. Deleting a session that never existed
  succeeds silently where the host's listing filters can prove the absence
  (Muse 1.4.2+); on hosts without them the refusal is reported rather than
  pretended away.
- ACP `additionalDirectories` now reach Muse itself as MSP `workspaceRoots`
  on 1.4.1+ hosts, so Muse's own tools work in the extra folders. Roots are
  validated and canonicalized up front, duplicates collapse, and a load,
  resume, fork, or host re-attach replaces the host's sticky root set on the
  next turn. Older hosts keep adapter-side confinement and log the limit.
- The reasoning selector offers exactly the selected model's tiers, with
  Muse's descriptions, on hosts that publish per-model variants, and follows
  a model change; a model that cannot describe its tiers keeps the fixed
  list. `config_option_update` now carries the complete `configOptions` list
  as ACP requires, and host-reported model, reasoning, and approval changes
  refresh it. AIR clients see the current model's default when the host set
  no session-level recommendation.
- Session cost now comes from Muse's own `session/tokenUsage.cost` on 1.4.2+
  hosts, including whether it is partial, instead of the adapter's catalog
  estimate; hosts without it keep the labeled estimate. Host cache read and
  write totals ride `_meta.museCumulative`, and the estimate's provenance
  moves into `cost._meta` where ACP allows extensions.
- Add `/feedback`. On hosts that grant the feedback capability it sends
  `feedback/submit` and shows the host's receipt; with form elicitation, one
  form collects the classification, note, and explicit consent for local
  tracing and the session record, both defaulting off. Without forms the
  classified `/feedback <bug|bad|good|other> <note>` syntax submits directly.
- A session that Muse unloads, for example when its host shuts down or the
  session sits idle, now stays in the editor's session list. With Muse's live
  listing stream it used to disappear until it changed again, although Muse
  keeps it on disk and can reload it.
- With a saved `:auto-review` profile, files Muse creates while it runs, such
  as the credential from a first login or a first workspace trust, now land
  in your Muse configuration instead of the temporary settings view, where
  they were lost when it was removed. A login completed in a terminal while
  the agent runs now reaches it: right away, or on Windows without symbolic
  links at the next prompt, new session, or editor authentication.
- Fix startup on Windows for a saved `:auto-review` permission profile
  without Developer Mode or administrator rights. The private settings view
  now links folders with directory junctions and, when symbolic links are not
  allowed, files with hard links. A file Muse replaces through a hard link,
  such as a refreshed credential, is moved back over the original when the
  host exits, or at the next launch if the editor terminated the agent.
  Previously every request failed with `A required privilege is not held by
  the client (os error 1314)`.
- If the settings view cannot be built, start `muse serve` with the saved
  settings instead of failing every request, and name the cause in the error
  shown when Muse then refuses the profile. Another app's configuration entry
  that cannot be linked is now left out of the view instead of failing it.

## 0.9.0

- When Muse is not installed, report it as ACP's auth-required error with a
  clear "Muse Code is not installed" message, and name the terminal auth
  method **Set up Muse Code**. `muse-acp login` then offers to
  run Muse's official installer, asking first, before `muse login`.
  Previously requests failed with a generic `-32603` startup error.
- When `muse` is not on `PATH`, look for it where Muse's installer puts it
  (`MUSE_INSTALL_DIR`, `~/.local/bin`, or `%LOCALAPPDATA%\Programs\muse` on
  Windows). Editors often launch agents without that directory on `PATH`.

## 0.8.1

- Classify Muse 1.4.1 as a tested host. Its stable schema adds to the 1.3.0
  surface without removing or requiring anything, and the live-host smoke
  test passes, so startup logs now report `status=tested` instead of
  `status=unknown`.

## 0.8.0

- Forward the MCP servers an editor attaches to a session, such as Zed's
  context servers or the JetBrains IDE server, to Muse 1.3.0 and newer through
  typed session MCP configuration. Stdio and HTTP servers are supported, and
  ACP HTTP MCP support is advertised when the host grants `sessionMcp`. Tool
  calls go through Muse approvals. Servers are loaded as optional, so one that
  cannot start does not break the session. Loading a session re-sends the
  servers, and so does a host restart. SSE servers, malformed entries, and
  hosts without the grant are logged by server name only. Previously every
  client MCP server was dropped.
- Advertise an ACP terminal auth method, `muse-login`, so editors can offer
  login when Muse is not authenticated. It runs the new `muse-acp login`
  command, which hands the terminal to `muse login` (a browser device-code
  approval) and exits with its status. The adapter still never handles
  credentials.
- Complete the ACP handshake even when `muse serve` cannot start (for example,
  when Muse is not installed). Later requests return the startup diagnostic
  instead of the agent exiting, so the editor can show what to fix.
- Fail `session/new` and `session/load` with ACP's auth-required error when
  Muse has no credential, so editors such as Zed open their login screen
  before the first prompt. This reads Muse's experimental `account/read`,
  which reports only which kind of credential is in effect; the adapter now
  opts into the experimental MSP API for it. If the check is unavailable, the
  first prompt reports the error as before.
- On Windows, find the `muse.cmd` launcher that the Muse installer puts on
  `PATH`. Previously Muse was never found there unless `MUSE_CLI` was set.
- On Windows, `muse-acp install` records the full path to `muse-acp.exe` in
  Zed's settings, because the PowerShell installer does not change `PATH`.
- Make goal turns stoppable. A `/goal` command that starts a goal turn now
  keeps the editor prompt open until that turn ends, so the editor shows it
  running and Stop interrupts it (the host then pauses the goal). Stop and
  closing a session also reach a turn the host started on its own, such as a
  goal continuation.
- `/goal stop`, `/goal cancel`, and other one-word control words now fail with
  guidance instead of becoming a goal whose objective is that word.
- `/goal`, `/rename`, and `/workflow-child` accept @-mentions, which join the
  command as `[@name](uri)` links; images still fail the command. Previously
  a mention turned the command into an ordinary prompt.
- `/goal`, `/rename`, and `/workflow-child` now work while a tool permission
  prompt is open, so a goal can be paused or cleared while its turn waits.
- Fix `muse-acp install` and `uninstall` on Windows: they now edit
  `%APPDATA%\Zed\settings.json`, where Zed reads its settings, instead of
  `~/.config/zed/settings.json`.

## 0.7.0

- Add adapter slash commands for Muse host controls: `/goal` sets, edits,
  pauses, resumes, or clears the session goal; `/rename <name>` renames the
  session; and `/workflow-child skip|retry <childId>` controls one child of a
  running workflow using its current attempt. Workflow cards now show child
  ids, and a bare `/workflow-child` lists the children you can control.
- Stop relaunching a Muse host that crashes right after every restart: at
  most five automatic restarts are allowed in any ten minutes, with growing
  backoff, before the adapter stops with an explicit message.
- Re-pin the vendored Muse SDK conformance corpus to `a7c10c5`. Its manifest
  matches the live-validated 1.3.0-R3401.1 host surface, so it now classifies
  as tested. Every emitted host request is validated against the schema's
  method index.
- Document the negotiated protocol extensions in the README and reconcile the
  roadmap's Muse 1.3.0 disposition table with landed support.

## 0.6.2

- Rewrite the README for new users, covering quick start, editor setup,
  configuration, and diagnostics, and align the contributing, releasing, roadmap,
  security, and protocol guides with current adapter behavior. The
  refreshed README ships in the release archives and the npm package.

## 0.6.1

- Start Muse hosts with human approvals when the saved profile is
  `:auto-review`, avoiding the unavailable-reviewer session failure without
  changing the user's Muse settings or editor launcher.
- Exercise permission-profile startup, approvals, restart, and cleanup on
  Windows as well as Unix, and fix the stdout saturation tests that blocked
  macOS and Windows release builds.
- Distribute the native adapter through `@brokkai/muse-acp` on npm, including
  all five supported platform binaries.

## 0.6.0

- Stop overriding Muse's reasoning default: sessions start at "Muse
  default" and send no per-turn tier until the user picks one, and the
  option is offered only while no standing default is in force.
- Gate leading-slash prompts on the skill catalog: fork sessions read
  skill/list, a failed re-read forgets a stale catalog, `/skill <name>`
  and `compact` always submit, and slash text naming no skill is sent as
  ordinary text.
- Settle orphaned in-flight prompts across durable host restarts,
  including legacy sessions and sessions the restarted host cannot
  re-attach.
- Withdraw stale client requests with `$/cancel_request` for resolved
  child-stream approvals and questions settled elsewhere.
- Harden the transcript gap walk: refill from the hole, stop on cursor
  cycles, and discard live twins of delivered pages.

## 0.5.0

- Add Muse Code 1.3.0 compatibility, native skills, session reasoning defaults,
  subscription usage observations, and negotiated stored-output access.
- Improve session listing, pagination, workspace filtering, durable names,
  metadata, status, and attention updates.
- Restore transcript history, usage, and active tasks across resume and host
  restarts; prevent duplicate updates during snapshot and gap replay.
- Add native subagent controls, workflow and background-task cancellation,
  queued-prompt cancellation, and host-confirmed file-change reports.
- Improve approval rule previews, rejection feedback, question clarification,
  multiple-selection forms, and legacy model selection.
- Harden workspace resource confinement, host shutdown, pipe timeouts, tool
  failure reporting, and authentication diagnostics.
- Support native Windows resource paths and terminate Windows host process
  trees on timeout; fix Windows integration test paths and synchronization.
- Add a PowerShell installer and document platform support, the Muse 1.3.0
  event compatibility matrix, and experimental account authentication.

## 0.4.5

- Recover draft releases through authenticated listing and read uploaded assets
  back by release ID when GitHub hides drafts from the tag endpoint.

## 0.4.4

- Surface actionable Muse authentication failures in ACP sessions.
- Decode local Windows file URIs with drive letters correctly.
- Ignore local worktree and Brokk files.
- Validate all release platforms and actual Actions publisher permissions before
  publication; verify complete artifacts and safely resume partial uploads.
