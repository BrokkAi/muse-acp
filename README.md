# muse-acp

`muse-acp` is a stdio Agent Client Protocol (ACP) agent that exposes a local
[Muse Code SDK](https://meta-models.github.io/muse-code-sdk/) host. It keeps one
persistent `muse serve` MSP child alive for all ACP sessions instead of spawning
a stateless `muse exec` process for every prompt.

The implementation uses the official `agent-client-protocol` Rust runtime for
JSON-RPC framing, batching, request correlation, cancellation, and protocol
version negotiation.

## Protocol support

The agent advertises both ACP v1 and v2 and uses the runtime's protocol router.
The negotiated implementation owns the connection for its lifetime; the gateway
does not translate between v1 and v2 after initialization.

| ACP operation | Muse MSP operation |
| --- | --- |
| `initialize` | MSP `initialize` has already completed before the ACP server starts |
| v1 `session/load` | `session/resume` |
| v1 `session/prompt` | `turn/start`, then waits for `turn/completed` |
| v1/v2 `session/list` | `session/list` |
| v1/v2 `session/new` | `session/start` |
| v2 `session/resume` | `session/resume`, optional replay from start |
| v2 `session/prompt` | `turn/start`; acceptance returns immediately |
| v1/v2 `session/delete` | guarded Muse session-index and durable-storage cleanup |
| `_session/steering` | `turn/steer` for a running turn; compatibility fallback uses `turn/start` with `ifBusy: "steer"` |
| v1/v2 `session/cancel` | pending interaction resolution plus `turn/cancel` |
| v1/v2 `session/close` | cancellation, `view/unsubscribe`, and local cleanup |
| v1 `authenticate`/`logout`; v2 `auth/login`/`auth/logout` | terminal Muse login and `muse logout` |
| `session/set_config_option` for `muse.model` | `session/setModel` |
| `session/set_config_option` for `muse.reasoningEffort` | stored locally and sent as every `turn/start.reasoningEffort` |

Muse session IDs are exposed directly as ACP session IDs. New and resumed Muse
sessions are durable by default, which is what makes list, resume, and replay
possible.

MSP view notifications are translated to ACP `session/update` notifications in
cursor order. Supported updates include:

- user, agent, and reasoning messages and streamed chunks;
- tool calls, user shells, subagents, and workflows;
- todo-list plan updates;
- context-window usage;
- running/idle/requires-action state;
- model configuration changes;
- approval requests and user-input forms.

Prompt text and images are passed to MSP as `TurnInputPart` values. File
resource links are converted to Muse-style relative `@path` text mentions.
Audio, embedded resources, and additional workspace roots are intentionally
rejected. Inline MCP servers are also rejected; see the compatibility note
below.

Approval choices are mapped to ACP permission options. MSP user-input questions
are mapped to ACP form elicitation when the client advertises form support;
otherwise the MSP input is cancelled so the turn is not left stuck. Cancelling
or closing a session also sends `$/cancel_request` for outstanding ACP
permission or elicitation requests.

## Mid-turn steering

The deployed Codex and Claude ACP agents expose steering through the
underscore-prefixed extension method `_session/steering`. `muse-acp` follows
that contract and advertises it in the top-level initialize result:

```json
{
  "_meta": {
    "steering": {
      "supported": true
    }
  }
}
```

A request contains `sessionId` and the same prompt content array as
`session/prompt`. While a turn is running, the gateway sends MSP `turn/steer`
with the expected turn ID and the session's currently selected reasoning
effort, then returns `{ "outcome": "injected" }`. The steering user message is
reported once through `session/update`; Muse's own echo is deduplicated.

If no turn is running and the request opts in through
`_meta.steering.idleBehavior = "promptRequired"`, the adapter returns
`{ "outcome": "promptRequired", "reason": "noRunningTurn" }` without consuming
the content. This is the host-owned fallback used by Mjolnir. Without that
opt-in, the adapter preserves the older deployed behavior by starting a new
turn and returning `{ "outcome": "startedNewTurn" }`.

## Authentication and durable session deletion

When the ACP client advertises terminal authentication (either
`clientCapabilities.auth.terminal` or the deployed
`clientCapabilities._meta.terminal-auth` extension), initialization advertises
a `muse-login` terminal method. The method runs this executable with the hidden
`login` argument, which in turn execs `MUSE_CLI login`. ACP v1 exposes
`authenticate`/`logout`; ACP v2 exposes `auth/login`/`auth/logout`. Logout runs
`MUSE_CLI logout` and does not secretly terminate already-active sessions.

Both protocol versions advertise `session/delete`. Because MSP v1 has no
deletion method, the adapter implements deletion against Muse's versioned
session index, removes that session's transcript directory and MSP view cache,
and commits the index deletion in one immediate SQLite transaction. The index
schema and path containment are checked first; unrecognized future index
versions fail closed instead of guessing.

## Inline MCP compatibility

ACP allows `session/new` and `session/resume` to carry inline stdio and HTTP
MCP servers, and an agent that advertises those capabilities should connect to
every requested server. Muse MSP v1 does not currently expose per-session
transport registration: `session/start` and `session/resume` have no MCP
fields, and `session/start.config` is reserved with no members. The official
TypeScript SDK source closure at upstream commit
`507c86fef428fb0eebade068433fdc4e058eed88` confirms that boundary:
`StartSessionOptions` omits the empty `config`, `ResumeSessionOptions` is
derived from `SessionResumeParams`, `MuseClientSpawnOptions` exposes only
launch-level process arguments/environment rather than a per-session agent or
MCP definition, and the generated `MspMethod` union has no MCP method.
Consequently `muse-acp` does not advertise MCP capabilities and rejects a
non-empty `mcpServers` array rather than silently dropping tools or leaking a
server into unrelated sessions. Muse itself supports configuration/plugin-based
MCP servers, but those are host-level surfaces; they cannot safely represent
ACP's inline, session-scoped transports in this persistent multi-session
gateway. This is a Muse MSP integration gap, not an intentional product limit.

## Model and reasoning-effort selection

Sessions expose two ACP configuration options:

- ID: `muse.model` in ACP v2 (`id: "muse.model"` in ACP v1)
- values: `provider-hex:profile-hex:model-hex`
- ID: `muse.reasoningEffort` in ACP v2 (`id: "muse.reasoningEffort"` in ACP v1)
- category: `thought_level`
- values: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `ultra`

Hex encoding makes the value opaque and safe even when provider, profile, or
model IDs contain `:` or non-ASCII characters. Setting the option sends an MSP
`session/setModel` request and emits a configuration update when Muse confirms
the selection.

Reasoning effort is mandatory at the adapter boundary: every session has a
selected value, and every MSP `turn/start` request contains it. New sessions
default to `medium`; clients can change it through the normal ACP
`session/set_config_option` flow. The value is not written to Muse's durable
session metadata because MSP models `reasoningEffort` as a per-turn submission
field, so a resumed session starts at `medium` unless the client selects another
tier.

## Running

Build and let an ACP client spawn the executable over stdio:

```sh
cargo build --release
./target/release/muse-acp
```

Stdout is reserved for protocol traffic. Diagnostics from `muse-acp` and Muse
are written to stderr.

### Environment

| Variable | Default | Meaning |
| --- | --- | --- |
| `MUSE_CLI` | `muse` | Executable to run as `MUSE_CLI serve` |
| `MUSE_SERVE_EXTRA_ARGS` | empty | Extra arguments inserted after `serve`; split on whitespace |
| `MUSE_PROVIDER` | unset | Development/smoke-test initial `providerId` |
| `MUSE_MODEL` | unset | Development/smoke-test initial `modelId` |

For example:

```sh
MUSE_CLI=muse \
MUSE_SERVE_EXTRA_ARGS='--no-session-log' \
./target/release/muse-acp
```

The arguments are intended for Muse CLI flags, not shell syntax. A value such
as `--flag "value with spaces"` cannot be represented by the simple whitespace
splitter.

## Testing

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- --deny warnings
```

Unit tests cover prompt conversion, model value round trips, reasoning-effort
configuration, item/delta mapping, plan updates, and usage mapping. Integration
tests use a fake MSP host to check both protocol versions, event ordering,
reasoning-effort forwarding, v2 prompt acceptance, v1 turn-completion behavior,
steering (including idle `promptRequired` semantics), terminal authentication,
logout, durable session deletion, and cancellation.

A useful real-host smoke test is:

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":2,"info":{"name":"smoke","version":"0"},"capabilities":{}}}' |
MUSE_SERVE_EXTRA_ARGS=--no-session-log ./target/debug/muse-acp
```

`--no-session-log` is not required by the adapter. It is useful in restricted
sandboxes where Muse cannot create its durable session-lock files. Normal use
should let Muse write durable sessions.

## Documentation and compatibility

Concept documentation and generated MSP references are available at:

- <https://meta-models.github.io/muse-code-sdk/guides/msp-concepts/>
- <https://meta-models.github.io/muse-code-sdk/guides/msp-wire/>
- <https://meta-models.github.io/muse-code-sdk/generated/msp/methods/>

Muse MSP schemas are versioned and can differ between preview docs and an
installed binary. The adapter intentionally follows the wire contract supported
by the installed host. Consult the host's generated schema when investigating a
version-specific incompatibility.

## Current limits

- One persistent Muse child serves the entire ACP connection.
- One workspace root per session; additional roots are rejected.
- Inline MCP server transport is blocked on Muse MSP adding a per-session
  registration surface (see the compatibility note above).
- Replay supports ACP's `{"type":"start"}` cursor; custom replay cursors are
  rejected rather than guessed.
- Reasoning effort defaults to `medium` when a session is resumed because MSP
  does not persist a session-level effort setting.
- Unknown MSP transcript item kinds are ignored rather than fabricated as ACP
  tool calls.
