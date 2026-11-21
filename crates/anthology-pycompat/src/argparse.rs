//! `pycompat::argparse`: CPython 3.12's `argparse`, for the command lines the port carries
//! (design §12, plan §3.2, GC-16).
//!
//! A [`Spec`] is built the way the Python scripts build an `ArgumentParser`: `prog`,
//! `description`, options and positionals ([`Arg`]: `store`, `store_true`/`store_false`/
//! `store_const`, `append`; `nargs` None/`?`/`*`/`+`/N; `metavar` or a metavar tuple; `choices`;
//! `required`; `default`; `type` str/int/float/`Path`; `help` or `help=SUPPRESS`), mutually
//! exclusive groups, one set of subcommands ([`Subcommands`], `add_subparsers(dest=...,
//! required=...)` and `add_parser(name, help=...)`), `allow_abbrev`, and the automatic
//! `-h/--help`. [`parse`] is `parser.parse_args(args)` (`args` without the program name):
//! the same matching (the `A`/`O`/`-` pattern and its regular expressions, abbreviations,
//! `--opt=value`, `-xVALUE`, combined short flags, `--`, negative numbers, ambiguous
//! prefixes), the same namespace (by `dest`, in Python's order, defaults included, string
//! defaults converted by `type`), and, when Python exits, a [`UsageError`] holding exactly what
//! it prints and its exit status: 2 with the usage and `prog: error: <message>` on stderr (a
//! subcommand's errors under the subcommand's `prog`), 0 with the help on stdout for `-h`.
//!
//! The help is `HelpFormatter`'s output for the width set with [`Spec::width`] (Python takes it
//! from the `COLUMNS` environment variable, minus 2; [`help_width`] does that sum; the default
//! is 78, as for a non-terminal without `COLUMNS`). Text is wrapped as `textwrap` wraps it.
//!
//! Arguments are bytes (`OsString`): a value keeps its bytes, and a message shows a byte that
//! is not UTF-8 as Python's stderr does, `\udcXX`. Not supported, so never silently different:
//! `parents`, argument groups with titles, `prefix_chars`, `fromfile_prefix_chars`,
//! `set_defaults`, `count`/`version`/`extend` actions, `nargs=REMAINDER`, aliases, a custom
//! `usage`, and `%`-format keys in help other than `prog`, `default`, `const`, `dest`,
//! `metavar`, `choices`, `nargs`, `type`, `required`, `help`, `option_strings` (one is left as
//! written, where Python would raise). A builder call that Python would reject (a conflicting
//! option string, a metavar tuple of the wrong length, `required` on a positional) panics.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::{Mutex, OnceLock};

use crate::json::{self, Int};
use crate::re;
use crate::str::{char_len, is_space, py_float, py_int, repr, repr_surrogateescape, strip_ws};

/// `HelpFormatter`'s width without `COLUMNS` or a terminal: 80 columns, minus 2.
pub const DEFAULT_WIDTH: i64 = 78;

/// `shutil.get_terminal_size().columns - 2`, `HelpFormatter`'s width: `columns_env` is the
/// `COLUMNS` environment variable (used when it is a positive integer, read as `int()` reads
/// it), else `terminal_columns` (the terminal's width, when there is one and it is not 0), else
/// 80.
pub fn help_width(columns_env: Option<&str>, terminal_columns: Option<u16>) -> i64 {
    let env = match columns_env.and_then(py_int) {
        Some(Int::Small(n)) if n > 0 => Some(n),
        Some(Int::Big(t)) if !t.starts_with('-') => Some(i64::MAX),
        _ => None,
    };
    let columns = env
        .or_else(|| terminal_columns.filter(|&c| c > 0).map(i64::from))
        .unwrap_or(80);
    columns.saturating_sub(2)
}

/// A value in the parsed namespace.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `None`.
    None,
    /// `True` / `False`.
    Bool(bool),
    /// A str (from the command line, its bytes as given).
    Str(OsString),
    /// An `int`.
    Int(Int),
    /// A `float`.
    Float(f64),
    /// A list (`nargs` other than None and `?`, `append`).
    List(Vec<Value>),
}

impl Value {
    /// A `Str` from text.
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    /// The str, if it is one and is UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => s.to_str(),
            _ => None,
        }
    }

    /// The str's bytes, if it is one.
    pub fn as_os_str(&self) -> Option<&OsStr> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The bool, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The int, if it is one.
    pub fn as_int(&self) -> Option<&Int> {
        match self {
            Value::Int(n) => Some(n),
            _ => None,
        }
    }

    /// The int, if it is one and fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(Int::Small(n)) => Some(*n),
            _ => None,
        }
    }

    /// The float, if it is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// The list, if it is one.
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    /// Is it `None`?
    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }

    /// `str(v)` (a str's bytes that are not UTF-8 shown as `\udcXX`).
    pub fn py_str(&self) -> String {
        match self {
            Value::None => "None".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Str(s) => display(s.as_bytes()),
            Value::Int(Int::Small(n)) => n.to_string(),
            Value::Int(Int::Big(t)) => t.clone(),
            Value::Float(f) => float_repr(*f),
            Value::List(items) => {
                let items: Vec<String> = items.iter().map(Value::py_repr).collect();
                format!("[{}]", items.join(", "))
            }
        }
    }

    /// `repr(v)`.
    pub fn py_repr(&self) -> String {
        match self {
            Value::Str(s) => repr_surrogateescape(s.as_bytes()),
            v => v.py_str(),
        }
    }
}

fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        json::dumps(&json::Value::Float(f), &json::PY_DEFAULT)
    }
}

/// Bytes as Python writes the str `os.fsdecode` makes of them to stderr
/// (`backslashreplace`): UTF-8 as is, any other byte as `\udcXX`.
fn display(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
        for &x in chunk.invalid() {
            out.push_str(&format!("\\udc{x:02x}"));
        }
    }
    out
}

/// `nargs` other than the default (None: exactly one value, not in a list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nargs {
    /// `"?"`.
    Optional,
    /// `"*"`.
    ZeroOrMore,
    /// `"+"`.
    OneOrMore,
    /// An int: exactly that many values, in a list.
    Exactly(usize),
}

/// `type=`: None (str), `int`, `float`, `pathlib.Path` (its `str()`, e.g. `a//b/` → `a/b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Str,
    Int,
    Float,
    Path,
}

impl Type {
    /// The name `%(type)s` and the `invalid <type> value` message use.
    fn name(self) -> &'static str {
        match self {
            Type::Str => "None",
            Type::Int => "int",
            Type::Float => "float",
            Type::Path => "Path",
        }
    }
}

