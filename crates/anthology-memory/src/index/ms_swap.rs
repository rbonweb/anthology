//! SWAP(MS1|MS2): private stand-ins for the plan §3.6 items of MS1 (`notes`, `repo`,
//! `approve`, `render`) and MS2 (`save`) that the index and `anthology mem` call, written while
//! those lanes run in parallel (wave 2/3 lead rules). Each item has its §3.6 signature exactly,
//! so the swap is: delete this file and its `mod` line, and point `index.rs`'s `use` at
//! `crate::{notes, repo, approve, save}`. The bodies port the Python they stand for
//! (`memory_index.parse_text`, `memory_sync.{repo_root, repo_key, note_sources, is_approved,
//! save_note, regenerate_index}`) closely enough for MI1's tests; `regenerate_index` leaves out
//! the line and byte budget (no MI1 test reaches it).
#![allow(clippy::too_many_arguments)]

pub mod notes {
    //! SWAP(MS1): `anthology_memory::notes`.
    use std::path::Path;

    /// SWAP(MS1): §3.6 `Relations`.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Relations {
        pub supersedes: Vec<String>,
        pub depends_on: Vec<String>,
        pub relates_to: Vec<String>,
    }

    /// SWAP(MS1): §3.6 `NoteInfo`. This stand-in sets `kind` to Python's `type` (`other` when
    /// the note names none) and `updated` to `None` when Python's is empty.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct NoteInfo {
        pub name: String,
        pub description: String,
        pub kind: Option<String>,
        pub updated: Option<String>,
        pub relations: Relations,
        pub body: String,
    }

    #[derive(Clone, Debug)]
    enum Y {
        Str(String),
        List(Vec<String>),
        Map(Vec<(String, Y)>),
    }

    fn put(map: &mut Vec<(String, Y)>, key: String, value: Y) {
        match map.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => map.push((key, value)),
        }
    }

    fn strip(s: &str) -> &str {
        anthology_pycompat::str::strip_ws(s)
    }

    fn scalar(value: &str) -> String {
        let v = strip(value);
        let cs: Vec<char> = v.chars().collect();
        if cs.len() >= 2 && cs[0] == cs[cs.len() - 1] && (cs[0] == '\'' || cs[0] == '"') {
            let inner: String = cs[1..cs.len() - 1].iter().collect();
            if cs[0] == '\'' {
                return inner.replace("''", "'");
            }
            return inner
                .replace("\\n", "\n")
                .replace("\\\"", "\"")
                .replace("\\\\", "\\");
        }
        v.to_string()
    }

    fn flow_list(value: &str) -> Y {
        let v = strip(value);
        if !v.ends_with(']') {
            return Y::Str(v.to_string());
        }
        let cs: Vec<char> = v.chars().collect();
        let inner: String = cs[1..cs.len() - 1].iter().collect();
        Y::List(
            inner
                .split(',')
                .filter(|p| !strip(p).is_empty())
                .map(scalar)
                .collect(),
        )
    }

    fn is_item(text: &str) -> bool {
        text == "-" || text.starts_with("- ")
    }

    /// Python's recursion limit stands behind the `except Exception` fallback.
    const MAX_DEPTH: usize = 900;

    fn parse_block(
        lines: &[(usize, String)],
        mut i: usize,
        indent: usize,
        depth: usize,
    ) -> Option<(Y, usize)> {
        if depth > MAX_DEPTH {
            return None;
        }
        if is_item(&lines[i].1) {
            let mut items = Vec::new();
            while i < lines.len() && lines[i].0 == indent && is_item(&lines[i].1) {
                items.push(scalar(&lines[i].1[1..]));
                i += 1;
            }
            return Some((Y::List(items), i));
        }
        let mut out = Vec::new();
        while i < lines.len() {
            let (ind, text) = (&lines[i].0, &lines[i].1);
            let ind = *ind;
            if ind < indent {
                break;
            }
            if ind > indent || is_item(text) {
                i += 1;
                continue;
            }
            i += 1;
            let Some((key, val)) = text.split_once(':') else {
                continue;
            };
            let key = scalar(key);
            let val = strip(val);
            if val.is_empty() {
                let nested = lines
                    .get(i)
                    .filter(|n| n.0 > indent || (n.0 == indent && is_item(&n.1)));
                if let Some(n) = nested {
                    let (v, next) = parse_block(lines, i, n.0, depth + 1)?;
                    put(&mut out, key, v);
                    i = next;
                } else {
                    put(&mut out, key, Y::Str(String::new()));
                }
            } else if (val.starts_with('>') || val.starts_with('|'))
                && val[1..]
                    .trim_matches(|c: char| "+-0123456789".contains(c))
                    .is_empty()
            {
                let mut parts = Vec::new();
                while i < lines.len() && lines[i].0 > indent {
                    parts.push(lines[i].1.clone());
                    i += 1;
                }
                put(&mut out, key, Y::Str(parts.join(" ")));
            } else if val.starts_with('[') {
                put(&mut out, key, flow_list(val));
            } else {
                put(&mut out, key, Y::Str(scalar(val)));
            }
        }
        Some((Y::Map(out), i))
    }

    fn parse_yaml(head: &str) -> Option<Vec<(String, Y)>> {
        let mut lines = Vec::new();
        for raw in head.split('\n') {
            let stripped = strip(raw);
            if stripped.is_empty() || stripped.starts_with('#') {
                continue;
            }
            let indent = raw.chars().take_while(|&c| c == ' ').count();
            lines.push((indent, stripped.to_string()));
        }
        if lines.is_empty() {
            return Some(Vec::new());
        }
        match parse_block(&lines, 0, lines[0].0, 0)?.0 {
            Y::Map(m) => Some(m),
            _ => Some(Vec::new()),
        }
    }

    fn get<'a>(m: &'a [(String, Y)], k: &str) -> Option<&'a Y> {
        m.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }

    fn text(v: Option<&Y>) -> String {
        match v {
            Some(Y::Str(s)) => strip(s).to_string(),
            _ => String::new(),
        }
    }

    fn targets(v: &Y) -> Vec<String> {
        match v {
            Y::Str(s) => vec![strip(s).to_string()]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect(),
            Y::List(l) => l
                .iter()
                .map(|t| strip(t).to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            Y::Map(_) => Vec::new(),
        }
    }

    /// The first `^---[ \t]*$` line (re.M): its start and end byte offsets.
    fn front_end(rest: &str) -> Option<(usize, usize)> {
        let mut at = 0;
        loop {
            let line_end = rest[at..].find('\n').map_or(rest.len(), |n| at + n);
            let line = &rest[at..line_end];
            if let Some(tail) = line.strip_prefix("---")
                && tail.chars().all(|c| c == ' ' || c == '\t')
            {
                return Some((at, line_end));
            }
            if line_end >= rest.len() {
                return None;
            }
            at = line_end + 1;
        }
    }

    /// SWAP(MS1): `parse_text(text, stem)`.
    pub fn parse_text(text: &str, stem: &str) -> NoteInfo {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let text = text.replace("\r\n", "\n");
        let plain = NoteInfo {
            name: stem.to_string(),
            kind: Some("other".into()),
            body: text.clone(),
            ..NoteInfo::default()
        };
        let Some(rest) = text.strip_prefix("---\n") else {
            return plain;
        };
        let Some((start, end)) = front_end(rest) else {
            return plain;
        };
        let body = rest[end..].trim_start_matches('\n').to_string();
        let Some(data) = parse_yaml(&rest[..start]) else {
            return NoteInfo { body, ..plain };
        };
        let meta: &[(String, Y)] = match get(&data, "metadata") {
            Some(Y::Map(m)) => m,
            _ => &[],
        };
        let or = |a: String, b: String| if a.is_empty() { b } else { a };
        let name = or(text_of(&data, "name"), stem.to_string());
        let kind = or(
            or(text_of(meta, "type"), text_of(&data, "type")),
            "other".into(),
        );
        let updated = or(
            or(text_of(meta, "modified"), text_of(meta, "updated")),
            or(text_of(&data, "modified"), text_of(&data, "updated")),
        );
        let mut relations = Relations::default();
        if let Some(Y::Map(rel)) = get(meta, "relations") {
            for (k, v) in rel {
                let t = targets(v);
                if t.is_empty() {
                    continue;
                }
                match strip(k) {
                    "supersedes" => relations.supersedes = t,
                    "depends_on" => relations.depends_on = t,
                    "relates_to" => relations.relates_to = t,
                    _ => {}
                }
            }
        }
        NoteInfo {
            name,
            description: text_of(&data, "description"),
            kind: Some(kind),
            updated: (!updated.is_empty()).then_some(updated),
            relations,
            body,
        }
    }

    fn text_of(m: &[(String, Y)], k: &str) -> String {
        text(get(m, k))
    }

    /// SWAP(MS1): `parse_note(path)`; `None` when the file cannot be read.
    pub fn parse_note(path: &Path) -> Option<NoteInfo> {
        let data = std::fs::read(path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let stem = String::from_utf8_lossy(crate::index::py_stem(name.as_bytes())).into_owned();
        Some(parse_text(&String::from_utf8_lossy(&data), &stem))
    }
}

pub mod repo {
    //! SWAP(MS1): `anthology_memory::repo`.
    use std::path::{Path, PathBuf};

    use anthology_core::Env;
    use anthology_core::git::Git;
    use anthology_pycompat::path::realpath;

    /// SWAP(MS1): §3.6 `NoteSource`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct NoteSource {
        pub scope: String,
        pub dir: PathBuf,
        pub agent: Option<String>,
    }

    /// SWAP(MS1): `anthology_home(env)`.
    pub fn anthology_home(env: &Env) -> PathBuf {
        anthology_core::paths::anthology_home(env)
    }

    fn git_out(git: &dyn Git, env: &Env, cwd: &Path, args: &[&str]) -> Option<String> {
        let out = git.run(env, cwd, args)?;
        if out.code != 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let t = anthology_pycompat::str::strip_ws(&text);
        (!t.is_empty()).then(|| t.to_string())
    }

    /// SWAP(MS1): `repo_root(cwd)`.
    pub fn repo_root(cwd: &Path, env: &Env, git: &dyn Git) -> Option<PathBuf> {
        let top = git_out(git, env, cwd, &["rev-parse", "--show-toplevel"])?;
        Some(realpath(Path::new(&top)))
    }

    fn main_root(root: &Path, env: &Env, git: &dyn Git) -> PathBuf {
        let mut common = git_out(
            git,
            env,
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        if common.is_none() {
            common = git_out(git, env, root, &["rev-parse", "--git-common-dir"])
                .map(|rel| root.join(rel).to_string_lossy().into_owned());
        }
        if let Some(c) = common {
            let norm = anthology_pycompat::path::normpath(&c);
            let base = norm.rsplit('/').next().unwrap_or("");
            if base == ".git" {
                let parent = Path::new(&norm).parent().unwrap_or(Path::new("/"));
                return realpath(parent);
            }
        }
        let top = git_out(git, env, root, &["rev-parse", "--show-toplevel"]);
        realpath(&top.map_or_else(|| root.to_path_buf(), PathBuf::from))
    }

    fn normalize_remote(url: &str, base: &Path, env: &Env) -> String {
        let u = anthology_pycompat::str::strip_ws(url);
        let mut path: Option<String> = None;
        let mut key = String::new();
        if u.get(..7)
            .is_some_and(|h| h.eq_ignore_ascii_case("file://"))
        {
            path = Some(u[7..].to_string());
        } else if let Some((_, rest)) = u.split_once("://") {
            let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
            let authority = authority.rsplit('@').next().unwrap_or("");
            let (mut host, mut port) = (authority, "");
            if authority.matches(':').count() == 1 {
                (host, port) = authority.split_once(':').unwrap_or((authority, ""));
            }
            if ["22", "443", "80"].contains(&port) {
                port = "";
            }
            key = format!(
                "{host}{}{}/{tail}",
                if port.is_empty() { "" } else { ":" },
                port
            );
        } else if u.starts_with('/')
            || u.starts_with("./")
            || u.starts_with("../")
            || u.starts_with('~')
        {
            path = Some(u.to_string());
        } else {
            // `^(?:[^@/\s]+@)?([^:/\s]+):(.*)$`
            let scp = || -> Option<String> {
                let mut s = u;
                if let Some(at) = s.find('@') {
                    let user = &s[..at];
                    if !user.is_empty()
                        && !user
                            .chars()
                            .any(|c| c == '/' || c == '@' || c.is_whitespace())
                    {
                        s = &s[at + 1..];
                    }
                }
                let colon = s.find(':')?;
                let host = &s[..colon];
                if host.is_empty() || host.chars().any(|c| c == '/' || c.is_whitespace()) {
                    return None;
                }
                Some(format!("{host}/{}", &s[colon + 1..]))
            };
            match scp() {
                Some(k) => key = k,
                None => path = Some(u.to_string()),
            }
        }
        if let Some(p) = path {
            let expanded = if let Some(rest) = p.strip_prefix('~') {
                if rest.is_empty() || rest.starts_with('/') {
                    let home = anthology_core::paths::home(env);
                    format!("{}{rest}", home.to_string_lossy())
                } else {
                    p.clone()
                }
            } else {
                p.clone()
            };
            let full = realpath(&base.join(expanded));
            let full = full.to_string_lossy();
            let full = full.strip_suffix(".git").unwrap_or(&full);
            return format!("path:{full}");
        }
        let mut collapsed = String::new();
        for c in key.chars() {
            if c == '/' && collapsed.ends_with('/') {
                continue;
            }
            collapsed.push(c);
        }
        let k = collapsed.trim_end_matches('/');
        let k = k.strip_suffix(".git").unwrap_or(k);
        anthology_pycompat::str::lower(k.trim_end_matches('/'))
    }

    /// SWAP(MS1): `repo_key(root)`.
    pub fn repo_key(root: &Path, env: &Env, git: &dyn Git) -> String {
        let mut names: Vec<String> = git_out(git, env, root, &["remote"])
            .map(|o| {
                anthology_pycompat::str::split_ws(&o)
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        if let Some(first) = names.first() {
            let name = if names.iter().any(|n| n == "origin") {
                "origin"
            } else {
                first.as_str()
            };
            let url = git_out(git, env, root, &["remote", "get-url", name]).or_else(|| {
                git_out(
                    git,
                    env,
                    root,
                    &["config", "--get", &format!("remote.{name}.url")],
                )
            });
            if let Some(url) = url {
                return normalize_remote(&url, root, env);
            }
        }
        format!("path:{}", main_root(root, env, git).to_string_lossy())
    }

    fn real_dir(p: &Path) -> bool {
        p.is_dir() && !p.is_symlink()
    }

    /// SWAP(MS1): `note_sources(root, home)` with a root.
    pub fn note_sources(root: &Path, home: &Path) -> Vec<NoteSource> {
        let mut out = Vec::new();
        let repo = root.join(".claude").join("memory");
        if real_dir(&repo) {
            out.push(NoteSource {
                scope: "repo".into(),
                dir: repo,
                agent: None,
            });
        }
        let agents = root.join(".claude").join("agent-memory");
        if real_dir(&agents)
            && let Ok(rd) = std::fs::read_dir(&agents)
        {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs {
                if real_dir(&d) {
                    let agent = d.file_name().map(|n| n.to_string_lossy().into_owned());
                    out.push(NoteSource {
                        scope: "agent".into(),
                        dir: d,
                        agent,
                    });
                }
            }
        }
        let personal = home.join("personal-memory");
        if real_dir(&personal) {
            out.push(NoteSource {
                scope: "personal".into(),
                dir: personal,
                agent: None,
            });
        }
        out
    }
}

pub mod approve {
    //! SWAP(MS1): `anthology_memory::approve`.
    use anthology_core::Env;
    use anthology_pycompat::json::{self, Value};

    /// SWAP(MS1): `is_approved(key)`.
    pub fn is_approved(key: &str, env: &Env) -> bool {
        let path = anthology_core::paths::anthology_home(env).join("approved-repos.json");
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return false;
        };
        let Ok(data) = json::loads(&text) else {
            return false;
        };
        let repos = data.as_object().and_then(|m| m.get("repos"));
        matches!(repos, Some(Value::Object(m)) if m.get(key).is_some())
    }
}

pub mod save {
    //! SWAP(MS2): `anthology_memory::save` (and SWAP(MS1): `render::regenerate_index`, without
    //! the budget).
    use std::fs;
    use std::path::{Path, PathBuf};

    use anthology_core::clock::{Date, today_local};
    use anthology_core::{Env, fsx, roster::ROSTER};
    use anthology_pycompat::pyerr::PyExc;
    use anthology_pycompat::str::{char_len, split_ws, strip_ws, unicode_category};

    use crate::privacy::{PrivacyDeps, SECRET_PATTERNS, describe, privacy_findings, search_any};

    /// SWAP(MS2): §3.6 `SaveError`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct SaveError(pub String);

    /// SWAP(MS2): §3.6 `Saved`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Saved {
        pub action: String,
        pub relative: String,
        pub root: PathBuf,
        pub index_warning: Option<String>,
    }

    const SAVE_TYPES: [&str; 3] = ["project", "feedback", "reference"];

    fn err<T>(m: impl Into<String>) -> Result<T, SaveError> {
        Err(SaveError(m.into()))
    }

    fn slug_ok(s: &str) -> bool {
        !s.is_empty()
            && s.split('-').all(|w| {
                !w.is_empty()
                    && w.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
    }

    fn check_line(label: &str, value: &str, limit: usize) -> Result<String, SaveError> {
        let v = strip_ws(value);
        if v.is_empty() {
            return err(format!("{label} must not be empty"));
        }
        if char_len(v) > limit {
            return err(format!("{label} is too long (at most {limit} characters)"));
        }
        if v.chars()
            .any(|c| matches!(unicode_category(c), "Cc" | "Cf" | "Zl" | "Zp"))
        {
            return err(format!(
                "{label} must be one line without control or invisible formatting characters"
            ));
        }
        Ok(v.to_string())
    }

    fn check_body(value: &str) -> Result<String, SaveError> {
        let v = value.replace("\r\n", "\n").replace('\r', "\n");
        if v.chars()
            .any(|c| unicode_category(c) == "Cc" && c != '\n' && c != '\t')
        {
            return err("body must not contain control characters (other than newline and tab)");
        }
        let mut rest = v.as_str();
        loop {
            let blank = rest.trim_start_matches([' ', '\t']);
            match blank.strip_prefix('\n') {
                Some(after) => rest = after,
                None => break,
            }
        }
        let rest = rest.trim_end_matches(anthology_pycompat::str::is_space);
        if rest.is_empty() {
            return err("body must not be empty");
        }
        if rest.len() > 16384 {
            return err("body is too long (at most 16384 bytes)");
        }
        Ok(format!("{rest}\n"))
    }

    fn refuse_secrets(texts: &[&str], env: &Env) -> Result<(), SaveError> {
        let joined = texts.join("\n");
        let patterns: Vec<(&str, bool)> = SECRET_PATTERNS.iter().map(|p| (p.1, p.2)).collect();
        if search_any(&patterns, &joined) {
            return err(
                "refusing to save: the note looks like it contains a secret \
                        (memory is committed to the repo)",
            );
        }
        for (var, value) in env.iter() {
            let (Some(var), Some(value)) = (var.to_str(), value.to_str()) else {
                continue;
            };
            let up = var.to_ascii_uppercase();
            let secret = ["TOKEN", "SECRET", "KEY", "PASSWORD", "PASSWD", "CREDENTIAL"]
                .iter()
                .any(|w| up.contains(w));
            if char_len(value) >= 8 && secret && joined.contains(value) {
                return err(format!(
                    "refusing to save: the note contains the value of ${var} \
                     (memory is committed to the repo)"
                ));
            }
        }
        Ok(())
    }

    fn privacy_check(texts: &[&str], env: &Env) -> Result<(), SaveError> {
        let deps = PrivacyDeps::from_env(env);
        for (label, text) in ["slug", "name", "description", "body"].iter().zip(texts) {
            let findings = privacy_findings(text, None, None, &deps);
            if !findings.is_empty() {
                return err(format!(
                    "refusing to save: the {label} contains personal or private data ({}); \
                     memory is committed to the repo, so keep that in personal memory",
                    describe(&findings)
                ));
            }
        }
        Ok(())
    }

    fn ensure_real_dir(path: &Path, rel: &str) -> Result<(), SaveError> {
        if fs::symlink_metadata(path).is_err()
            && let Err(e) =
                std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o755)
                    .create(path)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            let exc = PyExc::from(e);
            return err(format!(
                "could not create {rel}: {}",
                anthology_pycompat::errno_tables::strerror(match exc {
                    PyExc::OSError { errno, .. } => errno,
                    _ => 5,
                })
            ));
        }
        if !fsx::is_real_dir(path) {
            return err(format!(
                "refusing to write: {rel} is a symlink or not a folder"
            ));
        }
        Ok(())
    }

    fn one_line(t: &str) -> String {
        split_ws(t).join(" ")
    }

    fn norm_type(t: &str) -> String {
        anthology_pycompat::str::lower(strip_ws(t.split('#').next().unwrap_or("")))
    }

    /// SWAP(MS1): `regenerate_index(agent_dir, exclude_types=("user",))` without the budget.
    fn regenerate_index(dir: &Path) -> Result<bool, PyExc> {
        let mut notes = Vec::new();
        let mut names: Vec<String> = fs::read_dir(dir)?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            if !name.ends_with(".md") || name == "MEMORY.md" {
                continue;
            }
            let p = dir.join(&name);
            if !fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_file()) {
                continue;
            }
            if let Some(info) = super::notes::parse_note(&p) {
                notes.push((info, name));
            }
        }
        if notes.is_empty() {
            return Ok(false);
        }
        const GENERATED: &str = "<!-- Generated by anthology from the notes' frontmatter; edit the notes, not this file. -->";
        notes.retain(|(n, _)| norm_type(n.kind.as_deref().unwrap_or("")) != "user");
        notes.sort_by(|a, b| (&a.0.name, &a.1).cmp(&(&b.0.name, &b.1)));
        let order = ["user", "feedback", "project", "reference"];
        let mut types: Vec<String> = Vec::new();
        for (n, _) in &notes {
            let t = n.kind.clone().unwrap_or_else(|| "other".into());
            if !types.contains(&t) {
                types.push(t);
            }
        }
        types.sort_by_key(|t| match order.iter().position(|o| o == t) {
            Some(i) => (i, String::new()),
            None => (order.len(), t.clone()),
        });
        let mut lines = vec![GENERATED.to_string()];
        for t in &types {
            lines.push(format!("## {t}"));
            let mut items: Vec<&(super::notes::NoteInfo, String)> = notes
                .iter()
                .filter(|(n, _)| n.kind.as_deref().unwrap_or("other") == t)
                .collect();
            let mut dated: Vec<_> = items
                .iter()
                .copied()
                .filter(|(n, _)| n.updated.is_some())
                .collect();
            dated.sort_by(|a, b| b.0.updated.cmp(&a.0.updated));
            items.retain(|(n, _)| n.updated.is_none());
            for (n, file) in dated.into_iter().chain(items) {
                let mut line = format!(
                    "- [{}]({})",
                    one_line(&n.name),
                    anthology_pycompat::quote::quote(file, "/")
                );
                let desc = one_line(&n.description);
                if !desc.is_empty() {
                    line.push_str(" — ");
                    line.push_str(&desc);
                }
                if char_len(&line) > 200 {
                    line = format!("{}…", anthology_pycompat::str::char_slice(&line, 0, 199));
                }
                lines.push(line);
            }
        }
        let data = format!("{}\n", lines.join("\n")).into_bytes();
        let target = dir.join("MEMORY.md");
        if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_file())
            && fsx::read_regular(&target, None).is_ok_and(|old| old == data)
        {
            return Ok(false);
        }
        fsx::atomic_write(&target, &data, 0o644, ".tmp-", ".part")?;
        Ok(true)
    }

    /// SWAP(MS2): `save_note(...)`.
    pub fn save_note(
        root: &Path,
        agent: &str,
        kind: &str,
        slug: &str,
        name: &str,
        description: &str,
        body: &str,
        replace: bool,
        today: Option<Date>,
        env: &Env,
    ) -> Result<Saved, SaveError> {
        if !ROSTER.contains(&agent) {
            return err(format!("unknown agent; use one of: {}", ROSTER.join(", ")));
        }
        if !SAVE_TYPES.contains(&kind) {
            return err(format!("type must be one of: {}", SAVE_TYPES.join(", ")));
        }
        if !(slug_ok(slug) && char_len(slug) <= 64) {
            return err(
                "slug must be lowercase words joined by single dashes (a-z, 0-9), \
                        at most 64 characters",
            );
        }
        let name = check_line("name", name, 120)?;
        let description = check_line("description", description, 300)?;
        let body = check_body(body)?;
        let texts = [slug, name.as_str(), description.as_str(), body.as_str()];
        refuse_secrets(&texts, env)?;
        privacy_check(&texts, env)?;
        let mut slug = slug;
        if let Some(rest) = slug.strip_prefix(&format!("{kind}-"))
            && !rest.is_empty()
        {
            slug = rest;
        }
        if !root.is_dir() {
            return err(format!("not a folder: {}", root.display()));
        }
        let root = anthology_pycompat::path::realpath(root);
        let agent_rel = format!(".claude/agent-memory/anthology-{agent}");
        let agent_dir = root.join(&agent_rel);
        for rel in [".claude", ".claude/agent-memory", agent_rel.as_str()] {
            ensure_real_dir(&root.join(rel), rel)?;
        }
        if anthology_pycompat::path::realpath(&agent_dir) != agent_dir {
            return err(format!(
                "refusing to write: {agent_rel} is a symlink or not a folder"
            ));
        }
        let relative = format!("{agent_rel}/{kind}-{slug}.md");
        let path = root.join(&relative);
        let q = |v: &str| format!("'{}'", v.replace('\'', "''"));
        let text = format!(
            "---\nname: {}\ndescription: {}\nmetadata:\n  type: {kind}\n  modified: {}\n---\n\n{body}",
            q(&name),
            q(&description),
            today.unwrap_or_else(today_local).iso()
        );
        let mut action = "created";
        let written = if replace {
            match fs::symlink_metadata(&path) {
                Ok(m) if !m.file_type().is_file() => {
                    return err(format!(
                        "refusing to write: {relative} is a symlink or not a regular file"
                    ));
                }
                Ok(_) => action = "replaced",
                Err(_) => {}
            }
            fsx::atomic_write(&path, text.as_bytes(), 0o644, ".tmp-", ".part")
        } else {
            match fsx::create_excl(&path, text.as_bytes(), 0o644, true) {
                Err(PyExc::OSError { errno: 17, .. }) => {
                    return err(format!(
                        "{relative} already exists; pass --replace to overwrite it"
                    ));
                }
                other => other,
            }
        };
        if let Err(e) = written {
            return err(match e {
                PyExc::OSError { errno, .. } => format!(
                    "could not write {relative}: {}",
                    anthology_pycompat::errno_tables::strerror(errno)
                ),
                other => other.py_str(),
            });
        }
        let index_warning = regenerate_index(&agent_dir).err().map(|e| e.py_str());
        Ok(Saved {
            action: action.into(),
            relative,
            root,
            index_warning,
        })
    }
}
