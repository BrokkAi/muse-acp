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

See [ROADMAP.md](ROADMAP.md) for protocol-compatibility, reliability, feature,
and release priorities.
The [MSP event compatibility matrix](docs/event-compatibility.md) records the
ACP mapping or intentional disposition of every notification in the pinned
schema, plus observed host extensions and server-initiated requests.

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
  | MUSE_ACP_INSTALL_DIR="$HOME/bin" MUSE_ACP_VERSION=v0.2.2 sh
```

On supported Windows x86_64 systems, run the PowerShell installer for the MSVC
release:

```powershell
irm https://github.com/BrokkAi/muse-acp/releases/latest/download/install.ps1 | iex
```

It verifies the ZIP's SHA-256 checksum and installs `muse-acp.exe` to
`$env:LOCALAPPDATA\Programs\muse-acp`. Set `MUSE_ACP_VERSION` or
`MUSE_ACP_INSTALL_DIR` before running the command to pin a version or choose
another absolute directory. The installer leaves `PATH` unchanged; add the
reported directory to the user PATH and restart PowerShell. For a manual
install, download the Windows archive and adjacent `.sha256` file from
[GitHub Releases](https://github.com/BrokkAi/muse-acp/releases).

Linux release binaries require glibc. To build and install from a checkout,
run `cargo install --path .`.

### Supported release targets

The release and installer support matrix is explicit about the adapter binary,
the host runtime, and the install path:

| OS | Architecture | Runtime / host requirement | Release installer | Status |
| --- | --- | --- | --- | --- |
| macOS | x86_64 | macOS host and Muse Code for macOS | `install.sh` | Supported |
| macOS | arm64 | macOS host and Muse Code for macOS | `install.sh` | Supported |
| Linux | x86_64 | glibc; Muse Code for Linux | `install.sh` | Supported |
| Linux | arm64 | glibc; Muse 1.0.2 may need sandbox support or `--disable-sandbox` | `install.sh` | Adapter supported; see the sandbox advisory below |
| Windows | x86_64 | MSVC release target and Muse Code for Windows | `install.ps1` | Supported |

Windows arm64, Linux musl, 32-bit systems, and other operating systems have no
published release target. The editor registration commands use `HOME` on Unix
and `USERPROFILE` on Windows for their default settings paths; IntelliJ still
requires the installed executable's absolute path when `--command` is
supplied.

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
| host `item.truncated` + `outputRef`/`patchRef`/`patchSummary` | `_meta.muse` saturation and stored-output metadata; negotiated `_session/readOutput` forwards byte-ranged `item/readOutput` |
| `turn/completed` | v1 `session/prompt` response `{stopReason}` plus the turn's `usage` when the host reported any; v2 `state_update` idle + `stopReason` |
| successful native file `toolCall` items | negotiated AIR `agentFileChangeReport` after the owning turn completes; paths come only from explicit host tool arguments and are deduplicated across replay |
| `turn/cancel` | `session/cancel` (waits for the terminal event; `already_terminal` = success) |
| `approval/requested` + `approval/request` | `session/request_permission` → `approval/decide` (deny-safe fallback) |
| `session/resume` + history | `session/resume` (+ `replayFrom: {type:start}` replays messages); a retained view cursor is explicitly re-attached with `view/subscribe` so `(after, head]` events are replayed once across detachment or host restart; live-only resume still re-attaches from the returned head; usage is restored on attach: from `history.snapshot.state` when a snapshot is served, else by asking for the snapshot rung explicitly, else from one backward `view/page` read for the running totals; `session/viewHealthChanged` records an actionable stderr diagnostic |
| `sessionDurability` (default durable) | continuity across turns; Muse's durable session ID is used directly by ACP |
| `turn/start` `ifBusy` (queue default) | concurrent prompts per session; each completes its own response; `session/cancel` stops all of them |
| `TurnInputPart` image | image blocks (inline base64 or local `file://` path); advertised in caps |
| `userInput/requested` | `elicitation/create` form bridge (needs client `elicitation.form` caps); MSP `userInputDialogs` is declared from that capability and older/granting hosts still use auto-cancel as a backstop |
| `session/setApprovalMode` | `configOptions` mode selector using the MSP names verbatim (`allowAll`/`promptUnmatched`/`onRequest`/`denyUnmatched`) + `session/set_config_option`; legacy v1 `modes` / `session/set_mode` |
| `model/list` + `session/setModel` | `configOptions` model selector + `session/set_config_option`; legacy v1 `session/set_model` |
| `session/goalChanged` | provider-neutral goal metadata in `session_info_update`; goal mutation remains host/TUI-driven because ACP has no negotiated goal-control capability |
| `session/setReasoningEffort` + `session/reasoningEffortChanged` + `SnapshotState.reasoningEffort` | `configOptions` reasoning selector as the standing session default; an unset default keeps the per-turn override fallback (`none` through `ultra`) |
| `turn/steer` | v2 `_session/steering` extension with exact-turn targeting and race-safe idle behavior |
| backgrounded `toolCall` + `userShell` items | negotiated AIR async tasks: `async_task_spawned`/`async_task_state_update` plus the backgrounded marker on the owning command card; `_session/async_task/stop` maps to MSP `task/stop`, while `session/cancel` maps to `task/stopAll`; terminal item events settle the task state |
| `workflow` items | negotiated AIR async tasks keyed by `workflowRunId`; `_session/async_task/stop` admits MSP `workflow/cancel`, while the later workflow item and turn views settle the task and launching turn; ACP has no workflow child skip/retry surface |
| `subagent` items | negotiated: `subagent_spawned` + `subagent_state_update` on the parent and the child transcript replayed from `session/read` onto the child session id; otherwise a synthetic tool card with `_meta.muse` provenance |
| Muse skills | ACP `available_commands_update` is refreshed from MSP `skill/list` (and `skill/changed`); slash invocations such as `/plan` use native `skill` turn parts, with host `skillNotFound` errors preserved |
| `session/contextUsage` + `session/tokenUsage` | `usage_update` (`used`/`size` from context occupancy, also restored on attach; `_meta.museCumulative` session totals, `_meta.musePressure`); each completion is counted once, so a `view/gap` refill that replays one already seen does not re-price it; `cost` is a client-local list-price estimate from `model/list` catalog rates, summed per completion — partial in both directions (historic and unpriceable completions are excluded, cached tokens are charged at the catalog cached rate), never a billing figure. The cost object is labeled `source: adapter-estimate`, `basis: catalog-list-price`, and `billing: false` so clients cannot mistake it for Muse billing |

