#!/usr/bin/env python3
"""The signed approvals the five checks are demonstrated on.

Each case is a real approval, signed by a real key and produced by `mint approve`. The page runs them
through `mint approve-check`, which calls the same function the gate calls and refuses in the same
words. Nothing here is a mock: between the cases only one thing varies, and it is which check the
approval fails.

Run by tools/publish.sh, after the binary is built.
"""
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MINT = os.path.join(ROOT, "target", "release", "mint")
OUT = os.path.join(ROOT, "demo", "approval-cases")

# The action the page checks against, and one it does not, so \"the right approval for the wrong thing\"
# can be shown as the refusal it is.
ACTION = "click:button:Submit"
OTHER_ACTION = "click:button:Cancel"

# A key this machine uses to answer for actions, and a key it has no reason to trust at all.
APPROVER_KEY = os.environ.get("MINT_APPROVER_KEY", "/root/.secrets/mint-approver.key")
STRANGER_KEY = "/tmp/mint-stranger-approver.key"
# Some other runtime's identity, for the case where an approval is real but issued to somebody else.
OTHER_RUNTIME = "3f1c" * 16

DAY = 86_400_000


def mint(args, key=None):
    env = dict(os.environ)
    if key:
        env["MINT_APPROVER_KEY"] = key
    done = subprocess.run([MINT] + args, capture_output=True, text=True, env=env)
    text = (done.stdout or "") + (done.stderr or "")
    if done.returncode != 0:
        sys.stderr.write("mint %s failed:\n%s" % (" ".join(args), text))
        sys.exit(1)
    return text


def issue(name, action, days=None, key=None, for_agent=None, issued=None, expires=None, note=""):
    path = os.path.join(OUT, name + ".json")
    args = ["approve", "--action", action, "--out", path]
    if note:
        args += ["--note", note]
    if for_agent:
        args += ["--for", for_agent]
    if days is not None:
        args += ["--days", str(days)]
    if issued is not None:
        args += ["--issued-ms", str(issued)]
    if expires is not None:
        args += ["--expires-ms", str(expires)]
    mint(args, key=key)
    return path


def main():
    if not os.path.exists(MINT):
        sys.exit("build the binary first: cargo build --release")
    os.makedirs(OUT, exist_ok=True)
    now = int(time.time() * 1000)

    # The one that holds: right issuer, right action, right runtime, still inside its time bounds.
    issue(
        "good",
        ACTION,
        days=7,
        note="checked the invoice against the order by hand",
    )
    issuer = json.load(open(os.path.join(OUT, "good.json")))["sig"]["kid"]

    # Four that do not, each failing a different check.
    issue("unpinned", ACTION, days=7, key=STRANGER_KEY, note="signed by a key nobody was told to trust")
    issue("other-action", OTHER_ACTION, days=7, note="the right issuer, the wrong thing")
    issue("other-runtime", ACTION, days=7, for_agent=OTHER_RUNTIME, note="issued to another runtime")
    issue(
        "expired",
        ACTION,
        issued=now - 2 * DAY,
        expires=now - 1 * DAY,
        note="valid yesterday, which is not now",
    )

    # The pinned set: one key, with the name to record beside it.
    with open(os.path.join(OUT, "approvers.json"), "w") as fh:
        json.dump({"approvers": [{"key": issuer, "name": "dmitry"}]}, fh, indent=2)

    print("  five approvals written to demo/approval-cases, pinned to %s" % issuer[:12])


if __name__ == "__main__":
    main()
