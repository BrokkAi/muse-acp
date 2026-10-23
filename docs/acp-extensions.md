# ACP extension contract

This page is the client-facing contract for Muse-specific ACP extensions and
metadata. It covers both protocol versions. Standard ACP fields take precedence
whenever they can express the same fact.


## Negotiation

ACP v1 clients advertise capabilities under
`initialize.params.clientCapabilities`; ACP v2 clients use
`initialize.params.capabilities`. Muse-specific capability opt-ins use
`_meta.muse`.

| Capability | Direction | Meaning |
| --- | --- | --- |
| `_meta.muse.capabilities: ["computerUse"]` | Client to agent | Request the Muse computer-use host grants and `_session/computerUse`. |
| `_meta.muse.capabilities: ["hooks"]` | Client to agent | Enable `_session/hooks`. |
| `_meta.muse.capabilities: ["plugins"]` | Client to agent | Enable `_session/plugins`. |
| `_meta.muse.capabilities: ["readOutput"]` | Client to agent | Enable `_session/readOutput`. |
| `_meta.muse.capabilities: ["sideChat"]` | Client to agent | Enable `_session/sideChat`. |
| `_meta.muse.capabilities: ["userShell"]` | Client to agent | Allow `_session/userShell`; AIR `asyncTasks` and a host grant are also required. |
| `_meta.muse.computerUse: true` | Client to agent | Legacy boolean spelling accepted for computer use. |
| `_meta.steering.supported: true` | Agent to client | Advertises the `_session/steering` convention. |
| `_meta.steering.idleBehaviors: ["promptRequired"]` | Agent to client | Advertises that a steer sent with `idleBehavior: "promptRequired"` while no turn is running is returned as `promptRequired` instead of starting a turn. |
| `_meta.jetbrains.air` | Both | JetBrains AIR capabilities. This namespace belongs to the AIR extension, not Muse metadata. |

ACP v1 clients also advertise standard compaction support with
`clientCapabilities.session.compaction: {}`. ACP v2 needs no compaction
capability.


## Extension methods

Extension method names begin with `_`. Unsupported methods return JSON-RPC
method-not-found; clients must negotiate the corresponding capability before
calling one.

| Method | Enabled by | Purpose and fallback |
| --- | --- | --- |
| `_session/steering` | `_meta.steering.supported` | Inject text into an exact turn. The ecosystem `session/inject` proposal remains the future standard. |
| `_session/readOutput` | `muse.capabilities: ["readOutput"]` | Read host-stored tool output by reference. |
| `_session/hooks` | `muse.capabilities: ["hooks"]` | Read installed hook handlers. |
| `_session/sideChat` | `muse.capabilities: ["sideChat"]` | Create a side session from a loaded session. |
| `_session/plugins` | `muse.capabilities: ["plugins"]` | Read installed plugin rows. |
| `_session/computerUse` | `muse.capabilities: ["computerUse"]` | Read or update computer-use setup and settings. |
| `_session/userShell` | `muse.capabilities: ["userShell"]` plus AIR `asyncTasks` and host grant | Run a shell command outside a turn. |
| `_session/async_task/stop` | AIR `asyncTasks` | Stop one background task. Without it, `session/cancel` maps to host stop-all behavior. |
| `_session/hooks`, `_session/plugins`, and other reads | Their listed opt-ins | When unavailable, use ordinary prompt commands only where the README documents them; otherwise the surface stays absent. |

### Compatibility methods

| Method | Status | Behavior |
| --- | --- | --- |
| `session/set_model` | Non-standard. The legacy unstable `SetSessionModelRequest` (ACP v0.10), absent from every current v1 and v2 schema. | Accepts `{sessionId, modelId}`, forwards the id to Muse `session/setModel`, and answers `{"model": modelId}`; Muse's `session/modelChanged` then moves the standard `model` selector through `config_option_update`. New clients should use `session/set_config_option` with the `model` option instead. The adapter never returns the legacy `models` session state, so clients that follow that unstable feature do not call it; it remains for older clients and scripts. |

Subagent updates use the ACP subagent RFD through
`capabilities.session.subagents` or AIR `nativeSubagentSessions`. Without
negotiation they fall back to synthesized tool cards.


## Lifecycle request metadata