Muse 1.3.0 subscription usage is surfaced under `_meta.museSubscriptionUsage`.
The adapter reads the host's last observation with `usage/read` and refreshes it
for every attached session when `usage/changed` arrives. The payload keeps the
host's `observedAtMs`, `tier`, `window`, and `weekly` values intact and is marked
`source: msp-host-observation` and `billing: false`; it is an observation as of
the host arrival stamp, never a cost or billing value. When a context window is
available it rides the next `usage_update`; otherwise it uses
`session_info_update` so the adapter does not invent ACP `used`/`size` values.
An omitted `usage` member remains absent rather than being filled with zeros.

Zed currently initializes custom agents with ACP v1 even though it supports
config selectors, so the adapter returns `configOptions` in both protocol
versions: v1 uses the selector field `id` (plus a legacy `modes` fallback), while
v2 uses `configId`.

Form questions also work in both versions when the client advertises
`elicitation.form: {}` under `clientCapabilities` (v1) or `capabilities` (v2).
The adapter waits for this ACP handshake before starting MSP, then declares
`userInputDialogs: true` or `false` for the connection. A form-less client is
therefore withheld from MSP user-input requests before a question is created;
the auto-cancel path remains for hosts that predate or ignore that member.
Repeated deliveries of a pending question reuse its existing form, including
requests reissued during session resume.
For an older or noncompliant host that still sends a question after receiving
`userInputDialogs: false`, the adapter cancels it so the turn can continue; it
does not emit an unsupported ACP request.

