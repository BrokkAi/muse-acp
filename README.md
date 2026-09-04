# muse-acp (serve-backed)

ACP server (v2 primary, v1 fallback) for Muse, backed by one `muse serve`
host over the Muse Session Protocol (MSP). Std-only Rust, no dependencies.

## How it works

One `muse serve` child serves all ACP sessions. `session/start`
auto-subscribes us to the session view, so turns stream in as `item/*` and
`turn/*` notifications, folded into ACP `session/update`s:

| MSP | ACP |
| --- | --- |
| `item/delta` (message text) | `agent_message_chunk` (v2 carries `messageId`) |
| toolCall `item/started\|updated\|completed` | `tool_call` (v1 create) / `tool_call_update` upsert with kind/title/status/content/rawInput |
| `turn/completed` | v1 `session/prompt` response `{stopReason}`; v2 `state_update` idle + `stopReason` |
| `turn/cancel` | `session/cancel` (waits for the terminal event; `already_terminal` = success) |
| `approval/requested` + `approval/request` | `session/request_permission` → `approval/decide` (deny-safe fallback) |
| `session/resume` + history | `session/resume` (+ `replayFrom: {type:start}` replays messages) |
| `sessionDurability` (default durable) | continuity across turns; cross-restart resume via `_meta.mspSessionId` |
| `turn/start` `ifBusy` (queue default) | concurrent prompts per session; each completes its own response; `session/cancel` stops all of them |
| `TurnInputPart` image | image blocks (inline base64 or local `file://` path); advertised in caps |
| `userInput/requested` | `elicitation/create` form bridge (needs client `elicitation.form` caps), else auto-cancel |
| `session/setApprovalMode` | v2 `configOptions` mode selector (`ask`/`auto`/`deny`) + `session/set_config_option`; v1 `session/set_mode` |
| `model/list` + `session/setModel` | v2 `configOptions` model selector + `session/set_config_option`; v1 `session/set_model` |

## Run

```sh
cargo build
./target/debug/muse-acp
```

Env:

```sh
MUSE_CLI=muse                      # host binary (default: muse)
MUSE_SERVE_ARGS="--trust-workspace" # host-lifetime flags (see `muse serve --help`)
MUSE_APPROVAL_MODE=promptUnmatched  # allowAll|promptUnmatched|onRequest|denyUnmatched
```

`session/new {cwd}` starts a host session in `cwd`. Approval posture defaults
to the host default; set `MUSE_APPROVAL_MODE=promptUnmatched` to force every
unmatched tool call through `session/request_permission`.

## Protocol notes

- v2 `session/prompt` replies `{}` on accept; completion is the terminal
  `state_update`. v1 replies `{stopReason}`.
- Concurrent prompts queue host-side; every turn completes its own response.
- Images in, audio out: the host input type is closed (`text|image`), so audio
  blocks are rejected with the reason. Auth has no host surface
  (`authMethods: []` is the honest answer); muse credentials live outside ACP.
- `session/list` reports adapter-owned sessions.
- Authority for MSP shapes is the schema the host ships
  (`muse schema generate-json-schema`); the docs site may describe a newer
  host — a fingerprint mismatch is logged, not fatal.

## Verify

```sh
cargo build
./target/debug/muse-acp --selftest  # static wire literals parse
python3 /tmp/acp2_tool.py     # v2 tool turn
python3 /tmp/acp2_perm.py     # approval deny -> turn continues
python3 /tmp/acp2_misc.py cancel|resume|v1
python3 /tmp/acp2_conc.py     # concurrent turns, both idle
python3 /tmp/acp2_focus.py    # image color, deny mode
python3 /tmp/acp2_ui2.py      # elicitation bridge -> answered file
```
