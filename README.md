# latch

[![CI](https://github.com/TerryTsai/latch/actions/workflows/ci.yml/badge.svg)](https://github.com/TerryTsai/latch/actions/workflows/ci.yml)
[![standard-readme compliant](https://img.shields.io/badge/readme%20style-standard-brightgreen.svg)](https://github.com/RichardLitt/standard-readme)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

A WebAuthn-only auth server for one user.

You tap a passkey to log in; whatever sits in front of your services hits `/verify` to find out whether you did. There are no users to manage, no password fallback, and no recovery flow — if you lose every registered passkey, you delete the JSON file of public keys and register again. The whole program is small enough to audit in an afternoon, and the published binary is a single static file with no system dependencies.

<p align="center"><img src="docs/screenshot-dark.png" alt="latch sign-in page" width="240"></p>

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [Footprint](#footprint)
- [Build from source](#build-from-source)
- [Maintainer](#maintainer)
- [Contributing](#contributing)
- [License](#license)

## Install

Pre-built binaries on [releases](https://github.com/TerryTsai/latch/releases). Drop the binary wherever you keep binaries.

For a one-shot install on Linux with systemd:

```
curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | bash
```

## Usage

Config is read from environment variables:

```
LATCH_RP_ID            (required)  e.g. latch.example.com
LATCH_RP_ORIGIN        (required)  e.g. https://latch.example.com
LATCH_COOKIE_DOMAIN    (required)  e.g. example.com
LATCH_LISTEN           (optional)  default 127.0.0.1:8080
LATCH_CREDS_PATH       (optional)  default creds.json
```

Run the binary. `latch --check` validates the env without starting; `latch --version` prints the version.

On first visit, with no credentials yet on disk, the page is in register mode — your tap enrolls a passkey. From then on the same page is a login.

Endpoints:

- `GET /` — login page
- `GET /verify` — 200 if session valid, 401 otherwise
- `POST /begin` / `POST /complete` — WebAuthn ceremony
- `POST /logout` — clears session

Session cookie is `latch_s`, scoped to `LATCH_COOKIE_DOMAIN`.

To recover from losing every registered credential, delete the JSON file at `LATCH_CREDS_PATH` and restart. Next visit is in register mode again.

## Footprint

**5.7 MB** static binary, statically linked against musl libc with vendored OpenSSL. No `libc.so`, no `libssl.so`, no shared library dependencies of any kind. Drops onto any x86_64 Linux — glibc, musl/Alpine, distroless, scratch container — without a runtime.

Idle at **3.6 MiB RSS**, **0% CPU**, **~12 ms** cold start.

Under sustained synthetic load (100,000 sequential `/verify` checks at 200 concurrent connections), it holds **~3,200 requests per second** on a single core and RSS peaks around **12 MiB** — orders of magnitude more headroom than a single-user homelab will ever use.

None of this is the result of careful tuning. It's what's left when an auth server doesn't need to support multiple users, OAuth, theming, or a database.

## Build from source

```
git clone https://github.com/TerryTsai/latch && cd latch
cargo build --release
```

Rust 1.65+. The default build dynamically links system OpenSSL — produces a 1.2 MB binary that runs on the host. For a fully static binary with vendored OpenSSL (matching the published releases), add the musl target and the `vendored` feature:

```
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
cargo build --release --target x86_64-unknown-linux-musl --features vendored
```

Output: `target/x86_64-unknown-linux-musl/release/latch`, ~5.7 MB, zero shared library dependencies.

## Maintainer

[@TerryTsai](https://github.com/TerryTsai)

## Contributing

PRs welcome for bugs. Features that expand the surface area beyond one user, one login button, and one `/verify` endpoint are unlikely to land — that boundary is the design.

## License

[MIT](LICENSE) © Terry Tsai
