#!/usr/bin/env python3
"""mint showcase: takes frames and numbers from the runtime and serves them to the page.

Why polling and not a websocket: the picture changes only on agent steps, that is
about once a second. Polling every 300 ms delivers the same, but goes through any
tunnel without handshake trouble and does not break on an intermediary's buffering.

Three entry points:
  GET  /         — the page for the projector
  GET  /state    — current state: numbers and frames for both sides
  POST /push     — where the runtime sends a step
"""
import json
import os
import re
import subprocess
import sys
import threading
import time
from collections import defaultdict, deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PORT = 8840
DEMO_DIR = "/root/mint/demo"
LOCK = threading.Lock()

# ── the demonstrations ────────────────────────────────────────────────────────────────────────────
#
# Four of the eight pages are interactive: a visitor presses a button and the answer is computed right
# then by the real program, not read from a file prepared in advance. That is worth more than a
# screenshot, and it is also the only part of this server that runs anything on request.
#
# So the shape is deliberately narrow. The caller chooses an index into a table fixed in this file —
# never a path, never an argument, never a URL. Nothing is written, no browser is started, no model is
# called, and each case is a read of a file that already exists. A request that is not one of these
# cases is refused rather than guessed at.
MINT = "/root/mint/target/release/mint"
# The checkout itself: the fixture script lives there, and a relative skill path is relative to it.
REPO = "/root/mint"
# The verifier runs from the demo directory with relative paths, so that the output it prints names the
# same files the page says it ran on. Rewriting its output to look tidier would make the one thing the
# page claims — that this is what the program actually said — untrue.
ROOT = DEMO_DIR
API_POLICY = "gate-policy.json"
API_LOG = "gate-receipts.json"
API_ENDORSEMENT = "gate-endorsement.json"
API_OBSERVATIONS = "gate-observations.json"
API_TAMPER_DIR = "tamper"

# slug -> (what was done to this copy, the file to check, the rules to check it against)
API_TAMPER = {
    "edited": (f"{API_TAMPER_DIR}/tampered-edited.json", API_POLICY),
    "dropped": (f"{API_TAMPER_DIR}/tampered-dropped.json", API_POLICY),
    "swapped": (f"{API_TAMPER_DIR}/tampered-swapped.json", API_POLICY),
    "forged": (f"{API_TAMPER_DIR}/tampered-forged.json", API_POLICY),
    "policy": (f"{API_TAMPER_DIR}/tampered-policy.json", f"{API_TAMPER_DIR}/policy-swapped.json"),
}
API_ENDORSE = {
    "0": f"{API_TAMPER_DIR}/endorse-0.json",
    "1": f"{API_TAMPER_DIR}/endorse-1.json",
    "2": f"{API_TAMPER_DIR}/endorse-2.json",
    "3": f"{API_TAMPER_DIR}/endorse-3.json",
}

# One task a visitor can start, run by the real runtime in the real browser while they watch.
#
# Every one of these was measured to work before it was put here, and the targets are the ones the
# evaluation used: the skills were learned on one repository and these are the transfers. The last two
# point at our own gate lab, because watching a write get held is the part of this that is hardest to
# believe and easiest to show.
LIVE_TASKS = [
    {
        "n": 1,
        "skill": "gh-commit.skill.json",
        "start": "https://github.com/facebook/react",
        "goal": "open the latest commit of this repository",
        "what": "Open the latest commit — on a repository the skill has never seen",
    },
    {
        "n": 2,
        "skill": "gh-issues.skill.json",
        "start": "https://github.com/rust-lang/rust",
        "goal": "open the issue list of this repository",
        "what": "Open the issue list — a different repository, a different layout",
    },
    {
        "n": 3,
        "skill": "gh-prs.skill.json",
        "start": "https://github.com/facebook/react",
        "goal": "open the pull request list of this repository",
        "what": "Open the pull request list — the third transfer",
    },
    {
        "n": 4,
        "skill": "demo/gate-write.skill.json",
        "start": "http://127.0.0.1:8840/gate-lab",
        "goal": "submit the filing on the gate lab",
        "what": "Try to submit a form — and watch the gate hold it",
    },
    {
        "n": 5,
        "skill": "demo/gate-read.skill.json",
        "start": "http://127.0.0.1:8840/gate-lab",
        "goal": "open the record on the gate lab",
        "what": "Read the same page — which the gate lets through",
    },
]

# The same task, run by both kinds of agent. The ordinary half is a model deciding every step; the
# mint half is the compiled skill for the same task, on the same page, in the same browser.
COMPARE_TASKS = [
    {
        # Five steps of the plainest kind of work there is: two fields filled, a box ticked, a choice
        # made, and the form sent — none of which changes the address until the end, which is exactly
        # why a runtime that only counts navigation could never learn it.
        "n": 1,
        "skill": "form-fill.skill.json",
        "start": "https://www.selenium.dev/selenium/web/web-form.html",
        "goal": "fill in the form: the text field and the textarea, tick the Default checkbox, choose the Default radio, and submit it",
        "what": "Five steps of real work: two fields filled, a box ticked, a choice made, the form sent",
        # This task writes, so replaying it needs a document that permits those writes. Under the
        # shipped rules the same skill stops at the tick and says which action needs approving.
        "policy": "demo/learn-policy.json",
    },
    {
        # Learned on one repository, replayed on a different one, where every page has different data.
        "n": 2,
        "skill": "gh-pr-chain.skill.json",
        "start": "https://github.com/rust-lang/rust",
        "goal": "open the first pull request's commits, then the first commit in that list",
        "what": "Four steps through a real repository: pull requests, a pull request, its Commits tab, a commit",
    },
    {
        "n": 3,
        "skill": "gh-issues.skill.json",
        "start": "https://github.com/tokio-rs/tokio",
        "goal": "open the issue list of this repository",
        "what": "Two steps: open the issue list of a repository the skill has never seen",
    },
    {
        "n": 4,
        "skill": "gh-commit.skill.json",
        "start": "https://github.com/facebook/react",
        "goal": "open the latest commit of this repository",
        "what": "Two steps: open the latest commit of a third repository",
    },
    {
        "n": 5,
        "skill": "gh-prs.skill.json",
        "start": "https://github.com/rust-lang/rust",
        "goal": "open the pull request list of this repository",
        "what": "One step: open the pull request list of a fourth repository",
    },
]
COMPARE = {"running": False, "task": None, "started": 0.0, "took_ms": 0, "exit": None,
           "stdout": "", "error": ""}

# One at a time. The browser is a single shared thing, and two runs in it would produce a recording of
# neither. A visitor who arrives mid-run is told the machine is busy rather than shown a mess.
LIVE = {"running": False, "task": None, "started": 0.0, "took_ms": 0, "exit": None,
        "stdout": "", "error": ""}
LIVE_LOCK = threading.Lock()
LIVE_BY_IP = defaultdict(deque)
LIVE_IP_MAX = 6
LIVE_WINDOW_S = 600.0

