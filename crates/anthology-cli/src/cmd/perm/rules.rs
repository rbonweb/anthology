//! The user's `Read()` deny and ask rules, as a matcher for absolute paths.
//!
//! The anchors follow Claude Code's, read wide: wherever an anchor could mean more than one
//! directory, the rule is tried at every one of them, so the matcher can only match more than Claude
//! Code would, never less. `//p` is the absolute `/p`; `~/p` is under the home; `/p` is tried under
//! every root, the working directory, `~/.claude` and `/`; `./p` and a `p` with a slash are tried
//! under the working directory and every root; a `p` with no slash is a name at any depth. Each base
//! is tried as given and with its symlinks resolved. Every match ignores ASCII and Unicode case (the
//! default macOS volume does), and a rule matches a path when it matches the path or one of its
//! ancestors, so `Read(./secrets)` holds `./secrets/a` too.

use std::path::{Component, Path, PathBuf};

/// One component of a rule's path pattern.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Comp {
    Literal(String),
    Glob(Vec<GlobItem>),
    /// `**`: any number of components, none included.
    Any,
}

impl Comp {
    fn parse(text: &str) -> Comp {
        if text == "**" {
            Comp::Any
        } else if text.contains(['*', '?', '[', '\\']) {
            Comp::Glob(compile_glob(text))
        } else {
            Comp::Literal(text.to_string())
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Comp::Literal(literal) => literal == name,
            Comp::Glob(glob) => {
                let name: Vec<char> = name.chars().collect();
                glob_match(glob, &name)
            }
            Comp::Any => true,
        }
    }
}

#[derive(Debug)]
enum Rule {
    /// A pattern with no `/` (bar a trailing one): a name at any depth.
    AnyDepth(Comp),
    /// One pattern per directory the anchor may mean.
    Anchored(Vec<Vec<Comp>>),
}

/// The directories the anchors stand for. Each list holds the paths as given; the matcher adds the
/// same paths with symlinks resolved.
pub struct Bases<'a> {
    pub cwd: &'a Path,
    pub roots: &'a [PathBuf],
    pub home: Option<&'a Path>,
}

/// The rules, folded to lower case.
#[derive(Debug, Default)]
pub struct Matcher {
    rules: Vec<Rule>,
}

/// Why a rule list cannot be read: the caller gives no decision.
pub fn every_read() -> String {
    "a Read rule denies every read".to_string()
}

impl Matcher {
    /// `texts` are whole rule texts (`Read(./secret.txt)`). A rule this check cannot read, or one
    /// that denies every read, is an error: the caller then gives no decision.
    pub fn new(texts: &[String], bases: &Bases) -> Result<Matcher, String> {
        let mut rules = Vec::new();
        for text in texts {
            // Braces and `!` have a meaning in Claude Code's matcher this one does not model.
            let Some(pattern) = rule_pattern(text).filter(|p| !p.contains(['{', '}', '!'])) else {
                return Err(format!(
                    "a permission rule this check does not read: {}",
                    super::shown(text)
                ));
            };
            let pattern = pattern.trim();
            if matches!(pattern, "" | "*" | "**" | "**/*" | "/**" | "//**") {
                return Err(every_read());
            }
            rules.push(rule(&fold(pattern), bases));
        }
        Ok(Matcher { rules })
    }

    /// Whether a rule matches `path` or one of its ancestors.
    pub fn denied(&self, path: &Path) -> bool {
        let comps = path_comps(path);
        self.rules.iter().any(|rule| match rule {
            Rule::AnyDepth(comp) => comps.iter().any(|name| comp.matches(name)),
            Rule::Anchored(patterns) => patterns.iter().any(|p| match_prefix(p, &comps, false)),
        })
    }

    /// Whether an anchored rule matches `dir`, an ancestor of it, or could match something below it.
    /// A name-at-any-depth rule is not asked: it could match below any directory.
    pub fn anchored_reaches(&self, dir: &Path) -> bool {
        let comps = path_comps(dir);
        self.rules.iter().any(|rule| match rule {
            Rule::AnyDepth(_) => false,
            Rule::Anchored(patterns) => patterns.iter().any(|p| match_prefix(p, &comps, true)),
        })
    }
}

