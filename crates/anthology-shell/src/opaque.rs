//! Opaque-construct reasons (SH2): what the expansion could not see into, per entry. Additive to Python's
//! `_Entry` (design §7.1): the guard ignores them (parity), rtk's gate asks on any of them. Each reason is set
//! on the entry it concerns:
//!
//! - `DynamicCommandWord`: the program word is computed at run time — a `$(...)`/backtick placeholder was
//!   dropped where the program was expected, or the program word holds `$`, a backtick or a glob (`*?[`).
//! - `ShellScriptFromExpansion`: a shell's script is computed at run time — its `-c` script (or `eval`'s
//!   text) is empty or holds `$` or a backtick, or a shell runs a script file whose name does.
//! - `Eval`: the program is `eval` (its words are re-parsed by the shell; analyzed as one script).
//! - `Source`: the program is `source` or `.`: it runs a file the analysis cannot read.
//! - `DepthCap`: something nested under this entry (or in the text it came from) was not analyzed because
//!   `MAX_DEPTH` (or the `find -exec` nesting cap) was reached.
//! - `StdinShellUnreadable`: a shell reads its script from stdin and no literal text (heredoc, here-string,
//!   `echo`/`printf` in the pipeline) is known for it (`curl x | sh`).
//! - `InterpreterCode`: the program is an interpreter (`python3`, `node`, ...): its code is scanned as text
//!   only (`Entry.code`), never as commands.
//! - `UnknownWrapperOption`: a wrapper's option was not one the analysis knows to take a value, so the word
//!   read as the program may be that option's value.
//! - `Unbalanced`: the text the entry came from had an unbalanced quote or substitution (lenient split).
//! - `RegexLimit`: a regex backtrack limit was hit while analyzing the text the entry came from
//!   (`Analysis.incomplete` is then set too).

/// Why an entry is opaque (plan §3.4).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Opaque {
    DynamicCommandWord,
    ShellScriptFromExpansion,
    Eval,
    Source,
    DepthCap,
    StdinShellUnreadable,
    InterpreterCode,
    UnknownWrapperOption,
    Unbalanced,
    RegexLimit,
}

/// A word whose value is only known at run time.
pub(crate) fn is_dynamic(word: &str) -> bool {
    word.contains(['$', '`', '*', '?', '['])
}

/// Add `reason` once.
pub(crate) fn mark(opaque: &mut Vec<Opaque>, reason: Opaque) {
    if !opaque.contains(&reason) {
        opaque.push(reason);
    }
}
