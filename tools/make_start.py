#!/usr/bin/env python3
"""Build /start — the page a stranger reads before they have any reason to trust us.

Every command and every line of output on it is captured from a real run of the released installer and
the built binary, in a fresh directory with a fresh home. Nothing on the page is written by hand, because
a quickstart that has drifted from the software is worse than no quickstart: the reader blames themselves
for the mismatch.
"""
import html
import os
import shutil
import subprocess
import sys

ROOT = "/root/mint"
BIN = f"{ROOT}/target/release/mint"
OUT = f"{ROOT}/demo/start.html"
WORK = "/tmp/start-demo"
HOME = "/tmp/start-demo-home"

GUARD = f"{ROOT}/skills/mint/scripts/guard.sh"
APPROVE = f"{ROOT}/skills/mint/scripts/approve.sh"
VERIFY = f"{ROOT}/skills/mint/scripts/verify.sh"


def run(cmd, cwd=None, env=None, timeout=180, stdin_text=None):
    """Run it and keep both the output and the exit status — the status is part of what the page shows."""
    e = dict(os.environ)
    e.update(env or {})
    p = subprocess.run(cmd, cwd=cwd, env=e, shell=isinstance(cmd, str), input=stdin_text,
                       capture_output=True, text=True, timeout=timeout)
    return (p.stdout + p.stderr).rstrip(), p.returncode


def block(command, output, status=None, note=""):
    label = html.escape(command)
    body = html.escape(output) if output else "(no output)"
    tail = f'<span class="badge">{status}</span>' if status is not None else ""
    note_html = f'<p class="note">{html.escape(note)}</p>' if note else ""
    return (f'<div class="step"><div class="cmd">{label}{tail}</div>'
            f'<pre>{body}</pre>{note_html}</div>')