# A page of buttons is not a load test. Sixty calls a minute from one address is far more than a
# person clicking can produce, and low enough that nothing here can be used to keep the machine busy.
API_BUDGET = deque()
API_BUDGET_MAX = 60
API_WINDOW_S = 60.0
API_BY_IP = defaultdict(deque)
API_IP_MAX = 20
API_SLOTS = threading.Semaphore(2)


def _live_allowed(ip):
    """A run costs seconds of a shared browser. Six in ten minutes per address is generous for a person
    and useless for anyone trying to use this as free compute."""
    now = time.time()
    with LOCK:
        mine = LIVE_BY_IP[ip]
        while mine and now - mine[0] > LIVE_WINDOW_S:
            mine.popleft()
        if len(mine) >= LIVE_IP_MAX:
            return False
        mine.append(now)
        return True


def _allowed(ip):
    """Both a global and a per-address budget. Cheap, and it fails closed."""
    now = time.time()
    with LOCK:
        while API_BUDGET and now - API_BUDGET[0] > API_WINDOW_S:
            API_BUDGET.popleft()
        if len(API_BUDGET) >= API_BUDGET_MAX:
            return False
        mine = API_BY_IP[ip]
        while mine and now - mine[0] > API_WINDOW_S:
            mine.popleft()
        if len(mine) >= API_IP_MAX:
            return False
        API_BUDGET.append(now)
        mine.append(now)
        return True


def live_reset(goal="", skill=""):
    """Clear only the live pane. The stage race keeps its own recording, and a visitor starting a run
    should not blank what the projector is showing."""
    with LOCK:
        STATE["goal"] = goal
        side = STATE["sides"]["live"]
        side.update(steps=[], model_calls=0, decision_us=0, total_ms=0, frame="", url="",
                    page_title="", verdict="", opinion="", finished=False,
                    receipts=None, held=0, asked=0, goal=goal, skill=skill)


def compare_start(n):
    """Run the same task through both kinds of agent, one after the other, and stream both sides."""
    with LIVE_LOCK:
        if LIVE["running"] or COMPARE["running"]:
            return None, "a run is already going — one at a time, try again in a moment"
        if n < 0 or n >= len(COMPARE_TASKS):
            return None, "no such task"
        task = COMPARE_TASKS[n]
        COMPARE.update(running=True, task=n, started=time.time(), exit=None, stdout="", error="")
    # Both panes are the two halves of this comparison, so both are cleared and both are filled.
    with LOCK:
        STATE["goal"] = task["goal"]
        STATE["started"] = time.time()
        for name in ("slow", "fast"):
            STATE["sides"][name].update(
                steps=[], model_calls=0, decision_us=0, total_ms=0, frame="", url="",
                page_title="", verdict="", opinion="", finished=False, receipts=None)

    def worker():
        out_dir = f"{DEMO_DIR}/compare"
        os.makedirs(out_dir, exist_ok=True)
        log_path = f"{out_dir}/last.json"
        cmd = [MINT, "demo", "--skill", task["skill"], "--start", task["start"],
               "--push", "http://127.0.0.1:8840", "--receipts", log_path]
        # A task that writes is run under the document it declares, so the run is permitted and the
        # receipts name the rules it was permitted by. Tasks that only read use the shipped ones.
        if task.get("policy"):
            cmd += ["--policy", task["policy"]]
        # A task of fifty steps needs a ceiling that allows fifty, or the ordinary half is stopped by
        # the harness rather than by the task.
        if task.get("max_steps"):
            cmd += ["--max-steps", str(task["max_steps"])]
        code, out, err = None, "", ""
        try:
            done = subprocess.run(cmd, cwd="/root/mint", capture_output=True, text=True, timeout=300)
            code, out, err = done.returncode, done.stdout or "", done.stderr or ""
        except subprocess.TimeoutExpired:
            code, err = -1, "the comparison did not finish in five minutes"
        except OSError as exc:
            code, err = -1, "could not start the runtime: %s" % exc
        with LIVE_LOCK:
            COMPARE.update(running=False, exit=code, stdout=out.strip()[-1200:],
                           error=err.strip()[-600:] if err else out.strip()[-600:],
                           took_ms=int((time.time() - COMPARE["started"]) * 1000))
        with LOCK:
            for name in ("slow", "fast"):
                side = STATE["sides"][name]
                if not side.get("finished"):
                    side["finished"] = True
                    if not side.get("verdict"):
                        side["verdict"] = (err.strip()[-200:]
                                           or "the run stopped without reporting a result")

    threading.Thread(target=worker, daemon=True).start()
    return {"ok": True, "task": task["n"], "what": task["what"], "goal": task["goal"]}, None


def compare_result():
    """Both halves, side by side, with what each of them leaves behind."""
    with LOCK:
        slow = dict(STATE["sides"]["slow"])
        fast = dict(STATE["sides"]["fast"])
    with LIVE_LOCK:
        running, took, err = COMPARE["running"], COMPARE["took_ms"], COMPARE["error"]
    idx = COMPARE["task"]
    shown = COMPARE_TASKS[idx]["n"] if isinstance(idx, int) and 0 <= idx < len(COMPARE_TASKS) else None
    log = f"{DEMO_DIR}/compare/last.json"
    return {
        "ok": True,
        "task": shown,
        "running": running,
        "took_ms": took,
        "error": err,
        "goal": slow.get("goal") or fast.get("goal") or "",
        "slow": {"steps": slow.get("steps", []), "model_calls": slow.get("model_calls", 0),
                 "total_ms": slow.get("total_ms", 0), "verdict": slow.get("verdict", ""),
                 "finished": slow.get("finished", False), "url": slow.get("url", "")},
        "fast": {"steps": fast.get("steps", []), "model_calls": fast.get("model_calls", 0),
                 "total_ms": fast.get("total_ms", 0), "verdict": fast.get("verdict", ""),
                 "finished": fast.get("finished", False), "url": fast.get("url", ""),
                 "receipts": fast.get("receipts")},
        "log_url": "/compare-run.json",
        "log_exists": os.path.exists(log),
    }, None


def compare_verify():
    """Check the compiled half's log with the verifier a stranger would use."""
    if not os.path.exists(f"{DEMO_DIR}/compare/last.json"):
        return None, "that comparison did not write a log"
    code, text = run_levos(["receipts", "--in", "compare/last.json",
                            "--policy", "compare/last.json.policy.json"])
    if code != 0 and not text:
        return None, "the verifier produced nothing"
    return {"ok": True, "output": text,
            "command": "mint receipts --in compare/last.json --policy compare/last.json.policy.json"}, None


