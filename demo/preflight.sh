#!/usr/bin/env bash
# Pre-flight check of the showcase. Run it before going on stage.
# Checks every part on its own and says plainly what is wrong.
set -uo pipefail
cd /root/mint

ok()   { printf "  \033[32m✓\033[0m %s\n" "$1"; }
bad()  { printf "  \033[31m✗\033[0m %s\n" "$1"; }
warn() { printf "  \033[33m!\033[0m %s\n" "$1"; }

echo "== 1. the browser we drive =="
if curl -s -m 5 -o /dev/null http://127.0.0.1:8830/json/version; then
  ok "Chromium answers on 8830"
else
  bad "Chromium does not answer on 8830 — raise it:"
  echo "     /root/.cache/ms-playwright/chromium-1243/chrome-linux64/chrome \\"
  echo "       --headless=new --no-sandbox --disable-gpu --remote-debugging-port=8830 \\"
  echo "       --user-data-dir=/root/.hermes/profiles/work2/cache/scratch/mint-profile about:blank"
fi

echo "== 2. the showcase =="
if curl -s -m 5 -o /dev/null http://127.0.0.1:8840/; then ok "the showcase is listening on 8840"
else
  bad "the showcase is down — starting it"
  (cd demo && nohup python3 relay.py > relay.log 2>&1 &) ; sleep 2
  curl -s -m 5 -o /dev/null http://127.0.0.1:8840/ && ok "it came up"
fi

echo "== 3. the public address =="
# Served by nginx on this box: mint.arliwork.com proxies to 127.0.0.1:8840, TLS from letsencrypt.
# There is no tunnel in this path any more, and no address that changes when something restarts.
URL=$(cat demo/public-url.txt 2>/dev/null || echo "https://mint.arliwork.com")
CODE=$(curl -s -m 20 -o /dev/null -w "%{http_code}" "$URL/site" || echo 000)
if [ "$CODE" = "200" ]; then ok "answers from outside: $URL"
else
  bad "$URL/site returned $CODE — check: systemctl status nginx | nginx -t | curl -sI http://127.0.0.1:8840/site"
fi

echo "== 4. the skills are present =="
for f in gh-commit gh-issues gh-prs; do
  if [ -f "$f.skill.json" ]; then
    S=$(python3 -c "import json;d=json.load(open('$f.skill.json'));print(f\"{len(d['steps'])} step(s), verified: {d.get('learn_declared_done')}\")")
    ok "$f.skill.json — $S"
  else bad "$f.skill.json is missing"; fi
done

echo "== 5. the task chain =="
python3 -c "
import json
j=json.load(open('demo/jobs.json'))
missing=[x['skill'] for x in j if not __import__('os').path.exists(x['skill'])]
print(f'  tasks: {len(j)}' + (' - every file is present' if not missing else f' - MISSING: {missing}'))
"

echo "== 6. a dry run (one task, ~20 s) =="
if timeout 200 ./target/release/mint demo --skill gh-commit.skill.json \
     --start "https://github.com/rust-lang/rust" \
     --push http://127.0.0.1:8840 --max-steps 2 2>&1 | grep -q "THE NUMBER FOR THE STAGE"; then
  ok "the run went through"
  curl -s -m 10 http://127.0.0.1:8840/state | python3 -c "
import json,sys
d=json.load(sys.stdin); f=d['sides']['fast']; s=d['sides']['slow']
print(f\"     on the showcase: {len(s['steps'])} decisions against {len(f['steps'])}, model calls {s['model_calls']} against {f['model_calls']}\")
"
else
  bad "the run did not go through — see the output above"
fi

echo
echo "READY. Open on stage: ${URL:-<no address yet>}"
echo "The fallback if the network fails: demo/race.mp4"
