# MSP event compatibility

This is the authoritative disposition matrix for the MSP stable schema pinned
under `tests/protocol/stable`. The disposition is the event's primary effect:

- **Mapped to ACP** emits an ACP response or `session/update` (including a
  negotiated ACP extension).
- **Internally tracked** changes adapter state without directly emitting an ACP
  message.
- **Consumed** is acted on without an ACP representation, for example recovery
  or diagnostics.
- **Intentionally ignored** is safe to receive but needs no action.
- **Unsupported pending protocol decision** has no safe mapping yet.

Some mapped events also update internal state. The details call out those
secondary effects.

## Published schema notifications

The schema includes notifications in both directions. `initialized` and
`userInput/engaged` are the client-to-server entries; all other rows below
are emitted by the MSP host.

<!-- schema-notifications:start -->
| MSP notification | Disposition | ACP behavior |
| --- | --- | --- |
| `approval/requested` | Mapped to ACP | Opens `session/request_permission`; duplicate approval IDs are suppressed and unusable requests fail closed. |
| `approval/resolved` | Mapped to ACP | Withdraws an outstanding permission or feedback form with `$/cancel_request`, including child-stream approvals resolved elsewhere, and reasserts ACP v2 `running` when work remains. The permission response itself drives `approval/decide`. |
| `approval/updated` | Mapped to ACP | Reasserts ACP v2 `running` state for an updated, still-active approval flow. |
| `computerUseSetup/changed` | Consumed | Logged as a diagnostic for the setup flow the opt-in `_session/computerUse` extension and `/cua` drive. The adapter keeps no CUA state, so the next read re-asks the host. |
| `initialized` | Consumed | Sent by the adapter after the MSP `initialize` handshake. An unexpected inbound copy is harmlessly ignored. |
| `item/completed` | Mapped to ACP | Folds the authoritative final item into message, tool-call, async-task, subagent, or other negotiated ACP updates. |
| `item/delta` | Mapped to ACP | Streams supported item field appends as ACP message or tool-call updates. Unknown item fields and kinds are tolerated. |
| `item/started` | Mapped to ACP | Opens or upserts the corresponding ACP item and starts negotiated async-task or subagent presentation where applicable. |
| `item/updated` | Mapped to ACP | Upserts the corresponding ACP item from its higher-revision snapshot. |
| `plugin/changed` | Consumed | Logs the installed row as a diagnostic. The adapter keeps no plugin cache, so a client re-reads the opt-in `_session/plugins` view after a change. |
| `plugin/marketplaceChanged` | Consumed | Logs the marketplace and change as a diagnostic; clients re-read `_session/plugins` instead of a local cache. |
| `recap/completed` | Mapped to ACP | The single settlement of each `recap/request` the adapter made: `completed` publishes the (title-prefixed) summary as agent text and settles the held prompt `end_turn`, `cancelled` settles it `cancelled`, and `failed`/`timedOut`/unknown outcomes surface as failures. A settlement whose correlation id matches no in-flight request is logged and ignored. |
| `session/approvalModeChanged` | Internally tracked | Updates the selected approval-mode value used in later ACP configuration snapshots. |
| `session/branchChanged` | Mapped to ACP | Stores the latest branch fact and emits it in `session/update` metadata together with the current goal. |
| `session/closed` | Consumed | Logs the unload and its reason. An unload is not a deletion: the session is `notLoaded`, its log stays on disk, and `session/resume` reloads it, so it stays in ACP `session/list`. |
| `session/contextUsage` | Mapped to ACP | Replaces the tracked context occupancy and emits ACP `usage_update` with used tokens, window size, and Muse pressure metadata. |
| `session/goalChanged` | Mapped to ACP | Stores replacement-or-clear semantics and emits the goal in `session/update` metadata together with branch state. |
| `session/listChanged` | Mapped to ACP | Replaces a cached list row when `sessionListStream` is granted; pushes a changed active-session title through `session_info_update`. |
| `session/modelChanged` | Mapped to ACP | Updates the selected model value, resets a held per-turn reasoning tier the new model does not serve, and resends the complete `configOptions` list. |
| `session/modelRouteUnserved` | Intentionally ignored | No ACP mapping; the unhandled-notification path logs a diagnostic. |
| `session/nameChanged` | Mapped to ACP | Updates host title facts and publishes a changed title through `session_info_update`. |
| `session/reasoningEffortChanged` | Mapped to ACP | Updates the standing reasoning default and emits `config_option_update`; negotiated recommendations require a host `default` or `policy` source. |
| `session/started` | Internally tracked | Caches the new session's list row when Muse grants `sessionListStream`. The row may arrive before or after the `session/start` result. Clients still request `session/list` to discover membership changes. |
| `session/statusChanged` | Mapped to ACP | Publishes host status and attention metadata and ACP v2 state; directs pending-request reconciliation without inventing unknown status values. |
| `session/todoListChanged` | Mapped to ACP | Replaces the session plan with the reported todo list (plan id `muse-todo`); an empty list clears it. ACP v1 gets `plan` with an empty `entries` array, v2 gets `plan_update` / `plan_removed`. |
| `session/tokenUsage` | Mapped to ACP | Deduplicates completion usage, tracks cumulative, cache, and per-turn totals, prefers the host's own session `cost` (with its partial flag) over the catalog estimate on hosts that report it, and emits ACP `usage_update` once context occupancy is known. |
| `session/viewHealthChanged` | Consumed | Logs the view health and reason with guidance to reattach through resume. |
| `skill/changed` | Mapped to ACP | Refreshes `skill/list` and the ACP available-command catalog. |
| `turn/completed` | Mapped to ACP | Settles the matching ACP prompt, reports its stop reason and per-turn usage, and moves ACP v2 to idle when no work remains. Failures also receive host detail. |
| `turn/foregroundCompleted` | Consumed | Explicitly non-terminal: foreground work is done while background reminder checks hold the turn open, so the ACP prompt stays open until `turn/completed` / `turn/unqueued`. ACP has no matching state; the adapter logs the turn, session, and blocking reminder-agent ids (agent ids, never display names). |
| `turn/retracted` | Mapped to ACP | Removes the retracted turn from tracked work and settles its ACP prompt as cancelled. |
| `turn/retryScheduled` | Consumed | Records attempt and backoff facts in diagnostics. It remains non-terminal and never settles the ACP prompt. |
| `turn/started` | Internally tracked | Marks the active MSP turn so steering, cancellation, and reconciliation target the running work. |
| `turn/unqueued` | Mapped to ACP | Removes the reclaimed queued turn from tracked work and settles its ACP prompt as cancelled. |
| `usage/changed` | Mapped to ACP | Broadcasts the host's subscription observation to attached sessions as `_meta.museSubscriptionUsage`, separately from cost estimates. |
| `userInput/engaged` | Intentionally ignored | A client-to-server engagement note: the user started interacting with a timed prompt, so the host disarms its auto-resolution countdown. The adapter sends no engagement notes itself; no ACP mapping, and an inbound copy would take the unhandled-notification path, which logs a diagnostic. |
| `userInput/requested` | Mapped to ACP | Opens ACP `elicitation/create` when form elicitation was negotiated; otherwise sends `userInput/cancel` so the turn cannot hang. |
| `userInput/settled` | Mapped to ACP | A question this adapter answered, cancelled, or interrupted is already cleared locally. A form still open for a question settled elsewhere (another client, an interrupt, auto-resolution) is withdrawn with `$/cancel_request`. |
| `view/gap` | Consumed | Pages forward from the bracket's `after` cursor until the walk meets `next` and recursively processes the missing events. When `next` is ephemeral (never paged) the walk runs to the end of the durable view, and live twins of the events it delivered are refused once. |
<!-- schema-notifications:end -->

