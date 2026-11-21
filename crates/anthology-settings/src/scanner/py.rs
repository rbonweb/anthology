//! Private stand-ins for the parts of P1's `pycompat` that ST1 needs before P1 lands (lead
//! ruling R1). Each mirrors the plan §3.2 signature, so the swap to `anthology_pycompat::…` is
//! mechanical; the settings difftest checks them through every diff and path it compares.
//!
//! - [`is_space`]: `re`'s `\s` / `str.isspace` (CPython 3.12).
//! - [`splitlines`]: Python's `str.splitlines()`.
//! - [`unified_diff`] and [`SequenceMatcher`]: `difflib` (3.12), `lineterm=""`.
//! - [`realpath`], [`normpath_bytes`], [`dirname`]: `posixpath` (3.12, `strict=False`).
//! - [`repr_str`]: `repr(str)`, through `PyExc::KeyError`'s text (exact once P1 fills the
//!   printable tables).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::hash::Hash;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use anthology_pycompat::pyerr::PyExc;

/// `re.match(r"\s", c)` for a str pattern (equal to `c.isspace()` in CPython 3.12).
pub(crate) fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\u{1c}'..='\u{20}'
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// `s.splitlines()`: split at `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`–`\x1e`, U+0085, U+2028 and
/// U+2029; a final line break adds no empty line.
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' && matches!(it.peek(), Some((_, '\n'))) {
            it.next();
            end += 1;
        }
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A `(tag, i1, i2, j1, j2)` opcode.
pub(crate) type Opcode = (&'static str, usize, usize, usize, usize);

/// `difflib.SequenceMatcher(None, a, b, autojunk)`.
pub(crate) struct SequenceMatcher<'a, T> {
    a: &'a [T],
    b: &'a [T],
    b2j: HashMap<&'a T, Vec<usize>>,
}

impl<'a, T: Eq + Hash> SequenceMatcher<'a, T> {
    pub(crate) fn new(a: &'a [T], b: &'a [T], autojunk: bool) -> Self {
        let mut b2j: HashMap<&'a T, Vec<usize>> = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt).or_default().push(i);
        }
        let n = b.len();
        if autojunk && n >= 200 {
            let ntest = n / 100 + 1;
            let popular: Vec<&'a T> = b2j
                .iter()
                .filter(|(_, idxs)| idxs.len() > ntest)
                .map(|(k, _)| *k)
                .collect();
            for elt in popular {
                b2j.remove(elt);
            }
        }
        SequenceMatcher { a, b, b2j }
    }

    /// `find_longest_match(alo, ahi, blo, bhi)`; the junk set is empty (`isjunk=None`), so only
    /// the first two extension loops can run.
    fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ai) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(ai) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// `get_matching_blocks()`.
    pub(crate) fn get_matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut out = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    out.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            out.push((i1, j1, k1));
        }
        out.push((la, lb, 0));
        out
    }

    /// `get_opcodes()`.
    pub(crate) fn get_opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut answer = Vec::new();
        for (ai, bj, size) in self.get_matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                answer.push((tag, i, ai, j, bj));
            }
            (i, j) = (ai + size, bj + size);
            if size > 0 {
                answer.push(("equal", ai, i, bj, j));
            }
        }
        answer
    }

    /// `get_grouped_opcodes(n)`.
    pub(crate) fn get_grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.get_opcodes();
        if codes.is_empty() {
            codes.push(("equal", 0, 1, 0, 1));
        }
        if codes[0].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[0];
            codes[0] = (
                tag,
                i1.max(i2.saturating_sub(n)),
                i2,
                j1.max(j2.saturating_sub(n)),
                j2,
            );
        }
        let last = codes.len() - 1;
        if codes[last].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[last];
            codes[last] = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > nn {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == "equal") {
            groups.push(group);
        }
        groups
    }
}

