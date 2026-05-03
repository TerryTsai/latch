#!/usr/bin/env bash
# install.sh — drops the latch binary into place. After this, run `latch init`.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | bash
#
# Env overrides:
#   LATCH_REPO    owner/repo on GitHub (default: TerryTsai/latch)
#   LATCH_PREFIX  install prefix (default: /usr/local/bin)

set -euo pipefail

REPO="${LATCH_REPO:-TerryTsai/latch}"
PREFIX="${LATCH_PREFIX:-/usr/local/bin}"

ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

if [[ $EUID -ne 0 ]]; then
    echo "elevating with sudo..."
    exec sudo -E bash "$0" "$@"
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4)
if [[ -z "$TAG" ]]; then
    echo "could not determine latest release tag" >&2; exit 1
fi

URL="https://github.com/$REPO/releases/download/$TAG/latch-$TARGET.tar.gz"
echo "downloading $URL"
curl -fsSL "$URL" -o "$TMPDIR/latch.tar.gz"

# Verify against published sha256.
SHA_URL="$URL.sha256"
if curl -fsSL "$SHA_URL" -o "$TMPDIR/latch.tar.gz.sha256" 2>/dev/null; then
    EXPECTED=$(awk '{print $1}' "$TMPDIR/latch.tar.gz.sha256")
    ACTUAL=$(sha256sum "$TMPDIR/latch.tar.gz" | awk '{print $1}')
    if [[ "$EXPECTED" != "$ACTUAL" ]]; then
        echo "sha256 mismatch: expected $EXPECTED, got $ACTUAL" >&2
        exit 1
    fi
    echo "sha256 verified"
fi

tar xzf "$TMPDIR/latch.tar.gz" -C "$TMPDIR"
install -m755 "$TMPDIR/latch" "$PREFIX/latch"

echo
echo "latch $TAG installed at $PREFIX/latch"
echo "next:"
echo "  sudo latch init     # create config"
echo "  sudo latch start    # install systemd unit and start"
