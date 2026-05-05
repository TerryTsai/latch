// Subcommand implementations. Dispatched from main.rs after parsing.
//
// Conventions every cmd follows:
//   - data on stdout, diagnostics on stderr
//   - --json mode (or non-TTY stdout) emits a single JSON value
//   - errors return crate::output::Error with a sysexits-style code

pub mod check;
pub mod config;
pub mod passkeys;
pub mod service;
pub mod update;
