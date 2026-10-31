# Verification

One command checks a signed log:

```
mint receipts --in receipts.json --policy policy.json --endorsement endorsement.json
```

- `--in` is required: the log document.
- `--policy` is optional. With it, the published rules are replayed against the
  recorded observations — the independent check. Without it, the rules are taken
  from the log itself, which only shows that the log agrees with itself; the
  command says which was used.
- `--endorsement` is optional. With it, the counter-signature is checked against
  the rules and the runtime. Without it, any endorsement embedded in the log is
  used, and failing that the command reports that nobody outside the runtime has
  signed the rules.

## What is checked, and in what order

For each receipt, in log order:

1. **The chain link.** `prev_hash` must equal the `link_hash` of the previous
   receipt (null for the first). This runs before the signature, so a deleted or
   reordered receipt is reported as a chain break at the point it stops lining
   up.
2. **The signature.** `sig.value` and `body.agent` are decoded from hex;
   `sig.value` must be 64 bytes and `body.agent` 32 bytes and a valid Ed25519
   public key; then `verify_strict` must accept the signature over the canonical
   body bytes. A receipt that was edited after signing fails here.

Across the whole log:

3. **Repeated effects.** Any idempotency key with more than one `outcome`
   receipt is an effect that happened twice — the one thing a replayer must never
   do. This is a property of the log, so it is checked across all receipts.
4. **The replay** (only when rules are supplied). For each receipt:
   - its `policy_hash` must equal the hash of the rules in hand, and
   - the rules applied to `action.observed` must produce the same `class`
     (`side_effect`) the receipt recorded, and
   - the verdict must match: **exactly** for `decided_by == "policy"`, or be at
     least as restrictive for `decided_by == "once-only"` (see
     `docs/POLICY-FORMAT.md`).

The report's `ok` is true only when the replay found nothing and there were no
repeated effects.

## The report

A clean log:

```
  receipts.json
    OK — 7 receipts, every signature and every link in the chain holds, and 7 of them reproduced from the published rules
```

If no policy was supplied, the tail reads "though the rules were not replayed".

A log with a policy break:

```
  <path>
    BROKEN at receipt <seq> (<id>) — the rules disagree
    <reason>
```

A log with a repeated effect:

```
  <path>
    BROKEN — an effect was performed more than once
    the same action, released twice: <key>
```

A broken signature or chain link:

```
  <path>
    BROKEN at receipt <seq> (<id>)
    <reason>
    verified before the break: <n>
```

"verified before the break" is how many receipts passed before the first
failure; a high number means most of the log is intact and one point is not.

## Five ways of lying to a log

Running with `--tamper <dir>` writes five altered copies to that directory and
reports which check fires. This is the part that matters more than checking an
honest log: a verifier shown only an honest log has not been shown to verify
anything. The five cases, with the real output from a run against the live
artifacts:

```
$ mint receipts --in receipts.json --policy policy.json --endorsement endorsement.json --tamper tamper

  a line edited, signature left alone
      → caught at receipt 0
      the signature does not match this receipt — it was altered after signing, or signed with another key
  a receipt deleted from the middle
      → caught at receipt 2
      the chain is broken here: it points at 491dcc501d1d but the previous receipt hashes to 9e6abed0f9ca
  two receipts reordered
      → caught at receipt 2
      the chain is broken here: it points at 491dcc501d1d but the previous receipt hashes to 9e6abed0f9ca
  a held write rewritten to 'allow', then re-signed by the key holder
      → caught by the rules at receipt 2
      the rules give 'hold' for this step but the receipt records 'allow' — whatever decided it, it was not the policy that was published
  the rules changed after the log was signed
      → caught by the rules at receipt 0
      this receipt was decided under policy a8bed408510e but the policy supplied hashes to 88b822c2d8cf — the rules it claims to have followed are not the
```

What each case is, and why it is caught:

1. **Edited** — a body field changed and the signature left as it was. The
   signature check fires first: the canonical bytes no longer match the
   signature. This is the easy one.
2. **Dropped** — a receipt deleted from the middle. The following receipt still
   points at the deleted one's link hash, so the chain breaks where the gap is.
3. **Swapped** — two receipts reordered. Same mechanism: a receipt now follows
   the wrong predecessor, so `prev_hash` does not match the new previous link.
4. **Forged** — a held write rewritten to `allow`, and then the **whole chain
   re-signed by the holder of the key** (this is `ReceiptLog::adopt`, which
   recomputes every link and re-signs). Every signature is valid and the chain is
   intact, so checks 1–3 all pass. Only the replay catches it: the published
   rules give `hold` for that step, and the receipt records `allow`, which is
   laxer than the rules. This is the case that shows the replay is doing real
   work rather than restating the signatures.
5. **Policy** — the log is untouched but the rules supplied are a different
   document (here, a version whose `on_write_without_approval` is `refuse` before
   the log was signed against the `hold` version). The receipt's `policy_hash`
   does not match the rules in hand, so the replay stops at the first receipt.
   This shows why the policy is signed by hash: it pins which document the
   verdicts were reached under.

## Four ways of lying to an endorsement

An endorsement is a separate document with its own ways of lying. Each case
below is properly signed — the check under test is the one that has to fire. Real
output:

```
  the endorsement, and what it must not accept:
  signed by the runtime's own key
      — a valid signature from the same party that wrote the rules
      → rejected: the endorsement was signed by the runtime's own key — that is not a counter-signature, it is the same party agreeing with itself
  over a different policy
      — an endorsement of rules that are not these
      → rejected: the endorsement covers policy 4d5836a06b56 but the rules in hand hash to a8bed408510e
  naming a different runtime
      — these rules, but accepted for somebody else
      → rejected: the endorsement names runtime 00ff00ff but this log was written by 0cd20b19fef0
  already expired
      — a spent endorsement, however well signed
      → rejected: the endorsement expired at 1, which is 1793474392189 before now — a spent endorsement is not an endorsement
```

The checks in `endorse::check`, in order (identity before content, so the message
names the real problem):

1. The signature algorithm is `ed25519` (`unknown signature algorithm: …`).
2. The key that signed is the key the endorsement names
   (`sig.kid` must equal `body.endorser`) — otherwise "the endorsement names one
   endorser and was signed with another".
3. The signature verifies under that key — otherwise "the endorsement does not
   verify: …".
4. **It is not the runtime's own key.** The rejection reads "the endorsement was
   signed by the runtime's own key — that is not a counter-signature, it is the
   same party agreeing with itself". A verifier that accepted this would report
   a counter-signature where there is none, which is worse than having no
   endorsement support at all. This check is not optional.
5. **It covers these rules** — `body.policy_hash` must equal the hash of the
   rules in hand ("the endorsement covers policy … but the rules in hand hash to
   …").
6. **It covers this runtime** — `body.runtime` must equal the key that wrote the
   log ("the endorsement names runtime … but this log was written by …").
7. **It has not run out** — if `not_after_ms` is set, the verifier's own clock
   must be at or before it ("the endorsement expired at …, which is … before
   now — a spent endorsement is not an endorsement"). The expiry is compared
   against the verifier's clock; the receipts' own times remain the runtime's.

A verifier that accepted an endorsement signed by the runtime's own key would be
worse than one with no endorsement support at all, because it would report a
counter-signature where there is none.

## What verification does not establish

Verification establishes that the record was issued by the holder of the stated
key and has not been altered since, and that the verdicts obey the published
rules. It does not establish that the actions were the right ones, that the
inputs were true, that a person approved anything, or that the world changed as
described. Timestamps and the environment the observations came from are not
attested. See `docs/THREAT-MODEL.md`.
