//! `analyze()` (SH2): guard_rules.py `_analyze` and `_Entry`. The command is lexed, prefixes are peeled off
//! each segment, and everything that runs (nested shells, substitutions, heredocs fed to a shell, `eval`,
//! `find -exec`, docker shells, rtk runners) is flattened into entries in execution order, with the `cd`
//! directory tracked.

use crate::collect::{Ctx, with_path_regexes};
use crate::lexer::{Group, GroupId, Redirect};
use crate::limits::Limits;
use crate::opaque::Opaque;

/// A flattened command ready for the per-rule predicates (`_Entry`, field for field, plus `opaque`).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Entry {
    /// Every word, wrappers included.
    pub raw: Vec<String>,
    pub redirs: Vec<Redirect>,
    /// The program word, lowercased.
    pub prog_path: String,
    /// `prog_name` of the program word.
    pub prog: String,
    pub args: Vec<String>,
    /// The tracked `cd` directory, normalized (`None` until a `cd`).
    pub cwd: Option<String>,
    /// docker: G1 and G2 do not apply.
    pub exempt: bool,
    pub group: GroupId,
    /// Inline code an interpreter runs: its arguments, plus text fed to its stdin.
    pub code: Vec<String>,
    /// What the analysis could not see into (additive; the guard ignores it).
    pub opaque: Vec<Opaque>,
}

/// `analyze()`'s result. `groups` holds every group an entry may reference (ids unique, ascending).
/// `incomplete`: a regex backtrack limit or a work bound was hit; the guard blocks (design §9.5).
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Analysis {
    pub entries: Vec<Entry>,
    pub groups: Vec<Group>,
    pub incomplete: bool,
}

/// `analyze()` with explicit limits (a test lowers `backtrack_limit`).
pub fn analyze_with(command: &str, home: &str, limits: &Limits) -> Analysis {
    with_path_regexes(limits, |re| {
        let mut ctx = Ctx::new(home, limits, re);
        ctx.collect(command, false, 0);
        Analysis {
            entries: ctx.entries,
            groups: ctx.groups,
            incomplete: ctx.incomplete,
        }
    })
}

/// Flatten `command` into the entries that run (guard_rules.py `_analyze(command, home)`).
pub fn analyze(command: &str, home: &str) -> Analysis {
    analyze_with(command, home, &Limits::default())
}
