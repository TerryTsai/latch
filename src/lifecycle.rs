// Self-management commands: init, start, stop, restart, status, uninstall.
// Everything that's about latch's *lifecycle on a host*, not about serving HTTP.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{self, Config, DEFAULT_CONFIG, DEFAULT_LISTEN, DEFAULT_STATE_DIR};

const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/latch.service";
const SERVICE_USER: &str = "latch";

// --- init ------------------------------------------------------------------

pub fn init(rp_id: Option<String>, path: Option<PathBuf>, print: bool, yes: bool) -> Result<(), String> {
    let target = path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));

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

    let content = render_config(&rp_id, &rp_origin, &cookie_domain);

    if print {
        print!("{content}");
        return Ok(());
    }

    if !yes {
        eprintln!();
        eprintln!("  rp_id         = {rp_id}");
        eprintln!("  rp_origin     = {rp_origin}   (derived)");
        eprintln!("  cookie_domain = {cookie_domain}   (derived)");
        eprintln!("  listen        = {DEFAULT_LISTEN}   (default)");
        eprintln!("  state_dir     = {DEFAULT_STATE_DIR}   (default)");
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
    eprintln!("Next: latch start");
    Ok(())
}

fn render_config(rp_id: &str, rp_origin: &str, cookie_domain: &str) -> String {
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
         # state_dir     = \"{DEFAULT_STATE_DIR}\"\n"
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
    require_root("start")?;
    let cfg = Config::load(config_path)?;
    cfg.print();

    ensure_user()?;
    ensure_state_dir(&cfg.state_dir)?;
    write_unit(&cfg)?;

    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", "latch.service"])?;

    std::thread::sleep(std::time::Duration::from_millis(500));
    if !is_unit_active() {
        return Err("started, but service is not active. Check: journalctl -u latch -n 50".into());
    }
    eprintln!("latch is running. journalctl -u latch -f to follow logs.");
    Ok(())
}

pub fn stop() -> Result<(), String> {
    require_root("stop")?;
    systemctl(&["stop", "latch.service"])?;
    eprintln!("latch stopped.");
    Ok(())
}

pub fn restart() -> Result<(), String> {
    require_root("restart")?;
    systemctl(&["restart", "latch.service"])?;
    eprintln!("latch restarted.");
    Ok(())
}

pub fn status() -> Result<(), String> {
    let active = is_unit_active();
    let enabled = is_enabled();
    println!("latch:  {}  ({})",
        if active { "active" } else { "inactive" },
        if enabled { "enabled" } else { "disabled" },
    );
    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        println!("unit:   {SYSTEMD_UNIT_PATH}");
    } else {
        println!("unit:   not installed (run `latch start`)");
    }
    if Path::new(config::DEFAULT_CONFIG).exists() {
        println!("config: {}", config::DEFAULT_CONFIG);
    }
    Ok(())
}

// --- uninstall -------------------------------------------------------------

pub fn uninstall(purge: bool) -> Result<(), String> {
    require_root("uninstall")?;

    let _ = systemctl(&["stop",    "latch.service"]);
    let _ = systemctl(&["disable", "latch.service"]);
    let _ = fs::remove_file(SYSTEMD_UNIT_PATH);
    let _ = systemctl(&["daemon-reload"]);
    eprintln!("removed systemd unit");

    if purge {
        // Best-effort cleanup of state and config.
        if let Ok(cfg) = Config::load(None) {
            if cfg.state_dir.exists() {
                let _ = fs::remove_dir_all(&cfg.state_dir);
                eprintln!("removed state dir: {}", cfg.state_dir.display());
            }
        }
        if Path::new(config::DEFAULT_CONFIG).exists() {
            let _ = fs::remove_file(config::DEFAULT_CONFIG);
            if let Some(parent) = Path::new(config::DEFAULT_CONFIG).parent() {
                let _ = fs::remove_dir(parent);  // empty-only
            }
            eprintln!("removed config");
        }
        let _ = remove_user();
    }

    if let Ok(bin) = std::env::current_exe() {
        eprintln!();
        eprintln!("the binary itself is at {}", bin.display());
        eprintln!("remove with: sudo rm {}", bin.display());
    }
    Ok(())
}

// --- systemd glue ----------------------------------------------------------

fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = Command::new("systemctl").args(args).status()
        .map_err(|e| format!("run systemctl: {e}"))?;
    if !status.success() {
        return Err(format!("systemctl {} failed (exit {})",
            args.join(" "),
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        ));
    }
    Ok(())
}

pub fn is_unit_active() -> bool {
    Command::new("systemctl").args(["is-active", "--quiet", "latch.service"])
        .status().map(|s| s.success()).unwrap_or(false)
}

fn is_enabled() -> bool {
    Command::new("systemctl").args(["is-enabled", "--quiet", "latch.service"])
        .status().map(|s| s.success()).unwrap_or(false)
}

fn write_unit(cfg: &Config) -> Result<(), String> {
    let bin = std::env::current_exe()
        .map_err(|e| format!("can't locate own binary: {e}"))?;
    let unit = render_unit(&bin, &cfg.source, &cfg.state_dir);
    fs::write(SYSTEMD_UNIT_PATH, unit)
        .map_err(|e| format!("write unit: {e}"))?;
    Ok(())
}

fn render_unit(bin: &Path, config: &Path, state_dir: &Path) -> String {
    format!(r#"[Unit]
Description=latch — single-user passkey-based auth
After=network.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={bin} run --config {config}
WorkingDirectory={state_dir}
User={SERVICE_USER}
Group={SERVICE_USER}
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

// --- system user & state dir ----------------------------------------------

fn ensure_user() -> Result<(), String> {
    let exists = Command::new("getent")
        .args(["passwd", SERVICE_USER])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if exists { return Ok(()); }

    let status = Command::new("useradd")
        .args([
            "--system",
            "--no-create-home",
            "--shell", "/usr/sbin/nologin",
            "--home-dir", DEFAULT_STATE_DIR,
            SERVICE_USER,
        ])
        .status()
        .map_err(|e| format!("useradd: {e}"))?;
    if !status.success() {
        return Err(format!("useradd failed: {status}"));
    }
    eprintln!("created system user '{SERVICE_USER}'");
    Ok(())
}

fn remove_user() -> Result<(), String> {
    let _ = Command::new("userdel").arg(SERVICE_USER).status();
    Ok(())
}

fn ensure_state_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("mkdir {}: {e}", path.display()))?;
    let status = Command::new("chown")
        .arg(format!("{SERVICE_USER}:{SERVICE_USER}"))
        .arg(path)
        .status()
        .map_err(|e| format!("chown: {e}"))?;
    if !status.success() {
        return Err(format!("chown failed: {status}"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod: {e}"))?;
    Ok(())
}

// --- helpers ---------------------------------------------------------------

fn require_root(action: &str) -> Result<(), String> {
    // SAFETY: getuid() is a libc syscall with no preconditions.
    let euid = unsafe { getuid() };
    if euid != 0 {
        return Err(format!("`latch {action}` requires root (try sudo)"));
    }
    Ok(())
}

extern "C" {
    fn getuid() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_config_has_required_field() {
        let s = render_config("a.b.com", "https://a.b.com", "b.com");
        assert!(s.contains("rp_id = \"a.b.com\""));
        assert!(s.contains("# rp_origin"));
    }
}
