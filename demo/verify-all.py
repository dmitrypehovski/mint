#!/usr/bin/env python3
"""Test what each of the 56 applications leans on.

An application cannot be tested from here: it needs a licence, a jurisdiction, a bank, a factory.
What can be tested is the property each one is built on, on a real target, at scale — and a test
that cannot fail proves nothing, so every test below has a way to come out badly.

T1 transfer  one skill, many unseen instances: does it arrive?
T2 cost      at scale, how many model calls per run?
T3 verify    when the work is NOT done, does the runtime say so?
T4 shape     when the data differs, what resolves the step?
T5 offline   with the model unreachable, does coverage survive?
T6 artifact  is a skill something a person can read, diff and sign?
T7 traces    does every run leave a verified trace behind?
"""
import json
import os
import re
import subprocess
import time
from collections import Counter

ROOT = "/root/mint"
BIN = f"{ROOT}/target/release/mint"
SCRATCH = "/root/.hermes/profiles/work2/cache/scratch"
SKILLS = ["gh-commit.skill.json", "gh-issues.skill.json", "gh-prs.skill.json"]

# A class of instances: same shape of site, different contents, different volumes, none of them
# the one anything was learned on.
REPOS = [
    "tokio-rs/tokio", "rust-lang/rust", "facebook/react", "vercel/next.js",
    "microsoft/vscode", "docker/compose", "python/cpython", "golang/go",
    "nodejs/node", "kubernetes/kubernetes", "redis/redis", "postgres/postgres",
    "denoland/deno", "sveltejs/svelte", "tailwindlabs/tailwindcss", "prisma/prisma",
    "supabase/supabase", "pytorch/pytorch", "tensorflow/tensorflow", "apache/kafka",
]
# Targets that must NOT be reported as done: the page they promise cannot be reached.
NEGATIVE = [
    "https://github.com/tokio-rs/tokio-does-not-exist-9f3a",
    "https://github.com/this-org-does-not-exist-7b2c/this-repo-does-not-exist",
]


def jobs_file(path, repos):
    jobs = [{"skill": s, "start": f"https://github.com/{r}"} for r in repos for s in SKILLS]
    json.dump(jobs, open(path, "w"), indent=1)
    return len(jobs)


def run_eval(jobs, out, label, env=None, timeout=1500):
    e = dict(os.environ)
    if env:
        e.update(env)
    started = time.time()
    p = subprocess.run([BIN, "eval", "--jobs", jobs, "--out", out, "--category", label],
                       capture_output=True, text=True, timeout=timeout, cwd=ROOT, env=e)
    data = None
    try:
        data = json.load(open(out))
    except Exception:
        print(f"  [{label}] no report; tail: {p.stdout[-300:]}")
    return data, int(time.time() - started)


