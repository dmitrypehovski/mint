# Threat model

This document states plainly what a receipt proves and what it does not. It is
the credibility signal of the whole system: a receipt that promises more than it
can support is worse than no receipt, because a reader cannot tell where the
support ends. Where the code and the documentation could tell a nicer story, the
code wins.

## What a receipt proves

A receipt proves exactly two things, and the receipt carries the statement in
itself so no reader has to infer the scope. The constant in `src/receipt.rs`:

> A receipt proves that this record was issued by the holder of the stated key
> and has not been altered since. It does not prove the action was correct, that
> its inputs were true, that a person approved it, or that the world changed the
> way the record describes.

So, positively:

- **Authenticity.** The record was signed by the private key corresponding to
  the public key named in `body.agent`. Only a holder of that key could produce
  the signature; that is what Ed25519 gives.
- **Integrity.** The body has not been altered since signing. Any change to any
  field breaks the signature.
- **Chain integrity.** The receipts form a hash chain: each commits to the
  previous one's link hash, so a receipt cannot be silently deleted, reordered or
  inserted without the chain breaking at that point.
- **Order of decision and effect.** A performed action has a decision receipt
  signed *before* the effect and an outcome receipt signed *after* it. A refused
  action has only the decision receipt, and the absence of a second receipt is
  evidence that nothing was performed.
- **Rule compliance.** When the published rules are supplied, the recorded
  verdicts are reproduced from the recorded observations — or, for state-dependent
  verdicts, are shown to be at least as restrictive as the rules require.

## What a receipt does not prove

### It does not prove the action was the right one

A receipt records that a step was allowed and performed under a rule. It says
nothing about whether that was the correct thing to do. An agent can be
perfectly well-receipted and still do the wrong work, for the wrong reason,
against the wrong target.

### It does not prove the inputs were true

The `observed` object is what the runtime says it saw. If the runtime observes
incorrectly — or lies about what it observed — the receipt records the lie
faithfully. The rules are replayed against the recorded observations, so a
verifier checks that the verdict follows from the observations; it does not and
cannot check that the observations match the page. Nothing here attests to the
truth of the perception layer.

### It does not prove a person approved

`observed.approval` being `true` means a token matching the identity of that
exact action was presented. It does not mean a human saw the action, understood
it, or agreed to it. The approval token format is `approve:{identity}`, a
string; it carries no signature and no identity of who approved. A runtime that
sets `approval: true` without a human deciding anything produces a receipt that
reads as approved and is not.

### It does not prove the world changed as described

The outcome receipt's `status` (`"completed"` or `"failed"`) and `result_note`
are the runtime's report. `result_hash` is the sha256 of the note string — a
fixed identity for the note, not evidence about the page. The receiver of the
action, on the far side, is not party to the log and has signed nothing.

### It does not prove trust, when both keys live on one machine

The demonstration at `mint.arliwork.com` carries an endorsement, and it says so
in its own note: "signed by the counterparty key. Both keys are on this machine,
so this shows the mechanism, not trust." Running the endorsement with two keys
we generated ourselves demonstrates that the mechanism works and proves nothing
about who stands behind the second key. The endorser's signature can prove that a
key other than the runtime's signed exactly this policy hash for exactly this
runtime. It cannot make the counterparty real. The strength of the endorsement is
the strength of whoever holds the second key, and no amount of code changes that.

### Timestamps are the signer's own clock

`decided_at_ms`, `signed_at_ms`, `at_ms` and `not_after_ms` come from the
signing machine's clock (`std::time::SystemTime::now()`). Nothing attests to
them. A machine with a wrong or manipulated clock produces receipts with wrong or
manipulated times. The endorsement's `not_after_ms` is compared against the
verifier's own clock, so expiry is anchored to the verifier — but the times
inside the receipts remain the runtime's. There is no external time authority
here.

### The environment the observations came from is not attested

A receipt describes a page by fields the runtime chose: tag, role, has_href,
action. It does not commit to the actual HTML, the rendered pixels, the URL the
browser was on, the browser build, or the machine. A verifier cannot reconstruct
the page from a receipt or confirm it existed. Two runtimes could record the same
observations from entirely different pages.

### Key custody is the whole strength of the claim

Every guarantee above rests on one thing: who holds the private key. If the key
is lost, old receipts can no longer be tied to this agent. If the key is stolen,
the holder can sign anything and it will verify. The gate is a control on what
*this* runtime does; it is not a control on what a key holder can claim. The key
file is written with owner-only permissions and lives outside the repository
(`/root/.secrets/mint-receipt.key` here), and `.gitignore` excludes `*.key` so a
checkout never ships one — but that is hygiene, not a security boundary.

## The gate's own honesty about its limits

`src/gate.rs` describes the boundary and then states what is not yet true of it:

> the approvals here are tokens carrying the identity of the action they
> authorise, compared as strings. They are not signed, and the decision log is
> not yet hash-chained. That is the next layer, and until it exists a receipt
> from this runtime is a record, not a proof.

Read together with the receipts layer, the accurate summary is: the *receipts*
are signed and hash-chained (that is `src/receipt.rs`, and it is what
`mint receipts` verifies); the *approvals* remain unsigned strings, and the
decision log inside the gate is a working buffer that becomes signed receipts
when signing is switched on. A reader who needs approvals to be signed documents
should not yet rely on this runtime for that; the recommendation is stated here
rather than buried.

## Summary

| question | answer |
|---|---|
| Was this record issued by the key holder and left unaltered? | Yes, if the signature verifies. |
| Does the chain show nothing was deleted or reordered? | Yes, if the links hold. |
| Did a decision come before the effect? | Yes for performed actions: two receipts, ordered. |
| Did the runtime obey the rules it published? | Yes, if the replay reproduces the verdicts (or is stricter). |
| Was the action correct? | Not proven. |
| Were the inputs true? | Not proven. |
| Did a person approve? | Not proven — approvals are unsigned strings. |
| Did the world change as described? | Not proven. |
| Is the endorser someone to trust? | Not by itself — in the demonstration, both keys are on one machine. |
| Are the timestamps trustworthy? | They are the signer's own clock; nothing attests to them. |
| Is the environment attested? | No. |
