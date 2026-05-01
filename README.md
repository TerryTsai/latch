# latch

[![CI](https://github.com/TerryTsai/latch/actions/workflows/ci.yml/badge.svg)](https://github.com/TerryTsai/latch/actions/workflows/ci.yml)
[![standard-readme compliant](https://img.shields.io/badge/readme%20style-standard-brightgreen.svg)](https://github.com/RichardLitt/standard-readme)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

A WebAuthn-only auth server for one user.

You tap a passkey to log in; whatever sits in front of your services hits `/verify` to find out whether you did. There are no users to manage, no password fallback, and no recovery flow — if you lose every registered passkey, you delete the JSON file of public keys and register again. The whole program is under 500 lines of Rust, small enough to audit in an afternoon if that matters to you.

## Table of Contents

- [Install](#install)
- [Usage](#usage)
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

Endpoints:

- `GET /` — login page
- `GET /verify` — 200 if session valid, 401 otherwise
- `POST /begin` / `POST /complete` — WebAuthn ceremony
- `POST /logout` — clears session

Session cookie is `latch_s`, scoped to `LATCH_COOKIE_DOMAIN`.

To recover from losing every registered credential, delete the JSON file at `LATCH_CREDS_PATH` and restart. Next visit is in register mode again.

## Build from source

```
git clone https://github.com/TerryTsai/latch && cd latch
cargo build --release
```

Rust 1.65+. ~1.2 MB stripped.

## Maintainer

[@TerryTsai](https://github.com/TerryTsai)

## Contributing

PRs welcome for bugs. Features that expand the surface area beyond one user, one login button, and one `/verify` endpoint are unlikely to land — that boundary is the design.

## License

[MIT](LICENSE) © Terry Tsai
