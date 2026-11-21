//! A stand-in for CPython 3.12's `argparse` `_parse_known_args` (ruling R5), covering what
//! `startup_budget.py` and `eval_baseline.py` use: long options with one value or none
//! (`store_true`), `-h/--help`, positionals with `nargs=None` or `"*"`, unique-prefix
//! abbreviations, `--opt=value`, `--`, the negative-number rule, and argparse's error messages.
//! P1's `pycompat::argparse` replaces it.
//!
//! The model is argparse's: each argument is classified `O` (an option) or `A`, a run of `A`s
//! before the next option is offered to the positionals still waiting (greedy, as many
//! positionals as match), and leftovers are `unrecognized arguments`.

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;

use crate::Output;
use crate::json_edit::str_repr;
use crate::memory_diff::pystr;

/// A long option (`--name`): it takes one value, or none (`store_true`). With `float`, the
/// value goes through Python's `float()` when it is consumed (`type=float`).
pub struct Opt {
    pub long: &'static str,
    pub takes_value: bool,
    pub float: bool,
}

/// How many values a positional takes.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Nargs {
    /// `nargs=None`: exactly one (required).
    One,
    /// `nargs="*"`.
    Star,
}

/// A positional argument.
pub struct Pos {
    pub name: &'static str,
    pub nargs: Nargs,
}

/// One parser. `-h/--help` is implicit and first, as argparse adds it.
pub struct Spec<'a> {
    pub prog: &'a str,
    pub usage: &'a str,
    pub help: &'a str,
    pub opts: &'a [Opt],
    pub pos: &'a [Pos],
}

/// What was parsed: per positional its values (in `Spec::pos` order), and per option given its
/// value (`None` for a flag), in order.
#[derive(Debug, Default)]
pub struct Parsed {
    pub pos: Vec<Vec<OsString>>,
    pub opts: Vec<(&'static str, Option<OsString>)>,
    pub extras: Vec<OsString>,
}

impl Parsed {
    /// The last value given for `--long`.
    pub fn value(&self, long: &str) -> Option<&OsString> {
        self.opts
            .iter()
            .rev()
            .find(|(l, _)| *l == long)
            .and_then(|(_, v)| v.as_ref())
    }

    /// Whether `--long` was given.
    pub fn flag(&self, long: &str) -> bool {
        self.opts.iter().any(|(l, _)| *l == long)
    }
}

/// Why parsing stopped early.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    Help,
    Error(String),
}

const HELP: &str = "--help";

/// An option match: which option (`None`: unknown), and its explicit `=value`; or an
/// ambiguous prefix, which argparse records when it classifies the argument and raises only
/// when parsing reaches it.
#[derive(Clone, Debug)]
enum Matched {
    Opt(Option<&'static str>, Option<OsString>),
    Ambiguous(String),
}

/// argparse's `_negative_number_matcher`: `^-\d+$|^-\d*\.\d+$` (Unicode `\d`).
fn is_negative_number(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('-') else {
        return false;
    };
    let digits = |t: &str| t.chars().all(pystr::is_decimal);
    match rest.split_once('.') {
        None => !rest.is_empty() && digits(rest),
        Some((a, b)) => digits(a) && !b.is_empty() && digits(b),
    }
}

fn all_longs<'a>(spec: &'a Spec<'_>) -> impl Iterator<Item = &'static str> + 'a {
    std::iter::once(HELP).chain(spec.opts.iter().map(|o| o.long))
}

