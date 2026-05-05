// `latch passkeys` — list, reset.
//
// list: prints registered passkeys (count + per-passkey {cred_id, aaguid}).
// reset: deletes all passkeys; prompts on TTY, requires --yes on non-TTY.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use webauthn_rs::prelude::Passkey;

use crate::config::Config;
use crate::output::{self, Error, Result};
use crate::state;

// --- list ------------------------------------------------------------------

pub fn list(explicit: Option<&Path>) -> Result<()> {
    let cfg = Config::load(explicit)?;
    let passkeys = state::load_passkeys(&cfg.passkeys_path);

    if output::json_mode() {
        let arr: Vec<serde_json::Value> = passkeys.iter().map(passkey_json).collect();
        output::emit_json(&serde_json::Value::Array(arr));
    } else if passkeys.is_empty() {
        println!("0 passkeys at {}", cfg.passkeys_path.display());
    } else {
        println!("{} passkey(s) at {}", passkeys.len(), cfg.passkeys_path.display());
        for (i, pk) in passkeys.iter().enumerate() {
            let id  = passkey_cred_id(pk);
            let aag = passkey_aaguid(pk);
            match aag {
                Some(a) => println!("  [{i}] cred_id={id}  aaguid={a}"),
                None    => println!("  [{i}] cred_id={id}"),
            }
        }
    }
    Ok(())
}

// --- reset -----------------------------------------------------------------

pub fn reset(explicit: Option<&Path>, yes: bool) -> Result<()> {
    let cfg = Config::load(explicit)?;
    let passkeys = state::load_passkeys(&cfg.passkeys_path);
    let n = passkeys.len();

    // Refuse before reading state: confirmation must come from a human (TTY)
    // or be explicit (--yes). Don't assume yes just because the file is empty.
    if !yes {
        if !output::is_tty(0) {
            return Err(Error::usage(
                format!("would delete {n} passkey(s) at {}", cfg.passkeys_path.display())
            ).with_hint("pass --yes to confirm on a non-TTY"));
        }
        eprint!("This will delete {n} passkey(s) at {}. Continue? [y/N] ",
                cfg.passkeys_path.display());
        io::stderr().flush().ok();
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)
            .map_err(|e| Error::fail(format!("stdin: {e}")))?;
        if !buf.trim().eq_ignore_ascii_case("y") {
            return Err(Error::fail("aborted"));
        }
    }

    if !cfg.passkeys_path.exists() {
        eprintln!("nothing to delete ({} doesn't exist)", cfg.passkeys_path.display());
        return Ok(());
    }

    fs::remove_file(&cfg.passkeys_path)
        .map_err(|e| Error::fail(format!("remove {}: {e}", cfg.passkeys_path.display())))?;
    eprintln!("deleted {} ({n} passkey(s))", cfg.passkeys_path.display());
    eprintln!("the next visit to the page will register a new passkey.");
    Ok(())
}

// --- passkey introspection -------------------------------------------------
//
// Passkey is a webauthn-rs type whose internals aren't directly exposed.
// We round-trip through serde_json to extract cred_id and aaguid; same
// representation as on disk, so this is cheap and stable.

fn passkey_json(pk: &Passkey) -> serde_json::Value {
    let v: serde_json::Value = serde_json::to_value(pk).unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "cred_id": passkey_cred_id_from_json(&v),
        "aaguid":  passkey_aaguid_from_json(&v),
    })
}

fn passkey_cred_id(pk: &Passkey) -> String {
    let v: serde_json::Value = serde_json::to_value(pk).unwrap_or(serde_json::Value::Null);
    passkey_cred_id_from_json(&v)
}

fn passkey_aaguid(pk: &Passkey) -> Option<String> {
    let v: serde_json::Value = serde_json::to_value(pk).unwrap_or(serde_json::Value::Null);
    passkey_aaguid_from_json(&v)
}

fn passkey_cred_id_from_json(v: &serde_json::Value) -> String {
    // webauthn-rs serializes credential id as base64url string under
    // multiple possible field names depending on version. Try a few.
    for path in &[
        &["cred", "cred_id"][..],
        &["cred_id"][..],
        &["id"][..],
    ] {
        if let Some(s) = dig(v, path).and_then(|x| x.as_str()) {
            return s.to_string();
        }
    }
    "?".into()
}

fn passkey_aaguid_from_json(v: &serde_json::Value) -> Option<String> {
    for path in &[
        &["cred", "aaguid"][..],
        &["aaguid"][..],
    ] {
        if let Some(s) = dig(v, path).and_then(|x| x.as_str()) {
            if !s.is_empty() && s != "00000000-0000-0000-0000-000000000000" {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn dig<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    Some(cur)
}
