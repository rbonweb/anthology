//! A small stand-in for Python's `argparse` (ruling R5), for the DV2 tools whose options all
//! take one value: long options (`--opt v`, `--opt=v`, unique-prefix abbreviations), positionals
//! with optional `choices`, options between positionals, `--`, argparse's negative-number rule,
//! and its error messages. P1's `pycompat::argparse` replaces it.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use crate::Output;
use crate::json_edit::str_repr;
use crate::memory_diff::pystr;

/// A long option that takes exactly one value.
pub struct OptSpec {
    pub long: &'static str,
    pub dest: &'static str,
}

/// A required positional (`nargs=None`), with its `choices` (empty: any value).
pub struct PosSpec {
    pub name: &'static str,
    pub choices: &'static [&'static str],
}

/// One parser: its `prog`, its usage line, its options and positionals. `-h/--help` is implicit.
pub struct Parser<'a> {
    pub prog: &'a str,
    pub usage: &'a str,
    pub opts: &'a [OptSpec],
    pub pos: &'a [PosSpec],
}

/// Why parsing stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    /// `-h` or `--help`: print the help, exit 0.
    Help,
    /// A usage error: exit 2 after printing `usage` and `<prog>: error: <message>`.
    Error(String),
}

/// What `parse_known_args` found.
#[derive(Debug, Default)]
pub struct Parsed {
    /// The positional values, in order (all of them: a missing one is an error).
    pub pos: Vec<OsString>,
    /// `(dest, value)` for each option given, in order; a later one overrides an earlier one.
    pub opts: Vec<(&'static str, OsString)>,
    /// The arguments no parser claimed (`unrecognized arguments`).
    pub extras: Vec<String>,
}

impl Parsed {
    /// The value of option `dest`, the last one given.
    pub fn opt(&self, dest: &str) -> Option<&OsString> {
        self.opts
            .iter()
            .rev()
            .find(|(d, _)| *d == dest)
            .map(|(_, v)| v)
    }
}

enum Kind {
    /// A positional (also every argument after `--`).
    Positional,
    /// `-h` / `--help`, with an explicit argument glued on (`--help=x`, `-hx`).
    Help(Option<String>),
    /// An option of `opts`, with an explicit argument glued on (`--opt=v`).
    Value(usize, Option<String>),
    /// Starts with `-` and is none of the above.
    Unknown,
    /// A long-option prefix that fits several options: the error message.
    Ambiguous(String),
}

/// argparse's `_negative_number_matcher`, `^-\d+$|^-\d*\.\d+$` (Unicode `\d`; `$` also matches
/// before a final newline).
fn is_negative_number(s: &str) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let Some(body) = s.strip_prefix('-') else {
        return false;
    };
    let digits = |t: &str| t.chars().all(pystr::is_decimal);
    match body.split_once('.') {
        None => !body.is_empty() && digits(body),
        Some((a, b)) => digits(a) && !b.is_empty() && digits(b),
    }
}

fn classify(p: &Parser<'_>, s: &str) -> Kind {
    if !s.starts_with('-') {
        return Kind::Positional;
    }
    if s == "-h" || s == "--help" {
        return Kind::Help(None);
    }
    if let Some(i) = p.opts.iter().position(|o| o.long == s) {
        return Kind::Value(i, None);
    }
    if s.len() == 1 {
        return Kind::Positional;
    }
    if let Some((name, explicit)) = s.split_once('=') {
        if name == "--help" {
            return Kind::Help(Some(explicit.to_string()));
        }
        if let Some(i) = p.opts.iter().position(|o| o.long == name) {
            return Kind::Value(i, Some(explicit.to_string()));
        }
    }
    if s.starts_with("--") {
        // Unique-prefix abbreviations (`allow_abbrev`).
        let (prefix, explicit) = match s.split_once('=') {
            Some((a, b)) => (a, Some(b.to_string())),
            None => (s, None),
        };
        let mut hits: Vec<Kind> = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        if "--help".starts_with(prefix) {
            hits.push(Kind::Help(explicit.clone()));
            names.push("--help");
        }
        for (i, o) in p.opts.iter().enumerate() {
            if o.long.starts_with(prefix) {
                hits.push(Kind::Value(i, explicit.clone()));
                names.push(o.long);
            }
        }
        if hits.len() > 1 {
            return Kind::Ambiguous(format!(
                "ambiguous option: {s} could match {}",
                names.join(", ")
            ));
        }
        if hits.len() == 1 {
            return hits.remove(0);
        }
    } else if s.starts_with("-=") {
        // The single-dash prefix before `=` is `-`, which every option string starts with.
        let mut names = vec!["-h", "--help"];
        names.extend(p.opts.iter().map(|o| o.long));
        return Kind::Ambiguous(format!(
            "ambiguous option: {s} could match {}",
            names.join(", ")
        ));
    } else if let Some(rest) = s.strip_prefix("-h") {
        // A single-dash cluster (`-hx`): Python 3.12 runs `-h` and leaves the rest as strays
        // (the help exits first); an `=` or a `-` right after `-h` is an error.
        return match rest.strip_prefix('=') {
            Some(explicit) => Kind::Help(Some(explicit.to_string())),
            None if rest.starts_with('-') => Kind::Help(Some(rest.to_string())),
            None => Kind::Help(None),
        };
    }
    if is_negative_number(s) || s.contains(' ') {
        return Kind::Positional;
    }
    Kind::Unknown
}