| Location | Direction | Shape and behavior |
| --- | --- | --- |
| `session/load` or `session/resume` `params._meta.muse.permissionProfile` | Client to agent | Non-empty host-catalog profile id. Forwarded to Muse `session/resume`; unknown/unavailable ids keep the host's typed `commandRejected`. `session/fork` never forwards it. |
| `session/load` or `session/resume` `params._meta.mspSessionId` | Client to agent | Legacy recovery id for a session stored under an adapter-generated ACP id. Omit for normal modern sessions. |
| `session/load`, `session/resume`, `session/new`, and `session/fork` result `_meta.mspSessionId` | Agent to client | The durable Muse session id. Modern ACP `sessionId` normally already equals it; this is primarily compatibility metadata. |
| `session/fork` request `_meta.jetbrains.air.forkPoint` | Client to agent | AIR cut-point selector. It is resolved against host history before a fork command is sent. |
| Prompt `_meta.jetbrains.air.agentFileChangeReportRequest` | Client to agent | Requests one AIR file-change report for the turn. |
| `_session/steering` `_meta.steering.idleBehavior` | Client to agent | `promptRequired`: when no turn is running, return `{"outcome":"promptRequired","reason":"noRunningTurn"}` instead of starting a turn, so the client can send the text as a prompt. Other values are rejected. |

`session/new` does not accept the Muse resume profile extension. Host
permission profiles are separate from ACP `approvalMode`.


## Lifecycle result metadata

| Location | Shape | Meaning |
| --- | --- | --- |
| `session/new`, `session/load`, `session/resume`, `session/fork` result `_meta.mspSessionId` | string | Compatibility id described above. |
| Fork result `_meta.muse.forkedFrom` | object | Host fork provenance. |
| Side-chat result / attached side session `_meta.muse.sideFrom` | object | Host side-chat provenance. |
| Config option `_meta.jetbrains.air.recommendedValue` | string | AIR recommendation metadata; it never changes the selected value. |
| Auth method `_meta["terminal-auth"]` | `{label, command, args}` | Legacy terminal-auth description used with older editors. |


## Session information metadata

ACP `session_info_update` is the shared surface for live session facts.
In ACP v2, `_meta` patches by top-level key. Muse therefore publishes the
complete current `muse` object whenever any Muse member changes. Clients must
not deep-merge partial `muse` objects; they replace the stored `muse` value with
the value from the latest frame.

| Key | Shape | Meaning |
| --- | --- | --- |
| `_meta.goal` | object or `null` | Provider-neutral goal display state. `null` is an authoritative clear. Muse 1.4.11+ goals carry `createdAtMs` and `updatedAtMs` (epoch milliseconds) verbatim from the durable goal row; a goal whose producer never recorded them omits them, never reporting zero. |
| `_meta.muse.branch` | object or `null` | Branch/VCS/workspace observation from the host. |
| `_meta.muse.status` | string | Host session status. ACP v2 also receives standard `state_update`. |
| `_meta.muse.attention` | array of strings | Known host attention flags. Unknown flags are ignored. |
| `_meta.muse.nameOrigin` | `automatic` or `custom` | Proven title origin. Unknown origins are omitted. |
| `_meta.muse.generatedName` | string or `null` | Host-generated short title. `null` withdraws a previously published value. |
| `_meta.muse.sideFrom` | object | Side-chat parent provenance. |
| `_meta.muse.forkedFrom` | object | Fork provenance. |
| `_meta.muse.approvalReviewing` | array or `null` | Native reviewer activity rows. `null` clears the prior rows when none remain. |
| `_meta.muse.foregroundCompleted` | object or `null` | `turnId` and `blockingAgents` while foreground work is done but background checks hold the prompt open. `null` clears the hint at the terminal. |
| `_meta.muse.modelRetry` | object or `null` | Current model-attempt backoff details. `null` clears the hint at the turn terminal. |
| `_meta.muse.subscriptionUsage` | object or `null` | Host subscription observation, separate from billing and ACP context usage. `null` clears a prior value. The legacy `_meta.museSubscriptionUsage` alias is also emitted. |

The complete `muse` object can contain any combination of the keys above.
`session/list` rows may also carry `_meta.muse.branch` and
`_meta.muse.attention`; list rows are pull-only snapshots, not patch targets.


## Commands, messages, and tool cards

Standard ACP fields carry standard meaning:

- session title: `session_info_update.title`
- available-command selector: `availableCommands[].name`
- tool identity: `ToolCall.name`
- display title: `ToolCall.title`
- tool category and lifecycle: `ToolCall.kind` and `ToolCall.status`
- invocation input: `ToolCall.rawInput`
- streamed tool output on ACP v2: each `item/delta` on a tool or shell card's
  `output` field is appended with `tool_call_content_chunk`, up to the
  adapter's display budget (`MUSE_TOOL_OUTPUT_LIMIT`); the completion still
  replaces the card's whole content with the bounded final output, so the
  result matches a replacement alone. ACP v1 cards show output at completion