Option-bearing questions first offer `Answer questions` or `Explain instead`;
the latter sends a question-scoped `userInput/clarify` with up to 500
characters.

Goal control stays with the Muse host/TUI. The adapter does not invent an ACP
editor request for `goal/set`, `goal/edit`, `goal/pause`, `goal/resume`, or
`goal/clear` because ACP has no standard negotiated goal-control capability.
When the host issues one of those stable MSP commands, the resulting
`session/goalChanged` is folded and published to the editor. MSP's wake gates
therefore remain authoritative: set/edit/resume may wake an idle unfinished
goal, while pause/clear never wake.

A resumed client receives the running AIR task set from the latest durable
item fold even when it did not request transcript replay; completed historical
shell commands are not re-announced. Closing the ACP connection ends the
adapter and its host child. If a durable host restarts in place, the adapter
reattaches each session and reconciles task items from the returned fold; a
terminal item settles a task that was already announced. MSP 1.3.0 task
controls support targeted stops and session-wide background cancellation.

Per-turn file reports are available when the client advertises AIR v1
`agentFileChangeReport` and places a valid `agentFileChangeReportRequest` on
the prompt. The adapter reports workspace paths from successful native Muse
file-tool completions only. It includes both endpoints of a rename and sends
paths without reading contents, so deleted and binary files are safe. Rejected
writes are excluded. Shell commands, generators, and unknown tools never cause
a guessed path; their presence marks `declaredComplete: false` instead.

Model choices are refreshed from Muse when creating, loading, or resuming a
session and after a config option changes. The adapter does not permanently
cache the first nonempty catalog. If a refresh fails, it retains the last
successful catalog; stderr records failures and each snapshot's source and
model count. An open selector does not itself trigger a refresh.

The reasoning selector sets Muse's session-wide default on hosts that support
`session/setReasoningEffort`. The adapter folds
`session/reasoningEffortChanged` and restores `SnapshotState.reasoningEffort`
on resume. Until a host reports a standing default, including on older hosts
that do not implement the setter, the selected tier remains a per-turn
`reasoningEffort` override for compatibility.

Restoring usage on attach takes up to two extra reads, and only when the
resume itself carried none. `session/contextUsage` is not durable-sourced, so
it never appears in a `view/page`; the context occupancy is only ever served in
a snapshot, and the default `auto` history rung usually resolves to `inline`.
The adapter therefore asks for the snapshot rung explicitly, and falls back to
a backward page for the running totals alone. When neither carries usage, the
session reports none until the next live `session/contextUsage`.

Catalog pricing follows the same snapshot: a successful refresh replaces the
per-model rates outright, so a model that comes back without a usable `cost`,
or that leaves the catalog, stops being priced rather than keeping the rates it
used to have. Only a failed refresh retains the previous rates. Completions
already added to a session's running estimate keep the price they were charged
at; a rate change never re-prices history.

### Client-provided MCP policy

JetBrains may attach its integrated stdio MCP server to `session/new` even when
the agent advertises no optional MCP transports (stdio, HTTP, and SSE are all
reported unsupported). This adapter deliberately does not forward client MCP
configuration:

- MSP 1.3.0 does expose a native session-scoped surface,
  `SessionConfig.mcpServers`, with typed `SessionMcpServerConfig` entries and
  the grantable `sessionMcp` capability. That is a protocol improvement, but
  it is not by itself an authorization or lifecycle contract for forwarding an
  ACP client's server definition. Muse still owns the tool runtime, approval
  flow, and sandbox. Wire typing and validation prove the shape of a request,
  not that the supplied server is allowed by those approval, sandbox, and
  workspace policies.
- This adapter does not negotiate `sessionMcp` or translate client-owned MCP
  configuration into a Muse session. Forwarding would need to define required
  versus optional startup failures, disconnects, and behavior across resume,
  fork, and close while proving that every server and tool remains inside the
  session workspace and Muse approval flow.
