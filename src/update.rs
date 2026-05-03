// `latch update` — fetch the latest release, verify sha256, replace this
// binary atomically, restart if running under systemd.
//
// Shells out to curl and tar (both ubiquitous on Linux); uses OpenSSL
// (already vendored) for SHA-256.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use openssl::hash::{Hasher, MessageDigest};

use crate::config::Mode;
use crate::lifecycle;

const REPO: &str = "TerryTsai/latch";
const TARGET: &str = "x86_64-unknown-linux-musl";

pub fn run() -> Result<(), String> {
    require_writable_binary()?;
    let current = env!("CARGO_PKG_VERSION");
    let latest = fetch_latest_tag()?;
    let latest_ver = latest.trim_start_matches('v');

    if current == latest_ver {
        eprintln!("already up to date ({current})");
        return Ok(());
    }
    eprintln!("updating: {current} → {latest_ver}");

    let tarball_url = format!(
        "https://github.com/{REPO}/releases/download/{latest}/latch-{TARGET}.tar.gz"
    );
    let sha_url = format!("{tarball_url}.sha256");

    eprintln!("downloading {tarball_url}");
    let tarball = curl_bytes(&tarball_url)?;
    let sha_text = curl_text(&sha_url)?;
    let expected = sha_text.split_whitespace().next()
        .ok_or("malformed sha256 file")?
        .to_lowercase();
    let actual = sha256_hex(&tarball)?;
    if expected != actual {
        return Err(format!("sha256 mismatch: expected {expected}, got {actual}"));
    }
    eprintln!("sha256 verified");

    let tmp = tempdir()?;
    let tar_path = tmp.join("latch.tar.gz");
    fs::write(&tar_path, &tarball).map_err(|e| format!("write tarball: {e}"))?;

    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&tar_path)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() { return Err("tar failed".into()); }

    let new_bin = tmp.join("latch");
    if !new_bin.exists() {
        return Err("tarball didn't contain `latch`".into());
    }

    let current_bin = std::env::current_exe()
        .map_err(|e| format!("locate own binary: {e}"))?;
    let staged = current_bin.with_extension("new");
    fs::copy(&new_bin, &staged).map_err(|e| format!("copy: {e}"))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("chmod: {e}"))?;
    fs::rename(&staged, &current_bin).map_err(|e| format!("rename: {e}"))?;
    eprintln!("replaced {} with {latest_ver}", current_bin.display());

    let _ = fs::remove_dir_all(&tmp);

    let mode = Mode::detect();
    if lifecycle::is_unit_active(mode) {
        eprintln!("restarting systemd service...");
        let mut cmd = Command::new("systemctl");
        if mode == Mode::User { cmd.arg("--user"); }
        cmd.args(["restart", "latch.service"])
            .status()
            .map_err(|e| format!("restart: {e}"))?;
    } else {
        eprintln!("not running under systemd; restart manually if needed.");
    }

    Ok(())
}

fn require_writable_binary() -> Result<(), String> {
    let bin = std::env::current_exe().map_err(|e| e.to_string())?;
    let euid = unsafe { getuid() };
    if euid == 0 { return Ok(()); }
    let metadata = fs::metadata(&bin).map_err(|e| format!("stat: {e}"))?;
    let owned = std::os::unix::fs::MetadataExt::uid(&metadata) == euid;
    let world_writable = metadata.permissions().mode() & 0o002 != 0;
    if !owned && !world_writable {
        return Err(format!(
            "can't replace {} as the current user.\nrun:\n    sudo latch update",
            bin.display(),
        ));
    }
    Ok(())
}

fn fetch_latest_tag() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl_text(&url)?;
    let tag = body
        .split("\"tag_name\":")
        .nth(1)
        .and_then(|s| s.split('"').nth(1))
        .ok_or("no tag_name in API response")?;
    Ok(tag.to_string())
}

fn curl_bytes(url: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--tlsv1.2"])
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl {url}: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(out.stdout)
}

fn curl_text(url: &str) -> Result<String, String> {
    let bytes = curl_bytes(url)?;
    String::from_utf8(bytes).map_err(|e| format!("response not utf-8: {e}"))
}

fn sha256_hex(bytes: &[u8]) -> Result<String, String> {
    let mut h = Hasher::new(MessageDigest::sha256()).map_err(|e| e.to_string())?;
    h.update(bytes).map_err(|e| e.to_string())?;
    let digest = h.finish().map_err(|e| e.to_string())?;
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn tempdir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    let name = format!("latch-update-{}", std::process::id());
    let dir = base.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir tmp: {e}"))?;
    Ok(dir)
}

extern "C" {
    fn getuid() -> u32;
}
