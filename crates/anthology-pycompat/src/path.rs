//! `pycompat::path`: `posixpath.normpath`, `posixpath.join` and `os.path.realpath` of CPython
//! 3.12 (design §12). Paths are bytes underneath, as on POSIX; the `&str` forms are the same
//! algorithm on UTF-8.

use std::ffi::OsStr;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// `posixpath.normpath(p)`: `""` is `"."`; exactly two leading slashes are kept, one or three
/// or more become one; `.` and empty parts go; `..` removes the part before it unless that is
/// `..` too or there is nothing before a relative path's start (it stays), and above `/` it
/// goes.
pub fn normpath(p: &str) -> String {
    String::from_utf8(normpath_bytes(p.as_bytes()))
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// [`normpath`] on bytes.
pub fn normpath_bytes(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let (initial, rest): (&[u8], &[u8]) = if p[0] != b'/' {
        (b"", p)
    } else if p.get(1) != Some(&b'/') || p.get(2) == Some(&b'/') {
        (b"/", &p[1..])
    } else {
        (b"//", &p[2..])
    };
    let mut comps: Vec<&[u8]> = Vec::new();
    for comp in rest.split(|&c| c == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp != b".."
            || (initial.is_empty() && comps.is_empty())
            || comps.last() == Some(&&b".."[..])
        {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let mut out = initial.to_vec();
    out.extend(comps.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `posixpath.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    String::from_utf8(join_bytes(a.as_bytes(), b.as_bytes()))
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn join_bytes(a: &[u8], b: &[u8]) -> Vec<u8> {
    if b.first() == Some(&b'/') {
        b.to_vec()
    } else if a.is_empty() || a.last() == Some(&b'/') {
        [a, b].concat()
    } else {
        [a, b"/", b].concat()
    }
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

/// `os.path.realpath(p)` (`strict=False`): symbolic links resolved part by part from the left;
/// a part that does not exist (or cannot be examined) is kept as it is; a symlink loop leaves
/// the rest of the path unresolved, as 3.12 does. A relative `p` is resolved against the
/// current directory (`os.getcwd()`). A link whose target cannot be read is kept as a plain
/// part (Python would raise there).
pub fn realpath(p: &Path) -> PathBuf {
    let mut seen = std::collections::HashMap::new();
    let (path, _) = join_realpath(Vec::new(), p.as_os_str().as_bytes(), &mut seen);
    PathBuf::from(std::ffi::OsString::from_vec(abspath(&path)))
}

/// `posixpath.abspath`.
fn abspath(p: &[u8]) -> Vec<u8> {
    if p.first() == Some(&b'/') {
        return normpath_bytes(p);
    }
    let cwd = std::env::current_dir()
        .map(|d| d.into_os_string().into_vec())
        .unwrap_or_default();
    normpath_bytes(&join_bytes(&cwd, p))
}

/// `posixpath._joinrealpath(path, rest, strict=False, seen)`.
fn join_realpath(
    mut path: Vec<u8>,
    rest: &[u8],
    seen: &mut std::collections::HashMap<Vec<u8>, Option<Vec<u8>>>,
) -> (Vec<u8>, bool) {
    let mut rest = rest;
    if rest.first() == Some(&b'/') {
        rest = &rest[1..];
        path = b"/".to_vec();
    }
    while !rest.is_empty() {
        let (name, tail) = match rest.iter().position(|&c| c == b'/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &b""[..]),
        };
        rest = tail;
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            if path.is_empty() {
                path = b"..".to_vec();
            } else {
                let (head, last) = split(&path);
                path = head;
                if last == b".." {
                    path = join_bytes(&join_bytes(&path, b".."), b"..");
                }
            }
            continue;
        }
        let newpath = join_bytes(&path, name);
        let is_link = std::fs::symlink_metadata(OsStr::from_bytes(&newpath))
            .is_ok_and(|m| m.file_type().is_symlink());
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(cached) = seen.get(&newpath) {
            match cached {
                Some(resolved) => {
                    path = resolved.clone();
                    continue;
                }
                // A symlink loop: the resolved part, then the rest unchanged.
                None => return (join_bytes(&newpath, rest), false),
            }
        }
        let Ok(target) = std::fs::read_link(OsStr::from_bytes(&newpath)) else {
            path = newpath;
            continue;
        };
        seen.insert(newpath.clone(), None);
        let (resolved, ok) = join_realpath(path, target.as_os_str().as_bytes(), seen);
        if !ok {
            return (join_bytes(&resolved, rest), false);
        }
        seen.insert(newpath, Some(resolved.clone()));
        path = resolved;
    }
    (path, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normpath_is_posixpaths() {
        for (p, want) in [
            ("", "."),
            ("/", "/"),
            ("//", "//"),
            ("///", "/"),
            ("//a/../b", "//b"),
            ("a/../..", ".."),
            ("/../a", "/a"),
            ("./a/./b/", "a/b"),
            ("a//b", "a/b"),
            ("../../a/..", "../.."),
        ] {
            assert_eq!(normpath(p), want, "{p:?}");
        }
    }

    #[test]
    fn join_is_posixpaths() {
        assert_eq!(join("a", "b"), "a/b");
        assert_eq!(join("a/", "b"), "a/b");
        assert_eq!(join("", "b"), "b");
        assert_eq!(join("a", "/b"), "/b");
        assert_eq!(join("a", ""), "a/");
    }
}
