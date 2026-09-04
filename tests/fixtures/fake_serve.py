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
  queued       1st turn/start: silence; 2nd: completed(TURN_1), completed(TURN_2)
  unqueued     turn/unqueued for the turn (never runs)
  quiet        turn/start answers only; nothing follows (for close/cancel)
  load         session/resume serves inline history (for session/load replay)
  resume_active session/resume reports a running turn (for steering reattach)
"""
import json
import os
import sys

FP = "sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7"
MSP_SID = "msp-sess-1"
SCENARIO = os.environ.get("FAKE_SCENARIO", "happy")
MODE = os.environ.get("FAKE_MODE", "promptUnmatched")
LOG = os.environ.get("FAKE_LOG", "")
TURNS = [0]

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


def session_obj():
    return {"sessionId": MSP_SID, "modelId": "fake-model",
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


def on_turn_start(params):
    tid = turn_id()
    base = {"sessionId": MSP_SID, "turnId": tid}
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
    elif SCENARIO == "approval":
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
    elif SCENARIO == "questions":
        notify("userInput/requested", {
            "sessionId": MSP_SID, "userInputId": "ui-1",
            "questions": [{
                "id": "q0", "header": "Pick", "question": "Which?",
                "selection": {"mode": "single"},
                "options": [{"label": "Alpha"}, {"label": "Beta"}],
            }]})
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
        return {"schema": {"fingerprint": FP}, "capabilities": {}}
    if method == "session/start":
        return {"session": session_obj(), "viewCursor": "cur-0"}
    if method == "session/resume":
        return {"session": session_obj(), "viewCursor": "cur-9",
                "pendingRequests": [],
                "history": {"mode": "inline", "items": history_items(),
                            "snapshot": None}}
    if method == "session/setApprovalMode":
        return {"commandId": "x", "status": "ok", "applyOutcome": "applied",
                "effectiveMode": {"lastCommandId": "x", "mode": MODE,
                                  "source": "explicit"}}
    if method == "model/list":
        return {"models": [{"modelId": "fake-model",
                             "displayLabel": "Fake"}]}
    if method == "turn/start":
        params = msg.get("params", {})
        log_input(params)
        return on_turn_start(params)
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
        if method == "initialized":
            continue
        if method:
            if ident is not None:
                send({"jsonrpc": "2.0", "id": ident,
                      "result": result_for(method, msg)})


main()
