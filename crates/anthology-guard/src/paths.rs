//! Path matching for G3 and G6 (GD1): guard_rules.py from `_GLOB_CHARS` through
//! `_code_hits_profile`, plus the tool-input walkers `_path_strings`, `_code_strings` and
//! `_search_roots`.
//!
//! `_expand` and `_normalize` are anthology-shell's (`expand_home`, `normalize_path`): one
//! implementation for the tracked `cd` directory and the rules (lane notes). Their fancy-regex
//! patterns (`_HOME_VAR`, `_TILDE_USER`) run under the check's backtrack limit; hitting it is a
//! [`Limit`] (GC-16). `_ESCAPED_SPACE` is a linear scan ([`unescape_spaces`]).
//!
//! **Past Python's caps** (owner direction, fail closed; D-8 "Rust stricter"): Python stops
//! reading a tool input below 12 nested levels, skips a word over 4096 code points, keeps 32
//! brace expansions and expands braces for 4 rounds, and allows what it did not read. With
//! `strict` (production) Rust reads the whole input and every word, whatever its length, and
//! expands braces fully on top of Python's expansions (a union, so Rust never allows what
//! Python blocks). Only a brace expansion too large to check ([`BRACE_MAX_RESULTS`],
//! [`BRACE_MAX_BYTES`]) is left unread; it marks the check [`Paths::capped`], and `check`
//! blocks it as too complex (G0), except in the code keys of a tool input (`code`, `script`, ...):
//! that text is code, not a shell word, and is read with Python's brace expansion alone then
//! ([`Paths::code_hits_profile`]). Under a relative or empty home, a G3 path that uses the home
//! (or a relative `cd` directory) is also read against an absolute stand-in for it
//! ([`STANDIN_HOME`]), so a profile under that home is found wherever the home really is.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use anthology_pycompat::json::Value;
use anthology_pycompat::str as pystr;
use anthology_pycompat::{fnmatch, path as pypath, quote};
use anthology_shell::{Limits, expand_with, normalize_with};

use crate::data::{PREFIX_VARIANTS, PROFILE_ROOTS};
use crate::rules::{Limit, any_of, either};

/// `_MAX_WALK_DEPTH`: nested dict/list values read in a tool input.
pub(crate) const MAX_WALK_DEPTH: usize = 12;
/// `_MAX_PATH_STRING`: longer words (in code points) are not read as paths.
pub(crate) const MAX_PATH_STRING: usize = 4096;
/// Strict: the backtrack budget of `_HOME_VAR` and `_TILDE_USER` per byte of a word longer
/// than [`MAX_PATH_STRING`]. Both patterns are linear (a step or two per position), so the
/// default limit would stop a long ordinary word near a million bytes; scaled, it still only
/// stops a pattern that backtracks.
const STEPS_PER_BYTE: usize = 8;
/// `_expand_braces`'s default `limit` and its round count.
const BRACE_LIMIT: usize = 32;
const BRACE_ROUNDS: usize = 4;
/// Strict brace expansion: more results than this, or more bytes than [`BRACE_MAX_BYTES`], or
/// more rounds than [`BRACE_MAX_ROUNDS`], is too large to check (the check is capped).
pub(crate) const BRACE_MAX_RESULTS: usize = 4096;
pub(crate) const BRACE_MAX_BYTES: usize = 8 << 20;
const BRACE_MAX_ROUNDS: usize = 64;
/// Strict, under a relative or empty home: the absolute folder the home is read under, and the
/// one a relative `cd` directory is read under. No real path holds the NUL, so a profile
/// sequence found below one is in the home (or the directory) itself.
const STANDIN_HOME: &str = "/\u{0}home";
const STANDIN_CWD: &str = "/\u{0}cwd";

/// `_MEMORY_COMPONENTS`.
const MEMORY_SEQUENCES: &[&[&str]] = &[&[".claude", "memory"], &[".claude", "agent-memory"]];

