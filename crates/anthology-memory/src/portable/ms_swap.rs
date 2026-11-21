//! SWAP(MS1): stand-ins for the plan §3.6 items of MS1 that portable calls (`repo`, `approve`,
//! `notes`, `render`), while MS1 runs in parallel (wave 2/3 lead rules). Each module is the
//! reviewed MS1 code, copied from `origin/port-ms1` (`crates/anthology-memory/src/{repo,approve,
//! notes,render}.rs`) without its tests, and `repo` without the items portable does not use
//! (`legacy_dirs`, `encode_project_path`, `note_sources`, `unsafe_dir`). The swap is: delete
//! this file and its `mod` line, and point `portable.rs`'s `pub use` at
//! `crate::{approve, notes, render, repo}`. Portable needs nothing of MS2 (`memory_sync`'s
//! `privacy_findings` is `crate::privacy`, MP1).

pub mod repo {
    //! SWAP(MS1): `anthology_memory::repo`, copied from `origin/port-ms1` without `legacy_dirs`, `note_sources` and `unsafe_dir`. A port of `memory_sync`'s `repo_root`, `_main_root`,
    //! `normalize_remote`, `repo_key`, `legacy_dirs`, `note_sources` and `_unsafe_dir` (plan §3.6).
    //!
    //! Git runs through the [`Git`] seam (GC-19; `memory_sync._git`): `RealGit` is
    //! `git -c core.fsmonitor=false …` with the 10 s timeout and the `GIT_DIR` family removed.
    //! `_git_out` is [`git_out`]: exit 0 and non-empty stripped stdout, else `None`.
    //!
    //! Python reads `os.environ` in `expanduser` and `os.getcwd()`; here `expanduser` reads `env`
    //! (callers pass the process environment, as Python's callers do).

    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use anthology_core::Env;
    use anthology_core::git::Git;
    use anthology_pycompat::path::{normpath_bytes, realpath};
    use anthology_pycompat::re::{self, Regex};
    use anthology_pycompat::str::{lower, split_ws, strip_ws};

    /// `memory_sync.config_dir(env)` and `anthology_home(env)` (GC-1).
    pub use anthology_core::paths::{anthology_home, config_dir};

    /// `memory_sync._DEFAULT_PORTS`.
    const DEFAULT_PORTS: [&str; 3] = ["22", "443", "80"];

    /// `_git_out(root, *args)`: git's stdout (UTF-8 with replacement, stripped) when it exits 0
    /// and prints something; `None` otherwise or when git cannot run.
    pub fn git_out(git: &dyn Git, env: &Env, cwd: &Path, args: &[&str]) -> Option<String> {
        let out = git.run(env, cwd, args)?;
        if out.code != 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let text = strip_ws(&text);
        (!text.is_empty()).then(|| text.to_string())
    }

    /// `memory_sync.repo_root(cwd)`: the work tree root (realpath) containing `cwd`, or `None`
    /// when `cwd` is not in a git repo.
    pub fn repo_root(cwd: &Path, env: &Env, git: &dyn Git) -> Option<PathBuf> {
        let top = git_out(git, env, cwd, &["rev-parse", "--show-toplevel"])?;
        Some(realpath(Path::new(&top)))
    }

