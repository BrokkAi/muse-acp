#!/usr/bin/env python3
"""Fake `muse serve` MSP host for the committed ACP integration test.

Speaks just enough MSP JSON-RPC (line-delimited, over stdin/stdout) for the
adapter handshake, then plays one scripted scenario per run. FAKE_SCENARIO
selects the script; FAKE_MODE sets the folded host approval mode
(default promptUnmatched); FAKE_LOG (optional) records received methods.

Scenarios (TURN_N = incrementing turn id per turn/start):
  happy        agentMessage completion, then turn/completed(completed)
  failed       no message, then turn/completed(failed)
  tool         toolCall completion with result text, then completed
  approval     approval/requested notification (two choices), then completed
  approval_req approval/request server-initiated REQUEST (no notification)
  questions    userInput/requested with options, then completed
  questions_resume reissue a pending question after both attach and usage backfill
  queued       1st turn/start: silence; 2nd: completed(TURN_1), completed(TURN_2)
  unqueued     turn/unqueued for the turn (never runs)
  quiet        turn/start answers only; nothing follows (for close/cancel)
  load         session/resume serves inline history (for session/load replay)
  resume_active session/resume reports a running turn (for steering reattach)
  catalog_grows model/list expands after the first snapshot
  catalog_refresh_failure valid catalog, malformed response, RPC error, empty catalog
  usage_gap    view/gap refill page overlapping a live completion
  usage_rates_dropped priced catalog, then a refresh whose model has no cost
  usage_rates_empty   ... then a refresh returning models: []
  usage_rates_invalid ... then a refresh whose rates do not parse
  usage_rates_failure ... then a FAILED refresh (rates must survive)
  usage_resume session/resume serves an anchoredSnapshot carrying usage
  usage_snapshot_null snapshot whose contextUsage is null (the common shape)
  usage_inline inline by default; the explicit snapshot rung carries usage
  usage_inline_nosnapshot every rung downgrades; only the durable page has
               totals, and contextUsage is never durable (as on the real host)
"""
import json
import os
import sys
import time

FP = "sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7"
SCHEMA = {"fingerprint": FP, "version": 1}
MSP_SID = "msp-sess-1"
SCENARIO = os.environ.get("FAKE_SCENARIO", "happy")
MODE = os.environ.get("FAKE_MODE", "promptUnmatched")
LOG = os.environ.get("FAKE_LOG", "")
TURNS = [0]
CATALOG_READS = [0]

# Compatibility-diagnostics knobs: the fixture defaults to the validated
# host shape, but tests can present an unknown fingerprint or a future
# envelope schema version.
if os.environ.get("FAKE_FINGERPRINT", ""):
    SCHEMA["fingerprint"] = os.environ["FAKE_FINGERPRINT"]
if os.environ.get("FAKE_SCHEMA_VERSION", ""):
    SCHEMA["version"] = int(os.environ["FAKE_SCHEMA_VERSION"])

APPROVAL_PARAMS = {
    "sessionId": MSP_SID, "approvalId": "ap-1", "toolCallId": "call-1",
    "toolName": "workspace-shell",
    "subject": {"kind": "shell", "command": "cargo test"},
    "availableChoices": [
        {"choiceId": "c-allow", "label": "Allow",
         "decision": "approved", "scope": "once"},
        {"choiceId": "c-deny", "label": "Deny",
         "decision": "denied", "scope": "once"},
    ],
}
if os.environ.get("FAKE_APPROVAL_SUBJECT", "") == "file-write":
    APPROVAL_PARAMS["toolName"] = "workspace-files"
    APPROVAL_PARAMS["subject"] = {
        "kind": "fileAccess", "access": "write", "path": "/tmp/output.txt",
    }
if os.environ.get("FAKE_APPROVAL", "") == "all-approve":
    APPROVAL_PARAMS["availableChoices"] = [
        {"choiceId": "c-yes", "label": "Yes",
         "decision": "approved", "scope": "once"},
        {"choiceId": "c-always", "label": "Always",
         "decision": "approved", "scope": "session"},
    ]


def log_method(method):
    if LOG:
        with open(LOG, "a") as f:
            f.write(method + "\n")


