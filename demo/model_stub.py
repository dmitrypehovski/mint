#!/usr/bin/env python3
"""Stand-in model endpoint: proves the adapter speaks both shapes.

Serves the OpenAI shape and Anthropic's native shape, and records which headers it saw,
so "the model is a swappable part" is a tested claim rather than a comment.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

SEEN = []


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        SEEN.append({
            "path": self.path,
            "auth": "bearer" if self.headers.get("Authorization") else None,
            "x_api_key": "yes" if self.headers.get("x-api-key") else None,
            "anthropic_version": self.headers.get("anthropic-version"),
            "model": body.get("model"),
            "response_format": bool(body.get("response_format")),
        })
        # The learning run stops immediately when the model says the goal is reached.
        decision = json.dumps({"idx": -1, "intent": "done", "confidence": 1.0})
        if "/messages" in self.path:
            out = {"content": [{"type": "text", "text": decision}]}
        else:
            out = {"choices": [{"message": {"role": "assistant", "content": decision}}]}
        raw = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        raw = json.dumps(SEEN).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8899
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