    /// `memory_sync._main_root(root)`: the main work tree's root, so linked worktrees map to the
    /// same place as the clone.
    pub fn main_root(root: &Path, env: &Env, git: &dyn Git) -> PathBuf {
        let common = git_out(
            git,
            env,
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map(String::into_bytes)
        .or_else(|| {
            git_out(git, env, root, &["rev-parse", "--git-common-dir"])
                .map(|rel| join_bytes(root.as_os_str().as_bytes(), rel.as_bytes()))
        });
        if let Some(common) = common {
            let norm = normpath_bytes(&common);
            if basename(&norm) == b".git" {
                return realpath(Path::new(OsStr::from_bytes(dirname(&norm))));
            }
        }
        match git_out(git, env, root, &["rev-parse", "--show-toplevel"]) {
            Some(top) => realpath(Path::new(&top)),
            None => realpath(root),
        }
    }

    /// `memory_sync.normalize_remote(url, base)`: a stable key for a git remote URL:
    /// `host[:port]/owner/repo` lowercased, credentials and default ports dropped, `.git` and
    /// trailing `/` removed; a local path remote becomes `path:<realpath>` (resolved against
    /// `base`, or the current folder when `None`; `~` expanded with `env`'s `HOME`).
    pub fn normalize_remote(url: &str, base: Option<&Path>, env: &Env) -> String {
        let u = strip_ws(url);
        let mut path: Option<&str> = None;
        let mut key = String::new();
        if lower(u).starts_with("file://") {
            path = Some(after_chars(u, 7));
        } else if let Some((_, rest)) = u.split_once("://") {
            let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
            let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
            let (mut host, mut port) = (authority, "");
            if authority.matches(':').count() == 1 {
                (host, port) = authority.split_once(':').expect("one colon");
            }
            if DEFAULT_PORTS.contains(&port) {
                port = "";
            }
            key = host.to_string();
            if !port.is_empty() {
                key.push(':');
                key.push_str(port);
            }
            key.push('/');
            key.push_str(tail);
        } else if ["/", "./", "../", "~"].iter().any(|p| u.starts_with(p)) {
            path = Some(u);
        } else {
            match scp_match(u) {
                Some((host, rest)) => key = format!("{host}/{rest}"),
                None => path = Some(u),
            }
        }
        if let Some(path) = path {
            let base: Vec<u8> = match base {
                Some(b) => b.as_os_str().as_bytes().to_vec(),
                None => std::env::current_dir()
                    .map(|d| d.into_os_string().into_vec())
                    .unwrap_or_default(),
            };
            let joined = join_bytes(&base, &expanduser(path, env));
            let full = realpath(Path::new(OsStr::from_bytes(&joined)));
            let mut full = full.into_os_string().into_vec();
            if full.ends_with(b".git") {
                full.truncate(full.len() - 4);
            }
            return format!("path:{}", String::from_utf8_lossy(&full));
        }
        let key = collapse_slashes(&key);
        let mut key = key.trim_end_matches('/');
        if let Some(k) = key.strip_suffix(".git") {
            key = k;
        }
        lower(key.trim_end_matches('/'))
    }

    /// `memory_sync.repo_key(root)`: the repo's normalised remote URL (`origin`, else the first
    /// remote by name), else `path:<realpath of the main work tree>`.
    pub fn repo_key(root: &Path, env: &Env, git: &dyn Git) -> String {
        let names = git_out(git, env, root, &["remote"])
            .map(|out| {
                let mut names: Vec<String> = split_ws(&out).into_iter().map(String::from).collect();
                names.sort();
                names
            })
            .unwrap_or_default();
        if let Some(first) = names.first() {
            let name = if names.iter().any(|n| n == "origin") {
                "origin"
            } else {
                first.as_str()
            };
            let config_key = format!("remote.{name}.url");
            let url = git_out(git, env, root, &["remote", "get-url", name])
                .or_else(|| git_out(git, env, root, &["config", "--get", &config_key]));
            if let Some(url) = url {
                return normalize_remote(&url, Some(root), env);
            }
        }
        format!("path:{}", main_root(root, env, git).to_string_lossy())
    }

    // ------------------------------------------------------------------------------------------
    // Small helpers
    // ------------------------------------------------------------------------------------------

    /// `_SCP.match(u)` (`^(?:[^@/\s]+@)?([^:/\s]+):(.*)$`): the host and the rest.
    fn scp_match(u: &str) -> Option<(String, String)> {
        static RX: OnceLock<Regex> = OnceLock::new();
        let rx = RX.get_or_init(|| {
            re::compile(r"^(?:[^@/\s]+@)?([^:/\s]+):(.*)$", false).expect("_SCP compiles")
        });
        // `$` makes it fancy-regex; a backtrack limit is not reachable for this linear pattern,
        // and an error reads as no match (the URL is then treated as a path, as Python's
        // fallthrough does).
        let groups = rx.captures_at(u, 0).ok().flatten()?;
        let (h0, h1) = groups.get(1).copied().flatten()?;
        let (r0, r1) = groups.get(2).copied().flatten()?;
        Some((u[h0..h1].to_string(), u[r0..r1].to_string()))
    }

    /// `s[n:]` in code points.
    fn after_chars(s: &str, n: usize) -> &str {
        s.char_indices().nth(n).map_or("", |(i, _)| &s[i..])
    }

    /// `re.sub(r"/+", "/", key)`.
    fn collapse_slashes(key: &str) -> String {
        let mut out = String::with_capacity(key.len());
        let mut prev = false;
        for c in key.chars() {
            if c == '/' && prev {
                continue;
            }
            prev = c == '/';
            out.push(c);
        }
        out
    }

    /// `posixpath.join(a, b)`.
    fn join_bytes(a: &[u8], b: &[u8]) -> Vec<u8> {
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

    /// `posixpath.basename(p)`.
    fn basename(p: &[u8]) -> &[u8] {
        p.iter()
            .rposition(|&c| c == b'/')
            .map_or(p, |i| &p[i + 1..])
    }

    /// `posixpath.dirname(p)`.
    fn dirname(p: &[u8]) -> &[u8] {
        let i = p.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
        let head = &p[..i];
        if !head.is_empty() && head.iter().any(|&c| c != b'/') {
            let end = head.iter().rposition(|&c| c != b'/').map_or(0, |j| j + 1);
            &head[..end]
        } else {
            head
        }
    }

    /// `os.path.expanduser(path)` (CPython 3.12 `posixpath`) with `env`'s `HOME`: `~` and `~/…`
    /// use `HOME` (as given, even empty) or the passwd home; `~user` is left as it is (Rust has no
    /// passwd lookup by name; Python expands it when the user exists).
    fn expanduser(path: &str, env: &Env) -> Vec<u8> {
        let p = path.as_bytes();
        if !path.starts_with('~') {
            return p.to_vec();
        }
        let i = p
            .iter()
            .skip(1)
            .position(|&c| c == b'/')
            .map_or(p.len(), |j| j + 1);
        if i != 1 {
            return p.to_vec();
        }
        let home: OsString = match env.get_os("HOME") {
            Some(h) => h.to_os_string(),
            None => match anthology_core::sys::passwd_home() {
                Some(h) => h,
                None => return p.to_vec(),
            },
        };
        let home = home.as_bytes();
        let end = home.iter().rposition(|&c| c != b'/').map_or(0, |j| j + 1);
        let mut out = home[..end].to_vec();
        out.extend_from_slice(&p[i..]);
        if out.is_empty() {
            out.push(b'/');
        }
        out
    }
}

pub mod approve {
    //! SWAP(MS1): `anthology_memory::approve`, copied from `origin/port-ms1`.
    //!
    //! Repo approval (MS1): a port of `memory_sync`'s `_load_approved`, `is_approved`, `approve`,
    //! `unapprove` and `ensure_union_attrs` (plan §3.6).
    //!
    //! Approvals live in `<anthology home>/approved-repos.json`: `{"repos": {<key>: {"approved":
    //! "YYYY-MM-DD"}}, "version": 1}`, written as `json.dumps(indent=2, sort_keys=True) + "\n"`
    //! through `_atomic_write`. An unreadable or invalid file means nothing is approved; the next
    //! approval rewrites it.
    //!
    //! Errors are Python's: an `OSError` as `PyExc::OSError`, and a refusal to write through a
    //! symlinked or special `.gitattributes` as `PyExc::Other("UnsafeTarget", <message>)`
    //! (`anthology_core::fsx`).

    use std::path::{Path, PathBuf};

    use anthology_core::git::Git;
    use anthology_core::{Env, clock, fsx};
    use anthology_pycompat::json::{self, Map, Value};
    use anthology_pycompat::pyerr::PyExc;
    use anthology_pycompat::str::split_ws;

    use super::repo::{anthology_home, repo_key};

    /// `memory_sync.UNION_LINES`: the `merge=union` attributes of the generated indexes.
    pub const UNION_LINES: [&str; 2] = [
        ".claude/memory/MEMORY.md merge=union",
        ".claude/agent-memory/*/MEMORY.md merge=union",
    ];

    /// `_approved_path(env)`.
    pub fn approved_path(env: &Env) -> PathBuf {
        anthology_home(env).join("approved-repos.json")
    }

    /// `memory_sync._load_approved(env)`: `{"version": 1, "repos": {...}}`, with `repos` empty
    /// when the file is missing, unreadable, not UTF-8, not JSON, or not an object holding an
    /// object under `repos`.
    pub fn load_approved(env: &Env) -> Map {
        let repos = std::fs::read(approved_path(env))
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|text| json::loads(&text).ok())
            .and_then(Value::into_object)
            .and_then(|mut data| data.remove("repos"))
            .and_then(Value::into_object)
            .unwrap_or_default();
        let mut out = Map::new();
        out.insert("version".into(), Value::Int(json::Int::Small(1)));
        out.insert("repos".into(), Value::Object(repos));
        out
    }

    /// `memory_sync.is_approved(key, env)`.
    pub fn is_approved(key: &str, env: &Env) -> bool {
        repos(&load_approved(env)).contains_key(key)
    }

    /// `memory_sync.approve(root, env)`: records the repo at `root` as approved (today's date,
    /// once), then adds the union merge attributes. Returns its key.
    ///
    /// The approval is saved first: when `.gitattributes` is a symlink or not a regular file, the
    /// `UnsafeTarget` error comes after the approval is already recorded, and nothing is written
    /// through the link.
    pub fn approve(root: &Path, env: &Env, git: &dyn Git) -> Result<String, PyExc> {
        let key = repo_key(root, env, git);
        let mut data = load_approved(env);
        if !repos(&data).contains_key(&key) {
            let mut entry = Map::new();
            entry.insert("approved".into(), Value::Str(clock::today_local().iso()));
            repos_mut(&mut data).insert(key.clone(), Value::Object(entry));
            save_approved(&data, env)?;
        }
        ensure_union_attrs(root)?;
        Ok(key)
    }

    /// `memory_sync.unapprove(key, env)`: `false` when `key` was not approved.
    pub fn unapprove(key: &str, env: &Env) -> Result<bool, PyExc> {
        let mut data = load_approved(env);
        if repos_mut(&mut data).remove(key).is_none() {
            return Ok(false);
        }
        save_approved(&data, env)?;
        Ok(true)
    }

    /// `memory_sync.ensure_union_attrs(root)`: adds each missing [`UNION_LINES`] line to
    /// `<root>/.gitattributes` (lines compare with their whitespace collapsed); returns whether
    /// it changed anything. Refuses a symlinked or special `.gitattributes` (`UnsafeTarget`).
    pub fn ensure_union_attrs(root: &Path) -> Result<bool, PyExc> {
        let path = root.join(".gitattributes");
        let existing = fsx::read_regular(&path, None)?;
        let existing = String::from_utf8_lossy(&existing);
        let have: Vec<String> = splitlines(&existing)
            .into_iter()
            .map(|l| split_ws(l).join(" "))
            .collect();
        let mut changed = false;
        for line in UNION_LINES {
            if !have.iter().any(|h| h == line) {
                fsx::append_line(&path, line)?;
                changed = true;
            }
        }
        Ok(changed)
    }

    fn save_approved(data: &Map, env: &Env) -> Result<(), PyExc> {
        let mut text = json::dumps(&Value::Object(data.clone()), &json::INDENT2_SORTED);
        text.push('\n');
        fsx::atomic_write(
            &approved_path(env),
            text.as_bytes(),
            0o644,
            ".tmp-",
            ".part",
        )
    }

    fn repos(data: &Map) -> &Map {
        data.get("repos")
            .and_then(Value::as_object)
            .expect("load_approved always holds repos")
    }

    fn repos_mut(data: &mut Map) -> &mut Map {
        data.get_mut("repos")
            .and_then(Value::as_object_mut)
            .expect("load_approved always holds repos")
    }

    /// `str.splitlines()`: lines split at `\n`, `\r\n`, `\r`, `\v`, `\f`, `\x1c`–`\x1e`, U+0085,
    /// U+2028 and U+2029, without the breaks; no trailing empty line.
    pub(crate) fn splitlines(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut it = s.char_indices().peekable();
        while let Some((i, c)) = it.next() {
            let brk = matches!(
                c,
                '\n' | '\r'
                    | '\x0b'
                    | '\x0c'
                    | '\x1c'
                    | '\x1d'
                    | '\x1e'
                    | '\u{85}'
                    | '\u{2028}'
                    | '\u{2029}'
            );
            if !brk {
                continue;
            }
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' && it.peek().is_some_and(|&(_, n)| n == '\n') {
                it.next();
                next += 1;
            }
            start = next;
        }
        if start < s.len() {
            out.push(&s[start..]);
        }
        out
    }
}

