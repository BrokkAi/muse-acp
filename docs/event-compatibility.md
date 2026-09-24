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

The schema includes notifications in both directions. `initialized` is the one
client-to-server entry; all other rows below are emitted by the MSP host.

<!-- schema-notifications:start -->
| MSP notification | Disposition | ACP behavior |
| --- | --- | --- |
| `approval/requested` | Mapped to ACP | Opens `session/request_permission`; duplicate approval IDs are suppressed and unusable requests fail closed. |
| `approval/resolved` | Mapped to ACP | Reasserts ACP v2 `running` state when the resolved approval unblocks active work. The permission response itself drives `approval/decide`. |
| `approval/updated` | Mapped to ACP | Reasserts ACP v2 `running` state for an updated, still-active approval flow. |
| `initialized` | Consumed | Sent by the adapter after the MSP `initialize` handshake. An unexpected inbound copy is harmlessly ignored. |
| `item/completed` | Mapped to ACP | Folds the authoritative final item into message, tool-call, async-task, subagent, or other negotiated ACP updates. |
| `item/delta` | Mapped to ACP | Streams supported item field appends as ACP message or tool-call updates. Unknown item fields and kinds are tolerated. |
| `item/started` | Mapped to ACP | Opens or upserts the corresponding ACP item and starts negotiated async-task or subagent presentation where applicable. |
| `item/updated` | Mapped to ACP | Upserts the corresponding ACP item from its higher-revision snapshot. |
| `session/approvalModeChanged` | Internally tracked | Updates the selected approval-mode value used in later ACP configuration snapshots. |
| `session/branchChanged` | Mapped to ACP | Stores the latest branch fact and emits it in `session/update` metadata together with the current goal. |
| `session/contextUsage` | Mapped to ACP | Replaces the tracked context occupancy and emits ACP `usage_update` with used tokens, window size, and Muse pressure metadata. |
| `session/goalChanged` | Mapped to ACP | Stores replacement-or-clear semantics and emits the goal in `session/update` metadata together with branch state. |
| `session/modelChanged` | Internally tracked | Updates the selected model value used in later ACP configuration snapshots. |
| `session/todoListChanged` | Mapped to ACP | Replaces the ACP plan with the reported todo list; an empty list clears the plan. |
| `session/tokenUsage` | Mapped to ACP | Deduplicates completion usage, tracks cumulative and per-turn totals, estimates catalog-priced cost when possible, and emits ACP `usage_update` once context occupancy is known. |
| `turn/completed` | Mapped to ACP | Settles the matching ACP prompt, reports its stop reason and per-turn usage, and moves ACP v2 to idle when no work remains. Failures also receive host detail. |
| `turn/retracted` | Mapped to ACP | Removes the retracted turn from tracked work and settles its ACP prompt as cancelled. |
| `turn/retryScheduled` | Consumed | Records attempt and backoff facts in diagnostics. It remains non-terminal and never settles the ACP prompt. |
| `turn/started` | Internally tracked | Marks the active MSP turn so steering, cancellation, and reconciliation target the running work. |
| `turn/unqueued` | Mapped to ACP | Removes the reclaimed queued turn from tracked work and settles its ACP prompt as cancelled. |
| `userInput/requested` | Mapped to ACP | Opens ACP `elicitation/create` when form elicitation was negotiated; otherwise sends `userInput/cancel` so the turn cannot hang. |
| `userInput/settled` | Mapped to ACP | A question this adapter answered or cancelled is already cleared locally. A form still open for a question settled elsewhere (another client, an interrupt, auto-resolution) is withdrawn with `$/cancel_request`. |
| `view/gap` | Consumed | Pages forward from the last view cursor and recursively processes the missing events, relying on fold and usage deduplication for overlap. |
<!-- schema-notifications:end -->

## Host-emitted extensions

These notifications have been observed from a host but are absent from the
published `notifications` index.

| MSP notification | Disposition | ACP behavior |
| --- | --- | --- |
| `session/started` | Intentionally ignored | The preceding `session/start` result already establishes the session and subscription, so the extra lifecycle notice carries no additional ACP state. |

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

`turn/cancel`, `approval/decide`, `userInput/answer`, `userInput/cancel`, and
`userInput/clarify` are adapter-to-host commands. They are not event rows and
are covered by request-schema conformance tests.
