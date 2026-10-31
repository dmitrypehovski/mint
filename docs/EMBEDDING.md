# Embedding

The library (`levos` crate, `src/lib.rs`) is four pieces that stand alone under a
browser, model or network: the write gate (`levos::gate`), the receipts
(`levos::receipt`), the rules document (`levos::policy`), and the
counter-signature checks (`levos::endorse`). Any of them can be taken into
another Rust agent and the rest left behind.

The one design rule that makes this more than a library call: **the gate is an
argument of the one function that can act on a page.** In this runtime that
function is `perform`, and its signature requires a gate:

```rust
async fn perform(
    cdp: &mut Cdp,
    gate: &mut gate::Gate,      // required: an action cannot be taken without it
    step: &Step,
    index: usize,
    el: &Element,
    approval: Option<&str>,
) -> Result<(bool, Option<String>)>
```

Inside it, the gate decides and the effect follows the decision:

```rust
let (verdict, key) = gate.decide(step, index, el, approval);
if !verdict.performed() {
    return Ok((false, Some(format!("{}: {}", verdict.as_str(), verdict.reason()))));
}
let ok = /* click or type the element */;
gate.record_outcome(&key, ok, if ok { "performed" } else { "the page did not take it" });
```

Because the gate is a required argument, a caller cannot perform an action
without a decision being written first; removing the parameter is a compile
error, not a missed convention. Copy that shape into your agent: make your
action function take `&mut Gate` and nothing else can reach the page.

## 1. Decide by rules, with nothing attached

The smallest useful piece. No browser, no key, no network — and it is the same
code path the runtime uses, so a verifier and the runtime agree by construction:

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
assert_eq!(said.class, "write");
assert_eq!(said.verdict, "hold");
```

`ruling` returns a `Ruling { class, verdict, reason }`. `class` is `read`,
`write` or `unknown`; `verdict` is `allow`, `hold` or `refuse`. This is the
function to call if you want the rules but not the rest of the runtime. The
observations you feed it are the same struct (`Observed`) a receipt records, so
if you also emit receipts, a verifier can replay the same call.

## 2. Put the gate in the path of an action

The gate adds the decision log, the single-use release set, and — when signing
is switched on — the receipts. It decides from a `step` (a `levos::skill::Step`)
and an `element` (a `levos::cdp::Element`). If your agent already has its own
action and element types, you have two options:

- Build the `Step` and `Element` the gate expects from your own data. `Element`
  is `{ handle, tag, role, name, href }` and `Step` describes the action, the
  element's role, and where it points. The gate reads only `action`, `tag`,
  `role`, `href`, `learned_page`, and a few identity fields.
- Or call `levos::policy::ruling` with an `Observed` you build yourself, exactly
  as in case 1, and use `Gate::with_policy` only if you want the release-set
  behaviour. `Observed` is the vocabulary the rules match on; the gate builds it
  in `gate::observe(action, element, approval)`.

A gate decides by a specific document. The shipped rules are the default; to
decide by your own:

```rust
use levos::gate::Gate;
use levos::policy::Policy;

let rules = Policy::from_file("your-policy.json")?;
let mut gate = Gate::with_policy("example.com", rules);
```

`gate.decide(step, index, element, approval)` returns `(Verdict, key)`. The
verdict is `Allow {}`, `Hold { reason }` or `Refuse { reason }` (every variant is
an object, so all verdicts in a log have one shape). `key` is the idempotency key
you pass back to `gate.record_outcome(key, ok, note)` after the effect; the same
key twice is refused (`decided_by = "once-only"`). `gate.blocked()` returns the
steps that were not allowed, with reasons, and `gate.report()` returns the tally
and the decision log.

## 3. Issue signed receipts

Signing turns the gate's decisions into the objects a stranger can check. Load or
create the signing key, start a log, and hand it to the gate:

```rust
use levos::gate::Gate;
use levos::policy::Policy;
use levos::receipt::{ReceiptLog, Sealer};

