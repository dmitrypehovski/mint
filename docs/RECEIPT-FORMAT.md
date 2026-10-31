# Receipt format

Spec string: `mint.receipt/0.1` (constant `SPEC` in `src/receipt.rs`).

This document is enough to implement the format in another language: it gives
every field, the exact canonical serialization rule, how the hash chain is
linked, and a worked example with a real receipt. Everything here is stated from
the code in `src/receipt.rs`; the grep commands that confirm the two rules that
matter are given at the end.

## Two documents

Two shapes are written from one log:

- **The log document** — a single JSON object with metadata, the rules, an
  optional endorsement, and the array of receipts. This is what
  `/receipts.json` is on the demonstration site.
- **One line per receipt** — the same receipts, one compact JSON object per
  line, for streaming or appending. Field order within a line is the struct's
  declaration order, so each line is exactly `{"body":<canonical body>,"sig":{…}}`.

Both are checked the same way. Only the array is read for verification; the rest
of the log document is context.

## The log document

Built by `ReceiptLog::document()` in `src/receipt.rs`.

| field | type | meaning |
|---|---|---|
| `spec` | string | `"mint.receipt/0.1"` |
| `host` | string | the site this runtime was driving (for example `127.0.0.1:8840`) |
| `agent` | string | the runtime's public key, 32 bytes hex — the key every receipt in this log is signed with |
| `policy_hash` | string | sha256 (hex) of the rules document the log was decided under |
| `policy` | object | the full rules document, kept whole so a verifier needs no other file |
| `endorsement` | object or null | the counter-signature, if any. Carried for convenience only; a verifier checks its signature wherever it found it |
| `claim` | string | what any receipt in this log supports (see below) |
| `count` | number | number of receipts |
| `receipts` | array | the receipts, oldest first |

The log document is emitted through `serde_json::json!` (a `serde_json::Value`).
Because this crate does not enable `serde_json`'s `preserve_order` feature, a
`Value` object is a sorted map, so **the keys of the log document and of every
receipt inside it are written in alphabetical order**. Field order in the file is
therefore not the struct's declaration order, and hashing the file text is not
how you compute the canonical form (see below). Verification reads the log back
into the structs and re-serializes; the on-disk order is irrelevant to it.

## The receipt

A receipt is `{"body": <body>, "sig": <sig>}` (structs `Receipt`, `ReceiptBody`,
`Sig` in `src/receipt.rs`).

### Body, field by field (declaration order)

| # | field | type | meaning |
|---|---|---|---|
| 1 | `spec` | string | `"mint.receipt/0.1"` |
| 2 | `seq` | number (u64) | position in the log, starting at 0 |
| 3 | `id` | string | `format!("{}-{:04}", &agent[..12], seq)` — the first 12 hex chars of the agent key, a hyphen, then the sequence zero-padded to 4 |
| 4 | `kind` | string | `"decision"` (written before the effect) or `"outcome"` (written after one) |
| 5 | `refers_to` | string or null | for an outcome, the `id` of the decision receipt it reports on; for a decision, null |
| 6 | `decided_at_ms` | number (u128) | when the decision was taken, ms since the Unix epoch |
| 7 | `signed_at_ms` | number (u128) | when this receipt was signed |
| 8 | `agent` | string | the signing public key, 32 bytes hex |
| 9 | `host` | string | the site the runtime was driving |
| 10 | `action` | object | the `ActionRef` (below) |
| 11 | `decision` | object | the `DecisionRef` (below) |
| 12 | `status` | string | decision receipts: `""`. outcome receipts: `"completed"` or `"failed"` |
| 13 | `result_hash` | string or null | decision receipts: null. outcome receipts: sha256 (hex) of the `result_note` UTF-8 bytes |
| 14 | `result_note` | string or null | decision receipts: null. outcome receipts: a short note (for a performed step, `"performed"`) |
| 15 | `policy_hash` | string | sha256 (hex) of the rules this receipt was decided under |
| 16 | `prev_hash` | string or null | the `link_hash` of the previous receipt; null for the first |
| 17 | `claim` | string | the `CLAIM` constant (below) |

### `action` (struct `ActionRef`)

