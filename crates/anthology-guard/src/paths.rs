//! Path matching for G3 and G6 (GD1): guard_rules.py from `_GLOB_CHARS` through
//! `_code_hits_profile`, plus the tool-input walkers `_path_strings`, `_code_strings` and
//! `_search_roots`.
//!
//! `_expand` and `_normalize` are anthology-shell's (`expand_home`, `normalize_path`): one
//! implementation for the tracked `cd` directory and the rules (lane notes). Their fancy-regex
//! patterns (`_HOME_VAR`, `_TILDE_USER`) and this module's `_ESCAPED_SPACE` run under the
//! check's backtrack limit; hitting it is a [`Limit`] (GC-16).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use anthology_pycompat::json::Value;
use anthology_pycompat::str as pystr;
use anthology_pycompat::{fnmatch, path as pypath, quote};
use anthology_shell::{Limits, normalize_with};

use crate::data::{PREFIX_VARIANTS, PROFILE_ROOTS};
use crate::rules::{Limit, any_of, either};

/// `_MAX_WALK_DEPTH`: nested dict/list values read in a tool input.
pub(crate) const MAX_WALK_DEPTH: usize = 12;
/// `_MAX_PATH_STRING`: longer words (in code points) are not read as paths.
pub(crate) const MAX_PATH_STRING: usize = 4096;
/// `_expand_braces`'s default `limit` and its round count.
const BRACE_LIMIT: usize = 32;
const BRACE_ROUNDS: usize = 4;

/// `_MEMORY_COMPONENTS`.
const MEMORY_SEQUENCES: &[&[&str]] = &[&[".claude", "memory"], &[".claude", "agent-memory"]];

/// `_CODE_PUNCTUATION`: stripped from both ends of a code word.
const CODE_PUNCTUATION: &[char] = &[
    '(', ')', '[', ']', '{', '}', '<', '>', ',', ';', '\'', '"', '`',
];

/// The compiled patterns this module needs that are not anthology-shell's.
pub(crate) struct PathRegexes {
    /// `_ESCAPED_SPACE` (`(?<!\\)\\+ `), fancy-regex under the check's limit (GC-16).
    escaped_space: Option<fancy_regex::Regex>,
}

impl PathRegexes {
    pub(crate) fn new(limits: &Limits) -> PathRegexes {
        PathRegexes {
            escaped_space: fancy(r"(?<!\\)\\+ ", false, limits.backtrack_limit),
        }
    }
}

/// A Python pattern as a fancy-regex built with `limit` (`None` if it cannot be built, which
/// the callers treat as a limit: fail closed).
pub(crate) fn fancy(py: &str, ignorecase: bool, limit: usize) -> Option<fancy_regex::Regex> {
    let flags = anthology_pycompat::re::Flags {
        ignorecase,
        ..Default::default()
    };
    let t = anthology_pycompat::re::translate_with(py, flags).ok()?;
    fancy_regex::RegexBuilder::new(&t.pattern)
        .backtrack_limit(limit)
        .build()
        .ok()
}

/// A Python pattern without look-around, compiled once (`pycompat::re`, so classes and
/// IGNORECASE mean what they mean in Python).
fn plain(py: &str, ignorecase: bool) -> anthology_pycompat::re::Regex {
    anthology_pycompat::re::compile(py, ignorecase)
        .unwrap_or_else(|e| panic!("guard pattern {py:?} does not compile: {e}"))
}

/// Pycompat search that cannot hit a limit (a plain pattern).
fn search(re: &anthology_pycompat::re::Regex, text: &str) -> bool {
    re.is_match(text).unwrap_or(true)
}

