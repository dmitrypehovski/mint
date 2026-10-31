# Policy format

Spec string: `mint.policy/0.1` (constant `SPEC` in `src/policy.rs`).

The rules are a document, not code. The same function (`policy::ruling`) decides
in the gate and re-decides in a verifier, so a third party can apply the
published rules to observations recorded in a receipt and check that the same
verdict comes back out. This document specifies the document, the matching
rules, and the one ordering a verifier depends on: a runtime may be stricter
than the rules it published, never laxer.

## The document (struct `Policy`)

Fields in declaration order:

| field | type | meaning |
|---|---|---|
| `spec` | string | `"mint.policy/0.1"` |
| `version` | number (u32) | the document's own version, bumped when the rules change |
| `note` | string | what the document means, in the document — so a person can read what they are being asked to accept |
| `on_write_without_approval` | string | what a write with no approval bound to it gets: `"hold"` or `"refuse"` |
| `on_unclassifiable` | string | what a step matching no rule gets: `"hold"` or `"refuse"` |
| `approvals_are_single_use` | boolean | an approval authorises one performance of one action, not a category |
| `reads` | array of `Rule` | tried first |
| `writes` | array of `Rule` | tried after `reads` |

### `Rule`

| field | type | meaning |
|---|---|---|
| `when` | array of `Matcher` | every matcher in the list must hold for the rule to apply |
| `class` | string | the class to assign when it does: `"read"` or `"write"` |

### `Matcher`

| field | type | meaning |
|---|---|---|
| `attr` | string | one of `action`, `tag`, `role`, `has_href` |
| `value` | string | the value that attribute must equal |

The observation side (`policy::Observed`) is the same struct that is recorded in
every receipt, so a verifier replays the rules against exactly what the runtime
had in front of it:

| field | type |
|---|---|
| `action` | string (`"click"` or `"type"`) |
| `tag` | string |
| `role` | string |
| `has_href` | boolean |
| `approval` | boolean |

## Matching

`policy::classify` walks the rules for a step:

```rust
for rule in policy.reads.iter().chain(policy.writes.iter()) {
    if observed.matches(rule) {
        return rule.class.clone();
    }
}
"unknown".into()
```

The order is fixed:

1. **Reads first, then writes.** A rule in `writes` is never consulted while a
   `reads` rule still matches.
2. **First match wins.** The first rule whose matchers all hold decides the
   class; later rules are not considered.
3. **No match is `unknown`.** `unknown` is a class, not a wildcard. It is not a
   class any rule can assign — a rule's `class` is only ever `read` or `write`.
   A step that matches nothing is classified `unknown`.

Each matcher is tested against the step's `attrs()`:

```rust
("action",  self.action.clone()),
("tag",     self.tag.to_lowercase()),
("role",    self.role.to_lowercase()),
("has_href", if self.has_href { "yes" } else { "no" }),
```

`action` is compared as-is. `tag` and `role` are lowercased on the observation
side, and the matcher's `value` is lowercased before comparison, so case does not
matter for those two. `has_href` is written as the string `yes` / `no`, not a
boolean, so that both halves of a matcher are strings and the document stays
readable.

## From class and approval to a verdict (`policy::ruling`)

| class | condition | verdict |
|---|---|---|
| `read` | — | `allow` |
| `write` | `observed.approval` is true | `allow` |
| `write` | no approval | `on_write_without_approval` |
| `unknown` | — | `on_unclassifiable` |

An approval reaches the ruling only after the gate has checked it against the
identity of *this* action; by the time ruling sees `approval: true`, the token
named this exact action. Matching a token is state, not a rule, so the policy is
asked only about the answer.

The verdict set is exactly `allow`, `hold`, `refuse`.

## `approvals_are_single_use`

