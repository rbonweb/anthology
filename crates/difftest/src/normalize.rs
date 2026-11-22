//! Output normalization (F4, design §13.2): `<ROOT>`, `<TODAY>`, durations, and the D-2 and D-4
//! hooks a surface opts into.
//!
//! A surface builds one [`Normalizer`] per case (its sandbox roots differ per side) and applies
//! it to both sides' [`Observed`]. The plain rules (roots, today, durations) are not divergences:
//! they remove what legitimately differs between two runs. The D-2 and D-4 hooks hide an
//! allowed GC-16 divergence, so [`Normalizer::apply_python`] notes their class on the oracle
//! whenever they change Python's output; the case then needs a matching entry in the surface's
//! divergence file, exactly like an oracle-marked divergence.
//!
//! ```ignore
//! let n = Normalizer::new().root(&sandbox).today_local().durations().d2_prefix("error: ");
//! n.apply_python(&mut python, oracle);
//! n.apply(&mut rust);
//! ```
#![allow(
    dead_code,
    reason = "published for the surface lanes; not every item has a caller yet"
)]

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anthology_testkit::tree::TreeEntry;

use crate::divergences::Class;
use crate::observe::Observed;
use crate::oracle::Oracle;

/// `<ROOT>` replaces a sandbox root.
pub const ROOT: &str = "<ROOT>";
/// `<TODAY>` replaces today's ISO date.
pub const TODAY: &str = "<TODAY>";
/// `<DURATION>` replaces a duration.
pub const DURATION: &str = "<DURATION>";
/// What the D-2 hook leaves after its prefix.
pub const LIBTEXT: &str = "<LIBTEXT>";
/// What the D-4 hook leaves of stderr at exit 2.
pub const USAGE: &str = "<USAGE>";

/// A composable set of rewrites (module docs).
#[derive(Clone, Debug, Default)]
pub struct Normalizer {
    roots: Vec<(Vec<u8>, String)>,
    today: Option<String>,
    durations: bool,
    d2: Vec<Vec<u8>>,
    d4: bool,
}

impl Normalizer {
    pub fn new() -> Normalizer {
        Normalizer::default()
    }

    /// `path` (and its realpath) → `<ROOT>`.
    pub fn root(self, path: &Path) -> Normalizer {
        self.root_as(path, ROOT)
    }

    /// `path` (and its realpath) → `mark`. Longer roots are replaced first.
    pub fn root_as(mut self, path: &Path, mark: &str) -> Normalizer {
        let mut add = |p: &Path| {
            let b = p.as_os_str().as_bytes().to_vec();
            if !b.is_empty() && !self.roots.iter().any(|(r, _)| *r == b) {
                self.roots.push((b, mark.to_string()));
            }
        };
        add(path);
        if let Ok(real) = std::fs::canonicalize(path) {
            add(&real);
        }
        self.roots.sort_by_key(|r| std::cmp::Reverse(r.0.len()));
        self
    }

    /// The ISO date `iso` → `<TODAY>`.
    pub fn today(mut self, iso: &str) -> Normalizer {
        self.today = Some(iso.to_string());
        self
    }

    /// Today's local date → `<TODAY>`.
    pub fn today_local(self) -> Normalizer {
        let iso = anthology_core::clock::today_local().iso();
        self.today(&iso)
    }

    /// Numbers followed by `ms` or `s` (`12ms`, `0.53s`, `1.5 s`) → `<DURATION>`. Only for
    /// surfaces that print real timings: it also hides fixed texts such as "times out after
    /// 20 s", which then go uncompared.
    pub fn durations(mut self) -> Normalizer {
        self.durations = true;
        self
    }

    /// D-2: on any line holding `prefix`, the text after it → `<LIBTEXT>`.
    pub fn d2_prefix(mut self, prefix: &str) -> Normalizer {
        self.d2.push(prefix.as_bytes().to_vec());
        self
    }

    /// D-4: at exit 2, when stderr starts with argparse's `usage: `, the usage block (every line
    /// before the first `<prog>: error: ` line) → `<USAGE>`. The error line and everything
    /// after it are still compared, and so is any other exit-2 output.
    pub fn d4_usage(mut self) -> Normalizer {
        self.d4 = true;
        self
    }

