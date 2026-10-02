# Changelog

## Unreleased

- Classify Muse 1.4.2 as a tested host. Its stable schema only adds to
  1.4.1's, and the live-host suite passes against it, so startup logs now
  report `status=tested` instead of `status=unknown`.
- With a saved `:auto-review` profile, files Muse creates while it runs, such
  as the credential from a first login or a first workspace trust, now land
  in your Muse configuration instead of the temporary settings view, where
  they were lost when it was removed. A login completed in a terminal while
  the agent runs now reaches it: right away, or on Windows without symbolic
  links when the editor next authenticates.
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