/// `parser.parse_known_args(args)`.
pub fn parse_known(p: &Parser<'_>, args: &[OsString]) -> Result<Parsed, Stop> {
    let strs: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let n = strs.len();
    // argparse classifies every argument up front but only raises an ambiguous-option error
    // when parsing reaches that argument (an earlier `-h` or a missing option value wins).
    let mut out = Parsed::default();
    let mut dashdash = false;
    // The index just after the last positional (a `--` right there is swallowed with it).
    let mut after_pos = usize::MAX;
    let mut i = 0;
    while i < n {
        let s = &strs[i];
        if !dashdash && s == "--" {
            dashdash = true;
            let slots_left = out.pos.len() < p.pos.len();
            if after_pos != i && !(slots_left && i + 1 < n) {
                out.extras.push(s.clone());
            }
            if after_pos == i {
                after_pos = i + 1;
            }
            i += 1;
            continue;
        }
        let kind = if dashdash {
            Kind::Positional
        } else {
            classify(p, s)
        };
        match kind {
            Kind::Positional => {
                match p.pos.get(out.pos.len()) {
                    Some(spec) => {
                        if !spec.choices.is_empty() && !spec.choices.contains(&s.as_str()) {
                            return Err(Stop::Error(invalid_choice(spec.name, s, spec.choices)));
                        }
                        out.pos.push(args[i].clone());
                        after_pos = i + 1;
                    }
                    None => out.extras.push(s.clone()),
                }
                i += 1;
            }
            Kind::Unknown => {
                out.extras.push(s.clone());
                i += 1;
            }
            Kind::Ambiguous(m) => return Err(Stop::Error(m)),
            Kind::Help(None) => return Err(Stop::Help),
            Kind::Help(Some(explicit)) => {
                return Err(Stop::Error(format!(
                    "argument -h/--help: ignored explicit argument {}",
                    str_repr(&explicit)
                )));
            }
            Kind::Value(idx, Some(_)) => {
                // `--opt=v` carries its own value (even an empty one); keep its raw bytes.
                let raw = args[i].as_bytes();
                let at = raw
                    .iter()
                    .position(|&b| b == b'=')
                    .map_or(raw.len(), |k| k + 1);
                out.opts
                    .push((p.opts[idx].dest, OsString::from_vec(raw[at..].to_vec())));
                i += 1;
            }
            Kind::Value(idx, None) => {
                let name = p.opts[idx].long;
                let j = i + 1;
                // The value is the next argument, unless that looks like an option; an option
                // cannot take `--` as part of its arguments.
                if j < n
                    && (dashdash
                        || (strs[j] != "--" && matches!(classify(p, &strs[j]), Kind::Positional)))
                {
                    out.opts.push((p.opts[idx].dest, args[j].clone()));
                    i = j + 1;
                } else {
                    return Err(Stop::Error(format!(
                        "argument {name}: expected one argument"
                    )));
                }
            }
        }
    }
    if out.pos.len() < p.pos.len() {
        let names: Vec<&str> = p.pos[out.pos.len()..].iter().map(|s| s.name).collect();
        return Err(Stop::Error(format!(
            "the following arguments are required: {}",
            names.join(", ")
        )));
    }
    Ok(out)
}

