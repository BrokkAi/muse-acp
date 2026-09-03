#!/usr/bin/env python3
"""Fake `muse serve` MSP host for the committed ACP integration test.

Speaks just enough MSP JSON-RPC (line-delimited, over stdin/stdout) for the
adapter handshake and scripted turn scenarios. The scenario is chosen with
FAKE_SCENARIO; after answering `turn/start` the fake emits the scenario's
server-initiated notifications, then stays alive until stdin closes.

Scenarios:
  happy      agentMessage completion, then turn/completed(completed)
  failed     no message, then turn/completed(failed)
  tool       toolCall completion with result text, then completed
  approval   approval/requested with two choices, then completed
  questions  userInput/requested with options, then completed
"""
import json
import os
import sys

FP = "sha256:03312c213efd14277a0e0a102f70adeae497a469ca4edf7242f479953ed758b7"
MSP_SID = "msp-sess-1"
SCENARIO = os.environ.get("FAKE_SCENARIO", "happy")


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def events_for_turn():
    base = {"sessionId": MSP_SID, "turnId": "turn-1"}
    if SCENARIO == "happy":
        return [
            ("item/completed", {**base, "item": {
                "itemId": "it-1", "kind": "agentMessage",
                "status": "completed", "text": "hello from fake host"}}),
            ("turn/completed", {**base, "terminal": "completed"}),
        ]
    if SCENARIO == "failed":
        return [("turn/completed", {**base, "terminal": "failed"})]
    if SCENARIO == "tool":
        return [
            ("item/completed", {**base, "item": {
                "itemId": "it-t1", "kind": "toolCall", "callId": "call-1",
                "status": "completed", "tool": "read",
                "args": {"path": "/tmp/x"}, "result": "file bytes"}}),
            ("turn/completed", {**base, "terminal": "completed"}),
        ]
    if SCENARIO == "approval":
        return [
            ("approval/requested", {
                "sessionId": MSP_SID, "approvalId": "ap-1",
                "toolCallId": "call-1",
                "availableChoices": [
                    {"choiceId": "c-allow", "label": "Allow",
                     "decision": "approved", "scope": "once"},
                    {"choiceId": "c-deny", "label": "Deny",
                     "decision": "denied", "scope": "once"},
                ]}),
            ("turn/completed", {**base, "terminal": "completed"}),
        ]
    if SCENARIO == "questions":
        return [
            ("userInput/requested", {
                "sessionId": MSP_SID, "userInputId": "ui-1",
                "questions": [{
                    "id": "q0", "header": "Pick", "question": "Which?",
                    "selection": {"mode": "single"},
                    "options": [{"label": "Alpha"}, {"label": "Beta"}],
                }]}),
            ("turn/completed", {**base, "terminal": "completed"}),
        ]
    return [("turn/completed", {**base, "terminal": "completed"})]


def result_for(method):
    if method == "initialize":
        return {"schema": {"fingerprint": FP}, "capabilities": {}}
    if method == "session/start":
        return {"session": {"sessionId": MSP_SID, "modelId": "fake-model"},
                "viewCursor": "cur-0"}
    if method == "model/list":
        return {"models": [{"modelId": "fake-model",
                             "displayLabel": "Fake"}]}
    if method == "turn/start":
        return {"turnId": "turn-1"}
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
        if method == "initialized":
            continue
        if method:
            if ident is not None:
                send({"jsonrpc": "2.0", "id": ident,
                      "result": result_for(method)})
            if method == "turn/start":
                for m, p in events_for_turn():
                    notify(m, p)


main()
