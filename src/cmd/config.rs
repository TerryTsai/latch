// `latch config` — init, show, path.
//
// init writes a TOML file (interactive by default; non-interactive with
// flags + --yes). show emits the resolved (file + env) config to stdout.
// path prints where the config is loaded from.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::{self, Config, DEFAULT_LISTEN, Mode};
use crate::output::{self, json_mode, Error, Result};

// --- init ------------------------------------------------------------------

#[derive(Default)]
pub struct InitOpts {
    pub hostname:      Option<String>,
    pub origin:        Option<String>,
    pub cookie_domain: Option<String>,
    pub listen:        Option<String>,
    pub data_dir:      Option<PathBuf>,
    pub path:          Option<PathBuf>,
    pub print:         bool,
    pub yes:           bool,
}

pub fn init(opts: InitOpts) -> Result<()> {
    let mode = Mode::detect();
    let target = opts.path.unwrap_or_else(|| config::default_config_path(mode));

    let hostname = match opts.hostname {
        Some(s) => s,
        None    => {
            if !output::is_tty(0) {
                return Err(Error::usage("hostname is required on non-TTY")
                    .with_hint("pass --hostname=<host> or run interactively"));
            }
            prompt("Hostname (e.g. latch.example.com): ")?
        }
    };
    let hostname = hostname.trim().to_string();
    if hostname.is_empty() {
        return Err(Error::usage("hostname is required"));
    }
    if hostname.contains("example.com") {
        return Err(Error::usage("hostname can't be a placeholder"));
    }

    let derived_origin   = format!("https://{hostname}");
    let derived_domain   = config::derive_cookie_domain(&hostname);
    let default_data_dir = config::default_data_dir(mode);

    let content = render_config(
        &hostname,
        opts.origin.as_deref(),        &derived_origin,
        opts.cookie_domain.as_deref(), &derived_domain,
        opts.listen.as_deref(),
        opts.data_dir.as_deref(),      &default_data_dir,
    );

    if opts.print {
        // --print writes the rendered config to stdout (data) so it can pipe.
        print!("{content}");
        return Ok(());
    }

    if !opts.yes {
        eprintln!();
        eprintln!("  mode          = {}", mode.label());
        eprintln!("  hostname      = {hostname}");
        show_field("origin",        opts.origin.as_deref(),        &derived_origin);
        show_field("cookie_domain", opts.cookie_domain.as_deref(), &derived_domain);
        show_field("listen",        opts.listen.as_deref(),        DEFAULT_LISTEN);
        show_field("data_dir",      opts.data_dir.as_deref().map(|p| p.display().to_string()).as_deref(),
                                    &default_data_dir.display().to_string());
        eprintln!();
        let answer = prompt(&format!("Write to {}? [Y/n] ", target.display()))?;
        if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
            return Err(Error::fail("aborted"));
        }
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| Error::cantcreat(format!("mkdir {}: {e}", parent.display())))?;
    }
    fs::write(&target, content)
        .map_err(|e| Error::cantcreat(format!("write {}: {e}", target.display())))?;

    eprintln!("Wrote {}", target.display());
    eprintln!("Next:");
    match mode {
        Mode::System => eprintln!("    sudo latch service start"),
        Mode::User   => eprintln!("    latch service start"),
    }
    Ok(())
}

fn show_field(name: &str, value: Option<&str>, default: &str) {
    match value {
        Some(v) => eprintln!("  {name:<13} = {v}"),
        None    => eprintln!("  {name:<13} = {default}   (default)"),
    }
}

