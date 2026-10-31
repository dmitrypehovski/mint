#!/usr/bin/env python3
"""Build the demonstration pages.

Everything on these pages comes from an artifact the runtime actually produced: the signed log, the
published rules, the evaluation of eighteen real tasks, the page measurements. Nothing is retyped and
nothing is a mock-up, so re-running the commands the pages print reproduces what they show.

The interactive ones call the local relay, which shells out to the real binary with fixed arguments
and returns its stdout unedited. That is deliberate: a demonstration that shows a canned answer is
worth less than no demonstration, because it teaches a visitor to believe output they cannot check.
"""
import json
import os
import re

ROOT = "/root/levos"
DEMO = os.path.join(ROOT, "demo")
OUT = os.path.join(DEMO, "demos")


def load(name):
    with open(os.path.join(DEMO, name), encoding="utf-8") as fh:
        return json.load(fh)


receipts = load("gate-receipts.json")
policy = load("gate-policy.json")
obs = load("gate-observations.json")["observations"]
report = load("gate-report.json")
evaluation = load("evaluation.json")
measurements = load("measurements.json")
endorsement = load("gate-endorsement.json")

site = open(os.path.join(DEMO, "site.html"), encoding="utf-8").read()

# The family stylesheet, taken from the site rather than rewritten, so the demonstrations cannot drift
# away from the page they belong to.
CSS_SITE = re.search(r"<style>(.*?)</style>", site, re.S).group(1)
FONTS = "\n".join(re.findall(r'<link[^>]*fonts\.googleapis[^>]*>', site))
NAV = re.search(r"<nav>.*?</nav>", site, re.S).group(0)
# The site's own navigation points at bare anchors, which on any other page lead nowhere. Every one of
# them is rewritten to name the page it belongs to, and the demonstrations are put next to the name.
NAV = re.sub(r'<a href="#([a-z]+)"', r'<a href="/site#\1"', NAV)
NAV = NAV.replace('<b>mint</b>', '<b><a href="/site" style="color:inherit">mint</a></b>', 1)
if 'href="/demos"' not in NAV:
    NAV = NAV.replace('<a href="/site#watch">',
                      '<a href="/demos" style="color:var(--ink);font-weight:600">Demos</a>'
                      '<a href="/site#watch">', 1)

