#!/bin/bash
# The gate, in the path the agent cannot walk around.
#
# The harness runs this before every shell command and obeys the answer. That matters because a skill is
# only documentation: a live session with the skill installed committed without asking, and the log stayed
# empty. Here the answer is not a suggestion to the model, it is a decision the harness enforces.
#
# Fail closed. If mint cannot be found, nothing is judged, and a command that was not judged does not run
# — with a sentence saying how to fix it, because a gate that stops work without explaining itself gets
# switched off, and then it is not a gate either.
set -u

MINT_BIN="${MINT_BIN:-}"
if [ -z "$MINT_BIN" ]; then
  for candidate in "$HOME/.local/bin/mint" "$(command -v mint 2>/dev/null)" /usr/local/bin/mint; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then MINT_BIN="$candidate"; break; fi
  done
fi

if [ -z "${MINT_BIN:-}" ] || [ ! -x "$MINT_BIN" ]; then
  cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"mint is not installed, so this command could not be judged and was not run. Install it with: curl -fsSLO https://github.com/dmitrypehovski/mint/releases/latest/download/install.sh && bash install.sh"}}
JSON
  exit 0
fi

exec "$MINT_BIN" hook "$@"
