//! `anthology-installer`: one command that installs Anthology through the official `claude plugin`
//! commands, then the guard binary through the plugin's own `hooks/bootstrap.sh`.
//!
//! It ships no copy of the binary or the plugins, never writes `settings.json` itself and
//! never pipes a download into a shell. Everything it changes, Claude Code or the anthology binary
//! changes for it.
#![forbid(unsafe_code)]

pub mod cli;
pub mod platform;
pub mod proc;
pub mod report;
