
import asyncio, json, base64, time, urllib.request
import websockets

async def main():
    targets = json.load(urllib.request.urlopen("http://127.0.0.1:8830/json"))
    page = next(t for t in targets if t["type"] == "page")
    ws = await websockets.connect(page["webSocketDebuggerUrl"], max_size=80*1024*1024, ping_interval=None)
    n = [0]
    async def send(method, params=None):
        n[0] += 1
        await ws.send(json.dumps({"id": n[0], "method": method, "params": params or {}}))
        while True:
            m = json.loads(await asyncio.wait_for(ws.recv(), timeout=30))
            if m.get("id") == n[0]:
                return m

    await send("Page.enable")
    await send("Page.navigate", {"url": "https://github.com/rust-lang/rust"})
    await asyncio.sleep(4)

    frames = []
    print("turning the stream on...")
    await send("Page.startScreencast", {"format": "jpeg", "quality": 70,
                                        "maxWidth": 1280, "maxHeight": 800, "everyNthFrame": 1})
    t0 = time.perf_counter()
    deadline = t0 + 12
    while time.perf_counter() < deadline and len(frames) < 8:
        try:
            m = json.loads(await asyncio.wait_for(ws.recv(), timeout=5))
        except asyncio.TimeoutError:
            continue
        if m.get("method") == "Page.screencastFrame":
            d = m["params"]
            frames.append((len(d.get("data","")), d.get("metadata",{}).get("timestamp")))
            await ws.send(json.dumps({"id": 9000+len(frames), "method": "Page.screencastFrameAck",
                                      "params": {"sessionId": d["sessionId"]}}))
    dt = time.perf_counter() - t0
    print(f"frames received: {len(frames)} in {dt:.1f} s")
    if frames:
        sizes = [f[0] for f in frames]
        print(f"frame size (base64): median {sorted(sizes)[len(sizes)//2]} bytes, max {max(sizes)}")
        print(f"rate: {len(frames)/dt:.1f} frames/s")
        # keep the last frame as an image
        await ws.send(json.dumps({"id": 9999, "method": "Page.stopScreencast", "params": {}}))
    await ws.close()

asyncio.run(main())
