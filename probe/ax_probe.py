
import asyncio, json, time, sys
import websockets

async def cdp(url):
    return await websockets.connect(url, max_size=80*1024*1024, ping_interval=None)

async def send(ws, mid, method, params=None, timeout=30):
    await ws.send(json.dumps({"id": mid, "method": method, "params": params or {}}))
    while True:
        msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=timeout))
        if msg.get("id") == mid:
            return msg

async def main():
    import urllib.request
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:8830/json"))
    page = next(t for t in targets if t["type"] == "page")
    ws = await cdp(page["webSocketDebuggerUrl"])
    await send(ws, 1, "Page.enable")
    await send(ws, 2, "DOM.enable")
    await send(ws, 3, "Accessibility.enable")
    url = "https://news.ycombinator.com"
    await send(ws, 4, "Page.navigate", {"url": url})
    await asyncio.sleep(3.5)

    # (1) the naive path: the whole page text, the way any agent sends it
    t = time.perf_counter()
    r = await send(ws, 10, "Runtime.evaluate", {"expression": "document.body.innerText", "returnByValue": True})
    naive_ms = (time.perf_counter()-t)*1000
    naive_text = r["result"]["result"].get("value","") or ""
    print(f"NAIVE (whole page text):       {len(naive_text):>7} chars, capture {naive_ms:6.0f} ms")

    # (2) structural state: only interactive elements, with addressable paths
    t = time.perf_counter()
    r = await send(ws, 11, "Accessibility.getFullAXTree", {"depth": -1})
    ax_ms = (time.perf_counter()-t)*1000
    nodes = r.get("result",{}).get("nodes",[])
    full_size = len(json.dumps(nodes, ensure_ascii=False))
    interactive = []
    for n in nodes:
        role = (n.get("role") or {}).get("value") or ""
        name = (n.get("name") or {}).get("value") or ""
        if not name: continue
        if role in ("link","button","textbox","searchbox","checkbox","combobox","menuitem","tab","radio"):
            interactive.append({"r": role, "n": name[:80]})
    compact = json.dumps({"url": url, "el": interactive}, ensure_ascii=False, separators=(",",":"))
    print(f"FULL TREE:                     {full_size:>7} chars, capture {ax_ms:6.0f} ms, {len(nodes)} nodes")
    print(f"STRUCTURAL STATE:              {len(compact):>7} chars, interactive {len(interactive)}")
    print(f"  reduction: {full_size/max(len(compact),1):.1f}x against the tree, {len(naive_text)/max(len(compact),1):.1f}x against the text")
    print("  a sample:", json.dumps(interactive[:3], ensure_ascii=False)[:220])

asyncio.run(main())
