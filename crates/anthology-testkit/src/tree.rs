//! `snapshot`, `Tree` (F4, plan §3.7): a folder as data, to compare and to print.
//!
//! [`snapshot`] walks a folder without following symlinks. Every entry below it becomes one
//! [`TreeEntry`], keyed by its path relative to the folder, `/`-separated (the folder itself is not
//! an entry). Keys are sorted. A symlink target that starts with the folder's path (as given, or
//! its realpath) is rewritten to start with `<ROOT>`, so trees from two sandboxes compare equal.
//! A name that is not UTF-8 panics (two such names would otherwise collide as text keys).
//!
//! The JSON form ([`Tree::to_json`], [`Tree::from_json`]) is what a difftest oracle returns
//! under `"files"`:
//!
//! ```text
//! {"a/b.txt": {"type": "file", "mode": "0644", "text": "..."},   // UTF-8 content
//!  "a/c.bin": {"type": "file", "mode": "0600", "hex": "00ff"},     // any other content
//!  "a":       {"type": "dir", "mode": "0755"},
//!  "l":       {"type": "symlink", "target": "<ROOT>/a"}}
//! ```
//!
//! `mode` is the permission bits in octal (`st_mode & 0o7777`), as Python's `"%04o"` prints
//! them. `to_json` writes `text` when the bytes are UTF-8, else `hex`; `from_json` takes either.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anthology_pycompat::json::{Map, Value};

/// What [`snapshot`] puts in place of the snapshotted folder in symlink targets.
pub const ROOT_MARK: &str = "<ROOT>";

/// One entry of a [`Tree`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeEntry {
    /// A regular file: its bytes and permission bits.
    File { bytes: Vec<u8>, mode: u32 },
    /// A folder and its permission bits.
    Dir { mode: u32 },
    /// A symlink and its target (`<ROOT>`-normalized by [`snapshot`]).
    Symlink { target: String },
    /// Anything else (a FIFO, a socket, a device), by its kind.
    Other { kind: String },
}

/// A folder's entries by relative path, sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree(pub BTreeMap<String, TreeEntry>);

/// Every entry under `dir`, never following symlinks. Panics when `dir` cannot be read.
pub fn snapshot(dir: &Path) -> Tree {
    let mut roots = vec![dir.as_os_str().as_bytes().to_vec()];
    if let Ok(real) = fs::canonicalize(dir) {
        let real = real.as_os_str().as_bytes().to_vec();
        if !roots.contains(&real) {
            roots.push(real);
        }
    }
    let mut tree = Tree::default();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, rel)) = stack.pop() {
        let entries = fs::read_dir(&path)
            .unwrap_or_else(|e| panic!("snapshot: cannot list {}: {e}", path.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("snapshot: {}: {e}", path.display()));
            let name = entry.file_name().into_string().unwrap_or_else(|n| {
                panic!(
                    "snapshot: a name that is not UTF-8 under {}: {n:?}",
                    path.display()
                )
            });
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let full = entry.path();
            let meta = fs::symlink_metadata(&full)
                .unwrap_or_else(|e| panic!("snapshot: {}: {e}", full.display()));
            let mode = meta.permissions().mode() & 0o7777;
            let kind = meta.file_type();
            let item = if kind.is_symlink() {
                let target = fs::read_link(&full)
                    .unwrap_or_else(|e| panic!("snapshot: {}: {e}", full.display()));
                TreeEntry::Symlink {
                    target: normalize_target(target.as_os_str().as_bytes(), &roots),
                }
            } else if kind.is_dir() {
                stack.push((full, rel.clone()));
                TreeEntry::Dir { mode }
            } else if kind.is_file() {
                let bytes =
                    fs::read(&full).unwrap_or_else(|e| panic!("snapshot: {}: {e}", full.display()));
                TreeEntry::File { bytes, mode }
            } else {
                TreeEntry::Other {
                    kind: other_kind(&kind),
                }
            };
            tree.0.insert(rel, item);
        }
    }
    tree
}

