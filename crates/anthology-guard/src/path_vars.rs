//! Host path variables (carve-out d, ADR-038): the variables the user lists in
//! `ANTHOLOGY_GUARD_PATH_VARS` that the guard may read a `"$NAME"` word through.
//!
//! An app that embeds Claude Code may tell its agent to run its tool as `"$TOOL" "$SCRIPT" …`, with
//! the variables set in Claude Code's environment. The guard sees the hook's environment, not the
//! shell's, and the two can differ (a `CLAUDE_ENV_FILE` export or a shell startup file can change
//! the shell's), so nothing is read unless the user lists the name: listing it is the user saying
//! that nothing changes it on the way to the shell (`SECURITY.md`).
//!
//! The module is not re-exported at the crate root; use `anthology_guard::path_vars::HostPathVars`.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_shell::{DIRECTORY_PREFIX_NAMES, PathValue, PathVars};

/// The environment variable that lists the host path variables, comma-separated, with no blanks.
pub const PATH_VARS_VAR: &str = "ANTHOLOGY_GUARD_PATH_VARS";

/// Names that change how a shell runs or finds things, or that a shell sets itself from what a command
/// reads (`_`, `REPLY`, `OPTARG`, `MATCH`, `MBEGIN`, `MEND`, `MAPFILE`), so a command can change them
/// without spelling them. Carve-out (a)'s names are denied too ([`DIRECTORY_PREFIX_NAMES`]), and
/// `BASH_ENV` is one of the `BASH_*` names ([`DENIED_PREFIXES`]).
const DENIED: &[&str] = &[
    "PATH", "IFS", "PWD", "OLDPWD", "SHELL", "ENV", "ZDOTDIR", "CDPATH", "FPATH", "PS1", "PS2",
    "PS3", "PS4", "_", "REPLY", "OPTARG", "MATCH", "MBEGIN", "MEND", "MAPFILE",
];

/// Name prefixes denied: prompts, the loaders' variables and the shells' own.
const DENIED_PREFIXES: &[&str] = &["PROMPT", "LD_", "DYLD_", "BASH_", "ZSH_"];

/// The system's temp folders; the environment's `TMPDIR`, `TMP`, `TEMP` and `TEMPDIR` are added.
const TEMP_DIRS: &[&str] = &["/tmp", "/var/tmp", "/dev/shm", "/var/folders"];

/// The environment variables that name a temp folder.
const TEMP_VARS: &[&str] = &["TMPDIR", "TMP", "TEMP", "TEMPDIR"];

/// The host path variables that passed every check, read once when the hook starts.
#[derive(Clone, Debug, Default)]
pub struct HostPathVars {
    values: BTreeMap<String, PathValue>,
}

impl HostPathVars {
    /// The listed variables of `env` (a lookup in the hook's environment) whose names and values pass
    /// every check, with `cwd` the hook's working folder. `None` when none does, or when
    /// [`PATH_VARS_VAR`] is unset: the guard then reads every command as it did before.
    ///
    /// A name passes when it matches `^[A-Z_][A-Z0-9_]*$` and is not denied; its value when it is an
    /// absolute path of `[A-Za-z0-9_./+-]` with no `..` that names a regular file whose canonical path
    /// keeps to the same pattern, and neither the value nor the canonical path lies in a temp folder
    /// or under `cwd` (each folder as written and resolved). No `cwd` reads nothing.
    pub fn new(env: &dyn Fn(&str) -> Option<String>, cwd: Option<&Path>) -> Option<HostPathVars> {
        let list = env(PATH_VARS_VAR)?;
        let cwd = cwd?;
        let mut outside: Vec<PathBuf> = TEMP_DIRS.iter().map(PathBuf::from).collect();
        outside.extend(TEMP_VARS.iter().filter_map(|v| env(v)).map(PathBuf::from));
        outside.push(cwd.to_path_buf());
        let resolved: Vec<PathBuf> = outside
            .iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect();
        outside.extend(resolved);
        let values: BTreeMap<String, PathValue> = list
            .split(',')
            .filter(|name| name_allowed(name))
            .filter_map(|name| {
                let value = read(&env(name)?, &outside)?;
                Some((name.to_string(), value))
            })
            .collect();
        (!values.is_empty()).then_some(HostPathVars { values })
    }
}

impl PathVars for HostPathVars {
    fn resolve(&self, name: &str) -> Option<PathValue> {
        self.values.get(name).cloned()
    }
}

/// `^[A-Z_][A-Z0-9_]*$`, not [`DENIED`], not [`DIRECTORY_PREFIX_NAMES`], with no [`DENIED_PREFIXES`].
fn name_allowed(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == b'_')
        && bytes.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        && !DENIED.contains(&name)
        && !DIRECTORY_PREFIX_NAMES.contains(&name)
        && !DENIED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// `^/[A-Za-z0-9_./+-]*$` with no `..` (`/` alone is a folder, never read).
fn value_allowed(value: &str) -> bool {
    value.starts_with('/')
        && !value.contains("..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_./+-".contains(&c))
}

/// `value` read as a regular file that lies outside every folder of `outside`, or `None`.
fn read(value: &str, outside: &[PathBuf]) -> Option<PathValue> {
    if !value_allowed(value) {
        return None;
    }
    let canonical = std::fs::canonicalize(value).ok()?;
    let canonical_text = canonical.to_str().filter(|c| value_allowed(c))?.to_string();
    let meta = std::fs::metadata(&canonical).ok()?;
    let inside = |p: &Path| outside.iter().any(|dir| p.starts_with(dir));
    if !meta.is_file() || inside(Path::new(value)) || inside(&canonical) {
        return None;
    }
    Some(PathValue {
        value: value.to_string(),
        canonical: canonical_text,
        executable: meta.permissions().mode() & 0o111 != 0,
    })
}
