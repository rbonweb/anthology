//! Trees: copying, walking, snapshotting and comparing (vendor.py "tree helpers").

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::manifest::IGNORED_EXTRAS;
use super::py;
use super::{Result, VendorError};

/// An OS error as `VendorError`, with Python's `[Errno N] text: 'path'` form.
pub fn os_err(e: &std::io::Error, path: &Path) -> VendorError {
    VendorError(py::os_error_text(e, path))
}

/// `p.relative_to(base).as_posix()`.
pub fn rel_posix(p: &Path, base: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// `p.relative_to(base).as_posix()` as raw OS bytes: a key that keeps distinct non-UTF-8 names
/// distinct and sorts as Python's surrogate-escaped strings do (by raw bytes, controller
/// ruling (b)).
pub fn rel_bytes(p: &Path, base: &Path) -> Vec<u8> {
    p.strip_prefix(base)
        .unwrap_or(p)
        .as_os_str()
        .as_encoded_bytes()
        .to_vec()
}

/// A [`rel_bytes`] key as text, for messages.
pub fn show(rel: &[u8]) -> String {
    String::from_utf8_lossy(rel).into_owned()
}

/// `Path.is_symlink()`.
pub fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// `Path.is_file()` (follows symlinks).
pub fn is_file(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// `Path.is_dir()` (follows symlinks).
pub fn is_dir(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// `Path.exists()` (follows symlinks).
pub fn exists(p: &Path) -> bool {
    fs::metadata(p).is_ok()
}

/// `_norm_mode(path)`: 755 when any execute bit is set (following symlinks), else 644.
pub fn norm_mode(path: &Path) -> Result<u32> {
    let mode = fs::metadata(path)
        .map_err(|e| os_err(&e, path))?
        .permissions()
        .mode();
    Ok(if mode & 0o111 != 0 { 0o755 } else { 0o644 })
}

/// One directory of an `os.walk`: `dirs` may be pruned or reordered by the visitor, as
/// `dirnames[:] = …` does.
pub struct WalkDir {
    pub path: PathBuf,
    pub dirs: Vec<OsString>,
    pub files: Vec<OsString>,
}

/// `os.walk(top)` (top-down, `followlinks=False`): a directory that cannot be listed is
/// skipped, an entry is a dir when `is_dir()` (following symlinks) says so, and a symlinked
/// directory is listed in `dirs` but not descended into.
pub fn os_walk(top: &Path, mut visit: impl FnMut(&mut WalkDir) -> Result<()>) -> Result<()> {
    let mut stack = vec![top.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        let mut w = WalkDir {
            path: dir,
            dirs: Vec::new(),
            files: Vec::new(),
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            if is_dir(&w.path.join(&name)) {
                w.dirs.push(name);
            } else {
                w.files.push(name);
            }
        }
        visit(&mut w)?;
        for d in w.dirs.iter().rev() {
            let sub = w.path.join(d);
            if !is_symlink(&sub) {
                stack.push(sub);
            }
        }
    }
    Ok(())
}

/// `sorted(names)` (byte order is code point order for UTF-8 names).
pub fn sort_names(names: &mut [OsString]) {
    names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
}

/// `_reject_dir_symlinks`: os.walk does not descend into symlinks to directories, so they
/// would be dropped silently (and `check` could not notice). They are not supported.
pub fn reject_dir_symlinks(dirpath: &Path, dirnames: &[OsString], base: &Path) -> Result<()> {
    for d in dirnames {
        let p = dirpath.join(d);
        if is_symlink(&p) {
            return Err(VendorError(format!(
                "{}: symlinks to directories are not supported (not followed, not copied); \
                 remove it or handle it with a transform",
                rel_posix(&p, base)
            )));
        }
    }
    Ok(())
}

/// `tree.rglob("*")`, sorted as `sorted()` sorts paths (by components): every entry below
/// `tree`, dot files included, without descending into symlinked directories.
pub fn rglob(tree: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![tree.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let p = entry.path();
            if is_dir(&p) && !is_symlink(&p) {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

/// `copy_tree(src, dst)`: copy without any `.git` directory; modes normalized to 644/755;
/// file symlinks are recreated, directory symlinks refused.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|e| os_err(&e, dst))?;
    os_walk(src, |w| {
        w.dirs.retain(|d| d != OsStr::new(".git"));
        sort_names(&mut w.dirs);
        reject_dir_symlinks(&w.path, &w.dirs, src)?;
        let rel = w.path.strip_prefix(src).unwrap_or(Path::new(""));
        let here = dst.join(rel);
        fs::create_dir_all(&here).map_err(|e| os_err(&e, &here))?;
        sort_names(&mut w.files);
        for name in &w.files {
            let (s, d) = (w.path.join(name), here.join(name));
            if is_symlink(&s) {
                let target = fs::read_link(&s).map_err(|e| os_err(&e, &s))?;
                std::os::unix::fs::symlink(&target, &d).map_err(|e| os_err(&e, &d))?;
            } else {
                copy_file(&s, &d)?;
                let mode = norm_mode(&s)?;
                fs::set_permissions(&d, fs::Permissions::from_mode(mode))
                    .map_err(|e| os_err(&e, &d))?;
            }
        }
        Ok(())
    })
}

/// `shutil.copyfile(s, d)`: the bytes only; a named pipe is refused as Python refuses it.
fn copy_file(s: &Path, d: &Path) -> Result<()> {
    let meta = fs::metadata(s).map_err(|e| os_err(&e, s))?;
    if !meta.is_file() {
        return Err(VendorError(format!(
            "`{}` is not a regular file (SpecialFileError)",
            s.display()
        )));
    }
    let data = fs::read(s).map_err(|e| os_err(&e, s))?;
    fs::write(d, data).map_err(|e| os_err(&e, d))
}

/// One entry of a [`snapshot`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snap {
    Link(OsString),
    File { exec: bool, data: Vec<u8> },
}

/// `snapshot(tree)`: rel path → link target, or exec bit and bytes. Keys are the rel paths' raw
/// bytes ([`rel_bytes`]), so the map sorts as Python sorts the keys.
pub fn snapshot(tree: &Path) -> Result<BTreeMap<Vec<u8>, Snap>> {
    let mut out = BTreeMap::new();
    os_walk(tree, |w| {
        sort_names(&mut w.dirs);
        reject_dir_symlinks(&w.path, &w.dirs, tree)?;
        for name in &w.files {
            let p = w.path.join(name);
            let rel = rel_bytes(&p, tree);
            if is_symlink(&p) {
                let target = fs::read_link(&p).map_err(|e| os_err(&e, &p))?;
                out.insert(rel, Snap::Link(target.into_os_string()));
            } else {
                let meta = fs::metadata(&p).map_err(|e| os_err(&e, &p))?;
                let data = fs::read(&p).map_err(|e| os_err(&e, &p))?;
                out.insert(
                    rel,
                    Snap::File {
                        exec: meta.permissions().mode() & 0o111 != 0,
                        data,
                    },
                );
            }
        }
        Ok(())
    })?;
    Ok(out)
}

/// `compare_trees(expected, actual, label)`: one line per difference.
pub fn compare_trees(expected: &Path, actual: &Path, label: &str) -> Result<Vec<String>> {
    if !is_dir(actual) {
        return Ok(vec![format!("{label}: committed target is missing")]);
    }
    let want = snapshot(expected)?;
    let have = snapshot(actual)?;
    let mut keys: Vec<&Vec<u8>> = want.keys().chain(have.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut problems = Vec::new();
    for key in keys {
        let rel = show(key);
        match (want.get(key), have.get(key)) {
            (_, None) => problems.push(format!("{label}: missing from the committed tree: {rel}")),
            (None, Some(_)) => {
                if !IGNORED_EXTRAS.contains(&py::basename(&rel)) {
                    problems.push(format!("{label}: extra file not in the build: {rel}"));
                }
            }
            (Some(Snap::Link(a)), Some(Snap::Link(b))) => {
                if a != b {
                    problems.push(format!("{label}: {rel} differs (file type or link target)"));
                }
            }
            (Some(Snap::File { exec: we, data: wd }), Some(Snap::File { exec: he, data: hd })) => {
                if wd != hd {
                    problems.push(format!("{label}: {rel} differs from the build"));
                } else if we != he {
                    let bit = if *we { "set" } else { "clear" };
                    problems.push(format!(
                        "{label}: {rel} mode differs (executable bit {bit} in the build)"
                    ));
                }
            }
            _ => problems.push(format!("{label}: {rel} differs (file type or link target)")),
        }
    }
    Ok(problems)
}