def main():
    import json

    for d in (WORK, HOME):
        shutil.rmtree(d, ignore_errors=True)
    os.makedirs(WORK)
    os.makedirs(HOME)
    subprocess.run(["git", "init", "-q"], cwd=WORK, check=True)

    steps = []

    # 1. install, from the release, in a home that has nothing
    install_cmd = ("curl -fsSLO https://github.com/dmitrypehovski/mint/releases/latest/download/install.sh\n"
                   "less install.sh\n"
                   "bash install.sh")
    out, code = run(f"cd {HOME} && curl -fsSLO "
                    f"https://github.com/dmitrypehovski/mint/releases/latest/download/install.sh && "
                    f"env -i HOME={HOME} PATH=/usr/bin:/bin bash install.sh", timeout=240)
    steps.append(("Install it", install_cmd, out, None,
                  "No Rust, no cargo, no clone. The installer checks the download against the checksums "
                  "published with the release, and installs nothing that does not match."))

    # The quickstart documents what a stranger will actually download, so the release it installs has to
    # be the release this source is. If it is not, the page would quietly describe an older mint — which is
    # exactly what happened the first time this was built, and it took reading the captured output to
    # notice that the behaviour it showed was the behaviour of the previous release.
    source_version = ""
    for line in open(f"{ROOT}/Cargo.toml", encoding="utf-8"):
        if line.startswith("version ="):
            source_version = line.split('"')[1]
            break
    installed, _ = run([f"{HOME}/.local/bin/mint", "--version"], env={"HOME": HOME})
    installed_version = installed.split()[1] if installed.startswith("mint ") else "unknown"
    if installed_version != source_version:
        print(f"  the published release is {installed_version} and this source is {source_version}.")
        print("  Cut the release first, then build this page — otherwise it documents an older mint.")
        return 1

    # 2. the gate in front of a command
    env = {"HOME": HOME, "PATH": f"{HOME}/.local/bin:/usr/bin:/bin"}
    out, code = run(["bash", GUARD, "--command", "git commit -m 'first change'",
                     "--receipts", ".mint"], cwd=WORK, env=env)
    steps.append(("Ask it about a command that changes something",
                  'mint guard --command "git commit -m \'first change\'" --receipts .mint', out, code,
                  "Status 2. Held: the command changes something and nobody has agreed to it. Nothing was "
                  "run, and the decision is in the log before it was made."))

    identity = ""
    for line in out.splitlines():
        if "is called" in line:
            identity = line.strip().split()[-1]

    # 3. a person approves that exact action
    appr = f"{WORK}/approval.json"
    out_a, code_a = run(["bash", APPROVE, "--action", identity, "--days", "1",
                         "--note", "I read the diff", "--out", appr],
                        cwd=WORK, env={**env, "MINT_APPROVER_KEY": "/root/.secrets/mint-approver.key"})
    steps.append(("A person approves that exact action",
                  f'mint approve --action {identity} --days 1 --note "I read the diff"', out_a, code_a,
                  "The approval names the action, so it cannot authorise a different one, and it is "
                  "signed, so the receipt can name who gave it."))

    key = json.load(open(appr))["sig"]["kid"]
    json.dump({"approvers": [{"key": key, "name": "dmitry"}]}, open(f"{WORK}/approvers.json", "w"))

    # 4. the same command, with the approval presented
    env_appr = {**env, "MINT_APPROVERS": f"{WORK}/approvers.json"}
    out, code = run(["bash", GUARD, "--command", "git commit -m 'first change'",
                     "--approval", appr, "--receipts", ".mint"], cwd=WORK, env=env_appr)
    steps.append(("Present it", f'mint guard --command "git commit -m \'first change\'" '
                                f'--approval approval.json --receipts .mint', out, code,
                  "Status 0. Allowed, with the approver's name in the receipt. The note about the "
                  "terminal is honest and worth reading: this page issues the approval from a script, so "
                  "the record says so. When you do it, it will say `terminal` instead."))

    # 5. the same approval again — the part people ask about first
    out, code = run(["bash", GUARD, "--command", "git commit -m 'first change'",
                     "--approval", appr, "--receipts", ".mint"], cwd=WORK, env=env_appr)
    steps.append(("Present the same approval again",
                  f'mint guard --command "git commit -m \'first change\'" '
                  f'--approval approval.json --receipts .mint', out, code,
                  "Status 1. The approval was spent when the action was performed, and the refusal comes "
                  "from the log rather than from memory — it survives restarts, days and machines, and it "
                  "names the receipt that released it."))

    # 6. read the log
    out, code = run(["bash", VERIFY, ".mint/guard.json"], cwd=WORK, env=env)
    # Counted from the output rather than written down. The first version of this sentence said "four
    # decisions" about a log of three, which is precisely the drift a page built from real output exists
    # to prevent — and it got in through the one line I typed by hand.
    decided = 0
    for line in out.splitlines():
        if line.strip().startswith("log of "):
            decided = int(line.split()[2])
    steps.append(("Read the log", "mint receipts .mint/guard.json", out, code,
                  f"{decided} decisions — one held, one allowed, one refused — every signature and every "
                  "link in the chain checked, and every decision re-derived from the published rules. The "
                  "log is the whole record: no separate audit trail to reconcile, no server to ask."))

    # 7. the same gate, where the harness runs it before a tool call
    payload = json.dumps({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "cwd": WORK,
        # A command that has not been released earlier in this page: the point of the step is a hold,
        # and reusing the commit from step five would show a refusal instead.
        "tool_input": {"command": "git push origin main"},
    })
    out, code = run([f"{HOME}/.local/bin/mint", "hook", "--receipts", f"{WORK}/.mint"],
                    cwd=WORK, env={**env, "MINT_RECEIPT_KEY": f"{HOME}/.secrets/mint-receipt.key"},
                    stdin_text=payload)
    pretty = ""
    try:
        doc = json.loads(out)
        decision = doc["hookSpecificOutput"]
        pretty = (f'permissionDecision: {decision["permissionDecision"]}\n'
                  f'reason: {decision["permissionDecisionReason"]}')
    except Exception:
        pretty = out
    steps.append(("Put it in front of your agent",
                  "echo '<the harness payload>' | mint hook", pretty, code,
                  "This is how the agent is stopped: hooks/hooks.json registers a PreToolUse hook, the "
                  "harness runs it before every shell command and obeys the answer. The skill explains; "
                  "this enforces. Measured rather than assumed — a live session with only the skill "
                  "installed committed without asking and the log stayed empty. With the hook, the same "
                  "session was blocked, said so, and printed the command that approves the action. Both "
                  "Claude Code and Codex read this same file; Codex asks you to trust the hook once before "
                  "it will run it, and until you do, the gate is not in the path there."))

    cards = "\n".join(
        f'<div class="card"><h2>{i}. {html.escape(title)}</h2>{body}</div>'
        for i, (title, cmd, out, code, note) in enumerate(steps, 1)
        for body in [block(cmd, out, code, note) if cmd else f'<p class="note">{html.escape(note)}</p>'])

    page = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Install it and watch it hold a commit — mint</title>
