#!/usr/bin/env bash
# Everything the public site is built from, in the order that leaves no artifact stale.
#
# The witness artifacts are generated here rather than by hand because a checkpoint that lags its log
# is worse than no checkpoint: it is a claim about a head that has moved on. Same for the fixtures the
# demonstrations run against and for the approvals the five checks are shown on.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "— the fixtures the demonstrations run against"
python3 tools/make_invoice_dir.py /tmp/inv-publish >/dev/null

echo "— the approvals the five checks are demonstrated on"
python3 tools/make_approvals.py

echo "— the pages, from the artifacts"
python3 tools/make_demos.py

echo "— the checkpoint, the counter-signature and the witness's ledger"
python3 tools/make_witness.py

echo "done: pages, checkpoint and witness artifacts are all built from what is on disk now"
