#!/usr/bin/env bash
# Install the mint binary from a release, checking it against the published checksums.
#
# The plugin itself downloads nothing: this is a script a person runs, and it prints the address it
# fetches from and the hash it expects before it installs anything. If the download does not match its
# checksum, nothing is installed.
#
#   ./install.sh                          the latest release, into ~/.local/bin
#   MINT_VERSION=v0.1.0 ./install.sh      a specific release
#   MINT_INSTALL_DIR=/usr/local/bin ./install.sh
set -euo pipefail

REPO="${MINT_REPO:-dmitrypehovski/mint}"
TAG="${MINT_VERSION:-}"
DEST="${MINT_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="linux" ;;
  Darwin) os="darwin" ;;
  *) echo "unsupported system: $(uname -s). Build from source instead: cargo build --release" >&2; exit 3 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="arm64" ;;
  *) echo "unsupported architecture: $(uname -m). Build from source instead: cargo build --release" >&2; exit 3 ;;
esac
target="$os-$arch"

if [ -z "$TAG" ] && [ -n "${MINT_RELEASE_BASE:-}" ]; then
  echo "with MINT_RELEASE_BASE set, say which release: MINT_VERSION=v0.1.0" >&2
  exit 3
fi
if [ -z "$TAG" ]; then
  TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" | sed 's#.*/tag/##')"
  if [ -z "$TAG" ]; then echo "could not work out the latest release of $REPO" >&2; exit 1; fi
fi
# A mirror, for a machine that cannot reach GitHub, or for testing the refusal below without breaking
# anything real.
base="${MINT_RELEASE_BASE:-https://github.com/$REPO/releases/download/$TAG}"
echo "  mint $TAG for $target, into $DEST"
echo "  from $base"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if ! curl -fsSL -o "$tmp/mint-$target.tar.gz" "$base/mint-$target.tar.gz"; then
  echo "could not fetch mint-$target.tar.gz from $base — nothing was installed" >&2
  exit 1
fi
if ! curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"; then
  echo "this release publishes no checksums, so there is nothing to check the download against" >&2
  echo "— refusing to install" >&2
  exit 1
fi

expected="$(awk -v name="mint-$target.tar.gz" '$2 == name { print $1 }' "$tmp/SHA256SUMS")"
if [ -z "$expected" ]; then
  echo "the release publishes no checksum for $target — refusing to install" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/mint-$target.tar.gz" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/mint-$target.tar.gz" | awk '{print $1}')"
fi
if [ "$actual" != "$expected" ]; then
  echo "the download does not match its checksum — refusing to install" >&2
  echo "  expected $expected" >&2
  echo "  got      $actual" >&2
  exit 1
fi
echo "  checksum $actual matches"

tar -xzf "$tmp/mint-$target.tar.gz" -C "$tmp"
mkdir -p "$DEST"
install -m 0755 "$tmp/mint" "$DEST/mint"
echo "  installed $("$DEST/mint" --version)"
case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "  note: $DEST is not on your PATH — add it, or set MINT_BIN=$DEST/mint" ;;
esac
