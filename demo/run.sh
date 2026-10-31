#!/usr/bin/env bash
# Warm the showcase up before going on stage.
# Runs the demo so the panes and counters are filled, and checks that the
# public address answers from outside.
set -euo pipefail

cd /root/levos
SKILL="${1:-gh-commit.skill.json}"
START="${2:-https://github.com/rust-lang/rust}"

echo "== the public address =="
URL=$(cat demo/public-url.txt 2>/dev/null || echo "https://mint.arliwork.com")
echo "$URL"
curl -s -m 20 -o /dev/null -w "from outside: HTTP %{http_code}\n" "$URL/" || echo "no answer from outside"

echo "== the showcase, locally =="
curl -s -m 10 -o /dev/null -w "locally: HTTP %{http_code}\n" http://127.0.0.1:8840/ || {
  echo "the showcase is down — starting it"; (cd demo && nohup python3 relay.py > relay.log 2>&1 &) ; sleep 2; }

echo "== run: $SKILL on $START =="
./target/release/levos demo --skill "$SKILL" --start "$START" --push http://127.0.0.1:8840 --max-steps 4

echo
echo "Done. Open on stage: $URL"
