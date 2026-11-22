//! `readonly_paths`: every path a proven command reads, checked on the file system.
//!
//! A path is resolved against the working directory the `cd` targets lead to, with its symlinks
//! followed. It must exist, lie under a root (compared without case on macOS, whose default volume
//! ignores it), be a regular file or a directory, and match no `Read()` rule or secret pattern, as
//! written or as resolved. A glob is not proven: what the shell expands it to is not known here. A
//! recursive path stands for every entry under it: an anchored rule that could match below it
//! refuses, and the tree is walked, without following symlinks (no program of the table follows
//! one below its start), with every entry held against the rules and the secret patterns. A tree of
//! more than [`MAX_ENTRIES`] entries refuses. Every resolved path is recorded for the answer.

use std::path::{Path, PathBuf};

use anthology_shell::{ReadPath, is_secret_path, is_secret_tree};

use super::rules::Matcher;
use super::shown;

/// The most directory entries one request may list, over every walk.
pub const MAX_ENTRIES: usize = 20_000;

/// What the path check works with: the real roots, the rules, the entry budget, and the resolved
/// paths the command reads, in order.
pub struct Checker {
    pub roots: Vec<PathBuf>,
    pub rules: Matcher,
    pub reads: Vec<PathBuf>,
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
            reads: Vec::new(),
            left: MAX_ENTRIES,
        }
    }

    /// Whether `real` (resolved) lies under a root.
    pub fn inside(&self, real: &Path) -> bool {
        self.roots.iter().any(|root| under(real, root))
    }

    /// Records a resolved path the command reads, once.
    pub fn record(&mut self, real: &Path) {
        if !self.reads.iter().any(|p| p == real) {
            self.reads.push(real.to_path_buf());
        }
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
            self.record(&real);
            at = Place { lex, real };
        }
        Ok(at)
    }

    /// Checks one path of the command, relative to `at`.
    pub fn check(&mut self, at: &Place, path: &ReadPath) -> Result<(), String> {
        if path.path.contains(['*', '?', '[']) {
            return Err(format!("a glob: {}", shown(&path.path)));
        }
        let lex = join_clean(&at.lex, &path.path);
        let real = resolve(&join_clean(&at.real, &path.path))?;
        self.judge(&lex, &real, path.recursive)?;
        let meta = std::fs::metadata(&real)
            .map_err(|_| format!("cannot read: {}", shown(&lex.to_string_lossy())))?;
        if meta.is_dir() {
            if path.recursive {
                self.below(&lex, &real)?;
            }
        } else if !meta.is_file() {
            return Err(format!("special file: {}", shown(&lex.to_string_lossy())));
        }
        self.record(&real);
        Ok(())
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

    /// What lies below a directory read recursively: no anchored rule may reach into it, and no
    /// entry (walked without following symlinks) may match a rule or a secret pattern.
    pub fn below(&mut self, lex: &Path, real: &Path) -> Result<(), String> {
        if self.rules.anchored_reaches(lex) || self.rules.anchored_reaches(real) {
            return Err(format!(
                "Read rule could match below: {}",
                shown(&lex.to_string_lossy())
            ));
        }
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
}

/// Whether `path` lies under `root`, component by component; without case on macOS.
fn under(path: &Path, root: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return path.starts_with(root);
    }
    let fold = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let (path, root) = (fold(path), fold(root));
    path.len() >= root.len() && path[..root.len()] == root[..]
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

/// `path` with its symlinks resolved; it must exist.
pub fn resolve(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|_| {
        if std::fs::symlink_metadata(path).is_ok() {
            format!("dangling symlink: {}", shown(&path.to_string_lossy()))
        } else {
            format!("does not exist: {}", shown(&path.to_string_lossy()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_clean_drops_dots_and_lets_an_absolute_path_replace() {
        assert_eq!(
            join_clean(Path::new("/w"), "./a//b/."),
            PathBuf::from("/w/a/b")
        );
        assert_eq!(join_clean(Path::new("/w"), "/x/./y"), PathBuf::from("/x/y"));
    }

    #[test]
    fn under_compares_whole_components() {
        assert!(under(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(!under(Path::new("/a/bc"), Path::new("/a/b")));
        assert_eq!(
            under(Path::new("/A/B/c"), Path::new("/a/b")),
            cfg!(target_os = "macos")
        );
    }
}