/// `_CODE_PUNCTUATION`: stripped from both ends of a code word.
const CODE_PUNCTUATION: &[char] = &[
    '(', ')', '[', ']', '{', '}', '<', '>', ',', ';', '\'', '"', '`',
];

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
    profile: Vec<(Vec<String>, bool)>,
    memory: Vec<(Vec<String>, bool)>,
    /// `[c for c in _normalize(home, home, None).split("/") if c] if home else []`.
    home_comps: Result<Vec<String>, Limit>,
    globs: RefCell<HashMap<String, Option<fnmatch::Pattern>>>,
    /// Production's readings past Python's caps (module docs); `false` is Python's.
    pub(crate) strict: bool,
    /// Something was too large to read (strict only): `check` blocks as too complex.
    pub(crate) capped: Cell<bool>,
    /// Strict, under a relative or empty home: [`STANDIN_HOME`] joined with the home, and its
    /// components (`home_comps` for it).
    standin: Option<(String, Result<Vec<String>, Limit>)>,
}

impl<'a> Paths<'a> {
    pub(crate) fn new(home: &'a str, limits: &'a Limits, strict: bool) -> Paths<'a> {
        // `_PROFILE_COMPONENTS`: lowercased components, and whether the last one is matched as
        // a prefix (`google-chrome-beta`).
        let profile = PROFILE_ROOTS
            .iter()
            .map(|root| {
                let comps = pystr::lower(root).split('/').map(str::to_string).collect();
                (comps, root.starts_with(".config/"))
            })
            .collect();
        let comps_of = |home: &str| {
            normalize_with(home, home, None, limits)
                .map(|h| {
                    h.split('/')
                        .filter(|c| !c.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .map_err(|_| Limit)
        };
        let home_comps = if home.is_empty() {
            Ok(Vec::new())
        } else {
            comps_of(home)
        };
        let standin = (strict && !home.starts_with('/')).then(|| {
            let h = if home.is_empty() {
                STANDIN_HOME.to_string()
            } else {
                format!("{STANDIN_HOME}/{home}")
            };
            let comps = comps_of(&h);
            (h, comps)
        });
        let memory = MEMORY_SEQUENCES
            .iter()
            .map(|seq| (seq.iter().map(|c| (*c).to_string()).collect(), false))
            .collect();
        Paths {
            home,
            limits,
            profile,
            memory,
            home_comps,
            globs: RefCell::new(HashMap::new()),
            strict,
            capped: Cell::new(false),
            standin,
        }
    }

    /// How deep the tool-input walkers read: Python's [`MAX_WALK_DEPTH`], or all of it.
    pub(crate) fn walk_depth(&self) -> Option<usize> {
        (!self.strict).then_some(MAX_WALK_DEPTH)
    }

    /// Strict, under a relative or empty home ([`Paths::standin`]): `_is_profile_path` (G3) or
    /// `_is_memory_path` (G6) read against the stand-in home, for a candidate that uses the home
    /// or resolves against a relative `cd` directory (read under [`STANDIN_CWD`]). Python reads
    /// such a path as relative, from its start only, so `~/.config/chromium` under
    /// `HOME=relative/home` was not a profile, nor `~/.claude/memory/a.md` a memory path. A home
    /// path that climbs out of the stand-in home (`~/../..`) leads somewhere unknown: a
    /// [`Limit`]. A relative path that only climbs out of the `cd` directory (`cd ..`) keeps
    /// Python's reading, as it does under an absolute home.
    fn standin_hit(
        &self,
        candidate: &str,
        cwd: Option<&str>,
        sequences: &[(Vec<String>, bool)],
        profile: bool,
        anywhere: bool,
    ) -> Result<bool, Limit> {
        let Some((home, comps)) = &self.standin else {
            return Ok(false);
        };
        let limits = self.limits_for(candidate);
        // `_expand` with a marker home: the marker shows up only where the home is used.
        let marked = expand_with(candidate, "\0", &limits).map_err(|_| Limit)?;
        let uses_home = marked.matches('\0').count() > candidate.matches('\0').count();
        let expanded = expand_with(candidate, home, &limits).map_err(|_| Limit)?;
        let relative = !expanded.starts_with('/');
        let relative_cwd = cwd.filter(|c| !c.is_empty() && !c.starts_with('/'));
        if !uses_home && !(relative && relative_cwd.is_some()) {
            return Ok(false);
        }
        let base = match cwd.filter(|c| !c.is_empty()) {
            Some(c) if c.starts_with('/') => c.to_string(),
            Some(c) => format!("{STANDIN_CWD}/{c}"),
            None => STANDIN_CWD.to_string(),
        };
        let path = normalize_with(candidate, home, Some(&base), &limits).map_err(|_| Limit)?;
        if !path.contains('\0') {
            return if uses_home { Err(Limit) } else { Ok(false) };
        }
        // `_is_memory_path` passes no home components (as Python does)
        let comps: &[String] = if profile {
            comps.as_ref().map_err(|_| Limit)?
        } else {
            &[]
        };
        Ok(self.contains_sequence(&path, sequences, comps, anywhere))
    }

    /// `_normalize(path, home, cwd)`.
    fn normalize(&self, path: &str, cwd: Option<&str>) -> Result<String, Limit> {
        normalize_with(path, self.home, cwd, &self.limits_for(path)).map_err(|_| Limit)
    }

    /// The limits for reading `path`: the check's, with the backtrack limit scaled to a long
    /// word in strict mode ([`STEPS_PER_BYTE`]).
    fn limits_for(&self, path: &str) -> Limits {
        let mut limits = *self.limits;
        if self.strict && path.len() > MAX_PATH_STRING {
            let scaled = path.len().saturating_mul(STEPS_PER_BYTE);
            limits.backtrack_limit = limits.backtrack_limit.max(scaled);
        }
        limits
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

    /// Strict, a candidate longer than Python's [`MAX_PATH_STRING`] (which Python never read):
    /// whether it can name one of `sequences` at all, a necessary condition checked in one scan
    /// so a long ordinary word stays cheap. A window's last component equals (or starts with)
    /// the sequence's last one, and comes from the candidate's text, the home or the `cd`
    /// directory: so without a glob, a `$` or a `~` (which splice the home into a component),
    /// the protected text must start a component of the candidate (at its start or after a `/`
    /// or a backslash), or appear in the home or the directory.
    fn long_word_may_hit(
        &self,
        candidate: &str,
        cwd: Option<&str>,
        sequences: &[(Vec<String>, bool)],
    ) -> bool {
        if candidate.contains(['*', '?', '[', '$', '~'])
            || self.home.contains(['*', '?', '['])
            || cwd.is_some_and(|c| c.contains(['*', '?', '[']))
        {
            return true;
        }
        let lowered = pystr::lower(candidate);
        let home = pystr::lower(self.home);
        let cwd = pystr::lower(cwd.unwrap_or(""));
        sequences.iter().any(|(sequence, _)| {
            let Some(last) = sequence.last().map(String::as_str) else {
                return true;
            };
            home.contains(last)
                || cwd.contains(last)
                || lowered
                    .match_indices(last)
                    .any(|(at, _)| at == 0 || matches!(lowered.as_bytes()[at - 1], b'/' | b'\\'))
        })
    }

    /// Strict: whether every brace expansion of `piece` is one path component that cannot
    /// complete a protected sequence, so the full expansion can be skipped. Expanding only
    /// deletes text, so a piece with no `/` or backslash expands to text with none; without a
    /// `~` or a `$` no home is spliced in. That text normalizes to one component, and every
    /// profile and memory sequence has at least two: the rest of a window would have to come
    /// from the `cd` directory, so no component of it may match a sequence's component. This is
    /// what keeps JSON and dict literals in commands (`curl -d '[{"a":1,"b":2}, …]'`) from
    /// multiplying out to the brace cap.
    fn one_component(&self, piece: &str, cwd: Option<&str>) -> bool {
        if piece.contains(['/', '\\', '~', '$']) {
            return false;
        }
        let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
            return true;
        };
        let lowered = pystr::lower(cwd);
        !lowered.split('/').any(|comp| {
            comp.contains(['*', '?', '['])
                || self
                    .profile
                    .iter()
                    .chain(&self.memory)
                    .any(|(sequence, prefix)| {
                        let last = sequence.len() - 1;
                        sequence
                            .iter()
                            .enumerate()
                            .any(|(k, p)| self.component_matches(comp, p, *prefix && k == last))
                    })
        })
    }

    /// Whether a strict check can skip `candidate` ([`Paths::long_word_may_hit`]).
    fn skip_long(
        &self,
        candidate: &str,
        cwd: Option<&str>,
        sequences: &[(Vec<String>, bool)],
    ) -> bool {
        self.strict
            && candidate.len() > MAX_PATH_STRING
            && !self.long_word_may_hit(candidate, cwd, sequences)
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
        if self.skip_long(candidate, cwd, &self.profile) {
            return Ok(false);
        }
        let hit = any_of(bases(cwd), |base| {
            let path = self.normalize(candidate, base)?;
            Ok(!path.is_empty()
                && self.contains_sequence(&path, &self.profile, home_comps, anywhere))
        });
        either(hit, || {
            self.standin_hit(candidate, cwd, &self.profile, true, anywhere)
        })
    }

    /// `_is_memory_path(candidate, home, cwd)`.
    pub(crate) fn is_memory_path(&self, candidate: &str, cwd: Option<&str>) -> Result<bool, Limit> {
        if self.skip_long(candidate, cwd, &self.memory) {
            return Ok(false);
        }
        let hit = any_of(bases(cwd), |base| {
            let path = self.normalize(candidate, base)?;
            Ok(!path.is_empty() && self.contains_sequence(&path, &self.memory, &[], false))
        });
        either(hit, || {
            self.standin_hit(candidate, cwd, &self.memory, false, false)
        })
    }

    /// `_words_hit(words, predicate, home, cwd)`.
    pub(crate) fn words_hit<S: AsRef<str>>(
        &self,
        words: &[S],
        cwd: Option<&str>,
        predicate: impl Fn(&Paths<'_>, &str, Option<&str>) -> Result<bool, Limit>,
    ) -> Result<bool, Limit> {
        self.words_hit_with(words, cwd, true, predicate)
    }

    /// [`Paths::words_hit`]; `cap_blocks` says what a brace expansion too large to check means
    /// (strict only). `true`: the check is [`Paths::capped`]. `false`: the word is read with
    /// Python's own brace expansion only, as the non-strict check reads it.
    fn words_hit_with<S: AsRef<str>>(
        &self,
        words: &[S],
        cwd: Option<&str>,
        cap_blocks: bool,
        predicate: impl Fn(&Paths<'_>, &str, Option<&str>) -> Result<bool, Limit>,
    ) -> Result<bool, Limit> {
        any_of(words, |word| {
            let word = word.as_ref();
            if !self.strict && pystr::char_len(word) > MAX_PATH_STRING {
                return Ok(false);
            }
            any_of(path_candidates(word), |piece| {
                let candidates = if !piece.contains('{') {
                    vec![piece]
                } else if self.strict {
                    let mut all = expand_braces(&piece);
                    if !self.one_component(&piece, cwd) {
                        match expand_braces_fully(&piece) {
                            Some(full) => all.extend(full),
                            None if cap_blocks => self.capped.set(true),
                            None => {}
                        }
                    }
                    all
                } else {
                    expand_braces(&piece)
                };
                any_of(&candidates, |c| predicate(self, c.as_str(), cwd))
            })
        })
    }

    /// `_code_candidates(text)`, added to `found`.
    fn code_candidates(&self, text: &str, found: &mut HashSet<String>) -> Result<(), Limit> {
        let text = unescape_spaces(text);
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

    /// `_code_hits_profile(texts, home, cwd)`: a profile path spelled inside any of the code
    /// texts (G3). `cap_blocks` is as in [`Paths::words_hit_with`]: `true` for code that a shell
    /// command runs (`python3 -c ...`), `false` for the code keys of a tool input, which are
    /// not a shell word (a JavaScript object literal is not a brace expansion).
    pub(crate) fn code_hits_profile<S: AsRef<str>>(
        &self,
        texts: &[S],
        cwd: Option<&str>,
        cap_blocks: bool,
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
        let hit = self.words_hit_with(&words, cwd, cap_blocks, |p, c, cwd| {
            p.is_profile_path(c, cwd, true)
        });
        if limited {
            either(Err(Limit), || hit)
        } else {
            hit
        }
    }
}

/// `_ESCAPED_SPACE.sub(" ", text)` (`(?<!\\)\\+ `): every maximal run of backslashes followed by
/// a space becomes one space, however many backslashes escape it. A linear scan: the pattern's
/// look-behind only says the run is maximal, so no regex (and no backtrack limit) is needed.
pub(crate) fn unescape_spaces(text: &str) -> String {
    if !text.contains("\\ ") {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] == b'\\' {
            j += 1;
        }
        if bytes.get(j) == Some(&b' ') {
            out.push_str(&text[last..i]);
            out.push(' ');
            last = j + 1;
            i = j + 1;
        } else {
            i = j;
        }
    }
    out.push_str(&text[last..]);
    out
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

/// Every expansion of `path`'s brace groups, round after round until none is left (no
/// truncation); `None` when that is more than [`BRACE_MAX_RESULTS`] results,
/// [`BRACE_MAX_BYTES`] bytes or [`BRACE_MAX_ROUNDS`] rounds.
pub(crate) fn expand_braces_fully(path: &str) -> Option<Vec<String>> {
    let mut results = vec![path.to_string()];
    for _ in 0..BRACE_MAX_ROUNDS {
        let mut expanded = Vec::new();
        let mut bytes = 0usize;
        let mut changed = false;
        for item in &results {
            let groups = match BRACE_ALTERNATIVES.captures_at(item, 0) {
                Ok(Some(groups)) => groups,
                _ => {
                    bytes += item.len();
                    expanded.push(item.clone());
                    continue;
                }
            };
            let (Some((start, end)), Some((gs, ge))) = (groups[0], groups[1]) else {
                bytes += item.len();
                expanded.push(item.clone());
                continue;
            };
            changed = true;
            for alt in item[gs..ge].split(',') {
                bytes += start + alt.len() + (item.len() - end);
                if expanded.len() >= BRACE_MAX_RESULTS || bytes > BRACE_MAX_BYTES {
                    return None;
                }
                expanded.push(format!("{}{alt}{}", &item[..start], &item[end..]));
            }
        }
        results = expanded;
        if !changed {
            return Some(results);
        }
    }
    None
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
/// keys in `skip` are never read. `max_depth` is Python's cap ([`Paths::walk_depth`]); `None`
/// reads the whole value. Iterative, so any depth is safe; the order is Python's.
pub(crate) fn path_strings<'v>(
    value: &'v Value,
    skip: &[&str],
    max_depth: Option<usize>,
    out: &mut Vec<&'v str>,
) {
    let mut stack = vec![(value, false, 0usize)];
    while let Some((value, in_path_key, depth)) = stack.pop() {
        if max_depth.is_some_and(|m| depth > m) {
            continue;
        }
        match value {
            Value::Str(s) if in_path_key => out.push(s),
            Value::Object(map) => {
                let start = stack.len();
                for (key, item) in map.iter() {
                    if skip.contains(&key) {
                        continue;
                    }
                    let path_like = in_path_key || {
                        let lowered = pystr::lower(key);
                        PATH_KEY_WORDS.iter().any(|w| lowered.contains(w))
                    };
                    stack.push((item, path_like, depth + 1));
                }
                stack[start..].reverse();
            }
            Value::Array(items) => {
                let start = stack.len();
                stack.extend(items.iter().map(|item| (item, in_path_key, depth + 1)));
                stack[start..].reverse();
            }
            _ => {}
        }
    }
}

/// `_CODE_KEYS`.
const CODE_KEYS: &[&str] = &["script", "code", "expression", "func", "fn"];

/// `_code_strings(value)`: strings under keys named `script`, `code`, `expression`, `func` or
/// `fn`, at any depth (code a tool runs). `max_depth` as in [`path_strings`].
pub(crate) fn code_strings<'v>(value: &'v Value, max_depth: Option<usize>, out: &mut Vec<&'v str>) {
    let mut stack = vec![(value, false, 0usize)];
    while let Some((value, in_code_key, depth)) = stack.pop() {
        if max_depth.is_some_and(|m| depth > m) {
            continue;
        }
        match value {
            Value::Str(s) if in_code_key => out.push(s),
            Value::Object(map) => {
                let start = stack.len();
                for (key, item) in map.iter() {
                    let code_key = in_code_key || CODE_KEYS.contains(&pystr::lower(key).as_str());
                    stack.push((item, code_key, depth + 1));
                }
                stack[start..].reverse();
            }
            Value::Array(items) => {
                let start = stack.len();
                stack.extend(items.iter().map(|item| (item, in_code_key, depth + 1)));
                stack[start..].reverse();
            }
            _ => {}
        }
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
    fn full_brace_expansion_has_no_round_or_count_truncation() {
        let many = "{a,b,c,d,e,f,g,h}{a,b,c,d,e,f,g,h}{a,b}";
        assert_eq!(expand_braces_fully(many).map(|r| r.len()), Some(128));
        // a fifth group Python leaves as text
        let five = "~/{a,b}/{a,b}/{a,b}/{a,b}/../../../../{.config,x}/chromium";
        assert!(expand_braces(five).iter().all(|r| r.contains('{')));
        let full = expand_braces_fully(five).unwrap();
        assert_eq!(full.len(), 32);
        assert!(full.contains(&"~/a/a/a/a/../../../../.config/chromium".to_string()));
        assert_eq!(expand_braces_fully("a/b"), Some(vec!["a/b".to_string()]));
        // past the caps: too large to check
        assert_eq!(expand_braces_fully(&"{a,b}".repeat(13)), None);
        assert!(expand_braces_fully(&"{a,b}".repeat(12)).is_some());
        let wide = format!("{{{}}}", vec!["x"; BRACE_MAX_RESULTS + 1].join(","));
        assert_eq!(expand_braces_fully(&wide), None);
        let long = format!("{}{{a,b}}", "x".repeat(BRACE_MAX_BYTES / 2));
        assert_eq!(expand_braces_fully(&long), None);
    }

    #[test]
    fn walkers_follow_keys_and_stop_at_depth() {
        let v = anthology_pycompat::json::loads(
            r#"{"file_path": "a", "content": "b", "nested": {"Dir": ["c", {"x": "d"}]}, "code": "e"}"#,
        )
        .unwrap();
        for depth in [Some(MAX_WALK_DEPTH), None] {
            let mut out = Vec::new();
            path_strings(&v, &["content"], depth, &mut out);
            assert_eq!(out, ["a", "c", "d"]);
            let mut out = Vec::new();
            code_strings(&v, depth, &mut out);
            assert_eq!(out, ["e"]);
        }
        let mut deep = anthology_pycompat::json::Value::Str("~/.config/chromium".into());
        for _ in 0..20 {
            deep = anthology_pycompat::json::Value::Array(vec![deep]);
        }
        let mut map = anthology_pycompat::json::Map::new();
        map.insert("path".into(), deep);
        let deep = Value::Object(map);
        let mut out = Vec::new();
        path_strings(&deep, &[], Some(MAX_WALK_DEPTH), &mut out);
        assert!(out.is_empty());
        // strict: the whole input is read, at any depth
        path_strings(&deep, &[], None, &mut out);
        assert_eq!(out, ["~/.config/chromium"]);
    }

    #[test]
    fn escaped_spaces_match_the_python_pattern() {
        assert_eq!(unescape_spaces("a\\ b"), "a b");
        assert_eq!(unescape_spaces("a\\\\\\ b"), "a b");
        assert_eq!(unescape_spaces("a\\b\\\\ c"), "a\\b c");
        assert_eq!(unescape_spaces("\\\\"), "\\\\");
        assert_eq!(unescape_spaces("x\\"), "x\\");
        assert_eq!(unescape_spaces("\\ \\ "), "  ");
        assert_eq!(unescape_spaces("\u{e9}\\ \u{fc}"), "\u{e9} \u{fc}");
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