def live_start(n):
    """Start one task. Fixed table, one at a time, and the process is the real runtime."""
    with LIVE_LOCK:
        if LIVE["running"]:
            return None, "a run is already going — one at a time, try again in a moment"
        if n < 0 or n >= len(LIVE_TASKS):
            return None, "no such task"
        task = LIVE_TASKS[n]
        LIVE.update(running=True, task=n, started=time.time(), exit=None, stdout="", error="")
    live_reset(task["goal"], task["skill"])

    def worker():
        out_dir = f"{DEMO_DIR}/live"
        os.makedirs(out_dir, exist_ok=True)
        log_path = f"{out_dir}/last.json"
        cmd = [MINT, "live", "--skill", task["skill"], "--start", task["start"],
               "--push", "http://127.0.0.1:8840", "--receipts", log_path]
        code, out, err = None, "", ""
        try:
            done = subprocess.run(cmd, cwd="/root/mint", capture_output=True, text=True,
                                  timeout=240)
            code, out, err = done.returncode, done.stdout or "", done.stderr or ""
        except subprocess.TimeoutExpired:
            code, err = -1, "the run did not finish in four minutes"
        except OSError as exc:
            code, err = -1, "could not start the runtime: %s" % exc
        with LIVE_LOCK:
            LIVE.update(running=False, exit=code, stdout=out.strip()[-1200:],
                        error=err.strip()[-600:], took_ms=int((time.time() - LIVE["started"]) * 1000))
        # The pane must not sit on "running" if the process died before its last push.
        with LOCK:
            if not STATE["sides"]["live"].get("finished"):
                STATE["sides"]["live"]["finished"] = True
                if not STATE["sides"]["live"].get("verdict"):
                    STATE["sides"]["live"]["verdict"] = (
                        err.strip()[-200:] or "the run stopped without reporting a result")

    threading.Thread(target=worker, daemon=True).start()
    # Echo the number the page showed, not the list index it was translated from.
    return {"ok": True, "task": task["n"], "what": task["what"], "goal": task["goal"]}, None


def live_result():
    """What the last run produced: the verdict the runtime reached and its signed receipts."""
    with LOCK:
        side = dict(STATE["sides"]["live"])
    with LIVE_LOCK:
        # The number the page showed, so a caller can tell whether this summary describes the run it
        # started or one somebody else started a moment earlier.
        idx = LIVE["task"]
        shown = LIVE_TASKS[idx]["n"] if isinstance(idx, int) and 0 <= idx < len(LIVE_TASKS) else None
        if shown is None:
            state_note = ("no run has been started since this server last restarted; what you see"
                          " above is the last one, kept so the pane is never blank")
        else:
            state_note = ""
        state = {"running": LIVE["running"], "task": shown, "note": state_note, "took_ms": LIVE["took_ms"],
                 "exit": LIVE["exit"], "error": LIVE["error"], "tail": LIVE["stdout"]}
    log = f"{DEMO_DIR}/live/last.json"
    state.update({
        "ok": True,
        "goal": side.get("goal", ""),
        "verdict": side.get("verdict", ""),
        "steps_done": len(side.get("steps", [])),
        "asked": side.get("asked", 0),
        "held": side.get("held", 0),
        "receipts": side.get("receipts"),
        "log_url": "/live-run.json",
        "log_exists": os.path.exists(log),
    })
    return state, None


def api_live_verify():
    """Check the log the run just wrote, with the same verifier a stranger would use."""
    log = f"{DEMO_DIR}/live/last.json"
    if not os.path.exists(log):
        return None, "that run did not write a log"
    args = ["receipts", "--in", "live/last.json", "--policy", "live/last.json.policy.json"]
    code, text = run_mint(args)
    if code != 0 and not text:
        return None, "the verifier produced nothing"
    return {"ok": True, "output": text,
            "command": "mint receipts --in live/last.json --policy live/last.json.policy.json"}, None


def api_witness_check(case):
    """Run the real verifier over the published objects, and over the two ways of lying about them.

    Fixed cases chosen by index: nothing is built from what the caller sent, and nothing here writes.
    Case 0 is the honest set of files. Case 1 is the same log with its last two receipts removed —
    every link inside that shorter chain still verifies, which is exactly why a checkpoint exists.
    Case 2 is the runtime trying to countersign its own head, and the refusal shown is the file the
    runtime itself produced when it was run, not a sentence written for the page.
    """
    if not _allowed_check_case(case):
        return {"ok": False, "error": "no such case"}, "refused"
    if case == "2":
        try:
            text = open(f"{DEMO_DIR}/witness-cases/self-witness.txt").read().strip()
        except FileNotFoundError:
            return {"ok": False, "error": "that case has not been generated"}, None
        return {
            "ok": True,
            "case": "the runtime countersigning its own head",
            "held": False,
            "output": text,
        }, None
    args = [
        "verify-witness",
        "--receipts", f"{DEMO_DIR}/gate-receipts.json" if case == "" else f"{DEMO_DIR}/witness-cases/truncated.json",
        "--checkpoint", f"{DEMO_DIR}/checkpoint.json",
        "--witness", f"{DEMO_DIR}/witness.json",
    ]
    done = subprocess.run(
        [f"{REPO}/target/release/mint"] + args, cwd=REPO, capture_output=True, text=True, timeout=30
    )
    out = (done.stdout + done.stderr).strip()
    what = (
        "the log, its checkpoint, and the countersignature over it"
        if case == ""
        else "the same log with its last two receipts removed"
    )
    return {"ok": True, "case": what, "held": done.returncode == 0, "output": out}, None


def _allowed_check_case(case):
    """Three cases exist. Anything else is refused rather than guessed at."""
    return case in ("", "1", "2")


