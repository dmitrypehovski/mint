# The observation, and what the rules look at

One action, in the shape the rules were written for. `guard.sh --action <file|->` takes this; `--command`
fills it in from the command through the same adapter that reads commands during a run.

```json
{
  "action": "click",
  "tag": "button",
  "role": "button",
  "has_href": false,
  "approval": false
}
```

- `action` — what is being done: `click`, `type`, `navigate`, `commit`, `command`, `read`…
- `tag` — the kind of thing it is: `button`, `a`, `input`, `command`, `file`.
- `role` — the role the page gives it, or `command` for a shell command.
- `has_href` — whether it points somewhere. A link that navigates is not a write; a button that submits is.
- `approval` — whether an approval bound to this exact action was presented. Set it through
  `--approval`, not by hand: the approval is checked before it is believed.

## What the shipped document does with it

- anything it can read — passes;
- anything that commits something — **held**, until an approval naming that exact action is presented;
- anything it cannot classify — **refused**. Not guessed at, not allowed with a warning.

The document is a file. Read it, argue with it, change it, and pass it with `--policy`. The hash of the
one in force goes into every receipt, so a verifier can see which rules were applied and apply them again.
