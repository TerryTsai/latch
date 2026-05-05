// latch — single-user passkey auth.
//
// `latch help` lists the command tree. The HTTP daemon lives in server.rs;
// this file is CLI parsing and dispatch. Every subcommand returns an
// output::Result so exit codes follow sysexits.h.

mod cmd;
mod config;
mod jwt;
mod output;
mod server;
mod state;

use std::env;
use std::path::PathBuf;

use crate::cmd::config::InitOpts;
use crate::output::{Error, EX_OK};

const VERSION: &str = env!("CARGO_PKG_VERSION");

// --- top-level dispatch ----------------------------------------------------

fn main() {
    // Reset SIGPIPE to default (terminate quietly) so `latch help | head`
    // doesn't panic on a broken stdout pipe. Rust installs SIG_IGN; we want
    // EPIPE to kill us cleanly with exit 141.
    // SAFETY: signal() is async-signal-safe; SIG_DFL has no body.
    unsafe { libc_signal(13 /* SIGPIPE */, 0 /* SIG_DFL */); }

    let args: Vec<String> = env::args().skip(1).collect();
    let code = match run(args) {
        Ok(()) => EX_OK,
        Err(e) => {
            e.print();
            e.code
        }
    };
    std::process::exit(code);
}

extern "C" {
    #[link_name = "signal"]
    fn libc_signal(signum: i32, handler: usize) -> usize;
}

fn run(args: Vec<String>) -> output::Result<()> {
    let mut globals = Globals::default();
    let positional = globals.consume(args)?;
    globals.apply();

    let mut it = positional.into_iter();
    let head = match it.next() {
        Some(h) => h,
        None    => { print_help(); return Ok(()); }
    };
    let rest: Vec<String> = it.collect();

    match head.as_str() {
        "serve"    => serve(rest, &globals),
        "check"    => check_cmd(rest, &globals),
        "config"   => dispatch_config(rest, &globals),
        "passkeys" => dispatch_passkeys(rest, &globals),
        "service"  => dispatch_service(rest, &globals),
        "update"   => no_args("update", &rest).and_then(|_| cmd::update::run()),
        "version"  => { println!("latch {VERSION}"); Ok(()) }
        "help"     => help_for(rest),
        other => Err(unknown_cmd(other)),
    }
}

// --- global flags ----------------------------------------------------------

#[derive(Default)]
struct Globals {
    config:    Option<PathBuf>,
    listen:    Option<String>,
    data_dir:  Option<PathBuf>,
    json:      bool,
    quiet:     bool,
    verbose:   bool,
    no_color:  bool,
    yes:       bool,
}

impl Globals {
    // Pull global flags out of the arg vector, return the positional rest.
    // Recognizes `--` as end-of-flags. Long-form `--flag` and `--flag=value`,
    // plus `-V`, `-v`, `-q`, `-y`, `-h`, `-c <path>`.
    fn consume(&mut self, args: Vec<String>) -> output::Result<Vec<String>> {
        let mut out = Vec::with_capacity(args.len());
        let mut it = args.into_iter();
        let mut end_of_flags = false;
        while let Some(a) = it.next() {
            if end_of_flags { out.push(a); continue; }
            match a.as_str() {
                "--" => { end_of_flags = true; }
                "-V" | "--version" => { println!("latch {VERSION}"); std::process::exit(EX_OK); }
                "-h" | "--help"    => { out.insert(0, "help".into()); end_of_flags = true; }
                "-q" | "--quiet"   => { self.quiet    = true; }
                "-v" | "--verbose" => { self.verbose  = true; }
                "-y" | "--yes"     => { self.yes      = true; }
                "--json"           => { self.json     = true; }
                "--no-color"       => { self.no_color = true; }
                "-c" | "--config"  => {
                    let v = it.next().ok_or_else(|| Error::usage("--config requires a path"))?;
                    self.config = Some(PathBuf::from(v));
                }
                "--listen" => {
                    let v = it.next().ok_or_else(|| Error::usage("--listen requires an addr"))?;
                    self.listen = Some(v);
                }
                "--data-dir" => {
                    let v = it.next().ok_or_else(|| Error::usage("--data-dir requires a path"))?;
                    self.data_dir = Some(PathBuf::from(v));
                }
                a if a.starts_with("--config=")   => self.config   = Some(PathBuf::from(&a[9..])),
                a if a.starts_with("--listen=")   => self.listen   = Some(a[9..].into()),
                a if a.starts_with("--data-dir=") => self.data_dir = Some(PathBuf::from(&a[11..])),
                _ => out.push(a),
            }
        }
        Ok(out)
    }

    fn apply(&self) {
        output::set_no_color(self.no_color);
        output::set_json(self.json);
        output::set_quiet(self.quiet);
        // Globals that override env happen at config-load time by setting
        // the env var so the existing precedence (file < env) works.
        if let Some(l) = &self.listen   { env::set_var("LATCH_LISTEN",   l); }
        if let Some(d) = &self.data_dir { env::set_var("LATCH_DATA_DIR", d); }
    }
}