    /// The plain rules and the D-2 hook applied to `b`.
    pub fn bytes(&self, b: &[u8]) -> Vec<u8> {
        let mut out = b.to_vec();
        for (root, mark) in &self.roots {
            out = replace_root(&out, root, mark.as_bytes());
        }
        if let Some(t) = &self.today {
            out = replace(&out, t.as_bytes(), TODAY.as_bytes());
        }
        if self.durations {
            out = durations(&out);
        }
        for prefix in &self.d2 {
            out = d2(&out, prefix);
        }
        out
    }

    /// Applies every rule to `o`; returns the divergence classes whose hooks changed it.
    pub fn apply(&self, o: &mut Observed) -> Vec<Class> {
        let mut classes = Vec::new();
        let plain = Normalizer {
            d2: Vec::new(),
            d4: false,
            ..self.clone()
        };
        for stream in [&mut o.stdout, &mut o.stderr] {
            let base = plain.bytes(stream);
            let full = self.bytes(stream);
            if full != base && !classes.contains(&Class::D2) {
                classes.push(Class::D2);
            }
            *stream = full;
        }
        if self.d4
            && o.exit == 2
            && let Some(new) = usage_block(&o.stderr)
            && new != o.stderr
        {
            o.stderr = new;
            classes.push(Class::D4);
        }
        if let Some(tree) = &mut o.files {
            for entry in tree.0.values_mut() {
                match entry {
                    TreeEntry::File { bytes, .. } => *bytes = plain.bytes(bytes),
                    TreeEntry::Symlink { target } => {
                        *target = String::from_utf8_lossy(&plain.bytes(target.as_bytes())).into()
                    }
                    _ => {}
                }
            }
        }
        classes
    }

    /// [`Normalizer::apply`] to Python's side, noting each hook class that changed it on the
    /// oracle (module docs).
    pub fn apply_python(&self, o: &mut Observed, oracle: &mut Oracle) {
        for class in self.apply(o) {
            oracle.mark(class.as_str());
        }
    }
}

/// `stderr` with its leading argparse usage block replaced (see `d4_usage`), or `None` when it
/// does not start with `usage: `.
fn usage_block(stderr: &[u8]) -> Option<Vec<u8>> {
    if !stderr.starts_with(b"usage: ") {
        return None;
    }
    let lines: Vec<&[u8]> = stderr.split(|&c| c == b'\n').collect();
    let is_error = |l: &&[u8]| {
        l.windows(9)
            .position(|w| w == b": error: ")
            .is_some_and(|at| at > 0 && !l[..at].contains(&b' '))
    };
    let end = lines.iter().position(is_error).unwrap_or(lines.len());
    let mut out = USAGE.as_bytes().to_vec();
    for line in &lines[end..] {
        out.push(b'\n');
        out.extend_from_slice(line);
    }
    if end == lines.len() && stderr.ends_with(b"\n") {
        out.push(b'\n');
    }
    Some(out)
}