| field | type | meaning |
|---|---|---|
| `identity` | string | sha256 (hex) of the canonical identity of the logical action (`gate::action_identity`) |
| `key` | string | the idempotency key: 32 hex chars that identify a repeat of the same effect (`gate::idempotency_key`) |
| `side_effect` | string | `"read"`, `"write"` or `"unknown"` — how the gate classified the step |
| `element` | string | a human-readable description, for example `button "Submit the filing"` |
| `step` | number | the step index within the compiled task |
| `observed` | object | the `Observed` struct — exactly what the runtime saw, so a verifier can apply the rules itself |

### `observed` (struct `policy::Observed`)

| field | type | meaning |
|---|---|---|
| `action` | string | `"click"` or `"type"` |
| `tag` | string | the element's tag name, lowercased by the matcher |
| `role` | string | the element's ARIA/role, lowercased by the matcher |
| `has_href` | boolean | whether the element points somewhere |
| `approval` | boolean | whether an approval matching this exact action was presented |

### `decision` (struct `DecisionRef`)

| field | type | meaning |
|---|---|---|
| `verdict` | string | `"allow"`, `"hold"` or `"refuse"` |
| `reason` | string | prose for a person; not part of what a verifier checks |
| `decided_by` | string | `"policy"` (the rules reproduce it) or `"once-only"` (it came from what this run had already done, which no rule can see) |

### `sig` (struct `Sig`)

| field | type | meaning |
|---|---|---|
| `alg` | string | `"ed25519"` |
| `kid` | string | the signing key's public identity — equal to `body.agent` |
| `value` | string | 64-byte Ed25519 signature, hex (128 chars) |

## Canonical serialization

The bytes that are signed, and the bytes the chain hashes, are:

```
serde_json::to_string(&body)
```

that is, the body struct serialized to compact JSON with **fields in declaration
order** — the order in the table above. No whitespace, no sorted keys, no
including the signature. This is stable across runs and machines because a
struct's fields always serialize in the order they are declared. The exact line
in the source is:

```rust
pub fn canonical(&self) -> String {
    serde_json::to_string(&self.body).unwrap_or_default()
}
```

A consequence to get right in another language: **do not hash the JSON text of
the on-disk log.** The log document's keys are sorted, and it is pretty-printed.
Recover the canonical form by producing the compact, declaration-ordered
serialization of the body object. The `…jsonl` file, by contrast, is written
straight from the struct, so each of its lines already contains the canonical
body bytes between `{"body":` and `,"sig":`.

## The signature

The signature covers exactly the canonical body bytes:

```
sig.value = ed25519_sign(sealer_key, canonical_body_bytes)
```

Verification (in `receipt::verify`) reconstructs the canonical bytes from the
parsed body, decodes `sig.value` and `body.agent` from hex, and calls
`verify_strict`. `body.agent` is the key that must have signed; a signature that
verified under a different key is rejected. See `docs/VERIFY.md`.

## The chain link

```
link_hash(receipt) = hex(sha256(canonical_body_bytes))
```

Each receipt's `prev_hash` is the `link_hash` of the receipt immediately before
it, or null for the first. In the source, the previous link is taken from the
log:

```rust
fn prev_hash(&self) -> Option<String> {
    self.receipts.last().map(|r| r.link_hash())
}
```

The chain commits to **content, not signatures**: `link_hash` is a hash of the
body only, and the signature is excluded. So `prev_hash` references the previous
receipt's `link_hash`, never its `sig.value`. A consequence follows from this:
if you alter a receipt's body, its own `link_hash` changes, so the *next*
receipt's `prev_hash` no longer matches, and the chain breaks at the next link
even though the altered receipt's own signature already fails first.

## Two receipts per action, one per refusal

- A step the gate **performs** gets a **decision** receipt (signed before the
  effect; `kind = "decision"`, `refers_to = null`, empty `status`) and then an
  **outcome** receipt (`kind = "outcome"`, `refers_to` = the decision's `id`,
  `status = "completed"` or `"failed"`, `result_note` set, `result_hash` =
  sha256 of the note).
- A step that is **held or refused** gets only the decision receipt. The absence
  of a second receipt for that decision is the evidence that nothing happened:
  the record that a decision was taken exists and was signed before anything
  could run, and no record of anything running follows it.

`result_hash` is `sha256` of the `result_note` string, hex — not of any page
content. It is a fixed identity for the note, so a note can be quoted and
matched without trusting the surrounding text.

## The claim, and the spec strings

Every receipt carries the exact claim it supports, so no reader has to infer the
scope. The constant is:

```
A receipt proves that this record was issued by the holder of the stated key and has not been altered since. It does not prove the action was correct, that its inputs were true, that a person approved it, or that the world changed the way the record describes.
```