/// argparse's `_parse_optional`: `None` for a positional (`A`), else the option it names.
fn parse_optional(spec: &Spec<'_>, arg: &OsString) -> Option<Matched> {
    let bytes = arg.as_bytes();
    if bytes.first() != Some(&b'-') {
        return None;
    }
    let text = arg.to_string_lossy();
    let exact = |s: &str| -> Option<&'static str> {
        if s == "-h" {
            return Some(HELP);
        }
        all_longs(spec).find(|l| *l == s)
    };
    if let Some(o) = exact(&text) {
        return Some(Matched::Opt(Some(o), None));
    }
    if bytes.len() == 1 {
        return None;
    }
    if let Some(eq) = bytes.iter().position(|&b| b == b'=') {
        let name = String::from_utf8_lossy(&bytes[..eq]);
        if let Some(o) = exact(&name) {
            let value = OsString::from(std::ffi::OsStr::from_bytes(&bytes[eq + 1..]));
            return Some(Matched::Opt(Some(o), Some(value)));
        }
    }
    // _get_option_tuples.
    let mut found: Vec<(&'static str, Option<OsString>)> = Vec::new();
    if bytes.starts_with(b"--") {
        let (prefix, explicit) = match bytes.iter().position(|&b| b == b'=') {
            Some(eq) => (
                String::from_utf8_lossy(&bytes[..eq]).into_owned(),
                Some(OsString::from(std::ffi::OsStr::from_bytes(
                    &bytes[eq + 1..],
                ))),
            ),
            None => (text.to_string(), None),
        };
        for l in all_longs(spec) {
            if l.starts_with(&prefix) {
                found.push((l, explicit.clone()));
            }
        }
    } else if bytes.len() >= 2 && bytes[1] != b'-' {
        // A single-dash prefix (3.12.16): with `=`, every option starting with the part before
        // it (`-=x`: all of them); else `-h` with the rest as its explicit value (`-hX`).
        if let Some(eq) = bytes.iter().position(|&b| b == b'=') {
            let prefix = String::from_utf8_lossy(&bytes[..eq]).into_owned();
            let explicit = OsString::from(std::ffi::OsStr::from_bytes(&bytes[eq + 1..]));
            for l in std::iter::once("-h").chain(all_longs(spec)) {
                if l.starts_with(&prefix) {
                    found.push((if l == "-h" { HELP } else { l }, Some(explicit.clone())));
                }
            }
            if found.len() > 1 {
                let names: Vec<&str> = std::iter::once("-h")
                    .chain(all_longs(spec))
                    .filter(|l| l.starts_with(&prefix))
                    .collect();
                return Some(Matched::Ambiguous(format!(
                    "ambiguous option: {text} could match {}",
                    names.join(", ")
                )));
            }
        } else if bytes[1] == b'h' {
            // `-hX`: CPython 3.12.16 runs the help before it looks at `X`.
            found.push((HELP, None));
        }
    }
    match found.len() {
        0 => {}
        1 => {
            let (o, v) = found.pop().expect("one");
            return Some(Matched::Opt(Some(o), v));
        }
        _ => {
            let names: Vec<&str> = found.iter().map(|(l, _)| *l).collect();
            return Some(Matched::Ambiguous(format!(
                "ambiguous option: {text} could match {}",
                names.join(", ")
            )));
        }
    }
    if is_negative_number(&text) {
        return None;
    }
    if text.contains(' ') {
        return None;
    }
    Some(Matched::Opt(None, None))
}

/// The option's name in messages (`argument -h/--help: …`).
fn shown(long: &str) -> String {
    if long == HELP {
        "-h/--help".to_string()
    } else {
        long.to_string()
    }
}

