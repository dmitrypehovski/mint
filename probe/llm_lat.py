
import json, time, urllib.request

key = None
for p in ("/root/.hermes/profiles/work2/.env", "/root/.hermes/.env"):
    try:
        for line in open(p, encoding="utf-8", errors="replace"):
            if line.startswith("OPENROUTER_API_KEY="):
                key = line.split("=", 1)[1].strip().strip('"').strip("'")
    except FileNotFoundError:
        pass
    if key: break
if not key: raise SystemExit("no key")

def post(path, body, timeout=90):
    req = urllib.request.Request("https://openrouter.ai/api/v1" + path,
        data=json.dumps(body).encode(), headers={"Authorization": "Bearer " + key,
        "Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)

req = urllib.request.Request("https://openrouter.ai/api/v1/models", headers={"Authorization": "Bearer " + key})
models = json.load(urllib.request.urlopen(req, timeout=60))["data"]
names = [m["id"] for m in models]
picks = [n for n in ("google/gemini-2.5-flash","google/gemini-2.5-flash-lite","anthropic/claude-haiku-4.5",
                     "openai/gpt-5-mini","deepseek/deepseek-chat","x-ai/grok-4-fast") if n in names]
print("models available:", len(names), "| measuring:", picks[:3])

state = {"url": "https://news.ycombinator.com", "el": [
    {"r":"link","n":"Guidelines"},{"r":"link","n":"FAQ"},
    {"r":"textbox","n":"Search"},{"r":"button","n":"More"}]}
prompt = ("Screen state: " + json.dumps(state, ensure_ascii=False) +
          "\nGoal: open the discussion of the top story.\nPick one action from the table, reply with JSON ONLY "
          "{\"action_id\":\"...\"}. Table: click:1, click:2, type:search, reobserve, abstain.")
for m in (picks[:2] or names[:1]):
    ts = []
    for i in range(3):
        t = time.perf_counter()
        try:
            r = post("/chat/completions", {"model": m, "messages":[{"role":"user","content":prompt}],
                                           "max_tokens":60, "temperature":0})
            dt = (time.perf_counter()-t)*1000; ts.append(dt)
            print(f"  {m} #{i+1}: {dt:6.0f} ms -> {(r['choices'][0]['message']['content'] or '')[:50]!r}")
        except Exception as e:
            print(f"  {m} #{i+1}: error {str(e)[:80]}")
    if ts: print(f"  -> median {sorted(ts)[len(ts)//2]:.0f} ms per decision\n")
