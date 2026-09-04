# muse-acp

Minimal ACP (Agent Client Protocol v1) adapter bridging stdio JSON-RPC to the
local `muse-code` CLI. Std-only Rust, no third-party deps.

## How it works

- Transport: JSON-RPC 2.0 over stdio, NDJSON (one object per `\n` line).
  Client spawns this binary, writes stdin, reads stdout. stderr is logs only.
- `initialize` → advertises text-only prompt caps
  (`loadSession: false`, no auth).
- `session/new {cwd}` → allocates `sessionId`, stores `cwd`.
  `mcpServers` is accepted and ignored (unsupported).
- `session/prompt {sessionId, prompt: ContentBlock[]}` → concats
  `{type:"text"}` blocks, spawns `muse exec <extra-args> <prompt>` with `current_dir=cwd`
  (non-interactive, stdin closed), streams each stdout chunk as
  `session/update {sessionUpdate:"agent_message_chunk"}` and replies
  `{stopReason:"end_turn"}` (nonzero exit → `-32603`; cancelled → `"cancelled"`).
- `session/cancel {sessionId}` (notification) → flags cancel + kills the child.
- `authenticate | session/load | session/set_mode | session/set_model | logout`
  → `-32601` not supported.

## Run

```sh
cargo run --quiet
# or
cargo build --release
./target/release/muse-acp
```

Custom CLI binary / flags:

```sh
MUSE_CLI=muse MUSE_CLI_EXTRA_ARGS="--model foo" cargo run --quiet
```

Default spawn is `muse exec <extra-args> <prompt…>` in `session/new`'s `cwd`
(non-interactive headless mode; extra args from `MUSE_CLI_EXTRA_ARGS`,
e.g. `--provider echo`, are inserted before the prompt).

## Install into Zed

```sh
cargo install --path .   # puts `muse-acp` on PATH
muse-acp install
```

This registers `muse-acp` in `~/.config/zed/settings.json` as a custom agent
server (resolved via `PATH`, so no binary copying involved):

```json
{
  "agent_servers": {
    "muse-acp": {
      "type": "custom",
      "command": "muse-acp",
      "args": [],
      "env": {}
    }
  }
}
```

Then restart Zed (or reload settings) and pick `muse-acp` in the Agent panel.
The edit is surgical: comments, formatting, and all other keys in
`settings.json` are preserved (a `.bak` backup is written first). Re-running
`install` is idempotent.

Options:

```sh
muse-acp install --command /path/to/muse-acp  # pin an absolute path instead
muse-acp install --env MUSE_CLI=muse --env FOO=bar
muse-acp install --settings /path/to/settings.json --dry-run
muse-acp uninstall                             # remove the entry again
```

## Minimal session (NDJSON on stdin)

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"x","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"}}
{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"<id-from-2>","prompt":[{"type":"text","text":"hello"}]}}
```

Cancel:

```json
{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"<id-from-2>"}}
```

## Layout

- `src/main.rs` — stdio loop, dispatch, muse runner, minimal JSON parser.
- `Cargo.toml` — package `muse-acp`, edition 2024, no dependencies.

## Limits

- Text prompts only; image/audio/resource blocks are skipped.
- One prompt worker per session thread; responses may interleave across sessions.
- Build not verified in this sandbox (`bwrap` blocks `cargo`/proc spawn here);
  run `cargo build` locally to confirm.
