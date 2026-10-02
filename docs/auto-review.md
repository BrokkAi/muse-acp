# Auto-review

Auto-review sends every Muse permission request to a reviewer agent instead of
to you. The reviewer approves or denies the action and explains why. It is off
by default, scoped to one editor session, and shown as an ACP `configOptions`
selector.

## How it works

The reviewer runs on its own `muse serve` host launched with
`--no-session-log --disable-write --disable-shell`. Reviews are memory-only:
they never appear in your saved sessions and cannot change files or run
commands. The reviewer can still read files to gather evidence.

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
- Deny: the adapter answers with the host's reject choice and passes the
  rationale as feedback when the host accepts feedback.
- Reviewer failure or unusable output: deny. The action does not run, and the
  reason is logged.

Every decision is logged to the adapter's stderr:

    [muse-acp] auto-review allow ap-1: routine workspace edit
    [muse-acp] auto-review deny ap-2: credential exfiltration to an untrusted host

## Enabling it

Set **Auto-review** to **On** in a client that renders ACP `configOptions`.
It applies to that session and is off for every new, loaded, resumed, or
forked session. Turning it off restores the normal prompt immediately for
later approvals.

## Limits

- The reviewer is a model; it can make mistakes. The sandbox, approval mode,
  and host policy remain the enforcement layer.
- Reviews cost an extra model call per permission request.
- The reviewer can read the workspace but cannot write, run commands, or use
  the network.
