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

## An approval is spent when the action is performed

The record is the memory, not the process. If the action an approval names has already been released
once — in this session, in an earlier one, on another machine that shares the log — the next call is
refused with the receipt that released it. That refusal does not come from the rules and no approval
changes it.

So: if you are refused with "this action was already released once", **do not go looking for another
approval and do not retry**. Either the work has been done, or somebody else did it. Say which receipt
released it and stop.

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
