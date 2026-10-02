# muse-acp

**Use your existing Muse Code subscription in Zed, JetBrains IDEs, and any other
ACP client.**

[![CI](https://github.com/BrokkAi/muse-acp/actions/workflows/ci.yml/badge.svg)](https://github.com/BrokkAi/muse-acp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/BrokkAi/muse-acp)](https://github.com/BrokkAi/muse-acp/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange.svg)](https://www.rust-lang.org/)

`muse-acp` is a bridge between editors that speak the
[Agent Client Protocol](https://agentclientprotocol.com/) (ACP) and
[Muse Code](https://dev.meta.ai/docs/muse-code). It talks to Muse over its
native [Muse Session Protocol](https://github.com/meta-models/muse-code-sdk)
(MSP), so your subscription keeps Muse's own session engine, tools,
authentication, and approval flow.

The adapter is one small Rust binary with no runtime dependencies. It supports
ACP v1 and v2, and installs on macOS, Linux, and Windows.

**Auto-review: stop clicking "Allow" for routine edits.** Choose
**Auto-review** in your editor's approval-mode selector, and `muse-acp`
approves Muse's requests to read and edit ordinary files in your workspace for
you, one request at a time. Shell commands, network access, MCP tools,
deletes, and anything outside the workspace or under a hidden folder such as
`.git` or `.github` still ask you first. See [Auto-review](#auto-review).

> `muse-acp` is an independent community project. Muse Code and Muse Spark are
> products of Meta Platforms, Inc. This project is not affiliated with,
> endorsed by, or supported by Meta.

## Requirements

- **Muse Code** installed, authenticated, and available as `muse` on `PATH`.
- A **stdio ACP client**: Zed, a JetBrains IDE with AI Assistant, or another
  ACP client such as [micro-acp](https://github.com/BrokkAi/micro-acp).
- **Node.js 22+** if you install from npm.
- **Rust 1.88+** if you build from source.

## Quick start

### 1. Install Muse Code

Install Muse and log in before installing the adapter:

```sh
curl -fsSL https://dev.meta.ai/install.sh | sh
muse login
```

On macOS you can also use Homebrew:

```sh
brew install --cask muse-code
```

Confirm `muse --version` works, then continue.

### 2. Install muse-acp

With Node.js 22 or later, install from npm:

```sh
npm install -g @brokkai/muse-acp
muse-acp --selftest
```

Or run it without a global install:

```sh
npx --yes @brokkai/muse-acp --selftest
```

The npm package bundles the native binaries for every supported platform, so it
needs no install scripts or separate downloads.

On Linux and macOS you can install the latest release instead:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/BrokkAi/muse-acp/releases/latest/download/install.sh | sh
```

The installer detects your platform, verifies the archive's SHA-256 checksum,
and installs `muse-acp` to `~/.local/bin`. Pin a version or choose another
absolute destination with environment variables on `sh`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/BrokkAi/muse-acp/releases/latest/download/install.sh \
  | MUSE_ACP_INSTALL_DIR="$HOME/bin" MUSE_ACP_VERSION=vX.Y.Z sh
```

On Windows x86_64, run the PowerShell installer for the MSVC release:

```powershell
irm https://github.com/BrokkAi/muse-acp/releases/latest/download/install.ps1 | iex
```

It verifies the ZIP's SHA-256 checksum and installs `muse-acp.exe` to
`$env:LOCALAPPDATA\Programs\muse-acp`, leaving `PATH` untouched. Set
`MUSE_ACP_VERSION` or `MUSE_ACP_INSTALL_DIR` to pin a version or choose another
absolute directory, then add the reported directory to your user `PATH`.

From a checkout, build and install with Cargo:

```sh
cargo install --path .
```

### 3. Connect your editor

```sh
muse-acp install            # Zed
muse-acp install-intellij   # IntelliJ IDEA and other JetBrains IDEs
```

Both commands are safe to re-run: they preserve existing agent entries, write a
`.bak` backup, and replace the settings file atomically. Use `--dry-run` to
preview an edit.

Any other ACP client can launch `muse-acp` directly over stdio.

## Editor setup

### Zed

`muse-acp install` registers the adapter in Zed's settings file,
`~/.config/zed/settings.json` (`%APPDATA%\Zed\settings.json` on Windows):

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

On Windows it records the full path to the running `muse-acp.exe` instead,
because the PowerShell installer leaves `PATH` untouched.

### JetBrains IDEs

`muse-acp install-intellij` writes `~/.jetbrains/acp.json`, recording the full
path to the running binary as JetBrains requires:

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

Then open AI Chat and select `muse-acp`.

### Installer options

`install` and `install-intellij` accept:

```sh
muse-acp install --command /absolute/path/to/muse-acp
muse-acp install --env MUSE_CLI=/absolute/path/to/muse
muse-acp install --settings /path/to/settings.json --dry-run
muse-acp uninstall
muse-acp uninstall-intellij
```

Run `muse-acp help` for the full option list.

## Configuration

The adapter reads its settings from the environment. Export them in your shell,
or set them in the client's agent entry so the editor passes them to the
adapter.

| Variable | Default | Purpose |
| --- | --- | --- |
| `MUSE_CLI` | `muse` | Muse host binary to launch. Use an absolute path when the editor's `PATH` differs from your shell's. On Windows the default also finds the `muse.cmd` launcher that the Muse installer puts on `PATH`. When `PATH` has no `muse`, the default falls back to the Muse installer's location (`MUSE_INSTALL_DIR`, else `~/.local/bin`, or `%LOCALAPPDATA%\Programs\muse` on Windows). |
| `MUSE_SERVE_ARGS` | none | Extra host-lifetime flags for `muse serve` (see `muse serve --help`). Split on whitespace; no shell quoting or expansion. |
| `MUSE_APPROVAL_MODE` | host default | Force an approval posture: `allowAll`, `autoReview`, `promptUnmatched`, `onRequest`, or `denyUnmatched`. `promptUnmatched` sends every unmatched tool call through `session/request_permission`. `autoReview` is `promptUnmatched` with [auto-review](#auto-review), and also applies to sessions you load or resume. |
| `MUSE_COMMAND_TIMEOUT_MS` | method-specific | Override the host admission-ack deadline, in milliseconds. |
| `MUSE_SHUTDOWN_TIMEOUT_MS` | `8000` | Shutdown deadline, 100–60000 ms. |
| `MUSE_TOOL_OUTPUT_LIMIT` | `8000` | Editor-facing tool output bound, in characters (minimum 200). |
| `MUSE_LOG` | `normal` | Set to `debug` for per-method protocol tracing (no payloads). |
| `MUSE_ALLOW_UNSCOPED_READS` | off | **Dangerous.** Set to `1`, `true`, `yes`, or `on` to allow local reads outside the approved workspace roots. |

Without `MUSE_COMMAND_TIMEOUT_MS`, admission deadlines are 30 seconds for the
handshake, queries, and approval or input decisions; 180 seconds for session
start, resume, and read and for view paging; and 60 seconds for other methods.
These bound how long the adapter waits for a command response, not how long a
model turn may run.

### Auto-review

Auto-review approves routine file work for you, so you only answer the
requests that need a person. Turn it on by choosing **Auto-review** in the
editor's approval-mode selector, or start every session with it by setting
`MUSE_APPROVAL_MODE=autoReview`. Muse itself runs in **Prompt unmatched**
mode, and `muse-acp` answers each approval request Muse sends:

- **Approved for you:** reading, listing, searching, writing, creating, and
  editing a file whose real path, after following symbolic links, is inside
  the session's workspace roots (the editor's working directory and any
  additional directories). A file that does not exist yet counts when its
  nearest existing folder is inside.
- **Still asks you:** shell commands and processes, network access, MCP and
  other tools, deleting or moving files, and any file outside the workspace.
  It also asks about anything under a hidden file or folder inside the
  workspace, such as `.git`, `.github`, `.env`, or `.vscode`, since those hold
  credentials and settings that run code. Requests Muse marks as protected
  writes or escalates for review, and any kind of request `muse-acp` does not
  recognize, still ask you as well.

Auto-review only ever picks Muse's allow-once choice. It never saves an
"allow for this session" or "always allow" rule, so turning it off takes
effect on the very next request. Each approval it makes is logged to the
agent log as `auto-review approved <access> <path> once`, and each request it
leaves to you is logged with the reason.

Muse remembers every other approval mode with the session, but it does not
know about auto-review. A session you load or resume therefore comes back in
Prompt unmatched, unless `MUSE_APPROVAL_MODE=autoReview` is set. A fork keeps
its source's mode.

This is not Muse's own `:auto-review` permission profile, which asks an AI
reviewer and which `muse serve` cannot run (see below). `muse-acp`'s
auto-review is a fixed set of rules and never consults a model.

### Approval-profile compatibility

If Muse is saved with the `:auto-review` permission profile, `muse serve`
cannot start its automated reviewer. The adapter gives its own `muse serve`
child a private, temporary settings view that uses `:ask-me`, so approvals come
to your editor. Your saved Muse settings, editor launcher, and session data are
left untouched, and the temporary view is removed when the host exits. Other
permission profiles are passed through unchanged.

The view links the rest of your configuration instead of copying it, so
credentials are never copied. On Windows it needs neither Developer Mode nor
administrator rights: folders are linked with directory junctions, and files
with symbolic links when Windows allows them, otherwise with hard links. When
Muse replaces a hard-linked file, such as a refreshed `auth.json`, the adapter
moves the new file back over the original when the host exits, unless the
original also changed in the meantime. If the editor stops the agent before
it can, the next launch finishes the job. With hard links, a file you replace
outside the agent while it runs, for example by running `muse login` in a
terminal, reaches `muse serve` after the agent restarts. If the view cannot
be built, for example because the temporary folder is on a different drive
from your configuration, `muse serve` starts with your saved settings, and the
error the editor shows if Muse then refuses the profile says why.

## What's supported

- **Sessions** — new, load, resume, list, close, and fork, with durable Muse
  session IDs that survive adapter and host restarts.
- **Turns** — streamed text and tool updates, queued concurrent prompts,
  cancellation with a terminal event, and exact-turn steering over the ACP v2
  `_session/steering` extension.
- **Approvals** — Muse approval requests surfaced as
  `session/request_permission`, with a deny-safe fallback, and
  [auto-review](#auto-review) to approve workspace file reads and edits for
  you.
- **MCP servers** — stdio and HTTP MCP servers attached by the editor load
  into the Muse session on Muse 1.3.0 and newer; see
  [Client-provided MCP servers](#client-provided-mcp-servers).
- **Questions** — Muse `userInput/requested` bridged to ACP
  `elicitation/create` forms when the client advertises form support; otherwise
  the host falls back to auto-cancel.
- **Configuration** — model, approval mode, and reasoning effort exposed as ACP
  `configOptions` selectors, refreshed from Muse on create, load, resume, and
  config change. The reasoning selector starts at `Muse default` and sends no
  override until you pick a tier.
- **Skills** — Muse's skill catalog drives ACP `available_commands_update`.
  Slash prompts such as `/plan` use native skill turn parts; `/compact` invokes
  Muse's native compaction.
- **Goals** — `/goal <objective>` sets the session goal, `/goal edit
  <objective>` replaces it, and `/goal pause`, `/goal resume`, `/goal clear`
  manage it, mapping onto the host `goal/*` methods. When a command starts a
  goal turn, the prompt stays open until that turn ends, and Stop interrupts
  it (Muse then pauses the goal). Stop also reaches goal turns Muse starts on
  its own. Objectives may include @-mentions. Goal state still streams back
  through `session/goalChanged` display metadata.
- **Session names** — `/rename <name>` renames the session through the host's
  `session/rename`; the new title arrives through `session/nameChanged` as an
  ACP `session_info_update`.
- **Workflow children** — workflow cards list each child with its id, and
  `/workflow-child skip <childId>` or `/workflow-child retry <childId>` controls
  one child of a running workflow through the host's `workflow/childControl`.
  A bare `/workflow-child` lists the children you can control.
- **Content** — text, inline and local-file images, `resource_link` text
  expansion, and embedded context. Audio is rejected, because Muse's input type
  is closed to `text` and `image`.
- **Usage** — context occupancy as `usage_update`, cumulative session totals,
  subscription observations under `_meta.museSubscriptionUsage`, and a
  client-local list-price cost estimate explicitly labeled as not a billing
  figure.
- **Tasks and subagents** — backgrounded shell commands, workflows, and native
  subagents surfaced through negotiated ACP extensions where the client
  advertises support, with synthesized tool cards otherwise.
- **File changes** — per-turn workspace file-change reports from the host's
  native file tools when the client negotiates the AIR extension.
- **Stored output** — bounded tool output with head and tail retained, plus
  opt-in `_session/readOutput` access to the host's stored bytes.

### Protocol extensions

Beyond core ACP, the adapter negotiates these extensions. Each activates
only when the client opts in too, with the documented fallback otherwise:

- `_session/steering` (ACP v2 only) — exact-turn steering; rejected with
  `-32601` on v1. Follows the ecosystem `_session/steering` convention
  (`steering.supported`); the standards-track `session/inject` proposal is
  still unmerged — adopting it is future work.
- `_session/readOutput` — opt-in reads of host-stored tool output.
- `_session/userShell` — shell commands outside any turn; needs editor
  opt-in, AIR `asyncTasks`, and a host grant all together.
- `_session/async_task/stop` — stop one background task; `session/cancel`
  maps background work to `task/stopAll`.
- JetBrains AIR v1 (`agentFileChangeReport`, `nativeSubagentSessions`,
  `asyncTasks`, `recommendedValue`) — per-turn file-change reports, native
  subagent sessions, async-task observation and stops, model and reasoning
  recommendations.

The [MSP event compatibility matrix](docs/event-compatibility.md) records the
ACP mapping or intentional disposition of every notification in the pinned
schema. [ROADMAP.md](ROADMAP.md) tracks compatibility, reliability, and release
priorities.

### Client-provided MCP servers

MCP servers that your editor attaches to a session become tools in that Muse
session. This includes Zed's context servers and the IDE server JetBrains
passes. It needs Muse 1.3.0 or newer: the adapter requests the host's
`sessionMcp` capability and loads the servers through `session/start` and
`session/resume` configuration.

- **Transports** — stdio servers and HTTP (streamable HTTP) servers. The
  adapter advertises HTTP support (`mcpCapabilities.http` in ACP v1,
  `session.mcp` in ACP v2) only when the host granted `sessionMcp`. SSE
  servers are not supported by Muse and are dropped with a log line.
- **Failures** — every server is loaded as optional. A server that cannot
  start is skipped, and the session keeps working without its tools. An
  invalid entry (for example, one with no command) is dropped with a log line
  and does not fail the session.
- **Approvals** — the model sees each tool as `mcp__<server>__<tool>`, and
  every call goes through Muse's normal approval flow. Muse's "Always allow
  this MCP tool" choice saves a persistent rule for that server and tool name.
  It applies to any later server with the same name, including one configured
  in Muse itself.
- **Reloading** — Muse fixes a session's MCP servers when it loads the
  session, and it does not save them. Loading a session again re-sends the
  editor's current servers. If the adapter's Muse host already has the session
  loaded with a different set, for example after you close a thread, change
  context servers, and reopen it, the session keeps its current servers until
  muse-acp restarts. A restarted Muse host gets the servers again
  automatically.
- **Forks** — a forked session starts without the editor's MCP servers,
  because Muse's fork takes no configuration. It gets them the next time Muse
  loads it.
- **Older Muse hosts** — without the `sessionMcp` grant, the servers are
  dropped and the adapter logs `ignoring client-provided MCP servers: this
  Muse host did not grant sessionMcp`. Configure MCP servers in Muse itself
  instead.

The adapter log names forwarded and dropped servers but never prints their
commands, arguments, URLs, environment values, or headers.

## How it compares

Several independent projects bridge Muse Code to ACP. They broadly split into
two designs: adapters that speak Muse's native session protocol over a
long-lived `muse serve`, and bridges that wrap the one-shot `muse exec --json`
event stream. The table below reflects each project's public documentation and
package metadata as of September 2026; check the projects themselves for
current behavior.

| Adapter | Language / runtime | Muse transport | Install | Editor targets | License |
| --- | --- | --- | --- | --- | --- |
| **muse-acp** (this project) | Rust; single native binary, no runtime | MSP over one long-lived `muse serve` | npm `@brokkai/muse-acp`, release installers, `cargo install` | Zed, JetBrains, any stdio ACP client | Apache-2.0 |
| [bex-co/muse-code-acp](https://github.com/bex-co/muse-code-acp) | TypeScript; Node.js 22+ | Muse SDK over `muse serve` | npm `@bex-co/muse-code-acp` | Zed, VS Code, other ACP clients | Apache-2.0 |
| [sanjay3290/muse-acp](https://github.com/sanjay3290/muse-acp) | TypeScript; Node.js 20+ | MSP over `muse serve` | npm `muse-acp` | ACP clients (Zed example) | Apache-2.0 |
| [julianubico/muse-code-acp-bridge](https://github.com/julianubico/muse-code-acp-bridge) | JavaScript; Node.js 22.13+ | `muse exec --json` JSONL | from source (documented npm name not currently published) | acpx custom agents | MIT |
| [einklover/muse-acp-server](https://github.com/einklover/muse-acp-server) | TypeScript; Node.js 22+ | `muse exec --json`, with model traffic proxied through OpenCode credentials | from source | Paseo | MIT |
| [jannotix/muse-acp-agent](https://github.com/jannotix/muse-acp-agent) | TypeScript | Uses Muse Code models as the reasoning core | from source | ACP clients | Apache-2.0 |

The headless `muse exec --json` design is a good fit for one-shot automation and
scripted workflows. This adapter chooses MSP so a single editor session keeps
native streaming, approvals, cancellation, configuration, resume, and usage,
instead of starting a new Muse CLI process for each prompt.

Adjacent projects worth knowing about, though not ACP adapters themselves:
[BrokkAi/mjolnir](https://github.com/BrokkAi/mjolnir) is a Rust control plane
for several ACP coding agents including Muse, and
[agentic-control-plane/muse-code-acp-plugin](https://github.com/agentic-control-plane/muse-code-acp-plugin)
is a Muse plugin that policy-checks tool calls.

## How it works

One `muse serve` child serves all ACP sessions for the adapter's lifetime.
`session/new` starts a host session in the requested `cwd`; `session/start`
auto-subscribes the adapter to the session view so turns stream in as `item/*`
and `turn/*` notifications. The adapter folds those into ACP `session/update`
messages:

| Muse (MSP) | Editor (ACP) |
| --- | --- |
| `session/start`, turns, and history | `session/new`, `session/prompt`, `session/load`, `session/resume`, `session/fork` |
| `item/delta` message text | `agent_message_chunk` |
| `toolCall` items | `tool_call` and `tool_call_update` |
| `turn/completed` | v1 prompt response with stop reason and usage; v2 `state_update` |
| `approval/requested` | `session/request_permission`, answered with `approval/decide` |
| `userInput/requested` | `elicitation/create` form |
| `model/list`, `session/setModel`, `session/setApprovalMode`, `session/setReasoningEffort` | `configOptions` selectors and `session/set_config_option` |
| `skill/list`, `skill/changed` | `available_commands_update` |
| `session/contextUsage`, `session/tokenUsage` | `usage_update` |
| `turn/steer` | `_session/steering` (ACP v2) |
| Forks, subagents, async tasks, user shell, stored output | negotiated ACP extensions |

Muse's stable schema is the authority for MSP shapes (`muse schema
generate-json-schema`). A schema fingerprint mismatch with the vendored bundle
is logged, not fatal.

## Workspace and file access

ACP's `cwd` is the primary workspace root and the base for relative resource
paths; the adapter passes it to Muse as MSP's single `workspaceRoot`. If a
client sends `additionalDirectories`, each must be absolute, and the adapter
treats `[cwd, ...additionalDirectories]` as the ordered set of roots approved
for local image and textual `resource_link` expansion. MSP v1 has no
additional-root field, so extra roots do not widen Muse's own tool workspace or
sandbox policy.

Local reads are confined to that root set. The adapter resolves each path and
root through the filesystem before checking containment, so `..`,
percent-encoded separators, case differences, and symlinks cannot escape the
approved roots. Only valid UTF-8 text without binary control bytes is expanded,
up to 256 KiB per resource; malformed `file://` escapes and remote hosts are
rejected. Setting `MUSE_ALLOW_UNSCOPED_READS` disables this boundary and should
not be used with untrusted sessions.

## Authentication and remote environments

When Muse has no credential, `session/new` and `session/load` fail with ACP's
`-32000` authentication-required error, so editors such as Zed open their login
screen before the first prompt. The adapter learns this from Muse's
`account/read`, which reports only which kind of credential is in effect. That
method is experimental, so the adapter opts into Muse's experimental API. If
the check is unavailable, the first prompt reports the same error instead. A
credential that expires later also yields `-32000` with login guidance.
Editors that support ACP terminal auth then offer the
adapter's `muse-login` method. It runs `muse-acp login` in a terminal, which
runs `muse login` with the executable selected by `MUSE_CLI`, so you can
approve the device code in your browser. You can also run either command
yourself, then restart the editor agent.

```sh
muse-acp login                   # or: npx --yes @brokkai/muse-acp login
```

`META_API_KEY`, when set in the agent's environment, takes priority over the
account login.

If `muse serve` cannot start at all, the adapter still completes the ACP
handshake. Every later request then returns the startup diagnostic, so the
editor shows what to fix.

If Muse is not installed, requests fail with the same `-32000` error, and the
auth method is named **Set up Muse Code**. `muse-acp login` then
shows the official Muse installer command (`curl -fsSL
https://dev.meta.ai/install.sh | bash`, or `irm https://dev.meta.ai/install.ps1
| iex` on Windows) and asks before running it. It installs only on Enter or
`y`, then continues with `muse login`. It never installs when `MUSE_CLI` is
set. Restart the editor agent afterwards if it still reports Muse as missing.
Muse's installer puts `muse` in `~/.local/bin` (`%LOCALAPPDATA%\Programs\muse`
on Windows, or `MUSE_INSTALL_DIR`), and the adapter looks there when `muse` is
not on the editor's `PATH`.

Run the login **where the adapter runs, as the same OS user**. For SSH,
containers, remote IDE backends, or a different OS account, a login on your
desktop does not help; log in on the remote host. The adapter never opens a
browser, never prompts for credentials over ACP stdio, and never copies
credentials between machines. Its only ACP auth method runs Muse's own login
in a terminal, and `account/read` never returns key material, so credentials
stay in the Muse environment.

## Sandbox advisory for Linux arm64

Muse 1.0.2 may fail to start its sandbox on Linux arm64 when a required sandbox
binary is missing. Prefer upgrading Muse or installing the sandbox support.
Only if neither is possible, and only if you accept running host tools without
the sandbox's isolation:

```sh
MUSE_SERVE_ARGS="--trust-workspace --disable-sandbox"
```

`--disable-sandbox` materially reduces isolation, and neither approval prompts
nor this adapter's read confinement replace it. Sandbox posture is fixed for
the life of `muse serve`; re-enable it as soon as the host supports your
platform.

## Diagnostics

```sh
muse-acp --selftest   # static payload + schema compatibility + CLI probe
muse-acp --support    # redacted support bundle, safe to paste into a report
```

`--selftest` validates the adapter's built-in payloads, prints the MSP schema
compatibility table, and reports whether the configured Muse CLI can be invoked
(`cli-ready` or `cli-unready`). It exits `0` even when Muse is not installed, so
you can collect output on a machine that is still being set up.

Set `MUSE_LOG=debug` for per-method protocol tracing. Tracing records method
names and outcomes, not payloads.

When the editor closes the ACP connection, the adapter starts a shutdown
deadline, fails outstanding requests, and exits. If the deadline expires first,
it records a diagnostic and exits nonzero.

## Supported platforms

| OS | Architecture | Notes |
| --- | --- | --- |
| macOS | x86_64, arm64 | Supported |
| Linux | x86_64, arm64 | glibc; see the Linux arm64 sandbox advisory above |
| Windows | x86_64 | MSVC release |

Windows arm64, Linux musl, 32-bit systems, and other operating systems have no
published release target.

## Development

Build and run from a checkout:

```sh
cargo build
./target/debug/muse-acp --selftest
```

Before submitting a change:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run --locked -- --selftest
node --test npm/test/launcher.test.cjs
python3 -m unittest discover -s scripts -p 'test_*.py'
```

The integration tests use the checked-in fake MSP host at
`tests/fixtures/fake_serve.py`, so they do not require a live Muse session.
See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development workflow and
[RELEASING.md](RELEASING.md) for the release process.

## Security

Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md),
never in a public issue.

## License

Copyright 2026 Brokk.ai. Licensed under the Apache License, Version 2.0. See
[LICENSE](LICENSE) and [NOTICE](NOTICE).
