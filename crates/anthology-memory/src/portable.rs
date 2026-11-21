//! Memory export and import (MX1): `memory_portable.py` and `anthology memory export|import`
//! (GC-12, GC-5, GC-4). Import validation, which never extracts the archive, is [`load`].
//!
//! A plan is the JSON object `plan_import` returns (`json::Value::Object`, the Python dict,
//! key for key): `apply_import` takes any value, as Python takes any object, and re-validates
//! it, so hand-forged plans are tested the way Python tests them.
#![allow(dead_code, reason = "skeleton: MX1-C fills the bodies")]

pub mod load;

use std::ffi::OsString;
use std::path::Path;

use anthology_core::Env;
use anthology_core::git::Git;
use anthology_pycompat::json::{Map, Value};
use anthology_pycompat::pyerr::PyExc;

/// What the Python functions raise, by class.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// `ImportRejected(msg)`.
    Rejected(String),
    /// `ExportRefused(msg)`.
    Refused(String),
    /// `ValueError(msg)`.
    Value(String),
    /// An `OSError`.
    Os(PyExc),
}

impl From<load::Rejected> for Error {
    fn from(r: load::Rejected) -> Self {
        Error::Rejected(r.0)
    }
}

/// `export(cwd, out_dir, today, env)`: `{"path", "counts", "secrets", "repo_included"}`.
pub fn export(
    cwd: &Path,
    out_dir: Option<&Path>,
    today: Option<&str>,
    env: &Env,
    git: &dyn Git,
) -> Result<Map, Error> {
    let _ = (cwd, out_dir, today, env, git);
    todo!("MX1-C")
}

/// `plan_import(archive, cwd, today, env)`: the plan object. Writes nothing.
pub fn plan_import(
    archive: &Path,
    cwd: &Path,
    today: Option<&str>,
    env: &Env,
    git: &dyn Git,
) -> Result<Map, Error> {
    let _ = (archive, cwd, today, env, git);
    todo!("MX1-C")
}

/// `apply_import(plan)`: `{"written", "conflicts", "skipped", "blocked", "indexes"}`.
pub fn apply_import(plan: &Value, git: &dyn Git) -> Result<Map, Error> {
    let _ = (plan, git);
    todo!("MX1-C")
}

/// What `main` printed and returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// `main(argv)` with `prog` `memory_portable.py` replaced as the CLI shows it; `cwd` is
/// `os.getcwd()`.
pub fn main(args: &[OsString], cwd: &Path, env: &Env, git: &dyn Git) -> Output {
    let _ = (args, cwd, env, git);
    todo!("MX1-C")
}