// --- subcommand dispatch ---------------------------------------------------

fn serve(args: Vec<String>, g: &Globals) -> output::Result<()> {
    no_args("serve", &args)?;
    let cfg = config::Config::load(g.config.as_deref())
        .map_err(Error::config)?;
    cfg.print();
    server::run(cfg).map_err(Error::fail)
}

fn check_cmd(args: Vec<String>, g: &Globals) -> output::Result<()> {
    no_args("check", &args)?;
    cmd::check::run(g.config.as_deref())
}

fn dispatch_config(args: Vec<String>, g: &Globals) -> output::Result<()> {
    let mut it = args.into_iter();
    let leaf = it.next().ok_or_else(||
        Error::usage("missing subcommand: config init|show|path"))?;
    let rest: Vec<String> = it.collect();
    match leaf.as_str() {
        "init" => cmd::config::init(parse_init(rest, g)?),
        "show" => { no_args("config show", &rest)?; cmd::config::show(g.config.as_deref()) }
        "path" => { no_args("config path", &rest)?; cmd::config::path(g.config.as_deref()) }
        other  => Err(unknown_subcmd("config", other)),
    }
}

fn dispatch_passkeys(args: Vec<String>, g: &Globals) -> output::Result<()> {
    let mut it = args.into_iter();
    let leaf = it.next().ok_or_else(||
        Error::usage("missing subcommand: passkeys list|reset"))?;
    let rest: Vec<String> = it.collect();
    match leaf.as_str() {
        "list"  => { no_args("passkeys list",  &rest)?; cmd::passkeys::list (g.config.as_deref()) }
        "reset" => { no_args("passkeys reset", &rest)?; cmd::passkeys::reset(g.config.as_deref(), g.yes) }
        other   => Err(unknown_subcmd("passkeys", other)),
    }
}

fn dispatch_service(args: Vec<String>, g: &Globals) -> output::Result<()> {
    let mut it = args.into_iter();
    let leaf = it.next().ok_or_else(||
        Error::usage("missing subcommand: service start|stop|restart|status|uninstall"))?;
    let rest: Vec<String> = it.collect();
    match leaf.as_str() {
        "start"     => { no_args("service start",     &rest)?; cmd::service::start(g.config.as_deref()) }
        "stop"      => { no_args("service stop",      &rest)?; cmd::service::stop() }
        "restart"   => { no_args("service restart",   &rest)?; cmd::service::restart() }
        "status"    => { no_args("service status",    &rest)?; cmd::service::status() }
        "uninstall" => { no_args("service uninstall", &rest)?; cmd::service::uninstall() }
        other       => Err(unknown_subcmd("service", other)),
    }
}

// --- init parsing ----------------------------------------------------------

fn parse_init(args: Vec<String>, g: &Globals) -> output::Result<InitOpts> {
    let mut o = InitOpts { yes: g.yes, ..Default::default() };
    if let Some(d) = &g.data_dir { o.data_dir = Some(d.clone()); }
    if let Some(l) = &g.listen   { o.listen   = Some(l.clone()); }
    for a in args {
        if      let Some(v) = a.strip_prefix("--hostname=")      { o.hostname      = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--origin=")        { o.origin        = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--cookie-domain=") { o.cookie_domain = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--listen=")        { o.listen        = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--data-dir=")      { o.data_dir      = Some(PathBuf::from(v)); }
        else if let Some(v) = a.strip_prefix("--path=")          { o.path          = Some(PathBuf::from(v)); }
        else if a == "--print"            { o.print = true; }
        else if a == "--yes" || a == "-y" { o.yes   = true; }
        else { return Err(Error::usage(format!("config init: unknown flag `{a}`"))); }
    }
    Ok(o)
}

// --- helpers ---------------------------------------------------------------

fn no_args(name: &str, args: &[String]) -> output::Result<()> {
    if args.is_empty() { Ok(()) } else {
        Err(Error::usage(format!("{name}: takes no arguments (got `{}`)", args[0])))
    }
}

fn unknown_cmd(name: &str) -> Error {
    Error::usage(format!("unknown command `{name}`"))
        .with_hint("run `latch help` to list commands")
}

fn unknown_subcmd(group: &str, name: &str) -> Error {
    Error::usage(format!("unknown {group} subcommand `{name}`"))
        .with_hint(format!("run `latch help {group}` for the list"))
}

// --- help ------------------------------------------------------------------

fn help_for(args: Vec<String>) -> output::Result<()> {
    match args.first().map(String::as_str) {
        None             => { print_help();          Ok(()) }
        Some("serve")    => { println!("{}", help_serve());    Ok(()) }
        Some("check")    => { println!("{}", help_check());    Ok(()) }
        Some("config")   => { println!("{}", help_config());   Ok(()) }
        Some("passkeys") => { println!("{}", help_passkeys()); Ok(()) }
        Some("service")  => { println!("{}", help_service());  Ok(()) }
        Some("update")   => { println!("{}", help_update());   Ok(()) }
        Some(other)      => Err(unknown_cmd(other)),
    }
}

