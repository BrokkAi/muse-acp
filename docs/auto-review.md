# Auto-review

Auto-review sends every Muse permission request to a reviewer agent instead of
to you. The reviewer approves or denies the action and explains why. It is off
by default, scoped to one editor session, and shown as an ACP `configOptions`
entry: a `type: "boolean"` toggle for ACP v2 clients and for ACP v1 clients
that advertise `clientCapabilities.session.configOptions.boolean`, and an
`off`/`on` selector for other ACP v1 clients. `session/set_config_option`
accepts a boolean value or the `off`/`on` ids for it either way.

## How it works

The reviewer runs on its own `muse serve` host launched with
`--no-session-log --disable-write --disable-shell`. Reviews are memory-only:
they never appear in your saved sessions and cannot change files or run
commands. The reviewer can still read files to gather evidence.

That host is pinned to the `low` reasoning tier
(`-c reasoning_effort="low"`), as Codex pins its guardian reviewer to `low`
whenever the model serves it. A review is a bounded yes/no judgement, so it
must not inherit a heavy session default: the reviewer used to take the Muse
default tier, and a high default spent the whole review deadline reasoning
until every approval was denied as a timeout.

`muse serve` only grew the `-c/--config` flag in Muse 1.4.4, while Auto-review
works from 1.4.2. A host that refuses the command line (exit 2, `unknown option
-c`) is retried once without the pin and logged as `reviewer host refused -c
(Muse before 1.4.4)`, so such a host still reviews, at its own default tier.
The pin is never repeated: a `reasoning_effort` a user already set through
`MUSE_SERVE_ARGS` wins, because `muse serve` refuses a setting given twice.

The reviewer prompt has six parts:

1. The fixed safety policy, adapted from Codex's guardian policy: evidence
   handling, user-authorization scoring, risk levels, and outcome thresholds.
2. Your trusted instructions: the text you sent to the session.
3. Recent evidence: bounded agent messages and tool results.
4. The review environment: workspace roots and the current approval mode.
5. The exact approval request: subject, tool, raw arguments, flags, and
   available choices.
6. The output contract: strict JSON.

The reply must be:

```json
{
  "risk_level": "low | medium | high | critical",
  "user_authorization": "unknown | low | medium | high",
  "outcome": "allow | deny",
  "rationale": "one concise sentence"
}
```

`outcome` is re-derived from the thresholds so a model that allows a critical
action cannot override the policy: critical denies; high allows only with
medium or high authorization; low and medium allow.

## Decisions

- Allow: the adapter answers with a once-scoped allowing choice when the host
  offers one, otherwise the first allowing choice.
- Deny: the adapter answers with the host's reject choice, shows
  `Permission denied by Auto-review: <rationale>` in the editor transcript,
  and passes the rationale as feedback when the host accepts feedback.
- Reviewer failure or unusable output: deny. The action does not run, the
  transcript identifies the reviewer failure, and the detailed reason is
  logged. A reviewer turn that fails denies with the host's error, for
  example `reviewer turn failed: contextWindowExceeded: prompt is too long`.
  An unusable reply says whether the reply was empty and logs a short
  excerpt. Either way the adapter resets the reviewer host before the next
  review.
- Each review runs in a fresh reviewer session, so no review inherits an
  earlier review's history. The adapter restarts the reviewer host every 32
  reviews.
- Reviewer timeout: a review whose turn never ends is denied after 180 seconds
  (twice Codex's 90-second guardian review timeout, overridable with
  `MUSE_REVIEW_TIMEOUT_MS`). The adapter logs the timeout, shows the timeout
  duration in the editor transcript, denies with a
  `reviewer timed out after <duration>` rationale, resets the reviewer session,
  and starts the next queued review.

  The deadline is a last resort, not the usual path. A reviewer turn whose
  answer is finished but held open by background reminder checks
  (`turn/foregroundCompleted`) settles as soon as its reply parses as a
  verdict, so a complete review never waits for a terminal that may not come.
  A provider backoff during the review (`turn/retryScheduled`,
  `turn/modelRetryScheduled`) is logged as `auto-review reviewer turn: …`, so a
  slow review is distinguishable from a hung one.

Every decision is logged to the adapter's stderr:

    [muse-acp] auto-review allow ap-1: routine workspace edit
    [muse-acp] auto-review deny ap-2: credential exfiltration to an untrusted host

## Enabling it

Set **Auto-review** to **On** in a client that renders ACP `configOptions`.
It applies to that session and is off for every new, loaded, resumed, or
forked session. Turning it off restores the normal prompt immediately for
later approvals.

## Native host review versus Auto-review

Muse 1.4.9+ can first hold an approval in its own native reviewer. The adapter
publishes that as `session_info_update._meta.muse.approvalReviewing`; it opens
no permission request and does not start adapter Auto-review from that
activity notice.

The layers are independent. If the host later releases the approval as
`approval/requested`, Auto-review can evaluate it when enabled; otherwise the
editor receives the permission prompt. If the host resolves it directly as
`approval/resolved`, the reviewing metadata clears and the approval never
becomes an editor decision.

## Limits

- **Muse 1.4.2 or newer.** On Muse 1.3.0 and 1.4.1 the reviewer host's own
  background reminder checks hold the review turn open, so the review never
  finishes and the approval is denied when the review timeout expires.
  Auto-review is off by default, so this only affects sessions that select
  it.
- The reviewer is a model; it can make mistakes. The sandbox, approval mode,
  and host policy remain the enforcement layer.
- Reviews cost an extra model call per permission request.
- The reviewer can read the workspace but cannot write, run commands, or use
  the network.
