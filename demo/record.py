#!/usr/bin/env python3
"""Record a run to mp4 — the fallback in case the room has no network.

A separate browser watches the live showcase and shoots it frame by frame while the
runtime runs the demonstration. The frames are assembled into a video.
"""
import asyncio
import json
import os
import subprocess
import time
import urllib.request

import websockets

VIEW_PORT = 8850
# Recording happens on this same machine, so the local showcase address is used: no tunnel
# is needed, and a tunnel address is short-lived — one already went dead and the recording
URL = "http://127.0.0.1:8840/"
OUT = "/root/mint/demo/frames"
FPS = 2.0
SECONDS = 78          # kept for compatibility
MAX_SECONDS = 180     # safety stop: shoot while the run lasts, but no longer


async def main():
    os.makedirs(OUT, exist_ok=True)
    for f in os.listdir(OUT):
        os.remove(os.path.join(OUT, f))

    # this browser is the one that shoots the page
    targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{VIEW_PORT}/json"))
    page = next(t for t in targets if t["type"] == "page")
    ws = await websockets.connect(page["webSocketDebuggerUrl"],
                                  max_size=80 * 1024 * 1024, ping_interval=None)
    n = [0]

    async def send(method, params=None):
        n[0] += 1
        await ws.send(json.dumps({"id": n[0], "method": method, "params": params or {}}))
        while True:
            r = json.loads(await asyncio.wait_for(ws.recv(), timeout=40))
            if r.get("id") == n[0]:
                return r

    await send("Page.enable")
    await send("Emulation.setDeviceMetricsOverride",
               {"width": 1600, "height": 1000, "deviceScaleFactor": 1, "mobile": False})
    await send("Page.navigate", {"url": URL})
    await asyncio.sleep(3)

    # start the run itself
    demo = subprocess.Popen(
        ["/root/mint/target/release/mint", "chain",
         "--jobs", "/root/mint/demo/jobs.json",
         "--push", "http://127.0.0.1:8840", "--max-steps", "3"],
        cwd="/root/mint", stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print("run started, shooting the showcase...", flush=True)

    started = time.time()
    i = 0

    async def shoot():
        nonlocal i
        r = await send("Page.captureScreenshot", {"format": "jpeg", "quality": 80})
        data = r.get("result", {}).get("data")
        if data:
            i += 1
            with open(f"{OUT}/f{i:05d}.jpg", "wb") as fh:
                import base64
                fh.write(base64.b64decode(data))

    # Shoot while the run lasts, not for a fixed time: a recording cut by a timer ends
    # on a frame where the counters have not reached the verdict, so the verdict on screen
    # is wrong. A fallback clip has to end on the real numbers.
    while demo.poll() is None and time.time() - started < MAX_SECONDS:
        await shoot()
        await asyncio.sleep(1.0 / FPS)
    # tail: let the showcase show the final line and shoot it
    for _ in range(int(FPS * 3)):
        await shoot()
        await asyncio.sleep(1.0 / FPS)
    print(f"frames shot: {i}", flush=True)
    try:
        demo.wait(timeout=30)
    except subprocess.TimeoutExpired:
        demo.kill()
    await ws.close()

    out = "/root/mint/demo/race.mp4"
    subprocess.run([
        "ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS),
        "-i", f"{OUT}/f%05d.jpg", "-c:v", "libx264", "-pix_fmt", "yuv420p",
        "-vf", "scale=1600:-2", out], check=True)
    size = os.path.getsize(out)
    print(f"video: {out} ({size//1024} KB)")


asyncio.run(main())