pub mod notes {
    //! SWAP(MS1): `anthology_memory::notes`, copied from `origin/port-ms1`.
    //!
    //! Note parsing (MS1): a port of `memory_index.parse_text` / `parse_note` (the small frontmatter
    //! parser `memory_sync`, the indexer and the portable archive share) and of `memory_sync`'s
    //! `note_info`, `normalize_type` and `is_user_note` (plan §3.6, GC-12).
    //!
    //! The parser reads the YAML subset the notes use (nested maps by indentation, `- item` lists,
    //! `[a, b]` flow lists, `>`/`|` block scalars, single- and double-quoted scalars), exactly as
    //! the Python does, and never fails: broken frontmatter gives a note listed by its stem.
    //!
    //! **Divergence (D-1).** Python's `_parse_block` recurses once per nesting level, so a
    //! frontmatter nested about a thousand levels deep raises `RecursionError`, which `parse_text`
    //! catches, resetting the note to its stem (type `other`). Rust parses any depth (an explicit
    //! stack, no recursion), so such a note keeps its fields. That only ever makes Rust stricter:
    //! a deep `type: user` note is still recognised as a user note.
    //!
    //! [`NoteInfo`] field mapping: `kind` is Python's `type` and is always `Some` from the parser
    //! (`"other"` when the note names none; consumers that build a `NoteInfo` by hand may use
    //! `None`, which every function here reads as `"other"`); `updated` is `None` where Python's is
    //! `""`. `relations` keeps the three kinds the indexer reads (`RELATION_KINDS`).

    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path};
    use std::sync::OnceLock;

    use anthology_pycompat::re::{self, Flags, Regex};
    use anthology_pycompat::str::{char_len, char_slice, lower, strip_ws};

    /// `memory_sync.USER_REASON`: why a `type: user` note never goes into a repo (GC-12).
    pub const USER_REASON: &str = "type: user notes are personal; they never go into a repo";

    /// `memory_sync.REPO_EXCLUDE`: the note types a repo or agent index never lists.
    pub const REPO_EXCLUDE: &[&str] = &["user"];

    /// `memory_index.RELATION_KINDS`.
    pub const RELATION_KINDS: [&str; 3] = ["supersedes", "depends_on", "relates_to"];

    /// The `metadata.relations` targets of a note, by kind (`info["relations"].get(kind, [])`).
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Relations {
        pub supersedes: Vec<String>,
        pub depends_on: Vec<String>,
        pub relates_to: Vec<String>,
    }

    impl Relations {
        /// The targets of `kind` (one of [`RELATION_KINDS`]; any other kind has none).
        pub fn get(&self, kind: &str) -> &[String] {
            match kind {
                "supersedes" => &self.supersedes,
                "depends_on" => &self.depends_on,
                "relates_to" => &self.relates_to,
                _ => &[],
            }
        }
    }

    /// A parsed note (`memory_index.parse_text`'s dict; see the module docs for the mapping).
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct NoteInfo {
        pub name: String,
        pub description: String,
        /// Python's `type`: `Some("other")` when the note names none.
        pub kind: Option<String>,
        /// Python's `updated`: `None` where Python has `""`.
        pub updated: Option<String>,
        pub relations: Relations,
        pub body: String,
    }

    impl NoteInfo {
        /// Python's `info["type"]`: the type, `"other"` when there is none.
        pub fn type_str(&self) -> &str {
            match self.kind.as_deref() {
                Some(t) if !t.is_empty() => t,
                _ => "other",
            }
        }

        /// Python's `info["updated"]`: the date text, `""` when there is none.
        pub fn updated_str(&self) -> &str {
            self.updated.as_deref().unwrap_or("")
        }
    }

    /// `memory_index.parse_text(text, stem)`: never fails; broken frontmatter yields a note listed
    /// by `stem`.
    pub fn parse_text(text: &str, stem: &str) -> NoteInfo {
        parse_text_ok(text, stem).0
    }

    /// [`parse_text`] and Python's `info["ok"]`: whether the frontmatter was read.
    pub fn parse_text_ok(text: &str, stem: &str) -> (NoteInfo, bool) {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let text = text.replace("\r\n", "\n");
        let mut info = NoteInfo {
            name: stem.to_string(),
            description: String::new(),
            kind: Some("other".to_string()),
            updated: None,
            relations: Relations::default(),
            body: text.clone(),
        };
        let Some(rest) = text.strip_prefix("---\n") else {
            return (info, false);
        };
        let Some((start, end)) = front_end(rest) else {
            return (info, false);
        };
        info.body = rest[end..].trim_start_matches('\n').to_string();
        let data = parse_yaml(&rest[..start]);
        let empty = YMap::default();
        let meta = match data.get("metadata") {
            Some(Y::Map(m)) => m,
            _ => &empty,
        };
        let name = text_of(data.get("name"));
        info.name = if name.is_empty() {
            stem.to_string()
        } else {
            name
        };
        info.description = text_of(data.get("description"));
        let kind = first_text(&[meta.get("type"), data.get("type")]);
        info.kind = Some(if kind.is_empty() {
            "other".to_string()
        } else {
            kind
        });
        let updated = first_text(&[
            meta.get("modified"),
            meta.get("updated"),
            data.get("modified"),
            data.get("updated"),
        ]);
        info.updated = (!updated.is_empty()).then_some(updated);
        if let Some(Y::Map(rel)) = meta.get("relations") {
            // `{_text(k): _targets(v) for k, v in relations.items() if _targets(v)}`: per kind, the
            // last key that strips to it and has targets wins.
            for (k, v) in &rel.entries {
                let targets = targets_of(v);
                if targets.is_empty() {
                    continue;
                }
                match strip_ws(k) {
                    "supersedes" => info.relations.supersedes = targets,
                    "depends_on" => info.relations.depends_on = targets,
                    "relates_to" => info.relations.relates_to = targets,
                    _ => {}
                }
            }
        }
        (info, true)
    }

    /// `memory_index.parse_note(path)`: the note at `path`, parsed with its stem (`Path.stem`);
    /// `None` when the file cannot be read (Python raises `OSError`). Follows symlinks, as
    /// `read_bytes` does.
    pub fn parse_note(path: &Path) -> Option<NoteInfo> {
        let data = std::fs::read(path).ok()?;
        Some(parse_text(&decode_replace(&data), &path_stem(path)))
    }

    /// `memory_sync.note_info(name, data)`: `parse_text(data.decode("utf-8", errors="replace"),
    /// name[:-3])`.
    pub fn note_info(name: &str, data: &[u8]) -> NoteInfo {
        let stem = char_slice(name, 0, char_len(name).saturating_sub(3));
        parse_text(&decode_replace(data), stem)
    }

    /// `memory_sync.is_user_note(info)`: a `type: user` note, in any case, with or without a
    /// trailing `# comment`.
    pub fn is_user_note(info: &NoteInfo) -> bool {
        normalize_type(info.kind.as_deref()) == "user"
    }

    /// `memory_sync.normalize_type(value)`: lower case, a trailing `# comment` and the surrounding
    /// whitespace dropped (`str(value or "").split("#", 1)[0].strip().lower()`).
    pub fn normalize_type(value: Option<&str>) -> String {
        let v = value.unwrap_or("");
        let head = v.split_once('#').map_or(v, |(h, _)| h);
        lower(strip_ws(head))
    }

    /// `bytes.decode("utf-8", errors="replace")`: CPython replaces each maximal invalid subpart
    /// with one U+FFFD, as Rust's lossy decoding does.
    pub fn decode_replace(data: &[u8]) -> String {
        String::from_utf8_lossy(data).into_owned()
    }

    /// `PurePath.stem` (CPython 3.12): the last component without its last suffix; a name whose
    /// only dot is the first or the last character keeps it. A name that is not UTF-8 is decoded
    /// lossily (F-19).
    pub fn path_stem(path: &Path) -> String {
        let name = match path.components().next_back() {
            Some(Component::Normal(n)) => n,
            Some(Component::ParentDir) => OsStr::new(".."),
            _ => OsStr::new(""),
        };
        let b = name.as_bytes();
        let stem = match b.iter().rposition(|&c| c == b'.') {
            Some(i) if i > 0 && i < b.len() - 1 => &b[..i],
            _ => b,
        };
        String::from_utf8_lossy(stem).into_owned()
    }

    /// Python's sort key for a file name (`str` order of the surrogate-escaped name): code points,
    /// with each byte that is not UTF-8 as `U+DC80..U+DCFF`. For a UTF-8 name it is byte order.
    pub(crate) fn py_name_key(name: &OsStr) -> Vec<u32> {
        let mut out = Vec::with_capacity(name.len());
        for chunk in name.as_bytes().utf8_chunks() {
            out.extend(chunk.valid().chars().map(u32::from));
            out.extend(chunk.invalid().iter().map(|&b| 0xDC00 + u32::from(b)));
        }
        out
    }

    // ------------------------------------------------------------------------------------------
    // The frontmatter end and the YAML subset
    // ------------------------------------------------------------------------------------------

    /// `_FRONT_END.search(rest)` (`^---[ \t]*$`, MULTILINE): the span of the closing line.
    fn front_end(rest: &str) -> Option<(usize, usize)> {
        static RX: OnceLock<Regex> = OnceLock::new();
        let rx = RX.get_or_init(|| {
            re::compile_with(
                r"^---[ \t]*$",
                Flags {
                    multiline: true,
                    ..Flags::default()
                },
            )
            .expect("_FRONT_END compiles")
        });
        // A plain (non-fancy) pattern cannot hit a backtrack limit.
        rx.find_at(rest, 0).ok().flatten()
    }

    /// A parsed YAML value: a scalar, a list of scalars, or a map.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Y {
        Str(String),
        List(Vec<String>),
        Map(YMap),
    }

    /// A Python dict: insertion order, a repeated key replaces the value in its first position.
    /// The index keeps a frontmatter of many keys linear, as Python's dict is.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct YMap {
        entries: Vec<(String, Y)>,
        index: HashMap<String, usize>,
    }

    impl YMap {
        fn get(&self, k: &str) -> Option<&Y> {
            self.index.get(k).map(|&i| &self.entries[i].1)
        }

        fn insert(&mut self, k: String, v: Y) {
            match self.index.get(&k) {
                Some(&i) => self.entries[i].1 = v,
                None => {
                    self.index.insert(k.clone(), self.entries.len());
                    self.entries.push((k, v));
                }
            }
        }
    }

    /// `_text(value)`: a stripped string, or `""` for anything else.
    fn text_of(v: Option<&Y>) -> String {
        match v {
            Some(Y::Str(s)) => strip_ws(s).to_string(),
            _ => String::new(),
        }
    }

    /// `_text(a) or _text(b) or ...`.
    fn first_text(vals: &[Option<&Y>]) -> String {
        vals.iter()
            .map(|v| text_of(*v))
            .find(|t| !t.is_empty())
            .unwrap_or_default()
    }

    /// `_targets(value)`: the non-empty stripped strings of a string or a list.
    fn targets_of(v: &Y) -> Vec<String> {
        let items: Vec<&str> = match v {
            Y::Str(s) => vec![s.as_str()],
            Y::List(l) => l.iter().map(String::as_str).collect(),
            Y::Map(_) => return Vec::new(),
        };
        items
            .into_iter()
            .map(strip_ws)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// `_scalar(value)`: stripped; one pair of matching quotes removed (`''` in single quotes is
    /// `'`; `\n`, `\"` and `\\` in double quotes are unescaped, in that order).
    fn scalar(value: &str) -> String {
        let v = strip_ws(value);
        let quoted = char_len(v) >= 2 && {
            let first = v.as_bytes()[0];
            (first == b'\'' || first == b'"') && v.as_bytes()[v.len() - 1] == first
        };
        if !quoted {
            return v.to_string();
        }
        let inner = &v[1..v.len() - 1];
        if v.starts_with('\'') {
            inner.replace("''", "'")
        } else {
            inner
                .replace("\\n", "\n")
                .replace("\\\"", "\"")
                .replace("\\\\", "\\")
        }
    }

    /// `_flow_list(value)`: `[a, "b"]` is a list; without the closing `]` the text stays a string.
    fn flow_list(value: &str) -> Y {
        let v = strip_ws(value);
        if !v.ends_with(']') {
            return Y::Str(v.to_string());
        }
        let inner = char_slice(v, 1, char_len(v).saturating_sub(1));
        Y::List(
            inner
                .split(',')
                .filter(|p| !strip_ws(p).is_empty())
                .map(scalar)
                .collect(),
        )
    }

    /// `_is_item(text)`.
    fn is_item(text: &str) -> bool {
        text == "-" || text.starts_with("- ")
    }

    /// `_parse_yaml(head)`: the top-level map, or an empty one.
    fn parse_yaml(head: &str) -> YMap {
        let lines: Vec<(usize, &str)> = head
            .split('\n')
            .filter_map(|raw| {
                let stripped = strip_ws(raw);
                if stripped.is_empty() || stripped.starts_with('#') {
                    return None;
                }
                Some((raw.len() - raw.trim_start_matches(' ').len(), stripped))
            })
            .collect();
        if lines.is_empty() {
            return YMap::default();
        }
        match parse_block(&lines, 0, lines[0].0) {
            Y::Map(m) => m,
            _ => YMap::default(),
        }
    }

    /// The `- item` lines at `indent` from `i`: `_parse_block`'s list case.
    fn parse_items(lines: &[(usize, &str)], mut i: usize, indent: usize) -> (Y, usize) {
        let mut items = Vec::new();
        while i < lines.len() && lines[i].0 == indent && is_item(lines[i].1) {
            items.push(scalar(&lines[i].1[1..]));
            i += 1;
        }
        (Y::List(items), i)
    }

    /// A map being read: its entries, its indent, and the key waiting for a nested map.
    struct Frame {
        out: YMap,
        indent: usize,
        pending: Option<String>,
        /// Whether its entries are kept (see [`KEPT_DEPTH`]).
        keep: bool,
    }

    /// Maps nested deeper than this (the top level is 0, `metadata` 1, `relations` 2) are parsed
    /// for their extent but keep no entries: nothing reads below `relations.<kind>`, where only
    /// "it is a map" matters. So the parsed tree is never more than a few levels deep, and
    /// neither building nor dropping it recurses per nesting level.
    const KEPT_DEPTH: usize = 3;

    impl Frame {
        /// `out[key] = value`, when this frame keeps its entries.
        fn put(&mut self, key: String, value: Y) {
            if self.keep {
                self.out.insert(key, value);
            }
        }
    }

    /// `_parse_block(lines, start, indent)`'s value, without recursion: a nested map is a new
    /// frame on an explicit stack, finished into its parent's pending key.
    fn parse_block(lines: &[(usize, &str)], start: usize, indent: usize) -> Y {
        if is_item(lines[start].1) {
            return parse_items(lines, start, indent).0;
        }
        let mut i = start;
        let mut stack = vec![Frame {
            out: YMap::default(),
            indent,
            pending: None,
            keep: true,
        }];
        loop {
            let top = stack.last_mut().expect("a frame");
            let mut nested = None;
            while i < lines.len() {
                let (ind, text) = lines[i];
                if ind < top.indent {
                    break;
                }
                if ind > top.indent || is_item(text) {
                    i += 1;
                    continue;
                }
                i += 1;
                let Some((key, val)) = text.split_once(':') else {
                    continue;
                };
                let key = scalar(key);
                let val = strip_ws(val);
                if val.is_empty() {
                    match lines.get(i) {
                        Some(&(n_ind, n_text))
                            if n_ind > top.indent || (n_ind == top.indent && is_item(n_text)) =>
                        {
                            if is_item(n_text) {
                                let (items, next) = parse_items(lines, i, n_ind);
                                top.put(key, items);
                                i = next;
                            } else {
                                top.pending = Some(key);
                                nested = Some(n_ind);
                                break;
                            }
                        }
                        _ => top.put(key, Y::Str(String::new())),
                    }
                } else if is_block_scalar(val) {
                    let mut parts = Vec::new();
                    while i < lines.len() && lines[i].0 > top.indent {
                        parts.push(lines[i].1);
                        i += 1;
                    }
                    top.put(key, Y::Str(parts.join(" ")));
                } else if val.starts_with('[') {
                    top.put(key, flow_list(val));
                } else {
                    top.put(key, Y::Str(scalar(val)));
                }
            }
            if let Some(indent) = nested {
                let keep = stack.len() < KEPT_DEPTH;
                stack.push(Frame {
                    out: YMap::default(),
                    indent,
                    pending: None,
                    keep,
                });
                continue;
            }
            let done = stack.pop().expect("a frame");
            match stack.last_mut() {
                None => return Y::Map(done.out),
                Some(parent) => {
                    let key = parent.pending.take().expect("a pending key");
                    parent.put(key, Y::Map(done.out));
                }
            }
        }
    }

    /// `val[0] in ">|" and not val[1:].strip("+-0123456789")`.
    fn is_block_scalar(val: &str) -> bool {
        let mut chars = val.chars();
        matches!(chars.next(), Some('>' | '|'))
            && chars.all(|c| c == '+' || c == '-' || c.is_ascii_digit())
    }
}

