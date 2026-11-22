//! `readonly_paths`: every path a proven command reads, checked on the file system.
//!
//! A path is resolved against the working directory the `cd` targets lead to, with its symlinks
//! followed (a path that does not exist yet is resolved through its longest existing ancestor). The
//! resolved path must lie under a root, must not be a special file (a FIFO, socket or device), and
//! must not match a `Read()` rule or a secret pattern, as written or as resolved. A glob is
//! expanded here, each match checked the same way. A recursive path stands for every entry under
//! it: the tree is walked, without following symlinks (no program of the table follows one below
//! its start), and every entry is held against the rules and the secret patterns. A tree that holds
//! more than [`MAX_ENTRIES`] entries is not walked to the end: no decision.

use std::path::{Component, Path, PathBuf};

use anthology_shell::{ReadPath, is_secret_path, is_secret_tree};

use super::rules::Matcher;
use super::shown;

/// The most directory entries one request may list, over every glob and walk.
pub const MAX_ENTRIES: usize = 20_000;

/// What the path check works with: the real roots, the rules and the entry budget.
pub struct Checker {
    pub roots: Vec<PathBuf>,
    pub rules: Matcher,
    left: usize,
}

/// A directory as written (lexical, for the rules anchored there) and as resolved.
#[derive(Clone)]
pub struct Place {
    pub lex: PathBuf,
    pub real: PathBuf,
}

impl Checker {
    pub fn new(roots: Vec<PathBuf>, rules: Matcher) -> Checker {
        Checker {
            roots,
            rules,
            left: MAX_ENTRIES,
        }
    }

    /// Whether `real` (resolved) lies under a root.
    pub fn inside(&self, real: &Path) -> bool {
        self.roots.iter().any(|root| real.starts_with(root))
    }

    fn spend(&mut self) -> Result<(), String> {
        if self.left == 0 {
            return Err(format!("too many files to check (over {MAX_ENTRIES})"));
        }
        self.left -= 1;
        Ok(())
    }

    /// Follows the `cd` targets from `start` and returns the working directory the rest runs in.
    pub fn follow_cd(&mut self, start: Place, targets: &[String]) -> Result<Place, String> {
        let mut at = start;
        for target in targets {
            let lex = join_clean(&at.lex, target);
            let real = std::fs::canonicalize(join_clean(&at.real, target))
                .ok()
                .filter(|p| p.is_dir())
                .ok_or_else(|| format!("cd target is not a directory: {}", shown(target)))?;
            self.judge(&lex, &real, false)?;
            at = Place { lex, real };
        }
        Ok(at)
    }

    /// Checks one path of the command, relative to `at`.
    pub fn check(&mut self, at: &Place, path: &ReadPath) -> Result<(), String> {
        if path.path.contains(['*', '?', '[']) {
            return self.expand(at, &path.path, path.recursive);
        }
        let lex = join_clean(&at.lex, &path.path);
        let real = resolve(&join_clean(&at.real, &path.path))?;
        self.literal(&lex, &real, path.recursive)
    }

    /// The containment, rule and secret checks of one resolved path.
    pub fn judge(&self, lex: &Path, real: &Path, recursive: bool) -> Result<(), String> {
        if !self.inside(real) || real.starts_with("/dev") || real.starts_with("/proc") {
            return Err(format!(
                "outside every root: {}",
                shown(&lex.to_string_lossy())
            ));
        }
        if self.rules.denied(lex) || self.rules.denied(real) {
            return Err(format!(
                "Read rule matches: {}",
                shown(&lex.to_string_lossy())
            ));
        }
        let secret = |p: &Path| {
            let p = p.to_string_lossy();
            if recursive {
                is_secret_tree(&p)
            } else {
                is_secret_path(&p)
            }
        };
        if secret(lex) || secret(real) {
            return Err(format!(
                "path of a secret: {}",
                shown(&lex.to_string_lossy())
            ));
        }
        Ok(())
    }