/// The pattern of a `Read(...)` rule, `""` for a bare `Read`, or `None` for any other text.
fn rule_pattern(text: &str) -> Option<&str> {
    if text == "Read" {
        return Some("");
    }
    text.strip_prefix("Read(")?.strip_suffix(')')
}

fn fold(text: &str) -> String {
    text.to_lowercase()
}

fn rule(pattern: &str, bases: &Bases) -> Rule {
    let resolved = |dirs: &[&Path]| -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            for dir in [Some(dir.to_path_buf()), std::fs::canonicalize(dir).ok()]
                .into_iter()
                .flatten()
            {
                if !out.contains(&dir) {
                    out.push(dir);
                }
            }
        }
        out
    };
    let mut cwd_and_roots: Vec<&Path> = vec![bases.cwd];
    cwd_and_roots.extend(bases.roots.iter().map(PathBuf::as_path));
    let home_claude = bases.home.map(|h| h.join(".claude"));
    let anchored = |dirs: Vec<PathBuf>, rest: &str| -> Rule {
        let mut patterns: Vec<Vec<Comp>> = Vec::new();
        // An unknown base becomes a leading `**`, which can only match more.
        let dirs: Vec<Option<PathBuf>> = if dirs.is_empty() {
            vec![None]
        } else {
            dirs.into_iter().map(Some).collect()
        };
        for dir in dirs {
            let mut comps: Vec<Comp> = match &dir {
                Some(dir) => path_comps(dir).into_iter().map(Comp::Literal).collect(),
                None => vec![Comp::Any],
            };
            for part in rest.split('/') {
                match part {
                    "" | "." => {}
                    ".." => {
                        if matches!(comps.last(), Some(Comp::Literal(_))) {
                            comps.pop();
                        }
                    }
                    part => comps.push(Comp::parse(part)),
                }
            }
            if !patterns.contains(&comps) {
                patterns.push(comps);
            }
        }
        Rule::Anchored(patterns)
    };
    if let Some(rest) = pattern.strip_prefix("//") {
        anchored(vec![PathBuf::from("/")], rest)
    } else if pattern == "~" || pattern.starts_with("~/") {
        let home: Vec<&Path> = bases.home.into_iter().collect();
        anchored(resolved(&home), &pattern[1..])
    } else if let Some(rest) = pattern.strip_prefix('/') {
        let mut dirs = cwd_and_roots.clone();
        dirs.extend(home_claude.as_deref());
        dirs.push(Path::new("/"));
        anchored(resolved(&dirs), rest)
    } else {
        let bare = pattern.trim_end_matches('/');
        if bare.contains('/') || bare == "." || bare == ".." {
            anchored(resolved(&cwd_and_roots), bare)
        } else {
            Rule::AnyDepth(Comp::parse(bare))
        }
    }
}

/// The normal components of `path`, folded.
fn path_comps(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(fold(&name.to_string_lossy())),
            _ => None,
        })
        .collect()
}

/// Whether `pattern` matches `path` or an ancestor of it (a pattern consumed in full matches what
/// lies below). With `below`, also when `path` runs out first: a longer path could match. `**`
/// backtracks to the last one only, so the cost is bounded by the product of the lengths.
fn match_prefix(pattern: &[Comp], path: &[String], below: bool) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    loop {
        match pattern.get(p) {
            None => return true,
            Some(_) if below && t == path.len() => return true,
            Some(Comp::Any) => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(comp) if path.get(t).is_some_and(|name| comp.matches(name)) => {
                p += 1;
                t += 1;
                continue;
            }
            Some(_) => {}
        }
        match star {
            Some((star_p, star_t)) if star_t < path.len() => {
                star = Some((star_p, star_t + 1));
                p = star_p + 1;
                t = star_t + 1;
            }
            _ => return false,
        }
    }
}

/// One element of a compiled glob.
#[derive(Clone, Debug, Eq, PartialEq)]
enum GlobItem {
    /// `*`: any run of characters within the component.
    Star,
    /// `?`
    Any,
    /// `[...]`
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Literal(char),
}

impl GlobItem {
    fn matches(&self, c: char) -> bool {
        match self {
            GlobItem::Star | GlobItem::Any => true,
            GlobItem::Class { negated, ranges } => {
                ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated
            }
            GlobItem::Literal(literal) => *literal == c,
        }
    }
}

