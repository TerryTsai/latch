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

pub const COOKIE_SESSION:   &str = "latch_session";
pub const COOKIE_CHALLENGE: &str = "latch_challenge";

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

// --- config schema ---------------------------------------------------------

// Every field is Option so the same struct represents a parsed TOML file,
// an env-only layer, or the merged result. Resolve() validates rp_id is
// present at the end.
#[derive(Default, Deserialize)]
pub struct ConfigFile {
    pub rp_id:         Option<String>,
    pub rp_origin:     Option<String>,
    pub cookie_domain: Option<String>,
    pub listen:        Option<String>,
    pub state_dir:     Option<String>,
}

impl ConfigFile {
    pub fn from_env() -> Self {
        Self {
            rp_id:         env::var("LATCH_RP_ID").ok(),
            rp_origin:     env::var("LATCH_RP_ORIGIN").ok(),
            cookie_domain: env::var("LATCH_COOKIE_DOMAIN").ok(),
            listen:        env::var("LATCH_LISTEN").ok(),
            state_dir:     env::var("LATCH_STATE_DIR").ok(),
        }
    }

    // Right-hand wins on each field where it's Some.
    pub fn merge(&mut self, other: ConfigFile) {
        if other.rp_id.is_some()         { self.rp_id         = other.rp_id; }
        if other.rp_origin.is_some()     { self.rp_origin     = other.rp_origin; }
        if other.cookie_domain.is_some() { self.cookie_domain = other.cookie_domain; }
        if other.listen.is_some()        { self.listen        = other.listen; }
        if other.state_dir.is_some()     { self.state_dir     = other.state_dir; }
    }
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
    pub source:        ConfigSource,
}

pub enum ConfigSource {
    File(PathBuf),
    Env,
}

impl ConfigSource {
    fn display(&self) -> String {
        match self {
            ConfigSource::File(p) => p.display().to_string(),
            ConfigSource::Env     => "(env)".into(),
        }
    }
}

impl Config {
    // Load order: TOML file (if found) ← merged with ← env vars.
    // If no file is found but LATCH_RP_ID is set, run from env alone.
    pub fn load(explicit: Option<&Path>) -> Result<Self, String> {
        let env_layer = ConfigFile::from_env();

        let (mut cf, source) = match find_config(explicit) {
            Ok(path) => {
                let raw = fs::read_to_string(&path)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                let cf: ConfigFile = toml::from_str(&raw)
                    .map_err(|e| format!("parse {}: {e}", path.display()))?;
                (cf, ConfigSource::File(path))
            }
            Err(e) => {
                if env_layer.rp_id.is_none() { return Err(e); }
                (ConfigFile::default(), ConfigSource::Env)
            }
        };

        cf.merge(env_layer);
        Self::resolve(cf, source)
    }

    pub fn resolve(cf: ConfigFile, source: ConfigSource) -> Result<Self, String> {
        let rp_id = cf.rp_id
            .ok_or("rp_id is required (set in config or via LATCH_RP_ID)")?;
        if rp_id.contains("example.com") {
            return Err("rp_id still has placeholder; edit your config or env".into());
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

    let default = default_config_path(Mode::detect());
    if default.exists() { return Ok(default); }

    Err(format!(
        "no config found. Run `latch init` to create {}, or set LATCH_RP_ID in env",
        default.display(),
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
            source:        ConfigSource::File(PathBuf::from("/tmp/test-latch/config.toml")),
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

    #[test]
    fn merge_replaces_present_fields_only() {
        let mut a = ConfigFile {
            rp_id: Some("a.com".into()),
            listen: Some("127.0.0.1:1".into()),
            ..Default::default()
        };
        let b = ConfigFile {
            rp_id: Some("b.com".into()),
            state_dir: Some("/x".into()),
            ..Default::default()
        };
        a.merge(b);
        assert_eq!(a.rp_id.as_deref(), Some("b.com"));
        assert_eq!(a.listen.as_deref(), Some("127.0.0.1:1"));  // unchanged
        assert_eq!(a.state_dir.as_deref(), Some("/x"));
    }

    #[test]
    fn resolve_requires_rp_id() {
        let cf = ConfigFile::default();
        assert!(Config::resolve(cf, ConfigSource::Env).is_err());
    }

    #[test]
    fn resolve_synthesizes_from_minimum() {
        let cf = ConfigFile {
            rp_id: Some("latch.foo.org".into()),
            ..Default::default()
        };
        let c = Config::resolve(cf, ConfigSource::Env).unwrap();
        assert_eq!(c.rp_origin, "https://latch.foo.org");
        assert_eq!(c.cookie_domain, "foo.org");
        assert_eq!(c.listen, DEFAULT_LISTEN);
    }
}
