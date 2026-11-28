//! The condition evaluator and the per-file run (Task 5).
//!
//! A run owns the per-file state: which atoms the prefilter saw, and a memo of every string and
//! rule it has looked at. A string runs its regex only when a condition asks for it and its atoms
//! were seen, so a file that holds none of a rule's words costs one automaton pass.

use std::cell::RefCell;

use aho_corasick::AhoCorasick;

use super::parse::{Expr, Op, Quant};
use super::{RuleSet, YaraHit};
use crate::FileCtx;

/// The longest `seen` excerpt, in bytes.
const SEEN_MAX: usize = 200;
/// `#id` stops counting here.
const COUNT_CAP: usize = 100_000;

/// One entry of a rule's `path_include` or `path_exclude`.
#[derive(Debug)]
pub(super) enum PathPat {
    /// `*.js`: the file's extension.
    Ext(String),
    /// `*/setup.py`: the file's name, in any folder.
    Name(String),
    /// `setup.py`: the whole path.
    Exact(String),
    /// `dist/*`: below a folder of that name, at any depth.
    Dir(String),
}

impl PathPat {
    /// The patterns of a comma-separated meta value.
    pub(super) fn parse_list(list: &str) -> Result<Vec<PathPat>, String> {
        let mut out = Vec::new();
        for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let plain = |s: &str| !s.contains(['*', '?', '[']);
            let pat = if let Some(ext) = item
                .strip_prefix("*.")
                .filter(|e| plain(e) && !e.contains('/'))
            {
                PathPat::Ext(ext.to_ascii_lowercase())
            } else if let Some(name) = item
                .strip_prefix("*/")
                .filter(|n| plain(n) && !n.contains('/'))
            {
                PathPat::Name(name.to_string())
            } else if let Some(dir) = item
                .strip_suffix("/*")
                .filter(|d| plain(d) && !d.contains('/'))
            {
                PathPat::Dir(dir.to_string())
            } else if plain(item) {
                PathPat::Exact(item.to_string())
            } else {
                return Err(format!("unsupported path pattern `{item}`"));
            };
            out.push(pat);
        }
        Ok(out)
    }

    pub(super) fn matches(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        match self {
            PathPat::Ext(e) => name
                .rsplit_once('.')
                .is_some_and(|(_, x)| x.eq_ignore_ascii_case(e)),
            PathPat::Name(n) => name == n,
            PathPat::Exact(p) => path == p,
            PathPat::Dir(d) => path.split('/').rev().skip(1).any(|part| part == d),
        }
    }
}

/// The package ecosystem a file belongs to, from its extension; empty when it names none.
fn ecosystem(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase());
    match ext.as_deref() {
        Some("py" | "pyw" | "pyi" | "pyx" | "pth") => "pypi",
        Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx") => "npm",
        Some("go") => "go",
        Some("rb" | "gemspec") => "rubygems",
        Some("rs") => "crates",
        _ => "",
    }
}

/// What a string did on this file.
#[derive(Clone, Copy, Default)]
struct Memo {
    /// `Some(None)`: looked at, no match.
    first: Option<Option<(usize, usize)>>,
    count: Option<usize>,
}

struct Run<'a> {
    set: &'a RuleSet,
    path: &'a str,
    bytes: &'a [u8],
    eco: &'static str,
    /// Which atoms the automaton saw; `None` runs every string.
    seen: Option<Vec<bool>>,
    strings: RefCell<Vec<Memo>>,
    rules: RefCell<Vec<Option<bool>>>,
}

impl RuleSet {
    /// The hits of every rule on `f`.
    pub(super) fn eval_with(&self, f: &FileCtx, prefilter: bool) -> Vec<YaraHit> {
        let reported: Vec<usize> = self
            .reported
            .iter()
            .copied()
            .filter(|&r| self.rules[r].applies(f.path))
            .collect();
        if reported.is_empty() {
            return Vec::new();
        }
        let seen = if prefilter {
            self.ac
                .as_ref()
                .map(|ac| scan_atoms(ac, self.atom_count, f.bytes))
        } else {
            None
        };
        let run = Run {
            set: self,
            path: f.path,
            bytes: f.bytes,
            eco: ecosystem(f.path),
            seen,
            strings: RefCell::new(vec![Memo::default(); self.strings.len()]),
            rules: RefCell::new(vec![None; self.rules.len()]),
        };
        reported
            .into_iter()
            .filter(|&r| run.rule(r))
            .map(|r| run.hit(r))
            .collect()
    }
}

/// Which atoms occur in `bytes`, in one pass.
fn scan_atoms(ac: &AhoCorasick, atoms: usize, bytes: &[u8]) -> Vec<bool> {
    let mut seen = vec![false; atoms];
    let mut left = atoms;
    for m in ac.find_overlapping_iter(bytes) {
        let id = m.pattern().as_usize();
        if !seen[id] {
            seen[id] = true;
            left -= 1;
            if left == 0 {
                break;
            }
        }
    }
    seen
}

/// No ASCII letter, digit or `_` right before `start` or right after `end`.
fn is_fullword(bytes: &[u8], start: usize, end: usize) -> bool {
    let word = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_';
    !(start > 0 && bytes.get(start - 1).is_some_and(word)) && !bytes.get(end).is_some_and(word)
}

