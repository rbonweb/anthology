//! `analyze()` (SH2): guard_rules.py `_analyze` and `_Entry`. The command is lexed, prefixes are peeled off
//! each segment, and everything that runs (nested shells, substitutions, heredocs fed to a shell, `eval`,
//! `find -exec`, docker shells, rtk runners) is flattened into entries in execution order, with the `cd`
//! directory tracked.
//!
//! A heredoc body inside `$(...)` can be read three ways (bash 5, bash 3.2, and any parenthesis ending it;
//! `heredoc::comsub_cut`). When a line where they differ is seen, the command is analyzed again with each other
//! reading, as Python's `_check` does, and the entries of every pass are kept (Python stops at the first pass a
//! rule blocks; the union is what each rule sees across the passes). All passes share one work budget.

use crate::collect::{
    Ctx, calls_only_direct, changes_prefixes, evaluates_values, sets_positionals, unseen_values,
    with_path_regexes,
};
use crate::lexer::{Group, GroupId, Reading, Redirect, Work};
use crate::limits::{Limits, extra_limit, work_limit};
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
/// `incomplete`: the work budget ran out (or a path regex hit its backtrack limit); the guard blocks, "this
/// command is too complex to check safely" (design §9.5).
///
/// `alternates` (fix-wave I, lead ruling Q3=A): other readings of a wrapped command whose option values the
/// tables may have misread (`npx --registry X playwright install` read as `X playwright install`). Each differs
/// from the entry it came from, which is marked `UnknownWrapperOption`. The guard checks them against G1 only.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Analysis {
    pub entries: Vec<Entry>,
    pub groups: Vec<Group>,
    pub incomplete: bool,
    pub alternates: Vec<Entry>,
}

/// `analyze()` with explicit limits (a test lowers `backtrack_limit`).
pub fn analyze_with(command: &str, home: &str, limits: &Limits) -> Analysis {
    with_path_regexes(limits, |re| {
        let work = Work::new(work_limit(command), extra_limit(command), Reading::Bash5);
        let mut ctx = Ctx::new(home, re, work);
        ctx.collect(command, false, 0);
        for reading in [Reading::Exact, Reading::Wide] {
            if ctx.halted() || !ctx.work.paren_lines {
                break;
            }
            ctx.next_pass(reading);
            ctx.collect(command, false, 0);
        }
        // PE-1 to PE-6: a command it cannot prove; PE-1: positional parameters read in place but set some
        // other way, or a function so read run some other way; carve-out (a): a directory prefix read through
        // but changed; PE-4: a value the analysis cannot see, evaluated as an expression or subscript, may run
        // a command
        let unproven = ctx.unproven
            || (ctx.positional && sets_positionals(&ctx.entries))
            || !calls_only_direct(&ctx.entries, &ctx.alternates, &ctx.function_calls)
            || (!ctx.prefix_names.is_empty()
                && changes_prefixes(&ctx.entries, &ctx.arith_texts, &ctx.prefix_names))
            || (unseen_values(&ctx.entries)
                && evaluates_values(&ctx.entries, &ctx.arith_texts, ctx.held_entries()));
        Analysis {
            entries: ctx.entries,
            groups: ctx.groups,
            incomplete: ctx.incomplete || unproven,
            alternates: ctx.alternates,
        }
    })
}

/// Flatten `command` into the entries that run (guard_rules.py `_analyze(command, home)`).
pub fn analyze(command: &str, home: &str) -> Analysis {
    analyze_with(command, home, &Limits::default())
}
