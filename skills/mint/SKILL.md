---
name: mint
description: Judge an action before it happens and leave a signed receipt for it. Use before anything that writes — a commit, a push, a deploy, a deletion, a payment, a form submission, a config change, a migration — to have it judged against a published rules document, held when it commits something, and recorded in a receipt chain that somebody else can verify without trusting you or this machine. Also use to verify a log of receipts you were handed, to issue an approval for one specific action, and to show what an agent was allowed to do.
argument-hint: "[the command you are about to run, or a log to verify]"
allowed-tools:
  - Bash(${CLAUDE_SKILL_DIR}/scripts/mint.sh *)
  - Bash(${CLAUDE_SKILL_DIR}/scripts/guard.sh *)
  - Bash(${CLAUDE_SKILL_DIR}/scripts/verify.sh *)
  - Bash(${CLAUDE_SKILL_DIR}/scripts/approve.sh *)
---

# mint

Every action an agent takes either passes through a published set of rules or it does not happen, and
each decision leaves a signed receipt behind. The point is not the permission. It is that afterwards
somebody who trusts neither you nor this machine can check what was allowed, what was held, and who said
yes.

**In Codex** (or any agent that left the lines above unfilled): `${CLAUDE_SKILL_DIR}` means this skill's
folder, the one holding this SKILL.md.

## Before you do something that writes

Ask, and read the status:

```bash
${CLAUDE_SKILL_DIR}/scripts/guard.sh --command "git commit -m 'fix the parser'"
```

```
  write   hold   — this step commits something and no approval bound to it was presented
  decided by mint.policy/0.1 v1, rules a8bed408510ec43a
  the action is called guard:2877d388e3395fa4
  receipt 0cd20b19fef0-0000 written to .mint/guard.json — the log now holds 1
```

The status is the part you act on:

- **0 — allowed.** Do the thing.
- **2 — held.** Stop. The rules are not refusing; they are asking for a person. Nothing has happened yet.
- **1 — refused.** Stop. An approval would not change this, so do not go looking for one.

Reads pass without ceremony, so ask before anything that changes something. The rules are a document, not
a mood: `--policy <file>` judges by a file you were given, and without it the shipped document is used.

## Never approve your own action

An approval is somebody else's answer about one specific action. If you are holding it, it was not
somebody else's.

When the status is **2**, tell the person:

1. what you are about to do, in one sentence;
2. the action's name (`guard:…`) and the reason the rules gave, in the rules' own words;
3. that `scripts/approve.sh --action guard:… --days 1 --note "…"` issues an approval for exactly that
   action, and that it must be run by whoever is answering for it, with their own key.

Then wait. When they have approved, present it and the same action goes through:

```bash
${CLAUDE_SKILL_DIR}/scripts/guard.sh --command "git commit -m 'fix the parser'" --approval approval.json
```

The receipt now names the approver. An approval for a different action, from a key this runtime was not
told to trust, for another runtime, or expired, is refused by name — say which check failed rather than
retrying.

## The hook is the gate; this file is the explanation

Claude Code runs `hooks/hooks.json` before every shell command, and that hook is what stops a command the
rules hold. You will meet it as a **denied tool call** whose reason names the action and the exact command
that approves it. That is not a suggestion: the command did not run.

Do not try to reach the same effect another way — a different command, a shell wrapper, a script written to
a file and run — because the hook sees the command you run and the receipt records what was judged. When a
command is held, tell the person the action's name and the reason, and wait. When you are refused, say so
and stop; a refusal is not overridable.

## An approval is spent when the action is performed

The record is the memory, not the process. If the action an approval names has already been released
once — in this session, in an earlier one, on another machine that shares the log — the next call is
refused with the receipt that released it. That refusal does not come from the rules and no approval
changes it.

So: if you are refused with "this action was already released once", **do not go looking for another
approval and do not retry**. Either the work has been done, or somebody else did it. Say which receipt
released it and stop.

## You cannot approve your own action, and should not try

An approval is a person's act. Running `mint approve` (or `revoke`, or the wrapper scripts) from inside this
session is refused by the hook, the refusal is written to the log, and the command does not run.

So: when a command is held, **tell the person the action's name and the reason, and stop**. Do not run the
approval yourself, do not write a script that runs it, do not look for another route to the same effect.
The record shows what was attempted, and an attempt is not a mistake worth hiding — it is worth reporting.

## A withdrawal is not a mistake to work around

An approval can be taken back after it was given: a person issues a withdrawal with
`mint revoke --action guard:…`, and a runtime given it stops counting that approval. You will see
"the approval presented is withdrawn" and the action will be held again.

Tell the person who withdrew it and why it said so. **Do not look for another approver, do not edit the
withdrawal, and do not present the same approval again.** A withdrawal exists because something changed;
the only useful thing you can add is the reason the withdrawal gave.

## Verifying a log you were handed

```bash
${CLAUDE_SKILL_DIR}/scripts/verify.sh .mint/guard.json --policy rules.json
```

The status is 0 only when the log holds. Against its own rules a log shows only that it agrees with
itself, so pass the document you were given whenever you have it. **Report what it printed, including a
failure, in its own words.** A log that does not hold is the finding, not an inconvenience: do not
re-sign it, do not edit it, do not describe it as verified.

## What a receipt says, and what it does not

It says: this action was observed this way, judged this way by this document, at this time, by a runtime
holding this key, and (where one was used) approved by this named person at this time. It carries a
commitment to the parameters rather than the parameters themselves, so a password typed into a field is
not in it, and neither is your code.

It does not say: that the action was right, that its inputs were true, that a person read the diff, or
that the world changed the way the record describes. Do not present it as if it did.

## Where the receipts go

`.mint/` in the project, one growing chain (`guard.json`). They are meant to be kept and handed over —
that is what they are for — and they contain no code and no values. Give `MINT_RECEIPTS` to put them
somewhere else.