- Ignoring the configuration cannot widen permissions or introduce a new MCP
  process/connection. It is logged (`ignoring client-provided MCP servers`) so
  the absence of those tools is diagnosable rather than silent.

Their presence never blocks the session or its selectors. To use MCP tools,
configure them in Muse itself; if MSP later gains a native foreign-tool
surface, this policy will be revisited only after the adapter negotiates the
capability, uses an exact typed mapping, and has host-backed approval,
workspace-confinement, lifecycle, failure, and regression-test guarantees.

## Run

```sh
cargo build
./target/debug/muse-acp --selftest   # static + schema-compat + CLI probe
./target/debug/muse-acp --support    # redacted support bundle (no secrets)
```

`--selftest` validates the adapter's static payloads, prints the MSP schema
compatibility table, and probes the configured Muse CLI
(`cli-ready`/`cli-unready` with the binary and reported version). It is a
diagnostic and always exits successfully, so support output can be collected
before Muse is installed.

Env:

```sh
MUSE_CLI=muse                      # host binary (default: muse)
MUSE_SERVE_ARGS="--trust-workspace" # host-lifetime flags (see `muse serve --help`)
MUSE_APPROVAL_MODE=promptUnmatched  # allowAll|promptUnmatched|onRequest|denyUnmatched
MUSE_COMMAND_TIMEOUT_MS=60000       # override host admission-ack timeout (milliseconds)
MUSE_TOOL_OUTPUT_LIMIT=8000         # editor-facing tool output bound (characters)
MUSE_LOG=debug                     # per-method protocol tracing (no payloads)
# MUSE_ALLOW_UNSCOPED_READS=1       # DANGEROUS: allow local reads outside session cwd
```

`session/new {cwd}` starts a host session in `cwd`. Approval posture defaults
to the host default; set `MUSE_APPROVAL_MODE=promptUnmatched` to force every
unmatched tool call through `session/request_permission`.

Tool cards preserve host saturation facts in `_meta.muse`: `source: "host"`
means the host bounded the streamed surface, while `source: "adapter"` carries
the adapter's local character counts. Host `itemId`, `outputRef`, `patchRef`,
and edit-tool `patchSummary` fields are kept beside the bounded card. A client
may negotiate `_meta.muse.capabilities: ["readOutput"]` during `initialize`
and then use the adapter extension `_session/readOutput` with `sessionId`,
`itemId`, `outputRef`, and optional byte offsets and lengths to retrieve the
stored bytes. The adapter forwards the host's `outputUnavailable` data as the
typed `-32041` error.

### Workspace roots and local resources

ACP `cwd` is the primary workspace root and the base for relative resource
paths. The adapter passes it to Muse as MSP's single `workspaceRoot`. When a
client supplies `additionalDirectories`, each entry must be an absolute path;
the adapter treats `[cwd, ...additionalDirectories]` as the ordered set of
roots approved for local image and textual `resource_link` expansion. MSP v1
has no additional-root field, so these extra roots do not change Muse's own
tool workspace or sandbox policy.

The adapter accepts nested, unrelated, and symlinked roots and removes exact
duplicates while preserving first occurrence order. It resolves the requested
path and every root through the filesystem before checking containment. This
means `..`, percent-encoded separators, path spelling differences on a
case-normalizing filesystem, and symlinks cannot escape the union of approved
roots. A symlink supplied as a root authorizes its resolved target. Hard links
are path entries rather than redirects: a hard-link name inside a root is in
scope, while another name for the same inode outside every root is not.

Only valid UTF-8 text without binary control bytes is expanded as text, with a
256 KiB limit. Malformed `file://` percent escapes are rejected, remote file
hosts are rejected, and non-file resource links remain mentions. Embedded
non-image blobs remain unsupported.

On `session/load`, `session/resume`, and `session/fork`, the request's complete
additional-directory list becomes active. Omitting it or sending an empty list
activates no extra roots, so old filesystem scope is never restored implicitly.
Live `session/list` entries report that active list; Muse sessions discovered
after an adapter restart have only their persisted MSP `workspaceRoot`.