def log_input(params):
    path = os.environ.get("FAKE_INPUT", "")
    if path:
        with open(path, "a") as f:
            f.write(json.dumps(params) + "\n")


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def turn_id():
    TURNS[0] += 1
    return f"turn-{TURNS[0]}"


# The session notifications currently target; a fork switches it so turns
# started on the forked session complete on the forked session.
ACTIVE_SESSION = [MSP_SID]


def session_obj(session_id=None, workspace_root="/tmp/fake-ws"):
    if session_id is None:
        session_id = ACTIVE_SESSION[0]
    return {"sessionId": session_id, "modelId": "fake-model",
            "workspaceRoot": workspace_root,
            "activeTurnId": "turn-resumed" if SCENARIO == "resume_active" else None,
            "approvalMode": {"lastCommandId": None, "mode": MODE,
                             "source": "serverDefault"}}


def history_items():
    return [
        {"itemId": "h-1", "kind": "userMessage", "text": "old question"},
        {"itemId": "h-2", "kind": "agentMessage", "text": "old answer"},
        {"itemId": "h-3", "kind": "toolCall", "callId": "h-call",
         "status": "completed", "tool": "read",
         "args": {"path": "/tmp/h"}, "result": "old bytes"},
    ]


def token_usage(cursor, prompt, output, cumulative_prompt, cumulative_output):
    """One `session/tokenUsage` completion leg, identified by view cursor."""
    return {"sessionId": MSP_SID, "turnId": f"turn-{TURNS[0]}",
            "promptTokens": prompt, "totalTokens": prompt + output,
            "modelId": "fake-model",
            "usage": {"inputTokens": prompt, "outputTokens": output,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": cursor,
            "sourceRange": {"start": 0, "end": int(cursor.split("-")[1])},
            "cumulative": {"promptTokens": cumulative_prompt,
                           "outputTokens": cumulative_output,
                           "totalTokens": cumulative_prompt + cumulative_output}}


def context_usage(used, cursor):
    return {"sessionId": MSP_SID, "usedTokens": used, "windowTokens": 200000,
            "pressure": "normal", "viewCursor": cursor,
            "sourceRange": {"start": 0, "end": int(cursor.split("-")[1])}}


def usage_snapshot_history(context=True, cumulative=(100, 20)):
    """anchoredSnapshot history whose state already knows the usage. MSP
    serves `contextUsage` as null until the fold holds a tracked anchor, so
    context=False is the ordinary shape, not an exotic one."""
    return {"mode": "anchoredSnapshot", "items": None, "snapshot": {
        "schemaVersion": 1, "viewCursor": "cur-9",
        "anchor": {"boundaryCursor": "cur-8", "summarizedThrough": "anchor-8"},
        "state": {"items": [], "activeTurn": None, "queuedTurns": [],
                  "pendingApprovals": [], "pendingUserInputs": [],
                  "approvalMode": session_obj()["approvalMode"],
                  "effectiveModel": None, "branch": None, "goal": None,
                  "todoList": None, "turnCount": 1,
                  "contextUsage": ({"usedTokens": 120,
                                    "windowTokens": 200000,
                                    "pressure": "normal"} if context else None),
                  "tokenUsage": {"promptTokens": cumulative[0],
                                 "outputTokens": cumulative[1],
                                 "totalTokens": sum(cumulative)}}}}


TODO_ITEMS = [
    {"text": "Read the schema", "status": "completed"},
    {"text": "Map todo lists", "status": "inProgress",
     "activeForm": "Mapping todo lists"},
    {"text": "Add tests", "status": "pending"},
    {"text": "Dropped task", "status": "cancelled"},
]


def question_params(user_input_id="ui-1"):
    return {"sessionId": MSP_SID, "userInputId": user_input_id,
            "turnId": "turn-question", "itemId": f"item-{user_input_id}",
            "toolCallId": f"call-{user_input_id}", "toolName": "request_user_input",
            "viewCursor": "cur-8",
            "questions": [{
                "id": "q0", "header": "Pick", "question": "Which?",
                "selection": {"mode": "single"},
                "options": [{"label": "Alpha"}, {"label": "Beta"}],
            }]}


def on_turn_start(params):
    tid = turn_id()
    base = {"sessionId": ACTIVE_SESSION[0], "turnId": tid}
    if SCENARIO == "happy":
        notify("item/completed", {**base, "item": {
            "itemId": "it-1", "kind": "agentMessage",
            "status": "completed", "text": "hello from fake host"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "failed":
        notify("turn/completed", {**base, "terminal": "failed"})
    elif SCENARIO == "tool":
        notify("item/completed", {**base, "item": {
            "itemId": "it-t1", "kind": "toolCall", "callId": "call-1",
            "status": "completed", "tool": "read",
            "args": {"path": "/tmp/x"}, "result": "file bytes"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("approval", "pending_reconcile_dup"):
        notify("approval/requested", dict(APPROVAL_PARAMS))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "approval_hang":
        # The turn stays open until the client answers or cancels.
        notify("approval/requested", dict(APPROVAL_PARAMS))
    elif SCENARIO == "approval_req":
        # Reissued multi-stage style: a server-initiated REQUEST with its
        # own id, no notification. Adapter must ack it AND bridge it.
        send({"jsonrpc": "2.0", "id": 9100, "method": "approval/request",
              "params": dict(APPROVAL_PARAMS)})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("questions", "questions_resume"):
        qid = "ui-2" if SCENARIO == "questions_resume" else "ui-1"
        notify("userInput/requested", question_params(qid))
        if SCENARIO == "questions_resume":
            # The request and view notification also describe the same ask.
            send({"jsonrpc": "2.0", "id": 9200, "method": "userInput/request",
                  "params": question_params(qid)})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "todo":
        notify("session/todoListChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-t1",
            "revision": 1, "sourceTool": "todo_write",
            "items": TODO_ITEMS})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "todo_cleared":
        notify("session/todoListChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-t1",
            "revision": 1, "sourceTool": "todo_write", "items": TODO_ITEMS})
        notify("session/todoListChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-t2",
            "revision": 2, "sourceTool": "todo_write", "items": []})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "reasoning_stream":
        rid = "it-r1"
        notify("item/started", {**base, "item": {
            "itemId": rid, "kind": "reasoning", "status": "inProgress",
            "revision": 1}})
        notify("item/delta", {"sessionId": MSP_SID, "itemId": rid,
                              "field": "summary.0", "delta": "Considering ",
                              "viewCursor": "cur-r1"})
        notify("item/delta", {"sessionId": MSP_SID, "itemId": rid,
                              "field": "summary.0", "delta": "the schema",
                              "viewCursor": "cur-r2"})
        notify("item/delta", {"sessionId": MSP_SID, "itemId": rid,
                              "field": "summary.1", "delta": "Then testing",
                              "viewCursor": "cur-r3"})
        notify("item/completed", {**base, "item": {
            "itemId": rid, "kind": "reasoning", "status": "completed",
            "revision": 2, "truncated": False,
            "summary": ["Considering the schema", "Then testing"]}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m1", "kind": "agentMessage", "status": "completed",
            "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "subagent":
        sid_item = "it-sub1"
        notify("item/started", {**base, "item": {
            "itemId": sid_item, "kind": "subagent", "status": "inProgress",
            "revision": 1, "subagentId": "sub-1", "agentPath": "researcher",
            "depth": 1, "objective": "survey failing tests",
            "childSessionId": "child-sess-1", "controlStatus": "running"}})
        notify("item/completed", {**base, "item": {
            "itemId": sid_item, "kind": "subagent", "status": "completed",
            "revision": 2, "subagentId": "sub-1", "agentPath": "researcher",
            "depth": 1, "objective": "survey failing tests",
            "childSessionId": "child-sess-1", "controlStatus": "closed",
            "result": {"summary": "child finished", "evidenceRefs": [],
                       "artifactRefs": []}}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "workflow":
        wf = "it-wf1"
        notify("item/updated", {**base, "item": {
            "itemId": wf, "kind": "workflow", "status": "inProgress",
            "revision": 2, "workflowRunId": "wfr-1", "entryId": "triage-batch",
            "scriptId": "triage@sha256:aa10", "triggerSource": "modelProposal",
            "children": [{"childId": "c1", "attempt": 1, "status": "started",
                          "phase": "triage", "label": "triage issue #1"}]}})
        notify("item/completed", {**base, "item": {
            "itemId": wf, "kind": "workflow", "status": "completed",
            "revision": 3, "workflowRunId": "wfr-1", "entryId": "triage-batch",
            "scriptId": "triage@sha256:aa10", "triggerSource": "modelProposal",
            "children": [{"childId": "c1", "attempt": 1, "status": "completed",
                          "label": "triage issue #1"}]}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usershell_item":
        notify("item/completed", {"sessionId": MSP_SID, "item": {
            "itemId": "it-sh1", "kind": "userShell", "status": "completed",
            "revision": 1, "turnId": None, "commandText": "git status",
            "exitCode": 0, "visibleOutput": "## main", "durationMs": 120},
            "viewCursor": "cur-sh1"})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m2", "kind": "agentMessage", "status": "completed",
            "text": "shell done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "unknown_kind":
        notify("item/completed", {**base, "item": {
            "itemId": "it-u1", "kind": "hologramPreview", "status": "completed",
            "revision": 1, "fallbackText": "Previewed a hologram"}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-u2", "kind": "mysteryKind", "status": "completed",
            "revision": 1}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m3", "kind": "agentMessage", "status": "completed",
            "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "goal_branch":
        notify("session/goalChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-g1",
            "goal": {"objective": "Green the suite", "status": "active",
                     "percentComplete": 42,
                     "currentWork": "Fixing fold tests",
                     "nextWork": "Re-run CI"}})
        notify("session/branchChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-b1",
            "branch": "feat/msp", "vcs": "git",
            "workspaceRoot": "/home/me/src/proj"})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "goal_clear":
        notify("session/goalChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-g1",
            "goal": {"objective": "Old", "status": "active",
                     "percentComplete": 10}})
        notify("session/goalChanged", {
            "sessionId": MSP_SID, "viewCursor": "cur-g2", "goal": None})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "reasoning_quiet":
        # No deltas: the completed summary must still be emitted once.
        notify("item/completed", {**base, "item": {
            "itemId": "it-r9", "kind": "reasoning", "status": "completed",
            "revision": 1, "summary": ["Committed thought"]}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-m9", "kind": "agentMessage", "status": "completed",
            "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "queued":
        if TURNS[0] == 2:
            notify("turn/completed", {"sessionId": MSP_SID,
                                     "turnId": "turn-1",
                                     "terminal": "completed"})
            notify("turn/completed", {"sessionId": MSP_SID,
                                     "turnId": "turn-2",
                                     "terminal": "completed"})
    elif SCENARIO == "unqueued":
        notify("turn/unqueued", dict(base))
    elif SCENARIO == "usage":
        # A tokenUsage before any contextUsage has no used/size pair and
        # must be held back, not emitted with nulls.
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 100, "totalTokens": 120, "modelId": "fake-model",
            "usage": {"inputTokens": 100, "outputTokens": 20,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-0", "sourceRange": {"start": 0, "end": 1},
            "cumulative": {"promptTokens": 100, "outputTokens": 20,
                           "totalTokens": 120}})
        notify("session/contextUsage", {
            "sessionId": MSP_SID, "usedTokens": 1234, "windowTokens": 200000,
            "pressure": "normal", "viewCursor": "cur-1",
            "sourceRange": {"start": 0, "end": 1}})
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500, "modelId": "fake-model",
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-2", "sourceRange": {"start": 0, "end": 2},
            "cumulative": {"promptTokens": 5000, "outputTokens": 2500,
                           "totalTokens": 7500}})
        # Pre-schema record: no modelId, so an unpriced leg. Totals still
        # advance; the running cost must not.
        notify("session/tokenUsage", {
            "sessionId": MSP_SID, "turnId": tid,
            "promptTokens": 1000, "totalTokens": 1500,
            "usage": {"inputTokens": 1000, "outputTokens": 500,
                      "cachedTokens": 0, "reasoningTokens": 0},
            "viewCursor": "cur-3", "sourceRange": {"start": 0, "end": 3},
            "cumulative": {"promptTokens": 6000, "outputTokens": 3000,
                           "totalTokens": 9000}})
        notify("item/completed", {**base, "item": {
            "itemId": "it-1", "kind": "agentMessage",
            "status": "completed", "text": "done"}})
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_gap":
        # cur-3 is delivered twice: once by the view/gap refill page and
        # once on the live stream. Two distinct completions, one price each.
        notify("session/contextUsage", context_usage(120, "cur-1"))
        notify("view/gap", {"sessionId": MSP_SID,
                            "after": "cur-1", "next": "cur-3"})
        notify("session/tokenUsage", token_usage("cur-3", 1000, 500, 1100, 520))
        notify("session/contextUsage", context_usage(1620, "cur-4"))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO in ("usage_rates_dropped", "usage_rates_empty",
                      "usage_rates_invalid", "usage_rates_failure"):
        if TURNS[0] == 1:
            notify("session/contextUsage", context_usage(120, "cur-1"))
        notify("session/tokenUsage", token_usage(
            f"cur-{TURNS[0] + 1}", 100, 20, 100 * TURNS[0], 20 * TURNS[0]))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_inline_nosnapshot":
        # The occupancy only ever arrives live; the totals must already be
        # the ones read back from the durable page, never null.
        notify("session/contextUsage", context_usage(1500, "cur-9"))
        notify("session/tokenUsage", token_usage("cur-10", 100, 20, 400, 80))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_snapshot_null":
        # Turn 1 establishes real occupancy; turn 2 runs after a reattach
        # whose snapshot has no contextUsage, and reports no new occupancy.
        if TURNS[0] == 1:
            notify("session/contextUsage", context_usage(120, "cur-1"))
        notify("session/tokenUsage", token_usage(
            f"cur-{TURNS[0] + 1}", 100, 20, 100 * TURNS[0], 20 * TURNS[0]))
        notify("turn/completed", {**base, "terminal": "completed"})
    elif SCENARIO == "usage_resume":
        # Occupancy is unchanged after the resume, so the host emits no
        # contextUsage: only the restored window makes this leg sendable.
        notify("session/tokenUsage", token_usage("cur-10", 100, 20, 200, 40))
        notify("turn/completed", {**base, "terminal": "completed"})
    disposition = "queued" if SCENARIO == "queued" and TURNS[0] > 1 else "started"
    return {
        "commandId": params.get("commandId", ""),
        "status": "accepted",
        "turnId": tid,
        "disposition": disposition,
        "startedNewTurn": disposition == "started",
    }


def result_for(method, msg):
    if method == "initialize":
        return {
            "schema": SCHEMA,
            "capabilities": {},
            "serverInfo": {
                "name": "muse-session-server-fixture",
                "version": "0.0.0-fixture",
            },
        }
    if method == "approval/listPending":
        if SCENARIO == "pending_reconcile":
            return {"approvals": [dict(APPROVAL_PARAMS, approvalId="ap-reconcile")],
                    "userInputs": [question_params("ui-reconcile")]}
        if SCENARIO == "pending_reconcile_dup":
            # The same approval the adapter is already displaying.
            return {"approvals": [dict(APPROVAL_PARAMS)], "userInputs": []}
        return {"approvals": [], "userInputs": []}
    if method == "session/start":
        return {"session": session_obj(), "viewCursor": "cur-0"}
    if method == "session/resume":
        params = msg.get("params", {})
        log_input(params)
        # Only the explicitly requested snapshot rung can carry occupancy;
        # the default `auto` rung resolves to inline on the real host.
        snapshot_rung = params.get("history") == "snapshot"
        if SCENARIO == "usage_resume":
            history = usage_snapshot_history()
        elif SCENARIO == "todo_resume":
            history = usage_snapshot_history()
            history["snapshot"]["state"]["todoList"] = {
                "items": TODO_ITEMS, "revision": 3,
                "sourceTool": "todo_write"}
        elif SCENARIO == "goal_branch_resume":
            history = usage_snapshot_history()
            history["snapshot"]["state"]["goal"] = {
                "objective": "Snapshot goal", "status": "paused",
                "percentComplete": 5}
            history["snapshot"]["state"]["branch"] = {
                "branch": "main", "vcs": "git",
                "workspaceRoot": "/home/me/src/proj"}
        elif SCENARIO == "usage_snapshot_null":
            history = usage_snapshot_history(context=False)
        elif SCENARIO in ("usage_inline", "questions_resume") and snapshot_rung:
            history = usage_snapshot_history(cumulative=(300, 60))
        else:
            history = {"mode": "inline", "items": history_items(),
                       "snapshot": None}
        pending = []
        if SCENARIO == "questions_resume":
            pending = [{"kind": "userInput", "userInputId": "ui-1",
                        "viewCursor": "cur-8"}]
            if history.get("snapshot"):
                history["snapshot"]["state"]["pendingUserInputs"] = [
                    {"userInputId": "ui-1", "itemId": "item-ui-1",
                     "viewCursor": "cur-8"}]
        return {"session": session_obj(params.get("sessionId", MSP_SID)),
                "viewCursor": "cur-9",
                "pendingRequests": pending,
                "history": history}
    if method == "view/page":
        page = msg.get("params", {})
        if SCENARIO == "usage_gap" and page.get("direction") != "backward":
            # Refill overlaps the live stream: cur-3 is in this page too.
            return {"events": [
                {"method": "session/tokenUsage",
                 "params": token_usage("cur-2", 100, 20, 100, 20)},
                {"method": "session/tokenUsage",
                 "params": token_usage("cur-3", 1000, 500, 1100, 520)}],
                "nextCursor": "cur-3"}
        if (SCENARIO in ("usage_inline", "usage_inline_nosnapshot")
                and page.get("direction") == "backward"):
            # Ascending by viewCursor, as MSP guarantees in both directions.
            # No session/contextUsage: it is not durable-sourced, so it never
            # appears in a page -- verified against a real Muse 1.0.3 host.
            return {"events": [
                {"method": "session/tokenUsage",
                 "params": token_usage("cur-8", 300, 60, 300, 60)}],
                "nextCursor": None}
        # Every other session pages back to nothing usable.
        return {"events": [], "nextCursor": None}
    if method == "session/list":
        live = session_obj()
        old = session_obj("msp-sess-old", "/tmp/old-ws")
        old["updatedAt"] = "2026-08-01T00:00:00Z"
        live["updatedAt"] = "2026-09-04T00:00:00Z"
        return {"sessions": [live, old], "nextCursor": None}
    if method == "session/setApprovalMode":
        return {"commandId": "x", "status": "ok", "applyOutcome": "applied",
                "effectiveMode": {"lastCommandId": "x", "mode": MODE,
                                  "source": "explicit"}}
    if method == "model/list":
        CATALOG_READS[0] += 1
        if SCENARIO == "usage_rates_failure" and CATALOG_READS[0] > 1:
            return {}  # malformed: no models array, so the refresh failed
        if SCENARIO == "catalog_refresh_failure":
            if CATALOG_READS[0] == 2:
                return {}
            if CATALOG_READS[0] == 4:
                return {"models": [], "source": "unresolvedCatalog"}
        models = [{"modelId": "fake-model", "displayLabel": "Fake",
                   "cost": {"input": "3.00", "output": "15.00",
                            "cached": "0.30", "currency": "USD"}}]
        if CATALOG_READS[0] > 1:
            if SCENARIO == "usage_rates_dropped":
                models[0]["cost"] = None
            elif SCENARIO == "usage_rates_empty":
                models = []
            elif SCENARIO == "usage_rates_invalid":
                models[0]["cost"]["input"] = "NaN"
        if SCENARIO == "catalog_grows" and CATALOG_READS[0] > 1:
            models.append({"modelId": "second-model", "displayLabel": "Second"})
        return {"models": models, "source": "fakeCatalog"}
    if method == "turn/start":
        params = msg.get("params", {})
        log_input(params)
        return on_turn_start(params)
    if method == "session/read":
        read_params = msg.get("params", {})
        log_input(read_params)
        # Items carry turnId so fork points can resolve to a completed turn.
        items = history_items() + [
            {"itemId": "msg-fork", "kind": "agentMessage",
             "text": "fork here", "turnId": "turn-1", "status": "completed"},
            {"itemId": "msg-fork-2", "kind": "agentMessage",
             "text": "later answer", "turnId": "turn-2", "status": "completed"},
            {"itemId": "shell-no-turn", "kind": "userShell",
             "commandText": "git status", "turnId": None,
             "status": "completed"},
        ]
        return {"session": session_obj(read_params.get("sessionId")),
                "history": {"mode": "inline", "items": items,
                            "snapshot": None},
                "viewCursor": "cur-read", "pendingRequests": []}
    if method == "session/fork":
        params = msg.get("params", {})
        log_input(params)
        ACTIVE_SESSION[0] = "msp-sess-forked"
        forked = session_obj()
        forked["forkedFrom"] = {
            "sessionId": params.get("sessionId", MSP_SID),
            "commandId": params.get("commandId", ""),
            "cutCursor": "opaque-cut",
            "cutExplicit": "cutPoint" in params,
        }
        return {"session": forked,
                "history": {"mode": "inline", "items": [], "snapshot": None},
                "viewCursor": "cur-f1", "pendingRequests": []}
    if method == "session/compact":
        log_input(msg.get("params", {}))
        if SCENARIO == "compact_noop":
            return {"commandId": msg["params"].get("commandId", ""),
                    "status": "noop", "reason": "no_compactable_history"}
        notify("item/completed", {"sessionId": MSP_SID, "item": {
            "itemId": "it-c1", "kind": "compaction", "status": "completed",
            "revision": 1, "outcome": "compacted",
            "tokensBefore": 12000, "tokensAfter": 8000,
            "trigger": "manual"}, "viewCursor": "cur-c1"})
        return {"commandId": msg["params"].get("commandId", ""),
                "status": "accepted"}
    if method == "userInput/answer":
        log_input(msg.get("params", {}))
        return {}
    if method == "turn/steer":
        params = msg.get("params", {})
        log_input(params)
        return {
            "commandId": params.get("commandId", ""),
            "status": "accepted",
            "turnId": params.get("expectedTurnId", ""),
        }
    if method == "turn/cancel":
        # Like the real host: a cancelled turn still reports its terminal.
        params = msg.get("params", {})
        notify("turn/completed", {"sessionId": MSP_SID,
                                  "turnId": params.get("turnId", ""),
                                  "terminal": "cancelled"})
        return {}
    return {}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = msg.get("method", "")
        ident = msg.get("id")
        log_method(method or "(response)")
        if not method and ident == "srv-77":
            # The adapter's reply to the fixture's unknown server request.
            log_method("unknown-request-reply:" + json.dumps(msg))
            continue
        if method == "initialized":
            if SCENARIO == "unknown_request":
                send({"jsonrpc": "2.0", "id": "srv-77",
                      "method": "future/request",
                      "params": {"sessionId": MSP_SID, "novel": True}})
            continue
        if method:
            if ident is not None:
                if (os.environ.get("FAKE_DELAY_METHOD", "") == method
                        and os.environ.get("FAKE_DELAY_MS", "")):
                    time.sleep(int(os.environ["FAKE_DELAY_MS"]) / 1000.0)
                if (method == "model/list" and SCENARIO == "catalog_refresh_failure"
                        and CATALOG_READS[0] == 2):
                    CATALOG_READS[0] += 1
                    send({"jsonrpc": "2.0", "id": ident,
                          "error": {"code": -32603, "message": "catalog unavailable"}})
                    continue
                send({"jsonrpc": "2.0", "id": ident,
                      "result": result_for(method, msg)})
                if SCENARIO == "questions_resume" and method == "session/resume":
                    # MSP reissues pending requests after the resume response.
                    send({"jsonrpc": "2.0", "id": 9100 + ident,
                          "method": "userInput/request", "params": question_params()})
                    if msg["params"].get("history") == "snapshot":
                        # A stream barrier: both reissues precede this item.
                        notify("item/completed", {"sessionId": MSP_SID, "item": {
                            "itemId": "reissue-barrier", "kind": "agentMessage",
                            "status": "completed", "text": "resume questions delivered"}})


main()
