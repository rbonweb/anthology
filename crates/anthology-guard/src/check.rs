//! `check()`: the rule order of GC-6 (GD1), guard_rules.py `_check` and `check`.
//!
//! Order: G5 on the tool name; G6 (file tool, then command, over the entries and the alternate
//! readings); G3 on the tool input's paths, search roots and code; then per entry G3, G4, G1, G2;
//! then the same four on each alternate reading of a wrapped command (`Analysis.alternates`).
//!
//! Python turns any exception into "allow". Rust has no such path: it blocks where it cannot
//! finish (design §9.5, D-1/D-7):
//! - a backtrack limit (anthology-shell's lexer or path patterns, or this crate's
//!   `_ESCAPED_SPACE` and G6 patterns) gives [`Verdict::Incomplete`] (GC-14 limit block);
//! - an analysis anthology-shell stopped at its work budget gives [`Verdict::TooComplex`]
//!   (owner direction, FW-H: "this command is too complex to check safely", naming `/plugin`);
//! - past one of Python's caps (owner direction LR-G, fail closed; D-8 "Rust stricter"), where
//!   Python stops reading and allows: a command nested past anthology-shell's `MAX_DEPTH` (an
//!   entry marked [`Opaque::DepthCap`]), a `cd` whose directory `cd` tracking drops
//!   (`MAX_CWD_DEPTH`), or a brace expansion too large to check ([`crate::paths`]) gives
//!   [`Verdict::TooComplex`]. Where it can, Rust reads what Python skips instead: the whole tool
//!   input (no depth cap), words over 4096 code points, every brace expansion, and a home path
//!   under a relative or empty home.
//!
//! A rule that blocks on what was analyzed is reported first, in GC-6 order; only when no rule
//! blocks does an unfinished check become `Incomplete` or `TooComplex`. Either way nothing
//! unfinished is allowed.

use std::sync::LazyLock;

use anthology_pycompat::json::Value;
use anthology_shell::{
    Analysis, Entry, Limits, MAX_CWD_DEPTH, Opaque, analyze_with, normalize_with,
};

use crate::paths::Paths;
use crate::rules::g1::g1_with;
use crate::rules::g2::g2;
use crate::rules::g3::{g3, g3_input};
use crate::rules::g4::g4;
use crate::rules::g5::g5;
use crate::rules::g6::{Secret, compile_secrets, g6_command, g6_file_tool};
use crate::rules::{Limit, Rule};

/// What `check` decided.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Verdict {
    Allow,
    Block(Rule),
    /// A backtrack limit was hit: the GC-14 `(internal)` limit block.
    Incomplete,
    /// The command's analysis went over anthology-shell's work budget: blocked as too complex
    /// to check (owner direction; additive to plan §3.5).
    TooComplex,
}

/// The id shown for a [`Verdict::TooComplex`] block: `Blocked by anthology-guardrails (G0): …`
/// (FW-H's id; not a policy rule, so [`Rule`] stays G1 to G6).
pub const TOO_COMPLEX_ID: &str = "G0";

/// The message of a [`Verdict::TooComplex`] block (FW-H's `TOO_COMPLEX`, verbatim).
pub const TOO_COMPLEX: &str = "this command is too complex to check safely; simplify it or split \
     it into smaller commands (to run it as written, the user can turn the guard off with \
     `/plugin disable anthology-guardrails@anthology`)";

/// [`TOO_COMPLEX`] without its `/plugin disable` parenthetical, for a host that has no `/plugin`
/// ([`crate::host::embedded`]). [`TOO_COMPLEX`] starts with this text.
pub const TOO_COMPLEX_EMBEDDED: &str = "this command is too complex to check safely; simplify it or \
     split it into smaller commands";

/// The patterns a check compiles once per backtrack limit.
struct Regexes {
    secrets: Vec<Option<Secret>>,
}

impl Regexes {
    fn new(limits: &Limits) -> Regexes {
        Regexes {
            secrets: compile_secrets(limits.backtrack_limit),
        }
    }
}

static DEFAULT_REGEXES: LazyLock<Regexes> = LazyLock::new(|| Regexes::new(&Limits::default()));

/// `guard_rules.check(tool_name, tool_input, home)`: whether the call is allowed.
pub fn check(tool_name: &Value, tool_input: &Value, home: &str) -> Verdict {
    check_with(tool_name, tool_input, home, &Limits::default())
}

/// [`check`] with explicit limits (a test lowers `backtrack_limit`, GC-19).
pub fn check_with(tool_name: &Value, tool_input: &Value, home: &str, limits: &Limits) -> Verdict {
    check_explained(tool_name, tool_input, home, limits).0
}

/// [`check_with`], and for a [`Verdict::TooComplex`] the reason anthology-shell marked on the entries
/// of the command, when one of the reasons that [`cause_of`] names is there. A text hint for the
/// block line and nothing else: the verdict is the one [`check_with`] gives.
pub fn check_explained(
    tool_name: &Value,
    tool_input: &Value,
    home: &str,
    limits: &Limits,
) -> (Verdict, Option<Opaque>) {
    if *limits == Limits::default() {
        run(tool_name, tool_input, home, limits, &DEFAULT_REGEXES, true)
    } else {
        run(
            tool_name,
            tool_input,
            home,
            limits,
            &Regexes::new(limits),
            true,
        )
    }
}