def run_mint(args, stdin_text=None, timeout=25):
    """Run the real binary with fixed arguments and hand back exactly what it printed."""
    if not API_SLOTS.acquire(timeout=10):
        return -1, "the machine is busy running another demonstration; try again in a moment"
    try:
        done = subprocess.run(
            [MINT] + args, cwd=ROOT, input=stdin_text,
            capture_output=True, text=True, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return -1, "the verifier did not finish within %d seconds" % timeout
    except OSError as exc:
        return -1, "could not run the verifier: %s" % exc
    finally:
        API_SLOTS.release()
    text = done.stdout or ""
    if done.stderr and done.stderr.strip():
        text += "\n" + done.stderr
    return done.returncode, text.strip()


def api_verdict(query):
    """Apply the published rules to one recorded observation, with the approval flipped if asked."""
    obs = json.load(open(API_OBSERVATIONS, encoding="utf-8"))["observations"]
    try:
        n = int(query.get("obs", ["0"])[0])
    except ValueError:
        return None, "obs must be a number"
    if n < 0 or n >= len(obs):
        return None, "no such observation"
    approved = query.get("approval", ["0"])[0] == "1"
    obs[n] = dict(obs[n], approval=approved)

    plain = ["rules", "--policy", API_POLICY, "--observations", "-"]
    code, text = run_mint(plain, stdin_text=json.dumps({"observations": obs}))
    if code != 0:
        return None, text
    asked = json.loads(run_mint(plain + ["--json"], stdin_text=json.dumps({"observations": obs}))[1])
    said = asked["results"][n]
    return {
        "ok": True,
        "class": said["class"],
        "verdict": said["verdict"],
        "reason": said["reason"],
        "policy_hash": asked["policy_hash"],
        "spec": asked["spec"],
        "output": text,
        "command": ("mint rules --policy policy.json --observations observations.json"
                    "   # element %d, approval presented: %s" % (n, "yes" if approved else "no")),
    }, None


def api_tamper(slug):
    if slug not in API_TAMPER:
        return None, "no such case"
    log, rules = API_TAMPER[slug]
    args = ["receipts", "--in", log, "--policy", rules]
    code, text = run_mint(args)
    if code != 0 and not text:
        return None, "the verifier produced nothing"
    return {
        "ok": True,
        "output": text,
        "command": "mint receipts --in %s --policy %s" % (
            os.path.basename(log), os.path.basename(rules)),
    }, None


def api_endorse(case):
    if case not in API_ENDORSE:
        return None, "no such case"
    args = ["receipts", "--in", API_LOG, "--policy", API_POLICY,
            "--endorsement", API_ENDORSE[case]]
    code, text = run_mint(args)
    if code != 0 and not text:
        return None, "the verifier produced nothing"
    return {
        "ok": True,
        "output": text,
        "command": "mint receipts --in receipts.json --policy policy.json --endorsement %s" % (
            os.path.basename(API_ENDORSE[case])),
    }, None


def api_verify():
    args = ["receipts", "--in", API_LOG, "--policy", API_POLICY,
            "--endorsement", API_ENDORSEMENT, "--tamper", API_TAMPER_DIR]
    code, text = run_mint(args, timeout=60)
    if code != 0 and not text:
        return None, "the verifier produced nothing"
    return {
        "ok": True,
        "output": text,
        "command": "mint receipts --in receipts.json --policy policy.json "
                   "--endorsement endorsement.json --tamper .",
    }, None

# The demonstration pages, by name. A request for anything not in this list is a 404: building a path
# out of what a caller sent is how a read-only server ends up reading something it was never asked to.
DEMO_PAGES = {
    "": "index.html",
    "gate": "gate.html",
    "refuse": "refuse.html",
    "tamper": "tamper.html",
    "endorse": "endorse.html",
    "receipt": "receipt.html",
    "learn": "learn.html",
    "tasks": "tasks.html",
    "state": "state.html",
    "live": "live.html",
    "compare": "compare.html",
    "directory": "directory.html",
    "examples": "examples.html",
    "witness": "witness.html",
}

# What may be served as a file, and nothing else. The demo page shows real screenshots and a real
# recording, so the showcase has to hand them out — but only ever from this directory, and only
# ever these kinds.
MEDIA_TYPES = {
    "jpg": "image/jpeg", "jpeg": "image/jpeg", "png": "image/png",
    "gif": "image/gif", "mp4": "video/mp4", "webm": "video/webm",
}

STATE = {
    "goal": "",
    "started": time.time(),
    "sides": {
        "slow": {
            "title": "Ordinary agent",
            "subtitle": "the model decides every step",
            "steps": [],
            "model_calls": 0,
            "decision_us": 0,
            "total_ms": 0,
            "frame": "",
            "url": "",
            "page_title": "",
            "verdict": "",
            "opinion": "",
            "finished": False,
        },
        # The directory surface: the same two halves as the comparison, on a filesystem instead of a
        # page. Fifty invoices into twelve month folders is the whole task — tedious for a person, and
        # fifty decisions for an agent that asks a model about every one.
        "dir_model": {
            "title": "Ordinary agent",
            "subtitle": "a model decides every command",
            "steps": [],
            "model_calls": 0,
            "decision_us": 0,
            "total_ms": 0,
            "frame": "",
            "url": "",
            "page_title": "",
            "verdict": "",
            "opinion": "",
            "finished": False,
        },
        "dir_skill": {
            "title": "mint",
            "subtitle": "the same task, compiled",
            "steps": [],
            "model_calls": 0,
            "decision_us": 0,
            "total_ms": 0,
            "frame": "",
            "url": "",
            "page_title": "",
            "verdict": "",
            "opinion": "",
            "finished": False,
        },
        "live": {
            "title": "A run you asked for",
            "subtitle": "the real runtime, in the real browser",
            "steps": [],
            "model_calls": 0,
            "decision_us": 0,
            "total_ms": 0,
            "frame": "",
            "url": "",
            "page_title": "",
            "verdict": "",
            "opinion": "",
            "finished": False,
            "receipts": None,
            "held": 0,
            "asked": 0,
            "goal": "",
            "skill": "",
        },
        "fast": {
            "title": "mint",
            "subtitle": "running the remembered skill",
            "steps": [],
            "model_calls": 0,
            "decision_us": 0,
            "total_ms": 0,
            "frame": "",
            "url": "",
            "page_title": "",
            "verdict": "",
            "opinion": "",
            "finished": False,
        },
    },
}


def reset(goal: str = "") -> None:
    with LOCK:
        STATE["goal"] = goal
        STATE["started"] = time.time()
        for side in STATE["sides"].values():
            side.update(
                steps=[], model_calls=0, decision_us=0, total_ms=0,
                frame="", url="", page_title="", verdict="", opinion="", finished=False,
            )


def push(payload: dict) -> None:
    side = payload.get("side")
    if side not in STATE["sides"]:
        return
    with LOCK:
        s = STATE["sides"][side]
        if payload.get("goal"):
            STATE["goal"] = payload["goal"]
        if payload.get("frame"):
            s["frame"] = payload["frame"]
        if payload.get("url"):
            s["url"] = payload["url"]
        if payload.get("page_title"):
            s["page_title"] = payload["page_title"]
        if payload.get("verdict"):
            s["verdict"] = payload["verdict"]
        if payload.get("opinion"):
            s["opinion"] = payload["opinion"]
        if payload.get("finished"):
            s["finished"] = True
        if payload.get("total_ms"):
            s["total_ms"] = payload["total_ms"]
        # "reached" and "listing" belong here too: without them a side says "did not reach the
        # goal" while the verdict printed beside it says the work was done correctly, which reads
        # as a contradiction on the page and is worse than a wrong number.
        for key in ("receipts", "held", "asked", "goal", "skill", "reached", "listing"):
            if payload.get(key) is not None:
                s[key] = payload[key]
        if payload.get("step"):
            step = payload["step"]
            s["steps"].append(step)
            if step.get("tier") == "model":
                s["model_calls"] += 1
            s["decision_us"] += int(step.get("decision_us", 0))
    _persist(force=bool(payload.get("finished")))



# ─────────────────────────────────────────────────────────────────────────────────────────────────
# The directory surface
#
# The task is a pile of files and a rule that says where each one goes. It runs on a machine's
# filesystem rather than in a browser, which is the point: nothing in the runtime's middle knows what
# a browser is, and this is the surface that shows it. Both halves start from a fresh copy of the same
# directory, because a task that moves files cannot be run twice in the same place.
DIR_TASKS = [
    {
        "n": 1,
        "skill": "invoices.skill.json",
        "goal": "Move every invoice into the folder named after the month in its name: an invoice "
                "named invoice-2026-03-....pdf belongs in the folder 2026-03, and every invoice must "
                "end up in its month folder. The number of files in the folders may differ.",
        "what": "Fifty invoices sorted into twelve month folders by the rule in their names",
        "policy": "demo/learn-policy.json",
        "max_steps": 70,
    },
]

DIR = {"running": False, "task": None, "took_ms": 0, "error": ""}
DIR_LOCK = threading.Lock()
DIR_TEMPLATE = "/tmp/invoices-template"


def dir_start(n):
    """Sort the pile twice: once with a model deciding every command, once with the compiled skill."""
    with DIR_LOCK:
        if DIR["running"] or COMPARE["running"] or LIVE["running"]:
            return None, "a run is already going — one at a time, try again in a moment"
        if n < 0 or n >= len(DIR_TASKS):
            return None, "no such task"
        task = DIR_TASKS[n]
        DIR.update(running=True, task=n, took_ms=0, error="")
    with LOCK:
        STATE["goal"] = task["goal"]
        for name in ("dir_model", "dir_skill"):
            STATE["sides"][name].update(
                steps=[], model_calls=0, decision_us=0, total_ms=0, frame="", url="",
                page_title="", verdict="", opinion="", finished=False, receipts=None)

    def worker():
        out_dir = f"{DEMO_DIR}/dir"
        os.makedirs(out_dir, exist_ok=True)
        log_path = f"{out_dir}/last.json"
        started = time.time()
        code, out, err = None, "", ""
        try:
            # The fixture is built here rather than committed: fifty files belong in a script, not in a
            # repository, and a task whose starting state can be rebuilt is a task anybody can check.
            made = subprocess.run([sys.executable, f"{REPO}/tools/make_invoice_dir.py", DIR_TEMPLATE],
                                  cwd=REPO, capture_output=True, text=True, timeout=60)
            if made.returncode != 0:
                raise RuntimeError(made.stderr.strip() or "the fixture could not be built")
            cmd = [MINT, "dir-demo", "--skill", task["skill"], "--dir", DIR_TEMPLATE,
                   "--push", "http://127.0.0.1:8840", "--receipts", log_path,
                   "--max-steps", str(task.get("max_steps", 70))]
            if task.get("policy"):
                cmd += ["--policy", task["policy"]]
            done = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True, timeout=300)
            code, out, err = done.returncode, done.stdout or "", done.stderr or ""
        except subprocess.TimeoutExpired:
            code, err = -1, "the run did not finish in five minutes"
        except Exception as exc:  # a demonstration that dies silently looks like a demonstration
            code, err = -1, f"the run failed on this machine: {exc}"
        with DIR_LOCK:
            DIR.update(running=False, took_ms=int((time.time() - started) * 1000),
                       error="" if code == 0 else (err or f"exit {code}"))
        with LOCK:
            for name in ("dir_model", "dir_skill"):
                if not STATE["sides"][name].get("finished"):
                    STATE["sides"][name]["finished"] = True
            if not STATE["sides"]["dir_model"].get("verdict"):
                STATE["sides"]["dir_model"]["verdict"] = (err or "the run did not finish")[:300]
            if not STATE["sides"]["dir_skill"].get("verdict"):
                STATE["sides"]["dir_skill"]["verdict"] = (err or "the run did not finish")[:300]
        _persist(force=True)

    threading.Thread(target=worker, daemon=True).start()
    return {"ok": True, "task": task["n"], "what": task["what"], "goal": task["goal"]}, None


