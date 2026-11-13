# Muse's native `muse acp` server (Muse 1.4.16+)

Agent context, not product documentation. Recorded 2026-11-13 while adopting
Muse 1.4.18.

## What changed

Muse 1.4.18 (`1.4.18-R8587.1`) ships `muse acp`, an ACP server built into the
Muse binary. Muse 1.4.15 (`1.4.15-R8339.1`) has no such subcommand, so it
arrived in 1.4.16, 1.4.17, or 1.4.18 (no 1.4.16 or 1.4.17 build was available
to narrow it down). An editor can now launch Muse directly as an ACP agent
without this adapter.

`muse acp --help` says the editor owns stdin/stdout, sandbox posture and
session durability are fixed per process, and approval mode is chosen per
session through the editor's mode selector. Flags: `--no-session-log`
(memory-only sessions, no load/resume/list), `-c KEY=VALUE`, `--preset`,
`--provider`, `--model`, `--contributor-mode`,
`--disable-request-user-input`, `--disable-sandbox`, `--sandbox-network`,
`--disable-write`, `--disable-shell`, and `--trust-workspace`.

## Observed ACP surface (echo provider, isolated HOME)

`initialize` with `protocolVersion: 1` answers:

    protocolVersion: 1
    agentInfo: {name: "muse", version: "1.4.18"}
    agentCapabilities:
      loadSession: true            (false with --no-session-log)
      mcpCapabilities: {http: true, sse: false}
      promptCapabilities: {image: true, embeddedContext: true, audio: false}
      sessionCapabilities: {additionalDirectories: {}, list: {}, resume: {}}
    authMethods: [{id: "muse-login", name: "Log in to Muse", description: device code}]

`session/new` returns a `mode` select config option and legacy `modes` whose
values are Muse approval modes (`allowAll`, `promptUnmatched`, `onRequest`,
`denyUnmatched`), then sends `available_commands_update` (compact plus skills).
A prompt streams `agent_message_chunk`, `usage_update` with
`_meta["dev.muse"]` (`contextSizeUnknown`, `pressure`), and ends with
`stopReason: end_turn`. Its extension namespace is `_meta["dev.muse"]`, not
this adapter's `_meta.muse`.

## Versus this adapter

Native `muse acp` covers ACP v1 basics. This adapter additionally offers: ACP
v2 (state updates, terminals), fork and side chat, model and reasoning-effort
selectors, the editor fs/terminal bridge as a session MCP server, JetBrains
AIR extensions (subagents, async tasks), Zed specifics, auto-review, plan and
read-only modes, `/recap`, `/workflow`, `/cua`, plugin and skill commands, the
`_meta.muse` fact set (cards for every host item kind), the host restart and
crash recovery path, and tested compatibility across Muse 1.4.11 through
1.4.18. The native server's model selector and terminal handling were not
probed with a real provider.

## Implications to watch

- Users may pick native `muse acp` for simple setups; the README could say
  when to prefer this adapter. Not done yet: needs the user's call on
  positioning.
- Behavior the native server gets right that this adapter does differently
  (for example `_meta["dev.muse"]` usage pressure) is worth comparing on the
  next adoption.
- To re-probe: run `muse acp --provider echo` with HOME and XDG_* pointed at an
  empty directory, send `initialize`, `session/new`, and `session/prompt`, one
  JSON object per line.