- shell output on ACP v2: a `bash` (or `shell`) tool call or user shell that
  runs gets an agent-owned terminal `term-<itemId>`. The card's content becomes
  a `terminal` reference; `terminal_update` carries the command (and `cwd`
  only for an explicit absolute-resolvable `workdir`), streamed output
  arrives as base64 `terminal_output_chunk` within the display budget, and the
  final `terminal_update` replaces the output with the bounded final text and
  sets `exitStatus`: a user shell's `exitCode`, with its signal number under
  `exitStatus._meta.muse.exitSignal` because Muse never names signals; a shell
  tool call records no exit code, so its exit status is empty. A call refused
  or cancelled before it ran keeps its text card
- file edits on ACP v2: a `diff` content item after the card's text output,
  built from the host's stored patch (`patchRef`, `kind: "tool_patch"`, read
  through `item/readOutput` when the call completes). `changes` lists each
  file as `modify`, or as `add` when a whole-file write (`write_file`,
  `create_file`) replaced no prior lines; `patch` is `git_patch` text with
  absolute paths. Muse's stored patch records neither file modes nor a
  missing final newline, so the patch carries no mode lines or
  `\ No newline at end of file` markers, and rewriting an existing empty file
  reads as an add. Calls without a stored patch, history replays, and ACP v1
  (whose `oldText`/`newText` need whole-file contents the patch does not
  hold) keep the text card
- follow-along file locations: `ToolCall.locations`, as absolute paths taken
  only from the explicit path arguments of Muse's file tools (`read_file`,
  `write_file`, `edit_file`, and the older single-path and move tools);
  shell, search, MCP, and unknown tools carry none
- context usage and cost: `usage_update.used`, `.size`, and `.cost`

Muse adds only facts with no standard ACP field:

| Location | Muse metadata |
| --- | --- |
| Skill command | `_meta.muse.title`: declared or host display name. The command's `name` remains the selector. |
| Agent-message update | `_meta.muse.messagePhase`: `commentary` or `finalAnswer`. |
| Agent-message update | `_meta.muse.cutOff: true` plus `_meta.muse.itemId` when the host committed a partial reply after a provider-stream failure. |
| Tool/host card | `_meta.muse.itemId`, `outputRef`, `patchRef`, `patchSummary`, and `truncated`. |
| Unknown or synthetic host item | `_meta.muse.itemKind`. |
| Monitor card | `_meta.muse.monitorId`, `monitorDescription`, and `truncated`. On Muse 1.4.11+, `monitorInbox: {pending, drained, voided}` counts the monitor's inbox items: admitted and not yet settled, drained to the agent, and voided by a missed wake (a drain and a void of one item settle latest-wins on their recorded stamps). |
| Subagent card | `_meta.muse.subagent: {subagentId, childSessionId, controlStatus, depth, drained}`. `drained: true` (Muse 1.4.11+) is the host's fact that the parent consumed the result, also stated in the card text; it is absent until then and never cleared. |
| Workflow card | `_meta.muse.workflow: {workflowRunId, entryId, scriptId, triggerSource, paused, resumeFromRunId}`. |
| Hook card | `_meta.muse.hookRun: {hookKey, event, runStatus, reason, handlerKind, enabled}`. |
| User-shell card | `_meta.muse.userShell: {exitCode, exitSignal}`. |
| Reminder card | `_meta.muse.reminderChild: {reminderAgentId, generationId, taskId, childSessionId, childSessionLogPath}`. |
| Background-check card | `_meta.muse.turnId` and `blockingAgents`. |
| Damaged session-log range card (Muse 1.4.11+) | `_meta.muse.damagedRange: {missingRecords, unreadableLines, readableBeforeAt, readableAfterAt, lastGoodTurnId}`, verbatim. `lastGoodTurnId` is the safe fork cut. |
| Computer-use permission request (Muse 1.4.12+) | `toolCall._meta.muse.cuaCardFacts: {page, windowTitle, steps}`, verbatim, for editors that render the card natively. The same facts are always shown as the request's text `content`: the page host (with its `xn--` form when it differs), the quoted window title, and each step as `Step N: <action> <roleWord> "<label>" — <effect>`. The window title and labels are the app's own text, so in the content they are one sanitized line of at most 200 characters with Markdown metacharacters backslash-escaped; the verbatim metadata is unsanitized. |
| Scheduled-fire card (Muse 1.4.11+) | `_meta.muse.scheduledFire: {jobId, deliveryNumber, scheduledFireAtMs, nextFireAtMs}`, verbatim epochs in milliseconds. |