impl Run<'_> {
    /// Could string `g` match at all, going by the atoms seen?
    fn candidate(&self, g: usize) -> bool {
        match &self.seen {
            None => true,
            Some(seen) => {
                let atoms = &self.set.strings[g].atoms;
                atoms.is_empty() || atoms.iter().any(|&a| seen[a])
            }
        }
    }

    fn matches(&self, g: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let s = &self.set.strings[g];
        s.re.find_iter(self.bytes)
            .map(|m| (m.start(), m.end()))
            .filter(move |&(a, b)| !s.fullword || is_fullword(self.bytes, a, b))
    }

    /// Does string `g` match?
    fn found(&self, g: usize) -> bool {
        if let Some(first) = self.strings.borrow()[g].first {
            return first.is_some();
        }
        let first = if self.candidate(g) {
            self.matches(g).next()
        } else {
            None
        };
        self.strings.borrow_mut()[g].first = Some(first);
        first.is_some()
    }

    /// How many times string `g` matches, without overlap.
    fn count(&self, g: usize) -> usize {
        if let Some(n) = self.strings.borrow()[g].count {
            return n;
        }
        let (first, n) = if self.candidate(g) {
            let mut it = self.matches(g);
            let first = it.next();
            (
                first,
                usize::from(first.is_some()) + it.take(COUNT_CAP).count(),
            )
        } else {
            (None, 0)
        };
        let mut memo = self.strings.borrow_mut();
        memo[g].first = Some(first);
        memo[g].count = Some(n);
        n
    }

    fn rule(&self, r: usize) -> bool {
        if let Some(v) = self.rules.borrow()[r] {
            return v;
        }
        let rule = &self.set.rules[r];
        let v = rule.applies(self.path) && self.val(rule.base, &rule.cond) != 0;
        self.rules.borrow_mut()[r] = Some(v);
        v
    }

    /// The value of `e` in a rule whose strings start at `base`. Booleans are 0 and 1.
    fn val(&self, base: usize, e: &Expr) -> i64 {
        match e {
            Expr::Bool(b) => i64::from(*b),
            Expr::Int(n) => *n,
            Expr::FileSize => i64::try_from(self.bytes.len()).unwrap_or(i64::MAX),
            Expr::Str(i) => i64::from(self.found(base + i)),
            Expr::Count(i) => i64::try_from(self.count(base + i)).unwrap_or(i64::MAX),
            // A name the compiler left unresolved never reaches a finished set.
            Expr::Rule(_) => 0,
            Expr::RuleRef(r) => i64::from(self.rule(*r)),
            Expr::Eco { eq, name } => i64::from((self.eco == name) == *eq),
            Expr::Not(x) => i64::from(self.val(base, x) == 0),
            Expr::And(v) => i64::from(v.iter().all(|x| self.val(base, x) != 0)),
            Expr::Or(v) => i64::from(v.iter().any(|x| self.val(base, x) != 0)),
            Expr::Cmp(op, a, b) => {
                let (a, b) = (self.val(base, a), self.val(base, b));
                i64::from(match op {
                    Op::Eq => a == b,
                    Op::Ne => a != b,
                    Op::Lt => a < b,
                    Op::Le => a <= b,
                    Op::Gt => a > b,
                    Op::Ge => a >= b,
                })
            }
            Expr::Sum(terms) => terms.iter().fold(0i64, |acc, (neg, x)| {
                let v = self.val(base, x);
                if *neg {
                    acc.saturating_sub(v)
                } else {
                    acc.saturating_add(v)
                }
            }),
            Expr::Of(q, set) => {
                let hit = |i: &usize| self.found(base + i);
                i64::from(match q {
                    Quant::Any => set.iter().any(hit),
                    Quant::All => set.iter().all(hit),
                    Quant::None => !set.iter().any(hit),
                    Quant::N(n) => set.iter().filter(|i| hit(i)).take(*n).count() >= *n,
                })
            }
        }
    }

    /// The hit of rule `r`, which holds: its earliest evaluated match, in it or in a rule it uses.
    fn hit(&self, r: usize) -> YaraHit {
        let mut todo = vec![r];
        let mut done = vec![false; self.set.rules.len()];
        let mut best: Option<(usize, usize)> = None;
        while let Some(n) = todo.pop() {
            if std::mem::replace(&mut done[n], true) {
                continue;
            }
            let rule = &self.set.rules[n];
            todo.extend(rule.refs.iter().copied());
            let memo = self.strings.borrow();
            for m in &memo[rule.base..rule.base + rule.len] {
                if let Some(Some(m)) = m.first
                    && best.is_none_or(|b| m.0 < b.0)
                {
                    best = Some(m);
                }
            }
        }
        let rule = &self.set.rules[r];
        match best {
            Some((start, end)) => YaraHit {
                rule: rule.name.clone(),
                severity: rule.severity,
                line: 1 + self.bytes[..start].iter().filter(|&&b| b == b'\n').count(),
                seen: String::from_utf8_lossy(&self.bytes[start..end.min(start + SEEN_MAX)])
                    .into_owned(),
            },
            None => YaraHit {
                rule: rule.name.clone(),
                severity: rule.severity,
                line: 0,
                seen: String::new(),
            },
        }
    }
}