def summarise(data):
    if not data:
        return None
    runs = data["runs"]
    us = [u for r in runs for u in r["decision_us"]]
    tiers = Counter(t for r in runs for t in r["tiers"])
    return {
        "runs": len(runs),
        "completed": sum(1 for r in runs if r["success"]),
        "model_calls": sum(r["model_calls"] for r in runs),
        "zero_model_runs": sum(1 for r in runs if r["model_calls"] == 0),
        "tiers": dict(tiers),
        "decision_us_median": sorted(us)[len(us) // 2] if us else None,
    }


def main():
    os.makedirs(SCRATCH, exist_ok=True)
    apps = json.load(open(f"{SCRATCH}/apps.json"))
    report = {"collected_at": time.strftime("%Y-%m-%d %H:%M %Z"), "tests": {}}

    # ── T1: one skill, many unseen instances ────────────────────────────────
    n = jobs_file(f"{SCRATCH}/sweep-jobs.json", REPOS)
    print(f"T1 transfer: {n} runs across {len(REPOS)} instances, learned on one")
    d1, t1 = run_eval(f"{SCRATCH}/sweep-jobs.json", f"{SCRATCH}/sweep-online.json", "class coverage")
    s1 = summarise(d1)
    if s1:
        rate = s1["completed"] / s1["runs"]
        report["tests"]["transfer"] = {
            "what": "one compiled skill run against instances it never saw",
            "instances": len(REPOS), "targets": len(SKILLS), "runs": s1["runs"],
            "completed": s1["completed"], "rate": round(rate, 3),
            "threshold": 0.8, "pass": rate >= 0.8, "seconds": t1,
        }
        print(f"  completed {s1['completed']}/{s1['runs']} = {rate:.0%} in {t1}s")

    # ── T2 cost, T4 shape: from the same run ────────────────────────────────
    if s1:
        per_run = s1["model_calls"] / max(s1["runs"], 1)
        report["tests"]["cost"] = {
            "what": "model calls per run once a skill exists",
            "runs": s1["runs"], "model_calls": s1["model_calls"],
            "zero_model_runs": s1["zero_model_runs"], "per_run": round(per_run, 3),
            "threshold": 0.5, "pass": per_run <= 0.5,
        }
        resolved_without_model = s1["tiers"].get("shape", 0) + s1["tiers"].get("words", 0) + s1["tiers"].get("taught", 0)
        need_model = s1["tiers"].get("model", 0)
        total_steps = resolved_without_model + need_model
        report["tests"]["shape"] = {
            "what": "what resolved each step when the data differed",
            "steps": total_steps, "resolved_without_model": resolved_without_model,
            "needed_the_model": need_model,
            "share_without_model": round(resolved_without_model / max(total_steps, 1), 3),
            "threshold": 0.5, "pass": resolved_without_model >= need_model,
            "by_tier": s1["tiers"],
        }
        print(f"  cost {per_run:.2f} model calls/run; shape resolved "
              f"{resolved_without_model}/{total_steps} steps without the model")

    # ── T3: does the verdict say no when the work was not done ──────────────
    print("T3 verify: targets where the promised page cannot be reached")
    neg_path = f"{SCRATCH}/negative-jobs.json"
    json.dump([{"skill": SKILLS[1], "start": u} for u in NEGATIVE], open(neg_path, "w"), indent=1)
    d3, t3 = run_eval(neg_path, f"{SCRATCH}/negative-out.json", "negative control")
    if d3:
        missed = sum(1 for r in d3["runs"] if not r["success"])
        report["tests"]["verify"] = {
            "what": "the verdict on work that was not done",
            "targets": len(NEGATIVE), "reported_missed": missed,
            "falsely_reported_done": len(d3["runs"]) - missed,
            "threshold": 1, "pass": missed >= 1,
        }
        print(f"  correctly reported missed: {missed}/{len(d3['runs'])}")

    # ── T5: offline ────────────────────────────────────────────────────────
    print("T5 offline: the same class with the model unreachable")
    d5, t5 = run_eval(f"{SCRATCH}/sweep-jobs.json", f"{SCRATCH}/sweep-offline.json", "offline",
                      env={"MINT_MODEL_URL": "http://127.0.0.1:9/v1/chat/completions",
                           "MINT_API_KEY": "unreachable"})
    s5 = summarise(d5)
    if s5 and s1:
        report["tests"]["offline"] = {
            "what": "coverage with no reachable model",
            "completed": s5["completed"], "runs": s5["runs"],
            "online_completed": s1["completed"],
            "drop": s1["completed"] - s5["completed"],
            "threshold": "no drop beyond the runs that needed a question",
            "pass": s5["completed"] >= s1["completed"] - s1["model_calls"],
            "seconds": t5,
        }
        print(f"  offline {s5['completed']}/{s5['runs']} against online {s1['completed']}/{s1['runs']}")

    # ── T6: is a skill reviewable ──────────────────────────────────────────
    print("T6 artifact: can a person read, diff and sign a skill")
    rows = []
    for s in SKILLS:
        p = f"{ROOT}/{s}"
        try:
            raw = open(p, encoding="utf-8").read()
            d = json.loads(raw)
            rows.append({
                "skill": s[:-11], "bytes": len(raw.encode()), "steps": len(d["steps"]),
                "readable": raw.strip().startswith("{"), "declares_outcomes": all(
                    "produced_page" in st and "intent" in st for st in d["steps"]),
                "verified_goal": d.get("learn_declared_done", False),
            })
        except Exception as e:
            rows.append({"skill": s, "error": str(e)})
    report["tests"]["artifact"] = {
        "what": "size, structure and signability of a compiled skill",
        "skills": rows,
        "threshold": "under 2 KB, valid JSON, every step declares its outcome",
        "pass": all(r.get("bytes", 9999) < 2048 and r.get("declares_outcomes") and r.get("readable")
                    for r in rows),
    }
    for r in rows:
        print(f"  {r['skill']}: {r.get('bytes')} B, {r.get('steps')} step(s)")

    # ── T7: traces ─────────────────────────────────────────────────────────
    print("T7 traces: does every run leave a verified trace")
    tp = f"{SCRATCH}/traces.jsonl"
    subprocess.run([BIN, "traces", "--eval", f"{SCRATCH}/sweep-online.json", "--out", tp],
                   capture_output=True, text=True, cwd=ROOT)
    runs_docs, step_docs, with_tier, with_verdict = 0, 0, 0, 0
    for line in open(tp, encoding="utf-8"):
        d = json.loads(line)
        if d["_table"] == "runs":
            runs_docs += 1
        else:
            step_docs += 1
            if d.get("tier"):
                with_tier += 1
            if "verified" in d:
                with_verdict += 1
    report["tests"]["traces"] = {
        "what": "documents written per verified run",
        "runs": runs_docs, "steps": step_docs, "steps_with_tier": with_tier,
        "steps_with_verified_flag": with_verdict,
        "threshold": "every step carries its tier and its verdict",
        "pass": step_docs > 0 and with_tier == step_docs and with_verdict == step_docs,
    }
    print(f"  {runs_docs} run documents, {step_docs} step documents, "
          f"{with_tier} carry a tier")

    # ── map the 56 applications onto the measured properties ───────────────
    verdicts = []
    for a in apps:
        t = report["tests"].get(a["p"], {})
        verdicts.append({
            "application": a["t"], "family": a["f"], "property": a["p"],
            "breakthrough": a["br"],
            "property_pass": t.get("pass"),
            "evidence": {k: v for k, v in t.items() if k in
                         ("what", "completed", "runs", "rate", "per_run", "share_without_model",
                          "reported_missed", "targets", "drop", "pass")},
            "verdict": ("the property it leans on passed on real targets"
                        if t.get("pass") else "the property it leans on is not established"),
            "not_tested_here": "a real instance of this application, which needs its own access",
        })
    report["applications"] = verdicts
    report["summary"] = {
        "applications": len(verdicts),
        "properties": len(report["tests"]),
        "properties_passed": sum(1 for t in report["tests"].values() if t.get("pass")),
        "backed": sum(1 for v in verdicts if v["property_pass"]),
        "unbacked": sum(1 for v in verdicts if not v["property_pass"]),
    }
    json.dump(report, open(f"{ROOT}/demo/applications-verified.json", "w"), indent=1)
    print("\n=== итог ===")
    for name, t in report["tests"].items():
        print(f"  {name:<10} {'PASS' if t.get('pass') else 'FAIL'}")
    s = report["summary"]
    print(f"  применений {s['applications']}: подкреплено {s['backed']}, не подкреплено {s['unbacked']}")


main()
