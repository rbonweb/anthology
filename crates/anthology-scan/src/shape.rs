//! Command shapes and the trigger by shape severity matrix (Task 3). The stubs return plain values.

use crate::{Severity, Trigger};

/// What a command does, from its text. The order is the severity order: the highest match wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    Allowlisted,
    Plain,
    Runner,
    Network,
    DecodeExec,
    FetchExec,
    RunsData,
}

/// The shape of `cmd`.
pub fn classify(_cmd: &str) -> Shape {
    Shape::Plain
}

/// The files `cmd` runs: the first path argument after an interpreter, or a leading `./x` or
/// `x.sh`.
pub fn files_run(_cmd: &str) -> Vec<String> {
    Vec::new()
}

/// The script name of `npm run X` and its kin, if `cmd` is one.
pub fn npm_run_target(_cmd: &str) -> Option<String> {
    None
}

/// The severity of a command of shape `s` that runs on `t`; `reached` is the highest severity
/// among the findings in the files it runs.
pub fn severity(_t: Trigger, _s: Shape, _reached: Option<Severity>) -> Severity {
    Severity::Info
}
