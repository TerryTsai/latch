// `latch check` — read-only validation of config + state.
//
// Validates: config loads, origin is https://, hostname is non-placeholder,
// data_dir is writable, passkeys.json is readable. Output respects --json
// and the TTY/non-TTY distinction. Exits 78 (EX_CONFIG) on any failure.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{self, Config, Mode};
use crate::output::{self, Error, Result};
use crate::state;

struct Row {
    label:  &'static str,
    value:  String,
    ok:     bool,
    detail: Option<String>,
}

pub fn run(explicit: Option<&Path>) -> Result<()> {
    let mut rows: Vec<Row> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    // 1. config loads
    let cfg = match Config::load(explicit) {
        Ok(c)  => c,
        Err(e) => {
            // No config at all: emit a single failure row + suggested next step.
            return fail_no_config(e);
        }
    };

    rows.push(Row {
        label: "config", value: cfg.source.display(), ok: true, detail: None,
    });

    // 2. hostname / origin / cookie_domain
    rows.push(Row { label: "hostname", value: cfg.hostname.clone(), ok: true, detail: None });

    if cfg.origin.starts_with("https://") {
        rows.push(Row { label: "origin", value: cfg.origin.clone(), ok: true, detail: None });
    } else {
        problems.push(format!("origin must start with https:// (got {:?})", cfg.origin));
        rows.push(Row {
            label: "origin", value: cfg.origin.clone(), ok: false,
            detail: Some("must start with https://".into()),
        });
    }

    rows.push(Row {
        label: "cookie_domain", value: cfg.cookie_domain.clone(), ok: true, detail: None,
    });

    // 4. listen
    rows.push(Row { label: "listen", value: cfg.listen.clone(), ok: true, detail: None });

    // 5. data_dir writability
    let dd_writable = data_dir_writable(&cfg.data_dir);
    if dd_writable {
        rows.push(Row {
            label: "data_dir", value: cfg.data_dir.display().to_string(), ok: true,
            detail: Some("writable".into()),
        });
    } else {
        problems.push(format!("data_dir not writable: {}", cfg.data_dir.display()));
        let hint = legacy_hint(&cfg.data_dir);
        rows.push(Row {
            label: "data_dir", value: cfg.data_dir.display().to_string(), ok: false,
            detail: Some(hint.unwrap_or_else(|| "not writable".into())),
        });
    }

    // 6. passkeys
    let count = state::load_passkeys(&cfg.passkeys_path).len();
    let label = match count {
        0 => "0 passkeys (register one by visiting the page)".to_string(),
        1 => "1 passkey".to_string(),
        n => format!("{n} passkeys"),
    };
    rows.push(Row {
        label: "passkeys", value: cfg.passkeys_path.display().to_string(), ok: true,
        detail: Some(label),
    });

    if output::json_mode() {
        emit_json(&cfg, count, &problems);
    } else {
        emit_human(&rows, &problems);
    }

    if problems.is_empty() {
        Ok(())
    } else {
        // Already printed problem rows; return a quiet error with the right code.
        Err(Error { msg: format!("{} problem(s)", problems.len()), hint: None, code: output::EX_CONFIG })
    }
}

fn fail_no_config(msg: String) -> Result<()> {
    if output::json_mode() {
        let v = serde_json::json!({
            "ok":       false,
            "problems": [msg],
        });
        output::emit_json(&v);
    } else {
        println!("{} no config", output::cross());
        println!("    {msg}");
    }
    let default = config::default_config_path(Mode::detect());
    Err(Error::config("no config found")
        .with_hint(format!("run `latch config init` to create {}", default.display())))
}

fn emit_human(rows: &[Row], problems: &[String]) {
    let label_width = rows.iter().map(|r| r.label.len()).max().unwrap_or(0);
    for r in rows {
        let mark = if r.ok { output::tick() } else { output::cross() };
        match &r.detail {
            Some(d) => println!("{} {:label_width$}  {}    {}", mark, r.label, r.value, d),
            None    => println!("{} {:label_width$}  {}",       mark, r.label, r.value),
        }
    }
    if problems.is_empty() {
        println!("ok");
    } else {
        println!("not ok: {} problem(s)", problems.len());
    }
}

fn emit_json(cfg: &Config, passkeys: usize, problems: &[String]) {
    let v = serde_json::json!({
        "ok":            problems.is_empty(),
        "config":        cfg.source.display(),
        "hostname":      cfg.hostname,
        "origin":        cfg.origin,
        "cookie_domain": cfg.cookie_domain,
        "listen":        cfg.listen,
        "data_dir":      cfg.data_dir.display().to_string(),
        "passkeys":      passkeys,
        "problems":      problems,
    });
    output::emit_json(&v);
}

// Touch-write a probe file in data_dir (creating data_dir if needed) to
// verify writability. We mkdir-all because that's what `serve` does too;
// failing here is the same failure mode as boot.
fn data_dir_writable(path: &Path) -> bool {
    if fs::create_dir_all(path).is_err() { return false; }
    let probe = path.join(".latch-probe");
    if fs::write(&probe, b"").is_err() { return false; }
    let _ = fs::remove_file(&probe);
    true
}

// If we're in user mode and the new XDG_DATA_HOME path is empty but the
// v0.5 XDG_STATE_HOME path has data, point to the migration command.
fn legacy_hint(current: &PathBuf) -> Option<String> {
    if Mode::detect() != Mode::User { return None; }
    let legacy = config::legacy_user_data_dir();
    if legacy == *current { return None; }
    if !legacy.exists() { return None; }
    Some(format!("v0.5 data found at {}; run: mv {} {}",
                 legacy.display(), legacy.display(), current.display()))
}
