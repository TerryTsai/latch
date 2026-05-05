# latch

[![CI](https://github.com/TerryTsai/latch/actions/workflows/ci.yml/badge.svg)](https://github.com/TerryTsai/latch/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

Passkey auth for one user.

Tap a passkey to get a session cookie. Your reverse proxy hits
`/verify` to check it's still valid.

<p align="center"><img src="docs/screenshot-dark.png" alt="latch sign-in page" width="240"></p>

## Quickstart

You need a hostname pointed at the box (`latch.example.com`) and a
reverse proxy in front of it. The hostname must serve HTTPS.

**1. Install**

```sh
curl -fsSL https://raw.githubusercontent.com/TerryTsai/latch/main/install.sh | bash
```

Drops `latch` into `~/.local/bin`. Pipe into `sudo bash` instead for a
system-wide install at `/usr/local/bin`.

**2. Configure and start**

```sh
latch config init                # asks for the hostname, derives the rest
latch service start              # writes a hardened systemd unit, starts it
latch check                      # confirm the config is valid
```

Visit `https://latch.example.com` and tap to register a passkey.

**3. Put it in front of your services**

For Caddy, define a `forward_auth` snippet once and import it on every
gated host:

```caddyfile
(gated) {
    forward_auth localhost:8080 {
        uri /verify
        copy_headers Cookie
        @unauth status 4xx
        handle_response @unauth {
            redir https://latch.example.com/login?return_to=https://{host}{uri} 302
        }
    }
}

grafana.example.com {
    import gated
    reverse_proxy localhost:3000
}
```

For nginx, use `auth_request /verify` against `http://127.0.0.1:8080`
with `proxy_set_header Cookie $http_cookie;`.

## Recovery

You lose your device. There are no recovery codes.

```sh
ssh box
latch passkeys reset
```

The next visit to the page is in register mode.

## /verify

Your reverse proxy hits `/verify` as a forward-auth subrequest:

| Response                    | Meaning            |
|-----------------------------|--------------------|
| `200` + `X-Forwarded-User`  | session valid      |
| `302` → `/login?return_to=` | browser unauthed   |
| `401` + JSON                | API client unauthed |

Session cookie is `latch_session`, scoped to `cookie_domain`. Signed
JWT (HS256). Logout adds the JTI to a persistent denylist.

`/`, `/begin`, `/complete`, and `/logout` are direct browser ↔ latch.
Don't proxy them.

## Configuration

The TOML file is the canonical schema; env vars override individual
fields. System mode default: `/etc/latch/config.toml`. User mode
default: `~/.config/latch/config.toml`.

```toml
# REQUIRED. Hostname where latch is reachable.
hostname = "latch.example.com"

# Optional. Defaults shown.
# origin        = "https://latch.example.com"
# cookie_domain = "example.com"
# listen        = "127.0.0.1:8080"
# data_dir      = "/var/lib/latch"          # system mode
# data_dir      = "~/.local/share/latch"    # user mode
```

`origin` defaults to `https://${hostname}`. `cookie_domain` strips the
leftmost label of the hostname (`latch.example.com` → `example.com`);
apex (`example.com`) and bare (`localhost`) hostnames scope to
themselves. `data_dir` holds three files latch manages: `passkeys.json`,
`key` (HS256 signing key, mode 0600), and `revoked.json`.

### Env vars

| TOML key        | Env var                |
|-----------------|------------------------|
| `hostname`      | `LATCH_HOSTNAME`       |
| `origin`        | `LATCH_ORIGIN`         |
| `cookie_domain` | `LATCH_COOKIE_DOMAIN`  |
| `listen`        | `LATCH_LISTEN`         |
| `data_dir`      | `LATCH_DATA_DIR`       |
| (file path)     | `LATCH_CONFIG`         |

With `LATCH_HOSTNAME` set, no config file is required — `latch serve`
synthesizes everything from env.

Search order for the config file: `--config`, `$LATCH_CONFIG`,
`./latch.toml`, mode default.

## Commands

`latch help` lists everything; `latch help <command>` shows examples.

| Command                   | Purpose                                              |
|---------------------------|------------------------------------------------------|
| `latch config init`       | write a config file (interactive)                    |
| `latch config show`       | print the resolved config (also `--json`)            |
| `latch config path`       | print the loaded config path                         |
| `latch check`             | validate config + state, exit 78 on problems         |
| `latch serve`             | run in the foreground                                |
| `latch service start`     | install systemd unit and start                       |
| `latch service status`    | active / inactive                                    |
| `latch service stop`      | stop                                                 |
| `latch service restart`   | restart                                              |
| `latch service uninstall` | remove unit; prints `rm` commands for the rest       |
| `latch passkeys list`     | list registered passkeys (also `--json`)             |
| `latch passkeys reset`    | delete all passkeys (prompts; `--yes` to skip)       |
| `latch update`            | fetch + verify + replace the binary, restart service |

Output goes to the right stream: data on stdout, diagnostics on stderr.
`--json` (or non-TTY stdout) gives machine output for `check`,
`passkeys list`, and `config show`. Color is suppressed when stdout
isn't a TTY, when `NO_COLOR` is set, or with `--no-color`. Exit codes
follow `sysexits.h` — `0` success, `64` usage, `73` cantcreat, `78`
config, `1` generic.

## Footprint

**1.5 MB** statically linked binary (musl + vendored OpenSSL). No shared
library dependencies; runs on glibc, musl/Alpine, distroless, scratch.

Idle at **3.6 MiB RSS**, **0% CPU**, **~12 ms** cold start. Under
sustained load (100k sequential `/verify` checks at 200 concurrent
connections) it holds **~3,200 req/s** on a single core with RSS
peaking around **12 MiB**.

## Build from source

```sh
git clone https://github.com/TerryTsai/latch && cd latch
cargo build --release
```

Rust 1.65+. The default build links system OpenSSL. For the fully
static binary that matches the published release:

```sh
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
cargo build --release --target x86_64-unknown-linux-musl --features vendored
```

Output: `target/x86_64-unknown-linux-musl/release/latch`.

## Contributing

PRs welcome for bugs and refinements. Features that expand the surface
beyond what `latch help` lists are unlikely to land.

## License

[MIT](LICENSE) © Terry Tsai
