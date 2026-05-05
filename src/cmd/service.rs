// Service lifecycle: start, stop, restart, status, uninstall.
// Auto-detects mode by euid: root → system, otherwise user. Each command
// branches on Mode for paths, systemctl scope, and unit rendering.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{self, Config, ConfigSource, Mode};
use crate::output::{Error, Result};

const UNIT_FILE: &str = "latch.service";
const SYSTEM_UNIT_PATH: &str = "/etc/systemd/system/latch.service";
const SYSTEM_USER: &str = "latch";

// --- start / stop / restart / status --------------------------------------

pub fn start(config_path: Option<&Path>) -> Result<()> {
    let mode = Mode::detect();
    let cfg = Config::load(config_path)?;
    cfg.print();

    if mode == Mode::System {
        ensure_system_user()?;
    }
    ensure_data_dir(mode, &cfg.data_dir)?;
    write_unit(mode, &cfg)?;

    systemctl(mode, &["daemon-reload"])?;
    systemctl(mode, &["enable", "--now", UNIT_FILE])?;

    std::thread::sleep(std::time::Duration::from_millis(500));
    if !is_unit_active(mode) {
        return Err(Error::fail(format!(
            "started, but service is not active. Check: {} -u latch -n 50",
            journalctl_cmd(mode),
        )));
    }
    print_started(mode);
    Ok(())
}

pub fn stop() -> Result<()> {
    let mode = Mode::detect();
    systemctl(mode, &["stop", UNIT_FILE])?;
    eprintln!("latch stopped.");
    Ok(())
}

pub fn restart() -> Result<()> {
    let mode = Mode::detect();
    systemctl(mode, &["restart", UNIT_FILE])?;
    eprintln!("latch restarted.");
    Ok(())
}

pub fn status() -> Result<()> {
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
        let cmd = match mode { Mode::System => "sudo latch service start", Mode::User => "latch service start" };
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

// Removes the systemd unit only. Prints copy-paste commands for the rest
// (data_dir, config, binary). No --purge: each destructive step is run
// explicitly by the user.
pub fn uninstall() -> Result<()> {
    let mode = Mode::detect();

    let _ = systemctl(mode, &["stop",    UNIT_FILE]);
    let _ = systemctl(mode, &["disable", UNIT_FILE]);
    let _ = fs::remove_file(unit_path(mode));
    let _ = systemctl(mode, &["daemon-reload"]);
    eprintln!("removed {} systemd unit.", mode.label());

    let sudo = if mode == Mode::System { "sudo " } else { "" };

    // Surface the data dir we'd actually use, so users wipe the right place.
    let data_dir = Config::load(None)
        .map(|c| c.data_dir)
        .unwrap_or_else(|_| config::default_data_dir(mode));
    let cfg_path = config::default_config_path(mode);
    let bin = std::env::current_exe().ok();

    eprintln!();
    eprintln!("to also remove your data:");
    eprintln!("    {sudo}rm -rf {}", data_dir.display());
    eprintln!("to remove the config:");
    eprintln!("    {sudo}rm {}", cfg_path.display());
    if mode == Mode::System {
        eprintln!("to remove the system user:");
        eprintln!("    sudo userdel {SYSTEM_USER}");
    }
    if let Some(bin) = bin {
        eprintln!("to remove the binary:");
        eprintln!("    {sudo}rm {}", bin.display());
    }
    Ok(())
}

// --- systemd glue ----------------------------------------------------------

fn systemctl(mode: Mode, args: &[&str]) -> Result<()> {
    let mut cmd = Command::new("systemctl");
    if mode == Mode::User { cmd.arg("--user"); }
    let status = cmd.args(args).status()
        .map_err(|e| format!("run systemctl: {e}"))?;
    if !status.success() {
        let scope = if mode == Mode::User { "--user " } else { "" };
        return Err(Error::fail(format!("systemctl {scope}{} failed (exit {})",
            args.join(" "),
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        )));
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

fn write_unit(mode: Mode, cfg: &Config) -> Result<()> {
    let config_path = match &cfg.source {
        ConfigSource::File(p) => p,
        ConfigSource::Env => return Err(Error::config(
            "latch service start needs a config file"
        ).with_hint("run `latch config init` first, or use `latch serve` directly with env vars")),
    };
    let bin = std::env::current_exe()
        .map_err(|e| format!("can't locate own binary: {e}"))?;
    let unit = render_unit(mode, &bin, config_path, &cfg.data_dir);
    let path = unit_path(mode);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(&path, unit).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn render_unit(mode: Mode, bin: &Path, config: &Path, data_dir: &Path) -> String {
    match mode {
        Mode::System => render_system_unit(bin, config, data_dir),
        Mode::User   => render_user_unit(bin, config, data_dir),
    }
}

fn render_system_unit(bin: &Path, config: &Path, data_dir: &Path) -> String {
    format!(r#"[Unit]
Description=latch — single-user passkey-based auth
After=network.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={bin} serve --config {config}
WorkingDirectory={data_dir}
User={SYSTEM_USER}
Group={SYSTEM_USER}
Restart=on-failure
RestartSec=2

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths={data_dir}
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
        bin      = bin.display(),
        config   = config.display(),
        data_dir = data_dir.display(),
    )
}

fn render_user_unit(bin: &Path, config: &Path, data_dir: &Path) -> String {
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
ExecStart={bin} serve --config {config}
WorkingDirectory={data_dir}
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
        bin      = bin.display(),
        config   = config.display(),
        data_dir = data_dir.display(),
    )
}

// --- system user & data dir -----------------------------------------------

fn ensure_system_user() -> Result<()> {
    let exists = Command::new("getent")
        .args(["passwd", SYSTEM_USER])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if exists { return Ok(()); }

    let home = config::default_data_dir(Mode::System);
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
        return Err(Error::fail(format!("useradd failed: {status}")));
    }
    eprintln!("created system user '{SYSTEM_USER}'");
    Ok(())
}

fn ensure_data_dir(mode: Mode, path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| format!("mkdir {}: {e}", path.display()))?;
    if mode == Mode::System {
        let status = Command::new("chown")
            .arg(format!("{SYSTEM_USER}:{SYSTEM_USER}"))
            .arg(path)
            .status()
            .map_err(|e| format!("chown: {e}"))?;
        if !status.success() {
            return Err(Error::fail(format!("chown failed: {status}")));
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
    fn user_unit_has_no_privileged_directives() {
        let u = render_user_unit(
            Path::new("/usr/local/bin/latch"),
            Path::new("/home/me/.config/latch/config.toml"),
            Path::new("/home/me/.local/share/latch"),
        );
        assert!(!u.contains("User="));
        assert!(!u.contains("Group="));
        assert!(!u.contains("ProtectHome="));
        assert!(!u.contains("ProtectSystem="));
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

    #[test]
    fn units_invoke_serve_subcommand() {
        let s = render_system_unit(Path::new("/usr/local/bin/latch"),
                                   Path::new("/etc/latch/config.toml"),
                                   Path::new("/var/lib/latch"));
        assert!(s.contains("ExecStart=/usr/local/bin/latch serve --config"));
        let u = render_user_unit(Path::new("/usr/local/bin/latch"),
                                 Path::new("/c"), Path::new("/d"));
        assert!(u.contains("ExecStart=/usr/local/bin/latch serve --config"));
    }
}