/// A gitignore-style glob for one component: `*`, `?`, `[...]`, `\x`.
fn compile_glob(pattern: &str) -> Vec<GlobItem> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut items = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '*' => {
                if items.last() != Some(&GlobItem::Star) {
                    items.push(GlobItem::Star);
                }
                i += 1;
            }
            '?' => {
                items.push(GlobItem::Any);
                i += 1;
            }
            '[' => match parse_class(&chars[i..]) {
                Some((item, used)) => {
                    items.push(item);
                    i += used;
                }
                None => {
                    items.push(GlobItem::Literal('['));
                    i += 1;
                }
            },
            '\\' if i + 1 < chars.len() => {
                items.push(GlobItem::Literal(chars[i + 1]));
                i += 2;
            }
            other => {
                items.push(GlobItem::Literal(other));
                i += 1;
            }
        }
    }
    items
}

/// A `[...]` class at the start of `pattern`, and how many characters it spans.
fn parse_class(pattern: &[char]) -> Option<(GlobItem, usize)> {
    let mut i = 1;
    let negated = matches!(pattern.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    while let Some(&c) = pattern.get(i) {
        if c == ']' && !first {
            return Some((GlobItem::Class { negated, ranges }, i + 1));
        }
        first = false;
        if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|e| *e != ']') {
            ranges.push((c, pattern[i + 2]));
            i += 3;
        } else {
            ranges.push((c, c));
            i += 1;
        }
    }
    None
}

/// Whether the whole of `text` matches `pattern`; on a mismatch it falls back to the last `*` only.
fn glob_match(pattern: &[GlobItem], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(GlobItem::Star) => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(item) if item.matches(text[t]) => {
                p += 1;
                t += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((star_p, star_t)) => {
                star = Some((star_p, star_t + 1));
                p = star_p + 1;
                t = star_t + 1;
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|item| *item == GlobItem::Star)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(rules: &[&str], cwd: &str, roots: &[&str], home: Option<&str>) -> Matcher {
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        let texts: Vec<String> = rules.iter().map(|r| r.to_string()).collect();
        Matcher::new(
            &texts,
            &Bases {
                cwd: Path::new(cwd),
                roots: &roots,
                home: home.map(Path::new),
            },
        )
        .unwrap()
    }

    #[test]
    fn anchors_are_read_wide() {
        let m = matcher(&["Read(/conf/x)"], "/w/sub", &["/w"], Some("/h"));
        for path in ["/w/conf/x", "/w/sub/conf/x", "/h/.claude/conf/x", "/conf/x"] {
            assert!(m.denied(Path::new(path)), "{path}");
        }
        assert!(!m.denied(Path::new("/elsewhere/conf/x")));
        let m = matcher(&["Read(~/n)"], "/w", &["/w"], None);
        assert!(
            m.denied(Path::new("/anywhere/n")),
            "an unknown home matches anywhere"
        );
        let m = matcher(&["Read(../up)"], "/w/sub", &[], None);
        assert!(m.denied(Path::new("/w/up")));
    }

    #[test]
    fn anchored_reaches_sees_rules_below_a_directory() {
        let m = matcher(&["Read(./.git/config)"], "/w", &[], None);
        assert!(!m.denied(Path::new("/w/.git")));
        assert!(m.anchored_reaches(Path::new("/w/.git")));
        assert!(!m.anchored_reaches(Path::new("/w/src")));
        let m = matcher(&["Read(*.pem)"], "/w", &[], None);
        assert!(!m.anchored_reaches(Path::new("/w/.git")));
    }

    #[test]
    fn globs_and_case_fold() {
        let m = matcher(&["Read(./[a-c]?.TXT)"], "/w", &[], None);
        assert!(m.denied(Path::new("/w/BX.txt")));
        assert!(!m.denied(Path::new("/w/dx.txt")));
        let m = matcher(&["Read(./a/**/k)"], "/w", &[], None);
        assert!(m.denied(Path::new("/w/a/k")) && m.denied(Path::new("/w/a/b/c/k/z")));
    }

    #[test]
    fn a_long_glob_is_not_exponential() {
        let m = matcher(&[&format!("Read({}b)", "*a".repeat(40))], "/w", &[], None);
        let start = std::time::Instant::now();
        assert!(!m.denied(Path::new(&format!("/w/{}", "a".repeat(5000)))));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
}