/// `difflib._format_range_unified(start, stop)`.
fn format_range_unified(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

/// `list(difflib.unified_diff(a, b, fromfile, tofile, n=n, lineterm=""))`.
pub(crate) fn unified_diff(
    a: &[&str],
    b: &[&str],
    fromfile: &str,
    tofile: &str,
    n: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let sm = SequenceMatcher::new(a, b, true);
    for (g, group) in sm.get_grouped_opcodes(n).into_iter().enumerate() {
        if g == 0 {
            out.push(format!("--- {fromfile}"));
            out.push(format!("+++ {tofile}"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push(format!(
            "@@ -{} +{} @@",
            format_range_unified(first.1, last.2),
            format_range_unified(first.3, last.4)
        ));
        for (tag, i1, i2, j1, j2) in group {
            if tag == "equal" {
                out.extend(a[i1..i2].iter().map(|l| format!(" {l}")));
                continue;
            }
            if tag == "replace" || tag == "delete" {
                out.extend(a[i1..i2].iter().map(|l| format!("-{l}")));
            }
            if tag == "replace" || tag == "insert" {
                out.extend(b[j1..j2].iter().map(|l| format!("+{l}")));
            }
        }
    }
    out
}

/// `posixpath.join(a, b)` for two parts.
fn join(a: &[u8], b: &[u8]) -> Vec<u8> {
    if b.first() == Some(&b'/') {
        return b.to_vec();
    }
    let mut out = a.to_vec();
    if !out.is_empty() && !out.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(b);
    out
}

/// `posixpath.split(p)`.
fn split(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let i = p.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let (head, tail) = (&p[..i], &p[i..]);
    let mut head = head.to_vec();
    if !head.is_empty() && head.iter().any(|&c| c != b'/') {
        while head.last() == Some(&b'/') {
            head.pop();
        }
    }
    (head, tail.to_vec())
}

/// `posixpath.dirname(p)`.
pub(crate) fn dirname(p: &str) -> String {
    let (head, _) = split(p.as_bytes());
    String::from_utf8_lossy(&head).into_owned()
}

/// `posixpath.normpath(p)`.
pub(crate) fn normpath_bytes(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let initial = if p.starts_with(b"//") && !p.starts_with(b"///") {
        2
    } else if p.starts_with(b"/") {
        1
    } else {
        0
    };
    let mut comps: Vec<&[u8]> = Vec::new();
    for comp in p.split(|&c| c == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp != b".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&&b".."[..])
        {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let mut out = vec![b'/'; initial];
    out.extend_from_slice(&comps.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `posixpath.abspath(p)`.
fn abspath(p: &[u8]) -> Vec<u8> {
    if p.first() == Some(&b'/') {
        return normpath_bytes(p);
    }
    // `os.getcwd()`: process state, not the environment (GC-18 is about variables). Only a
    // relative plugin root gets here, through `plugin_read_rule`; `validate_plugin_root`
    // refuses one before any rule is built.
    let cwd = std::env::current_dir()
        .map(|c| c.into_os_string().into_vec())
        .unwrap_or_else(|_| b"/".to_vec());
    normpath_bytes(&join(&cwd, p))
}

fn is_link(p: &[u8]) -> bool {
    std::fs::symlink_metadata(OsStr::from_bytes(p)).is_ok_and(|m| m.file_type().is_symlink())
}

/// `posixpath._joinrealpath(path, rest, strict=False, seen)`.
fn join_realpath(
    mut path: Vec<u8>,
    rest: &[u8],
    seen: &mut HashMap<Vec<u8>, Option<Vec<u8>>>,
) -> (Vec<u8>, bool) {
    let mut rest: Vec<u8> = rest.to_vec();
    if rest.first() == Some(&b'/') {
        rest.remove(0);
        path = b"/".to_vec();
    }
    while !rest.is_empty() {
        let (name, tail) = match rest.iter().position(|&c| c == b'/') {
            Some(i) => (rest[..i].to_vec(), rest[i + 1..].to_vec()),
            None => (std::mem::take(&mut rest), Vec::new()),
        };
        rest = tail;
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            if path.is_empty() {
                path = b"..".to_vec();
            } else {
                let (head, n) = split(&path);
                path = if n == b".." {
                    join(&join(&head, b".."), b"..")
                } else {
                    head
                };
            }
            continue;
        }
        let newpath = join(&path, &name);
        if !is_link(&newpath) {
            path = newpath;
            continue;
        }
        if let Some(cached) = seen.get(&newpath) {
            match cached {
                Some(p) => {
                    path = p.clone();
                    continue;
                }
                None => return (join(&newpath, &rest), false),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = match std::fs::read_link(OsStr::from_bytes(&newpath)) {
            Ok(t) => t.into_os_string().into_vec(),
            // Python's os.readlink would raise; lstat just said it is a link, so this is a
            // race. Treat the component as a plain name.
            Err(_) => {
                path = newpath;
                continue;
            }
        };
        let (p, ok) = join_realpath(path, &target, seen);
        if !ok {
            return (join(&p, &rest), false);
        }
        seen.insert(newpath, Some(p.clone()));
        path = p;
    }
    (path, true)
}

/// `os.path.realpath(p)` (`strict=False`), on the path's bytes.
pub(crate) fn realpath_os(p: &OsStr) -> OsString {
    let (path, _) = join_realpath(Vec::new(), p.as_bytes(), &mut HashMap::new());
    OsString::from_vec(abspath(&path))
}

/// `os.path.realpath(p)` as text (non-UTF-8 bytes become U+FFFD, F-19).
pub(crate) fn realpath(p: &str) -> String {
    realpath_os(OsStr::new(p)).to_string_lossy().into_owned()
}

/// `repr(s)`.
pub(crate) fn repr_str(s: &str) -> String {
    PyExc::KeyError(s.to_string()).py_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(
            splitlines("a\r\nb\rc\u{2028}d\u{85}e\u{1e}f\u{b}"),
            vec!["a", "b", "c", "d", "e", "f"]
        );
        assert_eq!(splitlines("\n\n"), vec!["", ""]);
        assert_eq!(splitlines("x\u{1f}y"), vec!["x\u{1f}y"]);
    }

    #[test]
    fn unified_diff_has_pythons_form() {
        let a = ["{", "  \"a\": 1", "}"];
        let b = ["{", "  \"a\": 1,", "  \"b\": 2", "}"];
        assert_eq!(
            unified_diff(&a, &b, "settings.json", "settings.json", 0),
            vec![
                "--- settings.json",
                "+++ settings.json",
                "@@ -2 +2,2 @@",
                "-  \"a\": 1",
                "+  \"a\": 1,",
                "+  \"b\": 2",
            ]
        );
        assert!(unified_diff(&a, &a, "x", "x", 0).is_empty());
        assert_eq!(
            unified_diff(&[], &["x"], "f", "f", 0),
            vec!["--- f", "+++ f", "@@ -0,0 +1 @@", "+x"]
        );
    }

    #[test]
    fn normpath_and_dirname_match_posixpath() {
        let norm = |p: &str| String::from_utf8(normpath_bytes(p.as_bytes())).unwrap();
        assert_eq!(norm("//a//b/./c/.."), "//a/b");
        assert_eq!(norm("///a"), "/a");
        assert_eq!(norm("a/../../b"), "../b");
        assert_eq!(norm(""), ".");
        assert_eq!(dirname("/a/b/"), "/a/b");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("/"), "/");
        assert_eq!(dirname("//a"), "//");
        assert_eq!(dirname("a"), "");
    }

    #[test]
    fn realpath_resolves_links_and_keeps_missing_parts() {
        let tmp = tempfile::tempdir().unwrap();
        let base = realpath(tmp.path().to_str().unwrap());
        std::fs::create_dir(format!("{base}/real")).unwrap();
        std::os::unix::fs::symlink(format!("{base}/real"), format!("{base}/link")).unwrap();
        std::os::unix::fs::symlink("loop", format!("{base}/loop")).unwrap();
        assert_eq!(
            realpath(&format!("{base}/link/x/../y")),
            format!("{base}/real/y")
        );
        assert_eq!(
            realpath(&format!("{base}/missing/a")),
            format!("{base}/missing/a")
        );
        assert_eq!(
            realpath(&format!("{base}/loop/z")),
            format!("{base}/loop/z")
        );
        assert_eq!(realpath("/"), "/");
    }
}