/// The nargs argparse matches on (`None`, `?`, `*`, `+`, N, `PARSER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum N {
    One,
    Opt,
    Star,
    Plus,
    Num(usize),
    Parser,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Store,
    StoreConst,
    Append,
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Metavar {
    One(String),
    Tuple(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Help {
    Unset,
    Text(String),
    Suppress,
}

impl Help {
    fn text(&self) -> Option<&str> {
        match self {
            Help::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// One `add_argument(...)`.
#[derive(Clone, Debug)]
pub struct Arg {
    option_strings: Vec<String>,
    dest: String,
    kind: Kind,
    nargs: Option<Nargs>,
    konst: Value,
    default: Value,
    default_set: bool,
    default_suppress: bool,
    ty: Type,
    choices: Option<Vec<String>>,
    required: bool,
    help: Help,
    metavar: Option<Metavar>,
}

impl Arg {
    fn base(option_strings: Vec<String>, dest: String) -> Arg {
        Arg {
            option_strings,
            dest,
            kind: Kind::Store,
            nargs: None,
            konst: Value::None,
            default: Value::None,
            default_set: false,
            default_suppress: false,
            ty: Type::Str,
            choices: None,
            required: false,
            help: Help::Unset,
            metavar: None,
        }
    }

    /// `add_argument(dest)`.
    pub fn positional(dest: &str) -> Arg {
        Arg::base(Vec::new(), dest.to_string())
    }

    /// `add_argument(*flags)`: the dest is the first long option (else the first option)
    /// without its leading dashes, `-` replaced by `_`.
    pub fn option(flags: &[&str]) -> Arg {
        assert!(
            !flags.is_empty(),
            "argparse: an option needs an option string"
        );
        for f in flags {
            assert!(
                f.starts_with('-'),
                "argparse: invalid option string {}: must start with a character '-'",
                repr(f)
            );
        }
        let long = flags
            .iter()
            .find(|f| f.chars().count() > 1 && f.as_bytes()[1] == b'-');
        let dest = long.unwrap_or(&flags[0]).trim_start_matches('-');
        assert!(
            !dest.is_empty(),
            "argparse: dest= is required for options like {}",
            repr(flags[0])
        );
        Arg::base(
            flags.iter().map(|f| f.to_string()).collect(),
            dest.replace('-', "_"),
        )
    }

    /// `dest=`.
    pub fn dest(mut self, dest: &str) -> Arg {
        self.dest = dest.to_string();
        self
    }

    /// `nargs=`.
    pub fn nargs(mut self, n: Nargs) -> Arg {
        self.nargs = Some(n);
        self
    }

    /// `action="store_true"` (default `False` unless one is given).
    pub fn store_true(self) -> Arg {
        self.store_bool(true)
    }

    /// `action="store_false"` (default `True` unless one is given).
    pub fn store_false(self) -> Arg {
        self.store_bool(false)
    }

    fn store_bool(mut self, v: bool) -> Arg {
        self.kind = Kind::StoreConst;
        self.konst = Value::Bool(v);
        if !self.default_set {
            self.default = Value::Bool(!v);
        }
        self
    }

    /// `action="store_const", const=v`.
    pub fn store_const(mut self, v: Value) -> Arg {
        self.kind = Kind::StoreConst;
        self.konst = v;
        self
    }

    /// `action="append"`.
    pub fn append(mut self) -> Arg {
        self.kind = Kind::Append;
        self
    }

    /// `const=` (the value of an option with `nargs="?"` given without one).
    pub fn konst(mut self, v: Value) -> Arg {
        self.konst = v;
        self
    }

    /// `default=` (a str default is converted by `type` when the argument is absent).
    pub fn default(mut self, v: Value) -> Arg {
        self.default = v;
        self.default_set = true;
        self
    }

    /// `type=`.
    pub fn ty(mut self, t: Type) -> Arg {
        self.ty = t;
        self
    }

    /// `choices=` (strs).
    pub fn choices(mut self, c: &[&str]) -> Arg {
        self.choices = Some(c.iter().map(|s| s.to_string()).collect());
        self
    }

    /// `required=True` (options only).
    pub fn required(mut self) -> Arg {
        assert!(
            !self.option_strings.is_empty(),
            "argparse: 'required' is an invalid argument for positionals"
        );
        self.required = true;
        self
    }

    /// `help=`.
    pub fn help(mut self, h: &str) -> Arg {
        self.help = Help::Text(h.to_string());
        self
    }

    /// `help=argparse.SUPPRESS`.
    pub fn help_suppress(mut self) -> Arg {
        self.help = Help::Suppress;
        self
    }

    /// `metavar=`.
    pub fn metavar(mut self, m: &str) -> Arg {
        self.metavar = Some(Metavar::One(m.to_string()));
        self
    }

    /// `metavar=(...)`, a tuple.
    pub fn metavars(mut self, m: &[&str]) -> Arg {
        self.metavar = Some(Metavar::Tuple(m.iter().map(|s| s.to_string()).collect()));
        self
    }

    fn n(&self) -> N {
        match self.kind {
            Kind::StoreConst | Kind::Help => N::Num(0),
            _ => match self.nargs {
                None => N::One,
                Some(Nargs::Optional) => N::Opt,
                Some(Nargs::ZeroOrMore) => N::Star,
                Some(Nargs::OneOrMore) => N::Plus,
                Some(Nargs::Exactly(n)) => N::Num(n),
            },
        }
    }
}

/// `add_subparsers(...)` and its `add_parser(...)` calls.
#[derive(Clone, Debug, Default)]
pub struct Subcommands {
    dest: Option<String>,
    required: bool,
    help: Option<String>,
    metavar: Option<String>,
    prog: Option<String>,
    parsers: Vec<(String, Option<String>, Spec)>,
}

impl Subcommands {
    /// `add_subparsers()`.
    pub fn new() -> Subcommands {
        Subcommands::default()
    }

    /// `dest=` (without one the command name is not stored).
    pub fn dest(mut self, d: &str) -> Subcommands {
        self.dest = Some(d.to_string());
        self
    }

    /// `required=True`.
    pub fn required(mut self) -> Subcommands {
        self.required = true;
        self
    }

    /// `help=`.
    pub fn help(mut self, h: &str) -> Subcommands {
        self.help = Some(h.to_string());
        self
    }

    /// `metavar=`.
    pub fn metavar(mut self, m: &str) -> Subcommands {
        self.metavar = Some(m.to_string());
        self
    }

    /// `prog=` (the prefix of each subcommand's prog; by default the parent's prog and the
    /// positionals before the subcommands).
    pub fn prog(mut self, p: &str) -> Subcommands {
        self.prog = Some(p.to_string());
        self
    }

    /// `add_parser(name, help=help)`, the subcommand's arguments in `spec` (made with
    /// [`Spec::command`], or [`Spec::new`] for an explicit `prog`).
    pub fn parser(mut self, name: &str, help: Option<&str>, spec: Spec) -> Subcommands {
        assert!(
            self.parsers.iter().all(|(n, _, _)| n != name),
            "argparse: conflicting subparser: {name}"
        );
        self.parsers
            .push((name.to_string(), help.map(str::to_string), spec));
        self
    }
}

#[derive(Clone, Debug)]
enum Action {
    Arg(Arg),
    Sub(Subcommands),
}

impl Action {
    fn option_strings(&self) -> &[String] {
        match self {
            Action::Arg(a) => &a.option_strings,
            Action::Sub(_) => &[],
        }
    }

    fn is_optional(&self) -> bool {
        !self.option_strings().is_empty()
    }

    fn dest(&self) -> Option<&str> {
        match self {
            Action::Arg(a) => Some(&a.dest),
            Action::Sub(s) => s.dest.as_deref(),
        }
    }

    fn n(&self) -> N {
        match self {
            Action::Arg(a) => a.n(),
            Action::Sub(_) => N::Parser,
        }
    }

    fn required(&self) -> bool {
        match self {
            Action::Arg(a) => a.required,
            Action::Sub(s) => s.required,
        }
    }

    fn help(&self) -> Help {
        match self {
            Action::Arg(a) => a.help.clone(),
            Action::Sub(s) => s.help.clone().map_or(Help::Unset, Help::Text),
        }
    }

    fn suppressed(&self) -> bool {
        self.help() == Help::Suppress
    }

    fn metavar(&self) -> Option<Metavar> {
        match self {
            Action::Arg(a) => a.metavar.clone(),
            Action::Sub(s) => s.metavar.clone().map(Metavar::One),
        }
    }

    fn choices(&self) -> Option<Vec<&str>> {
        match self {
            Action::Arg(a) => a
                .choices
                .as_ref()
                .map(|c| c.iter().map(String::as_str).collect()),
            Action::Sub(s) => Some(s.parsers.iter().map(|(n, _, _)| n.as_str()).collect()),
        }
    }

    fn default(&self) -> Value {
        match self {
            Action::Arg(a) => a.default.clone(),
            Action::Sub(_) => Value::None,
        }
    }

    fn default_suppressed(&self) -> bool {
        matches!(self, Action::Arg(a) if a.default_suppress)
    }

    fn konst(&self) -> Value {
        match self {
            Action::Arg(a) => a.konst.clone(),
            Action::Sub(_) => Value::None,
        }
    }

    fn ty(&self) -> Type {
        match self {
            Action::Arg(a) => a.ty,
            Action::Sub(_) => Type::Str,
        }
    }

    /// The choices' pseudo-actions a subcommands action lists in the help.
    fn subactions(&self) -> Vec<Action> {
        match self {
            Action::Arg(_) => Vec::new(),
            Action::Sub(s) => s
                .parsers
                .iter()
                .filter_map(|(name, help, _)| {
                    help.as_ref()
                        .map(|h| Action::Arg(Arg::positional(name).metavar(name).help(h)))
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
struct Group {
    required: bool,
    members: Vec<usize>,
}

/// An `ArgumentParser`: see the module docs.
#[derive(Clone, Debug)]
pub struct Spec {
    prog: Option<String>,
    description: Option<String>,
    epilog: Option<String>,
    width: i64,
    allow_abbrev: bool,
    actions: Vec<Action>,
    groups: Vec<Group>,
}

fn help_action() -> Arg {
    let mut a = Arg::option(&["-h", "--help"]).help("show this help message and exit");
    a.kind = Kind::Help;
    a.default_suppress = true;
    a
}

impl Spec {
    /// `ArgumentParser(prog=prog)` (with `-h/--help`).
    pub fn new(prog: &str) -> Spec {
        Spec {
            prog: Some(prog.to_string()),
            ..Spec::command()
        }
    }

    /// A subcommand's parser, `add_parser(name)`: its prog is the parent's prefix and `name`.
    pub fn command() -> Spec {
        Spec {
            prog: None,
            description: None,
            epilog: None,
            width: DEFAULT_WIDTH,
            allow_abbrev: true,
            actions: vec![Action::Arg(help_action())],
            groups: Vec::new(),
        }
    }

    /// `add_help=False` (before any argument is added).
    pub fn no_help(mut self) -> Spec {
        assert!(
            self.actions.len() == 1 && self.groups.is_empty(),
            "argparse: no_help() comes before the arguments"
        );
        self.actions.clear();
        self
    }

    /// `description=`.
    pub fn description(mut self, d: &str) -> Spec {
        self.description = Some(d.to_string());
        self
    }

    /// `epilog=`.
    pub fn epilog(mut self, e: &str) -> Spec {
        self.epilog = Some(e.to_string());
        self
    }

    /// `HelpFormatter`'s width (see [`help_width`]); a subcommand uses its top parser's.
    pub fn width(mut self, w: i64) -> Spec {
        self.width = w;
        self
    }

    /// `allow_abbrev=`.
    pub fn allow_abbrev(mut self, on: bool) -> Spec {
        self.allow_abbrev = on;
        self
    }

    fn push(&mut self, mut a: Arg) -> usize {
        let opt = !a.option_strings.is_empty();
        if !opt {
            match a.nargs {
                Some(Nargs::Exactly(0)) => panic!("argparse: nargs for positionals must be != 0"),
                Some(Nargs::Optional) | Some(Nargs::ZeroOrMore) => {}
                _ => a.required = true,
            }
        }
        if a.kind == Kind::Store || a.kind == Kind::Append {
            assert!(
                a.nargs != Some(Nargs::Exactly(0)),
                "argparse: nargs for store actions must be != 0; if you have nothing to store, \
                 actions such as store true or store const may be more appropriate"
            );
        }
        if opt && a.metavar.is_none() && a.n() != N::Num(0) {
            assert!(
                a.dest.is_ascii() || a.choices.is_some(),
                "argparse: a non-ASCII dest needs a metavar (str.upper is not ported)"
            );
        }
        if let Some(Metavar::Tuple(t)) = &a.metavar {
            let ok = match a.n() {
                N::One | N::Opt | N::Parser => t.len() == 1,
                N::Star => t.len() == 1 || t.len() == 2,
                N::Plus => t.len() == 2,
                N::Num(n) => t.len() == n,
            };
            assert!(ok, "argparse: length of metavar tuple does not match nargs");
        }
        for s in &a.option_strings {
            let taken = self.actions.iter().any(|x| x.option_strings().contains(s));
            assert!(
                !taken,
                "argparse: argument {}: conflicting option string: {s}",
                a.option_strings.join("/")
            );
        }
        self.actions.push(Action::Arg(a));
        self.actions.len() - 1
    }

    /// `add_argument(...)`.
    pub fn arg(mut self, a: Arg) -> Spec {
        self.push(a);
        self
    }

    /// `add_mutually_exclusive_group(required=required)` and its `add_argument` calls.
    pub fn mutex(mut self, required: bool, args: Vec<Arg>) -> Spec {
        let mut members = Vec::new();
        for a in args {
            assert!(
                !a.option_strings.is_empty()
                    || matches!(a.nargs, Some(Nargs::Optional) | Some(Nargs::ZeroOrMore)),
                "argparse: mutually exclusive arguments must be optional"
            );
            members.push(self.push(a));
        }
        self.groups.push(Group { required, members });
        self
    }

    /// `add_subparsers(...)` with its parsers (one per parser, as in Python).
    pub fn subcommands(mut self, s: Subcommands) -> Spec {
        assert!(
            !self.actions.iter().any(|a| matches!(a, Action::Sub(_))),
            "argparse: cannot have multiple subparser arguments"
        );
        self.actions.push(Action::Sub(s));
        self
    }
}

/// What `parse_args` returned: the namespace, by dest, in Python's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub values: Vec<(String, Value)>,
}

impl Parsed {
    /// `getattr(ns, dest)`.
    pub fn get(&self, dest: &str) -> Option<&Value> {
        self.values.iter().find(|(k, _)| k == dest).map(|(_, v)| v)
    }

    /// The str at `dest` (UTF-8 only).
    pub fn str(&self, dest: &str) -> Option<&str> {
        self.get(dest).and_then(Value::as_str)
    }

    /// The str at `dest`, its bytes.
    pub fn os_str(&self, dest: &str) -> Option<&OsStr> {
        self.get(dest).and_then(Value::as_os_str)
    }

    /// The bool at `dest` (`False` when it is not a bool).
    pub fn flag(&self, dest: &str) -> bool {
        self.get(dest).and_then(Value::as_bool).unwrap_or(false)
    }

    /// The int at `dest`, if it fits an `i64`.
    pub fn i64(&self, dest: &str) -> Option<i64> {
        self.get(dest).and_then(Value::as_i64)
    }

    /// The float at `dest`.
    pub fn f64(&self, dest: &str) -> Option<f64> {
        self.get(dest).and_then(Value::as_f64)
    }

    /// The list at `dest` (empty when it is `None` or not a list).
    pub fn list(&self, dest: &str) -> &[Value] {
        self.get(dest).and_then(Value::as_list).unwrap_or(&[])
    }

    /// The UTF-8 strs of the list at `dest`.
    pub fn strs(&self, dest: &str) -> Vec<&str> {
        self.list(dest).iter().filter_map(Value::as_str).collect()
    }
}

/// Where Python's `parse_args` exits: `code` and what it prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.code == 0 {
            f.write_str(&self.stdout)
        } else {
            f.write_str(&self.stderr)
        }
    }
}

impl std::error::Error for UsageError {}

/// `parser.parse_args(args)`; `args` excludes the program name.
pub fn parse(spec: &Spec, args: &[OsString]) -> Result<Parsed, UsageError> {
    let p = Parser::new(spec, spec.prog.clone().unwrap_or_default(), spec.width);
    let args: Vec<Bytes> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
    let (ns, extras) = p.parse_known_args(args)?;
    if !extras.is_empty() {
        let shown: Vec<String> = extras.iter().map(|a| display(a)).collect();
        return Err(p.error(&format!("unrecognized arguments: {}", shown.join(" "))));
    }
    Ok(Parsed {
        values: ns.items.into_iter().map(|(k, v, _)| (k, v)).collect(),
    })
}

/// `parser.format_help()`.
pub fn format_help(spec: &Spec) -> String {
    Parser::new(spec, spec.prog.clone().unwrap_or_default(), spec.width).format_help()
}

/// `parser.format_usage()`.
pub fn format_usage(spec: &Spec) -> String {
    Parser::new(spec, spec.prog.clone().unwrap_or_default(), spec.width).format_usage()
}

/// `parser.error(message)`: what it prints, exit status 2.
pub fn error(spec: &Spec, message: &str) -> UsageError {
    Parser::new(spec, spec.prog.clone().unwrap_or_default(), spec.width).error(message)
}

type Bytes = Vec<u8>;

/// One `(action, option_string, sep, explicit_arg)` of `_parse_optional`.
#[derive(Clone, Debug)]
struct OptTuple {
    action: Option<usize>,
    option_string: Bytes,
    sep: Option<&'static str>,
    explicit: Option<Bytes>,
}

enum Fail {
    /// An `ArgumentError`'s text: the parser that raised it reports it.
    Arg(String),
    /// A finished exit (help, or a subcommand's error).
    Exit(UsageError),
}

/// A namespace: (dest, value, still the action's default object).
#[derive(Default)]
struct Ns {
    items: Vec<(String, Value, bool)>,
    unrecognized: Vec<Bytes>,
}

impl Ns {
    fn get(&self, k: &str) -> Option<&Value> {
        self.items
            .iter()
            .find(|(d, _, _)| d == k)
            .map(|(_, v, _)| v)
    }

    fn is_default(&self, k: &str) -> bool {
        self.items.iter().any(|(d, _, def)| d == k && *def)
    }

    fn set(&mut self, k: &str, v: Value, is_default: bool) {
        match self.items.iter_mut().find(|(d, _, _)| d == k) {
            Some(e) => {
                e.1 = v;
                e.2 = is_default;
            }
            None => self.items.push((k.to_string(), v, is_default)),
        }
    }
}

/// The byte length of the first code point of `b` (an invalid byte counts as one, as
/// `surrogateescape` makes it one).
fn first_unit(b: &[u8]) -> usize {
    match b.utf8_chunks().next() {
        Some(c) if !c.valid().is_empty() => c.valid().chars().next().map_or(0, char::len_utf8),
        Some(c) if !c.invalid().is_empty() => 1,
        _ => 0,
    }
}

/// `len(s)` of bytes decoded with `surrogateescape`.
fn unit_len(b: &[u8]) -> usize {
    b.utf8_chunks()
        .map(|c| c.valid().chars().count() + c.invalid().len())
        .sum()
}

/// `s.partition("=")`: (before, sep, after).
fn partition(s: &[u8]) -> (Bytes, Option<&'static str>, Option<Bytes>) {
    match s.iter().position(|&c| c == b'=') {
        Some(i) => (s[..i].to_vec(), Some("="), Some(s[i + 1..].to_vec())),
        None => (s.to_vec(), None, None),
    }
}

fn cached(pattern: &str) -> re::Regex {
    static CACHE: OnceLock<Mutex<HashMap<String, re::Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(pattern.to_string())
        .or_insert_with(|| {
            re::compile(pattern, false)
                .unwrap_or_else(|e| panic!("argparse: pattern {pattern:?}: {e}"))
        })
        .clone()
}

/// `re.match(pattern, s)`'s groups (an argparse pattern needs no backtrack limit).
fn match_groups(pattern: &str, s: &str) -> Option<re::Groups> {
    cached(&format!("\\A(?:{pattern})"))
        .captures_at(s, 0)
        .ok()
        .flatten()
}

/// `_negative_number_matcher.match(s)`.
fn looks_negative(s: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(s) else {
        return false;
    };
    cached(r"^-\d+$|^-\d*\.\d+$").is_match(s).unwrap_or(false)
}

/// `str(pathlib.PurePosixPath(s))`.
fn pathlib_str(s: &[u8]) -> Bytes {
    let root: &[u8] = if s.starts_with(b"//") && !s.starts_with(b"///") {
        b"//"
    } else if s.starts_with(b"/") {
        b"/"
    } else {
        b""
    };
    let parts: Vec<&[u8]> = s
        .split(|&c| c == b'/')
        .filter(|p| !p.is_empty() && *p != b".")
        .collect();
    let mut out = root.to_vec();
    out.extend(parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

struct Parser<'a> {
    spec: &'a Spec,
    prog: String,
    width: i64,
    /// `_option_string_actions`, in insertion order.
    opts: Vec<(&'a [u8], usize)>,
    negative_like: bool,
}

impl<'a> Parser<'a> {
    fn new(spec: &'a Spec, prog: String, width: i64) -> Parser<'a> {
        let mut opts = Vec::new();
        for (i, a) in spec.actions.iter().enumerate() {
            for s in a.option_strings() {
                opts.push((s.as_bytes(), i));
            }
        }
        let negative_like = opts.iter().any(|(s, _)| looks_negative(s));
        Parser {
            spec,
            prog,
            width,
            opts,
            negative_like,
        }
    }

    fn action(&self, i: usize) -> &'a Action {
        &self.spec.actions[i]
    }

    fn lookup(&self, s: &[u8]) -> Option<usize> {
        self.opts.iter().find(|(o, _)| *o == s).map(|&(_, i)| i)
    }

    fn fmt(&self) -> Fmt<'_> {
        Fmt::new(self.spec, &self.prog, self.width)
    }

    fn format_help(&self) -> String {
        self.fmt().format_help()
    }

    fn format_usage(&self) -> String {
        let f = self.fmt();
        let all: Vec<usize> = (0..self.spec.actions.len()).collect();
        finish(f.format_usage(&all, "usage: "))
    }

    /// `parser.error(message)`.
    fn error(&self, message: &str) -> UsageError {
        UsageError {
            code: 2,
            stdout: String::new(),
            stderr: format!("{}{}: error: {message}\n", self.format_usage(), self.prog),
        }
    }

    /// `ArgumentError(action, msg)`'s text.
    fn arg_error(&self, a: usize, msg: &str) -> Fail {
        match action_name(self.action(a)) {
            Some(name) => Fail::Arg(format!("argument {name}: {msg}")),
            None => Fail::Arg(msg.to_string()),
        }
    }

    /// `parse_known_args(args)` on a fresh namespace, this parser reporting its errors.
    fn parse_known_args(&self, args: Vec<Bytes>) -> Result<(Ns, Vec<Bytes>), UsageError> {
        let mut ns = Ns::default();
        for a in &self.spec.actions {
            if let Some(dest) = a.dest()
                && !a.default_suppressed()
                && ns.get(dest).is_none()
            {
                ns.set(dest, a.default(), true);
            }
        }
        match self.parse_inner(args, &mut ns) {
            Ok(mut extras) => {
                extras.append(&mut ns.unrecognized);
                Ok((ns, extras))
            }
            Err(Fail::Arg(msg)) => Err(self.error(&msg)),
            Err(Fail::Exit(u)) => Err(u),
        }
    }

    /// `_parse_optional(arg_string)`.
    fn parse_optional(&self, arg: &[u8]) -> Option<Vec<OptTuple>> {
        if arg.is_empty() || arg[0] != b'-' {
            return None;
        }
        if let Some(a) = self.lookup(arg) {
            return Some(vec![OptTuple {
                action: Some(a),
                option_string: arg.to_vec(),
                sep: None,
                explicit: None,
            }]);
        }
        if unit_len(arg) == 1 {
            return None;
        }
        let (prefix, sep, explicit) = partition(arg);
        if sep.is_some()
            && let Some(a) = self.lookup(&prefix)
        {
            return Some(vec![OptTuple {
                action: Some(a),
                option_string: prefix,
                sep,
                explicit,
            }]);
        }
        let tuples = self.option_tuples(arg);
        if !tuples.is_empty() {
            return Some(tuples);
        }
        if looks_negative(arg) && !self.negative_like {
            return None;
        }
        if arg.contains(&b' ') {
            return None;
        }
        Some(vec![OptTuple {
            action: None,
            option_string: arg.to_vec(),
            sep: None,
            explicit: None,
        }])
    }

    /// `_get_option_tuples(option_string)`.
    fn option_tuples(&self, s: &[u8]) -> Vec<OptTuple> {
        let mut out = Vec::new();
        let (prefix, sep, explicit) = partition(s);
        if s[1] == b'-' {
            if self.spec.allow_abbrev {
                for &(o, a) in &self.opts {
                    if o.starts_with(&prefix) {
                        out.push(OptTuple {
                            action: Some(a),
                            option_string: o.to_vec(),
                            sep,
                            explicit: explicit.clone(),
                        });
                    }
                }
            }
        } else {
            let cut = 1 + first_unit(&s[1..]);
            let (short, short_explicit) = (&s[..cut], &s[cut..]);
            for &(o, a) in &self.opts {
                if o == short {
                    out.push(OptTuple {
                        action: Some(a),
                        option_string: o.to_vec(),
                        sep: Some(""),
                        explicit: Some(short_explicit.to_vec()),
                    });
                } else if self.spec.allow_abbrev && o.starts_with(&prefix) {
                    out.push(OptTuple {
                        action: Some(a),
                        option_string: o.to_vec(),
                        sep,
                        explicit: explicit.clone(),
                    });
                }
            }
        }
        out
    }

    /// `_match_argument(action, pattern)`.
    fn match_argument(&self, a: usize, pattern: &[u8]) -> Result<usize, Fail> {
        let action = self.action(a);
        let text = std::str::from_utf8(pattern).unwrap_or_default();
        match match_groups(&nargs_pattern(action), text) {
            Some(g) => Ok(g[1].map_or(0, |(s, e)| e - s)),
            None => {
                let msg = match action.n() {
                    N::One => "expected one argument".to_string(),
                    N::Opt => "expected at most one argument".to_string(),
                    N::Plus => "expected at least one argument".to_string(),
                    N::Num(1) => "expected 1 argument".to_string(),
                    N::Num(n) => format!("expected {n} arguments"),
                    N::Star => "expected * arguments".to_string(),
                    N::Parser => "expected A... arguments".to_string(),
                };
                Err(self.arg_error(a, &msg))
            }
        }
    }

    /// `_match_arguments_partial(actions, pattern)`.
    fn match_partial(&self, positionals: &[usize], pattern: &[u8]) -> Vec<usize> {
        let text = std::str::from_utf8(pattern).unwrap_or_default();
        for i in (1..=positionals.len()).rev() {
            let pat: String = positionals[..i]
                .iter()
                .map(|&a| nargs_pattern(self.action(a)))
                .collect();
            if let Some(g) = match_groups(&pat, text) {
                let mut result: Vec<usize> =
                    g[1..].iter().map(|x| x.map_or(0, |(s, e)| e - s)).collect();
                let end = g[0].map_or(0, |(_, e)| e);
                if end < pattern.len() && pattern[end] == b'O' {
                    while result.last() == Some(&0) {
                        result.pop();
                    }
                }
                return result;
            }
        }
        Vec::new()
    }

    /// `_get_value(action, arg_string)`.
    fn get_value(&self, a: usize, s: &[u8]) -> Result<Value, Fail> {
        let action = self.action(a);
        let ty = action.ty();
        let text = std::str::from_utf8(s).ok();
        let v = match ty {
            Type::Str => Some(Value::Str(OsString::from_vec(s.to_vec()))),
            Type::Path => Some(Value::Str(OsString::from_vec(pathlib_str(s)))),
            Type::Int => text.and_then(py_int).map(Value::Int),
            Type::Float => text.and_then(py_float).map(Value::Float),
        };
        v.ok_or_else(|| {
            self.arg_error(
                a,
                &format!("invalid {} value: {}", ty.name(), repr_surrogateescape(s)),
            )
        })
    }

    /// `_check_value(action, value)`.
    fn check_value(&self, a: usize, v: &Value, converted: bool) -> Result<(), Fail> {
        let action = self.action(a);
        let Some(choices) = action.choices() else {
            return Ok(());
        };
        // A `Path` never equals a str choice, nor does an int or a float.
        let found = match v {
            Value::Str(s) if !converted || action.ty() != Type::Path => {
                choices.iter().any(|c| c.as_bytes() == s.as_bytes())
            }
            _ => false,
        };
        if found {
            return Ok(());
        }
        let shown = match v {
            Value::Str(s) => repr_surrogateescape(s.as_bytes()),
            v => repr(&v.py_str()),
        };
        Err(self.arg_error(
            a,
            &format!(
                "invalid choice: {shown} (choose from {})",
                choices.join(", ")
            ),
        ))
    }

    /// `_get_values(action, arg_strings)`.
    fn get_values(&self, a: usize, args: &[Bytes]) -> Result<Value, Fail> {
        let action = self.action(a);
        let n = action.n();
        if args.is_empty() && n == N::Opt {
            let v = if action.is_optional() {
                action.konst()
            } else {
                action.default()
            };
            if let Value::Str(s) = &v {
                let v = self.get_value(a, s.as_bytes())?;
                self.check_value(a, &v, true)?;
                return Ok(v);
            }
            return Ok(v);
        }
        if args.is_empty() && n == N::Star && !action.is_optional() {
            let d = action.default();
            if d.is_none() {
                return Ok(Value::List(Vec::new()));
            }
            self.check_value(a, &d, false)?;
            return Ok(d);
        }
        if args.len() == 1 && matches!(n, N::One | N::Opt) {
            let v = self.get_value(a, &args[0])?;
            self.check_value(a, &v, true)?;
            return Ok(v);
        }
        let mut values = Vec::with_capacity(args.len());
        for s in args {
            values.push(self.get_value(a, s)?);
        }
        if n == N::Parser {
            if let Some(first) = values.first() {
                self.check_value(a, first, true)?;
            }
        } else {
            for v in &values {
                self.check_value(a, v, true)?;
            }
        }
        Ok(Value::List(values))
    }

    /// The action's `__call__`.
    fn call(&self, a: usize, values: Value, ns: &mut Ns) -> Result<(), Fail> {
        match self.action(a) {
            Action::Arg(arg) => match arg.kind {
                Kind::Store => ns.set(&arg.dest, values, false),
                Kind::StoreConst => ns.set(&arg.dest, arg.konst.clone(), false),
                Kind::Append => {
                    let mut items = match ns.get(&arg.dest) {
                        Some(Value::List(l)) => l.clone(),
                        _ => Vec::new(),
                    };
                    items.push(values);
                    ns.set(&arg.dest, Value::List(items), false);
                }
                Kind::Help => {
                    return Err(Fail::Exit(UsageError {
                        code: 0,
                        stdout: self.format_help(),
                        stderr: String::new(),
                    }));
                }
            },
            Action::Sub(sub) => {
                let Value::List(values) = values else {
                    unreachable!("a subcommands action gets a list")
                };
                let mut strings: Vec<Bytes> = values
                    .into_iter()
                    .map(|v| match v {
                        Value::Str(s) => s.into_vec(),
                        _ => unreachable!("subcommand arguments are strs"),
                    })
                    .collect();
                let name = strings.remove(0);
                if let Some(d) = &sub.dest {
                    ns.set(d, Value::Str(OsString::from_vec(name.clone())), false);
                }
                let Some((cmd, _, spec)) =
                    sub.parsers.iter().find(|(n, _, _)| n.as_bytes() == name)
                else {
                    unreachable!("_check_value accepted the name")
                };
                let prog = match &spec.prog {
                    Some(p) => p.clone(),
                    None => format!("{} {cmd}", self.sub_prefix(a)),
                };
                let child = Parser::new(spec, prog, self.width);
                let (subns, extras) = child.parse_known_args(strings).map_err(Fail::Exit)?;
                for (k, v, _) in subns.items {
                    ns.set(&k, v, false);
                }
                ns.unrecognized.extend(extras);
            }
        }
        Ok(())
    }

    /// `add_subparsers`' default prog: this prog and the positionals before it, as usage.
    fn sub_prefix(&self, a: usize) -> String {
        if let Action::Sub(Subcommands { prog: Some(p), .. }) = self.action(a) {
            return p.clone();
        }
        let positionals: Vec<usize> = (0..a).filter(|&i| !self.action(i).is_optional()).collect();
        let f = self.fmt();
        strip_ws(&finish(f.format_usage(&positionals, ""))).to_string()
    }

    /// `_parse_known_args(arg_strings, namespace)`.
    fn parse_inner(&self, args: Vec<Bytes>, ns: &mut Ns) -> Result<Vec<Bytes>, Fail> {
        let count = self.spec.actions.len();
        let mut conflicts: Vec<Vec<usize>> = vec![Vec::new(); count];
        for g in &self.spec.groups {
            for (i, &m) in g.members.iter().enumerate() {
                conflicts[m].extend(g.members[..i].iter().copied());
                conflicts[m].extend(g.members[i + 1..].iter().copied());
            }
        }
        let mut indices = BTreeMap::new();
        let mut pattern = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            if arg == b"--" {
                pattern.push(b'-');
                pattern.resize(args.len(), b'A');
                break;
            }
            match self.parse_optional(arg) {
                None => pattern.push(b'A'),
                Some(t) => {
                    indices.insert(i, t);
                    pattern.push(b'O');
                }
            }
        }
        let positionals = (0..count)
            .filter(|&i| !self.action(i).is_optional())
            .collect();
        let mut run = Run {
            p: self,
            args,
            pattern,
            indices,
            ns,
            seen: HashSet::new(),
            seen_non_default: HashSet::new(),
            conflicts,
            extras: Vec::new(),
            positionals,
        };
        run.run()
    }
}

/// `_get_nargs_pattern(action)`.
fn nargs_pattern(a: &Action) -> String {
    let opt = a.is_optional();
    let p = |o: &str, p: &str| if opt { o } else { p }.to_string();
    match a.n() {
        N::One => p("([A])", "(-*A-*)"),
        N::Opt => p("(A?)", "(-*A?-*)"),
        N::Star => p("(A*)", "(-*[A-]*)"),
        N::Plus => p("(A+)", "(-*A[A-]*)"),
        N::Parser => p("(A[AO]*)", "(-*A[-AO]*)"),
        N::Num(n) => {
            if opt {
                format!("([AO]{{{n}}})")
            } else {
                format!("((?:-*A){{{n}}}-*)")
            }
        }
    }
}

/// `_get_action_name(argument)`.
fn action_name(a: &Action) -> Option<String> {
    if a.is_optional() {
        return Some(a.option_strings().join("/"));
    }
    match a.metavar() {
        Some(Metavar::One(m)) => return Some(m),
        Some(Metavar::Tuple(t)) => {
            return Some(match a.n() {
                N::Star if t.len() == 2 => format!("{}[, {}]", t[0], t[1]),
                N::Plus => format!("{}[, {}]", t[0], t[1]),
                _ => t.join(", "),
            });
        }
        None => {}
    }
    if let Some(d) = a.dest() {
        return Some(d.to_string());
    }
    a.choices()
        .filter(|c| !c.is_empty())
        .map(|c| format!("{{{}}}", c.join(",")))
}

/// One `_parse_known_args` run's state.
struct Run<'p, 'a, 'n> {
    p: &'p Parser<'a>,
    args: Vec<Bytes>,
    pattern: Vec<u8>,
    indices: BTreeMap<usize, Vec<OptTuple>>,
    ns: &'n mut Ns,
    seen: HashSet<usize>,
    seen_non_default: HashSet<usize>,
    conflicts: Vec<Vec<usize>>,
    extras: Vec<Bytes>,
    positionals: Vec<usize>,
}

impl Run<'_, '_, '_> {
    fn take_action(&mut self, a: usize, args: Vec<Bytes>) -> Result<(), Fail> {
        self.seen.insert(a);
        let values = self.p.get_values(a, &args)?;
        if self.p.action(a).is_optional() || !args.is_empty() {
            self.seen_non_default.insert(a);
            for &c in &self.conflicts[a] {
                if self.seen_non_default.contains(&c) {
                    let name = action_name(self.p.action(c)).unwrap_or_else(|| "None".into());
                    return Err(self
                        .p
                        .arg_error(a, &format!("not allowed with argument {name}")));
                }
            }
        }
        self.p.call(a, values, self.ns)
    }

    fn consume_optional(&mut self, start: usize) -> Result<usize, Fail> {
        let tuples = self.indices[&start].clone();
        if tuples.len() > 1 {
            let options: Vec<String> = tuples.iter().map(|t| display(&t.option_string)).collect();
            return Err(Fail::Arg(format!(
                "ambiguous option: {} could match {}",
                display(&self.args[start]),
                options.join(", ")
            )));
        }
        let t = tuples.into_iter().next().expect("one option tuple");
        let (mut action, mut option_string, mut sep, mut explicit) =
            (t.action, t.option_string, t.sep, t.explicit);
        let mut todo: Vec<(usize, Vec<Bytes>)> = Vec::new();
        let stop;
        loop {
            let Some(a) = action else {
                self.extras.push(self.args[start].clone());
                return Ok(start + 1);
            };
            if let Some(exp) = explicit.clone() {
                let count = self.p.match_argument(a, b"A")?;
                let second_dash = option_string.get(1) == Some(&b'-');
                if count == 0 && !second_dash && !exp.is_empty() {
                    if sep == Some("=") || exp[0] == b'-' {
                        return Err(self.p.arg_error(
                            a,
                            &format!("ignored explicit argument {}", repr_surrogateescape(&exp)),
                        ));
                    }
                    todo.push((a, Vec::new()));
                    let ch = option_string[0];
                    let cut = first_unit(&exp);
                    let mut next = vec![ch];
                    next.extend_from_slice(&exp[..cut]);
                    option_string = next;
                    match self.p.lookup(&option_string) {
                        Some(na) => {
                            action = Some(na);
                            let rest = &exp[cut..];
                            if rest.is_empty() {
                                sep = None;
                                explicit = None;
                            } else if rest[0] == b'=' {
                                sep = Some("=");
                                explicit = Some(rest[1..].to_vec());
                            } else {
                                sep = Some("");
                                explicit = Some(rest.to_vec());
                            }
                        }
                        None => {
                            let mut e = vec![ch];
                            e.extend_from_slice(&exp);
                            self.extras.push(e);
                            stop = start + 1;
                            break;
                        }
                    }
                } else if count == 1 {
                    stop = start + 1;
                    todo.push((a, vec![exp]));
                    break;
                } else {
                    return Err(self.p.arg_error(
                        a,
                        &format!("ignored explicit argument {}", repr_surrogateescape(&exp)),
                    ));
                }
            } else {
                let s = start + 1;
                let count = self.p.match_argument(a, &self.pattern[s..])?;
                stop = s + count;
                todo.push((a, self.args[s..stop].to_vec()));
                break;
            }
        }
        for (a, args) in todo {
            self.take_action(a, args)?;
        }
        Ok(stop)
    }

    fn consume_positionals(&mut self, start: usize) -> Result<usize, Fail> {
        let counts = self
            .p
            .match_partial(&self.positionals, &self.pattern[start..]);
        let acts: Vec<usize> = self.positionals[..counts.len()].to_vec();
        let mut start = start;
        for (&a, &count) in acts.iter().zip(&counts) {
            let mut args = self.args[start..start + count].to_vec();
            let dashed = if self.p.action(a).n() == N::Parser {
                self.pattern[start] == b'-'
            } else {
                self.pattern[start..start + count].contains(&b'-')
            };
            if dashed && let Some(i) = args.iter().position(|x| x == b"--") {
                args.remove(i);
            }
            start += count;
            self.take_action(a, args)?;
        }
        self.positionals.drain(..counts.len());
        Ok(start)
    }

    fn run(&mut self) -> Result<Vec<Bytes>, Fail> {
        let max = self.indices.keys().next_back().copied();
        let mut start = 0;
        while max.is_some_and(|m| start <= m) {
            let next = *self
                .indices
                .range(start..)
                .next()
                .expect("an option at or after start")
                .0;
            if start != next {
                let end = self.consume_positionals(start)?;
                if end > start {
                    start = end;
                    continue;
                }
                start = end;
            }
            if !self.indices.contains_key(&start) {
                self.extras.extend(self.args[start..next].iter().cloned());
                start = next;
            }
            start = self.consume_optional(start)?;
        }
        let stop = self.consume_positionals(start)?;
        self.extras.extend(self.args[stop..].iter().cloned());

        let mut required = Vec::new();
        for (i, action) in self.p.spec.actions.iter().enumerate() {
            if self.seen.contains(&i) {
                continue;
            }
            if action.required() {
                required.push(action_name(action).unwrap_or_else(|| "None".into()));
            } else if let (Some(dest), Value::Str(d)) = (action.dest(), action.default())
                && !action.default_suppressed()
                && self.ns.is_default(dest)
            {
                let v = self.p.get_value(i, d.as_bytes())?;
                self.ns.set(dest, v, false);
            }
        }
        if !required.is_empty() {
            return Err(Fail::Arg(format!(
                "the following arguments are required: {}",
                required.join(", ")
            )));
        }
        for g in &self.p.spec.groups {
            if g.required && !g.members.iter().any(|m| self.seen_non_default.contains(m)) {
                let names: Vec<String> = g
                    .members
                    .iter()
                    .map(|&m| self.p.action(m))
                    .filter(|a| !a.suppressed())
                    .map(|a| action_name(a).unwrap_or_else(|| "None".into()))
                    .collect();
                return Err(Fail::Arg(format!(
                    "one of the arguments {} is required",
                    names.join(" ")
                )));
            }
        }
        Ok(std::mem::take(&mut self.extras))
    }
}

// ---- HelpFormatter -------------------------------------------------------------------------

/// `HelpFormatter.format_help`'s last step: runs of three or more newlines become two, then
/// `strip("\n") + "\n"`.
fn finish(help: String) -> String {
    if help.is_empty() {
        return help;
    }
    let mut out = String::with_capacity(help.len());
    let mut run = 0;
    for c in help.chars() {
        if c == '\n' {
            run += 1;
            continue;
        }
        if run > 0 {
            out.push_str(if run >= 3 { "\n\n" } else { &"\n\n"[..run] });
            run = 0;
        }
        out.push(c);
    }
    if run > 0 {
        out.push_str(if run >= 3 { "\n\n" } else { &"\n\n"[..run] });
    }
    let mut out = out.trim_matches('\n').to_string();
    out.push('\n');
    out
}

/// `'%*s' % (n, '')`: `n` spaces (a negative `n` left-justifies, the same here).
fn pad(n: i64) -> String {
    " ".repeat(n.unsigned_abs() as usize)
}

fn clen(s: &str) -> i64 {
    char_len(s) as i64
}

/// `_whitespace_matcher.sub(' ', text).strip()`: ASCII whitespace runs become one space.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_ws = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    strip_ws(&out).to_string()
}

/// `TextWrapper.wordsep_re` (break_on_hyphens), its VERBOSE spaces removed.
fn wordsep() -> &'static re::Regex {
    static RE: OnceLock<re::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let ws = r"[\t\n\x0b\x0c\r ]";
        let nws = r"[^\t\n\x0b\x0c\r ]";
        let wp = r#"[\w!"'&.,?]"#;
        let lt = r"[^\d\W]";
        let pattern = format!(
            r"({ws}+|(?<={wp})-{{2,}}(?=\w)|{nws}+?(?:-(?:(?<={lt}{lt}-)|(?<={lt}-{lt}-))(?={lt}-?{lt})|(?={ws}|\Z)|(?<={wp})(?=-{{2,}}\w)))"
        );
        re::compile(&pattern, false).expect("textwrap's wordsep_re translates")
    })
}