### Notices

One-shot messages that need no clearing and are not session history use the
standard `notice` update: always on ACP v2, and on ACP v1 only after
`clientCapabilities.session.notices: {}`; otherwise the same text arrives as
agent text. Today that is the note that Muse declined to retract a submitted
message (`severity: "info"`). Persistent hints (`modelRetry`,
`foregroundCompleted`, `approvalReviewing`) stay in clearable
`session_info_update._meta.muse` rows, because a notice cannot be cleared.
Auto-review denials, the deny-safe permission fallback, and a rejected
approval decision stay in the transcript: they explain why an action did not
happen, and an ACP v2 client may ignore notices.

`_meta.muse.truncated` has either `{source: "host"}` or the adapter truncation
facts `{source, originalChars, retainedChars, headChars, tailChars}`.


## Context compaction

ACP compaction is a standard session-update entity:

| Protocol | Behavior |
| --- | --- |
| ACP v2 | Always uses standard `compaction_update`. |
| ACP v1 with `clientCapabilities.session.compaction: {}` | Uses standard `compaction_update`. |
| ACP v1 without the capability | Uses a legacy think-kind tool card with `_meta.contextCompaction: {version: 1}`. |

The standard update uses the Muse item id as `compactionId` and maps the host
operation to `in_progress`, `completed`, `failed`, or `cancelled`. Muse exposes
no user-visible retained summary on this event, so `summary` is omitted rather
than synthesized. An unknown future compaction status is sent as the open
extension status `_museUnknown`; it is never guessed to be success or failure.
`outcome: "noop"` uses `_museNoop` rather than claiming that context was
compacted.

Host provenance rides
`_meta.muse.contextCompaction: {version, status?, outcome?, reason?, trigger?,
strategyId?, summarizedThrough?, tokensBefore?, tokensAfter?}`.


## Error data

A host refusal that reaches the editor keeps Muse's typed data where the
adapter forwards it (`commandRejected`, `capabilityRequired`, and similar
kinds). On Muse 1.4.11+, an error that carries a complete recovery
observation also has `data.recoveryObservation` with the construction's
`constructionId`, `sessionId`, `requestedMode`, `effectiveMode`,
`effectiveSource`, and `qualification`, verbatim. It describes whether
recovery was available for that construction; it is never an acceptance
receipt for the failed request. An incomplete observation is dropped.


## Usage metadata

ACP context occupancy and cumulative cost use standard `usage_update.used`,
`.size`, and `.cost`.

| Location | Key and meaning |
| --- | --- |
| `usage_update._meta.muse.cumulative` | Counted-once prompt/output/total/cache totals. |
| `usage_update._meta.muse.pressure` | Host context-pressure label. |
| `usage_update._meta.muse.subscriptionUsage` | Host subscription observation. |
| `usage_update.cost._meta.muse` | Cost provenance: host-reported versus adapter catalog estimate, partial and billing flags. |
| Prompt-result `usage._meta` | `muse.modelCalls`, optional `apiDurationMs`, and per-model `modelUsage`; `mjolnir.dev/usage-scope: "turn"` is a compatibility discriminator. |

The older top-level `museCumulative`, `musePressure`, and
`museSubscriptionUsage` spellings remain compatibility aliases in the same
frames. New clients should use the nested `muse` members.


## Standard mapping status

| Muse concept | Standard ACP surface | Remaining extension |
| --- | --- | --- |
| Session title and generated title | `session_info_update.title` | `generatedName`, `nameOrigin` |
| Session activity | ACP v2 `state_update` | host `status` and `attention` details |
| Tool identity and display | `ToolCall.name`, `.title`, `.kind`, `.status`, `.rawInput` | host item references and synthetic-card provenance |
| Context and cost | `usage_update.used`, `.size`, `.cost` | cumulative/subscription details |
| Compaction | `compaction_update` with negotiated v1 fallback | legacy v1 tool card and host provenance |
| Permission requests | `session/request_permission` | `approvalReviewing` activity rows |
| User questions | `elicitation/create` when negotiated, with titled fields and choices | option previews in a choice's `_meta.muse.preview` (`{content, format}`) |
| Turn steering | No stable standard method yet | `_session/steering` |
| Goals and branch | No standard session method/field | `_meta.goal`, `_meta.muse.branch` |