def dir_result():
    """Both halves of the directory run, and what each of them leaves behind."""
    with LOCK:
        model = dict(STATE["sides"]["dir_model"])
        skill = dict(STATE["sides"]["dir_skill"])
    with DIR_LOCK:
        running, took, err = DIR["running"], DIR["took_ms"], DIR["error"]
    idx = DIR["task"]
    shown = DIR_TASKS[idx]["n"] if isinstance(idx, int) and 0 <= idx < len(DIR_TASKS) else None
    log = f"{DEMO_DIR}/dir/last.json"
    return {
        "ok": True,
        "task": shown,
        "running": running,
        "took_ms": took,
        "error": err,
        "goal": model.get("goal") or skill.get("goal") or "",
        "slow": {"steps": model.get("steps", []), "total_ms": model.get("total_ms", 0),
                 "verdict": model.get("verdict", ""), "reached": model.get("reached", False),
                 "listing": model.get("listing", "")},
        "fast": {"steps": skill.get("steps", []), "model_calls": skill.get("model_calls", 0),
                 "total_ms": skill.get("total_ms", 0), "verdict": skill.get("verdict", ""),
                 "reached": skill.get("reached", False), "listing": skill.get("listing", ""),
                 "receipts": skill.get("receipts") or _receipts_from(log)},
    }, None



def _receipts_from(path):
    """The signed log a run left behind, if it left one. Missing is not an error: a run that was
    stopped by the gate writes nothing, and that is a fact worth showing rather than a failure."""
    try:
        with open(path, "rb") as fh:
            doc = json.load(fh)
    except Exception:
        return None
    if not isinstance(doc, dict):
        return None
    # No claim about verification here: this is a file being read, not a chain being checked. The
    # runtime verifies the chain in its own output, and the page shows what the run reported.
    return {"count": len(doc.get("receipts") or []), "verified": None,
            "path": "/dir/last.json"}

LAST_RUN_PATH = "/root/mint/demo/last-run.json"
_persisted_at = [0.0]


def _persist(force: bool = False) -> None:
    """Keep the last run on disk.

    The live view is embedded in the pitch page, and a live view that is blank until somebody
    happens to start a run is a worse demo than no live view at all. Keeping the last real run
    means the embedded view always shows the most recent race rather than an empty frame.
    """
    now = time.time()
    if not force and now - _persisted_at[0] < 3:
        return
    _persisted_at[0] = now
    try:
        with LOCK:
            snapshot = json.dumps(STATE)
        tmp = LAST_RUN_PATH + ".tmp"
        with open(tmp, "w") as fh:
            fh.write(snapshot)
        os.replace(tmp, LAST_RUN_PATH)
    except Exception:
        pass


def _restore() -> None:
    try:
        data = json.loads(open(LAST_RUN_PATH).read())
    except Exception:
        return
    with LOCK:
        # The saved run predates whatever panes have been added since. Merging rather than replacing
        # restores the race from disk *and* leaves a pane added later in place, instead of dropping it
        # until the next restart — which is how a live view ends up missing from a working server.
        fresh = dict(STATE["sides"])
        STATE.update(data)
        STATE["sides"] = {**fresh, **STATE.get("sides", {})}
    fast = len(STATE["sides"].get("fast", {}).get("steps", []))
    slow = len(STATE["sides"].get("slow", {}).get("steps", []))
    print(f"restored the last run: {fast} fast steps, {slow} slow steps", flush=True)