/// `TextWrapper._split(text)`: `wordsep_re.split(text)` without the empty strings.
fn split_chunks(text: &str) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in wordsep().find_iter(text) {
        let Ok((s, e)) = m else {
            break;
        };
        if s > last {
            out.push(text[last..s].chars().collect());
        }
        if e > s {
            out.push(text[s..e].chars().collect());
        }
        last = e;
    }
    if last < text.len() {
        out.push(text[last..].chars().collect());
    }
    out
}

fn blank(chunk: &[char]) -> bool {
    chunk.iter().all(|&c| is_space(c))
}

/// `textwrap.wrap(text, width, initial_indent=.., subsequent_indent=..)` with argparse's
/// settings (the text has no tabs and no ASCII whitespace but spaces).
fn wrap(text: &str, width: i64, initial: &str, subsequent: &str) -> Vec<String> {
    let mut chunks = split_chunks(text);
    chunks.reverse();
    let mut lines: Vec<String> = Vec::new();
    while !chunks.is_empty() {
        let mut cur: Vec<Vec<char>> = Vec::new();
        let mut cur_len: i64 = 0;
        let indent = if lines.is_empty() {
            initial
        } else {
            subsequent
        };
        let w = width - clen(indent);
        if !lines.is_empty() && chunks.last().is_some_and(|c| blank(c)) {
            chunks.pop();
        }
        while let Some(last) = chunks.last() {
            let l = last.len() as i64;
            if cur_len + l <= w {
                cur.push(chunks.pop().expect("a chunk"));
                cur_len += l;
            } else {
                break;
            }
        }
        if chunks.last().is_some_and(|c| c.len() as i64 > w) {
            handle_long_word(&mut chunks, &mut cur, cur_len, w);
        }
        if cur.last().is_some_and(|c| blank(c)) {
            cur.pop();
        }
        if !cur.is_empty() {
            let mut line = indent.to_string();
            for c in &cur {
                line.extend(c.iter());
            }
            lines.push(line);
        }
    }
    lines
}