/// The main parser of a command with subcommands: the index in `args` of the command (the first
/// positional; the sub-parser takes everything after it), and the stray options before it.
pub fn find_command(p: &Parser<'_>, args: &[OsString]) -> Result<(usize, Vec<String>), Stop> {
    let mut extras = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let s = a.to_string_lossy();
        if s == "--" {
            // Python 3.12.16 drops one leading `--`; the next argument is the command, taken
            // as is (even `--`, `-h` or `-x`). Nothing after it: no command.
            return match args.get(i + 1) {
                Some(_) => Ok((i + 1, extras)),
                None => Err(Stop::Error(
                    "the following arguments are required: cmd".into(),
                )),
            };
        }
        match classify(p, &s) {
            Kind::Positional => return Ok((i, extras)),
            Kind::Help(None) => return Err(Stop::Help),
            Kind::Help(Some(explicit)) => {
                return Err(Stop::Error(format!(
                    "argument -h/--help: ignored explicit argument {}",
                    str_repr(&explicit)
                )));
            }
            Kind::Ambiguous(m) => return Err(Stop::Error(m)),
            Kind::Value(..) | Kind::Unknown => extras.push(s.into_owned()),
        }
    }
    Err(Stop::Error(
        "the following arguments are required: cmd".into(),
    ))
}

/// argparse's `invalid choice` message, as CPython 3.12.16 words it (the choices are not
/// quoted; Python 3.9 quotes them, and the difftest oracle is 3.12).
pub fn invalid_choice(name: &str, value: &str, choices: &[&str]) -> String {
    format!(
        "argument {name}: invalid choice: {} (choose from {})",
        str_repr(value),
        choices.join(", ")
    )
}

/// The exit-2 output of `parser.error(message)`.
pub fn usage_error(p: &Parser<'_>, message: &str) -> Output {
    Output {
        stderr: format!("{}\n{}: error: {message}\n", p.usage, p.prog).into_bytes(),
        code: 2,
        ..Output::default()
    }
}

/// The exit-0 output of `-h`.
pub fn help(p: &Parser<'_>) -> Output {
    let text = HELPS
        .iter()
        .find(|(prog, _)| *prog == p.prog)
        .map_or_else(|| format!("{}\n", p.usage), |(_, h)| (*h).to_string());
    Output {
        stdout: text.into_bytes(),
        ..Output::default()
    }
}

/// `format_help()` of each parser, as CPython 3.12.16 prints it at 80 columns (captured from
/// the pinned image), by `prog`.
const HELPS: &[(&str, &str)] = &[
    (
        "check_version_bump.py",
        "usage: check_version_bump.py [-h] [--repo REPO] base head\n\nVersion-bump gate: a plugin whose files changed must carry a higher version.\n\npositional arguments:\n  base         base commit (all zeros = new branch, no-op)\n  head         head commit\n\noptions:\n  -h, --help   show this help message and exit\n  --repo REPO  repository root (default: current directory)\n",
    ),
    (
        "release_edit.py",
        "usage: release_edit.py [-h]\n                       {plugin-version,check-version,check-dependents,changelog-check,changelog-move,set-version,set-source-version}\n                       ...\n\nText and JSON helpers for scripts/release.sh (the shell orchestrates git and\nclaude).\n\npositional arguments:\n  {plugin-version,check-version,check-dependents,changelog-check,changelog-move,set-version,set-source-version}\n\noptions:\n  -h, --help            show this help message and exit\n",
    ),
    (
        "release_edit.py plugin-version",
        "usage: release_edit.py plugin-version [-h] path\n\npositional arguments:\n  path\n\noptions:\n  -h, --help  show this help message and exit\n",
    ),
    (
        "release_edit.py check-version",
        "usage: release_edit.py check-version [-h] current new {0,1}\n\npositional arguments:\n  current\n  new\n  {0,1}\n\noptions:\n  -h, --help  show this help message and exit\n",
    ),
    (
        "release_edit.py check-dependents",
        "usage: release_edit.py check-dependents [-h] root plugin version\n\npositional arguments:\n  root\n  plugin\n  version\n\noptions:\n  -h, --help  show this help message and exit\n",
    ),
    (
        "release_edit.py changelog-check",
        "usage: release_edit.py changelog-check [-h] path plugin\n\npositional arguments:\n  path\n  plugin\n\noptions:\n  -h, --help  show this help message and exit\n",
    ),
    (
        "release_edit.py changelog-move",
        "usage: release_edit.py changelog-move [-h] [--date DATE] path plugin version\n\npositional arguments:\n  path\n  plugin\n  version\n\noptions:\n  -h, --help   show this help message and exit\n  --date DATE\n",
    ),
    (
        "release_edit.py set-version",
        "usage: release_edit.py set-version [-h] path version\n\npositional arguments:\n  path\n  version\n\noptions:\n  -h, --help  show this help message and exit\n",
    ),
    (
        "release_edit.py set-source-version",
        "usage: release_edit.py set-source-version [-h] path target version\n\npositional arguments:\n  path\n  target\n  version\n\noptions:\n  -h, --help  show this help message and exit\n",
    ),
];

