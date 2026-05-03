use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use webauthn_rs::prelude::Url;

// --- universal constants ---------------------------------------------------

pub const RP_NAME:      &str = "latch";
pub const USER_ID:      &str = "c3a4b1f0-0000-0000-0000-000000000001";
pub const USER_NAME:    &str = "me";
pub const USER_DISPLAY: &str = "latch";

pub const SESSION_TTL:    Duration = Duration::from_secs(60 * 60 * 24 * 7);
pub const CHALLENGE_TTL:  Duration = Duration::from_secs(60 * 5);
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

pub const COOKIE_SESSION:   &str = "latch_s";
pub const COOKIE_CHALLENGE: &str = "latch_c";

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

// --- mode ------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode { System, User }

impl Mode {
    pub fn detect() -> Self {
        // SAFETY: getuid() has no preconditions.
        if unsafe { getuid() } == 0 { Mode::System } else { Mode::User }
    }

    pub fn label(&self) -> &'static str {
        match self { Mode::System => "system", Mode::User => "user" }
    }
}

extern "C" { fn getuid() -> u32; }

pub fn default_config_path(mode: Mode) -> PathBuf {
    match mode {
        Mode::System => PathBuf::from("/etc/latch/config.toml"),
        Mode::User   => xdg_config_home().join("latch/config.toml"),
    }
}

pub fn default_state_dir(mode: Mode) -> PathBuf {
    match mode {
        Mode::System => PathBuf::from("/var/lib/latch"),
        Mode::User   => xdg_state_home().join("latch"),
    }
}

pub fn xdg_config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".config"))
}

pub fn xdg_state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(".local/state"))
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

// --- config file -----------------------------------------------------------

#[derive(Deserialize)]
pub struct ConfigFile {
    pub rp_id:         String,
    pub rp_origin:     Option<String>,
    pub cookie_domain: Option<String>,
    pub listen:        Option<String>,
    pub state_dir:     Option<String>,
}

pub struct Config {
    pub rp_id:         String,
    pub rp_origin:     String,
    pub cookie_domain: String,
    pub listen:        String,
    pub state_dir:     PathBuf,
    pub creds_path:    PathBuf,
    pub key_path:      PathBuf,
    pub revoked_path:  PathBuf,
    pub source:        PathBuf,
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self, String> {
        let path = find_config(explicit)?;
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let cf: ConfigFile = toml::from_str(&raw)
            .map_err(|e| format!("parse {}: {e}", path.display()))?;
        Self::resolve(cf, path)
    }

    pub fn resolve(cf: ConfigFile, source: PathBuf) -> Result<Self, String> {
        let rp_id = cf.rp_id;
        if rp_id.contains("example.com") {
            return Err("rp_id still has placeholder; edit your config file".into());
        }
        let rp_origin = cf.rp_origin.unwrap_or_else(|| format!("https://{rp_id}"));
        let cookie_domain = match cf.cookie_domain {
            Some(d) => d,
            None    => derive_cookie_domain(&rp_id)?,
        };
        let listen = cf.listen.unwrap_or_else(|| DEFAULT_LISTEN.into());
        let state_dir = cf.state_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| default_state_dir(Mode::detect()));

        let creds_path   = state_dir.join("creds.json");
        let key_path     = state_dir.join("key");
        let revoked_path = state_dir.join("revoked.json");

        Ok(Self {
            rp_id, rp_origin, cookie_domain, listen,
            state_dir, creds_path, key_path, revoked_path, source,
        })
    }

    pub fn print(&self) {
        eprintln!("latch {} on {}", env!("CARGO_PKG_VERSION"), self.listen);
        eprintln!("  config        = {}", self.source.display());
        eprintln!("  rp_id         = {}", self.rp_id);
        eprintln!("  rp_origin     = {}", self.rp_origin);
        eprintln!("  cookie_domain = {}", self.cookie_domain);
        eprintln!("  state_dir     = {}", self.state_dir.display());
    }

    pub fn validate_next(&self, next: &str) -> String {
        if next.starts_with('/') && !next.starts_with("//") {
            return next.into();
        }
        let Ok(u) = Url::parse(next)  else { return "/".into() };
        let Some(host) = u.host_str() else { return "/".into() };
        let dot = format!(".{}", self.cookie_domain);
        if u.scheme() == "https" && (host == self.cookie_domain || host.ends_with(&dot)) {
            next.into()
        } else {
            "/".into()
        }
    }
}

pub fn find_config(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        if !p.exists() {
            return Err(format!("config not found: {}", p.display()));
        }
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = env::var("LATCH_CONFIG") {
        let p = PathBuf::from(p);
        if p.exists() { return Ok(p); }
    }
    let local = PathBuf::from("./latch.toml");
    if local.exists() { return Ok(local); }

    let mode = Mode::detect();
    let default = default_config_path(mode);
    if default.exists() { return Ok(default); }

    Err(format!(
        "no config found. Run `latch init` to create {}", default.display(),
    ))
}

pub fn derive_cookie_domain(rp_id: &str) -> Result<String, String> {
    let parts: Vec<&str> = rp_id.split('.').collect();
    if parts.len() < 2 {
        return Err(format!(
            "rp_id `{rp_id}` has no parent domain; set cookie_domain explicitly"
        ));
    }
    Ok(parts[1..].join("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            rp_id:         "latch.example.org".into(),
            rp_origin:     "https://latch.example.org".into(),
            cookie_domain: "example.org".into(),
            listen:        "127.0.0.1:0".into(),
            state_dir:     PathBuf::from("/tmp/test-latch"),
            creds_path:    PathBuf::from("/tmp/test-latch/creds.json"),
            key_path:      PathBuf::from("/tmp/test-latch/key"),
            revoked_path:  PathBuf::from("/tmp/test-latch/revoked.json"),
            source:        PathBuf::from("/tmp/test-latch/config.toml"),
        }
    }

    #[test]
    fn derives_cookie_domain() {
        assert_eq!(derive_cookie_domain("latch.example.com").unwrap(), "example.com");
        assert_eq!(derive_cookie_domain("auth.foo.bar.dev").unwrap(), "foo.bar.dev");
        assert!(derive_cookie_domain("apex").is_err());
    }

    #[test]
    fn validate_next_paths() {
        let c = cfg();
        assert_eq!(c.validate_next("/foo"), "/foo");
        assert_eq!(c.validate_next("/"), "/");
    }

    #[test]
    fn validate_next_rejects() {
        let c = cfg();
        assert_eq!(c.validate_next("//evil.com"), "/");
        assert_eq!(c.validate_next("https://evil.com"), "/");
        assert_eq!(c.validate_next("http://example.org/"), "/");
    }

    #[test]
    fn validate_next_accepts() {
        let c = cfg();
        assert_eq!(c.validate_next("https://app.example.org/dash"), "https://app.example.org/dash");
        assert_eq!(c.validate_next("https://example.org/"), "https://example.org/");
    }

    #[test]
    fn default_paths_differ_by_mode() {
        assert_eq!(default_config_path(Mode::System), PathBuf::from("/etc/latch/config.toml"));
        assert!(default_config_path(Mode::User).ends_with("latch/config.toml"));
        assert_eq!(default_state_dir(Mode::System), PathBuf::from("/var/lib/latch"));
        assert!(default_state_dir(Mode::User).ends_with("latch"));
    }
}