/// `parse_known_args` (no `required` check, no extras check: see [`parse`]).
pub fn parse_known(spec: &Spec<'_>, args: &[OsString]) -> Result<Parsed, Stop> {
    // Classify: 'O' option, 'A' argument, '-' the first `--` (everything after it is 'A').
    let mut kinds = Vec::with_capacity(args.len());
    let mut matched: Vec<Option<Matched>> = Vec::with_capacity(args.len());
    let mut rest_positional = false;
    for arg in args {
        if rest_positional {
            kinds.push(b'A');
            matched.push(None);
        } else if arg.as_bytes() == b"--" {
            rest_positional = true;
            kinds.push(b'-');
            matched.push(None);
        } else {
            match parse_optional(spec, arg) {
                None => {
                    kinds.push(b'A');
                    matched.push(None);
                }
                Some(m) => {
                    kinds.push(b'O');
                    matched.push(Some(m));
                }
            }
        }
    }
    let mut parsed = Parsed {
        pos: Vec::new(),
        ..Parsed::default()
    };
    let mut next_pos = 0usize;
    let opt_indices: Vec<usize> = (0..args.len()).filter(|&i| kinds[i] == b'O').collect();
    let max_opt = opt_indices.last().copied();
    let mut start = 0usize;

    // consume_positionals: match as many waiting positionals as fit the pattern from `start`.
    let consume = |start: usize, next_pos: &mut usize, parsed: &mut Parsed| -> usize {
        let pattern: &[u8] = &kinds[start..];
        let waiting = &spec.pos[*next_pos..];
        for count in (1..=waiting.len()).rev() {
            if let Some(lens) = match_positionals(&waiting[..count], pattern) {
                let mut at = start;
                for len in lens {
                    let mut values: Vec<OsString> = args[at..at + len].to_vec();
                    if let Some(i) = values.iter().position(|v| v.as_bytes() == b"--") {
                        values.remove(i);
                    }
                    parsed.pos.push(values);
                    at += len;
                }
                *next_pos += count;
                return at;
            }
        }
        start
    };

    while let Some(max) = max_opt {
        if start > max {
            break;
        }
        let next_opt = *opt_indices
            .iter()
            .find(|&&i| i >= start)
            .expect("an option at or after start");
        if start != next_opt {
            let end = consume(start, &mut next_pos, &mut parsed);
            if end > start {
                start = end;
                continue;
            }
            start = end;
        }
        if kinds.get(start) != Some(&b'O') {
            parsed.extras.extend_from_slice(&args[start..next_opt]);
            start = next_opt;
        }
        // consume_optional.
        let (option, explicit) = match matched[start].clone().expect("an option") {
            Matched::Opt(o, e) => (o, e),
            Matched::Ambiguous(m) => return Err(Stop::Error(m)),
        };
        let Some(long) = option else {
            parsed.extras.push(args[start].clone());
            start += 1;
            continue;
        };
        if long == HELP && explicit.is_none() {
            return Err(Stop::Help);
        }
        let opt = spec.opts.iter().find(|o| o.long == long);
        let takes_value = opt.is_some_and(|o| o.takes_value);
        let float = opt.is_some_and(|o| o.float);
        let checked = |v: OsString| -> Result<OsString, Stop> {
            if float && py_float(&v.to_string_lossy()).is_none() {
                return Err(Stop::Error(format!(
                    "argument {}: invalid float value: {}",
                    shown(long),
                    str_repr(&v.to_string_lossy())
                )));
            }
            Ok(v)
        };
        if let Some(v) = explicit {
            if !takes_value {
                return Err(Stop::Error(format!(
                    "argument {}: ignored explicit argument {}",
                    shown(long),
                    str_repr(&v.to_string_lossy())
                )));
            }
            parsed.opts.push((long, Some(checked(v)?)));
            start += 1;
        } else if takes_value {
            if kinds.get(start + 1) == Some(&b'A') {
                parsed
                    .opts
                    .push((long, Some(checked(args[start + 1].clone())?)));
                start += 2;
            } else {
                return Err(Stop::Error(format!(
                    "argument {}: expected one argument",
                    shown(long)
                )));
            }
        } else {
            parsed.opts.push((long, None));
            start += 1;
        }
    }
    let stop = consume(start, &mut next_pos, &mut parsed);
    parsed.extras.extend_from_slice(&args[stop..]);
    // Positionals never matched: `*` gets an empty list; a required one is reported by `parse`.
    for p in &spec.pos[next_pos..] {
        if p.nargs == Nargs::Star {
            parsed.pos.push(Vec::new());
        }
    }
    let missing: Vec<&str> = spec.pos[next_pos..]
        .iter()
        .filter(|p| p.nargs == Nargs::One)
        .map(|p| p.name)
        .collect();
    if !missing.is_empty() {
        return Err(Stop::Error(format!(
            "the following arguments are required: {}",
            missing.join(", ")
        )));
    }
    Ok(parsed)
}

/// argparse's `_match_arguments_partial` for one prefix of the waiting positionals: the number
/// of arguments each takes, or `None` when the pattern does not match.
fn match_positionals(pos: &[Pos], pattern: &[u8]) -> Option<Vec<usize>> {
    // Patterns: One = `-*A-*`, Star = `-*[A-]*`; matched greedily left to right with
    // backtracking, as `re.match` does over the concatenation.
    fn go(pos: &[Pos], pattern: &[u8], at: usize, out: &mut Vec<usize>) -> bool {
        let Some((first, rest)) = pos.split_first() else {
            return true;
        };
        let mut candidates: Vec<usize> = Vec::new();
        match first.nargs {
            Nargs::One => {
                let mut i = at;
                while pattern.get(i) == Some(&b'-') {
                    i += 1;
                }
                if pattern.get(i) == Some(&b'A') {
                    let mut j = i + 1;
                    while pattern.get(j) == Some(&b'-') {
                        j += 1;
                    }
                    // Greedy trailing `-*`, then shorter.
                    candidates.extend((i + 1..=j).rev());
                }
            }
            Nargs::Star => {
                let mut j = at;
                while matches!(pattern.get(j), Some(b'A' | b'-')) {
                    j += 1;
                }
                candidates.extend((at..=j).rev());
            }
        }
        for end in candidates {
            out.push(end - at);
            if go(rest, pattern, end, out) {
                return true;
            }
            out.pop();
        }
        false
    }
    let mut out = Vec::new();
    go(pos, pattern, 0, &mut out).then_some(out)
}

