use std::env;
use std::time::Duration;

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

// --- per-deployment config -------------------------------------------------

pub struct Config {
    pub rp_id:         String,
    pub rp_origin:     String,
    pub cookie_domain: String,
    pub listen:        String,
    pub creds_path:    String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            rp_id:         var("LATCH_RP_ID",         "latch.example.com"),
            rp_origin:     var("LATCH_RP_ORIGIN",     "https://latch.example.com"),
            cookie_domain: var("LATCH_COOKIE_DOMAIN", "example.com"),
            listen:        var("LATCH_LISTEN",        "127.0.0.1:8080"),
            creds_path:    var("LATCH_CREDS_PATH",    "creds.json"),
        }
    }

    pub fn check(&self) -> Result<(), String> {
        for (name, value) in [
            ("LATCH_RP_ID",         &self.rp_id),
            ("LATCH_RP_ORIGIN",     &self.rp_origin),
            ("LATCH_COOKIE_DOMAIN", &self.cookie_domain),
        ] {
            if value.contains("example.com") {
                return Err(format!("{name} still has placeholder ({value})"));
            }
        }
        Ok(())
    }

    pub fn print(&self) {
        eprintln!("latch {} on {}", env!("CARGO_PKG_VERSION"), self.listen);
        eprintln!("  RP_ID         = {}", self.rp_id);
        eprintln!("  RP_ORIGIN     = {}", self.rp_origin);
        eprintln!("  COOKIE_DOMAIN = {}", self.cookie_domain);
        eprintln!("  CREDS_PATH    = {}", self.creds_path);
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

fn var(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.into())
}