/// `TextWrapper._handle_long_word` (break_long_words, break_on_hyphens).
fn handle_long_word(chunks: &mut [Vec<char>], cur: &mut Vec<Vec<char>>, cur_len: i64, w: i64) {
    let space_left = if w < 1 { 1 } else { w - cur_len }.max(0) as usize;
    let chunk = chunks.last_mut().expect("a chunk");
    let mut end = space_left.min(chunk.len());
    if chunk.len() > space_left
        && let Some(h) = chunk[..space_left].iter().rposition(|&c| c == '-')
        && h > 0
        && chunk[..h].iter().any(|&c| c != '-')
    {
        end = h + 1;
    }
    cur.push(chunk[..end].to_vec());
    chunk.drain(..end);
}

/// Python `%` formatting of `fmt` with `%(name)s`/`%(name)r` keys: `lookup(name, repr)`;
/// `%%` is `%`; anything else is left as written.
fn percent(fmt: &str, lookup: &dyn Fn(&str, bool) -> Option<String>) -> String {
    let mut out = String::with_capacity(fmt.len());
    let mut rest = fmt;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(after) = tail.strip_prefix("%%") {
            out.push('%');
            rest = after;
            continue;
        }
        if let Some(inner) = tail.strip_prefix("%(")
            && let Some(close) = inner.find(')')
        {
            let name = &inner[..close];
            let conv = inner[close + 1..].chars().next();
            if let Some(c @ ('s' | 'r')) = conv
                && let Some(v) = lookup(name, c == 'r')
            {
                out.push_str(&v);
                rest = &inner[close + 2..];
                continue;
            }
        }
        out.push('%');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// `str(x)` / `repr(x)` of a str.
fn text_value(s: &str, r: bool) -> String {
    if r { repr(s) } else { s.to_string() }
}

struct Fmt<'a> {
    spec: &'a Spec,
    prog: &'a str,
    width: i64,
    max_help_position: i64,
    action_max_length: i64,
}