/// [`replace`] for a path: only where the path ends (the next byte is not a path name byte),
/// so `/tmp/x` does not rewrite `/tmp/x2`.
fn replace_root(hay: &[u8], root: &[u8], with: &[u8]) -> Vec<u8> {
    let name_byte = |c: u8| c.is_ascii_alphanumeric() || b"._-~+".contains(&c) || c >= 0x80;
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if !root.is_empty()
            && hay[i..].starts_with(root)
            && hay.get(i + root.len()).is_none_or(|&c| !name_byte(c))
        {
            out.extend_from_slice(with);
            i += root.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn replace(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn durations(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit() && (i == 0 || !is_word(b[i - 1]) && b[i - 1] != b'.');
        if starts {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            let mut k = j;
            if k < b.len() && b[k] == b' ' {
                k += 1;
            }
            let unit = if b[k..].starts_with(b"ms") {
                Some(k + 2)
            } else if b[k..].starts_with(b"s") {
                Some(k + 1)
            } else {
                None
            };
            if let Some(end) = unit.filter(|&e| e == b.len() || !is_word(b[e])) {
                out.extend_from_slice(DURATION.as_bytes());
                i = end;
                continue;
            }
            out.extend_from_slice(&b[i..j]);
            i = j;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn d2(b: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for (n, line) in b.split(|&c| c == b'\n').enumerate() {
        if n > 0 {
            out.push(b'\n');
        }
        match line.windows(prefix.len().max(1)).position(|w| w == prefix) {
            Some(at) if !prefix.is_empty() => {
                out.extend_from_slice(&line[..at + prefix.len()]);
                out.extend_from_slice(LIBTEXT.as_bytes());
            }
            _ => out.extend_from_slice(line),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_testkit::Tree;

    #[test]
    fn roots_today_and_durations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = std::fs::canonicalize(dir.path()).expect("realpath");
        let n = Normalizer::new()
            .root(dir.path())
            .today("2026-11-21")
            .durations();
        let text = format!(
            "{}/a {}/b {}2/c {}: on 2026-11-21 took 12ms, 0.53s and 2 s; 3 files, v1.2s3, \
             x12s, 5ms.",
            dir.path().display(),
            real.display(),
            real.display(),
            real.display()
        );
        let want = format!(
            "<ROOT>/a <ROOT>/b {}2/c <ROOT>: on <TODAY> took <DURATION>, <DURATION> and \
             <DURATION>; 3 files, v1.2s3, x12s, <DURATION>.",
            real.display()
        );
        assert_eq!(String::from_utf8_lossy(&n.bytes(text.as_bytes())), want);
    }

    #[test]
    fn hooks_report_their_class_only_when_they_change_something() {
        let n = Normalizer::new().d2_prefix("error: ").d4_usage();
        let mut o = Observed::text(1, "ok\n", "x\nerror: [Errno 2] text\n");
        assert_eq!(n.apply(&mut o), [Class::D2]);
        assert_eq!(o.stderr, b"x\nerror: <LIBTEXT>\n");
        let mut o = Observed::text(2, "", "usage: x [-h]\n       [--all]\nx: error: bad\n");
        assert_eq!(n.apply(&mut o), [Class::D2, Class::D4]);
        assert_eq!(o.stderr, b"<USAGE>\nx: error: <LIBTEXT>\n");
        let mut o = Observed::text(0, "fine", "");
        assert!(n.apply(&mut o).is_empty());
        let mut oracle = Oracle::spawn("/nonexistent".into(), vec![], "/".into());
        let mut o = Observed::text(2, "", "usage: x\n");
        n.apply_python(&mut o, &mut oracle);
        assert_eq!(o.stderr, b"<USAGE>\n");
        assert_eq!(oracle.take_notes().marks, ["D-4"]);
    }

    #[test]
    fn d4_keeps_the_error_line_and_other_exit_2_output() {
        let n = Normalizer::new().d4_usage();
        // Different usage text, different error lines: still a diff.
        let mut py = Observed::text(2, "", "usage: x [-h] A\nx: error: argument A: bad\n");
        let mut rs = Observed::text(2, "", "usage: x [-h] [A]\nx: error: argument B: bad\n");
        n.apply(&mut py);
        n.apply(&mut rs);
        assert_ne!(py, rs);
        assert_eq!(py.stderr, b"<USAGE>\nx: error: argument A: bad\n");
        // Same error line, different usage: equal.
        let mut rs = Observed::text(2, "", "usage: x [-h] [A]\nx: error: argument A: bad\n");
        n.apply(&mut rs);
        assert_eq!(py, rs);
        // Not argparse: untouched.
        let blocked = "Blocked by anthology-guardrails (G1): usage: of rm -rf\n";
        let mut o = Observed::text(2, "", blocked);
        assert!(n.apply(&mut o).is_empty());
        assert_eq!(o.stderr, blocked.as_bytes());
    }

    #[test]
    fn files_get_the_plain_rules() {
        let n = Normalizer::new().root(Path::new("/sb/1"));
        let mut tree = Tree::default();
        tree.0.insert(
            "f".into(),
            TreeEntry::File {
                bytes: b"path=/sb/1/x".to_vec(),
                mode: 0o644,
            },
        );
        tree.0.insert(
            "l".into(),
            TreeEntry::Symlink {
                target: "/sb/1/y".into(),
            },
        );
        let mut o = Observed {
            files: Some(tree),
            ..Observed::default()
        };
        n.apply(&mut o);
        let t = o.files.expect("files");
        assert_eq!(t.file("f"), Some(&b"path=<ROOT>/x"[..]));
        assert_eq!(
            t.get("l"),
            Some(&TreeEntry::Symlink {
                target: "<ROOT>/y".into()
            })
        );
    }
}
