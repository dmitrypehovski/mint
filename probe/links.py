
import asyncio, json, urllib.request
import websockets
EXTRACT = open("/root/levos/src/cdp.rs", encoding="utf-8").read().split('const EXTRACT: &str = r#"')[1].split('"#;')[0]
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
    await send("Page.enable"); await send("Runtime.enable")
    await send("Page.navigate", {"url":"https://github.com/rust-lang/rust"})
    await asyncio.sleep(5)
    r = await send("Runtime.evaluate", {"expression": EXTRACT, "returnByValue": True})
    data = json.loads(r["result"]["result"]["value"])
    print("elements:", len(data["els"]))
    for e in data["els"]:
        if any(k in e["name"].lower() for k in ("issue","pull","action","code","commit")):
            print(f'  {e["role"]:<10} «{e["name"][:28]:<28}» href={e["href"][:70]}')
    await ws.close()
asyncio.run(main())