# The front door: one page that says who this is for, what it does and how it works, in that
# order. The long version is still served, but it is no longer the thing a visitor lands on — a
# wall of fourteen sections is not an explanation, it is a filing cabinet.
GUIDE_PATH = "/root/mint/demo/guide.html"
SITE_PATH = "/root/mint/demo/site.html"
MEASURE_PATH = "/root/mint/demo/measurements.json"
EVAL_PATH = "/root/mint/demo/evaluation.json"
VERIFIED_PATH = "/root/mint/demo/applications-verified.json"

PAGE = r"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>mint — the race</title>
<style>
  :root { --bg:#0b0d12; --panel:#12151d; --line:#232838; --ink:#e8ecf5; --dim:#8b93a7;
          --slow:#ff6b6b; --fast:#4ade80; --accent:#7c5ce7; }
  * { box-sizing:border-box; }
  body { margin:0; background:var(--bg); color:var(--ink);
         font:16px/1.45 ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }
  header { padding:18px 26px 12px; border-bottom:1px solid var(--line); }
  h1 { margin:0 0 4px; font-size:22px; letter-spacing:-.02em; }
  .goal { color:var(--dim); font-size:15px; }
  .goal b { color:var(--ink); font-weight:600; }
  .row { display:grid; grid-template-columns:1fr 1fr; gap:16px; padding:16px 26px 26px; }
  .pane { background:var(--panel); border:1px solid var(--line); border-radius:14px; overflow:hidden;
          display:flex; flex-direction:column; }
  .pane.slow { border-color:#3a2229; } .pane.fast { border-color:#1f3a2b; }
  .head { padding:12px 16px; display:flex; justify-content:space-between; align-items:baseline;
          border-bottom:1px solid var(--line); }
  .name { font-weight:650; } .slow .name { color:var(--slow); } .fast .name { color:var(--fast); }
  .sub { color:var(--dim); font-size:13px; }
  .shot { background:#05070b; aspect-ratio:16/10; display:flex; align-items:center; justify-content:center; }
  .shot img { width:100%; height:100%; object-fit:cover; object-position:top center; display:block; }
  .shot .empty { color:#3b4256; font-size:14px; }
  .stats { display:grid; grid-template-columns:repeat(3,1fr); border-top:1px solid var(--line); }
  .stat { padding:12px 14px; border-right:1px solid var(--line); }
  .stat:last-child { border-right:0; }
  .k { color:var(--dim); font-size:12px; text-transform:uppercase; letter-spacing:.06em; }
  .v { font-size:26px; font-weight:700; font-variant-numeric:tabular-nums; margin-top:3px; }
  .v small { font-size:13px; color:var(--dim); font-weight:500; margin-left:4px; }
  .fast .v { color:var(--fast); } .slow .v { color:var(--slow); }
  .steps { padding:10px 14px; font:13px/1.6 ui-monospace,SFMono-Regular,Menlo,monospace;
           color:var(--dim); min-height:74px; max-height:150px; overflow:auto; }
  .steps div span { color:var(--ink); }
  .steps .t { color:var(--accent); }
  .steps .n { color:#f0b95c; }
  footer { padding:0 26px 26px; color:var(--dim); font-size:14px; }
  .verdict { margin-top:12px; padding:12px 16px; border-radius:10px; border:1px solid var(--line);
             background:var(--panel); }
  .verdict b { color:var(--ink); }
</style></head>
<body>
<header>
  <h1 id="headline">Two panes. Watch the decision counters.</h1>
  <div class="goal">Goal: <b id="goal">…</b></div>
</header>
<div class="row" id="row"></div>
<footer>
  <div class="verdict" id="verdict">Waiting for the run…</div>
</footer>
<script>
function ms(us){ if(!us) return "0"; const v=us/1000; return v<1 ? v.toFixed(2) : v.toFixed(0); }
function render(s){
  const row=document.getElementById('row');
  row.innerHTML='';
  for (const key of ['slow','fast']) {
    const d=s.sides[key];
    const pane=document.createElement('div');
    pane.className='pane '+key;
    const steps=d.steps.slice(-6).map(st=>
      `<div>${st.n}. <span>${esc(st.element||'')}</span> <span class="t">${st.tier==='model'?'model '+ms(st.decision_us)+' ms':'memory '+ms(st.decision_us)+' ms'}</span> ${st.note?`<span class="n">${esc(st.note)}</span>`:''}</div>`
    ).join('');
    pane.innerHTML=`
      <div class="head"><div><div class="name">${esc(d.title)}</div><div class="sub">${esc(d.subtitle)}</div></div>
        <div class="sub">${d.finished?'finished':'working…'}</div></div>
      <div class="shot">${d.frame?`<img src="${d.frame}" alt="">`:'<div class="empty">watching the browser…</div>'}</div>
      <div class="stats">
        <div class="stat"><div class="k">decisions, total</div><div class="v">${ms(d.decision_us)}<small>ms</small></div></div>
        <div class="stat"><div class="k">model calls</div><div class="v">${d.model_calls}</div></div>
        <div class="stat"><div class="k">whole task</div><div class="v">${d.total_ms?(d.total_ms/1000).toFixed(2):'—'}<small>s</small></div></div>
      </div>
      <div class="steps">${steps||'<div>no steps yet</div>'}</div>`;
    row.appendChild(pane);
  }
  document.getElementById('goal').textContent=s.goal||'…';
  const m=(s.goal||'').match(/^(\d+)\s/);
  if(m) document.getElementById('headline').textContent=
    m[1]+' tasks in a row. Two panes. Watch the decision counters.';
  const slow=s.sides.slow, fast=s.sides.fast;
  if (slow.steps.length && fast.steps.length && slow.decision_us && fast.decision_us) {
    // One unit on both sides, or the comparison becomes meaningless.
    const perSlow=slow.decision_us/slow.steps.length, perFast=fast.decision_us/fast.steps.length;
    const fmtUs=(u)=> u>=1000 ? Math.round(u).toLocaleString('en-US') : (u>=10 ? u.toFixed(0) : u.toFixed(3));
    const ratio=perSlow/perFast;
    document.getElementById('verdict').innerHTML=
      `<b>Result.</b> Decisions made: ${slow.steps.length} by the ordinary agent, ${fast.steps.length} by mint `
      +`(and that is for the same work). `
      +`Per decision: <b>${fmtUs(perSlow)} µs</b> against <b>${fmtUs(perFast)} µs</b> — <b>${Math.round(ratio).toLocaleString('en-US')}x</b> faster. `
      +`Model calls: <b>${slow.model_calls}</b> against <b>${fast.model_calls}</b>. `
      +(fast.verdict?`<br><br><b>Verdict (not an opinion):</b> ${esc(fast.verdict)}.`:'')
      +(fast.opinion?`<br><span style="color:#8b93a7">The model's opinion, kept separate as an opinion: ${esc(fast.opinion)}.</span>`:'');
  }
}
function esc(t){ return String(t).replace(/[<>&"]/g,c=>({'<':'&lt;','>':'&gt;','&':'&amp;','"':'&quot;'}[c])); }
async function tick(){ try { const r=await fetch('state',{cache:'no-store'}); render(await r.json()); } catch(e){} }
setInterval(tick,300); tick();
</script></body></html>
"""



# ---------------------------------------------------------------------------------------------
# A long task of the plainest kind: fifty fields and a send button.
#
# It lives here rather than on somebody else's site so that a demonstration is the same task on every
# day, and it is a real form: the values are received and counted, so "the form was sent" is not a
# claim about a screenshot. Fifty is not a big number for a person and it is fifty decisions for an
# agent that decides every step with a model — which is the point of putting it on this page.
FIELD_COUNT = 50


def fieldwork_page():
    fields = "\n".join(
        '    <label for="f%02d">Field %02d</label>\n'
        '    <input id="f%02d" name="f%02d" type="text" autocomplete="off">' % (i, i, i, i)
        for i in range(1, FIELD_COUNT + 1)
    )
    return """<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fieldwork — a long form</title>
<style>
  body { font: 15px/1.5 system-ui, -apple-system, Segoe UI, sans-serif; margin: 0; padding: 24px 18px 60px;
         background: #f7f7fa; color: #16161d; }
  h1 { font-size: 20px; margin: 0 0 6px; }
  p.sub { color: #5c5c6b; margin: 0 0 18px; max-width: 62ch; }
  form { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 10px 16px;
         max-width: 1100px; }
  label { display: block; font-size: 12px; color: #5c5c6b; margin-bottom: 3px; }
  input { width: 100%%; box-sizing: border-box; padding: 7px 9px; border: 1px solid #cfcfda;
          border-radius: 7px; font: inherit; background: #fff; }
  button { grid-column: 1 / -1; margin-top: 8px; padding: 11px 18px; border: 0; border-radius: 8px;
           background: #16161d; color: #fff; font: inherit; font-weight: 600; cursor: pointer; }
  .note { max-width: 62ch; color: #5c5c6b; font-size: 13px; margin-top: 22px; }
</style></head><body>
<h1>Fieldwork</h1>
<p class="sub">Fifty fields, one send button. The rule for this task: each field is filled with the
text of its own label — Field 01 with "Field 01", and so on — and then the form is sent.</p>
<form method="post" action="/fieldwork/submit">
%s
  <button type="submit">Send</button>
</form>
<p class="note">Nothing here is stored. The values are counted and echoed back so that a run can be
checked against what it claims to have done.</p>
</body></html>""" % fields


def fieldwork_received(body):
    """Count what arrived and echo it. A form that says 'thanks' tells a run nothing; this says how
    many values arrived and what they were, which is what a goal check can be held to."""
    from urllib.parse import parse_qs
    try:
        values = parse_qs(body.decode("utf-8", "replace"), keep_blank_values=True)
    except Exception:
        values = {}
    filled = {k: v[0] for k, v in values.items() if k.startswith("f") and v and v[0].strip()}
    ordered = sorted(filled.items())
    right = sum(1 for k, v in ordered if v.strip() == "Field " + k[1:])
    rows = "\n".join("    <li>%s = %s</li>" % (k, v) for k, v in ordered)
    return """<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Fieldwork — received</title>
<style>body { font: 15px/1.5 system-ui, sans-serif; margin: 0; padding: 24px 18px 60px; background: #f7f7fa; }
h1 { font-size: 20px; } ul { columns: 3; max-width: 1000px; font-size: 13px; color: #3b3b48; }</style>
</head><body>
<h1>Received %d values, %d of them matching their own label.</h1>
<p>Out of %d fields.</p>
<ul>
%s
</ul>
</body></html>""" % (len(ordered), right, FIELD_COUNT, rows)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # quiet logs: a poll lands here every 300 ms
        pass

    def _send(self, code, body: bytes, ctype: str):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body)

    def _send_file(self, path, ctype):
        """A file, with range support so a recording can be seeked and not only played through."""
        try:
            data = open(path, "rb").read()
        except (FileNotFoundError, IsADirectoryError):
            self._send(404, b"no such file", "text/plain")
            return
        m = re.match(r"bytes=(\d*)-(\d*)", self.headers.get("Range") or "")
        if m and (m.group(1) or m.group(2)):
            total = len(data)
            if m.group(1):
                start, end = int(m.group(1)), (int(m.group(2)) if m.group(2) else total - 1)
            else:
                start, end = max(0, total - int(m.group(2))), total - 1
            end = min(end, total - 1)
            if start > end or start >= total:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{total}")
                self.end_headers()
                return
            chunk = data[start:end + 1]
            self.send_response(206)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(len(chunk)))
            self.send_header("Cache-Control", "public, max-age=3600")
            self.end_headers()
            self.wfile.write(chunk)
            return
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "public, max-age=3600")
        self.end_headers()
        self.wfile.write(data)

    def _media(self, rel):
        """Serve one demo asset. Whitelisted by kind, resolved inside one directory, no climbing."""
        rel = rel.split("?", 1)[0].split("#", 1)[0]
        if not re.fullmatch(r"[A-Za-z0-9._/-]{1,180}", rel) or ".." in rel or rel.startswith("/"):
            self._send(400, b"bad name", "text/plain")
            return
        ctype = MEDIA_TYPES.get(rel.rsplit(".", 1)[-1].lower())
        if not ctype:
            self._send(404, b"that kind is not served", "text/plain")
            return
        self._send_file(os.path.join(DEMO_DIR, rel), ctype)

    def do_GET(self):
        # "/" is the explanation. The dark side-by-side view is still here at /race, and the long
        # version of the pitch at /site-full; neither is the front door any more.
        if self.path.startswith("/race") or self.path == "/live":
            self._send(200, PAGE.encode("utf-8"), "text/html; charset=utf-8")
        elif self.path in ("/", "/index.html", "/site", "/site/"):
            try:
                body = open(GUIDE_PATH, "rb").read()
                self._send(200, body, "text/html; charset=utf-8")
            except FileNotFoundError:
                self._send(404, b"no front page", "text/plain")
        elif self.path.startswith("/site-full"):
            try:
                body = open(SITE_PATH, "rb").read()
                self._send(200, body, "text/html; charset=utf-8")
            except FileNotFoundError:
                self._send(404, b"site not built", "text/plain")
        elif self.path.startswith("/applications-verified"):
            try:
                self._send(200, open(VERIFIED_PATH, "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"not tested yet", "text/plain")
        elif self.path.startswith("/evaluation"):
            try:
                self._send(200, open(EVAL_PATH, "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"no evaluation", "text/plain")
        elif self.path.startswith("/measurements"):
            try:
                body = open(MEASURE_PATH, "rb").read()
                self._send(200, body, "application/json")
            except FileNotFoundError:
                self._send(404, b"no measurements", "text/plain")
        elif self.path.startswith("/state"):
            with LOCK:
                body = json.dumps(STATE).encode("utf-8")
            self._send(200, body, "application/json")
        elif self.path.split("?")[0] == "/endorsement.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/gate-endorsement.json", "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"no endorsement on file", "text/plain")
        elif self.path.split("?")[0] == "/policy.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/gate-policy.json", "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"no published policy yet", "text/plain")
        elif self.path.split("?")[0] == "/receipts.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/gate-receipts.json", "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"no signed log yet", "text/plain")
        elif self.path.split("?")[0] == "/checkpoint.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/checkpoint.json", "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"no checkpoint on file", "text/plain")
        elif self.path.split("?")[0] == "/witness.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/witness.json", "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"no countersignature on file", "text/plain")
        elif self.path.split("?")[0] == "/witness-ledger.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/witness-ledger.json", "rb").read(), "application/json")
            except FileNotFoundError:
                self._send(404, b"the witness has recorded nothing yet", "text/plain")
        elif self.path.split("?")[0] == "/gate-lab":
            try:
                self._send(200, open(f"{DEMO_DIR}/gate-lab.html", "rb").read(), "text/html; charset=utf-8")
            except FileNotFoundError:
                self._send(404, b"no lab page", "text/plain")
        elif self.path.split("?")[0] == "/live-state":
            with LOCK:
                side = dict(STATE["sides"]["live"])
            with LIVE_LOCK:
                side["running"] = LIVE["running"]
                side["took_ms"] = LIVE["took_ms"]
            self._send_json(200, side)
        elif self.path.split("?")[0] == "/compare-run.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/compare/last.json", "rb").read(),
                           "application/json")
            except FileNotFoundError:
                self._send(404, b"no comparison has happened yet", "text/plain")
        elif self.path.split("?")[0] == "/live-run.json":
            try:
                self._send(200, open(f"{DEMO_DIR}/live/last.json", "rb").read(),
                           "application/json")
            except FileNotFoundError:
                self._send(404, b"no run has happened yet", "text/plain")
        elif self.path.split("?")[0] == "/demos.css":
            try:
                self._send(200, open(f"{DEMO_DIR}/demos.css", "rb").read(),
                           "text/css; charset=utf-8")
            except FileNotFoundError:
                self._send(404, b"no stylesheet", "text/plain")
        elif self.path.split("?")[0] == "/fieldwork":
            self._send(200, fieldwork_page().encode("utf-8"), "text/html; charset=utf-8")
        elif self.path.split("?")[0].startswith("/demos"):
            slug = self.path.split("?")[0][len("/demos"):].strip("/")
            name = DEMO_PAGES.get(slug)
            if name is None:
                self._send(404, b"no such demonstration", "text/plain")
            else:
                try:
                    self._send(200, open(f"{DEMO_DIR}/demos/{name}", "rb").read(),
                               "text/html; charset=utf-8")
                except FileNotFoundError:
                    self._send(404, b"that demonstration is not built", "text/plain")
        elif self.path.split("?")[0].startswith("/api/"):
            self._api()
        elif self.path.startswith("/media/"):
            self._media(self.path[len("/media/"):])
        else:
            self._send(404, b"not found", "text/plain")

    def _api(self):
        """The live half of the demonstrations. Fixed cases only; nothing here is written."""
        asked = urlparse(self.path)
        query = parse_qs(asked.query)
        ip = self.client_address[0]
        if not _allowed(ip):
            self._send_json(429, {"ok": False, "error": "too many requests; wait a moment"})
            return
        wanted = asked.path[len("/api/"):].strip("/")
        try:
            if wanted == "verdict":
                body, why = api_verdict(query)
            elif wanted == "tamper":
                body, why = api_tamper(query.get("case", [""])[0])
            elif wanted == "endorse":
                body, why = api_endorse(query.get("case", [""])[0])
            elif wanted == "witness/check":
                body, why = api_witness_check(query.get("case", [""])[0])
            elif wanted == "verify":
                body, why = api_verify()
            elif wanted == "live/start":
                if not _live_allowed(ip):
                    self._send_json(429, {"ok": False, "error":
                                          "you have started several runs; wait a few minutes"})
                    return
                # The page numbers the tasks from one, because a person picks a task, not an index.
                n = int(query.get("task", ["0"])[0])
                body, why = live_start(n - 1)
            elif wanted == "live/result":
                body, why = live_result()
            elif wanted == "live/verify":
                body, why = api_live_verify()
            elif wanted == "compare/start":
                if not _live_allowed(ip):
                    self._send_json(429, {"ok": False, "error":
                                          "you have started several runs; wait a few minutes"})
                    return
                body, why = compare_start(int(query.get("task", ["0"])[0]) - 1)
            elif wanted == "compare/result":
                body, why = compare_result()
            elif wanted == "dir/start":
                task_n = int(query.get("task", ["0"])[0])
                body, why = dir_start(task_n)
            elif wanted == "dir/result":
                body, why = dir_result()
            elif wanted == "compare/verify":
                body, why = compare_verify()
            else:
                body, why = None, "no such call"
        except Exception as exc:  # a demonstration that dies silently looks like a demonstration
            body, why = None, "the call failed on this machine: %s" % exc
        if body is None:
            self._send_json(400, {"ok": False, "error": why})
        else:
            self._send_json(200, body)

    def _send_json(self, code, payload):
        self._send(code, json.dumps(payload).encode("utf-8"), "application/json")

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        if self.path.startswith("/fieldwork/submit"):
            self._send(200, fieldwork_received(raw).encode("utf-8"), "text/html; charset=utf-8")
            return
        try:
            payload = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            self._send(400, b"bad json", "text/plain")
            return
        if self.path.startswith("/live/reset"):
            live_reset(payload.get("goal", ""), payload.get("skill", ""))
        elif self.path.startswith("/reset"):
            reset(payload.get("goal", ""))
        elif self.path.startswith("/push"):
            push(payload)
        else:
            # Anything else is not a thing this server does. Saying {"ok":true} to an unknown path
            # tells a caller it succeeded when nothing happened — which is worse than a refusal.
            self._send(404, b'{"ok":false,"error":"unknown path"}', "application/json")
            return
        self._send(200, b'{"ok":true}', "application/json")


def main():
    # Loopback only. nginx reaches it from this machine, the runtime pushes over this machine, and
    # nothing else has any business talking to it. Binding to every interface published the
    # showcase's own mutating endpoints to the internet, which the page never needed.
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"showcase listening on 127.0.0.1:{PORT}", flush=True)
    _restore()
    srv.serve_forever()


if __name__ == "__main__":
    main()