The spec strings for the three documents are:

| document | spec string |
|---|---|
| receipt | `mint.receipt/0.1` |
| policy | `mint.policy/0.1` |
| endorsement | `mint.endorsement/0.1` |

## Worked example

This is receipt `seq = 0` from `demo/gate-receipts.jsonl`, the first line of a
real signed log. Body fields in canonical (declaration) order:

```json
{"spec":"mint.receipt/0.1","seq":0,"id":"0cd20b19fef0-0000","kind":"decision","refers_to":null,"decided_at_ms":1793472340210,"signed_at_ms":1793472340210,"agent":"0cd20b19fef027dd62d1f68534cf57175c6c84a96c29bcc01240c3f337c1178f","host":"127.0.0.1:8840","action":{"identity":"b418283c3378d637739630deba8d5f1c568e03df5be2385fdae3efd23d5ba5fc","key":"dc474eb0323f1e81a03ed5e6b7f3baf8","side_effect":"read","element":"a \"Open the record\"","step":0,"observed":{"action":"click","tag":"a","role":"link","has_href":true,"approval":false}},"decision":{"verdict":"allow","reason":"","decided_by":"policy"},"status":"","result_hash":null,"result_note":null,"policy_hash":"a8bed408510ec43af571e8bfc9cddf2322907a892c6758db88abe8d559d2fc80","prev_hash":null,"claim":"A receipt proves that this record was issued by the holder of the stated key and has not been altered since. It does not prove the action was correct, that its inputs were true, that a person approved it, or that the world changed the way the record describes."}
```

Compute the link hash of this body (sha256 over the compact UTF-8 bytes above):

```
9e6abed0f9ca2716132f413ffcf7bff020c8085f0fc796fe4de15aa4e64e078f
```

The next receipt (`seq = 1`, `id = 0cd20b19fef0-0001`) carries exactly

```
"prev_hash": "9e6abed0f9ca2716132f413ffcf7bff020c8085f0fc796fe4de15aa4e64e078f"
```

so the chain link holds. Receipt 0's own `prev_hash` is `null`, which is correct
for the first receipt in a log. Its signature (`sig.value`, 128 hex chars) is the
Ed25519 signature over those same canonical bytes, made with the key named in
`body.agent` — the key `0cd20b19fef0…`.

You can reproduce the link hash yourself:

```
python3 - <<'PY'
import json, hashlib
r = json.loads(open('demo/gate-receipts.jsonl').readline())
canonical = json.dumps(r['body'], separators=(',', ':'), ensure_ascii=False)
print(hashlib.sha256(canonical.encode()).hexdigest())
PY
```

This prints the hash above, matching the next receipt's `prev_hash`. `json.dumps`
of a Python dict preserves insertion order, and `json.loads` preserves the order
of the file, which is the declaration order — so the same canonical bytes come
out.

## Implementing it in another language

1. Parse the log document and take the `receipts` array.
2. For each receipt, parse `body` and `sig`.
3. Rebuild the canonical body bytes as compact JSON with fields in the order
   listed under "Body, field by field", recursing in the same order into
   `action`, `observed` and `decision`. Do not include `sig`.
4. `sha256(canonical)` is the receipt's `link_hash`.
5. The next receipt's `prev_hash` must equal the previous receipt's
   `link_hash`; the first must be null.
6. `ed25519_verify(body.agent, canonical, sig.value)` must succeed.
7. If you also hold the rules document, apply it to `action.observed` and check
   the verdict against `decision.verdict`. See `docs/POLICY-FORMAT.md` and
   `docs/VERIFY.md`.

## Grep commands that confirm the rules above

```
$ grep -n 'serde_json::to_string(&self.body)' src/receipt.rs
99:        serde_json::to_string(&self.body).unwrap_or_default()
```

```
$ grep -n 'Sha256::digest(self.canonical().as_bytes())' src/receipt.rs
105:        hex(&Sha256::digest(self.canonical().as_bytes()))
```

```
$ grep -n 'map(|r| r.link_hash())' src/receipt.rs
269:        self.receipts.last().map(|r| r.link_hash())
```

The first confirms the canonical form is `serde_json::to_string` of the body
struct; the second confirms `link_hash` hashes those canonical bytes; the third
confirms `prev_hash` is taken from the previous receipt's `link_hash` (not its
signature).
