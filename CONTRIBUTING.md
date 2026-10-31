# Contributing

## Build and test

```
cargo build --release      # first build is slow; later ones take seconds
cargo test                 # unit tests plus the doc-test from src/lib.rs
```

`cargo test` runs three unit tests in `src/policy.rs` and one doc-test in
`src/lib.rs`. The doc-test is the embedding example in the library docs; it must
keep compiling and passing, because it is the example a reader will copy.

## Run what you document

Every command in the docs must have been run, and its real output belongs in the
docs. If a document states a number, a message or a verdict, that statement must
be reproducible by a command that is also in the document. Do not describe a
feature the code does not have; where the code and a nicer story disagree, follow
the code and change the story.

When you add or change a command-line command, update `src/help.txt` (the `levos
help` output is that file) and any doc that references it. When you change the
receipt, policy or endorsement format, update the matching `docs/*-FORMAT.md` and
the field tables in it.

## Changing the rules document

A receipt carries the hash of the exact policy it was decided under, and already-
published receipts are checked against the rules that produced them. So:

- **A change to the rules must not invalidate already-published receipts.** If a
  published log was decided under `mint.policy/0.1` version 1, that document must
  remain available and unchanged, so the log still verifies. New behaviour goes
  into a new version with a new hash; the old one stays published.
- **The shipped rules are behaviour-preserving.** `Policy::shipped()` is written
  to reproduce exactly what the runtime did before the rules became data. If a
  change to it changes a verdict, the receipts already issued under it stop
  verifying. `src/policy.rs` has a test for this intent
  (`the_rules_are_the_thing_that_decides`); add to it rather than around it.
- **Sign nothing you cannot replay.** If you change `ruling` or `classify`, make
  sure `levos receipts --policy` still reproduces recorded verdicts, or the
  forged-log case in `docs/VERIFY.md` loses its meaning.

## Keys

Never commit a key. Signing keys live outside the repository
(`/root/.secrets/` on this machine, read-only to the owner) and `.gitignore`
excludes `*.key`. The endorser key must be a different key from the runtime's;
`levos endorse` refuses to sign with the runtime's own key, and that refusal is
deliberate — do not add a flag to bypass it.

## Receipts and private data

A receipt records observations about a page (tag, role, whether an element
points somewhere) and a signed agent key. Do not add fields that carry page
content, user data or anything that should not end up in a published, permanent
log. The `Observed` struct is intentionally narrow for this reason.

## Style

Match the surrounding prose in the docs: plain statements, no marketing
adjectives, no exclamation marks. The code comments explain *why* a decision was
made; keep that habit. An honest note about what a thing does not do is worth
more than a confident claim about what it does.