pub mod render {
    //! SWAP(MS1): `anthology_memory::render`, copied from `origin/port-ms1`.
    //!
    //! The generated `MEMORY.md` index (MS1): a port of `memory_sync`'s `_list_notes`,
    //! `_entry_line`, `render_index` and `regenerate_index` (plan §3.6, GC-12).
    //!
    //! The index is derived from the notes' frontmatter, deterministic (never mtimes or the clock):
    //! grouped by type (user, feedback, project, reference, then the rest alphabetically), dated
    //! notes newest first, then undated ones, ties by name and file. Over budget, entries are
    //! shared round-robin across types and each cut type ends with an "N more" pointer; if even the
    //! headings and pointers of all types do not fit, the last types collapse into one "N more in
    //! other types" pointer.

    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::Path;

    use anthology_core::fsx;
    use anthology_pycompat::pyerr::PyExc;
    use anthology_pycompat::quote::quote;
    use anthology_pycompat::str::{char_len, char_slice, split_ws};

    use super::notes::{NoteInfo, normalize_type, parse_note, py_name_key};

    /// `memory_sync.GENERATED`: the first line of every generated index.
    pub const GENERATED: &str = "<!-- Generated by anthology from the notes' frontmatter; edit the notes, not this file. -->";

    /// `memory_sync.TYPE_ORDER`: the types listed first, in this order.
    pub const TYPE_ORDER: [&str; 4] = ["user", "feedback", "project", "reference"];

