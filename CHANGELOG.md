# Changelog

## v0.6.0 — clig.dev alignment

This release is a hard break. The CLI surface is reorganized to follow
[clig.dev](https://clig.dev) conventions: noun-grouped subcommands,
sysexits-style exit codes, machine-readable output, signal-handled
shutdown. There are no aliases for the old commands.

### Command tree (was → now)

| Before                       | After                          |
|------------------------------|--------------------------------|
| `latch run`                  | `latch serve`                  |
| `latch init`                 | `latch config init`            |
| (new)                        | `latch config show`            |
| (new)                        | `latch config path`            |
| (new)                        | `latch check`                  |
| (new)                        | `latch passkeys list`          |
| (new)                        | `latch passkeys reset`         |
| `latch start`                | `latch service start`          |
| `latch stop`                 | `latch service stop`           |
| `latch restart`              | `latch service restart`        |
| `latch status`               | `latch service status`         |
| `latch uninstall`            | `latch service uninstall`      |
| `latch uninstall --purge`    | (removed — see below)          |
| `latch update`               | `latch update`                 |
| `latch -v`, `--version`, `-V` | `latch -V`, `--version`        |
| (new)                        | `latch -v`, `--verbose`        |

`latch service start` rewrites the systemd unit on every call, so existing
deployments transparently pick up the new `ExecStart=/usr/local/bin/latch
serve …` line on the first invocation after upgrading.

### Configuration renames

| Before          | After       | Env (new)              |
|-----------------|-------------|------------------------|
| `rp_id`         | `hostname`  | `LATCH_HOSTNAME`       |
| `rp_origin`     | `origin`    | `LATCH_ORIGIN`         |
| `cookie_domain` | (unchanged) | `LATCH_COOKIE_DOMAIN`  |
| `listen`        | (unchanged) | `LATCH_LISTEN`         |
| `state_dir`     | `data_dir`  | `LATCH_DATA_DIR`       |

The on-disk file `creds.json` is renamed to `passkeys.json`.

The user-mode default data directory moves from
`~/.local/state/latch` (XDG_STATE_HOME) to `~/.local/share/latch`
(XDG_DATA_HOME). Migration:

```
mv ~/.local/state/latch ~/.local/share/latch
mv ~/.local/share/latch/creds.json ~/.local/share/latch/passkeys.json
```

`latch check` will detect the legacy path and print this command
when the new path is empty.

### Behavior changes

- `cookie_domain` derivation no longer errors on apex domains or
  bare hostnames. `example.com` and `localhost` derive to themselves.
- `--purge` is gone from `service uninstall`. The command removes the
  systemd unit and prints exact `rm` invocations for the data dir,
  config, and binary. Each destructive step is run explicitly.
- `latch serve` traps SIGTERM and SIGINT, drains within ~250 ms,
  exits 0.
- `--` is recognized as end-of-flags.
- Output discipline: data on stdout, diagnostics on stderr. `--json`
  forces machine output; non-TTY stdout switches to JSON automatically
  for `check`, `passkeys list`, and `config show`.
- Color (`✓`/`✗` in `latch check`) is enabled only when stdout is a
  TTY, `NO_COLOR` is unset, and `--no-color` is not passed.

### Exit codes (sysexits.h)

| Code | Meaning              | Example                                       |
|------|----------------------|-----------------------------------------------|
| 0    | success              |                                               |
| 1    | generic runtime fail |                                               |
| 64   | EX_USAGE             | unknown subcommand, `passkeys reset` no TTY   |
| 73   | EX_CANTCREAT         | cannot write config or data file              |
| 78   | EX_CONFIG            | `latch check` failed, env-only `service start` |

### Migration for existing v0.5 installs

```
# stop the running service so it doesn't fight you mid-upgrade
latch stop                            # use the v0.5 binary

latch update                          # picks up v0.6
mv ~/.local/state/latch ~/.local/share/latch         # user mode only
mv ~/.local/share/latch/creds.json ~/.local/share/latch/passkeys.json

# rename TOML keys (rp_id → hostname, rp_origin → origin, state_dir → data_dir)
$EDITOR ~/.config/latch/config.toml   # or /etc/latch/config.toml

latch service start                   # rewrites the systemd unit
latch check                           # confirm
```
