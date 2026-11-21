//! Opaque-construct reasons (SH2): what the expansion could not see into, per entry. Additive to Python's
//! `_Entry` (design §7.1): the guard ignores them (parity), rtk's gate asks on any of them. Rust may mark more
//! than Python sees. Each reason is set on the entry it concerns:
//!
//! - `DynamicCommandWord`: the program is only known at run time. A `$(...)`/backtick placeholder was dropped
//!   where the program was expected; or the program word holds `$`, a backtick or a glob (`*?[`; the test
//!   commands `[` and `[[` excepted); or the command's words come from stdin (`xargs`, `parallel`); or a
//!   wrapper or runner was peeled and left no program and no script (`... | xargs npx`).
//! - `ShellScriptFromExpansion`: a shell's script is computed at run time — its `-c` script (or `eval`'s
//!   text) is empty or holds `$` or a backtick, or a shell runs a script file whose name does.
//! - `Eval`: the program is `eval` (its words are re-parsed by the shell; analyzed as one script).
//! - `Source`: the program runs a file the analysis cannot read: `source`/`.` with an argument, or a shell
//!   given a script file (`bash ./install.sh`).
//! - `DepthCap`: something nested under this entry (or in the text it came from) was not analyzed because
//!   `MAX_DEPTH` (or the `find -exec` nesting cap) was reached.
//! - `StdinShellUnreadable`: a shell reads its script from stdin and no literal text (heredoc, here-string,
//!   `echo`/`printf` in the pipeline) is known for it (`curl x | sh`).
//! - `InterpreterCode`: the program is an interpreter (`python3`, `node`, ...): its code is scanned as text
//!   only (`Entry.code`), never as commands.
//! - `UnknownWrapperOption`: the analysis may have misread which word is the program. An option of a wrapper,
//!   package runner, package manager or `rtk` was not one known to take a value or not, so it may have taken
//!   the next word; or `env -S` was found past env's own options; or the program runs a command the analysis
//!   does not parse (`prefixes::UNPARSED_RUNNERS`: `su -c`, `flock f cmd`, `busybox sh`, `ssh host cmd`, ...).
//! - `Unbalanced`: the text the entry came from had an unbalanced quote or substitution (lenient split).
//! - `RegexLimit`: a regex backtrack limit was hit while lexing the text the entry came from
//!   (`Analysis.incomplete` is then set too). A work-budget stop sets only `Analysis.incomplete`.

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

/// The test commands, whose names hold `[` but are not globs.
pub(crate) const TEST_BRACKETS: &[&str] = &["[", "[["];

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
