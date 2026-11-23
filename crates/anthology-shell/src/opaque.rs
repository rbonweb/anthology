//! Opaque-construct reasons (SH2): what the expansion could not see into, per entry. Additive to Python's
//! `_Entry` (design §7.1): the guard ignores them (parity), atl's gate asks on any of them. Rust may mark more
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
//!   `echo`/`printf` in the pipeline) is known for it (`curl x | sh`); not for `--version`/`--help`.
//! - `InterpreterCode`: the program is an interpreter (`python3`, `node`, ...): its code is scanned as text
//!   only (`Entry.code`), never as commands. Not for a package manager running its own verb (`bun install`,
//!   `bun test`), though `code` keeps Python's value.
//! - `UnknownWrapperOption`: the analysis may have misread or missed a command. An option of a wrapper,
//!   package runner, package manager or `atl` was not one known to take a value or not, so it may have taken
//!   the next word; or env's script was found past env's own options or attached (`-S'...'`,
//!   `--split-string=...`); or the program runs a command the analysis does not parse
//!   (`prefixes::UNPARSED_RUNNERS`: `su -c`, `flock f cmd`, `busybox sh`, `ssh host cmd`, ...; or
//!   `hidden::runs_option_command`: `trap 'cmd' EXIT`, `fd -x`, `git -c alias.x='!cmd'`/`core.pager`,
//!   `tar --to-command`/`-I`, `rsync -e`); or an assignment names a command a program runs later
//!   (`GIT_PAGER='cmd' git log`, `BASH_ENV=f`).
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
