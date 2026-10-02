#!/usr/bin/env python3
"""Minimal stdio MCP server for live-host tests: one tool, `secret_word`.

Newline-delimited JSON-RPC 2.0 on stdin/stdout, standard library only.
"""
import json
import sys

TOOL = {
    "name": "secret_word",
    "description": "Returns the secret word.",
    "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
}


def reply(ident, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    if not line.strip():
        continue
    message = json.loads(line)
    method = message.get("method")
    ident = message.get("id")
    if ident is None:
        continue
    if method == "initialize":
        reply(ident, {
            "protocolVersion": message.get("params", {}).get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "probe", "version": "1"},
        })
    elif method == "tools/list":
        reply(ident, {"tools": [TOOL]})
    elif method == "tools/call":
        reply(ident, {"content": [{"type": "text", "text": "the secret word is pineapple"}]})
    else:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": ident,
                                     "error": {"code": -32601, "message": "method not found"}}) + "\n")
        sys.stdout.flush()
