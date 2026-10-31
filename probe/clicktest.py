
import asyncio, json, urllib.request
import websockets
async def main():
    t = json.load(urllib.request.urlopen("http://127.0.0.1:8830/json"))
    page = next(x for x in t if x["type"]=="page")
    ws = await websockets.connect(page["webSocketDebuggerUrl"], max_size=80*1024*1024, ping_interval=None)
    n=[0]
    async def send(m,p=None):
        n[0]+=1; await ws.send(json.dumps({"id":n[0],"method":m,"params":p or {}}))
        while True:
            r=json.loads(await asyncio.wait_for(ws.recv(),timeout=30))
            if r.get("id")==n[0]: return r
    async def ev(expr):
        r=await send("Runtime.evaluate",{"expression":expr,"returnByValue":True})
        return r.get("result",{}).get("result",{}).get("value")
    await send("Page.enable"); await send("Runtime.enable")
    await send("Page.navigate",{"url":"https://github.com/rust-lang/rust"})
    await asyncio.sleep(6)
    print("before:", await ev("location.pathname"))
    # 1) an ordinary synthetic click
    r = await ev("""(() => {
      const a=[...document.querySelectorAll('a')].find(x=>x.getAttribute('href')==='/rust-lang/rust/issues');
      if(!a) return 'not found';
      a.click(); return 'clicked';
    })()""")
    print("click:", r)
    await asyncio.sleep(3)
    print("after the click:", await ev("location.pathname"))
    # 2) a full mouse event with coordinates
    if "issues" not in (await ev("location.pathname") or ""):
        await send("Page.navigate",{"url":"https://github.com/rust-lang/rust"})
        await asyncio.sleep(5)
        box = await ev("""(() => {
          const a=[...document.querySelectorAll('a')].find(x=>x.getAttribute('href')==='/rust-lang/rust/issues');
          const r=a.getBoundingClientRect(); return JSON.stringify({x:r.x+r.width/2,y:r.y+r.height/2});
        })()""")
        p=json.loads(box)
        for kind,extra in [("mousePressed",{"clickCount":1}),("mouseReleased",{"clickCount":1})]:
            await send("Input.dispatchMouseEvent",{"type":kind,"x":p["x"],"y":p["y"],"button":"left",**extra})
        await asyncio.sleep(3)
        print("after a real mouse click:", await ev("location.pathname"))
    await ws.close()
asyncio.run(main())