let rules = Policy::shipped();
let sealer = Sealer::load_or_create("/path/to/agent.key")?;   // 0600 hex seed file
let log = ReceiptLog::new("example.com", &rules, sealer);

let mut gate = Gate::new("example.com");
gate.sign_with(log);            // from here on, every decision is receipted
```

After the run, write the log:

```rust
let document = gate.receipt_log().unwrap().document();
std::fs::write("receipts.json", serde_json::to_string_pretty(&document)?)?;
```

`document()` is the log document `docs/RECEIPT-FORMAT.md` specifies: metadata,
the rules themselves, the optional endorsement, and the receipts. Every decision
gets a receipt before the effect; every performed action gets a second one after
it. A refused or held action gets only the decision receipt, and that absence is
the evidence nothing happened.

Keep the key file owner-only and out of version control (`.gitignore` excludes
`*.key`). Losing the key means old receipts can no longer be tied to this agent;
that cost is why it is not in the repository.

There is a second entry point for re-signing an existing set of receipts,
`ReceiptLog::adopt`, which recomputes every link and re-signs. It exists so that
the verifier can be tested against a forger who holds the key (the `forged` case
in `docs/VERIFY.md`); it is the only way to produce a log where every signature
is valid and the chain is intact but the verdicts are laxer than the rules.

## 4. Verify someone else's log

Two public functions do the whole check:

```rust
use levos::policy::Policy;
use levos::receipt::verify;

let document: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("receipts.json")?)?;
let rules = Policy::from_file("policy.json")?;        // the independent check
let report = verify(&document, Some(&rules));

assert!(report.ok, "log did not verify");
println!("{} receipts checked, {} replayed", report.checked, report.replayed);
```

`verify(document, Option<&Policy>)` returns a `VerifyReport`:

| field | meaning |
|---|---|
| `ok` | true only when the replay found nothing and there were no repeated effects |
| `checked` | receipts whose signature and link both hold |
| `first_break` | the first broken signature or chain link, as `{ seq, id, reason }`, or null |
| `replayed` | receipts the rules reproduced (0 when no policy was supplied) |
| `repeated_effects` | idempotency keys with more than one outcome receipt |
| `policy_break` | what the rules found, if anything |

Pass `None` for the rules and the check is self-consistency only — it confirms
the log agrees with itself, not that it obeys rules from outside it. Pass the
rules from a file you hold to get the independent check. The same reasoning
applies to the endorsement: check it against the rules and the runtime you have
in hand, not against the copy embedded in the log:

```rust
use levos::endorse;

let runtime = document.get("agent").and_then(|a| a.as_str()).unwrap_or("");
let e: levos::endorse::Endorsement = serde_json::from_value(
    document.get("endorsement").cloned().unwrap_or(serde_json::Value::Null)
)?;
match endorse::check(&serde_json::to_value(&e)?, &rules, runtime, endorse::now_ms()) {
    Ok(e) => println!("endorsed by {} for scope {:?}", &e.body.endorser[..12], e.body.scope),
    Err(why) => println!("endorsement rejected: {why}"),
}
```

`endorse::check(document, rules, runtime, now)` returns the parsed endorsement or
the reason it is not a counter-signature. It rejects an endorsement signed by the
runtime's own key, one over different rules, one naming a different runtime, and
one past its expiry — see `docs/VERIFY.md`.

## Putting it together

The minimum credential an embedded runtime should carry:

1. A rules document (`Policy`), published wherever the receipts are.
2. A signing key (`Sealer`) held by the runtime, and a `ReceiptLog`.
3. A gate that is a required argument of the action path.
4. `verify()` on the way out, so the runtime refuses to publish a log it cannot
   itself check.
5. If trust is wanted, a second key held by somebody else that signs the policy
   hash (`endorse::sign`); the runtime must never sign its own.

Nothing above needs the browser driver (`levos::cdp`), the skill compiler
(`levos::skill`), or the model adapter (`levos::decide`). Those are the parts of
*this* agent; the four pieces are what you take.