// Each Optional field: when Some, written uncommented; when None, written
// as a commented default the user can later uncomment. Same shape either
// way so subsequent `latch config init` re-runs are idempotent in spirit.
pub(crate) fn render_config(
    hostname:            &str,
    origin:              Option<&str>, derived_origin: &str,
    cookie_domain:       Option<&str>, derived_domain: &str,
    listen:              Option<&str>,
    data_dir:            Option<&Path>, default_data_dir: &Path,
) -> String {
    let mut out = String::new();
    out.push_str("# latch config\n");
    out.push_str("# https://github.com/TerryTsai/latch\n\n");
    out.push_str("# REQUIRED. Hostname where latch is publicly reachable.\n");
    out.push_str(&format!("hostname = \"{hostname}\"\n\n"));
    out.push_str("# Optional overrides. Defaults shown commented.\n");
    line(&mut out, "origin",        origin,        derived_origin);
    line(&mut out, "cookie_domain", cookie_domain, derived_domain);
    line(&mut out, "listen",        listen,        DEFAULT_LISTEN);
    let dsd = default_data_dir.display().to_string();
    let dd  = data_dir.map(|p| p.display().to_string());
    line(&mut out, "data_dir",      dd.as_deref(), &dsd);
    out
}

fn line(buf: &mut String, name: &str, value: Option<&str>, default: &str) {
    match value {
        Some(v) => buf.push_str(&format!("{name:<13} = \"{v}\"\n")),
        None    => buf.push_str(&format!("# {name:<13} = \"{default}\"\n")),
    }
}

fn prompt(msg: &str) -> Result<String> {
    eprint!("{msg}");
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)
        .map_err(|e| Error::fail(format!("stdin: {e}")))?;
    Ok(buf.trim().to_string())
}

// --- show ------------------------------------------------------------------

pub fn show(explicit: Option<&Path>) -> Result<()> {
    let cfg = Config::load(explicit)
        .map_err(|e| Error::config(e).with_hint(load_hint()))?;

    if json_mode() {
        let v = serde_json::json!({
            "config":        cfg.source.display(),
            "hostname":      cfg.hostname,
            "origin":        cfg.origin,
            "cookie_domain": cfg.cookie_domain,
            "listen":        cfg.listen,
            "data_dir":      cfg.data_dir.display().to_string(),
            "passkeys_path": cfg.passkeys_path.display().to_string(),
        });
        output::emit_json(&v);
    } else {
        println!("config        = {}", cfg.source.display());
        println!("hostname      = {}", cfg.hostname);
        println!("origin        = {}", cfg.origin);
        println!("cookie_domain = {}", cfg.cookie_domain);
        println!("listen        = {}", cfg.listen);
        println!("data_dir      = {}", cfg.data_dir.display());
        println!("passkeys_path = {}", cfg.passkeys_path.display());
    }
    Ok(())
}

// --- path ------------------------------------------------------------------

// Print the path that would be loaded. One line, no decoration. Useful in
// scripts. If only env vars are set, prints "(env)".
pub fn path(explicit: Option<&Path>) -> Result<()> {
    match config::find_config(explicit) {
        Ok(p) => {
            println!("{}", p.display());
            Ok(())
        }
        Err(e) => {
            // Env-only is a valid mode if LATCH_HOSTNAME is set.
            if std::env::var("LATCH_HOSTNAME").is_ok() {
                println!("(env)");
                Ok(())
            } else {
                Err(Error::config(e).with_hint(load_hint()))
            }
        }
    }
}

fn load_hint() -> String {
    let default = config::default_config_path(Mode::detect());
    format!("run `latch config init` to create {}, or set LATCH_HOSTNAME", default.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_config_defaults_are_commented() {
        let s = render_config(
            "a.b.com",
            None, "https://a.b.com",
            None, "b.com",
            None,
            None, Path::new("/tmp/latch"),
        );
        assert!(s.contains("hostname = \"a.b.com\""));
        assert!(s.contains("# origin        = \"https://a.b.com\""));
        assert!(s.contains("# data_dir      = \"/tmp/latch\""));
    }

    #[test]
    fn render_config_explicit_overrides_uncommented() {
        let s = render_config(
            "a.b.com",
            Some("https://other.b.com"), "https://a.b.com",
            None, "b.com",
            Some("0.0.0.0:8080"),
            None, Path::new("/tmp/latch"),
        );
        assert!(s.contains("origin        = \"https://other.b.com\""));
        assert!(s.contains("listen        = \"0.0.0.0:8080\""));
        assert!(s.contains("# cookie_domain"));  // not overridden
    }
}
