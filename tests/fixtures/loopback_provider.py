#!/usr/bin/env python3
"""Loopback model provider for real `muse serve` tests (tests/live_loopback.rs).

A throwaway Muse config points `endpoint_transport.base_url` at this server,
so the host's model traffic never leaves the machine and needs no
credentials. It serves the model catalog (`GET .../muse-code/models`) and
scripted Responses API streams (`POST .../responses`).

Usage: loopback_provider.py <log file> <script file>

The script maps names to replies. A request whose last input item is a user
message containing `[[script:NAME]]` gets the reply NAME. A request whose
last item is a tool result gets the text "done". Muse's background reminder
observers (offered only `submit_reminder_decision`) decide "none". Anything
else gets the text "ok". A reply is {"text": "..."} or
{"tool": {"name": "...", "arguments": {...}}}, optionally with "hold_ms" to
hold the stream open before it completes.

The first line on stdout is the port. The log gets one JSON line per model
call: {"call", "reply", "request"}. Standard library only.
"""
import json
import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL = "fake-model"
LOG = sys.argv[1]
SCRIPT = json.load(open(sys.argv[2]))
LOCK = threading.Lock()
CALLS = [0]
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
    last = (request.get("input") or [{}])[-1]
    if last.get("type") == "function_call_output":
        return {"text": "done"}
    if last.get("role") == "user":
        marker = re.search(r"\[\[script:([\w-]+)\]\]", text_of(last))
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
        body = self.rfile.read(int(self.headers.get("content-length", "0")))
        if not self.path.endswith("/responses"):
            self.empty(404)
            return
        request = json.loads(body)
        with LOCK:
            index = CALLS[0]
            CALLS[0] += 1
            reply = reply_for(request)
            with open(LOG, "a") as log:
                log.write(json.dumps({"call": index, "reply": reply,
                                      "request": request}) + "\n")
        rid = f"resp_{index}"
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        self.close_connection = True
        try:
            self.wfile.write(sse({"type": "response.created", "sequence_number": 1,
                                  "response": response(rid, "in_progress")}))
            if "tool" in reply:
                tool = reply["tool"]
                self.wfile.write(sse({
                    "type": "response.function_call_arguments.done",
                    "sequence_number": 2, "output_index": 0,
                    "item_id": f"fc_{index}", "name": tool["name"],
                    "call_id": f"call_{index}",
                    "arguments": json.dumps(tool["arguments"]),
                }))
            else:
                self.wfile.write(sse({
                    "type": "response.output_text.delta", "sequence_number": 2,
                    "output_index": 0, "item_id": f"msg_{index}",
                    "content_index": 0, "delta": reply["text"],
                }))
            self.wfile.flush()
            if reply.get("hold_ms"):
                time.sleep(reply["hold_ms"] / 1000)
            self.wfile.write(sse({
                "type": "response.completed", "sequence_number": 3,
                "response": response(rid, "completed", usage={
                    "input_tokens": 1, "output_tokens": 1, "total_tokens": 2}),
            }))
            self.wfile.flush()
        except OSError:
            # The host hung up, for example after a cancel.
            pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
print(server.server_address[1], flush=True)
server.serve_forever()