static ENCODED_FILE_URL: std::sync::LazyLock<anthology_pycompat::re::Regex> =
    std::sync::LazyLock::new(|| {
        // re.IGNORECASE | re.DOTALL, used with `.match` (anchored at the start).
        anthology_pycompat::re::compile_with(
            r"\Afile:.*%[0-9a-f]{2}",
            anthology_pycompat::re::Flags {
                ignorecase: true,
                dotall: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("_ENCODED_FILE_URL: {e}"))
    });
static CODE_ROOT_HINT: std::sync::LazyLock<anthology_pycompat::re::Regex> =
    std::sync::LazyLock::new(|| plain(r"librar|config|chrom|google|[*?\[]", true));
static CODE_LITERAL: std::sync::LazyLock<anthology_pycompat::re::Regex> =
    std::sync::LazyLock::new(|| plain("'([^']*)'|\"([^\"]*)\"|`([^`]*)`", false));
static BRACE_ALTERNATIVES: std::sync::LazyLock<anthology_pycompat::re::Regex> =
    std::sync::LazyLock::new(|| plain(r"\{([^{}]*,[^{}]*)\}", false));

/// What the path predicates share during one check: the home, the limits, the profile
/// sequences, `home`'s own components and a cache of compiled glob patterns (Python's
/// `fnmatch` caches them too).
pub(crate) struct Paths<'a> {
    pub(crate) home: &'a str,
    limits: &'a Limits,
    re: &'a PathRegexes,
    profile: Vec<(Vec<String>, bool)>,
    memory: Vec<(Vec<String>, bool)>,
    /// `[c for c in _normalize(home, home, None).split("/") if c] if home else []`.
    home_comps: Result<Vec<String>, Limit>,
    globs: RefCell<HashMap<String, Option<fnmatch::Pattern>>>,
}

