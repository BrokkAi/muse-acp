# muse-acp

**Use your existing Muse Code subscription in Zed, IntelliJ IDEA, and other
JetBrains IDEs.**

[![CI](https://github.com/BrokkAi/muse-acp/actions/workflows/ci.yml/badge.svg)](https://github.com/BrokkAi/muse-acp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/BrokkAi/muse-acp)](https://github.com/BrokkAi/muse-acp/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange.svg)](https://www.rust-lang.org/)

`muse-acp` is a small, dependency-free Rust bridge between the
[Agent Client Protocol](https://agentclientprotocol.com/) (ACP) used by editors
and Muse Code's native [Muse Session Protocol](https://github.com/meta-models/muse-code-sdk)
(MSP). It ships as one native binary—no Node.js, npm, or Python runtime needed.

This project started from a simple itch: I wanted to use the Muse Code
subscription I already pay for inside the editors I already use, while keeping
Muse's session engine, tools, authentication, and approval flow.

> `muse-acp` is an independent community project. Muse Code and Muse Spark are
> products of Meta and are not affiliated with or supported by this project.

## Why MSP instead of `muse exec`?

`muse-acp` starts one long-lived `muse serve` process and translates between
ACP and MSP for the lifetime of the editor. Sessions, streamed updates,
cancellation, configuration, approvals, and resume behavior travel over Muse's
native protocol without starting a new Muse CLI process for every prompt.

Other thoughtful integrations make a different, pragmatic choice:
[bex-co/muse-code-acp](https://github.com/bex-co/muse-code-acp) and the
[`muse-codes` Rust SDK](https://github.com/meawoppl/rust-code-agent-sdks/tree/main/muse-codes)
wrap the headless `muse exec --json` event stream. That approach is useful for
one-shot automation and broad compatibility. This project is optimized for a
stateful IDE session, so MSP is the more direct fit.

## Quick start

### 1. Install Muse Code

You need [Muse Code](https://dev.meta.ai/docs/muse-code) and an authenticated
Muse account or subscription before installing this adapter:

```sh
curl -fsSL https://dev.meta.ai/install.sh | sh
muse login
```

On macOS, Muse Code is also available through Homebrew:

```sh
brew install --cask muse-code
```

Confirm `muse --version` works before continuing.

### 2. Install muse-acp

On Linux or macOS, install the latest release with:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/BrokkAi/muse-acp/releases/latest/download/install.sh | sh
```

The installer detects the platform, verifies the release archive's SHA-256
checksum, and installs `muse-acp` to `~/.local/bin`. Choose another absolute
destination or pin a version by setting an environment variable on `sh`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/BrokkAi/muse-acp/releases/latest/download/install.sh \
  | MUSE_ACP_INSTALL_DIR="$HOME/bin" MUSE_ACP_VERSION=v0.2.1 sh
```

Linux release binaries require glibc. On Windows, or for a manual install,
download the archive for your platform from [GitHub Releases](https://github.com/BrokkAi/muse-acp/releases),
verify it with the adjacent `.sha256` file, and place `muse-acp` (or
`muse-acp.exe` on Windows) on `PATH`. To build and install from a checkout, run
`cargo install --path .`.

### 3. Connect your editor

```sh
muse-acp install            # Zed
muse-acp install-intellij   # IntelliJ IDEA and other JetBrains IDEs
```

## Requirements

- Muse Code installed, authenticated, and available as `muse` on `PATH`.
- Zed, or a JetBrains IDE with AI Assistant and custom ACP agent support.
- Rust 1.88+ and Python 3 only when building or testing from source.

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
| `sessionDurability` (default durable) | continuity across turns; Muse's durable session ID is used directly by ACP |
| `turn/start` `ifBusy` (queue default) | concurrent prompts per session; each completes its own response; `session/cancel` stops all of them |
| `TurnInputPart` image | image blocks (inline base64 or local `file://` path); advertised in caps |
| `userInput/requested` | `elicitation/create` form bridge (needs client `elicitation.form` caps), else auto-cancel |
| `session/setApprovalMode` | `configOptions` mode selector (`ask`/`auto`/`deny`) + `session/set_config_option`; legacy v1 `modes` / `session/set_mode` |
| `model/list` + `session/setModel` | `configOptions` model selector + `session/set_config_option`; legacy v1 `session/set_model` |
| `reasoningEffort` on `turn/start` / `turn/steer` | `configOptions` reasoning selector (`none` through `ultra`) |
| `turn/steer` | v2 `_session/steering` extension with exact-turn targeting and race-safe idle behavior |
| Muse skills | ACP `available_commands_update`; aliases such as `/plan` are sent to Muse as `/skill plan` |

Zed currently initializes custom agents with ACP v1 even though it supports
config selectors, so the adapter returns `configOptions` in both protocol
versions: v1 uses the selector field `id` (plus a legacy `modes` fallback), while
v2 uses `configId`.

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
# MUSE_ALLOW_UNSCOPED_READS=1       # DANGEROUS: allow local reads outside session cwd
```

`session/new {cwd}` starts a host session in `cwd`. Approval posture defaults
to the host default; set `MUSE_APPROVAL_MODE=promptUnmatched` to force every
unmatched tool call through `session/request_permission`.

Local image and resource reads are confined to the session workspace by
default. `MUSE_ALLOW_UNSCOPED_READS` disables that boundary only when its value
is explicitly `1`, `true`, `yes`, or `on` (case-insensitive). Do not enable it
for untrusted sessions or workspaces.

### Linux arm64 sandbox advisory

Muse 1.0.2 may fail to start its sandbox on Linux arm64 because a required
sandbox binary is missing. Prefer upgrading Muse or installing the required
sandbox support. If neither is possible, and you explicitly accept running host
tools without the sandbox's isolation, use:

```sh
MUSE_SERVE_ARGS="--trust-workspace --disable-sandbox"
```

`--disable-sandbox` materially reduces isolation. Approval prompts and this
adapter's workspace read confinement are not substitutes for the host sandbox.
Sandbox posture is fixed for the `muse serve` lifetime; re-enable it as soon as
the host supports the platform, and re-check `muse serve --help` on newer builds.

## Editor setup

Both installers preserve existing agent entries, are safe to re-run, and write
a `.bak` file before changing an existing configuration. Use `--dry-run` to
preview an edit.

### IntelliJ IDEA and other JetBrains IDEs

JetBrains AI Assistant supports custom ACP agents through
[`~/.jetbrains/acp.json`](https://www.jetbrains.com/help/ai-assistant/activate-agents.html#add-acp-agents).
Register the installed binary with:

```sh
muse-acp install-intellij
```

The command records the running `muse-acp` binary's full path, as required by
JetBrains, while preserving other configured agents:

```json
{
  "agent_servers": {
    "muse-acp": {
      "command": "/Users/you/.local/bin/muse-acp",
      "args": [],
      "env": {}
    }
  }
}
```

Open AI Chat and select `muse-acp` as the agent. Useful variants:

```sh
muse-acp install-intellij --command /absolute/path/to/muse-acp
muse-acp install-intellij --env MUSE_CLI=/absolute/path/to/muse
muse-acp install-intellij --settings /path/to/acp.json --dry-run
muse-acp uninstall-intellij
```

### Zed

```sh
muse-acp install
```

After the binary is on `PATH`, `muse-acp install` registers it in
`~/.config/zed/settings.json` as a custom agent server:

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

```sh
muse-acp install --command /path/to/muse-acp
muse-acp install --env MUSE_CLI=muse --env MUSE_SERVE_ARGS=--trust-workspace
muse-acp install --settings /path/to/settings.json --dry-run
muse-acp uninstall
```

## Protocol notes

- v2 `session/prompt` replies `{}` on accept; completion is the terminal
  `state_update`. v1 replies `{stopReason}`.
- v2 initialization advertises steering at `_meta.steering.supported`. The
  `_session/steering` request accepts the same `sessionId` and `prompt` fields
  as `session/prompt`. `_meta.steering.idleBehavior: "promptRequired"` avoids
  starting a turn when the session is idle; otherwise the adapter uses MSP's
  atomic `ifBusy: "steer"` fallback.
- Concurrent prompts queue host-side; every turn completes its own response.
- Images in, audio out: the host input type is closed (`text|image`), so audio
  blocks are rejected with the reason. Auth has no host surface
  (`authMethods: []` is the honest answer); muse credentials live outside ACP.
- `session/list` reports durable Muse sessions, including sessions created
  outside the current adapter process, so Zed can import and restore them.
- Authority for MSP shapes is the schema the host ships
  (`muse schema generate-json-schema`); the docs site may describe a newer
  host — a fingerprint mismatch is logged, not fatal.

## Verify

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --locked -- --selftest
```

The integration tests use the checked-in fake MSP host at
`tests/fixtures/fake_serve.py`; they do not require a live Muse session.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow. Please
report vulnerabilities privately as described in [SECURITY.md](SECURITY.md),
not in a public issue.

## License

Copyright 2026 Brokk.ai.

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) and
[NOTICE](NOTICE).