impl<'a> Fmt<'a> {
    fn new(spec: &'a Spec, prog: &'a str, width: i64) -> Fmt<'a> {
        Fmt {
            spec,
            prog,
            width,
            max_help_position: 24.min((width - 20).max(4)),
            action_max_length: 0,
        }
    }

    fn metavar_get(&self, a: &Action, default: &str, n: usize) -> Vec<String> {
        match a.metavar() {
            Some(Metavar::Tuple(t)) => t,
            Some(Metavar::One(m)) => vec![m; n],
            None => match a.choices() {
                Some(c) => vec![format!("{{{}}}", c.join(",")); n],
                None => vec![default.to_string(); n],
            },
        }
    }

    /// `_format_args(action, default_metavar)`.
    fn format_args(&self, a: &Action, default: &str) -> String {
        match a.n() {
            N::One => self.metavar_get(a, default, 1)[0].clone(),
            N::Opt => format!("[{}]", self.metavar_get(a, default, 1)[0]),
            N::Star => {
                let m = self.metavar_get(a, default, 1);
                if m.len() == 2 {
                    format!("[{} [{} ...]]", m[0], m[1])
                } else {
                    format!("[{} ...]", m[0])
                }
            }
            N::Plus => {
                let m = self.metavar_get(a, default, 2);
                format!("{} [{} ...]", m[0], m[1])
            }
            N::Parser => format!("{} ...", self.metavar_get(a, default, 1)[0]),
            N::Num(n) => self.metavar_get(a, default, n).join(" "),
        }
    }

    fn default_metavar(&self, a: &Action) -> String {
        let dest = a.dest().unwrap_or("==SUPPRESS==");
        if a.is_optional() {
            dest.to_ascii_uppercase()
        } else {
            dest.to_string()
        }
    }

    /// `_format_action_invocation(action)`.
    fn invocation(&self, a: &Action) -> String {
        let default = self.default_metavar(a);
        if !a.is_optional() {
            return self.metavar_get(a, &default, 1).join(" ");
        }
        if a.n() == N::Num(0) {
            return a.option_strings().join(", ");
        }
        let args = self.format_args(a, &default);
        let parts: Vec<String> = a
            .option_strings()
            .iter()
            .map(|o| format!("{o} {args}"))
            .collect();
        parts.join(", ")
    }

    /// `_expand_help(action)`.
    fn expand_help(&self, a: &Action, help: &str) -> String {
        let lookup = |name: &str, r: bool| -> Option<String> {
            Some(match name {
                "prog" => text_value(self.prog, r),
                "dest" => text_value(a.dest()?, r),
                "default" if !a.default_suppressed() => {
                    let d = a.default();
                    if r { d.py_repr() } else { d.py_str() }
                }
                "const" => {
                    let k = a.konst();
                    if r { k.py_repr() } else { k.py_str() }
                }
                "required" => if a.required() { "True" } else { "False" }.to_string(),
                "help" => text_value(help, r),
                "metavar" => match a.metavar() {
                    None => "None".to_string(),
                    Some(Metavar::One(m)) => text_value(&m, r),
                    Some(Metavar::Tuple(t)) => {
                        let items: Vec<String> = t.iter().map(|m| repr(m)).collect();
                        if items.len() == 1 {
                            format!("({},)", items[0])
                        } else {
                            format!("({})", items.join(", "))
                        }
                    }
                },
                "choices" => match a.choices() {
                    None => "None".to_string(),
                    Some(c) => text_value(&c.join(", "), r),
                },
                "nargs" => match a.n() {
                    N::One => "None".to_string(),
                    N::Opt => text_value("?", r),
                    N::Star => text_value("*", r),
                    N::Plus => text_value("+", r),
                    N::Parser => text_value("A...", r),
                    N::Num(n) => n.to_string(),
                },
                "type" => match a.ty() {
                    Type::Str => "None".to_string(),
                    t => text_value(t.name(), r),
                },
                "option_strings" => {
                    let items: Vec<String> = a.option_strings().iter().map(|o| repr(o)).collect();
                    format!("[{}]", items.join(", "))
                }
                _ => return None,
            })
        };
        percent(help, &lookup)
    }

    /// `_format_action(action)` at `indent`.
    fn format_action(&self, a: &Action, indent: i64) -> String {
        let help_position = (self.action_max_length + 2).min(self.max_help_position);
        let help_width = (self.width - help_position).max(11);
        let action_width = help_position - indent - 2;
        let invocation = self.invocation(a);
        let help = a.help();
        let help_text = help.text().filter(|h| !h.is_empty());
        let mut indent_first = 0;
        let header = if help_text.is_none() {
            format!("{}{invocation}\n", pad(indent))
        } else if clen(&invocation) <= action_width {
            let fill = (action_width - clen(&invocation)).max(0);
            format!("{}{invocation}{}  ", pad(indent), pad(fill))
        } else {
            indent_first = help_position;
            format!("{}{invocation}\n", pad(indent))
        };
        let mut parts = vec![header.clone()];
        match help_text {
            Some(h) if !strip_ws(h).is_empty() => {
                let expanded = self.expand_help(a, h);
                if !expanded.is_empty() {
                    let lines = wrap(&collapse(&expanded), help_width, "", "");
                    if let Some((first, rest)) = lines.split_first() {
                        parts.push(format!("{}{first}\n", pad(indent_first)));
                        for line in rest {
                            parts.push(format!("{}{line}\n", pad(help_position)));
                        }
                    }
                }
            }
            _ => {
                if !header.ends_with('\n') {
                    parts.push("\n".to_string());
                }
            }
        }
        for sub in a.subactions() {
            parts.push(self.format_action(&sub, indent + 2));
        }
        parts.concat()
    }

    /// `_format_text(text)` at indent 0.
    fn format_text(&self, text: &str) -> String {
        let text = if text.contains("%(prog)") {
            percent(text, &|name, r| {
                (name == "prog").then(|| text_value(self.prog, r))
            })
        } else {
            text.to_string()
        };
        let width = self.width.max(11);
        let mut out = wrap(&collapse(&text), width, "", "").join("\n");
        out.push_str("\n\n");
        out
    }

    /// `_format_actions_usage(actions, groups)`.
    fn format_actions_usage(&self, actions: &[usize]) -> String {
        let spec_actions = &self.spec.actions;
        let mut group_actions: HashSet<usize> = HashSet::new();
        let mut inserts: BTreeMap<usize, String> = BTreeMap::new();
        for g in &self.spec.groups {
            assert!(!g.members.is_empty(), "argparse: empty group");
            let Some(start) = actions.iter().position(|&x| x == g.members[0]) else {
                continue;
            };
            let end = start + g.members.len();
            if actions.get(start..end) != Some(&g.members[..]) {
                continue;
            }
            let mut suppressed = 0;
            for &m in &g.members {
                group_actions.insert(m);
                if spec_actions[m].suppressed() {
                    suppressed += 1;
                }
            }
            let exposed = g.members.len() - suppressed;
            if exposed == 0 {
                continue;
            }
            let (open, close) = if !g.required {
                ("[", "]")
            } else if exposed > 1 {
                ("(", ")")
            } else {
                ("", "")
            };
            if !open.is_empty() {
                match inserts.get_mut(&start) {
                    Some(s) => {
                        s.push(' ');
                        s.push_str(open);
                    }
                    None => {
                        inserts.insert(start, open.to_string());
                    }
                }
                match inserts.get_mut(&end) {
                    Some(s) => s.push_str(close),
                    None => {
                        inserts.insert(end, close.to_string());
                    }
                }
            }
            for i in start + 1..end {
                inserts.insert(i, "|".to_string());
            }
        }

        let mut parts: Vec<Option<String>> = Vec::new();
        for (i, &ai) in actions.iter().enumerate() {
            let a = &spec_actions[ai];
            if a.suppressed() {
                parts.push(None);
                if inserts.get(&i).map(String::as_str) == Some("|") {
                    inserts.remove(&i);
                } else if inserts.get(&(i + 1)).map(String::as_str) == Some("|") {
                    inserts.remove(&(i + 1));
                }
            } else if !a.is_optional() {
                let mut part = self.format_args(a, &self.default_metavar(a));
                if group_actions.contains(&ai)
                    && part.starts_with('[')
                    && part.ends_with(']')
                    && part.len() >= 2
                {
                    part = part[1..part.len() - 1].to_string();
                }
                parts.push(Some(part));
            } else {
                let option_string = &a.option_strings()[0];
                let mut part = if a.n() == N::Num(0) {
                    option_string.clone()
                } else {
                    format!(
                        "{option_string} {}",
                        self.format_args(a, &self.default_metavar(a))
                    )
                };
                if !a.required() && !group_actions.contains(&ai) {
                    part = format!("[{part}]");
                }
                parts.push(Some(part));
            }
        }
        for (&i, s) in inserts.iter().rev() {
            let i = i.min(parts.len());
            parts.insert(i, Some(s.clone()));
        }
        let text: Vec<String> = parts.into_iter().flatten().collect();
        let text = text.join(" ");
        let text = drop_space_after_open(&text);
        let text = drop_space_before_close(&text);
        let text = drop_empty_pairs(&text);
        strip_ws(&text).to_string()
    }

    /// `_format_usage(None, actions, groups, prefix)`.
    fn format_usage(&self, actions: &[usize], prefix: &str) -> String {
        let usage = if actions.is_empty() {
            self.prog.to_string()
        } else {
            let (optionals, positionals): (Vec<usize>, Vec<usize>) = actions
                .iter()
                .partition(|&&i| self.spec.actions[i].is_optional());
            let all: Vec<usize> = optionals.iter().chain(&positionals).copied().collect();
            let action_usage = self.format_actions_usage(&all);
            let mut usage = self.prog.to_string();
            if !action_usage.is_empty() {
                if !usage.is_empty() {
                    usage.push(' ');
                }
                usage.push_str(&action_usage);
            }
            let text_width = self.width;
            if clen(prefix) + clen(&usage) > text_width {
                let opt_parts = usage_parts(&self.format_actions_usage(&optionals));
                let pos_parts = usage_parts(&self.format_actions_usage(&positionals));
                let prog = self.prog.to_string();
                let lines = if (clen(prefix) + clen(&prog)) as f64 <= 0.75 * text_width as f64 {
                    let indent = " ".repeat((clen(prefix) + clen(&prog) + 1) as usize);
                    if !opt_parts.is_empty() {
                        let mut first = vec![prog.clone()];
                        first.extend(opt_parts);
                        let mut lines = get_lines(&first, &indent, Some(prefix), text_width);
                        lines.extend(get_lines(&pos_parts, &indent, None, text_width));
                        lines
                    } else if !pos_parts.is_empty() {
                        let mut first = vec![prog.clone()];
                        first.extend(pos_parts);
                        get_lines(&first, &indent, Some(prefix), text_width)
                    } else {
                        vec![prog.clone()]
                    }
                } else {
                    let indent = " ".repeat(clen(prefix) as usize);
                    let parts: Vec<String> = opt_parts.iter().chain(&pos_parts).cloned().collect();
                    let mut lines = get_lines(&parts, &indent, None, text_width);
                    if lines.len() > 1 {
                        lines = get_lines(&opt_parts, &indent, None, text_width);
                        lines.extend(get_lines(&pos_parts, &indent, None, text_width));
                    }
                    let mut all = vec![prog.clone()];
                    all.extend(lines);
                    all
                };
                lines.join("\n")
            } else {
                usage
            }
        };
        format!("{prefix}{usage}\n\n")
    }

    /// `format_help()`.
    fn format_help(mut self) -> String {
        let actions = &self.spec.actions;
        let positionals: Vec<usize> = (0..actions.len())
            .filter(|&i| !actions[i].is_optional())
            .collect();
        let optionals: Vec<usize> = (0..actions.len())
            .filter(|&i| actions[i].is_optional())
            .collect();
        let mut max = 0;
        for &i in positionals.iter().chain(&optionals) {
            let a = &actions[i];
            if a.suppressed() {
                continue;
            }
            max = max.max(clen(&self.invocation(a)) + 2);
            for sub in a.subactions() {
                max = max.max(clen(&self.invocation(&sub)) + 4);
            }
        }
        self.action_max_length = max;

        let all: Vec<usize> = (0..actions.len()).collect();
        let mut out = self.format_usage(&all, "usage: ");
        if let Some(d) = &self.spec.description {
            out.push_str(&self.format_text(d));
        }
        for (title, list) in [
            ("positional arguments", &positionals),
            ("options", &optionals),
        ] {
            let items: String = list
                .iter()
                .filter(|&&i| !actions[i].suppressed())
                .map(|&i| self.format_action(&actions[i], 2))
                .collect();
            if !items.is_empty() {
                out.push_str(&format!("\n{title}:\n{items}\n"));
            }
        }
        if let Some(e) = &self.spec.epilog {
            out.push_str(&self.format_text(e));
        }
        finish(out)
    }
}