    fn literal(&mut self, lex: &Path, real: &Path, recursive: bool) -> Result<(), String> {
        self.judge(lex, real, recursive)?;
        match std::fs::metadata(real) {
            Ok(meta) if meta.is_dir() => {
                if recursive {
                    self.walk(lex, real)?;
                }
                Ok(())
            }
            Ok(meta) if meta.is_file() => Ok(()),
            Ok(_) => Err(format!("special file: {}", shown(&lex.to_string_lossy()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(format!("cannot read: {}", shown(&lex.to_string_lossy()))),
        }
    }

    /// Every entry under `real`, without following symlinks, against the rules and the secrets.
    fn walk(&mut self, lex: &Path, real: &Path) -> Result<(), String> {
        let mut stack = vec![(lex.to_path_buf(), real.to_path_buf(), String::new())];
        while let Some((lex, real, rel)) = stack.pop() {
            let entries = std::fs::read_dir(&real)
                .map_err(|_| format!("cannot list: {}", shown(&lex.to_string_lossy())))?;
            for entry in entries {
                let entry = entry.map_err(|_| "cannot list a directory".to_string())?;
                self.spend()?;
                let name = entry.file_name();
                let rel = if rel.is_empty() {
                    name.to_string_lossy().into_owned()
                } else {
                    format!("{rel}/{}", name.to_string_lossy())
                };
                let (lex, real) = (lex.join(&name), real.join(&name));
                let shown_rel = || shown(&lex.to_string_lossy());
                if self.rules.denied(&lex) || self.rules.denied(&real) {
                    return Err(format!("Read rule matches: {}", shown_rel()));
                }
                if is_secret_path(&rel) {
                    return Err(format!("path of a secret: {}", shown_rel()));
                }
                let kind = entry
                    .file_type()
                    .map_err(|_| format!("cannot read: {}", shown_rel()))?;
                if kind.is_dir() {
                    stack.push((lex, real, rel));
                } else if !kind.is_file() && !kind.is_symlink() {
                    return Err(format!("special file: {}", shown_rel()));
                }
            }
        }
        Ok(())
    }

    /// Expands a glob the way the shell could (read wide: `*` and `?` match a leading `.`, a class
    /// matches any one character, case is ignored) and checks every match. A `**` component makes
    /// the directory before it a recursive read.
    fn expand(&mut self, at: &Place, text: &str, recursive: bool) -> Result<(), String> {
        let start = if text.starts_with('/') {
            Place {
                lex: PathBuf::from("/"),
                real: PathBuf::from("/"),
            }
        } else {
            at.clone()
        };
        let mut frontier = vec![start];
        for comp in text.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if comp == "**" {
                for place in &frontier {
                    let real = resolve(&place.real)?;
                    self.literal(&place.lex, &real, true)?;
                }
                return Ok(());
            }
            if !comp.contains(['*', '?', '[']) {
                for place in &mut frontier {
                    place.lex.push(comp);
                    place.real.push(comp);
                }
                continue;
            }
            let glob = glob_tokens(comp);
            let mut next = Vec::new();
            for place in &frontier {
                let entries = match std::fs::read_dir(&place.real) {
                    Ok(entries) => entries,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => {
                        return Err(format!(
                            "cannot list: {}",
                            shown(&place.lex.to_string_lossy())
                        ));
                    }
                };
                for entry in entries {
                    let entry = entry.map_err(|_| "cannot list a directory".to_string())?;
                    self.spend()?;
                    let name = entry.file_name();
                    let folded: Vec<char> = name.to_string_lossy().to_lowercase().chars().collect();
                    if glob_matches(&glob, &folded) {
                        next.push(Place {
                            lex: place.lex.join(&name),
                            real: place.real.join(&name),
                        });
                    }
                }
            }
            frontier = next;
        }
        for place in frontier {
            let real = resolve(&place.real)?;
            self.literal(&place.lex, &real, recursive)?;
        }
        Ok(())
    }
}

/// `base` joined with `text` (an absolute `text` replaces it), without `.` or empty components.
/// The classifier refuses every `..`, so none is resolved here.
pub fn join_clean(base: &Path, text: &str) -> PathBuf {
    let mut out = if text.starts_with('/') {
        PathBuf::from("/")
    } else {
        base.to_path_buf()
    };
    for comp in text.split('/').filter(|c| !c.is_empty() && *c != ".") {
        out.push(comp);
    }
    out
}

/// `path` with its symlinks resolved. A path that does not exist is resolved through its longest
/// existing ancestor; a symlink that points nowhere (on the way or at the end) is refused, since
/// what it names can be created after the check.
pub fn resolve(path: &Path) -> Result<PathBuf, String> {
    let dangling = || format!("dangling symlink: {}", shown(&path.to_string_lossy()));
    match std::fs::canonicalize(path) {
        Ok(real) => return Ok(real),
        Err(e)
            if !matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Err(format!(
                "cannot resolve: {}",
                shown(&path.to_string_lossy())
            ));
        }
        Err(_) => {}
    }
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(dangling());
    }
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    let mut at = path;
    loop {
        let Some(Component::Normal(name)) = at.components().next_back() else {
            return Err(format!(
                "cannot resolve: {}",
                shown(&path.to_string_lossy())
            ));
        };
        rest.push(name);
        let parent = at
            .parent()
            .ok_or_else(|| format!("cannot resolve: {}", shown(&path.to_string_lossy())))?;
        if let Ok(mut real) = std::fs::canonicalize(parent) {
            if !real.is_dir() {
                return Err(format!(
                    "cannot resolve: {}",
                    shown(&path.to_string_lossy())
                ));
            }
            for name in rest.iter().rev() {
                real.push(name);
            }
            return Ok(real);
        }
        if std::fs::symlink_metadata(parent).is_ok() {
            return Err(dangling());
        }
        at = parent;
    }
}

