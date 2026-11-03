#!/usr/bin/env bash
# Issue an approval for exactly one action, signed with the approver's own key.
#
# Run this only when a person has said yes to this specific action. The key is MINT_APPROVER_KEY, or
# $HOME/.secrets/mint-approver.key. An approval names one action, one runtime and an expiry, and it is
# checked before it authorises anything: pinned issuer, signature, the action it names, the time bounds,
# and the runtime it was issued for.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$here/mint.sh" approve "$@"
