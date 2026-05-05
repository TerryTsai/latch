// Output discipline: typed errors with exit codes, TTY/color detection,
// human vs JSON emission. Used by every subcommand.
//
// Convention: data on stdout, diagnostics on stderr. Color only when
// stdout is a TTY, NO_COLOR is unset, and --no-color was not passed.

#![allow(dead_code)] // wired up across phases 3-4

use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

// --- exit codes (sysexits.h) ----------------------------------------------

pub const EX_OK:        i32 = 0;
pub const EX_FAIL:      i32 = 1;
pub const EX_USAGE:     i32 = 64;
pub const EX_CANTCREAT: i32 = 73;
pub const EX_CONFIG:    i32 = 78;

// --- error type -----------------------------------------------------------

pub struct Error {
    pub msg:  String,
    pub hint: Option<String>,
    pub code: i32,
}

impl Error {
    pub fn usage<S: Into<String>>(msg: S)      -> Self { Self { msg: msg.into(), hint: None, code: EX_USAGE } }
    pub fn config<S: Into<String>>(msg: S)     -> Self { Self { msg: msg.into(), hint: None, code: EX_CONFIG } }
    pub fn cantcreat<S: Into<String>>(msg: S)  -> Self { Self { msg: msg.into(), hint: None, code: EX_CANTCREAT } }
    pub fn fail<S: Into<String>>(msg: S)       -> Self { Self { msg: msg.into(), hint: None, code: EX_FAIL } }

    pub fn with_hint<S: Into<String>>(mut self, hint: S) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn print(&self) {
        eprintln!("error: {}", self.msg);
        if let Some(h) = &self.hint {
            eprintln!("  hint: {h}");
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.msg) }
}

impl From<String> for Error {
    fn from(s: String) -> Self { Self::fail(s) }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self { Self::fail(s.to_string()) }
}

pub type Result<T> = std::result::Result<T, Error>;

// --- TTY + color ----------------------------------------------------------

extern "C" { fn isatty(fd: i32) -> i32; }

pub fn is_tty(fd: i32) -> bool {
    // SAFETY: isatty has no preconditions; returns 0 or 1.
    unsafe { isatty(fd) == 1 }
}

static NO_COLOR_FLAG: AtomicBool = AtomicBool::new(false);

pub fn set_no_color(v: bool) { NO_COLOR_FLAG.store(v, Ordering::Relaxed); }

pub fn color_enabled() -> bool {
    if NO_COLOR_FLAG.load(Ordering::Relaxed) { return false; }
    if std::env::var_os("NO_COLOR").is_some() { return false; }
    is_tty(1)
}

const GREEN: &str = "\x1b[32m";
const RED:   &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

pub fn tick() -> String {
    if color_enabled() { format!("{GREEN}\u{2713}{RESET}") } else { "ok".into() }
}

pub fn cross() -> String {
    if color_enabled() { format!("{RED}\u{2717}{RESET}") } else { "FAIL".into() }
}

// --- machine-readable mode -----------------------------------------------

static JSON_FLAG:  AtomicBool = AtomicBool::new(false);
static QUIET_FLAG: AtomicBool = AtomicBool::new(false);

pub fn set_json(v: bool)  { JSON_FLAG.store(v, Ordering::Relaxed); }
pub fn set_quiet(v: bool) { QUIET_FLAG.store(v, Ordering::Relaxed); }

// JSON mode is explicit (--json) or implicit (stdout is not a TTY).
// Subcommands that produce structured data (check, passkeys list,
// config show) consult this to pick their renderer.
pub fn json_mode() -> bool {
    JSON_FLAG.load(Ordering::Relaxed) || !is_tty(1)
}

pub fn quiet() -> bool { QUIET_FLAG.load(Ordering::Relaxed) }

pub fn note(msg: &str) {
    if !quiet() { eprintln!("{msg}"); }
}

// --- json writer ----------------------------------------------------------

// Print a serde_json::Value to stdout with a trailing newline. Flushes so
// piped consumers see output promptly.
pub fn emit_json(v: &serde_json::Value) {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_with_hint_carries_both() {
        let e = Error::config("bad").with_hint("try this");
        assert_eq!(e.msg, "bad");
        assert_eq!(e.hint.as_deref(), Some("try this"));
        assert_eq!(e.code, EX_CONFIG);
    }

    #[test]
    fn from_string_is_generic_failure() {
        let e: Error = "boom".to_string().into();
        assert_eq!(e.code, EX_FAIL);
    }

    #[test]
    fn no_color_flag_disables_color() {
        set_no_color(true);
        assert!(!color_enabled());
        set_no_color(false);
    }
}