CSS_EXTRA = """
  /* The demonstrations. Additions only: the palette, the type and the grid are the family's. */
  .demohead{padding:44px 0 8px}
  .demohead .eyebrow{color:var(--green);font:600 12.5px/1 var(--mono);letter-spacing:.12em;
    text-transform:uppercase;margin-bottom:16px}
  .demohead h1{font-family:var(--display);font-size:clamp(28px,3.9vw,44px);line-height:1.06;
    letter-spacing:-.035em;margin:0 0 16px;font-weight:800;max-width:900px}
  .demohead p.lede{font-size:18px;color:var(--dim);max-width:820px;margin:0 0 10px}
  .demohead p.lede b{color:var(--ink);font-weight:600}
  .back{display:inline-block;margin:14px 0 0;font:600 12.5px/1 var(--mono);color:var(--dim)}
  .demos{display:grid;grid-template-columns:repeat(auto-fill,minmax(312px,1fr));gap:14px;margin:22px 0 0}
  .demo{background:var(--card);border:1px solid var(--rule);border-radius:12px;padding:17px 19px;
    display:block;transition:border-color .12s}
  .demo:hover{border-color:var(--rule-strong);text-decoration:none}
  .demo .n{font:600 12px/1 var(--mono);color:var(--green);letter-spacing:.1em}
  .demo h3{font-family:var(--display);font-size:18.5px;margin:9px 0 7px;letter-spacing:-.02em;
    color:var(--ink);font-weight:700}
  .demo p{color:var(--dim);font-size:14.5px;margin:0}
  .demo .tagx{display:inline-block;margin-top:11px;font:600 11.5px/1 var(--mono);color:var(--dim2);
    border:1px solid var(--rule);border-radius:999px;padding:5px 10px}
  .demo .tagx.live{color:var(--green-deep);border-color:var(--rule-strong);background:var(--green-wash)}
  h2.dh{font-family:var(--display);font-size:23px;letter-spacing:-.025em;margin:34px 0 6px;font-weight:700}
  h3.dh{font-family:var(--display);font-size:17px;letter-spacing:-.02em;margin:24px 0 6px;font-weight:700}
  .try{display:grid;gap:9px;margin:16px 0 0}
  .tryrow{display:grid;grid-template-columns:minmax(0,1fr) auto 96px;gap:13px;align-items:center;
    background:var(--card);border:1px solid var(--rule);border-radius:10px;padding:11px 14px}
  .tryrow .el{font:500 13.5px/1.35 var(--mono);color:var(--ink);overflow-wrap:anywhere}
  .tryrow .ob{font:11.5px/1.5 var(--mono);color:var(--dim2);margin-top:4px;overflow-wrap:anywhere}
  .sw{position:relative;width:46px;height:25px;border-radius:999px;background:var(--tint);
    border:1px solid var(--rule-strong);cursor:pointer;flex:none}
  .sw.on{background:var(--green);border-color:var(--green-deep)}
  .sw i{position:absolute;top:2px;left:2px;width:19px;height:19px;border-radius:50%;background:#fff;
    border:1px solid var(--rule-strong);transition:left .15s}
  .sw.on i{left:23px}
  .vd{font:700 11.5px/1 var(--mono);padding:6px 10px;border-radius:999px;border:1px solid;
    text-align:center;letter-spacing:.04em}
  .vd.allow{color:var(--green-deep);background:var(--green-wash);border-color:var(--rule-strong)}
  .vd.hold{color:var(--amber);background:#fbf6e9;border-color:#e3d5ae}
  .vd.refuse{color:var(--red);background:var(--red-wash);border-color:#e8c9c9}
  .vd.cls{color:var(--dim);background:var(--tint);border-color:var(--rule)}
  .runout{font:12.5px/1.6 var(--mono);white-space:pre-wrap;background:#0e1a08;color:#cfe6bd;
    border-radius:10px;padding:15px 17px;margin:13px 0 0;overflow-x:auto;min-height:46px}
  .runout:empty::before{content:"press a button — the real output appears here";color:#6d7f5e}
  .cmdtxt{font:12.5px/1.55 var(--mono);color:var(--dim);margin:11px 0 0;overflow-wrap:anywhere}
  .cmdtxt b{color:var(--ink);font-weight:600}
  .why{color:var(--dim);font-size:15px;margin:9px 0 0}
  .honest{background:var(--red-wash);border:1px solid #e8c9c9;border-radius:11px;padding:15px 18px;
    margin:22px 0 0}
  .honest h5{margin:0 0 7px;font:700 12px/1 var(--mono);letter-spacing:.08em;text-transform:uppercase;
    color:var(--red)}
  .honest p{color:var(--dim);font-size:14.5px;margin:0}
  .ladder{display:flex;gap:7px;flex-wrap:wrap;margin:16px 0 0}
  .ladder .st{border:1px solid var(--rule);border-radius:9px;padding:9px 11px;background:var(--card);
    font:11.5px/1.45 var(--mono);color:var(--dim);min-width:118px}
  .ladder .st b{display:block;color:var(--ink);font-size:12px;margin-bottom:3px}
  .ladder .st.refuse{border-color:#e8c9c9;background:var(--red-wash)}
  .ladder .st.hold{border-color:#e3d5ae;background:#fbf6e9}
  .ladder .st.allow{border-color:var(--rule-strong);background:var(--green-wash)}
  .ladder .st.gap{border-style:dashed;color:var(--dim2);background:transparent}
  .rct{display:grid;grid-template-columns:210px minmax(0,1fr);border:1px solid var(--rule);
    border-radius:10px;overflow:hidden;background:var(--card);margin:14px 0 0}
  .rct .k,.rct .v{padding:9px 14px;border-top:1px solid var(--rule);font:12.5px/1.55 var(--mono)}
  .rct .k{color:var(--dim);background:var(--green-wash);font-weight:600}
  .rct .v{color:var(--ink);overflow-wrap:anywhere}
  .rct .k.b:first-child,.rct .v.b:first-child{border-top:none}
  .pick{display:flex;gap:8px;flex-wrap:wrap;margin:14px 0 0}
  .pick button{font:600 12.5px/1 var(--mono);padding:9px 12px;border-radius:8px;
    border:1px solid var(--rule-strong);background:var(--card);color:var(--dim);cursor:pointer}
  .pick button:hover{border-color:var(--green);color:var(--ink)}
  .pick button.on{background:var(--green);border-color:var(--green);color:#fff}
  table.rt{width:100%;border-collapse:collapse;font-size:14px;margin:14px 0 0}
  table.rt th{text-align:left;font:600 11.5px/1 var(--mono);letter-spacing:.06em;text-transform:uppercase;
    color:var(--dim2);padding:9px 10px;border-bottom:1px solid var(--rule-strong)}
  table.rt td{padding:10px;border-bottom:1px solid var(--rule);color:var(--dim)}
  table.rt td.m{font:12.5px/1.45 var(--mono);color:var(--ink)}
  table.rt tr:hover td{background:var(--green-wash)}
  .yes{color:var(--green-deep);font-weight:600}
  .no{color:var(--red);font-weight:600}
  .warnc{color:var(--amber);font-weight:600}
  .btn{display:inline-block;font:600 13.5px/1 var(--sans);padding:11px 17px;border-radius:9px;
    background:var(--green);color:#fff;border:1px solid var(--green);cursor:pointer}
  .btn.ghost{background:var(--card);color:var(--green);border-color:var(--rule-strong)}
  .btn:hover{opacity:.93;text-decoration:none}
  .hint{font:12px/1.55 var(--mono);color:var(--dim2);margin-top:9px}
  /* The live run: the browser as it moves, next to the counters, above the step list. */
  .livewrap{display:grid;grid-template-columns:minmax(0,1.4fr) minmax(0,1fr);gap:18px;margin:16px 0 0;
    align-items:start}
  .shotbox{position:relative;border:1px solid var(--rule);border-radius:12px;background:var(--card);
    overflow:hidden;min-height:280px;display:flex;align-items:center;justify-content:center}
  .shotbox img{display:none;width:100%;height:auto}
  .shotbox #liveempty{padding:44px 22px;color:var(--dim2);font:12.5px/1.6 var(--mono);text-align:center}
  .livemeta{display:grid;gap:8px;align-content:start}
  .livemeta .lm{display:grid;grid-template-columns:auto minmax(0,1fr);gap:12px;background:var(--card);
    border:1px solid var(--rule);border-radius:9px;padding:10px 13px;font:12.5px/1.45 var(--mono)}
  .livemeta .lm span:first-child{color:var(--dim2)}
  .livemeta .lm span:last-child{color:var(--ink);overflow-wrap:anywhere}
  .livesteps{list-style:none;padding:0;margin:16px 0 0;display:grid;gap:8px}
  .livesteps li{display:grid;grid-template-columns:30px 60px minmax(0,1fr) 72px minmax(0,1.2fr);gap:10px;
    align-items:center;background:var(--card);border:1px solid var(--rule);border-radius:9px;
    padding:9px 13px;font:12.5px/1.45 var(--mono);color:var(--dim)}
  .livesteps .sn{color:var(--dim2)}
  .livesteps .sl{color:var(--ink);overflow-wrap:anywhere}
  .livesteps .sn2{color:var(--dim2);text-align:right}
  .livetask .btn{white-space:nowrap}
  @media(max-width:860px){.livewrap{grid-template-columns:1fr}
    .livesteps li{grid-template-columns:24px 56px minmax(0,1fr)}}
  .grid2{display:grid;grid-template-columns:1fr 1fr;gap:16px;margin:16px 0 0}
  @media(max-width:760px){.grid2{grid-template-columns:1fr}.rct{grid-template-columns:1fr}
    .tryrow{grid-template-columns:1fr auto;gap:9px}}
"""

HEAD = """<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title><!--TITLE--></title>
<meta name="description" content="<!--DESC-->">
<!--FONTS-->
<link rel="stylesheet" href="/demos.css">
</head><body>
<header><!--NAV--></header>
<div class="wrap">
<!--BODY-->
</div>
<script>
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
async function api(path) {
  const r = await fetch('/api/' + path, {cache: 'no-store'});
  const j = await r.json().catch(() => ({ok: false, error: 'HTTP ' + r.status}));
  if (!j.ok) throw new Error(j.error || ('HTTP ' + r.status));
  return j;
}
function show(el, text) { el.textContent = text; }
function fail(el, e) { el.textContent = 'the call did not complete: ' + (e && e.message ? e.message : e); }
function cmdline(el, text) { el.innerHTML = '<b>the command behind this:</b> ' + esc(text); }
<!--JS-->
</script>
</body></html>
"""


