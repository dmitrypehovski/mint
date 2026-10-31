#!/usr/bin/env bash
# The browser the runtime drives.
#
# Under pm2 on purpose: started by hand it did not survive a reboot, and every run needs it. The
# profile lives in /root/.cache, not in the Hermes scratch directory, because that one is pruned
# when idle and a browser profile disappearing under a running Chrome is not a thing to debug live.
set -uo pipefail

CHROME=/root/.cache/ms-playwright/chromium-1243/chrome-linux64/chrome
PROFILE=/root/.cache/levos-profile
PORT=8830

mkdir -p "$PROFILE"
exec "$CHROME" \
  --headless=new \
  --no-sandbox \
  --disable-gpu \
  --disable-dev-shm-usage \
  --hide-scrollbars \
  --remote-debugging-port="$PORT" \
  --user-data-dir="$PROFILE" \
  --window-size=1440,900 \
  --noerrdialogs \
  --no-first-run \
  --ozone-platform=headless \
  --ozone-override-screen-size=800,600 \
  --use-angle=swiftshader-webgl \
  about:blank
