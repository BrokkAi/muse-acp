#!/usr/bin/env python3
"""Loopback model provider for real `muse serve` tests (tests/live_loopback.rs).

A throwaway Muse config points `endpoint_transport.base_url` at this server,
so the host's model traffic never leaves the machine and needs no
credentials. It serves scripted Responses API streams (`POST .../responses`)
and the model catalog (`GET .../muse-code/models`), which some Muse builds
fetch and others replace with their bundled catalog.

Usage: loopback_provider.py <log file> <script file>

The script maps names to replies. A request whose last input item is a user
message containing `[[script:NAME]]` gets the reply NAME. A request whose
last item is a tool result gets the text "done". Muse's background reminder
observers (offered only `submit_reminder_decision`) decide "none". A
workflow child (offered `submit_result`) reports "child finished". Anything
else gets the text "ok". A reply is {"text": "..."} or
{"tool": {"name": "...", "arguments": {...}}}, optionally with "hold_ms" to
hold the stream open before it completes.

The adapter's auto-review turn asks a guardian-style reviewer instead of a
user, so a request whose prompt carries the guardian policy gets the script's
`review` reply when one is defined. That lets a test script both the agent
turn and the reviewer's verdict.

The first line on stdout is the port. The log gets one JSON line per model
call: {"call", "reply", "last"}, where "last" is the type and role of the
request's last input item, plus its "output" when it is a tool result and
the start of its "text".
Standard library only.
"""
import json
import re
import sys
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL = "fake-model"
LOG = sys.argv[1]
SCRIPT = json.load(open(sys.argv[2]))
LOCK = threading.Lock()
CALLS = [0]
CHILD_RESULT = "child finished"
OBSERVER_DECISION = {
    "advisory_text": None, "confidence": None, "decision": "none",
    "priority": None, "reason": "not needed", "skill_id": None,
    "visible_for_steps": None,
}
CATALOG = json.dumps({"object": "list", "data": [{
    "id": MODEL, "object": "model",
    "metadata": {"muse-code": {
        "release_date": "2026-01-01", "is_hidden": False,
        "limit": {"context": 1000000, "output": 1024},
    }},
}]}).encode()


def sse(value):
    return ("data: " + json.dumps(value) + "\n\n").encode()


def response(rid, status, **extra):
    return {"id": rid, "object": "response", "model": MODEL,
            "status": status, "output": [], **extra}


def text_of(item):
    content = item.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content
                       if isinstance(part, dict))
    return ""


def offered(request):
    names = set()
    for tool in request.get("tools", []):
        names.add(tool.get("name"))
        for inner in tool.get("tools", []):
            names.add(inner.get("name"))
    return names


def reply_for(request):
    # Only reminder observers are offered this tool, never the main agent.
    if "submit_reminder_decision" in offered(request):
        return {"tool": {"name": "submit_reminder_decision",
                         "arguments": OBSERVER_DECISION}}
    items = request.get("input") or [{}]
    if items[-1].get("type") == "function_call_output":
        return {"text": "done"}
    # A workflow child finishes by reporting its result.
    if "submit_result" in offered(request):
        return {"tool": {"name": "submit_result",
                         "arguments": {"text": CHILD_RESULT, "notes": None}}}
    # The prompt can arrive as several user messages, for example with the
    # adapter's plan-mode instruction after the user's text.
    prompt = []
    for item in reversed(items):
        if item.get("role") != "user":
            break
        prompt.append(text_of(item))
    prompt_text = "".join(prompt)
    if ("You are judging one planned coding-agent action" in prompt_text
            and "review" in SCRIPT):
        return SCRIPT["review"]
    marker = re.search(r"\[\[script:([\w-]+)\]\]", prompt_text)
    if marker and marker.group(1) in SCRIPT:
        return SCRIPT[marker.group(1)]
    return {"text": "ok"}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def empty(self, status):
        self.send_response(status)
        self.send_header("content-length", "0")
        self.end_headers()

    def do_GET(self):
        if not self.path.endswith("/muse-code/models"):
            self.empty(404)
            return
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(CATALOG)))
        self.end_headers()
        self.wfile.write(CATALOG)

    def do_POST(self):
        try:
            self.respond()
        except OSError:
            # The host hung up, for example after a cancel.
            pass
        except Exception:
            # Fail visibly instead of dropping the connection, which Muse
            # would retry until the test times out.
            traceback.print_exc()
            if not self.started:
                self.empty(500)

    def respond(self):
        self.started = False
        body = self.rfile.read(int(self.headers.get("content-length", "0")))
        if not self.path.endswith("/responses"):
            self.empty(404)
            return
        request = json.loads(body)
        reply = reply_for(request)
        if "tool" not in reply and "text" not in reply:
            raise ValueError(f"script reply needs text or tool: {reply}")
        last = (request.get("input") or [{}])[-1]
        with LOCK:
            index = CALLS[0]
            CALLS[0] += 1
            with open(LOG, "a") as log:
                log.write(json.dumps({
                    "call": index, "reply": reply,
                    "last": {"type": last.get("type"), "role": last.get("role"),
                             "output": last.get("output"),
                             "text": text_of(last)[:4000]},
                }) + "\n")
        rid = f"resp_{index}"
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        self.close_connection = True
        self.started = True
        events = iter(range(1, 100))

        def send(event):
            event["sequence_number"] = next(events)
            self.wfile.write(sse(event))
            self.wfile.flush()

        send({"type": "response.created", "response": response(rid, "in_progress")})
        if "tool" in reply:
            tool = reply["tool"]
            send({"type": "response.function_call_arguments.done",
                  "output_index": 0, "item_id": f"fc_{index}", "name": tool["name"],
                  "call_id": f"call_{index}", "arguments": json.dumps(tool["arguments"])})
            if reply.get("hold_ms"):
                time.sleep(reply["hold_ms"] / 1000)
        else:
            # The full message sequence, so Muse streams the text as it
            # arrives rather than when the response completes.
            item = {"type": "message", "id": f"msg_{index}", "role": "assistant",
                    "status": "in_progress", "content": []}
            part = {"type": "output_text", "text": "", "annotations": []}
            send({"type": "response.output_item.added", "output_index": 0, "item": item})
            send({"type": "response.content_part.added", "output_index": 0,
                  "item_id": item["id"], "content_index": 0, "part": part})
            send({"type": "response.output_text.delta", "output_index": 0,
                  "item_id": item["id"], "content_index": 0, "delta": reply["text"]})
            if reply.get("hold_ms"):
                time.sleep(reply["hold_ms"] / 1000)
            done_part = dict(part, text=reply["text"])
            send({"type": "response.output_text.done", "output_index": 0,
                  "item_id": item["id"], "content_index": 0, "text": reply["text"]})
            send({"type": "response.content_part.done", "output_index": 0,
                  "item_id": item["id"], "content_index": 0, "part": done_part})
            send({"type": "response.output_item.done", "output_index": 0,
                  "item": dict(item, status="completed", content=[done_part])})
        send({"type": "response.completed", "response": response(rid, "completed", usage={
            "input_tokens": 1, "output_tokens": 1, "total_tokens": 2})})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
print(server.server_address[1], flush=True)
server.serve_forever()