fn other_kind(kind: &fs::FileType) -> String {
    use std::os::unix::fs::FileTypeExt;
    if kind.is_fifo() {
        "fifo"
    } else if kind.is_socket() {
        "socket"
    } else if kind.is_block_device() {
        "block device"
    } else if kind.is_char_device() {
        "char device"
    } else {
        "unknown"
    }
    .to_string()
}

fn normalize_target(target: &[u8], roots: &[Vec<u8>]) -> String {
    for root in roots {
        if let Some(rest) = target.strip_prefix(root.as_slice())
            && (rest.is_empty() || rest.starts_with(b"/"))
        {
            return format!("{ROOT_MARK}{}", String::from_utf8_lossy(rest));
        }
    }
    String::from_utf8_lossy(target).into_owned()
}

impl Tree {
    /// The entry at `path`, if any.
    pub fn get(&self, path: &str) -> Option<&TreeEntry> {
        self.0.get(path)
    }

    /// The bytes of the file at `path`, if it is a file.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        match self.0.get(path) {
            Some(TreeEntry::File { bytes, .. }) => Some(bytes),
            _ => None,
        }
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Paths and entries, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &TreeEntry)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// This tree without `prefix` and everything below it (for example `.git`).
    pub fn without(mut self, prefix: &str) -> Tree {
        let below = format!("{prefix}/");
        self.0.retain(|k, _| k != prefix && !k.starts_with(&below));
        self
    }

    /// The differences from `self` to `other`, one readable line each; empty when equal.
    ///
    /// `- path: …` is only in `self`, `+ path: …` only in `other`, `~ path: …` differs.
    pub fn diff(&self, other: &Tree) -> Vec<String> {
        let mut out = Vec::new();
        for (path, a) in &self.0 {
            match other.0.get(path) {
                None => out.push(format!("- {path}: {}", describe(a))),
                Some(b) if a != b => out.push(format!("~ {path}: {}", change(a, b))),
                Some(_) => {}
            }
        }
        for (path, b) in &other.0 {
            if !self.0.contains_key(path) {
                out.push(format!("+ {path}: {}", describe(b)));
            }
        }
        out
    }

    /// The JSON form (module docs).
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (path, entry) in &self.0 {
            let mut e = Map::new();
            match entry {
                TreeEntry::File { bytes, mode } => {
                    e.insert("type".into(), Value::Str("file".into()));
                    e.insert("mode".into(), Value::Str(format!("{mode:04o}")));
                    match std::str::from_utf8(bytes) {
                        Ok(text) => e.insert("text".into(), Value::Str(text.into())),
                        Err(_) => e.insert("hex".into(), Value::Str(hex(bytes))),
                    }
                }
                TreeEntry::Dir { mode } => {
                    e.insert("type".into(), Value::Str("dir".into()));
                    e.insert("mode".into(), Value::Str(format!("{mode:04o}")));
                }
                TreeEntry::Symlink { target } => {
                    e.insert("type".into(), Value::Str("symlink".into()));
                    e.insert("target".into(), Value::Str(target.clone()));
                }
                TreeEntry::Other { kind } => {
                    e.insert("type".into(), Value::Str("other".into()));
                    e.insert("kind".into(), Value::Str(kind.clone()));
                }
            }
            m.insert(path.clone(), Value::Object(e));
        }
        Value::Object(m)
    }

    /// Reads the JSON form (module docs). Unknown types or keys are errors.
    pub fn from_json(v: &Value) -> Result<Tree, String> {
        let obj = v.as_object().ok_or("files: not an object")?;
        let mut tree = Tree::default();
        for (path, e) in obj.iter() {
            let e = e
                .as_object()
                .ok_or_else(|| format!("files[{path:?}]: not an object"))?;
            let field = |k: &str| e.get(k).and_then(Value::as_str);
            let allowed: &[&str] = match field("type") {
                Some("file") => &["type", "mode", "text", "hex"],
                Some("dir") => &["type", "mode"],
                Some("symlink") => &["type", "target"],
                Some("other") => &["type", "kind"],
                other => return Err(format!("files[{path:?}]: unknown type {other:?}")),
            };
            if let Some((k, _)) = e.iter().find(|(k, _)| !allowed.contains(k)) {
                return Err(format!("files[{path:?}]: unknown key {k:?}"));
            }
            let mode = || -> Result<u32, String> {
                let m = field("mode").ok_or_else(|| format!("files[{path:?}]: no mode"))?;
                u32::from_str_radix(m, 8).map_err(|_| format!("files[{path:?}]: bad mode {m:?}"))
            };
            let entry = match field("type") {
                Some("file") => {
                    let bytes = match (field("text"), field("hex")) {
                        (Some(t), None) => t.as_bytes().to_vec(),
                        (None, Some(h)) => {
                            unhex(h).ok_or_else(|| format!("files[{path:?}]: bad hex"))?
                        }
                        _ => return Err(format!("files[{path:?}]: needs one of text or hex")),
                    };
                    TreeEntry::File {
                        bytes,
                        mode: mode()?,
                    }
                }
                Some("dir") => TreeEntry::Dir { mode: mode()? },
                Some("symlink") => TreeEntry::Symlink {
                    target: field("target")
                        .ok_or_else(|| format!("files[{path:?}]: no target"))?
                        .to_string(),
                },
                _ => TreeEntry::Other {
                    kind: field("kind").unwrap_or("unknown").to_string(),
                },
            };
            tree.0.insert(path.to_string(), entry);
        }
        Ok(tree)
    }
}

