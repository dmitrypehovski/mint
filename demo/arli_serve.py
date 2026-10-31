#!/usr/bin/env python3
"""A compiled skill behind the marketplace's call shape.

The marketplace calls a skill with POST /v1/skill/{slug}/call and {"query": "..."} and expects
an answer with a receipt. This serves exactly that shape, but the answer is not a paragraph: it
is a verified execution. The runtime drives the target, checks every step against the page it
must produce, and reports which tier resolved each one.

One deliberate difference worth showing: the marketplace charges for any answer, including a
negative one, because a wrong question still costs the author a model call. Here a question that
cannot be resolved is refused before anything is spent, because there is no model in the path.
"""
import json
import re
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

CONFIG = {"slug": "", "skill": "", "targets": {}, "key": "", "price_usd": 0.20}


def load(path):
    global CONFIG
    CONFIG.update(json.load(open(path, encoding="utf-8")))


def receipt_id():
    return "rcpt_" + str(int(time.time() * 1000))[-9:]


def run_skill(url):
    """Drive the target with the compiled skill and report what happened."""
    job = {"skill": CONFIG["skill"], "start": url}
    tmp = "/root/.hermes/profiles/work2/cache/scratch/arli-call-jobs.json"
    json.dump([job], open(tmp, "w"))
    started = time.time()
    proc = subprocess.run(
        ["/root/levos/target/release/levos", "eval", "--jobs", tmp,
         "--out", "/root/.hermes/profiles/work2/cache/scratch/arli-call-out.json",
         "--category", "marketplace call"],
        capture_output=True, text=True, timeout=240, cwd="/root/levos")
    wall_ms = int((time.time() - started) * 1000)
    try:
        out = json.load(open("/root/.hermes/profiles/work2/cache/scratch/arli-call-out.json"))
        r = out["runs"][0]
    except Exception:
        return None, wall_ms, proc.stdout[-400:]
    return r, wall_ms, ""


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _json(self, code, obj):
        raw = json.dumps(obj, ensure_ascii=False, indent=1).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self._json(200, {"ok": True, "slug": CONFIG["slug"], "skill": CONFIG["skill"],
                         "targets": list(CONFIG["targets"].keys())})

    def do_POST(self):
        m = re.match(r"^/v1/skill/([^/]+)/call$", self.path)
        if not m:
            self._json(404, {"ok": False, "error": "unknown endpoint"})
            return
        if m.group(1) != CONFIG["slug"]:
            self._json(404, {"ok": False, "error": f"unknown slug {m.group(1)}"})
            return
        # A key check, like the marketplace's: a refusal costs nothing.
        if CONFIG["key"] and self.headers.get("x-arli-skill-key") != CONFIG["key"]:
            self._json(403, {"ok": False, "error": "skill not enabled for this account"})
            return
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        query = (body.get("query") or "").strip()
        if not query:
            self._json(400, {"ok": False, "error": "empty query"})
            return

        # Resolve the target before spending anything: a question that cannot be aimed is
        # refused here rather than answered with a shrug.
        url = None
        found = re.search(r"https?://\S+", query)
        if found:
            url = found.group(0)
        else:
            for label, target in CONFIG["targets"].items():
                if label.lower() in query.lower():
                    url = target
                    break
        if not url:
            self._json(400, {"ok": False, "verdict": "no target in the question",
                             "detail": "Name a target this skill knows, or give its address.",
                             "charged_usd": 0.0,
                             "note": "refused before any work: no model call was made"})
            return

        row, wall_ms, tail = run_skill(url)
        if row is None:
            self._json(400, {"ok": False, "verdict": "the run did not produce a report",
                             "detail": tail, "charged_usd": 0.0})
            return

        tiers = {}
        for t in row.get("tiers", []):
            tiers[t] = tiers.get(t, 0) + 1
        charged = round(CONFIG["price_usd"] * 0.7, 4)   # what the author keeps
        self._json(200, {
            "ok": True,
            "slug": CONFIG["slug"],
            "receipt": receipt_id(),
            "verdict": "done" if row["success"] else "attempted",
            "detail": (f"{row['decisions']} step(s) executed towards {row['goal']}; "
                       f"landed on {row['landed'] or 'an unnamed page'}"),
            # The evidence part, in the same spirit as a knowledge skill's source and caveat.
            "evidence": {
                "expected_page": row["expected"],
                "landed_page": row["landed"],
                "decisions": row["decisions"],
                "model_calls": row["model_calls"],
                "resolved_by": tiers,
            },
            "source": row["target"],
            "latency_ms": wall_ms,
            "charged_usd": charged,
            "author_share": 0.7,
            "caveat": ("Every step reports the tier that resolved it. A step resolved by the "
                       "model is not a deterministic run and is named as such."),
        })


if __name__ == "__main__":
    load(sys.argv[1])
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 8891
    print(f"serving {CONFIG['slug']} on 0.0.0.0:{port}", flush=True)
    HTTPServer(("0.0.0.0", port), Handler).serve_forever()
