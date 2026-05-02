#!/usr/bin/env bash
# install.sh — bootstrap latch on a fresh box.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | bash
#   curl -fsSL ... | bash -s -- --rp-id=latch.foo.com --cookie-domain=foo.com
#
# Env overrides:
#   LATCH_REPO    owner/repo on GitHub (default: TerryTsai/latch)
#   LATCH_PREFIX  install prefix (default: /usr/local)

set -euo pipefail

REPO="${LATCH_REPO:-TerryTsai/latch}"
PREFIX="${LATCH_PREFIX:-/usr/local}"

RP_ID=""
RP_ORIGIN=""
COOKIE_DOMAIN=""

usage() {
    cat <<EOF
install.sh — bootstrap latch

Usage: install.sh [--rp-id=HOST] [--rp-origin=URL] [--cookie-domain=DOMAIN]

Downloads a binary from the latest GitHub release if available, otherwise
builds from source (requires cargo). Sets up the latch user, /var/lib/latch,
the systemd unit, and /etc/latch/env. Without flags, prompts interactively.
EOF
}

for arg in "$@"; do
    case "$arg" in
        --rp-id=*)         RP_ID="${arg#*=}" ;;
        --rp-origin=*)     RP_ORIGIN="${arg#*=}" ;;
        --cookie-domain=*) COOKIE_DOMAIN="${arg#*=}" ;;
        --help|-h)         usage; exit 0 ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

if [[ $EUID -ne 0 ]]; then
    echo "elevating with sudo..."
    exec sudo -E bash "$0" "$@"
fi

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
ok()  { printf '   \033[32mok\033[0m %s\n' "$*"; }

# --- prompts (only for missing values) -----------------------------------

if [[ -z "$RP_ID" ]]; then
    read -r -p "RP ID (e.g. latch.example.com): " RP_ID
fi
if [[ -z "$RP_ORIGIN" ]]; then
    read -r -p "RP origin [https://$RP_ID]: " input || true
    RP_ORIGIN="${input:-https://$RP_ID}"
fi
if [[ -z "$COOKIE_DOMAIN" ]]; then
    DEFAULT="${RP_ID#*.}"
    read -r -p "Cookie domain [$DEFAULT]: " input || true
    COOKIE_DOMAIN="${input:-$DEFAULT}"
fi

# --- fetch binary or build from source -----------------------------------

ARCH=$(uname -m)
case "$ARCH" in
    # Static musl binaries — run on any glibc or musl Linux of the same arch.
    x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"

say "binary"
TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
    | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4 || true)

GOT_BINARY=0
if [[ -n "$TAG" ]]; then
    URL="https://github.com/$REPO/releases/download/$TAG/latch-$TARGET.tar.gz"
    if curl -fsSL "$URL" -o latch.tar.gz 2>/dev/null; then
        tar xf latch.tar.gz
        GOT_BINARY=1
        ok "downloaded latch $TAG ($TARGET)"
    fi
fi

if [[ $GOT_BINARY -eq 0 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "no release available and cargo not found; install Rust first" >&2
        exit 1
    fi
    git clone --depth 1 "https://github.com/$REPO.git" src
    ( cd src && cargo build --release )
    cp src/target/release/latch ./latch
    ok "built from source"
fi

chmod +x latch

# --- install ------------------------------------------------------------

say "user + dirs"
if ! getent passwd latch >/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin \
            --home-dir /var/lib/latch latch
    ok "user 'latch' created"
fi
install -d -o latch -g latch -m 700 /var/lib/latch
install -d -m 755 /etc/latch
ok "/etc/latch and /var/lib/latch ready"

say "binary"
install -m755 latch "$PREFIX/bin/latch"
ok "$PREFIX/bin/latch"

say "env file"
cat > /etc/latch/env <<EOF
LATCH_RP_ID=$RP_ID
LATCH_RP_ORIGIN=$RP_ORIGIN
LATCH_COOKIE_DOMAIN=$COOKIE_DOMAIN
LATCH_LISTEN=127.0.0.1:8080
LATCH_CREDS_PATH=/var/lib/latch/creds.json
EOF
chmod 644 /etc/latch/env
ok "/etc/latch/env"

say "systemd unit"
cat > /etc/systemd/system/latch.service <<'EOF'
[Unit]
Description=latch — single-user passkey-based auth
After=network.target
Wants=network-online.target

[Service]
Type=simple
ExecStartPre=/usr/local/bin/latch --check
ExecStart=/usr/local/bin/latch
EnvironmentFile=/etc/latch/env
WorkingDirectory=/var/lib/latch
User=latch
Group=latch
Restart=on-failure
RestartSec=2

NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/latch
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictNamespaces=true
RestrictRealtime=true
LockPersonality=true

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now latch
sleep 0.5
if systemctl is-active --quiet latch; then
    ok "latch.service active"
else
    systemctl status latch --no-pager
    exit 1
fi

cat <<EOF

Done.

  RP_ID         = $RP_ID
  RP_ORIGIN     = $RP_ORIGIN
  COOKIE_DOMAIN = $COOKIE_DOMAIN

Next:
  1. Wire your reverse proxy at $RP_ID → 127.0.0.1:8080
  2. Visit $RP_ORIGIN from an Apple device to enroll

Useful commands:
  journalctl -u latch -f          live logs
  $PREFIX/bin/latch --check       validate config
  sudoedit /etc/latch/env         edit config
  systemctl restart latch         apply changes
EOF
