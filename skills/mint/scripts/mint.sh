#!/usr/bin/env bash
# Find the mint binary, or say plainly how to get one. This script downloads nothing.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -n "${MINT_BIN:-}" ] && [ -x "${MINT_BIN}" ]; then exec "${MINT_BIN}" "$@"; fi
if command -v mint >/dev/null 2>&1; then exec mint "$@"; fi
# The repository this plugin was installed from, if it has been built there.
for candidate in "$here/../../../target/release/mint" "$HOME/.cargo/bin/mint" "/usr/local/bin/mint"; do
  if [ -x "$candidate" ]; then exec "$candidate" "$@"; fi
done

cat >&2 <<'MSG'
mint is not installed where this plugin can find it. Nothing was run, and nothing was recorded.

Build it once (needs Rust, https://rustup.rs):
  git clone https://github.com/dmitrypehovski/mint && cd mint && cargo build --release
  export PATH="$PWD/target/release:$PATH"

Or point the plugin at a binary you already have:
  export MINT_BIN=/path/to/mint
MSG
exit 3