/// `parser.parse_args(args)`: errors on a leftover argument, as `parse_args` does.
pub fn parse_args(p: &Parser<'_>, args: &[OsString]) -> Result<Parsed, Output> {
    match parse_known(p, args) {
        Ok(parsed) if parsed.extras.is_empty() => Ok(parsed),
        Ok(parsed) => Err(usage_error(
            p,
            &format!("unrecognized arguments: {}", parsed.extras.join(" ")),
        )),
        Err(Stop::Help) => Err(help(p)),
        Err(Stop::Error(m)) => Err(usage_error(p, &m)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: &[OptSpec] = &[OptSpec {
        long: "--repo",
        dest: "repo",
    }];
    const POS: &[PosSpec] = &[
        PosSpec {
            name: "base",
            choices: &[],
        },
        PosSpec {
            name: "mode",
            choices: &["0", "1"],
        },
    ];
    const P: Parser<'static> = Parser {
        prog: "p",
        usage: "usage: p",
        opts: OPTS,
        pos: POS,
    };

    fn parse(args: &[&str]) -> Result<Parsed, Stop> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        parse_known(&P, &args)
    }

    fn err(args: &[&str]) -> String {
        match parse(args) {
            Err(Stop::Error(m)) => m,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn options_between_positionals_and_abbreviations() {
        let p = parse(&["a", "--re", "r", "1"]).unwrap();
        assert_eq!(p.pos, ["a", "1"]);
        assert_eq!(p.opt("repo").unwrap(), "r");
        let p = parse(&["--repo=x=y", "a", "0"]).unwrap();
        assert_eq!(p.opt("repo").unwrap(), "x=y");
        let p = parse(&["--", "-x", "1"]).unwrap();
        assert_eq!(p.pos, ["-x", "1"]);
    }

    #[test]
    fn negative_numbers_are_values() {
        let p = parse(&["-1", "0", "--repo", "-0.5"]).unwrap();
        assert_eq!(p.pos, ["-1", "0"]);
        assert_eq!(p.opt("repo").unwrap(), "-0.5");
    }

    #[test]
    fn error_messages() {
        assert_eq!(err(&["a"]), "the following arguments are required: mode");
        assert_eq!(err(&[]), "the following arguments are required: base, mode");
        assert_eq!(
            err(&["a", "2"]),
            "argument mode: invalid choice: '2' (choose from 0, 1)"
        );
        assert_eq!(
            err(&["a", "--repo"]),
            "argument --repo: expected one argument"
        );
        // An option cannot take `--` as part of its argument.
        assert_eq!(
            err(&["a", "1", "--repo", "--", "c"]),
            "argument --repo: expected one argument"
        );
        // Python 3.12: `-hx` runs the help first.
        assert_eq!(parse(&["-hx"]).unwrap_err(), Stop::Help);
        assert_eq!(
            err(&["a", "--repo", "-x"]),
            "argument --repo: expected one argument"
        );
        let p = parse(&["a", "1", "--bogus", "x"]).unwrap();
        assert_eq!(p.extras, ["--bogus", "x"]);
        assert_eq!(parse(&["-h"]).unwrap_err(), Stop::Help);
        assert_eq!(
            err(&["--help=x"]),
            "argument -h/--help: ignored explicit argument 'x'"
        );
    }
}
