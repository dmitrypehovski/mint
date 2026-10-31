#!/usr/bin/env python3
"""Collect the measurements the site publishes. Everything on the page is measured here —
never recalled and never rounded in a convenient direction."""
import asyncio
import json
import os
import statistics
import time
import urllib.request

import websockets

PORT = 8830
PAGES = [
    "https://github.com/tokio-rs/tokio",
    "https://github.com/rust-lang/rust",
    "https://github.com/facebook/react",
]
EXTRACT = open("/root/mint/src/cdp.rs", encoding="utf-8").read().split('const EXTRACT: &str = r#"')[1].split('"#;')[0]


def openrouter_key():
    for p in ("/root/.hermes/profiles/work2/.env", "/root/.hermes/.env"):
        try:
            for line in open(p, encoding="utf-8", errors="replace"):
                if line.startswith("OPENROUTER_API_KEY="):
                    return line.split("=", 1)[1].strip().strip('"').strip("'")
        except FileNotFoundError:
            pass
    return None


async def measure():
    key = openrouter_key()
    out = {"pages": [], "model": {}, "skills": {}, "notes": []}

    targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json"))
    page = next(t for t in targets if t["type"] == "page")
    ws = await websockets.connect(page["webSocketDebuggerUrl"],
                                  max_size=80 * 1024 * 1024, ping_interval=None)
    n = [0]

    async def send(method, params=None, timeout=60):
        n[0] += 1
        await ws.send(json.dumps({"id": n[0], "method": method, "params": params or {}}))
        while True:
            r = json.loads(await asyncio.wait_for(ws.recv(), timeout=timeout))
            if r.get("id") == n[0]:
                return r

    await send("Page.enable")
    await send("Runtime.enable")
    await send("Accessibility.enable")

    for url in PAGES:
        await send("Page.navigate", {"url": url})
        for _ in range(80):
            r = await send("Runtime.evaluate", {"expression": "document.readyState", "returnByValue": True})
            if r.get("result", {}).get("result", {}).get("value") == "complete":
                break
            await asyncio.sleep(0.1)
        await asyncio.sleep(2.5)

        samples, sizes, counts = [], [], []
        for _ in range(10):
            t = time.perf_counter()
            r = await send("Runtime.evaluate", {"expression": EXTRACT, "returnByValue": True})
            dt = (time.perf_counter() - t) * 1000
            data = json.loads(r["result"]["result"]["value"])
            samples.append(dt)
            sizes.append(len(json.dumps(data["els"], ensure_ascii=False).encode()))
            counts.append(len(data["els"]))

        # what a naive agent would send: the whole page text and the whole accessibility tree
        r = await send("Runtime.evaluate", {"expression": "document.body.innerText.length", "returnByValue": True})
        inner = r["result"]["result"].get("value") or 0
        ax = await send("Accessibility.getFullAXTree", {"depth": -1})
        nodes = ax.get("result", {}).get("nodes", [])
        tree = len(json.dumps(nodes, ensure_ascii=False).encode())

        out["pages"].append({
            "url": url,
            "elements": counts[0],
            "capture_ms_median": round(statistics.median(samples), 1),
            "capture_ms_min": round(min(samples), 1),
            "capture_ms_max": round(max(samples), 1),
            "state_bytes": sizes[0],
            "naive_text_bytes": inner,
            "naive_tree_bytes": tree,
            "naive_tree_nodes": len(nodes),
            "reduction_vs_tree": round(tree / max(sizes[0], 1)),
        })

    # model latency on a state payload of realistic size
    if key:
        state_r = await send("Runtime.evaluate", {"expression": EXTRACT, "returnByValue": True})
        state = json.loads(state_r["result"]["result"]["value"])["els"][:60]
        prompt = ("Screen state: " + json.dumps(state, ensure_ascii=False)[:6000] +
                  "\nGoal: open the issue list. Reply with JSON ONLY {\"idx\": 0}.")
        lat = []
        for _ in range(6):
            t = time.perf_counter()
            try:
                req = urllib.request.Request(
                    "https://openrouter.ai/api/v1/chat/completions",
                    data=json.dumps({"model": os.environ.get("MINT_MODEL", "google/gemini-2.5-flash"),
                                     "messages": [{"role": "user", "content": prompt}],
                                     "max_tokens": 40, "temperature": 0}).encode(),
                    headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"},
                    method="POST")
                urllib.request.urlopen(req, timeout=90).read()
                lat.append((time.perf_counter() - t) * 1000)
            except Exception as e:
                out["notes"].append(f"model: {str(e)[:80]}")
        if lat:
            out["model"] = {
                "name": os.environ.get("MINT_MODEL", "google/gemini-2.5-flash") + " via the configured endpoint",
                "samples": len(lat),
                "ms_median": round(statistics.median(lat)),
                "ms_min": round(min(lat)),
                "ms_max": round(max(lat)),
                "state_bytes_sent": len(prompt.encode()),
            }
    await ws.close()
    return out


async def main():
    data = await measure()

    # skill file sizes are a measurement too
    import os
    for name in ("gh-commit", "gh-issues", "gh-prs"):
        p = f"/root/mint/{name}.skill.json"
        if os.path.exists(p):
            sk = json.load(open(p, encoding="utf-8"))
            data["skills"][name] = {
                "bytes": os.path.getsize(p),
                "steps": len(sk["steps"]),
                "goal": sk["goal"],
                "verified": sk.get("learn_declared_done"),
                "learn_model_calls": sk["learn_model_calls"],
                "learn_ms": sk["learn_ms"],
            }
    data["collected_at"] = time.strftime("%Y-%m-%d %H:%M %Z")
    json.dump(data, open("/root/mint/demo/measurements.json", "w"),
              ensure_ascii=False, indent=1)

    print("=== MEASUREMENTS ===")
    for p in data["pages"]:
        print(f"  {p['url'].split('/')[-1]:<22} elements {p['elements']:>4} | capture {p['capture_ms_median']:>5} ms "
              f"| our payload {p['state_bytes']:>6} B | tree {p['naive_tree_bytes']:>7} B "
              f"| smaller by {p['reduction_vs_tree']:>4}x")
    m = data["model"]
    if m:
        print(f"  model: median {m['ms_median']} ms (min {m['ms_min']}, max {m['ms_max']}, n={m['samples']})")
    for k, v in data["skills"].items():
        print(f"  skill {k}: {v['bytes']} bytes, {v['steps']} steps, verified {v['verified']}")


asyncio.run(main())
