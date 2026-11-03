#!/usr/bin/env bash
# Judge one action before doing it, and record the decision.
#
#   guard.sh --command "git commit -m 'x'"          what an agent is about to run
#   guard.sh --action observation.json              an observation read by the caller
#   guard.sh --command "..." --approval a.json      with a person's approval presented
#
# Exit status is mint's, and it is the part a caller branches on:
#   0  allowed — go ahead
#   2  held    — a person's approval for this exact action would change the answer
#   1  refused — the rules say no, and an approval does not change that
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipts="${MINT_RECEIPTS:-.mint}"
mkdir -p "$receipts"
exec "$here/mint.sh" guard --receipts "$receipts" "$@"
