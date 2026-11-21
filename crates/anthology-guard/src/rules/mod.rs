//! The rules G1 to G6 (GD1): guard_rules.py `MESSAGES`, `_g1` .. `_g6` and their helpers.
//!
//! Each rule module holds one predicate and the tables it reads. The predicates take what
//! `check` (GC-6 order) hands them: an [`anthology_shell::Entry`] for G1 to G4, the tool name
//! for G5, the tool input or the command for G6. A predicate that runs a fancy-regex pattern
//! returns `Result<bool, Limit>`: [`Limit`] means a backtrack limit was hit (GC-16), and the
//! caller blocks with `Verdict::Incomplete` unless another rule blocks first.

pub mod g1;
pub mod g2;
pub mod g3;
pub mod g4;
pub mod g5;
pub mod g6;

/// A rule of the guard (GC-6).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Rule {
    G1,
    G2,
    G3,
    G4,
    G5,
    G6,
}

impl Rule {
    /// Every rule, in id order.
    pub const ALL: [Rule; 6] = [Rule::G1, Rule::G2, Rule::G3, Rule::G4, Rule::G5, Rule::G6];

    /// `"G1"` .. `"G6"`.
    pub fn id(self) -> &'static str {
        match self {
            Rule::G1 => "G1",
            Rule::G2 => "G2",
            Rule::G3 => "G3",
            Rule::G4 => "G4",
            Rule::G5 => "G5",
            Rule::G6 => "G6",
        }
    }

    /// `guard_rules.MESSAGES[id]`, verbatim (GC-6).
    pub fn message(self) -> &'static str {
        match self {
            Rule::G1 => "run it inside Docker (OrbStack); for browser work use BrowserOS Neo",
            Rule::G2 => "never launch Chrome on the host; use BrowserOS Neo",
            Rule::G3 => "Chrome profiles are off-limits",
            Rule::G4 => "agent-browser is not used here; use the BrowserOS Neo MCP tools",
            Rule::G5 => "use the BrowserOS Neo MCP tools",
            Rule::G6 => "memory is committed to the repo; never store secrets there",
        }
    }
}

/// A fancy-regex backtrack limit was hit while a rule was evaluated (GC-16, D-7).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct Limit;

/// `any(f(x) for x in items)` where `f` may hit a limit: `Ok(true)` as soon as one item holds;
/// a limit on one item does not stop the others (one of them may still hold), and is returned
/// only when none holds.
pub(crate) fn any_of<T>(
    items: impl IntoIterator<Item = T>,
    mut f: impl FnMut(T) -> Result<bool, Limit>,
) -> Result<bool, Limit> {
    let mut limited = false;
    for item in items {
        match f(item) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(Limit) => limited = true,
        }
    }
    if limited { Err(Limit) } else { Ok(false) }
}

/// `a or b` for two results that may hit a limit (see [`any_of`]).
pub(crate) fn either(
    a: Result<bool, Limit>,
    b: impl FnOnce() -> Result<bool, Limit>,
) -> Result<bool, Limit> {
    match a {
        Ok(true) => Ok(true),
        Ok(false) => b(),
        Err(Limit) => match b() {
            Ok(true) => Ok(true),
            _ => Err(Limit),
        },
    }
}

/// `_first_arg(args)`: the first word that does not start with `-`.
pub(crate) fn first_arg(args: &[String]) -> Option<&str> {
    args.iter().map(String::as_str).find(|a| !a.starts_with('-'))
}

/// `_strip_version(word)`: `word` up to its last `@`, unless that `@` is the first character.
pub(crate) fn strip_version(word: &str) -> &str {
    match word.rfind('@') {
        Some(at) if at > 0 => &word[..at],
        _ => word,
    }
}

/// `_positional(args, value_flags)`: the words that are not options, skipping each value
/// flag's value.
pub(crate) fn positional<'a, S: AsRef<str>>(args: &'a [S], value_flags: &[&str]) -> Vec<&'a str> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_ref();
        if value_flags.contains(&arg) {
            i += 1;
        } else if !arg.starts_with('-') {
            words.push(arg);
        }
        i += 1;
    }
    words
}

/// `_SEARCH_PROGRAMS`: programs that only look at or print the text an argument appears in.
pub(crate) const SEARCH_PROGRAMS: &[&str] = &[
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "git", "echo", "printf",
];

/// `_JS_PACKAGE_MANAGERS`.
pub(crate) const JS_PACKAGE_MANAGERS: &[&str] = &["npm", "pnpm", "yarn", "bun"];

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn strip_version_keeps_a_leading_at() {
        assert_eq!(strip_version("playwright@1.49.0"), "playwright");
        assert_eq!(strip_version("@playwright/test@1.2"), "@playwright/test");
        assert_eq!(strip_version("@playwright/test"), "@playwright/test");
        assert_eq!(strip_version("x"), "x");
    }

    #[test]
    fn positional_skips_value_flags_and_options() {
        let a = args(&["--prefix", "web", "-D", "i", "puppeteer"]);
        assert_eq!(positional(&a, &["--prefix"]), ["i", "puppeteer"]);
        assert_eq!(first_arg(&a), Some("web"));
    }

    #[test]
    fn any_of_reports_a_limit_only_when_nothing_holds() {
        let r = |v: &Result<bool, Limit>| *v;
        assert_eq!(any_of(&[Err(Limit), Ok(true)], r), Ok(true));
        assert_eq!(any_of(&[Err(Limit), Ok(false)], r), Err(Limit));
        assert_eq!(any_of(&[Ok(false)], r), Ok(false));
        assert_eq!(either(Err(Limit), || Ok(true)), Ok(true));
        assert_eq!(either(Err(Limit), || Ok(false)), Err(Limit));
    }
}