Local reads are confined to this root set by default.
`MUSE_ALLOW_UNSCOPED_READS` disables that boundary only when its value is
explicitly `1`, `true`, `yes`, or `on` (case-insensitive). Do not enable it for
untrusted sessions or workspaces.

### Authentication and remote environments

`cli-ready` only means the CLI can be invoked; it does not verify a login.
When Muse explicitly reports that it is not authenticated or that a session or
credential has expired, the adapter supplies login guidance and the configured
executable and handshake version (when available). Session creation, resume,
and prompt rejections use ACP's `-32000` authentication-required error instead
of a generic internal error. Mid-turn failures use that error in ACP v1 and an
explanatory transcript message in v2, whose prompt has already been accepted.
Raw recognized authentication error text is omitted because it may contain
credentials. MSP 1.3.0's `TurnErrorKind.authRequired` is treated as an
authoritative Muse login failure. Older hosts without that kind still require
explicit login/expiry wording; generic HTTP 401/403, permission errors, and
unknown host errors are not enough to identify a Muse login failure, so an
unexplained host error still needs investigation.

Run `muse login` with the executable selected by `MUSE_CLI`, then restart the
editor agent and retry. For SSH, containers, remote IDE backends, or another OS
account, run login **where the adapter runs, as the same OS user**. A login on
your local desktop does not establish credentials for a remote host. GUI
editors may also have a different `PATH`; set `MUSE_CLI` to an absolute path
when the editor cannot find the CLI that works in your terminal.

The adapter never opens a browser, prompts for credentials over ACP stdio, or
copies credentials between machines. In a browserless environment, inspect
`muse login --help` on that host and use only the remote/browserless flow
supported by that installed Muse version. If it requires a browser or callback
that the environment cannot provide, complete the supported login setup before
starting the adapter; there is no adapter-provided headless login bypass.

The [vendored MSP schema](tests/protocol/PROVENANCE.md) exposes no compatible
login, logout, credential-refresh, or auth-status method. Accordingly,
`authMethods` remains empty and ACP authentication requests return an
unsupported-method error with external-login guidance. The adapter does not
advertise an authentication flow it cannot complete. Error-code semantics follow
the [ACP schema](https://agentclientprotocol.com/protocol/v1/schema#errorcode).

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

The editor registration commands preserve existing agent entries, are safe to
re-run, and write a `.bak` file before changing an existing configuration.
Settings are replaced
atomically (same-directory temp file plus rename) with rollback to the
pre-edit content if the write fails. Use `--dry-run` to preview an edit.
The Unix installer targets macOS and Linux, and the PowerShell installer
targets Windows x86_64. Both binary installers verify release checksums. The
Windows installer stores the executable under
`$env:LOCALAPPDATA\Programs\muse-acp` by default and does not edit `PATH`.

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
  `state_update`. v1 replies `{stopReason}`, plus a `usage` object when the
  host reported token usage for that turn. `usage` sums the turn's
  `session/tokenUsage` legs (each counted once, replays excluded) into the
  ACP v1 shape: `totalTokens`, `inputTokens`, `outputTokens`, and the
  optional `thoughtTokens`, `cachedReadTokens`, `cachedWriteTokens`.
  `inputTokens` is MSP's counted-once `promptTokens`, so cached input is
  already inside it and reasoning tokens are already inside `outputTokens`;
  a counter the host never reported is omitted rather than sent as zero, and
  a turn with no reported usage carries no `usage` at all.
  `usage._meta` adds `"mjolnir.dev/usage-scope": "turn"` and a `muse` block
  with `modelCalls`, `apiDurationMs` (summed `durationMs`), and `modelUsage`,
  the same counters per model id. A leg with no `modelId` counts in the
  totals and is left out of `modelUsage`. v2 sessions are unchanged: they
  settle through `state_update`, which carries no usage member.
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
