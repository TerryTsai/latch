#!/usr/bin/env bash
# install.sh — drops the latch binary into place. After this, run `latch init`.
#
# Default: installs to ~/.local/bin (no sudo). Run with sudo for a system-wide
# install at /usr/local/bin.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | sudo bash
#
# Env overrides:
#   LATCH_REPO    owner/repo on GitHub (default: TerryTsai/latch)
#   LATCH_PREFIX  install prefix (default: ~/.local/bin or /usr/local/bin)

set -euo pipefail

REPO="${LATCH_REPO:-TerryTsai/latch}"

if [[ $EUID -eq 0 ]]; then
    PREFIX="${LATCH_PREFIX:-/usr/local/bin}"
    MODE="system"
else
    PREFIX="${LATCH_PREFIX:-$HOME/.local/bin}"
    MODE="user"
    mkdir -p "$PREFIX"
fi

ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

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
echo "latch $TAG installed at $PREFIX/latch ($MODE mode)"

# PATH hint — only if user-mode and prefix isn't already on PATH.
if [[ "$MODE" == "user" ]]; then
    case ":$PATH:" in
        *":$PREFIX:"*) ;;
        *)
            echo
            echo "$PREFIX is not on your PATH. Add this to your shell rc (~/.bashrc, ~/.zshrc):"
            echo "    export PATH=\"$PREFIX:\$PATH\""
            ;;
    esac
fi

echo
echo "next:"
if [[ "$MODE" == "system" ]]; then
    echo "    sudo latch init     # write /etc/latch/config.toml"
    echo "    sudo latch start    # install systemd unit and start"
    echo
    echo "for a per-user install instead, re-run without sudo."
else
    echo "    latch init     # write ~/.config/latch/config.toml"
    echo "    latch start    # install --user systemd unit and start"
    echo
    echo "for a system-wide install instead, re-run with sudo."
fi
