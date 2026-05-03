// latch — single-user passkey auth.
//
// `latch --help` lists the lifecycle commands. The HTTP daemon lives in
// server.rs; this file is just CLI parsing and dispatch.

mod config;
mod jwt;
mod lifecycle;
mod server;
mod state;
mod update;

use std::env;
use std::path::PathBuf;

use crate::lifecycle::InitOpts;

const VERSION: &str = env!("CARGO_PKG_VERSION");

enum Cmd {
    Init      (InitOpts),
    Run       { config: Option<PathBuf> },
    Start     { config: Option<PathBuf> },
    Stop,
    Restart,
    Status,
    Update,
    Uninstall { purge: bool },
    Version,
    Help,
}

fn main() {
    let cmd = match parse() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("try `latch --help`");
            std::process::exit(2);
        }
    };

    let r = match cmd {
        Cmd::Init      (opts)     => lifecycle::init(opts),
        Cmd::Run       { config } => run_server(config),
        Cmd::Start     { config } => lifecycle::start(config.as_deref()),
        Cmd::Stop                 => lifecycle::stop(),
        Cmd::Restart              => lifecycle::restart(),
        Cmd::Status               => lifecycle::status(),
        Cmd::Update               => update::run(),
        Cmd::Uninstall { purge }  => lifecycle::uninstall(purge),
        Cmd::Version              => { println!("latch {VERSION}"); Ok(()) }
        Cmd::Help                 => { print_help(); Ok(()) }
    };

    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run_server(config_path: Option<PathBuf>) -> Result<(), String> {
    let cfg = config::Config::load(config_path.as_deref())?;
    cfg.print();
    server::run(cfg)
}

// --- CLI parsing -----------------------------------------------------------

fn parse() -> Result<Cmd, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        return Ok(Cmd::Help);
    }
    let head = args[0].as_str();
    let rest: &[String] = &args[1..];

    match head {
        "init"      => parse_init(rest),
        "run"       => parse_with_config(rest, "run").map(|c| Cmd::Run { config: c }),
        "start"     => parse_with_config(rest, "start").map(|c| Cmd::Start { config: c }),
        "stop"      => no_args(rest, "stop").map(|_| Cmd::Stop),
        "restart"   => no_args(rest, "restart").map(|_| Cmd::Restart),
        "status"    => no_args(rest, "status").map(|_| Cmd::Status),
        "update"    => no_args(rest, "update").map(|_| Cmd::Update),
        "uninstall" => parse_uninstall(rest),
        "--version" | "-v" | "version" => Ok(Cmd::Version),
        "--help"    | "-h" | "help"    => Ok(Cmd::Help),
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_init(args: &[String]) -> Result<Cmd, String> {
    let mut o = InitOpts::default();
    for a in args {
        if      let Some(v) = a.strip_prefix("--rp-id=")         { o.rp_id         = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--rp-origin=")     { o.rp_origin     = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--cookie-domain=") { o.cookie_domain = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--listen=")        { o.listen        = Some(v.into()); }
        else if let Some(v) = a.strip_prefix("--state-dir=")     { o.state_dir     = Some(PathBuf::from(v)); }
        else if let Some(v) = a.strip_prefix("--path=")          { o.path          = Some(PathBuf::from(v)); }
        else if a == "--print"           { o.print = true; }
        else if a == "--yes" || a == "-y" { o.yes  = true; }
        else if a == "--help" { return Err(init_help()); }
        else { return Err(format!("init: unknown flag `{a}`")); }
    }
    Ok(Cmd::Init(o))
}

fn parse_with_config(args: &[String], name: &str) -> Result<Option<PathBuf>, String> {
    let mut config = None;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--config" || a == "-c" {
            let next = iter.next().ok_or("--config requires a path")?;
            config = Some(PathBuf::from(next));
        } else if let Some(v) = a.strip_prefix("--config=") {
            config = Some(PathBuf::from(v));
        } else {
            return Err(format!("{name}: unknown flag `{a}`"));
        }
    }
    Ok(config)
}

fn parse_uninstall(args: &[String]) -> Result<Cmd, String> {
    let mut purge = false;
    for a in args {
        if a == "--purge" { purge = true; }
        else { return Err(format!("uninstall: unknown flag `{a}`")); }
    }
    Ok(Cmd::Uninstall { purge })
}

fn no_args(args: &[String], name: &str) -> Result<(), String> {
    if !args.is_empty() {
        return Err(format!("{name}: takes no arguments"));
    }
    Ok(())
}

fn init_help() -> String {
    "usage: latch init [--rp-id=HOST] [--rp-origin=URL] [--cookie-domain=DOMAIN] \
     [--listen=ADDR] [--state-dir=PATH] [--path=FILE] [--print] [--yes]".into()
}

fn print_help() {
    println!("latch {VERSION} — single-user passkey auth\n");
    println!("USAGE:");
    println!("  latch <command> [options]");
    println!();
    println!("COMMANDS:");
    println!("  init        write a new config file (interactive)");
    println!("  run         run the daemon in the foreground");
    println!("  start       install systemd unit and start in the background");
    println!("  stop        stop the systemd service");
    println!("  restart     restart the systemd service");
    println!("  status      show whether the service is running");
    println!("  update      download + install the latest release");
    println!("  uninstall   remove the systemd unit (--purge to also delete data)");
    println!();
    println!("FLAGS:");
    println!("  --version, -v   print version and exit");
    println!("  --help,    -h   this message");
    println!();
    println!("MODES:");
    println!("  Auto-detected from euid. Run as root for system-wide install,");
    println!("  otherwise everything lives under your home directory.");
    println!();
    println!("    system  /etc/latch/config.toml, /var/lib/latch, system systemd unit");
    println!("    user    ~/.config/latch/config.toml, ~/.local/state/latch, --user systemd unit");
    println!();
    println!("CONFIG:");
    println!("  TOML file is canonical; env vars override individual fields.");
    println!("  Env: LATCH_RP_ID, LATCH_RP_ORIGIN, LATCH_COOKIE_DOMAIN,");
    println!("       LATCH_LISTEN, LATCH_STATE_DIR, LATCH_CONFIG (file path).");
    println!("  Search order: --config, $LATCH_CONFIG, ./latch.toml, mode-default.");
    println!("  With LATCH_RP_ID set, no file is required (container use case).");
}