fn print_help() {
    println!("latch {VERSION} — single-user passkey auth\n");
    println!("USAGE:");
    println!("  latch <command> [subcommand] [options]\n");
    println!("COMMANDS:");
    println!("  serve              run the daemon in the foreground");
    println!("  check              validate config and state, then exit");
    println!("  config init        write a new config file");
    println!("  config show        print the resolved (file + env) config");
    println!("  config path        print the path of the loaded config");
    println!("  passkeys list      list registered passkeys");
    println!("  passkeys reset     delete all passkeys (prompts on TTY)");
    println!("  service start      install systemd unit and start in the background");
    println!("  service stop       stop the systemd service");
    println!("  service restart    restart the systemd service");
    println!("  service status     show whether the service is running");
    println!("  service uninstall  remove the systemd unit");
    println!("  update             download + install the latest release");
    println!("  version            print version (also -V, --version)");
    println!("  help [<command>]   per-command usage (also -h, --help)\n");
    println!("GLOBAL FLAGS:");
    println!("  --config <path>    config file (default: $XDG_CONFIG_HOME/latch/config.toml)");
    println!("  --listen <addr>    override LATCH_LISTEN");
    println!("  --data-dir <path>  override LATCH_DATA_DIR");
    println!("  --json             machine-readable output");
    println!("  -q, --quiet        diagnostics off");
    println!("  -v, --verbose      diagnostics on");
    println!("  --no-color         disable color (also honored: NO_COLOR env)");
    println!("  -y, --yes          skip confirmation prompts\n");
    println!("CONFIG:");
    println!("  TOML file is canonical; env vars override individual fields.");
    println!("  Env: LATCH_HOSTNAME, LATCH_ORIGIN, LATCH_COOKIE_DOMAIN,");
    println!("       LATCH_LISTEN, LATCH_DATA_DIR, LATCH_CONFIG.\n");
    println!("EXAMPLES:");
    println!("  latch check                                    # before deploy");
    println!("  latch serve                                    # foreground server");
    println!("  latch passkeys list --json | jq                # scripting");
    println!("  latch passkeys reset --yes                     # wipe credentials");
}

fn help_serve() -> &'static str {
"latch serve — run the daemon in the foreground.

USAGE:
  latch serve [--config PATH] [--listen ADDR] [--data-dir PATH]

Reads config from --config, $LATCH_CONFIG, ./latch.toml, or the default
($XDG_CONFIG_HOME/latch/config.toml or /etc/latch/config.toml).
Env vars override file values.

Traps SIGTERM/SIGINT and exits cleanly.

EXAMPLE:
  LATCH_HOSTNAME=latch.example.com latch serve --listen 127.0.0.1:9000"
}

fn help_check() -> &'static str {
"latch check — validate config and state, then exit.

USAGE:
  latch check [--config PATH] [--json]

Verifies that the config loads, origin is https://, hostname is not a
placeholder, data_dir is writable, and passkeys.json is readable.
Exits 78 on any problem.

EXAMPLE:
  latch check --json | jq .ok"
}

fn help_config() -> &'static str {
"latch config — manage the config file.

USAGE:
  latch config init  [--hostname HOST] [--origin URL] [--cookie-domain D]
                     [--listen ADDR] [--data-dir PATH] [--path FILE]
                     [--print] [--yes]
  latch config show  [--config PATH] [--json]
  latch config path  [--config PATH]

EXAMPLES:
  latch config init --hostname latch.example.com --yes
  latch config show --json | jq .hostname
  latch config path"
}

fn help_passkeys() -> &'static str {
"latch passkeys — manage registered passkeys.

USAGE:
  latch passkeys list  [--config PATH] [--json]
  latch passkeys reset [--config PATH] [--yes]

reset deletes the passkeys file. Prompts on TTY; requires --yes on
non-TTY (exit 64 otherwise).

EXAMPLES:
  latch passkeys list
  latch passkeys list --json | jq length
  latch passkeys reset --yes"
}

fn help_service() -> &'static str {
"latch service — manage the systemd unit.

USAGE:
  latch service start      install + enable + start (idempotent)
  latch service stop
  latch service restart
  latch service status
  latch service uninstall  remove the unit; prints copy-paste commands
                           for removing data, config, and binary

Run as root for system-wide install (/etc/latch, /var/lib/latch);
otherwise everything lives under your home directory.

EXAMPLE:
  sudo latch service start
  latch service status"
}

fn help_update() -> &'static str {
"latch update — download and install the latest release.

USAGE:
  latch update

Verifies the SHA-256 against the published checksum, replaces the
binary atomically, and restarts the systemd service if active.

EXAMPLE:
  sudo latch update"
}
