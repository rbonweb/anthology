//! `check()`: the rule order of GC-6 (GD1), guard_rules.py `_check` and `check`.
//!
//! Order: G5 on the tool name; G6 (file tool, then command); G3 on the tool input's paths,
//! search roots and code; then per entry G3, G4, G1, G2.
//!
//! Python turns any exception into "allow". Rust has no such path: it blocks where it cannot
//! finish (design §9.5, D-1/D-7):
//! - a backtrack limit (anthology-shell's lexer or path patterns, or this crate's
//!   `_ESCAPED_SPACE` and G6 patterns) gives [`Verdict::Incomplete`] (GC-14 limit block);
//! - an analysis anthology-shell stopped at its work budget gives [`Verdict::TooComplex`]
//!   (owner direction, FW-H: "this command is too complex to check safely", naming `/plugin`).
//!
//! A rule that blocks on what was analyzed is reported first, in GC-6 order; only when no rule
//! blocks does an unfinished check become `Incomplete` or `TooComplex`. Either way nothing
//! unfinished is allowed.

use std::sync::LazyLock;

use anthology_pycompat::json::Value;
use anthology_shell::{Analysis, Limits, Opaque, analyze_with};

use crate::paths::{PathRegexes, Paths};
use crate::rules::g1::g1;
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

/// The patterns a check compiles once per backtrack limit.
struct Regexes {
    paths: PathRegexes,
    secrets: Vec<Option<Secret>>,
}

impl Regexes {
    fn new(limits: &Limits) -> Regexes {
        Regexes {
            paths: PathRegexes::new(limits),
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
    if *limits == Limits::default() {
        run(tool_name, tool_input, home, limits, &DEFAULT_REGEXES)
    } else {
        run(tool_name, tool_input, home, limits, &Regexes::new(limits))
    }
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
) -> Verdict {
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
        return Verdict::Block(Rule::G5);
    }
    let analysis = match command {
        Some(c) => analyze_with(c, home, limits),
        None => Analysis::default(),
    };
    let paths = Paths::new(home, limits, &regexes.paths);
    let mut out = Outcome::default();

    let g6 = crate::rules::either(g6_file_tool(input, &paths, &regexes.secrets), || match command {
        Some(c) => g6_command(c, &analysis.entries, &paths, &regexes.secrets),
        None => Ok(false),
    });
    if let Some(v) = out.hit(Rule::G6, g6) {
        return v;
    }
    if let Some(v) = out.hit(Rule::G3, g3_input(name, input, &paths)) {
        return v;
    }
    for entry in &analysis.entries {
        if let Some(v) = out.hit(Rule::G3, g3(entry, &paths)) {
            return v;
        }
        if g4(entry) {
            return Verdict::Block(Rule::G4);
        }
        if g1(entry) {
            return Verdict::Block(Rule::G1);
        }
        if g2(entry) {
            return Verdict::Block(Rule::G2);
        }
    }
    let regex_limit = analysis
        .entries
        .iter()
        .any(|e| e.opaque.contains(&Opaque::RegexLimit));
    if out.limited || (analysis.incomplete && regex_limit) {
        Verdict::Incomplete
    } else if analysis.incomplete {
        Verdict::TooComplex
    } else {
        Verdict::Allow
    }
}