/// Python's `float(s)` for a str: surrounding whitespace, a sign, `inf`/`infinity`/`nan` in any
/// case, decimal digits of any script (`Nd`), and `_` only between two digits.
pub fn py_float(s: &str) -> Option<f64> {
    let t = pystr::strip(s);
    let mut ascii = String::with_capacity(t.len());
    for c in t.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            ascii.push(char::from(b'0' + pystr::decimal_value(c)?));
        }
    }
    let (sign, body) = match ascii.as_bytes().first() {
        Some(b'-') => (-1.0, &ascii[1..]),
        Some(b'+') => (1.0, &ascii[1..]),
        _ => (1.0, &ascii[..]),
    };
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if lower == "nan" {
        return Some(f64::NAN.copysign(sign));
    }
    let b = body.as_bytes();
    let mut plain = String::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => plain.push(char::from(c)),
            b'_' if i > 0
                && b[i - 1].is_ascii_digit()
                && b.get(i + 1).is_some_and(u8::is_ascii_digit) => {}
            _ => return None,
        }
    }
    // Rust's grammar equals Python's here except the specials handled above and a sign after
    // the sign (`--1`), which Rust rejects too.
    if plain.is_empty() || plain.starts_with(['+', '-']) {
        return None;
    }
    plain.parse::<f64>().ok().map(|v| sign * v)
}

/// `parse_args`: [`parse_known`], then `unrecognized arguments`.
pub fn parse(spec: &Spec<'_>, args: &[OsString]) -> Result<Parsed, Stop> {
    let parsed = parse_known(spec, args)?;
    if !parsed.extras.is_empty() {
        return Err(Stop::Error(unrecognized(&parsed.extras)));
    }
    Ok(parsed)
}

/// `unrecognized arguments: a b`.
pub fn unrecognized(extras: &[OsString]) -> String {
    let shown: Vec<String> = extras
        .iter()
        .map(|e| e.to_string_lossy().into_owned())
        .collect();
    format!("unrecognized arguments: {}", shown.join(" "))
}

/// What one argument is to a parser whose only option is `-h` (a subparsers' main parser),
/// when parsing reaches it.
#[derive(Debug, PartialEq, Eq)]
pub enum Class {
    Positional,
    Help,
    Unknown,
    Error(String),
}

/// [`Class`] of `arg` under `spec`.
pub fn classify(spec: &Spec<'_>, arg: &OsString) -> Class {
    match parse_optional(spec, arg) {
        None => Class::Positional,
        Some(Matched::Ambiguous(m)) => Class::Error(m),
        Some(Matched::Opt(None, _)) => Class::Unknown,
        Some(Matched::Opt(Some(HELP), None)) => Class::Help,
        Some(Matched::Opt(Some(long), Some(v))) => Class::Error(format!(
            "argument {}: ignored explicit argument {}",
            shown(long),
            str_repr(&v.to_string_lossy())
        )),
        Some(Matched::Opt(Some(long), None)) => {
            Class::Error(format!("argument {}: expected one argument", shown(long)))
        }
    }
}

/// Exit 2: the usage, then `<prog>: error: <message>`.
pub fn error(spec: &Spec<'_>, message: &str) -> Output {
    Output {
        stderr: format!("{}\n{}: error: {message}\n", spec.usage, spec.prog).into_bytes(),
        code: 2,
        ..Output::default()
    }
}

/// Exit 0 with the help text on stdout.
pub fn help(spec: &Spec<'_>) -> Output {
    Output {
        stdout: format!("{}\n{}", spec.usage, spec.help).into_bytes(),
        ..Output::default()
    }
}

