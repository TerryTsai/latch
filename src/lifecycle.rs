// Self-management commands: init, start, stop, restart, status, uninstall.
// Auto-detects mode by euid: root → system, otherwise user. Each command
// branches on Mode for paths, systemctl scope, and unit rendering.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{self, Config, DEFAULT_LISTEN, Mode};

const UNIT_FILE: &str = "latch.service";
const SYSTEM_UNIT_PATH: &str = "/etc/systemd/system/latch.service";
const SYSTEM_USER: &str = "latch";

// --- init ------------------------------------------------------------------

pub fn init(rp_id: Option<String>, path: Option<PathBuf>, print: bool, yes: bool) -> Result<(), String> {
    let mode = Mode::detect();
    let target = path.unwrap_or_else(|| config::default_config_path(mode));

    let rp_id = match rp_id {
        Some(s) => s,
        None    => prompt("Hostname (e.g. latch.example.com): ")?,
    };
    let rp_id = rp_id.trim().to_string();
    if rp_id.is_empty() { return Err("rp_id is required".into()); }
    if rp_id.contains("example.com") {
        return Err("rp_id can't be a placeholder".into());
    }

    let rp_origin = format!("https://{rp_id}");
    let cookie_domain = config::derive_cookie_domain(&rp_id)?;
    let state_dir = config::default_state_dir(mode);

    let content = render_config(&rp_id, &rp_origin, &cookie_domain, &state_dir);

    if print {
        print!("{content}");
        return Ok(());
    }

    if !yes {
        eprintln!();
        eprintln!("  mode          = {}", mode.label());
        eprintln!("  rp_id         = {rp_id}");
        eprintln!("  rp_origin     = {rp_origin}   (derived)");
        eprintln!("  cookie_domain = {cookie_domain}   (derived)");
        eprintln!("  listen        = {DEFAULT_LISTEN}   (default)");
        eprintln!("  state_dir     = {}   (default)", state_dir.display());
        eprintln!();
        let answer = prompt(&format!("Write to {}? [Y/n] ", target.display()))?;
        if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
            return Err("aborted".into());
        }
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(&target, content).map_err(|e| format!("write {}: {e}", target.display()))?;

    eprintln!("Wrote {}", target.display());
    eprintln!("Next:");
    match mode {
        Mode::System => eprintln!("    sudo latch start"),
        Mode::User   => eprintln!("    latch start"),
    }
    Ok(())
}

fn render_config(rp_id: &str, rp_origin: &str, cookie_domain: &str, state_dir: &Path) -> String {
    format!(
        "# latch config\n\
         # https://github.com/TerryTsai/latch\n\
         \n\
         # REQUIRED. Hostname where latch is publicly reachable.\n\
         rp_id = \"{rp_id}\"\n\
         \n\
         # Optional overrides. Defaults shown commented.\n\
         # rp_origin     = \"{rp_origin}\"\n\
         # cookie_domain = \"{cookie_domain}\"\n\
         # listen        = \"{DEFAULT_LISTEN}\"\n\
         # state_dir     = \"{}\"\n",
        state_dir.display(),
    )
}

fn prompt(msg: &str) -> Result<String, String> {
    eprint!("{msg}");
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).map_err(|e| format!("stdin: {e}"))?;
    Ok(buf.trim().to_string())
}

// --- start / stop / restart / status --------------------------------------

pub fn start(config_path: Option<&Path>) -> Result<(), String> {
    let mode = Mode::detect();
    let cfg = Config::load(config_path)?;
    cfg.print();

    if mode == Mode::System {
        ensure_system_user()?;
    }
    ensure_state_dir(mode, &cfg.state_dir)?;
    write_unit(mode, &cfg)?;

    systemctl(mode, &["daemon-reload"])?;
    systemctl(mode, &["enable", "--now", UNIT_FILE])?;

    std::thread::sleep(std::time::Duration::from_millis(500));
    if !is_unit_active(mode) {
        return Err(format!(
            "started, but service is not active. Check: {} -u latch -n 50",
            journalctl_cmd(mode),
        ));
    }
    print_started(mode);
    Ok(())
}

pub fn stop() -> Result<(), String> {
    let mode = Mode::detect();
    systemctl(mode, &["stop", UNIT_FILE])?;
    eprintln!("latch stopped.");
    Ok(())
}

pub fn restart() -> Result<(), String> {
    let mode = Mode::detect();
    systemctl(mode, &["restart", UNIT_FILE])?;
    eprintln!("latch restarted.");
    Ok(())
}