In the shipped document this is `true`. The gate releases an allowed action's
idempotency key at most once: a second attempt at the same key is refused with
`decided_by = "once-only"` ("this action was already released once; a repeat is
not performed"). That refusal is state-dependent — no rule can see it, because
it depends on what the run has already done — so it is recorded as `once-only`
rather than `policy`.

## Restrictiveness ordering: stricter is allowed, laxer is not

This is the rule a verifier relies on. Define an order over verdicts:

```
allow  <  hold  <  refuse
   0        1       2
```

In the source (`src/receipt.rs`):

```rust
fn restrictiveness(verdict: &str) -> u8 {
    match verdict {
        "allow" => 0,
        "hold" => 1,
        _ => 2,
    }
}
```

A verdict with a **higher** number is more restrictive. The verification rule is:

- For a receipt whose decision was reached from the rules (`decided_by ==
  "policy"`), the published rules must reproduce the verdict **exactly**. If they
  give `hold` and the receipt records `allow`, that is a break.
- For a receipt whose decision was reached from the run's own state (`decided_by
  == "once-only"`), the rules cannot reproduce it, so the test is **direction**,
  not equality: the recorded verdict must be at least as restrictive as the
  verdict the rules give. Refusing more than the rules require is safe; allowing
  more is not:

```rust
let too_lax = restrictiveness(&r.body.decision.verdict) < restrictiveness(&ruling.verdict);
let wrong = if state_dependent { too_lax } else { ruling.verdict != r.body.decision.verdict };
```

So a runtime may be **stricter** than the rules it published — it may hold or
refuse where the rules would allow — and the log still verifies. It may never be
**laxer**: it may not allow what its own rules hold or refuse. Allowing what the
policy holds is the failure that matters, and it is caught here even when every
signature is valid and the chain is intact (`docs/VERIFY.md` walks through the
`forged` case).

The gate only ever refuses more than a policy requires through this path, because
the shipped policy's `on_write_without_approval` is `hold` and the released-set
refusal is `refuse`, both of which are at least as restrictive as a bare
`allow` from the rules.

## The policy hash

```
policy_hash = hex(sha256(canonical))
canonical   = serde_json::to_string(self)   // field order = declaration order
```

Every receipt carries the `policy_hash` it was decided under, and the log
document carries the rules themselves. A verifier recomputes the hash from the
rules it holds and requires it to match the receipts' `policy_hash` before
replaying. If the rules were changed after the log was signed, the hashes differ
and the replay stops at the first receipt — this is the `policy` tamper case.

## The published policy (verbatim)

This is `https://mint.arliwork.com/policy.json` (identical to
`demo/gate-policy.json`), reformatted compactly here; the file on the site is
pretty-printed. Its hash on the live log is `a8bed408510e…`:

```json
{"spec":"mint.policy/0.1","version":1,"note":"Reads pass. Anything that commits is held until an approval bound to that exact action is presented, and released once. A step that matches no rule is refused even if an approval is offered, because an approval authorises a described action and nothing here can be described.","on_write_without_approval":"hold","on_unclassifiable":"refuse","approvals_are_single_use":true,"reads":[{"when":[{"attr":"has_href","value":"yes"}],"class":"read"},{"when":[{"attr":"action","value":"type"}],"class":"read"},{"when":[{"attr":"tag","value":"a"}],"class":"read"},{"when":[{"attr":"role","value":"link"}],"class":"read"}],"writes":[{"when":[{"attr":"tag","value":"button"}],"class":"write"},{"when":[{"attr":"role","value":"button"}],"class":"write"},{"when":[{"attr":"tag","value":"input"}],"class":"write"}]}
```

Reading it in order: a step with somewhere to go is a read; typing is a read; a
link (even with no destination) is a read; anything with the role `link` is a
read. Then, if none of those matched: a button is a write; anything with role
`button` is a write; any other input is a write. Anything left over — for
example a `div` with role `tab` — matches nothing and is `unknown`, which the
document refuses (`on_unclassifiable: "refuse"`).

Applying it to the real observations in `demo/gate-observations.json`:

```
$ ./target/release/levos rules --policy demo/gate-policy.json --observations demo/gate-observations.json
  0. read    allow  — safe to perform
  1. read    allow  — safe to perform
  2. write   hold   — this step commits something and no approval bound to it was presented
  3. unknown refuse — this element is neither a navigation nor a button, so what it would do cannot be described to anyone; refused rather than tried
  4. write   allow  — an approval bound to this exact action was presented

  decided by mint.policy/0.1 v1, hash a8bed408510ec43a
```

This is the whole policy engine with no browser, model or network attached. It
is the same code path the runtime uses to decide, which is what makes the replay
in `verify()` a real check rather than a restatement.

## Grep commands that confirm the rules above

```
$ grep -n 'reads.iter().chain(policy.writes.iter())' src/policy.rs
147:    for rule in policy.reads.iter().chain(policy.writes.iter()) {
```

```
$ grep -n -A6 'fn restrictiveness' src/receipt.rs
385:fn restrictiveness(verdict: &str) -> u8 {
386-    match verdict {
387-        "allow" => 0,
388-        "hold" => 1,
389-        _ => 2,
390-    }
391-}
```

The first confirms reads are tried before writes, with first match winning. The
second confirms the ordering `allow < hold < refuse` that the strictness rule
uses.