def page(name, title, desc, num, h1, lede, body, js=""):
    html = (HEAD.replace("<!--TITLE-->", title)
                .replace("<!--DESC-->", desc)
                .replace("<!--FONTS-->", FONTS)
                .replace("<!--NAV-->", NAV)
                .replace("<!--BODY-->", body)
                .replace("<!--JS-->", js))
    path = os.path.join(OUT, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(html)
    print(f"  {name:<22} {len(html):>7} bytes")
    return html


def head(num, h1, lede, tag=""):
    return f"""<div class="demohead">
  <div class="eyebrow">{'all demonstrations' if num == 0 else 'demonstration ' + str(num)}{(' · ' + tag) if tag else ''}</div>
  <h1>{h1}</h1>
  <p class="lede">{lede}</p>
{(chr(10) + '  <a class="back" href="/demos">&larr; all demonstrations</a>') if num else ''}
</div>"""


def honest(what):
    return f"""<div class="honest"><h5>what this does not show</h5><p>{what}</p></div>"""


# ── the stylesheet ──────────────────────────────────────────────────────────────────────────────
with open(os.path.join(DEMO, "demos.css"), "w", encoding="utf-8") as fh:
    fh.write("/* mint — the family stylesheet, plus what the demonstrations need. */\n")
    fh.write(CSS_SITE)
    fh.write(CSS_EXTRA)
print(f"  demos.css              written ({len(CSS_SITE) + len(CSS_EXTRA)} bytes css)")

# Observations as the runtime recorded them, with a name for the demonstration.
OBS_LABELS = [
    ("the link to the record", "a link with somewhere to go — nothing on the far side changes because someone arrived"),
    ("the reference number field", "typing fills a field; the commit, if there is one, is the submit"),
    ("the submit button", "this one commits the filing"),
    ("an element nobody can describe", "it is neither a navigation nor a button, so what it would do cannot be put into words"),
    ("the same submit button, approved", "the same action, with an approval bound to it"),
]

# ── 1. the gate ─────────────────────────────────────────────────────────────────────────────────
rows = []
for i, o in enumerate(obs[:5]):
    label, why = OBS_LABELS[i] if i < len(OBS_LABELS) else (f"observation {i}", "")
    ob = f"action={o['action']} tag={o['tag']} role={o['role']} href={'yes' if o['has_href'] else 'no'}"
    rows.append(f"""  <div class="tryrow" data-i="{i}">
    <div><div class="el">{label}</div><div class="ob">{ob}</div></div>
    <div class="sw{' on' if o['approval'] else ''}" data-sw="{i}" title="present an approval for this exact action"><i></i></div>
    <div class="vd cls" data-vd="{i}">…</div>
  </div>""")

body = head(
    1,
    "The gate answers one question: what would this step do?",
    "Every step goes through a gate before it touches anything. The gate does not look at what an "
    "element is called, and it does not take a model's word for it. It looks at what the element "
    "<b>would do</b> — read, write, or something nobody can describe — and from that comes allow, "
    "hold or refuse.",
    "live",
) + f"""
<h2 class="dh">The five things the runtime saw, and what the rules say about them</h2>
<p class="why">These are the observations from the published log, unchanged. Flip the switch to present
an approval for that exact action and the verdict is recomputed <b>now</b>, by the real rules engine,
not by the page.</p>
<div class="try">
{chr(10).join(rows)}
</div>
<p class="cmdtxt" id="cmdtxt"></p>
<pre class="runout" id="out"></pre>
<div class="grid2">
  <div><h3 class="dh">What the rules actually say</h3>
  <p class="why">A navigation is a read, whatever it looks like — arriving somewhere changes nothing on
  the far side. Typing is a read; the commit, if there is one, is the submit. Anything that commits is
  held until an approval <b>bound to that exact action</b> is presented, and then released once. An
  element that matches no rule is refused even when an approval is offered, because an approval
  authorises a described action and there is nothing here to describe.</p></div>
  <div><h3 class="dh">Try the fourth one with the switch on</h3>
  <p class="why">It still refuses. That is the whole point: an approval is a permission for one
  described action, not a key to the front door. A gate that can be talked into anything is not a gate,
  it is a formality with a good reputation.</p></div>
</div>
{honest("It does not show that the rules are good rules. They are one page of somebody's judgement about what "
        "should require approval, published so you can disagree with it. The mechanism is that they are a "
        "document and every verdict can be re-derived from it — not that this particular document is right.")}
"""

js = """
const obs = """ + json.dumps(obs[:5]) + """;
const state = obs.map((o) => !!o.approval);
async function verdict(i) {
  const j = await api('verdict?obs=' + i + '&approval=' + (state[i] ? 1 : 0));
  const box = document.querySelector('[data-vd="' + i + '"]');
  box.className = 'vd ' + j.verdict;
  box.textContent = j.class;
  box.title = j.reason;
  return j;
}
async function refresh(i, el) {
  const out = document.getElementById('out');
  try {
    const j = await verdict(i);
    show(out, j.output);
    cmdline(document.getElementById('cmdtxt'), j.command);
  } catch (e) { fail(out, e); }
}
document.querySelectorAll('[data-sw]').forEach((sw) => {
  sw.addEventListener('click', async () => {
    const i = Number(sw.dataset.sw);
    state[i] = !state[i];
    sw.classList.toggle('on', state[i]);
    await refresh(i, sw);
  });
});
(async () => {
  try {
    const j = await api('verdict?obs=0&approval=' + (state[0] ? 1 : 0));
    for (let i = 0; i < obs.length; i++) await verdict(i);
    show(document.getElementById('out'), j.output);
    cmdline(document.getElementById('cmdtxt'), j.command);
  } catch (e) { fail(document.getElementById('out'), e); }
})();
"""
page("gate.html", "mint — the gate", "Fit the gate to the effect: the real rules engine decides, live.", 1, "", "", body, js)

# ── 2. the refusal ──────────────────────────────────────────────────────────────────────────────
seq = receipts["receipts"]
ladder = []
for r in seq:
    b = r["body"]
    v = b["decision"]["verdict"] or "—"
    el = b["action"]["element"].replace('"', "")
    cls = {"allow": "allow", "hold": "hold", "refuse": "refuse"}.get(v, "")
    kind = "effect" if b["kind"] == "outcome" else "decision"
    ladder.append(f'<div class="st {cls}"><b>#{b["seq"]} {kind}</b>{el[:26]}<br>'
                  f'{v if b["kind"] == "decision" else b["status"]}</div>')

refusal = [r for r in seq if r["body"]["decision"]["verdict"] == "refuse"][0]

body = head(
    2,
    "When it does not understand, nothing happens.",
    "The strongest thing a runtime can do is nothing at all. This is the run where it met an element "
    "it could not describe, refused it, and stopped — and the record of that refusal is the only thing "
    "it left behind.",
) + f"""
<h2 class="dh">The seven receipts of that run</h2>
<p class="why">Read them as a ladder. Each rung is signed; each points at the one below it. Notice which
kind of receipt is missing where.</p>
<div class="ladder">
{chr(10).join(ladder)}
</div>
<p class="why">After the refusal at <b>#{refusal["body"]["seq"]}</b> there is no outcome receipt, and
there is not supposed to be one. An outcome receipt is a statement that something happened. Its
absence is the statement that nothing did. Both halves matter: a runtime that only records what it did
can be silent about what it tried, and silence is exactly what an audit is looking for.</p>

<h2 class="dh">The refusal, as it was signed</h2>
<div class="rct">
  <div class="k b">element</div><div class="v b">{refusal["body"]["action"]["element"]}</div>
  <div class="k">what it saw</div><div class="v">tag = {refusal["body"]["action"]["observed"]["tag"]} ·
    role = {refusal["body"]["action"]["observed"]["role"]} ·
    has a link = {"yes" if refusal["body"]["action"]["observed"]["has_href"] else "no"} ·
    approval presented = {"yes" if refusal["body"]["action"]["observed"]["approval"] else "no"}</div>
  <div class="k">would have done</div><div class="v">{refusal["body"]["action"]["side_effect"]}</div>
  <div class="k">verdict</div><div class="v"><span class="no">{refusal["body"]["decision"]["verdict"]}</span></div>
  <div class="k">why</div><div class="v">{refusal["body"]["decision"]["reason"]}</div>
  <div class="k">decided by</div><div class="v">{refusal["body"]["decision"]["decided_by"]}</div>
  <div class="k">rules it was decided under</div><div class="v">{refusal["body"]["policy_hash"][:32]}…</div>
</div>
<p class="cmdtxt"><b>the command:</b> levos receipts --in receipts.json --policy policy.json</p>
<p class="why"><a href="/gate-lab">The page it was looking at</a> — four elements, one of which is
deliberately undescribable. The runtime was pointed at it and told to file something.</p>
{honest("It does not show that refusing is always right. A runtime that refuses too much is useless, and this "
        "one has a real failure rate on pages it has never seen. What it shows is which way it fails when it "
        "does fail: the refusal is written down, signed, and visible — not a step that quietly did the wrong "
        "thing and reported success.")}
"""
page("refuse.html", "mint — when it does not understand",
     "The refusal, and the missing second receipt that proves nothing happened.", 2, "", "", body)

# ── 3. lie to the log ───────────────────────────────────────────────────────────────────────────
cases = [
    ("edited", "a line edited, signature left alone",
     "Somebody changes what a receipt says and does not have the key, so the signature no longer covers the bytes."),
    ("dropped", "a receipt deleted from the middle",
     "Somebody removes one rung of the ladder and hopes nobody counts."),
    ("swapped", "two receipts reordered",
     "The order of events is changed without changing the events."),
    ("forged", "a held write rewritten to 'allow', with the whole chain re-signed<br><b>by the real key holder</b>",
     "This is the one that matters. Every signature is valid. Every link holds. The only thing left that can catch it is the published rules, applied again to the observations the receipts actually recorded."),
    ("policy", "the rules swapped out after the log was signed",
     "Somebody keeps the log and replaces the document it claims to have followed."),
]
case_html = []
for slug, what, why in cases:
    case_html.append(f"""<h3 class="dh">{what}</h3>
<p class="why">{why}</p>
<p><button class="btn ghost" data-case="{slug}">run the verifier on this copy</button></p>
<pre class="runout" id="out-{slug}"></pre>""")

body = head(
    3,
    "Five ways to lie to the log. All five get caught.",
    "The log is signed with Ed25519 and each receipt is chained to the one before it. But signatures "
    "only stop people who do not have the key. The interesting case is the fifth one, and pressing a "
    "button runs the real verifier on a real tampered copy — this is not a video of a verifier.",
    "live",
) + f"""
<p class="why">Every button below checks a file that exists on this machine, written by the same binary
that wrote the original. The output is the verifier's own, unedited.</p>
{chr(10).join(case_html)}
<div class="grid2">
  <div><h3 class="dh">Why the fifth is the hard one</h3>
  <p class="why">If you hold the key, you can rewrite history and re-sign it, and every cryptographic
  check passes. Nothing in the chain can tell the difference, because there is no difference — the
  bytes are authentic. What catches it is that the receipt recorded the <b>observations</b>, not just
  the verdict. Re-apply the published rules to those observations and you get <i>hold</i> where the
  log says <i>allow</i>. Authentic, and wrong.</p></div>
  <div><h3 class="dh">Why the third way of lying is invisible without a chain</h3>
  <p class="why">Deleting or reordering receipts does not make any signature invalid. Each receipt is
  still perfectly signed. What breaks is the link: a receipt points at the hash of the one before it,
  and that pointer no longer describes the file it is in. Without the chain, an audit could only count
  receipts — never place them in an order.</p></div>
</div>
<p class="cmdtxt"><b>the command:</b> levos receipts --in receipts.json --policy policy.json <span
style="color:var(--dim2)">(add --tamper DIR and the binary writes all five forged copies itself, then
checks them)</span></p>
{honest("It does not show that the verifier is infallible. It shows which specific ways of lying it catches, and "
        "the list is finite and stated. A lie not on this list is not caught by this check — and the honest "
        "answer to “what about a lie you did not think of” is that the log records observations precisely so "
        "somebody else can catch the ones we missed.")}
"""

js = """
document.querySelectorAll('[data-case]').forEach((b) => {
  b.addEventListener('click', async () => {
    const slug = b.dataset.case;
    const out = document.getElementById('out-' + slug);
    out.textContent = 'running the real verifier…';
    try {
      const j = await api('tamper?case=' + slug);
      show(out, j.output);
    } catch (e) { fail(out, e); }
  });
});
"""
page("tamper.html", "mint — lie to the log", "Five ways to tamper with a signed, chained log, checked live.", 3, "", "", body, js)

# ── 4. lie to the endorsement ───────────────────────────────────────────────────────────────────
ecases = [
    ("0", "signed by the runtime's own key",
     "A perfectly valid signature — from the same party that wrote the rules. If a verifier accepts this, it will report a counter-signature where there is only one party agreeing with itself, which is worse than having none at all: the reader stops looking for the missing half."),
    ("1", "covering a different policy",
     "An honest signature over the wrong document, hoping nobody compares hashes."),
    ("2", "naming a different runtime",
     "These rules, accepted — but for somebody else's runtime."),
    ("3", "already expired",
     "A spent endorsement. Whatever it once authorised, it does not authorise it now."),
]
ecase_html = []
for n, what, why in ecases:
    ecase_html.append(f"""<h3 class="dh">{what}</h3>
<p class="why">{why}</p>
<p><button class="btn ghost" data-case="{n}">check the endorsement</button></p>
<pre class="runout" id="eout-{n}"></pre>""")

ebody = head(
    4,
    "Four ways to lie about who agreed.",
    "A receipt proves we did the work. It cannot prove anybody wanted it — a runtime signing its own "
    "rules is an author approving their own book. That takes a second key, held by somebody else. "
    "These are the four ways a second signature can be faked, and each is rejected for its own reason.",
    "live",
) + f"""
<div class="grid2">
  <div><h3 class="dh">The one that is on file</h3>
  <div class="rct">
    <div class="k b">signed by</div><div class="v">{endorsement["body"]["endorser"][:32]}…</div>
    <div class="k">the runtime</div><div class="v">{receipts["agent"][:32]}…</div>
    <div class="k">about</div><div class="v">policy {endorsement["body"]["policy_hash"][:32]}…</div>
    <div class="k">scope</div><div class="v">{endorsement["body"]["scope"]}</div>
    <div class="k">until</div><div class="v">{endorsement["body"]["not_after_ms"]}</div>
    <div class="k">note</div><div class="v">{endorsement["body"]["note"]}</div>
  </div></div>
  <div><h3 class="dh">What a counter-signature is worth</h3>
  <p class="why">Exactly what the key behind it is worth — no more, and no amount of code changes that.
  Two different key strings on the same machine prove the mechanism and nothing about trust, which is
  why the note above says so in the artifact itself rather than only in a pitch.</p></div>
</div>
<h2 class="dh">The four forgeries</h2>
{chr(10).join(ecase_html)}
<p class="cmdtxt"><b>the command:</b> levos receipts --in receipts.json --policy policy.json
--endorsement endorsement.json</p>
{honest("It does not show that a real counterparty exists. Nobody outside this machine has signed anything here, "
        "and no code can supply that. The endorser's key is generated on this machine by a command that says "
        "out loud what it is for. A genuine endorsement needs a person or an organisation on the other side who "
        "was asked and agreed, and that is the one part of this that has to be bought rather than built.")}
"""
ejs = """
document.querySelectorAll('[data-case]').forEach((b) => {
  b.addEventListener('click', async () => {
    const slug = b.dataset.case;
    const out = document.getElementById('eout-' + slug);
    out.textContent = 'checking the endorsement…';
    try {
      const j = await api('endorse?case=' + slug);
      show(out, j.output);
    } catch (e) { fail(out, e); }
  });
});
"""
page("endorse.html", "mint — lie to the endorsement",
     "Four ways to fake a second party's signature on the rules, checked live.", 4, "", "", ebody, ejs)

# ── 5. read a receipt ───────────────────────────────────────────────────────────────────────────
names = []
for r in receipts["receipts"]:
    b = r["body"]
    label = f'#{b["seq"]} ' + ("effect" if b["kind"] == "outcome" else "decision")
    names.append(f'<button data-r="{b["seq"]}">{label}</button>')

rbody = head(
    5,
    "One receipt, field by field.",
    "This is the entire format. Pick any of the seven from the published log and read it the way a "
    "stranger would: what it asserts, what the signature covers, and what the sentence at the bottom "
    "refuses to assert.",
) + f"""
<div class="pick">{''.join(names)}</div>
<div class="rct" id="rct"></div>
<h3 class="dh" id="which" style="margin-top:26px">what this proves, and what it does not</h3>
<p class="why" id="claim">{receipts["claim"]}</p>
<h3 class="dh">The two things a receipt is not allowed to leave out</h3>
<p class="why">It carries the <b>observations</b> the runtime acted on — the tag, the role, whether
there was a link, whether an approval was presented — and not merely its own conclusion. That is what
lets someone else re-apply the rules and disagree. And it carries its own <b>limit</b>, in the claim
above, so that a reader who only ever sees one receipt cannot mistake it for a statement about whether
the work was correct.</p>
<p class="cmdtxt"><b>the file:</b> <a href="/receipts.json">/receipts.json</a> — the whole log, as
published. This page reads that same file.</p>
{honest("It does not show that the elements named in the receipt were the ones on the page. The runtime attests to "
        "what it observed, not to the page it observed it on. Attesting to the environment needs a third party who "
        "can see both, and this does not have one.")}
"""
rjs = """
let log = null;
function row(k, v, bold) {
  return '<div class="k' + (bold ? ' b' : '') + '">' + k + '</div><div class="v' +
         (bold ? ' b' : '') + '">' + esc(v) + '</div>';
}
function render(seq) {
  if (!log) return;
  const r = log.receipts.find((x) => x.body.seq === seq);
  if (!r) return;
  const b = r.body;
  const o = b.action.observed;
  let h = '';
  h += row('receipt', '#' + b.seq + ' of ' + log.count + ' · ' + b.kind, true);
  h += row('id', b.id, true);
  h += row('the element', b.action.element);
  h += row('what it would do', b.action.side_effect);
  h += row('what it saw', 'tag=' + o.tag + ' role=' + o.role +
           ' has_link=' + (o.has_href ? 'yes' : 'no') + ' approval=' + (o.approval ? 'yes' : 'no'));
  h += row('verdict', b.decision.verdict);
  h += row('why', b.decision.reason || '(the rules allow this)');
  h += row('decided by', b.decision.decided_by + (b.decision.decided_by === 'once-only'
      ? ' — from what this run had already done, which no rule can see' : ' — any holder of the rules reaches this verdict again'));
  h += row('status', b.status || '(no effect to report)');
  h += row('effect of this action', b.action.identity);
  h += row('approval, once-only key', b.action.key);
  h += row('decided at', b.decided_at_ms + ' (the signer\\'s clock)');
  h += row('refers to', b.refers_to || '—');
  h += row('rules in force', b.policy_hash);
  h += row('previous receipt hashed to', b.prev_hash || '(this is the first)');
  h += row('signature', r.sig.alg + ':' + r.sig.value);
  h += row('signed by', r.sig.kid);
  h += row('the claim', b.claim);
  document.getElementById('rct').innerHTML = h;
  document.getElementById('which').textContent =
      '#' + b.seq + ': ' + (b.kind === 'outcome' ? 'an effect, recorded after it happened'
                                                 : 'a decision, recorded before it happened');
  document.querySelectorAll('.pick button').forEach((x) =>
      x.classList.toggle('on', Number(x.dataset.r) === seq));
}
document.querySelectorAll('.pick button').forEach((b) =>
    b.addEventListener('click', () => render(Number(b.dataset.r))));
(async () => {
  try {
    const r = await fetch('/receipts.json', {cache: 'no-store'});
    log = await r.json();
    render(0);
  } catch (e) { document.getElementById('rct').innerHTML =
      '<div class="k b">error</div><div class="v b">could not read the log</div>'; }
})();
"""
page("receipt.html", "mint — read a receipt", "The receipt format, field by field, from the published log.", 5, "", "", rbody, rjs)

# ── 6. what learning costs ──────────────────────────────────────────────────────────────────────
skills = measurements["skills"]
rows6 = []
for name, s in skills.items():
    rows6.append(f"""<tr><td class="m">{name}</td><td>{s["steps"]}</td><td class="m">{s["bytes"]}</td>
<td class="m">{s["learn_model_calls"]}</td><td class="m">{s["learn_ms"] / 1000:.1f}s</td>
<td>{"<span class='yes'>verified</span>" if s["verified"] else "<span class='warnc'>not yet verified on every shape</span>"}</td></tr>""")

body = head(
    6,
    "What learning costs, and what using it costs.",
    "A task is solved once with a model, and what comes out the other side is not a recording — it is a "
    "small program of typed steps that describe the shape of what to act on. Here is what that costs, "
    "measured, including the one that is not finished.",
) + f"""
<h2 class="dh">Four skills, learned from four real repositories</h2>
<table class="rt">
  <tr><th>skill</th><th>steps</th><th>size</th><th>model calls to learn</th><th>time to learn</th><th>state</th></tr>
  {chr(10).join(rows6)}
</table>
<p class="why">Learning costs three or four questions to a model and about three seconds. After that the
skill is a file under a kilobyte: {skills["gh-commit"]["bytes"]} to {max(s["bytes"] for s in skills.values())}
bytes of typed steps. Running it calls no model at all.</p>

<h2 class="dh">The same eighteen tasks, and when a model was needed</h2>
<p class="why">Eighteen real tasks against six real repositories. A model was needed for
{evaluation["summary"]["tasks"] - evaluation["summary"]["zero_model_calls"]} of them, and for
{evaluation["summary"]["zero_model_calls"]} of them nothing was asked.</p>
<table class="rt" id="tbl6"></table>
<p class="cmdtxt"><b>the file:</b> <a href="/measurements.json">/measurements.json</a> ·
<a href="/evaluation.json">/evaluation.json</a></p>
{honest("It does not show that the second run is free in general. It is free when the page still matches the shape "
        "the skill was compiled against; when it does not, the runtime stops, asks about that one step, and pays "
        "for that step. The claim is not zero cost — it is that a mistake costs one question rather than a broken "
        "run, and that the same question is not paid for twice.")}
"""
j6 = """
(async () => {
  const e = await (await fetch('/evaluation.json', {cache:'no-store'})).json();
  let h = '<tr><th>repository</th><th>task</th><th>decisions</th><th>model calls</th><th>time</th><th></th></tr>';
  for (const r of e.runs) {
    const zero = r.model_calls === 0;
    h += '<tr><td class="m">' + esc(r.target) + '</td><td>' + esc(r.goal) + '</td><td class="m">' +
         r.decisions + '</td><td class="m">' + (zero ? '<span class="yes">0</span>' : r.model_calls) +
         '</td><td class="m">' + (r.wall_ms / 1000).toFixed(2) + 's</td><td>' +
         (r.success ? '<span class="yes">done</span>' : '<span class="no">failed</span>') + '</td></tr>';
  }
  document.getElementById('tbl6').innerHTML = h;
})();
"""
page("learn.html", "mint — what learning costs", "Measured cost of learning a task once and running it thereafter.", 6, "", "", body, j6)

# ── 7. eighteen tasks ───────────────────────────────────────────────────────────────────────────
body = head(
    7,
    "Eighteen tasks against six real repositories.",
    "These are the actual runs, with their actual numbers: how many decisions, whether the model was "
    "needed, how long, and whether the step landed on the thing it was supposed to land on.",
) + f"""
<table class="rt" id="tbl7"></table>
<h2 class="dh">What each column means</h2>
<div class="grid2">
  <div><p class="why"><b>decisions</b> is how many times the runtime had to decide what to act on. The first
  task on a repository usually needs two; the shape is new. The next ones need one, because the same
  shape was already compiled.</p>
  <p class="why"><b>model calls</b> is how many of those decisions had to be asked about. Zero means the
  compiled skill recognised the page on its own.</p></div>
  <div><p class="why"><b>landed</b> is what the runtime was expected to reach — a commit page, an issue
  list, a pull request list — and it either did or did not.</p>
  <p class="why">Nothing here is averaged into a score. A number that hides which task failed is not a
  measurement, it is a claim about measurements.</p></div>
</div>
<p class="cmdtxt"><b>the file:</b> <a href="/evaluation.json">/evaluation.json</a> — every run, with its
per-run receipts</p>
{honest("It does not show that this generalises past the family of pages it was tested on. Clean, well-marked pages "
        "with stable structures are the easy case, and these are all such pages. Four separate failures on other "
        "pages are named elsewhere on the site rather than folded into a percentage.")}
"""
j7 = """
(async () => {
  const e = await (await fetch('/evaluation.json', {cache:'no-store'})).json();
  let h = '<tr><th>repository</th><th>what it was asked to do</th><th>expected to land</th><th>landed</th>' +
          '<th>decisions</th><th>model</th><th>time</th><th>receipts</th></tr>';
  for (const r of e.runs) {
    h += '<tr><td class="m">' + esc(r.target) + '</td><td>' + esc(r.goal) + '</td>' +
         '<td class="m">' + esc(r.expected) + '</td><td class="m">' + esc(r.landed || '—') + '</td>' +
         '<td class="m">' + r.decisions + '</td><td class="m">' + (r.model_calls === 0
             ? '<span class="yes">0</span>' : r.model_calls) + '</td><td class="m">' +
         (r.wall_ms / 1000).toFixed(2) + 's</td><td class="m">' +
         (r.receipts && r.receipts.signed ? r.receipts.count + ' signed' : '—') + '</td></tr>';
  }
  document.getElementById('tbl7').innerHTML = h;
})();
"""
page("tasks.html", "mint — eighteen real tasks", "Eighteen real tasks against six repositories, per-run numbers.", 7, "", "", body, j7)

# ── 8. what it hands a model ────────────────────────────────────────────────────────────────────
rows8 = []
for p in measurements["pages"]:
    rows8.append(f"""<tr><td class="m">{p["url"].replace("https://github.com/", "")}</td>
<td class="m">{p["elements"]}</td><td class="m">{p["capture_ms_median"]} ms</td>
<td class="m">{p["state_bytes"]:,}</td><td class="m">{p["naive_tree_bytes"]:,}</td>
<td class="m"><b>{p["reduction_vs_tree"]}×</b></td></tr>""")

body = head(
    8,
    "What it hands to a model, and what it refuses to hand over.",
    "Most of the cost and most of the mistakes in this kind of agent come from what gets sent to the "
    "model. So the runtime sends a small typed summary of a page instead of the page, and here is what "
    "that comes to on three real repositories.",
) + f"""
<table class="rt">
  <tr><th>repository</th><th>elements</th><th>to read the page</th><th>sent to the model</th>
  <th>a plain DOM dump would be</th><th>smaller by</th></tr>
  {chr(10).join(rows8)}
</table>
<p class="why">Reading a page takes single-digit milliseconds. The summary is around
{measurements["pages"][0]["state_bytes"] // 1000}–{max(p["state_bytes"] for p in measurements["pages"]) // 1000} kB.
The same page serialised as a DOM tree is around a megabyte and a few thousand nodes, and that is what
most agents in this category send — every time, at every step.</p>
<h2 class="dh">Why this is a correctness argument, not a cost argument</h2>
<p class="why">Two reasons, and neither is about money. First, a model given a megabyte of markup
answers about the markup: it has more to look at and less to be sure of. Second, and worse, a step
compiled against raw markup breaks when the markup changes — and markup changes on every page view,
for reasons that have nothing to do with the task. A step compiled against a <b>shape</b> survives a
deployment of the site it was learned on. That is the difference between a recording and a program.</p>
<p class="cmdtxt"><b>the file:</b> <a href="/measurements.json">/measurements.json</a> — captured
{measurements["collected_at"]}</p>
{honest("It does not show that the summary is sufficient for every task. It carries what the runtime knows how to act "
        "on: roles, names, whether something has a destination. A task that depends on a number buried in a table "
        "cell, or on a colour, or on text that is not a name, is not served by this summary — and the runtime "
        "refuses such a step rather than guessing at it.")}
"""
page("state.html", "mint — what a model is given",
     "Page capture measured on real repositories: what is sent to a model and what is not.", 8, "", "", body)

# ── 9. a run you start yourself ─────────────────────────────────────────────────────────────────
# The five tasks the relay will run. Kept in step with LIVE_TASKS in demo/relay.py by reading that
# file, so a task added there appears here rather than going unmentioned.
import ast

with open(os.path.join(DEMO, "relay.py"), encoding="utf-8") as fh:
    relay_src = fh.read()
m = re.search(r"LIVE_TASKS = \[(.*?)\n\]", relay_src, re.S)
assert m, "LIVE_TASKS is not where it was in relay.py"
live_tasks = ast.literal_eval("[" + m.group(1) + "]")

task_rows = []
for t in live_tasks:
    watch = "the gate holds this one" if "write" in t["skill"] else "0 model calls, in a real browser"
    task_rows.append(f"""  <div class="tryrow livetask" data-task="{t["n"]}">
    <div><div class="el">{t["what"]}</div><div class="ob">{t["start"]} · {watch}</div></div>
    <button class="btn" data-run="{t["n"]}">run it</button>
  </div>""")

body = head(
    9,
    "Press a button. It goes and does the thing.",
    "The other demonstrations show what the machinery decides. This one does not decide anything: it "
    "opens a real browser, goes to a real page, and works — while you watch. The frames below are the "
    "browser as it moves, the steps are what it did, and the receipts at the end are what it can prove.",
    "live, on demand",
) + f"""
<h2 class="dh">Pick a task</h2>
<p class="why">Each of these is a skill the runtime learned once and still has. The first three run on
repositories it has never seen — the same skill, learned on one project, executing on another. The last
two are on our own page: one reads it, one tries to submit a form and gets stopped.</p>
<div class="try">
{chr(10).join(task_rows)}
</div>
<p class="hint" id="livehint">One run at a time — the browser is a single shared thing. If somebody else
is mid-run you will be told, and nothing will be half-recorded.</p>

<h2 class="dh">The run</h2>
<div class="livewrap">
  <div class="shotbox"><img id="liveframe" alt="the browser, as the runtime drives it">
    <div id="liveempty">nothing running — pick a task above</div></div>
  <div class="livemeta">
    <div class="lm"><span class="k">goal</span><span id="livegoal">—</span></div>
    <div class="lm"><span class="k">where it is</span><span id="liveurl">—</span></div>
    <div class="lm"><span class="k">elapsed</span><span id="livetime">—</span></div>
    <div class="lm"><span class="k">model calls</span><span id="livemodel">—</span></div>
    <div class="lm"><span class="k">held by the gate</span><span id="liveheld">—</span></div>
  </div>
</div>
<ol class="livesteps" id="livesteps"></ol>
<div id="liveresult"></div>

<h2 class="dh">Why this is the honest version of a demo</h2>
<p class="why">A recorded video proves that something worked once, under conditions the person who
recorded it chose. A page that runs the real thing in front of you proves what happened just then —
including when it goes wrong, which it does: the third task runs on a page it has never seen and
occasionally has to ask a model about a step, and you will see that in the counters rather than read
that it was smoothed out. The run writes a signed log as it goes, and the button at the end checks it
with the same verifier a stranger would use.</p>
{honest("It does not show that it will work on your page. Three repositories with clean, well-marked structure are "
        "the easy case, and these are all such pages. It also does not show that the first run was free: the skill "
        "running here was learned once with a model in the loop, and that cost is paid before any of this. What you "
        "are watching is the second time.")}
"""

page("live.html", "mint — run it yourself, now",
     "Start a real task and watch the runtime do it in a real browser.", 9, "", "", body, """
let polling = null;
function tierBadge(t) {
  const cls = t === 'gate' ? 'vd refuse' : (t === 'model' ? 'vd hold' : 'vd allow');
  return '<span class="' + cls + '">' + esc(t) + '</span>';
}
function renderSteps(steps) {
  const ol = document.getElementById('livesteps');
  ol.innerHTML = steps.map((s) =>
    '<li><span class="sn">' + s.n + '</span>' + tierBadge(s.tier) +
    '<span class="sl">' + esc(s.element) + '</span>' +
    '<span class="sn2">' + (s.tier === 'skill' ? (s.decision_us / 1000).toFixed(2) + ' ms' : '') +
    '</span><span class="note2">' + esc(s.note || '') + '</span></li>').join('');
}
function render(d) {
  document.getElementById('livegoal').textContent = d.goal || '—';
  document.getElementById('liveurl').textContent = d.url || '—';
  document.getElementById('livetime').textContent = d.total_ms ? (d.total_ms / 1000).toFixed(1) + ' s' : '—';
  document.getElementById('livemodel').textContent = d.model_calls;
  document.getElementById('liveheld').textContent = d.held;
  if (d.frame) {
    document.getElementById('liveframe').src = d.frame;
    document.getElementById('liveframe').style.display = 'block';
    document.getElementById('liveempty').style.display = 'none';
  }
  renderSteps(d.steps || []);
}
function stopPolling() { if (polling) { clearTimeout(polling); polling = null; } }
async function tick() {
  let d;
  try {
    d = await (await fetch('/live-state', {cache: 'no-store'})).json();
  } catch (e) { polling = setTimeout(tick, 900); return; }
  render(d);
  if (d.running || !d.finished) {
    polling = setTimeout(tick, 550);
  } else {
    polling = null;
    await finish();
  }
}
async function finish() {
  const box = document.getElementById('liveresult');
  let r;
  try { r = await api('live/result'); } catch (e) { box.innerHTML = '<p class="why">' + esc(e.message) + '</p>'; return; }
  const rec = r.receipts || {};
  box.innerHTML =
    '<h3 class="dh">What it just did</h3>' +
    '<div class="rct">' +
      '<div class="k b">the verdict</div><div class="v b">' + esc(r.verdict || '(no verdict reported)') + '</div>' +
      '<div class="k">steps performed</div><div class="v">' + r.steps_done + '</div>' +
      '<div class="k">steps it had to ask about</div><div class="v">' + r.asked + '</div>' +
      '<div class="k">steps the gate held</div><div class="v">' + r.held + '</div>' +
      '<div class="k">took</div><div class="v">' + (r.took_ms / 1000).toFixed(1) + ' s</div>' +
      '<div class="k">signed receipts</div><div class="v">' + (rec.signed ? rec.count : 'none') +
        (rec.verified ? ' — and they verify' : '') + '</div>' +
    '</div>' +
    (rec.signed ? '<p><button class="btn ghost" id="checklog">check this log with the verifier</button></p>' +
      '<pre class="runout" id="logcheck"></pre>' : '');
  const btn = document.getElementById('checklog');
  if (btn) btn.addEventListener('click', async () => {
    const out = document.getElementById('logcheck');
    out.textContent = 'checking…';
    try { const j = await api('live/verify'); show(out, j.output); } catch (e) { fail(out, e); }
  });
}
document.querySelectorAll('[data-run]').forEach((b) => {
  b.addEventListener('click', async () => {
    stopPolling();
    const box = document.getElementById('liveresult');
    box.innerHTML = '';
    document.getElementById('livesteps').innerHTML = '';
    document.getElementById('liveempty').style.display = 'block';
    document.getElementById('liveempty').textContent = 'starting — the browser is opening…';
    document.getElementById('liveframe').style.display = 'none';
    document.querySelectorAll('[data-run]').forEach((x) => x.disabled = true);
    try {
      const j = await api('live/start?task=' + b.dataset.run);
      document.getElementById('livehint').textContent = 'running: ' + j.goal;
      polling = setTimeout(tick, 500);
    } catch (e) {
      document.getElementById('livehint').textContent = e.message;
    } finally {
      document.querySelectorAll('[data-run]').forEach((x) => x.disabled = false);
    }
  });
});
""")

# ── the hub ─────────────────────────────────────────────────────────────────────────────────────
hub = [
    ("live", 9, "Run it yourself, right now", "Pick a task and the runtime opens a real browser and does it while you watch — the frames as it moves, the steps with what resolved them, and a signed receipt at the end. One of the five gets stopped by the gate mid-run. <b>Live.</b>", "live", True),
    ("gate", 1, "Fit the gate to the effect", "Flip an approval switch and the real rules engine decides, live: reading passes, writing waits, and the undescribable is refused even with approval. <b>Interactive.</b>", "live", True),
    ("refuse.html", 2, "When it does not understand, nothing happens", "The run where the runtime met an element it could not describe. The absence of an outcome receipt is the evidence that nothing took place.", "", False),
    ("tamper.html", 3, "Five ways to lie to the log", "Edit a line, delete a receipt, reorder two, rewrite a verdict and re-sign the whole chain with the real key, or swap the rules. The verifier runs on each forged copy in front of you. <b>Interactive.</b>", "live", True),
    ("endorse.html", 4, "Four ways to lie about who agreed", "A second key signs the rules. Four forgeries, each rejected for its own reason — starting with the one where the runtime signs its own rules. <b>Interactive.</b>", "live", True),
    ("receipt.html", 5, "Read a receipt, field by field", "The whole format, from the published log: what it asserts, what the signature covers, and the sentence at the bottom that refuses to assert the rest. <b>Interactive.</b>", "live", True),
    ("learn.html", 6, "What learning costs, and what using it costs", "Four real skills, measured: three or four questions to a model to learn one, and none at all to run it afterwards.", "", False),
    ("tasks.html", 7, "Eighteen tasks against six repositories", "Every run with its own numbers, including which ones the model was needed for. No scores, no averages.", "", False),
    ("state.html", 8, "What it hands to a model", "A page read in milliseconds and summarised to tens of kilobytes, against the megabyte of markup most agents send. Measured on three real repositories.", "", False),
]
cards = []
for href, n, title, desc, tagx, live in hub:
    badge = f'<span class="tagx{" live" if live else ""}">{"live" if live else "real run, printed"} </span>'
    cards.append(f"""<a class="demo" href="/demos/{href.replace(".html", "")}">
  <div class="n">demonstration {n}</div>
  <h3>{title}</h3>
  <p>{desc}</p>
  {badge}
</a>""")

body = head(
    0,
    "Eight ways to try to catch it lying.",
    "Every demonstration here runs the real program. Nothing is a mock-up, a screenshot or a video: when "
    "you press a button, the runtime's own verifier runs and its output is printed below, unedited. All "
    "eight can be reproduced from a shell with the command each page prints.",
) + f"""
<div class="demos">
{chr(10).join(cards)}
</div>
<h2 class="dh">Run all of it yourself, without us</h2>
<p class="why">Four of the eight are interactive and call this machine when you press a button — which
is convenient, and worth nothing as evidence, because you are still taking our word for it. The other
four print what a run produced. Either way, the point is that none of it needs our permission:</p>
<pre class="runout">curl -O https://mint.arliwork.com/receipts.json
curl -O https://mint.arliwork.com/policy.json
curl -O https://mint.arliwork.com/endorsement.json
levos receipts --in receipts.json --policy policy.json --endorsement endorsement.json</pre>
<p class="why">That verifies every signature, walks the hash chain, re-applies the published rules to
every recorded decision, checks the counter-signature, and prints the receipt where anything first
disagrees. The log and the rules are the whole input; nothing is fetched from us at check time, and
nothing about the check depends on the program that produced the log.</p>
<p class="cmdtxt"><b>the files behind the demonstrations:</b>
<a href="/receipts.json">/receipts.json</a> ·
<a href="/policy.json">/policy.json</a> ·
<a href="/endorsement.json">/endorsement.json</a> ·
<a href="/evaluation.json">/evaluation.json</a> ·
<a href="/measurements.json">/measurements.json</a></p>
<div class="honest"><h5>what no demonstration here shows</h5><p>A real counterparty. Every key on this
site was generated on this machine, so the demonstrations prove the mechanism and say nothing about who
is behind it. Nor do they show that the work done was the work wanted — only that what was recorded
truthfully describes what was done, signably and checkably. Those two gaps are the ones worth arguing
about, and they are stated here rather than left for a reader to find.</p></div>
"""
page("index.html", "mint — demonstrations", "Eight demonstrations, four of them live, all from real runs.", 0, "", "", body)

print(f"\n  {len(hub)} pages + hub written to {OUT}")