pub fn status() -> Result<(), String> {
    let mode = Mode::detect();
    let active = is_unit_active(mode);
    let enabled = is_enabled(mode);
    println!("latch:  {}  ({})",
        if active { "active" } else { "inactive" },
        if enabled { "enabled" } else { "disabled" },
    );
    println!("mode:   {}", mode.label());
    let unit = unit_path(mode);
    if unit.exists() {
        println!("unit:   {}", unit.display());
    } else {
        let cmd = match mode { Mode::System => "sudo latch start", Mode::User => "latch start" };
        println!("unit:   not installed (run `{cmd}`)");
    }
    let cfg = config::default_config_path(mode);
    if cfg.exists() {
        println!("config: {}", cfg.display());
    }
    Ok(())
}

fn print_started(mode: Mode) {
    let journal = journalctl_cmd(mode);
    eprintln!();
    eprintln!("latch is running ({} mode).", mode.label());
    eprintln!();
    eprintln!("logs:");
    eprintln!("    {journal} -u latch -f");
    if mode == Mode::User {
        let user = std::env::var("USER").unwrap_or_else(|_| "$USER".into());
        eprintln!();
        eprintln!("to survive logout/reboot without an active session, run once:");
        eprintln!("    sudo loginctl enable-linger {user}");
    }
}

// --- uninstall -------------------------------------------------------------

pub fn uninstall(purge: bool) -> Result<(), String> {
    let mode = Mode::detect();

    let _ = systemctl(mode, &["stop",    UNIT_FILE]);
    let _ = systemctl(mode, &["disable", UNIT_FILE]);
    let _ = fs::remove_file(unit_path(mode));
    let _ = systemctl(mode, &["daemon-reload"]);
    eprintln!("removed {} systemd unit", mode.label());

    if purge {
        if let Ok(cfg) = Config::load(None) {
            if cfg.state_dir.exists() {
                let _ = fs::remove_dir_all(&cfg.state_dir);
                eprintln!("removed state dir: {}", cfg.state_dir.display());
            }
        }
        let cfg_path = config::default_config_path(mode);
        if cfg_path.exists() {
            let _ = fs::remove_file(&cfg_path);
            if let Some(parent) = cfg_path.parent() {
                let _ = fs::remove_dir(parent);  // empty-only
            }
            eprintln!("removed config");
        }
        if mode == Mode::System {
            let _ = remove_system_user();
        }
    }

    if let Ok(bin) = std::env::current_exe() {
        eprintln!();
        eprintln!("the binary itself is at {}", bin.display());
        eprintln!("remove with:");
        eprintln!("    {}rm {}",
            if mode == Mode::System { "sudo " } else { "" },
            bin.display(),
        );
    }
    Ok(())
}

// --- systemd glue ----------------------------------------------------------

fn systemctl(mode: Mode, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("systemctl");
    if mode == Mode::User { cmd.arg("--user"); }
    let status = cmd.args(args).status()
        .map_err(|e| format!("run systemctl: {e}"))?;
    if !status.success() {
        let scope = if mode == Mode::User { "--user " } else { "" };
        return Err(format!("systemctl {scope}{} failed (exit {})",
            args.join(" "),
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        ));
    }
    Ok(())
}

pub fn is_unit_active(mode: Mode) -> bool {
    let mut cmd = Command::new("systemctl");
    if mode == Mode::User { cmd.arg("--user"); }
    cmd.args(["is-active", "--quiet", UNIT_FILE])
        .status().map(|s| s.success()).unwrap_or(false)
}

fn is_enabled(mode: Mode) -> bool {
    let mut cmd = Command::new("systemctl");
    if mode == Mode::User { cmd.arg("--user"); }
    cmd.args(["is-enabled", "--quiet", UNIT_FILE])
        .status().map(|s| s.success()).unwrap_or(false)
}

fn journalctl_cmd(mode: Mode) -> &'static str {
    match mode { Mode::System => "journalctl", Mode::User => "journalctl --user" }
}

fn unit_path(mode: Mode) -> PathBuf {
    match mode {
        Mode::System => PathBuf::from(SYSTEM_UNIT_PATH),
        Mode::User   => config::xdg_config_home().join("systemd/user").join(UNIT_FILE),
    }
}