impl<'a> Paths<'a> {
    pub(crate) fn new(home: &'a str, limits: &'a Limits, re: &'a PathRegexes) -> Paths<'a> {
        // `_PROFILE_COMPONENTS`: lowercased components, and whether the last one is matched as
        // a prefix (`google-chrome-beta`).
        let profile = PROFILE_ROOTS
            .iter()
            .map(|root| {
                let comps = pystr::lower(root).split('/').map(str::to_string).collect();
                (comps, root.starts_with(".config/"))
            })
            .collect();
        let home_comps = if home.is_empty() {
            Ok(Vec::new())
        } else {
            normalize_with(home, home, None, limits)
                .map(|h| {
                    h.split('/')
                        .filter(|c| !c.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .map_err(|_| Limit)
        };
        let memory = MEMORY_SEQUENCES
            .iter()
            .map(|seq| (seq.iter().map(|c| (*c).to_string()).collect(), false))
            .collect();
        Paths {
            home,
            limits,
            re,
            profile,
            memory,
            home_comps,
            globs: RefCell::new(HashMap::new()),
        }
    }

    /// `_normalize(path, home, cwd)`.
    fn normalize(&self, path: &str, cwd: Option<&str>) -> Result<String, Limit> {
        normalize_with(path, self.home, cwd, self.limits).map_err(|_| Limit)
    }

    /// `_component_matches(candidate, protected, prefix)`.
    fn component_matches(&self, candidate: &str, protected: &str, prefix: bool) -> bool {
        if candidate.contains(['*', '?', '[']) {
            let mut globs = self.globs.borrow_mut();
            let pattern = globs
                .entry(candidate.to_string())
                .or_insert_with(|| fnmatch::compile(candidate));
            // `except re.error: return False`: a pattern Python cannot compile matches nothing.
            let Some(pattern) = pattern else {
                return false;
            };
            let variants: &[&str] = if prefix { &PREFIX_VARIANTS } else { &[""] };
            return variants
                .iter()
                .any(|v| pattern.matches(&format!("{protected}{v}")));
        }
        if prefix {
            candidate.starts_with(protected)
        } else {
            candidate == protected
        }
    }

    /// `_contains_sequence(path, sequences, prefix_flags, home_comps, anywhere)`.
    fn contains_sequence(
        &self,
        path: &str,
        sequences: &[(Vec<String>, bool)],
        home_comps: &[String],
        anywhere: bool,
    ) -> bool {
        let comps: Vec<&str> = path
            .split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .collect();
        let absolute = path.starts_with('/');
        let mut leading = 0;
        if !absolute {
            while leading < comps.len() && (comps[leading] == ".." || comps[leading].contains('$'))
            {
                leading += 1;
            }
        }
        let last_start = if absolute || anywhere {
            comps.len()
        } else {
            leading
        };
        for (sequence, prefix) in sequences {
            let size = sequence.len();
            if comps.len() < size {
                continue;
            }
            let end = last_start.min(comps.len() - size);
            for start in 0..=end {
                let window = &comps[start..start + size];
                let matched = window
                    .iter()
                    .zip(sequence.iter())
                    .enumerate()
                    .all(|(k, (c, p))| self.component_matches(c, p, *prefix && k == size - 1));
                if !matched {
                    continue;
                }
                let wildcard_only = window
                    .iter()
                    .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '*' || ch == '?'));
                if !wildcard_only {
                    return true;
                }
                if absolute && is_home_dir(&comps[..start], home_comps) {
                    return true;
                }
            }
        }
        false
    }

    /// `_is_profile_path(candidate, home, cwd, anywhere)`.
    pub(crate) fn is_profile_path(
        &self,
        candidate: &str,
        cwd: Option<&str>,
        anywhere: bool,
    ) -> Result<bool, Limit> {
        let Ok(home_comps) = &self.home_comps else {
            return Err(Limit);
        };
        any_of(bases(cwd), |base| {
            let path = self.normalize(candidate, base)?;
            Ok(!path.is_empty()
                && self.contains_sequence(&path, &self.profile, home_comps, anywhere))
        })
    }

    /// `_is_memory_path(candidate, home, cwd)`.
    pub(crate) fn is_memory_path(&self, candidate: &str, cwd: Option<&str>) -> Result<bool, Limit> {
        any_of(bases(cwd), |base| {
            let path = self.normalize(candidate, base)?;
            Ok(!path.is_empty() && self.contains_sequence(&path, &self.memory, &[], false))
        })
    }

    /// `_words_hit(words, predicate, home, cwd)`.
    pub(crate) fn words_hit<S: AsRef<str>>(
        &self,
        words: &[S],
        cwd: Option<&str>,
        predicate: impl Fn(&Paths<'_>, &str, Option<&str>) -> Result<bool, Limit>,
    ) -> Result<bool, Limit> {
        any_of(words, |word| {
            let word = word.as_ref();
            if pystr::char_len(word) > MAX_PATH_STRING {
                return Ok(false);
            }
            any_of(path_candidates(word), |piece| {
                let candidates = if piece.contains('{') {
                    expand_braces(&piece)
                } else {
                    vec![piece]
                };
                any_of(&candidates, |c| predicate(self, c.as_str(), cwd))
            })
        })
    }

    /// `_code_candidates(text)`, added to `found`.
    fn code_candidates(&self, text: &str, found: &mut HashSet<String>) -> Result<(), Limit> {
        let text = self.unescape_spaces(text)?;
        for groups in CODE_LITERAL.captures_iter(&text) {
            let groups = groups.map_err(|_| Limit)?;
            for (s, e) in groups.iter().skip(1).flatten() {
                if e > s {
                    found.insert(pystr::strip_ws(&text[*s..*e]).to_string());
                }
            }
        }
        for piece in text.split(['\'', '"', '`', '\n']) {
            found.insert(pystr::strip_ws(piece).to_string());
        }
        for word in pystr::split_ws(&text) {
            found.insert(word.trim_matches(CODE_PUNCTUATION).to_string());
        }
        found.remove("");
        Ok(())
    }

    /// `_ESCAPED_SPACE.sub(" ", text)`: an escaped space is a space however many backslashes
    /// escape it.
    fn unescape_spaces(&self, text: &str) -> Result<String, Limit> {
        if !text.contains("\\ ") {
            return Ok(text.to_string());
        }
        let re = self.re.escaped_space.as_ref().ok_or(Limit)?;
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in re.find_iter(text) {
            let m = m.map_err(|_| Limit)?;
            out.push_str(&text[last..m.start()]);
            out.push(' ');
            last = m.end();
        }
        out.push_str(&text[last..]);
        Ok(out)
    }

    /// `_code_hits_profile(texts, home, cwd)`: a profile path spelled inside any of the code
    /// texts (G3).
    pub(crate) fn code_hits_profile<S: AsRef<str>>(
        &self,
        texts: &[S],
        cwd: Option<&str>,
    ) -> Result<bool, Limit> {
        let mut candidates = HashSet::new();
        let mut limited = false;
        for text in texts {
            if self
                .code_candidates(text.as_ref(), &mut candidates)
                .is_err()
            {
                limited = true;
            }
        }
        let words: Vec<&String> = candidates.iter().filter(|c| may_name_profile(c)).collect();
        let hit = self.words_hit(&words, cwd, |p, c, cwd| p.is_profile_path(c, cwd, true));
        if limited {
            either(Err(Limit), || hit)
        } else {
            hit
        }
    }
}

/// `(None, cwd) if cwd else (None,)`: an empty `cwd` is no `cwd`.
fn bases(cwd: Option<&str>) -> Vec<Option<&str>> {
    match cwd {
        Some(c) if !c.is_empty() => vec![None, Some(c)],
        _ => vec![None],
    }
}

/// `_is_home_dir(comps, home_comps)`: `home`'s components, `/home/<u>`, `/users/<u>` or
/// `/root`.
fn is_home_dir(comps: &[&str], home_comps: &[String]) -> bool {
    (!home_comps.is_empty()
        && comps.len() == home_comps.len()
        && comps.iter().zip(home_comps).all(|(a, b)| a == b))
        || (comps.len() == 2 && (comps[0] == "home" || comps[0] == "users"))
        || comps == ["root"]
}

/// `_path_candidates(token)`: the token, the part after its first `=`, the decoded form of a
/// percent-encoded `file:` URL among those, and each `:`-separated piece; empty ones dropped.
pub(crate) fn path_candidates(token: &str) -> Vec<String> {
    let mut parts = vec![token.to_string()];
    if let Some((_, after)) = token.split_once('=') {
        parts.push(after.to_string());
    }
    let decoded: Vec<String> = parts
        .iter()
        .filter(|p| search(&ENCODED_FILE_URL, p))
        .map(|p| quote::unquote(p))
        .collect();
    parts.extend(decoded);
    let mut pieces = parts.clone();
    for part in &parts {
        if part.contains(':') {
            pieces.extend(part.split(':').map(str::to_string));
        }
    }
    pieces.retain(|p| !p.is_empty());
    pieces
}

/// `_expand_braces(path)`: `a/{b,c}` → `a/b`, `a/c`; four rounds, at most 32 results
/// (`${HOME}` has no comma and is left alone).
pub(crate) fn expand_braces(path: &str) -> Vec<String> {
    let mut results = vec![path.to_string()];
    for _ in 0..BRACE_ROUNDS {
        let mut expanded = Vec::new();
        let mut changed = false;
        for item in &results {
            match BRACE_ALTERNATIVES.captures_at(item, 0) {
                Ok(Some(groups)) => {
                    let (Some((start, end)), Some((gs, ge))) = (groups[0], groups[1]) else {
                        expanded.push(item.clone());
                        continue;
                    };
                    changed = true;
                    for alt in item[gs..ge].split(',') {
                        expanded.push(format!("{}{alt}{}", &item[..start], &item[end..]));
                    }
                }
                _ => expanded.push(item.clone()),
            }
        }
        expanded.truncate(BRACE_LIMIT);
        results = expanded;
        if !changed {
            break;
        }
    }
    results
}

/// `_may_name_profile(candidate)`: a separator (`/`, a backslash or an encoded `%2F`) and a
/// root-like word or a glob.
fn may_name_profile(candidate: &str) -> bool {
    (candidate.contains(['/', '\\']) || pystr::lower(candidate).contains("%2f"))
        && search(&CODE_ROOT_HINT, candidate)
}

/// `_PATH_KEY_WORDS`.
const PATH_KEY_WORDS: &[&str] = &[
    "path",
    "file",
    "dir",
    "folder",
    "uri",
    "url",
    "pattern",
    "glob",
    "source",
    "src",
    "dest",
    "destination",
    "target",
    "location",
    "root",
    "cwd",
];

/// `_path_strings(value, skip)`: strings under path-like keys at any depth (lists included);
/// keys in `skip` are never read.
pub(crate) fn path_strings<'v>(value: &'v Value, skip: &[&str], out: &mut Vec<&'v str>) {
    walk_paths(value, skip, false, 0, out);
}

fn walk_paths<'v>(
    value: &'v Value,
    skip: &[&str],
    in_path_key: bool,
    depth: usize,
    out: &mut Vec<&'v str>,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    match value {
        Value::Str(s) if in_path_key => out.push(s),
        Value::Object(map) => {
            for (key, item) in map.iter() {
                if skip.contains(&key) {
                    continue;
                }
                let path_like = in_path_key || {
                    let lowered = pystr::lower(key);
                    PATH_KEY_WORDS.iter().any(|w| lowered.contains(w))
                };
                walk_paths(item, skip, path_like, depth + 1, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_paths(item, skip, in_path_key, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// `_CODE_KEYS`.
const CODE_KEYS: &[&str] = &["script", "code", "expression", "func", "fn"];

/// `_code_strings(value)`: strings under keys named `script`, `code`, `expression`, `func` or
/// `fn`, at any depth (code a tool runs).
pub(crate) fn code_strings<'v>(value: &'v Value, out: &mut Vec<&'v str>) {
    walk_code(value, false, 0, out);
}

fn walk_code<'v>(value: &'v Value, in_code_key: bool, depth: usize, out: &mut Vec<&'v str>) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    match value {
        Value::Str(s) if in_code_key => out.push(s),
        Value::Object(map) => {
            for (key, item) in map.iter() {
                let code_key = in_code_key || CODE_KEYS.contains(&pystr::lower(key).as_str());
                walk_code(item, code_key, depth + 1, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_code(item, in_code_key, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// `_search_roots(tool_input, keys)`: Glob/Grep's `path` joined with each pattern key's value.
pub(crate) fn search_roots(input: &Value, keys: &[&str]) -> Vec<String> {
    let Some(Value::Str(base)) = input.as_object().and_then(|o| o.get("path")) else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|k| match input.as_object().and_then(|o| o.get(k)) {
            Some(Value::Str(p)) => Some(pypath::join(base, p)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_candidates_split_on_equals_and_colons_and_decode_file_urls() {
        assert_eq!(
            path_candidates("a=b:c"),
            ["a=b:c", "b:c", "a=b", "c", "b", "c"]
        );
        assert_eq!(
            path_candidates("file:%2Fx"),
            ["file:%2Fx", "file:/x", "file", "%2Fx", "file", "/x"]
        );
        // other URLs are not decoded
        assert_eq!(path_candidates("q%2Fx"), ["q%2Fx"]);
        assert!(path_candidates("").is_empty());
    }

    #[test]
    fn braces_expand_four_rounds_and_at_most_32() {
        assert_eq!(expand_braces("a/{b,c}/d"), ["a/b/d", "a/c/d"]);
        assert_eq!(expand_braces("${HOME}/x"), ["${HOME}/x"]);
        assert_eq!(expand_braces("{a,b}{c,d}"), ["ac", "ad", "bc", "bd"]);
        let many = "{a,b,c,d,e,f,g,h}{a,b,c,d,e,f,g,h}{a,b}";
        assert_eq!(expand_braces(many).len(), 32);
    }

    #[test]
    fn walkers_follow_keys_and_stop_at_depth() {
        let v = anthology_pycompat::json::loads(
            r#"{"file_path": "a", "content": "b", "nested": {"Dir": ["c", {"x": "d"}]}, "code": "e"}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        path_strings(&v, &["content"], &mut out);
        assert_eq!(out, ["a", "c", "d"]);
        let mut out = Vec::new();
        code_strings(&v, &mut out);
        assert_eq!(out, ["e"]);
        let mut deep = anthology_pycompat::json::Value::Str("~/.config/chromium".into());
        for _ in 0..20 {
            deep = anthology_pycompat::json::Value::Array(vec![deep]);
        }
        let mut map = anthology_pycompat::json::Map::new();
        map.insert("path".into(), deep);
        let deep = Value::Object(map);
        let mut out = Vec::new();
        path_strings(&deep, &[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn home_dirs() {
        let home = vec!["home".to_string(), "tester".to_string()];
        assert!(is_home_dir(&["home", "tester"], &home));
        assert!(is_home_dir(&["users", "x"], &[]));
        assert!(is_home_dir(&["root"], &[]));
        assert!(!is_home_dir(&["opt"], &home));
        assert!(!is_home_dir(&[], &[]));
    }
}