/// `re.findall(part_regexp, text)` of `_format_usage`.
fn usage_parts(text: &str) -> Vec<String> {
    static RE: OnceLock<re::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        re::compile(r"\(.*?\)+(?=\s|$)|\[.*?\]+(?=\s|$)|\S+", false)
            .expect("argparse's part_regexp translates")
    });
    re.find_iter(text)
        .map_while(Result::ok)
        .map(|(s, e)| text[s..e].to_string())
        .collect()
}

/// `get_lines(parts, indent, prefix)` of `_format_usage`.
fn get_lines(parts: &[String], indent: &str, prefix: Option<&str>, text_width: i64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line: Vec<&str> = Vec::new();
    let indent_length = clen(indent);
    let mut line_len = match prefix {
        Some(p) => clen(p) - 1,
        None => indent_length - 1,
    };
    for part in parts {
        if line_len + 1 + clen(part) > text_width && !line.is_empty() {
            lines.push(format!("{indent}{}", line.join(" ")));
            line.clear();
            line_len = indent_length - 1;
        }
        line.push(part);
        line_len += clen(part) + 1;
    }
    if !line.is_empty() {
        lines.push(format!("{indent}{}", line.join(" ")));
    }
    if prefix.is_some()
        && let Some(first) = lines.first_mut()
    {
        *first = first.chars().skip(indent_length as usize).collect();
    }
    lines
}