    /// `memory_sync.MAX_LINE`: an entry line is cut to this many code points (with `…`).
    pub const MAX_LINE: usize = 200;

    /// A line and byte budget (`memory_sync.Budget`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Budget {
        pub lines: usize,
        pub bytes: usize,
    }

    /// Under Claude Code's 200-line / 25 KB load limit for a `MEMORY.md`.
    pub const DEFAULT_BUDGET: Budget = Budget {
        lines: 150,
        bytes: 20000,
    };

    /// The index text injected into a session, per section.
    pub const CONTEXT_SECTION_BUDGET: Budget = Budget {
        lines: 40,
        bytes: 2500,
    };

    /// `memory_sync._list_notes(mem_dir)`: the notes of `mem_dir` (`*.md` except `MEMORY.md`,
    /// regular files only, never through a symlink), sorted by file name. A missing folder, or
    /// one that cannot be listed, gives none (see [`try_list_notes`] for Python's `OSError`).
    pub fn list_notes(mem_dir: &Path) -> Vec<NoteInfo> {
        try_list_notes(mem_dir)
            .map(|notes| notes.into_iter().map(|(_, info)| info).collect())
            .unwrap_or_default()
    }

    /// `memory_sync._list_notes(mem_dir)` with each note's file name (`info["file"]`). A missing
    /// folder (or not a folder) gives `Ok(vec![])`; a folder that cannot be listed is the
    /// `OSError` Python's `os.scandir` raises.
    pub fn try_list_notes(mem_dir: &Path) -> Result<Vec<(String, NoteInfo)>, PyExc> {
        if !std::fs::metadata(mem_dir).is_ok_and(|m| m.is_dir()) {
            return Ok(Vec::new());
        }
        let os_err = |e: std::io::Error| match PyExc::from(e) {
            PyExc::OSError { errno, .. } => PyExc::OSError {
                errno,
                filename: Some(mem_dir.to_string_lossy().into_owned()),
            },
            other => other,
        };
        let mut entries: Vec<(Vec<u32>, OsString, std::fs::DirEntry)> = Vec::new();
        for entry in std::fs::read_dir(mem_dir).map_err(os_err)? {
            let entry = entry.map_err(os_err)?;
            let name = entry.file_name();
            entries.push((py_name_key(&name), name, entry));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut notes = Vec::new();
        for (_, name, entry) in entries {
            let bytes = name.as_encoded_bytes();
            if !bytes.ends_with(b".md") || bytes == b"MEMORY.md" {
                continue;
            }
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let Some(info) = parse_note(&entry.path()) else {
                continue;
            };
            notes.push((name.to_string_lossy().into_owned(), info));
        }
        Ok(notes)
    }

    /// `" ".join(str(text).split())`.
    fn one_line(text: &str) -> String {
        split_ws(text).join(" ")
    }

    /// `_entry_line(note)`: `- [name](quoted file) — description`, cut to [`MAX_LINE`].
    fn entry_line(file: &str, info: &NoteInfo) -> String {
        let mut line = format!("- [{}]({})", one_line(&info.name), quote(file, "/"));
        let desc = one_line(&info.description);
        if !desc.is_empty() {
            line.push_str(" — ");
            line.push_str(&desc);
        }
        if char_len(&line) > MAX_LINE {
            line = format!("{}…", char_slice(&line, 0, MAX_LINE - 1));
        }
        line
    }

    /// `_type_rank(type_)`.
    fn type_rank(t: &str) -> (usize, &str) {
        match TYPE_ORDER.iter().position(|o| *o == t) {
            Some(i) => (i, ""),
            None => (TYPE_ORDER.len(), t),
        }
    }

    fn pointer(hidden: usize, suffix: &str) -> String {
        format!("- {hidden} more — use `anthology-mem search`{suffix}")
    }

    fn collapsed_pointer(hidden: usize, suffix: &str) -> String {
        format!("- {hidden} more in other types — use `anthology-mem search`{suffix}")
    }

    /// `_blen(line)`: its UTF-8 bytes and the newline.
    fn blen(line: &str) -> usize {
        line.len() + 1
    }

    /// `memory_sync.render_index(mem_dir, budget, suffix, exclude_types)`: the generated index
    /// text for the notes in `mem_dir`, `None` when it has no notes. Notes whose (normalised) type
    /// is in `exclude_types` are never listed; if that leaves none, the text is the header alone,
    /// so a stale index is replaced. `suffix` is appended to every "N more" pointer.
    pub fn render_index(
        mem_dir: &Path,
        budget: Budget,
        suffix: &str,
        exclude_types: &[&str],
    ) -> Result<Option<String>, PyExc> {
        let notes = try_list_notes(mem_dir)?;
        if notes.is_empty() {
            return Ok(None);
        }
        let excluded: Vec<String> = exclude_types
            .iter()
            .map(|t| normalize_type(Some(t)))
            .collect();
        let mut notes: Vec<(String, NoteInfo)> = notes
            .into_iter()
            .filter(|(_, n)| !excluded.contains(&normalize_type(Some(n.type_str()))))
            .collect();
        if notes.is_empty() {
            return Ok(Some(format!("{GENERATED}\n")));
        }
        notes.sort_by(|a, b| (&a.1.name, &a.0).cmp(&(&b.1.name, &b.0)));
        // `by_type`, then sorted by `_type_rank` (the keys are distinct, so the order is total).
        let mut grouped: BTreeMap<&str, Vec<&(String, NoteInfo)>> = BTreeMap::new();
        for n in &notes {
            grouped.entry(n.1.type_str()).or_default().push(n);
        }
        let mut by_type: Vec<(String, Vec<&(String, NoteInfo)>)> = grouped
            .into_iter()
            .map(|(t, items)| (t.to_string(), items))
            .collect();
        by_type.sort_by(|a, b| type_rank(&a.0).cmp(&type_rank(&b.0)));
        let mut groups: Vec<(String, Vec<String>)> = by_type
            .into_iter()
            .map(|(t, items)| {
                let mut dated: Vec<&&(String, NoteInfo)> = items
                    .iter()
                    .filter(|n| !n.1.updated_str().is_empty())
                    .collect();
                // `sorted(..., reverse=True)` is stable: equal dates keep their order.
                dated.sort_by(|a, b| b.1.updated_str().cmp(a.1.updated_str()));
                let undated = items.iter().filter(|n| n.1.updated_str().is_empty());
                let entries = dated
                    .into_iter()
                    .chain(undated)
                    .map(|n| entry_line(&n.0, &n.1))
                    .collect();
                (t, entries)
            })
            .collect();

        let cost = |groups: &[(String, Vec<String>)], gi: usize, k: usize| -> (usize, usize) {
            let (t, entries) = &groups[gi];
            let more = k < entries.len();
            let lines = 1 + k + usize::from(more);
            let mut size =
                blen(&format!("## {t}")) + entries[..k].iter().map(|e| blen(e)).sum::<usize>();
            if more {
                size += blen(&pointer(entries.len() - k, suffix));
            }
            (lines, size)
        };
        let fits = |lines: usize, size: usize| lines <= budget.lines && size <= budget.bytes;

        let mut counts: Vec<usize> = groups.iter().map(|g| g.1.len()).collect();
        let mut base_lines = 1;
        let mut base_bytes = blen(GENERATED);
        let mut costs: Vec<(usize, usize)> = (0..groups.len())
            .map(|gi| cost(&groups, gi, counts[gi]))
            .collect();
        let mut total_l = base_lines + costs.iter().map(|c| c.0).sum::<usize>();
        let mut total_b = base_bytes + costs.iter().map(|c| c.1).sum::<usize>();
        let mut collapsed = None;
        if !fits(total_l, total_b) {
            let zero: Vec<(usize, usize)> =
                (0..groups.len()).map(|gi| cost(&groups, gi, 0)).collect();
            let mut keep = groups.len();
            while keep > 0 {
                let hidden: usize = groups[keep..].iter().map(|g| g.1.len()).sum();
                let extra = if keep < groups.len() {
                    (1, blen(&collapsed_pointer(hidden, suffix)))
                } else {
                    (0, 0)
                };
                if fits(
                    base_lines + extra.0 + zero[..keep].iter().map(|c| c.0).sum::<usize>(),
                    base_bytes + extra.1 + zero[..keep].iter().map(|c| c.1).sum::<usize>(),
                ) {
                    break;
                }
                keep -= 1;
            }
            if keep < groups.len() {
                let hidden: usize = groups[keep..].iter().map(|g| g.1.len()).sum();
                let text = collapsed_pointer(hidden, suffix);
                groups.truncate(keep);
                base_lines += 1;
                base_bytes += blen(&text);
                collapsed = Some(text);
            }
            counts = vec![0; groups.len()];
            costs = (0..groups.len()).map(|gi| cost(&groups, gi, 0)).collect();
            total_l = base_lines + costs.iter().map(|c| c.0).sum::<usize>();
            total_b = base_bytes + costs.iter().map(|c| c.1).sum::<usize>();
            let mut live: Vec<usize> = (0..groups.len()).collect();
            while !live.is_empty() {
                for gi in live.clone() {
                    if counts[gi] >= groups[gi].1.len() {
                        live.retain(|&g| g != gi);
                        continue;
                    }
                    let new = cost(&groups, gi, counts[gi] + 1);
                    let nl = total_l - costs[gi].0 + new.0;
                    let nb = total_b - costs[gi].1 + new.1;
                    if !fits(nl, nb) {
                        live.retain(|&g| g != gi);
                        continue;
                    }
                    counts[gi] += 1;
                    costs[gi] = new;
                    total_l = nl;
                    total_b = nb;
                }
            }
        }
        let mut lines = vec![GENERATED.to_string()];
        for ((t, entries), &k) in groups.iter().zip(&counts) {
            lines.push(format!("## {t}"));
            lines.extend(entries[..k].iter().cloned());
            if k < entries.len() {
                lines.push(pointer(entries.len() - k, suffix));
            }
        }
        if let Some(c) = collapsed {
            lines.push(c);
        }
        Ok(Some(lines.join("\n") + "\n"))
    }

    /// `memory_sync.regenerate_index(mem_dir, budget, exclude_types)`: rewrites
    /// `<mem_dir>/MEMORY.md` from the notes' frontmatter ([`render_index`]) when its content
    /// changed; returns whether it wrote. A folder without notes is left alone. Only a regular
    /// `MEMORY.md` is compared (a FIFO or a symlink is never read): anything else is replaced,
    /// never written through.
    pub fn regenerate_index(
        mem_dir: &Path,
        budget: Budget,
        exclude_types: &[&str],
    ) -> Result<bool, PyExc> {
        let Some(text) = render_index(mem_dir, budget, "", exclude_types)? else {
            return Ok(false);
        };
        let data = text.into_bytes();
        let target = mem_dir.join("MEMORY.md");
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_file())
            && fsx::read_regular(&target, None).is_ok_and(|old| old == data)
        {
            return Ok(false);
        }
        fsx::atomic_write(&target, &data, 0o644, ".tmp-", ".part")?;
        Ok(true)
    }
}
