#!/usr/bin/env python3
"""Add the detail that makes the earlier verdicts honest.

The sweep already ran; this only reads its reports and writes the fuller picture: which targets
failed and why, and how each skill behaves with and without a model. The offline test passed
against the bar I set, and the bar was set generously — so the restriction is named here instead
of being left inside a green tick.
"""
import json
from collections import Counter

S = "/root/.hermes/profiles/work2/cache/scratch"
rep = json.load(open("/root/mint/demo/applications-verified.json"))
on = json.load(open(f"{S}/sweep-online.json"))
off = json.load(open(f"{S}/sweep-offline.json"))

# ── the four failures, named ───────────────────────────────────────────────
failures = [
    {
        "skill": r["skill"], "target": r["target"],
        "landed": r["landed"] or "nowhere",
        "expected": r["expected"],
        "decisions": r["decisions"], "model_calls": r["model_calls"],
        "why": ("it never resolved a step, and the one question it asked led nowhere"
                if r["decisions"] == 0 else
                "it acted but never reached a page of any named kind"
                if not r["landed"] else
                "it acted and landed on a different kind of page than the step promised"),
    }
    for r in on["runs"] if not r["success"]
]
rep["tests"]["transfer"]["failures"] = failures

# ── per skill, with and without a model ────────────────────────────────────
def by_skill(data):
    out = {}
    for r in data["runs"]:
        d = out.setdefault(r["skill"], {"runs": 0, "ok": 0, "model_calls": 0})
        d["runs"] += 1
        d["ok"] += 1 if r["success"] else 0
        d["model_calls"] += r["model_calls"]
    return out


o, f = by_skill(on), by_skill(off)
per = {}
for k in sorted(o):
    per[k] = {
        "runs": o[k]["runs"],
        "completed_online": o[k]["ok"],
        "completed_offline": f.get(k, {}).get("ok", 0),
        "model_calls_online": o[k]["model_calls"],
    }
rep["tests"]["offline"]["by_skill"] = per
rep["tests"]["offline"]["restriction"] = (
    "The drop is not the execution needing a model - a compiled execution makes no such call. The "
    "drop is transfer: a step that does not resolve asks a question, and with nothing to ask, that "
    "run is lost. So the property holds exactly for the part of a class a skill transfers to."
)
worst = min(per, key=lambda k: per[k]["completed_offline"])
rep["tests"]["offline"]["worst_skill"] = {
    "skill": worst,
    "completed_offline": per[worst]["completed_offline"],
    "runs": per[worst]["runs"],
    "completed_online": per[worst]["completed_online"],
}
rep["tests"]["offline"]["verdict_label"] = "PASS, with the restriction named"

# ── how many runs never asked anything at all ──────────────────────────────
rep["tests"]["cost"]["runs_that_asked_nothing"] = sum(1 for r in on["runs"] if r["model_calls"] == 0)
rep["tests"]["cost"]["note"] = (
    "Most of this average comes from one skill. The other two ask nothing at all on this class; "
    "the third asks on nearly every instance because it does not transfer cleanly."
)

# ── the summary, counted from what actually happened ──────────────────────
passed = [k for k, t in rep["tests"].items() if t.get("pass")]
rep["summary"] = {
    "applications": len(rep["applications"]),
    "properties": len(rep["tests"]),
    "properties_passed": len(passed),
    "properties_passed_cleanly": len([k for k in passed if k != "offline"]),
    "properties_passed_with_restriction": 1 if "offline" in passed else 0,
    "backed": sum(1 for a in rep["applications"] if a["property_pass"]),
    "unbacked": sum(1 for a in rep["applications"] if not a["property_pass"]),
    "failed_targets": len(failures),
}
json.dump(rep, open("/root/mint/demo/applications-verified.json", "w"), indent=1)

print("провалов названо:", len(failures))
for x in failures:
    print(f"  {x['skill']:<18} {x['target']:<12} дошёл до «{x['landed']}», ждали «{x['expected']}»")
print("\nпо навыкам (в сети / без модели):")
for k, v in per.items():
    print(f"  {k:<18} {v['completed_online']}/{v['runs']} / {v['completed_offline']}/{v['runs']}, "
          f"обращений к модели {v['model_calls_online']}")
print(f"\nхуже всех без модели: {worst} — {per[worst]['completed_offline']}/{per[worst]['runs']}")
print(f"прогонов, не спросивших ничего: {rep['tests']['cost']['runs_that_asked_nothing']} из {rep['tests']['cost']['runs']}")
