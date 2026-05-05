// Persistent state: passkeys, signing key, revoked-token denylist.
//
// All three live under the configured data_dir. Files are written
// atomically via tmp+rename. The signing key is mode 0600.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use webauthn_rs::prelude::Passkey;

// --- passkeys --------------------------------------------------------------

pub fn load_passkeys(path: &Path) -> Vec<Passkey> {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_passkeys(passkeys: &[Passkey], path: &Path) -> std::io::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(passkeys)?)
}

// --- signing key -----------------------------------------------------------

pub fn load_or_create_key(path: &Path) -> Vec<u8> {
    if let Ok(bytes) = fs::read(path) {
        if bytes.len() == 32 { return bytes; }
        eprintln!("warning: {} has unexpected size; regenerating", path.display());
    }
    let mut buf = [0u8; 32];
    fs::File::open("/dev/urandom").expect("open /dev/urandom")
        .read_exact(&mut buf).expect("read /dev/urandom");
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, buf).expect("write key tmp");
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).expect("chmod key");
    fs::rename(&tmp, path).expect("rename key");
    eprintln!("generated new signing key at {}", path.display());
    buf.to_vec()
}

// --- revoked token denylist ------------------------------------------------

pub type Revoked = HashMap<String, u64>; // jti -> exp_unix_seconds

pub fn load_revoked(path: &Path) -> Revoked {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_revoked(rev: &Revoked, path: &Path) -> std::io::Result<()> {
    write_atomic(path, &serde_json::to_vec(rev)?)
}

// --- atomic write helper ---------------------------------------------------

fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(content)?;
    f.sync_all()?;
    fs::rename(tmp, path)
}