/// [`check`] with the owner-directed D-8 parts left out: the FW-I readings of G1's own tables
/// (`g1_with(.., false)`) and LR-G's blocks past Python's caps (the depth cap, `cd` tracking, brace
/// expansion; final review I-1). Only those: the analyzer's FW readings (`fd -x`, `tar`, git aliases,
/// the unknown-option reading of a package manager and its alternates) stay on, so a generated case
/// that differs from Python by one of them still shows as unexplained (loud, not silent). Not for
/// production: the difftest's generator drops a generated case whose verdict this changes, so those
/// parts of D-8 stay case-scoped (GC-16).
pub fn check_without_d8(tool_name: &Value, tool_input: &Value, home: &str) -> Verdict {
    run(
        tool_name,
        tool_input,
        home,
        &Limits::default(),
        &DEFAULT_REGEXES,
        false,
    )
    .0
}

/// Tracks the first blocking rule and whether some step could not finish.
#[derive(Default)]
struct Outcome {
    limited: bool,
}

impl Outcome {
    /// `Some(rule)` when `result` blocks; a limit is remembered.
    fn hit(&mut self, rule: Rule, result: Result<bool, Limit>) -> Option<Verdict> {
        match result {
            Ok(true) => Some(Verdict::Block(rule)),
            Ok(false) => None,
            Err(Limit) => {
                self.limited = true;
                None
            }
        }
    }
}

fn run(
    tool_name: &Value,
    tool_input: &Value,
    home: &str,
    limits: &Limits,
    regexes: &Regexes,
    fw: bool,
) -> (Verdict, Option<Opaque>) {
    let name = tool_name.as_str().unwrap_or("");
    let empty = Value::Object(Default::default());
    let input = match tool_input {
        Value::Object(_) => tool_input,
        _ => &empty,
    };
    let command = input
        .as_object()
        .and_then(|o| o.get("command"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());

    if g5(name) {
        return (Verdict::Block(Rule::G5), None);
    }
    let analysis = match command {
        Some(c) => analyze_with(c, home, limits),
        None => Analysis::default(),
    };
    let paths = Paths::new(home, limits, fw);
    let mut out = Outcome::default();

    let g6 = crate::rules::either(
        g6_file_tool(input, &paths, &regexes.secrets),
        || match command {
            // the alternate readings too (review-gd-final I-1): the shipped reading may write memory
            Some(c) => g6_command(
                c,
                analysis.entries.iter().chain(&analysis.alternates),
                &paths,
                &regexes.secrets,
            ),
            None => Ok(false),
        },
    );
    if let Some(v) = out.hit(Rule::G6, g6) {
        return (v, None);
    }
    if let Some(v) = out.hit(Rule::G3, g3_input(name, input, &paths)) {
        return (v, None);
    }
    // Every entry, then each other reading of a wrapped command (`Analysis.alternates`), against
    // G3, G4, G1 and G2 (final review C-1, I-3): an alternate is a reading the shipped Python may
    // take, so a rule it hits blocks.
    for entry in analysis.entries.iter().chain(&analysis.alternates) {
        if let Some(v) = out.hit(Rule::G3, g3(entry, &paths)) {
            return (v, None);
        }
        if g4(entry) {
            return (Verdict::Block(Rule::G4), None);
        }
        if g1_with(entry, fw) {
            return (Verdict::Block(Rule::G1), None);
        }
        if g2(entry) {
            return (Verdict::Block(Rule::G2), None);
        }
    }
    let regex_limit = analysis
        .entries
        .iter()
        .any(|e| e.opaque.contains(&Opaque::RegexLimit));
    let mut capped = paths.capped.get();
    if fw && !capped {
        for entry in analysis.entries.iter().chain(&analysis.alternates) {
            match past_a_cap(entry, home, limits) {
                Ok(true) => capped = true,
                Ok(false) => {}
                Err(Limit) => out.limited = true,
            }
        }
    }
    if out.limited || (analysis.incomplete && regex_limit) {
        (Verdict::Incomplete, None)
    } else if analysis.incomplete || capped {
        let cause = analysis.incomplete.then(|| cause_of(&analysis)).flatten();
        (Verdict::TooComplex, cause)
    } else {
        (Verdict::Allow, None)
    }
}

/// The reasons [`check_explained`] can name, most likely cause first. Each is one anthology-shell
/// marked on an entry, and each has a hint in [`crate::hook`]. Not every marked entry is the cause: a
/// `source` or a shell reading stdin is often allowed, so with an unrelated reason beside it the hint
/// is a false one (text only; the verdict does not depend on it).
const CAUSES: [&[Opaque]; 4] = [
    &[Opaque::DynamicCommandWord],
    &[Opaque::Eval, Opaque::ShellScriptFromExpansion],
    &[Opaque::Source],
    &[Opaque::StdinShellUnreadable],
];

/// The first of [`CAUSES`] that some entry (or alternate reading) of `analysis` carries.
fn cause_of(analysis: &Analysis) -> Option<Opaque> {
    CAUSES.iter().find_map(|group| {
        group.iter().copied().find(|reason| {
            analysis
                .entries
                .iter()
                .chain(&analysis.alternates)
                .any(|e| e.opaque.contains(reason))
        })
    })
}

/// Whether anthology-shell stopped reading `entry` at one of Python's caps: a script nested past
/// `MAX_DEPTH` ([`Opaque::DepthCap`]), or a `cd` whose new directory has more than
/// `MAX_CWD_DEPTH` slashes, which `cd` tracking does not follow (the later words would resolve
/// against the old directory).
fn past_a_cap(entry: &Entry, home: &str, limits: &Limits) -> Result<bool, Limit> {
    if entry.opaque.contains(&Opaque::DepthCap) {
        return Ok(true);
    }
    if entry.prog != "cd" && entry.prog != "pushd" {
        return Ok(false);
    }
    match crate::rules::first_arg(&entry.args) {
        None | Some("-") => Ok(false),
        Some(target) => {
            let dir =
                normalize_with(target, home, entry.cwd.as_deref(), limits).map_err(|_| Limit)?;
            Ok(dir.matches('/').count() > MAX_CWD_DEPTH)
        }
    }
}