fn is_open(c: char) -> bool {
    c == '[' || c == '('
}

fn is_close(c: char) -> bool {
    c == ']' || c == ')'
}

/// `re.sub(r'([\[(]) ', r'\1', text)`.
fn drop_space_after_open(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        out.push(chars[i]);
        if is_open(chars[i]) && chars.get(i + 1) == Some(&' ') {
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// `re.sub(r' ([\])])', r'\1', text)`.
fn drop_space_before_close(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' ' && chars.get(i + 1).is_some_and(|&c| is_close(c)) {
            out.push(chars[i + 1]);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// `re.sub(r'[\[(] *[\])]', r'', text)`.
fn drop_empty_pairs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if is_open(chars[i]) {
            let mut j = i + 1;
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            if chars.get(j).is_some_and(|&c| is_close(c)) {
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(s: &str) -> Vec<String> {
        split_chunks(s)
            .into_iter()
            .map(|c| c.into_iter().collect())
            .collect()
    }

    #[test]
    fn wordsep_splits_as_textwrap() {
        assert_eq!(
            chunks("Hello there -- you goof-ball, use the -b option!"),
            [
                "Hello", " ", "there", " ", "--", " ", "you", " ", "goof-", "ball,", " ", "use",
                " ", "the", " ", "-b", " ", "option!"
            ]
        );
    }

    #[test]
    fn pathlib() {
        for (a, b) in [
            ("", "."),
            ("a//b/", "a/b"),
            ("//x", "//x"),
            ("///x/./y", "/x/y"),
            ("./", "."),
        ] {
            assert_eq!(pathlib_str(a.as_bytes()), b.as_bytes(), "{a:?}");
        }
    }

    #[test]
    fn finish_collapses_newlines() {
        assert_eq!(finish("\n\na\n\n\n\nb\n\n".into()), "a\n\nb\n");
    }

    #[test]
    fn percent_keys() {
        let f = |n: &str, r: bool| (n == "default").then(|| if r { "'x'" } else { "x" }.into());
        assert_eq!(
            percent("a %(default)s %(default)r %% %(other)s %", &f),
            "a x 'x' % %(other)s %"
        );
    }
}
