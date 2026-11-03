#!/usr/bin/env bash
# Verify a log of receipts. Exit status is 0 only when the log holds.
#
#   verify.sh .mint/guard.json                     against the rules the log itself carries
#   verify.sh .mint/guard.json --policy rules.json  against a document you were given
#
# The difference matters: against its own rules a log only shows that it agrees with itself.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ "$#" -eq 0 ]; then
  echo "usage: verify.sh <log.json> [--policy <rules.json>] [--endorsement <file>]" >&2
  exit 3
fi
exec "$here/mint.sh" receipts --in "$@"
