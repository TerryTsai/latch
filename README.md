# latch

[![CI](https://github.com/TerryTsai/latch/actions/workflows/ci.yml/badge.svg)](https://github.com/TerryTsai/latch/actions/workflows/ci.yml)
[![standard-readme compliant](https://img.shields.io/badge/readme%20style-standard-brightgreen.svg)](https://github.com/RichardLitt/standard-readme)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

A WebAuthn-only auth server for one user.

You tap a passkey to log in; whatever sits in front of your services hits `/verify` to find out whether you did. Self-managing static binary — `latch init`, `latch start`, `latch update`. Single configuration file, single state directory, single user. No password fallback, no recovery flow — if you lose every registered passkey, you delete the credential file and register again.

<p align="center"><img src="docs/screenshot-dark.png" alt="latch sign-in page" width="240"></p>

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [Configuration](#configuration)
- [Footprint](#footprint)
- [Build from source](#build-from-source)
- [Maintainer](#maintainer)
- [Contributing](#contributing)
- [License](#license)

## Install

```
curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | bash
```

This drops the latch binary at `/usr/local/bin/latch`, verifies its sha256 against the GitHub release, and exits. From here, every operational command is invoked on the binary itself.

## Usage

```
sudo latch init     # write /etc/latch/config.toml (interactive)
sudo latch start    # create system user, install systemd unit, start
sudo latch status   # active / inactive
sudo latch update   # pull and install the latest release
sudo latch stop     # stop service
sudo latch restart  # restart service
sudo latch uninstall          # remove systemd unit + binary
sudo latch uninstall --purge  # also delete config + state directory
latch run --config <path>     # run in the foreground (for testing or non-systemd hosts)
```

`latch init` asks for one value (the hostname latch will be reachable at) and derives the rest. `latch start` creates the `latch` system user and `/var/lib/latch/` if absent, writes a hardened systemd unit, then enables and starts the service. `latch update` downloads the latest release, verifies sha256, atomically replaces the running binary, and restarts the systemd service.

The HTTP contract is a fixed five-endpoint surface that doesn't change between releases:

- `GET /` — login page
- `GET /verify` — 200 if session valid, 401 otherwise
- `POST /begin` / `POST /complete` — WebAuthn ceremony
- `POST /logout` — clears session

The session cookie is `latch_s`, scoped to your `cookie_domain`. Sessions are signed JWTs (HS256) — they survive process restart and a logout adds the JTI to a small persistent denylist.

On first visit, with no credentials yet on disk, the page is in register mode — your tap enrolls a passkey. From then on the same page is a login.

To recover from losing every registered credential, delete the JSON file at `<state_dir>/creds.json` and restart. The next visit is in register mode again.

## Configuration

`/etc/latch/config.toml`:

```toml
# REQUIRED. Hostname where latch is reachable.
rp_id = "latch.example.com"

# Optional overrides. Defaults shown commented.
# rp_origin     = "https://latch.example.com"
# cookie_domain = "example.com"
# listen        = "127.0.0.1:8080"
# state_dir     = "/var/lib/latch"
```

`rp_origin` defaults to `https://${rp_id}`. `cookie_domain` defaults to `rp_id` with the leading label stripped (so `latch.example.com` → `example.com`). `state_dir` holds three files latch manages: `creds.json` (registered public keys), `key` (HS256 signing key, mode 0600), and `revoked.json` (logout denylist).

Config search order (first match wins):

1. `--config <path>` flag
2. `$LATCH_CONFIG` env var
3. `./latch.toml` (current directory)
4. `/etc/latch/config.toml`

## Footprint

**5.7 MB** static binary, statically linked against musl libc with vendored OpenSSL. No shared library dependencies. Drops onto any x86_64 Linux — glibc, musl/Alpine, distroless, scratch container — without a runtime.

Idle at **3.6 MiB RSS**, **0% CPU**, **~12 ms** cold start.

Under sustained synthetic load (100,000 sequential `/verify` checks at 200 concurrent connections), it holds **~3,200 requests per second** on a single core and RSS peaks around **12 MiB** — orders of magnitude more headroom than a single-user homelab will ever use.

## Build from source

```
git clone https://github.com/TerryTsai/latch && cd latch
cargo build --release
```

Rust 1.65+. The default build dynamically links system OpenSSL and produces a ~1.5 MB binary that runs on the host. For the fully static binary that matches the published release:

```
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
cargo build --release --target x86_64-unknown-linux-musl --features vendored
```

Output: `target/x86_64-unknown-linux-musl/release/latch`.

## Maintainer

[@TerryTsai](https://github.com/TerryTsai)

## Contributing

PRs welcome for bugs and refinements. Features that expand the surface area beyond what `latch --help` already lists are unlikely to land — that boundary is the design.

## License

[MIT](LICENSE) © Terry Tsai