fn write_unit(mode: Mode, cfg: &Config) -> Result<(), String> {
    let bin = std::env::current_exe()
        .map_err(|e| format!("can't locate own binary: {e}"))?;
    let unit = render_unit(mode, &bin, &cfg.source, &cfg.state_dir);
    let path = unit_path(mode);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(&path, unit).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn render_unit(mode: Mode, bin: &Path, config: &Path, state_dir: &Path) -> String {
    match mode {
        Mode::System => render_system_unit(bin, config, state_dir),
        Mode::User   => render_user_unit(bin, config, state_dir),
    }
}

fn render_system_unit(bin: &Path, config: &Path, state_dir: &Path) -> String {
    format!(r#"[Unit]
Description=latch — single-user passkey-based auth
After=network.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={bin} run --config {config}
WorkingDirectory={state_dir}
User={SYSTEM_USER}
Group={SYSTEM_USER}
Restart=on-failure
RestartSec=2

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths={state_dir}
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
ProtectHostname=true
ProtectProc=invisible
ProcSubset=pid
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
LockPersonality=true
CapabilityBoundingSet=
AmbientCapabilities=
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources
UMask=0077

[Install]
WantedBy=multi-user.target
"#,
        bin       = bin.display(),
        config    = config.display(),
        state_dir = state_dir.display(),
    )
}

fn render_user_unit(bin: &Path, config: &Path, state_dir: &Path) -> String {
    // systemd --user runs without CAP_SETPCAP, so any directive that
    // implicitly drops capabilities (ProtectKernel*, ProtectClock,
    // ProtectControlGroups, etc.) fails the CAPABILITIES exec step.
    // Stick to seccomp- and process-attribute-based hardening, which
    // works unprivileged.
    format!(r#"[Unit]
Description=latch — single-user passkey-based auth
After=network.target

[Service]
Type=simple
ExecStart={bin} run --config {config}
WorkingDirectory={state_dir}
Restart=on-failure
RestartSec=2

# Hardening (user-mode safe)
NoNewPrivileges=true
LockPersonality=true
RestrictRealtime=true
RestrictNamespaces=true
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources
UMask=0077

[Install]
WantedBy=default.target
"#,
        bin       = bin.display(),
        config    = config.display(),
        state_dir = state_dir.display(),
    )
}

// --- system user & state dir ----------------------------------------------

fn ensure_system_user() -> Result<(), String> {
    let exists = Command::new("getent")
        .args(["passwd", SYSTEM_USER])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if exists { return Ok(()); }

    let home = config::default_state_dir(Mode::System);
    let status = Command::new("useradd")
        .args([
            "--system",
            "--no-create-home",
            "--shell", "/usr/sbin/nologin",
            "--home-dir",
        ])
        .arg(&home)
        .arg(SYSTEM_USER)
        .status()
        .map_err(|e| format!("useradd: {e}"))?;
    if !status.success() {
        return Err(format!("useradd failed: {status}"));
    }
    eprintln!("created system user '{SYSTEM_USER}'");
    Ok(())
}

fn remove_system_user() -> Result<(), String> {
    let _ = Command::new("userdel").arg(SYSTEM_USER).status();
    Ok(())
}

fn ensure_state_dir(mode: Mode, path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("mkdir {}: {e}", path.display()))?;
    if mode == Mode::System {
        let status = Command::new("chown")
            .arg(format!("{SYSTEM_USER}:{SYSTEM_USER}"))
            .arg(path)
            .status()
            .map_err(|e| format!("chown: {e}"))?;
        if !status.success() {
            return Err(format!("chown failed: {status}"));
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_config_has_required_field() {
        let s = render_config(
            "a.b.com", "https://a.b.com", "b.com",
            Path::new("/tmp/latch"),
        );
        assert!(s.contains("rp_id = \"a.b.com\""));
        assert!(s.contains("# rp_origin"));
        assert!(s.contains("# state_dir     = \"/tmp/latch\""));
    }

    #[test]
    fn user_unit_has_no_privileged_directives() {
        let u = render_user_unit(
            Path::new("/usr/local/bin/latch"),
            Path::new("/home/me/.config/latch/config.toml"),
            Path::new("/home/me/.local/state/latch"),
        );
        assert!(!u.contains("User="));
        assert!(!u.contains("Group="));
        assert!(!u.contains("ProtectHome="));
        assert!(!u.contains("ProtectSystem="));
        // These implicitly drop caps and fail in --user mode.
        assert!(!u.contains("ProtectKernel"));
        assert!(!u.contains("ProtectClock"));
        assert!(!u.contains("ProtectControlGroups"));
        assert!(!u.contains("ProtectHostname"));
        assert!(!u.contains("PrivateDevices"));
        assert!(u.contains("NoNewPrivileges=true"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn system_unit_keeps_protections() {
        let u = render_system_unit(
            Path::new("/usr/local/bin/latch"),
            Path::new("/etc/latch/config.toml"),
            Path::new("/var/lib/latch"),
        );
        assert!(u.contains("User=latch"));
        assert!(u.contains("ProtectHome=true"));
        assert!(u.contains("ProtectSystem=strict"));
        assert!(u.contains("WantedBy=multi-user.target"));
    }
}