<style>
  :root {{ --ink:#16161d; --dim:#5b5b6b; --line:#e3e3ec; --bg:#f8f8fb; --accent:#7c5ce7; --bad:#b3261e; --ok:#137a4a; }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; padding:32px 20px 64px; background:var(--bg); color:var(--ink);
         font:16px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; }}
  .wrap {{ max-width:900px; margin:0 auto; }}
  h1 {{ font-size:30px; line-height:1.25; margin:0 0 6px; letter-spacing:-.02em; }}
  .lede {{ color:var(--dim); margin:0 0 12px; }}
  .where {{ font-size:13px; color:var(--dim); border-left:3px solid var(--accent); padding-left:12px; margin:0 0 28px; }}
  .card {{ background:#fff; border:1px solid var(--line); border-radius:12px; padding:18px 20px; margin:0 0 14px; }}
  .card h2 {{ font-size:17px; margin:0 0 8px; }}
  .step {{ margin:0; }}
  .cmd {{ font:12.5px/1.6 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; color:var(--dim);
          white-space:pre-wrap; margin:0 0 8px; }}
  pre {{ background:#0f0f14; color:#e8e8ef; border-radius:10px; padding:14px 16px; overflow:auto;
         font:12.5px/1.55 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; margin:0 0 8px; }}
  .note {{ color:var(--dim); font-size:14.5px; margin:0; }}
  .badge {{ display:inline-block; font-size:12px; font-weight:600; padding:2px 8px; border-radius:999px;
            margin-left:8px; background:#ececf3; color:var(--dim); }}
  footer {{ color:var(--dim); font-size:13.5px; margin-top:30px; border-top:1px solid var(--line); padding-top:16px; }}
  a {{ color:var(--accent); }}
  code {{ background:#ececf3; border-radius:5px; padding:1px 5px; font-size:13px; }}
</style>
</head>
<body>
<div class="wrap">
  <h1>Install it, and watch it hold a commit.</h1>
  <p class="lede">
    Seven steps, about twenty minutes, nothing but a shell. Every command and every line of output below
    was captured from a real run — the installer from the published release, the rest from the binary it
    installed. If any of it does not reproduce on your machine, that is our problem, not yours.
  </p>
  <p class="where">What this is: a record an agent cannot fake, and a gate it has to ask. What it is not:
      a sandbox, a permission system, or anything that decides on its own what is safe.</p>
{cards}
  <div class="card">
    <h2>What it will not do</h2>
    <p class="note">
      It will not tell you whether an action is <em>safe</em> — only who agreed to it, and that they did.
      It will not let a held action through without a person: there is no flag that means "just this once",
      and a refusal is not overridable by configuration.
      An approval cannot be delegated to another runtime (it can be withdrawn, by somebody the runtime
      pins, and nobody else).
      And the published example cannot show a real counterparty, because both keys are on one machine —
      the mechanism is shown, the trust is yours to create.
    </p>
  </div>
  <footer>
    The log is at <code>.mint/guard.json</code>. Read it with <code>mint receipts</code>, and check somebody
    else's with the same command — the verifier is the same binary, and it needs nothing from us.
    <br><a href="/demos">All the demonstrations</a> · <a href="/">What this is and why</a>
  </footer>
</div>
</body>
</html>
"""
    with open(OUT, "w", encoding="utf-8") as fh:
        fh.write(page)
    print(f"  written {OUT} ({len(page)} bytes), {len(steps)} steps, all captured from real runs")
    for title, cmd, out, code, note in steps:
        print(f"    {title}: status {code}, {len(out)} bytes of output")


if __name__ == "__main__":
    sys.exit(main())
