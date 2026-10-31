# levOS

A small agent runtime that solves a task in a browser once with a model in the
loop, compiles that run into a program, and then repeats the task without a
model.

The part that matters is not the speed. It is that the runtime cannot touch a
page except through a gate that decides by the side effect of a step before
performing it, writes a signed receipt before and after every action it
performs (two per action, one for a refusal), and publishes the rules it
decided by as a document, so that someone who was not there can re-derive every
verdict from the record.

## The problem

An agent that claims to have done work is asking to be believed. Logs are
writable by the party that keeps them, approvals are asserted rather than
shown, and a "we refused to do that" is indistinguishable from a "we never
tried". There is no object a stranger can hold that establishes what an agent
actually did, under which rules, with nobody's word needed. That object is what
this repository produces.

## The four pieces

1. **The write gate** (`src/gate.rs`). Every step crosses this boundary before
   it can change anything. A step is classified by the side effect it would
   have: reads pass; writes need an approval bound to that exact action and are
   released at most once; anything unclassifiable is refused. The gate is a
   required argument of the only function in the runtime that can act on a page,
   so it cannot be called around — removing it is a build error.
2. **Signed receipts** (`src/receipt.rs`). One small Ed25519-signed object per
   step, hash-chained so each commits to the one before it. A performed action
   gets two: a decision, signed before the effect, and an outcome, signed after
   it. A refused action gets only the decision, and that absence is the evidence
   that nothing happened.
3. **The rules as a document** (`src/policy.rs`). The gate decides by a JSON
   document, not by branches in the code. The same function decides and
   re-decides, so a verifier applies the published rules to the observations
   recorded in a receipt and checks that the same verdict comes back out.
4. **A counter-signature** (`src/endorse.rs`). A second party signs the hash of
   the rules, for a named runtime, in a stated scope, with an optional expiry.
   A runtime must never produce its own.

The live demonstration is at **https://mint.arliwork.com**, and eight
demonstrations you can poke at — four of them running the real verifier on
demand rather than showing a recording — are at
**https://mint.arliwork.com/demos**.

The site publishes three files: `/receipts.json` (a real signed log),
`/policy.json` (the rules), and `/endorsement.json` (the counter-signature). A
stranger can verify all three locally with one command, below.

## Quickstart

Build (the first build is slow; later ones take seconds):

```
git clone https://github.com/dmitrypehovski/levos && cd levos
cargo build --release
export PATH="$PWD/target/release:$PATH"   # so `levos` resolves to the binary
```

Verify the live published artifacts. Fetch the three files and check them:

```
mkdir levos-verify && cd levos-verify
curl -sO https://mint.arliwork.com/receipts.json
curl -sO https://mint.arliwork.com/policy.json
curl -sO https://mint.arliwork.com/endorsement.json

levos receipts \
    --in receipts.json \
    --policy policy.json \
    --endorsement endorsement.json
```

That command, run against the files as they were served, prints:

```
log of 7 receipts, agent 0cd20b19fef0
  policy a8bed408510e v1 — Reads pass. Anything that commits is held until an approval … (from policy.json)
  claim: A receipt proves that this record was issued by the holder of the stated key and has not been altered since. It does not prove the action was correct, that its inputs were true, that a person approved it, or that the world changed the way the record describes.

  receipts.json
    OK — 7 receipts, every signature and every link in the chain holds, and 7 of them reproduced from the published rules

  endorsed by afbf35b6d276 (from endorsement.json, a different key from the runtime's)
    scope: the gate demonstration published at mint.arliwork.com
    until: 1796064340023
    note:  signed by the counterparty key. Both keys are on this machine, so this shows the mechanism, not trust.
  verdict: log verified, rules replayed; the rules were accepted by somebody other than us
```

`OK` means every signature verifies, every link in the chain holds, and applying
the published rules to the observations recorded in the log reproduces all seven
recorded verdicts. The endorsement line means a key other than the runtime's
signed exactly this policy hash for exactly this runtime; it does not mean the
holder of that key is anyone you trust (see the threat model).

To see the same command catch a log that lies, add `--tamper <dir>`. It writes
five altered logs and four altered endorsements to that directory and reports
which check fires for each. See `docs/VERIFY.md` for the output.

To run the rules engine by itself, with no browser, model or network:

```
./target/release/levos rules \
    --policy demo/gate-policy.json \
    --observations demo/gate-observations.json
```

## Embedding the gate and policy in your own agent

The three pieces worth embedding — the gate, the receipts and the rules document
— stand alone and need no browser, model or network. The public API is in
`src/lib.rs`; `docs/EMBEDDING.md` shows how to put the gate in the path of an
action so that removing it will not compile, record observations, and verify
someone else's log.

The smallest example is the rules engine on its own:

```rust
use levos::policy::{ruling, Observed, Policy};

let rules = Policy::shipped();
let observed = Observed {
    action: "click".into(),
    tag: "button".into(),
    role: "button".into(),
    has_href: false,
    approval: false,
};
let said = ruling(&observed, &rules);
assert_eq!(said.verdict, "hold");
```

`cargo test` runs this as a doc-test, so the example in the source is checked by
the build rather than trusted.

## What it does not prove

A receipt proves that a record was issued by the holder of a stated key and has
not been altered since. Nothing more. In particular it does **not** prove that
the action was the right one, that its inputs were true, that a person approved
it, or that the world changed the way the record describes. Timestamps are the
signer's own clock and nothing attests to them. The demonstration publishes an
endorsement, but both keys in that demonstration live on one machine, so it
shows the mechanism and not trust. The strength of every claim here is the
strength of whoever holds the keys. `docs/THREAT-MODEL.md` states the limits in
full; it is the document to read before relying on any of this.

## Repository layout

```
src/
  gate.rs        the write gate: classify a step, decide, record before acting
  receipt.rs     Ed25519 receipts, the hash chain, and verify()
  policy.rs      the rules as a document, and the one function that rules
  endorse.rs     the counterparty's counter-signature and its checks
  lib.rs         the embeddable library
  main.rs        the command-line runtime
  cdp.rs         the browser driver (Chrome DevTools Protocol)
  skill.rs       a compiled task
  state.rs       comparing live page state against what a step expects
  decide.rs      the model adapter
docs/
  RECEIPT-FORMAT.md   field-by-field receipt and log format, for another language
  POLICY-FORMAT.md    the rules document and the restrictiveness ordering
  VERIFY.md           what verification checks, and how a log can lie
  THREAT-MODEL.md     what a receipt does and does not prove
  EMBEDDING.md        using the library in another Rust agent
  ARLI.md             how this fits an ecosystem of other agent lines
demo/
  gate-receipts.json  a real signed log (7 receipts)
  gate-policy.json    the rules those receipts were decided under
  gate-endorsement.json  the counter-signature
  gate-observations.json  the observations, exactly as recorded
  evaluation.json     18 real compiled-task runs, each with a verdict
  receipts/           the per-run signed logs those runs produced
```

## License

Apache-2.0. See `LICENSE`.