/// [`parse`], with a stop turned into its `Output`.
pub fn parse_or_exit(spec: &Spec<'_>, args: &[OsString]) -> Result<Parsed, Output> {
    match parse(spec, args) {
        Ok(p) => Ok(p),
        Err(Stop::Help) => Err(help(spec)),
        Err(Stop::Error(m)) => Err(error(spec, &m)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET_SPEC: Spec<'static> = Spec {
        prog: "p",
        usage: "usage: p",
        help: "",
        opts: &[Opt {
            long: "--root",
            takes_value: true,
            float: false,
        }],
        pos: &[Pos {
            name: "agents",
            nargs: Nargs::Star,
        }],
    };

    #[test]
    fn python_float() {
        assert_eq!(py_float(" 1.5 "), Some(1.5));
        assert_eq!(py_float("1_000.2_5"), Some(1000.25));
        assert_eq!(py_float("-.5e-3"), Some(-0.0005));
        assert_eq!(py_float("+InFiNiTy"), Some(f64::INFINITY));
        assert!(py_float("-nan").is_some_and(f64::is_nan));
        assert_eq!(py_float("\u{663}.\u{665}"), Some(3.5));
        for bad in [
            "", "_1", "1_", "1__0", "1_.5", "1e_5", "0x10", "1.5j", "--1", "+-1", "e5", "1 0",
        ] {
            assert_eq!(py_float(bad), None, "{bad:?}");
        }
    }

    fn run(args: &[&str]) -> Result<(Vec<String>, Option<String>), String> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        match parse(&BUDGET_SPEC, &args) {
            Ok(p) => Ok((
                p.pos[0]
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
                p.value("--root").map(|v| v.to_string_lossy().into_owned()),
            )),
            Err(Stop::Help) => Err("help".into()),
            Err(Stop::Error(m)) => Err(m),
        }
    }

    fn ok(agents: &[&str], root: Option<&str>) -> Result<(Vec<String>, Option<String>), String> {
        Ok((
            agents.iter().map(|s| s.to_string()).collect(),
            root.map(str::to_string),
        ))
    }

    /// Each expectation was taken from CPython 3.12.16 (the pinned image).
    #[test]
    fn matches_cpython_312() {
        assert_eq!(run(&["--root", "x", "b"]), ok(&["b"], Some("x")));
        assert_eq!(
            run(&["a", "--root", "x", "b"]),
            Err("unrecognized arguments: b".into())
        );
        assert_eq!(run(&["a", "b", "--root", "x"]), ok(&["a", "b"], Some("x")));
        assert_eq!(
            run(&["--root"]),
            Err("argument --root: expected one argument".into())
        );
        assert_eq!(run(&["--r", "x"]), ok(&[], Some("x")));
        assert_eq!(run(&["--root=x", "a"]), ok(&["a"], Some("x")));
        assert_eq!(run(&["-x"]), Err("unrecognized arguments: -x".into()));
        assert_eq!(run(&["--", "-x"]), ok(&["-x"], None));
        assert_eq!(run(&["a", "--", "b"]), ok(&["a", "b"], None));
        assert_eq!(run(&["-1"]), ok(&["-1"], None));
        assert_eq!(run(&["--root", "-1"]), ok(&[], Some("-1")));
        assert_eq!(run(&["a", "-x"]), Err("unrecognized arguments: -x".into()));
        assert_eq!(run(&["--root", "x", "--root", "y"]), ok(&[], Some("y")));
        assert_eq!(run(&["--ro=", "a"]), ok(&["a"], Some("")));
        assert_eq!(
            run(&["a", "--", "b", "--", "c"]),
            ok(&["a", "b", "--", "c"], None)
        );
        assert_eq!(
            run(&["a", "--root", "x", "--", "b"]),
            Err("unrecognized arguments: -- b".into())
        );
        assert_eq!(run(&["--"]), ok(&[], None));
        assert_eq!(run(&["--", "--"]), ok(&["--"], None));
        assert_eq!(
            run(&["--root", "--", "a"]),
            Err("argument --root: expected one argument".into())
        );
        assert_eq!(run(&["--root", "x", "--"]), ok(&[], Some("x")));
        assert_eq!(run(&["-"]), ok(&["-"], None));
        assert_eq!(run(&["--root=x=y"]), ok(&[], Some("x=y")));
        assert_eq!(run(&["a", "-1"]), ok(&["a", "-1"], None));
        assert_eq!(run(&["--root", "x", "-1"]), ok(&["-1"], Some("x")));
        assert_eq!(run(&["-h", "x"]), Err("help".into()));
        assert_eq!(run(&["--he"]), Err("help".into()));
        assert_eq!(
            run(&["x", "--root"]),
            Err("argument --root: expected one argument".into())
        );
        assert_eq!(
            run(&["--rootx"]),
            Err("unrecognized arguments: --rootx".into())
        );
        assert_eq!(run(&["-r"]), Err("unrecognized arguments: -r".into()));
        assert_eq!(run(&["--root", "-"]), ok(&[], Some("-")));
        assert_eq!(run(&["-1x"]), Err("unrecognized arguments: -1x".into()));
        assert_eq!(
            run(&["--root", "--x"]),
            Err("argument --root: expected one argument".into())
        );
        assert_eq!(
            run(&["--=x"]),
            Err("ambiguous option: --=x could match --help, --root".into())
        );
        assert_eq!(
            run(&["-=x"]),
            Err("ambiguous option: -=x could match -h, --help, --root".into())
        );
        // The ambiguity is raised when parsing reaches it, not when it is classified.
        assert_eq!(run(&["-h", "--=x"]), Err("help".into()));
        assert_eq!(
            run(&["--root", "--=x"]),
            Err("argument --root: expected one argument".into())
        );
    }
}
