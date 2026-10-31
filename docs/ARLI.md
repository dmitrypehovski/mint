# mint in the ARLI ecosystem

ARLI is an ecosystem with several lines. This project is one of them. This
document places it relative to the others and describes three concrete ways the
lines could meet. It states plainly which of those exist today (none, as live
integrations) and what each would need.

## The four lines

- **commons** — a shared floor where members sell *knowledge*. A skill is called
  for a fee, and the author is paid a share. The thing traded is what someone
  knows how to do.
- **match** — finds who overlaps with whom: which members' skills and needs line
  up.
- **connect** — settles trade between parties, and receipts *what was read*. The
  record is of the knowledge that changed hands.
- **mint** — this project. It does *work* and receipts *what was done*. The
  record is of an action performed on a page, gated before it ran and signed
  before and after.

The distinction between connect and mint is the axis: connect records a read,
mint records a done. They are complementary records, not competing ones.

## Route 1: a skill sold in commons returns a mint receipt

A compiled skill can be listed and sold — `mint arli-payload` already shapes a
skill into a marketplace listing of entries, each a set of terms and an answer:

```
$ mint arli-payload --skill gh-commit.skill.json --out payload.json
listing written to payload.json: 2 entries, $0.20 per call
```

The buyer of such a skill currently gets the skill. What they do not get is proof
of what actually ran when it was executed for them. If a skill sold in commons
returned a mint receipt — the same signed, chained log `mint.arliwork.com`
publishes — the buyer would hold an object that shows a decision was taken and
signed before the effect, an outcome was signed after it, and the whole thing
obeys a published set of rules. The buyer could verify it offline.

**What this would need.** commons (or whatever serves the skill) to carry a
receipt, or a reference to one, back to the caller alongside the result; the
runtime executing the skill to be running with signing on and to publish its
rules next to the receipt; and the buyer's client to know the receipt's format.
None of that is wired up today.

## Route 2: the rules document as a shared artifact

The rules are a document (`mint.policy/0.1`), and a receipt carries the hash of
the exact document its verdicts were reached under. That makes the policy a
natural shared artifact across the ecosystem: a group can publish the policy it
expects its runtimes to obey, and every runtime that adopts it produces logs that
all reference the same policy hash. A third party then takes the ecosystem's
policy, applies it to the observations recorded in any member's log, and checks
that the verdicts are reproduced — or are stricter than the policy requires, never
laxer. One policy, many independent logs, all checkable against the same
document.

**What this would need.** A published ecosystem policy (a URL and a stable
document — the hash makes any change visible); runtimes that adopt it as their
`--policy`; and verifiers willing to check members' logs against the ecosystem
policy rather than against the copy embedded in each log. The mechanism is built
(`mint rules`, `mint receipts --policy`); the shared document and the agreement
to use it are not.

## Route 3: a second party holds the endorser key

Everything code can do, it can do. The one thing code cannot supply is a *second
party*: a key held by someone other than the runtime, who signs the policy hash
to say "these rules, for this runtime, in this scope". An endorsement proves that
some key other than the runtime's signed exactly that policy hash for exactly
that runtime, and that nobody altered the statement since. It cannot make the
counterparty real. The demonstration publishes an endorsement that says so in its
own note — both keys are on one machine, so it shows the mechanism and not trust.

In an ecosystem, this is the natural division of labour: mint is the runtime, and
some *other* ARLI party — a governance body, a counterparty, a member acting as
auditor — holds the endorser key and signs the policy each runtime is allowed to
run under. The runtime is structurally unable to sign its own endorsement
(`mint endorse` refuses to run under the runtime's own key), so the endorsement
is meaningful only when the key lives elsewhere.

**What this would need.** An actual second party that holds a key and signs, and
a shared understanding of what a signature from that party is worth. This is the
least technical of the three routes and the one that most depends on people
rather than code.

## Status: none of these is a live integration

None of the three routes exists today as a working cross-line integration. This
project is self-contained: it runs, gates, receipts and endorses within its own
boundary, and the demonstration's two keys are both on the machine that produced
it. The routes above describe where the pieces *could* join, and each names what
would have to exist first. They are stated here as directions, not as features.

What *is* built and checkable today: the gate, the signed and chained receipts,
the rules document and its replay, the counter-signature checks, and the
`arli-payload` command that shapes a skill into a listing. See the README
quickstart and `docs/VERIFY.md`.