fn describe(e: &TreeEntry) -> String {
    match e {
        TreeEntry::File { bytes, mode } => format!("file {mode:04o}, {} bytes", bytes.len()),
        TreeEntry::Dir { mode } => format!("dir {mode:04o}"),
        TreeEntry::Symlink { target } => format!("symlink -> {target}"),
        TreeEntry::Other { kind } => kind.clone(),
    }
}

fn change(a: &TreeEntry, b: &TreeEntry) -> String {
    match (a, b) {
        (TreeEntry::File { bytes: x, mode: mx }, TreeEntry::File { bytes: y, mode: my }) => {
            let mut parts = Vec::new();
            if mx != my {
                parts.push(format!("mode {mx:04o} -> {my:04o}"));
            }
            if x != y {
                let at = x
                    .iter()
                    .zip(y)
                    .position(|(p, q)| p != q)
                    .unwrap_or(x.len().min(y.len()));
                parts.push(format!(
                    "content differs at byte {at} ({} -> {} bytes): {} vs {}",
                    x.len(),
                    y.len(),
                    excerpt(x, at),
                    excerpt(y, at)
                ));
            }
            parts.join("; ")
        }
        _ => format!("{} -> {}", describe(a), describe(b)),
    }
}

fn excerpt(b: &[u8], at: usize) -> String {
    let start = at.saturating_sub(16);
    let end = (at + 24).min(b.len());
    format!("{:?}", String::from_utf8_lossy(&b[start..end]))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json::{PY_DEFAULT, dumps, loads};
    use std::os::unix::fs::symlink;

    fn sample() -> (tempfile::TempDir, Tree) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("a/b")).expect("mkdir");
        fs::write(root.join("a/b/t.txt"), "hé\n").expect("write");
        fs::write(root.join("bin"), [0u8, 0xff, 7]).expect("write");
        fs::set_permissions(root.join("bin"), fs::Permissions::from_mode(0o600)).expect("chmod");
        fs::set_permissions(root.join("a"), fs::Permissions::from_mode(0o750)).expect("chmod");
        symlink(root.join("a/b"), root.join("abs")).expect("symlink");
        symlink("a/b/t.txt", root.join("rel")).expect("symlink");
        symlink("/elsewhere/x", root.join("out")).expect("symlink");
        let tree = snapshot(root);
        (dir, tree)
    }

    #[test]
    fn snapshot_lists_every_entry_sorted_without_following_links() {
        let (_dir, tree) = sample();
        let paths: Vec<&str> = tree.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["a", "a/b", "a/b/t.txt", "abs", "bin", "out", "rel"]);
        assert_eq!(tree.get("a"), Some(&TreeEntry::Dir { mode: 0o750 }));
        assert_eq!(tree.file("a/b/t.txt"), Some("hé\n".as_bytes()));
        assert_eq!(
            tree.get("bin"),
            Some(&TreeEntry::File {
                bytes: vec![0, 0xff, 7],
                mode: 0o600
            })
        );
        assert_eq!(
            tree.get("abs"),
            Some(&TreeEntry::Symlink {
                target: "<ROOT>/a/b".into()
            })
        );
        assert_eq!(
            tree.get("rel"),
            Some(&TreeEntry::Symlink {
                target: "a/b/t.txt".into()
            })
        );
        assert_eq!(
            tree.get("out"),
            Some(&TreeEntry::Symlink {
                target: "/elsewhere/x".into()
            })
        );
    }

    #[test]
    fn the_root_mark_also_replaces_the_realpath_and_only_whole_components() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = fs::canonicalize(dir.path()).expect("realpath");
        symlink(real.join("x"), dir.path().join("l1")).expect("symlink");
        let sibling = format!("{}2/x", real.display());
        symlink(&sibling, dir.path().join("l2")).expect("symlink");
        let tree = snapshot(dir.path());
        assert_eq!(
            tree.get("l1"),
            Some(&TreeEntry::Symlink {
                target: "<ROOT>/x".into()
            })
        );
        assert_eq!(
            tree.get("l2"),
            Some(&TreeEntry::Symlink { target: sibling })
        );
    }

    #[test]
    fn snapshots_of_two_folders_with_the_same_content_are_equal() {
        let (_a, one) = sample();
        let (_b, two) = sample();
        assert_eq!(one, two);
        assert!(one.diff(&two).is_empty());
    }

    #[test]
    fn diff_is_readable() {
        let (_dir, a) = sample();
        let mut b = a.clone();
        b.0.remove("rel");
        b.0.insert("new".into(), TreeEntry::Dir { mode: 0o700 });
        b.0.insert(
            "a/b/t.txt".into(),
            TreeEntry::File {
                bytes: b"hX\n".to_vec(),
                mode: 0o644,
            },
        );
        b.0.insert("a".into(), TreeEntry::Symlink { target: "x".into() });
        let lines = a.diff(&b);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(lines[0], "~ a: dir 0750 -> symlink -> x");
        assert!(lines[1].starts_with("~ a/b/t.txt: "), "{}", lines[1]);
        assert!(
            lines[1].contains("content differs at byte 1 (4 -> 3 bytes)"),
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "- rel: symlink -> a/b/t.txt");
        assert_eq!(lines[3], "+ new: dir 0700");
    }

    #[test]
    fn json_round_trips_and_picks_text_or_hex() {
        let (_dir, tree) = sample();
        let v = tree.to_json();
        let text = dumps(&v, &PY_DEFAULT);
        assert!(text.contains(r#""bin": {"type": "file", "mode": "0600", "hex": "00ff07"}"#));
        assert!(
            text.contains(r#""a/b/t.txt": {"type": "file", "mode": "0644", "text": "h\u00e9\n"}"#)
        );
        let back = Tree::from_json(&loads(&text).expect("json")).expect("tree");
        assert_eq!(back, tree);
    }

    #[test]
    fn from_json_is_strict() {
        for (doc, want) in [
            (r#"[]"#, "not an object"),
            (r#"{"a": {"type": "pipe"}}"#, "unknown type"),
            (
                r#"{"a": {"type": "dir", "mode": "0755", "x": 1}}"#,
                "unknown key \"x\"",
            ),
            (r#"{"a": {"type": "dir"}}"#, "no mode"),
            (r#"{"a": {"type": "dir", "mode": "9"}}"#, "bad mode"),
            (
                r#"{"a": {"type": "file", "mode": "0644"}}"#,
                "needs one of text or hex",
            ),
            (
                r#"{"a": {"type": "file", "mode": "0644", "hex": "0"}}"#,
                "bad hex",
            ),
            (r#"{"a": {"type": "symlink"}}"#, "no target"),
        ] {
            let err = Tree::from_json(&loads(doc).expect("json")).expect_err(doc);
            assert!(err.contains(want), "{doc}: {err}");
        }
    }

    #[test]
    fn without_drops_a_subtree() {
        let (_dir, tree) = sample();
        let t = tree.without("a");
        let paths: Vec<&str> = t.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["abs", "bin", "out", "rel"]);
        assert_eq!(t.len(), 4);
        assert!(!t.is_empty());
    }
}