/// A glob component tokenised wide: `*` any run, `?` and a bracket class any one character.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Tok {
    Star,
    One,
    Lit(char),
}

fn glob_tokens(component: &str) -> Vec<Tok> {
    let chars: Vec<char> = component.to_lowercase().chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push(Tok::Star),
            '?' => out.push(Tok::One),
            '[' => match chars[i + 1..].iter().position(|&c| c == ']') {
                Some(end) => {
                    out.push(Tok::One);
                    i += 1 + end;
                }
                None => out.push(Tok::Lit('[')),
            },
            c => out.push(Tok::Lit(c)),
        }
        i += 1;
    }
    out
}

fn glob_matches(pattern: &[Tok], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(Tok::Star) => {
                backtrack = Some((p, t));
                p += 1;
            }
            Some(Tok::One) => {
                p += 1;
                t += 1;
            }
            Some(Tok::Lit(c)) if *c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((star, at)) => {
                    backtrack = Some((star, at + 1));
                    p = star + 1;
                    t = at + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|tok| *tok == Tok::Star)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_glob_matches_dots_classes_and_any_case() {
        let m = |glob: &str, name: &str| {
            glob_matches(
                &glob_tokens(glob),
                &name.to_lowercase().chars().collect::<Vec<_>>(),
            )
        };
        assert!(m("*env", ".env"));
        assert!(m("[xyz].TXT", "q.txt"), "a class is any one character");
        assert!(m("a*b*c", "aXXbYYc"));
        assert!(!m("a*b", "ac"));
    }

    #[test]
    fn join_clean_drops_dots_and_lets_an_absolute_path_replace() {
        assert_eq!(
            join_clean(Path::new("/w"), "./a//b/."),
            PathBuf::from("/w/a/b")
        );
        assert_eq!(join_clean(Path::new("/w"), "/x/./y"), PathBuf::from("/x/y"));
    }
}