## Host-emitted extensions

These notifications are handled by the adapter but are absent from the
vendored stable schema's `notifications` index. They belong to Muse's
experimental surface, which reaches the adapter because it connects with
`experimentalApi` (needed for `account/read`). The [Muse 1.3.0 disposition
matrix](../ROADMAP.md#9-richer-event-compatibility-matrix) also records host methods,
errors, and deliberate omissions.

| MSP notification | Disposition | ACP behavior |
| --- | --- | --- |
| `mcpServer/oauthLoginCompleted` | Intentionally ignored | The terminal of an MCP OAuth login, sent to every experimental connection. The adapter never starts such a login, so it logs the server name and outcome (never the free-text message) and sends nothing to the editor. |

## Server-initiated requests

These are JSON-RPC requests rather than notifications. The adapter must reply
to them as well as bridge their payloads. Any other server-initiated request is
**unsupported pending protocol decision** and receives the typed MSP
`methodNotFound` error instead of a synthetic success.

| MSP request | Disposition | Response and ACP behavior |
| --- | --- | --- |
| `approval/request` | Mapped to ACP | Replies `{}` to acknowledge handling, then opens the same deny-safe `session/request_permission` flow as `approval/requested`. |
| `userInput/request` | Mapped to ACP | Replies `{}` to acknowledge handling, then opens or safely cancels the same elicitation flow as `userInput/requested`. |

## Client commands related to events

`turn/interrupt`, `turn/unqueue`, `approval/decide`, `userInput/answer`, `userInput/cancel`,
`userInput/clarify`, and `userInput/interrupt` are adapter-to-host commands.
`userInput/interrupt` settles a pending question the user already partly
answered: the adapter stages multi-question prompts one question per form and
holds each confirmed answer, so a stop or a decline of a later question
carries the answers to the host and stops the turn. They are not event rows
and are covered by request-schema conformance tests. The same holds for the
`goal/set`, `goal/edit`, `goal/pause`, `goal/resume`, and `goal/clear`
commands, which editors reach through the adapter's `/goal` slash command
rather than as raw ACP methods (a command that wakes a goal turn keeps its
prompt open until that turn's `turn/completed`), for `session/rename`, which editors reach
through `/rename <name>`, and for `workflow/childControl`, which editors reach
through `/workflow-child skip|retry <childId>`. `workflow/pause` and
`workflow/resume` are reached through `/workflow pause|resume [<runId>]`: a
single live run may be implied, the ids come from the folded workflow items
and are never guessed, and a bare `/workflow` lists the live runs.
