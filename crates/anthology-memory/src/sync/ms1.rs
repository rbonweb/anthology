//! SWAP(MS1): private stand-ins for the MS1 items `sync` and `save` use, with MS1's plan §3.6
//! signatures (plus the four items MS2 asked MS1 to publish: `render_index`, `GENERATED`,
//! `list_notes` and `approve::ensure_union_attrs`). The whole file is deleted when `port-ms1`
//! merges: `sync.rs` and `save.rs` then import `crate::{notes, repo, approve, render}` instead.
//! Ports of `memory_index.parse_text` and `memory_sync`'s identity, approval and index code,
//! close enough for MS2's tests; MS1's versions are the reviewed ones.
#![allow(dead_code, reason = "stand-in: not every MS1 item has an MS2 caller")]

use std::fs;
use std::path::{Path, PathBuf};

use anthology_core::git::Git;
use anthology_core::{Env, clock, fsx, paths};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::{path as pypath, quote, str as pystr};

// --- notes (MS1) ---------------------------------------------------------------------------

/// SWAP(MS1): `notes::Relations`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relations {
    pub supersedes: Vec<String>,
    pub depends_on: Vec<String>,
    pub relates_to: Vec<String>,
}

/// SWAP(MS1): `notes::NoteInfo`. `kind` is `None` where Python's `type` is its default `other`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteInfo {
    pub name: String,
    pub description: String,
    pub kind: Option<String>,
    pub updated: Option<String>,
    pub relations: Relations,
    pub body: String,
}

/// SWAP(MS1): `notes::USER_REASON`.
pub const USER_REASON: &str = "type: user notes are personal; they never go into a repo";
/// SWAP(MS1): `notes::REPO_EXCLUDE`.
pub const REPO_EXCLUDE: &[&str] = &["user"];

enum Y {
    S(String),
    L(Vec<String>),
    M(Vec<(String, Y)>),
}

fn scalar(value: &str) -> String {
    let v = pystr::strip_ws(value);
    let chars: Vec<char> = v.chars().collect();
    if chars.len() >= 2
        && chars[0] == chars[chars.len() - 1]
        && (chars[0] == '\'' || chars[0] == '"')
    {
        let inner: String = chars[1..chars.len() - 1].iter().collect();
        if chars[0] == '\'' {
            return inner.replace("''", "'");
        }
        return inner
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    v.to_string()
}

fn is_item(t: &str) -> bool {
    t == "-" || t.starts_with("- ")
}

fn parse_block(
    lines: &[(usize, String)],
    mut i: usize,
    indent: usize,
    depth: usize,
) -> Option<(Y, usize)> {
    if depth > 200 {
        return None;
    }
    if is_item(&lines[i].1) {
        let mut items = Vec::new();
        while i < lines.len() && lines[i].0 == indent && is_item(&lines[i].1) {
            items.push(scalar(&lines[i].1[1..]));
            i += 1;
        }
        return Some((Y::L(items), i));
    }
    let mut out: Vec<(String, Y)> = Vec::new();
    let set = |out: &mut Vec<(String, Y)>, k: String, v: Y| match out
        .iter_mut()
        .find(|(key, _)| *key == k)
    {
        Some(slot) => slot.1 = v,
        None => out.push((k, v)),
    };
    while i < lines.len() {
        let (ind, text) = (&lines[i].0, &lines[i].1);
        if *ind < indent {
            break;
        }
        if *ind > indent || is_item(text) {
            i += 1;
            continue;
        }
        let Some((key, val)) = text.split_once(':') else {
            i += 1;
            continue;
        };
        i += 1;
        let key = scalar(key);
        let val = pystr::strip_ws(val);
        if val.is_empty() {
            let nxt = lines.get(i);
            if let Some(n) = nxt.filter(|n| n.0 > indent || (n.0 == indent && is_item(&n.1))) {
                let (v, j) = parse_block(lines, i, n.0, depth + 1)?;
                set(&mut out, key, v);
                i = j;
            } else {
                set(&mut out, key, Y::S(String::new()));
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
            set(&mut out, key, Y::S(parts.join(" ")));
        } else if val.starts_with('[') {
            if val.ends_with(']') && val.chars().count() >= 2 {
                let inner = &val[1..val.len() - 1];
                let items = inner
                    .split(',')
                    .filter(|p| !pystr::strip_ws(p).is_empty())
                    .map(scalar)
                    .collect();
                set(&mut out, key, Y::L(items));
            } else {
                set(&mut out, key, Y::S(val.to_string()));
            }
        } else {
            set(&mut out, key, Y::S(scalar(val)));
        }
    }
    Some((Y::M(out), i))
}

fn get<'a>(m: &'a [(String, Y)], k: &str) -> Option<&'a Y> {
    m.iter().find(|(key, _)| key == k).map(|(_, v)| v)
}

fn text_of(v: Option<&Y>) -> String {
    match v {
        Some(Y::S(s)) => pystr::strip_ws(s).to_string(),
        _ => String::new(),
    }
}

fn front_end(rest: &str) -> Option<(usize, usize)> {
    let mut start = 0;
    for line in rest.split('\n') {
        if let Some(tail) = line.strip_prefix("---")
            && tail.chars().all(|c| c == ' ' || c == '\t')
        {
            return Some((start, start + line.len()));
        }
        start += line.len() + 1;
    }
    None
}

/// SWAP(MS1): `notes::parse_text` (`memory_index.parse_text`).
pub fn parse_text(text: &str, stem: &str) -> NoteInfo {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let mut info = NoteInfo {
        name: stem.to_string(),
        body: text.clone(),
        ..NoteInfo::default()
    };
    let Some(rest) = text.strip_prefix("---\n") else {
        return info;
    };
    let Some((s, e)) = front_end(rest) else {
        return info;
    };
    info.body = rest[e..].trim_start_matches('\n').to_string();
    let mut lines = Vec::new();
    for raw in rest[..s].split('\n') {
        let stripped = pystr::strip_ws(raw);
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start_matches(' ').len();
        lines.push((indent, stripped.to_string()));
    }
    if lines.is_empty() {
        return info;
    }
    let Some((Y::M(data), _)) = parse_block(&lines, 0, lines[0].0, 0) else {
        return info;
    };
    let empty = Vec::new();
    let meta = match get(&data, "metadata") {
        Some(Y::M(m)) => m,
        _ => &empty,
    };
    let name = text_of(get(&data, "name"));
    if !name.is_empty() {
        info.name = name;
    }
    info.description = text_of(get(&data, "description"));
    let kind = [text_of(get(meta, "type")), text_of(get(&data, "type"))]
        .into_iter()
        .find(|t| !t.is_empty());
    info.kind = kind;
    let updated = [
        text_of(get(meta, "modified")),
        text_of(get(meta, "updated")),
        text_of(get(&data, "modified")),
        text_of(get(&data, "updated")),
    ]
    .into_iter()
    .find(|t| !t.is_empty());
    info.updated = updated;
    info
}

/// SWAP(MS1): `notes::parse_note`.
pub fn parse_note(path: &Path) -> Option<NoteInfo> {
    let data = fs::read(path).ok()?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(parse_text(&String::from_utf8_lossy(&data), &stem))
}

/// SWAP(MS1): `notes::note_info` (`memory_sync.note_info`).
pub fn note_info(name: &str, data: &[u8]) -> NoteInfo {
    let stem = name.strip_suffix(".md").unwrap_or(name);
    parse_text(&String::from_utf8_lossy(data), stem)
}

/// SWAP(MS1): `memory_sync.normalize_type`.
pub fn normalize_type(value: &str) -> String {
    let head = value.split('#').next().unwrap_or("");
    pystr::lower(pystr::strip_ws(head))
}

/// SWAP(MS1): `notes::is_user_note`.
pub fn is_user_note(info: &NoteInfo) -> bool {
    normalize_type(info.kind.as_deref().unwrap_or("other")) == "user"
}

// --- repo (MS1) ----------------------------------------------------------------------------

fn git_out(git: &dyn Git, env: &Env, cwd: &Path, args: &[&str]) -> Option<String> {
    let out = git.run(env, cwd, args)?;
    if out.code != 0 {
        return None;
    }
    let s = pystr::strip_ws(&String::from_utf8_lossy(&out.stdout)).to_string();
    (!s.is_empty()).then_some(s)
}

/// SWAP(MS1): `repo::repo_root`.
pub fn repo_root(cwd: &Path, env: &Env, git: &dyn Git) -> Option<PathBuf> {
    let top = git_out(git, env, cwd, &["rev-parse", "--show-toplevel"])?;
    Some(pypath::realpath(Path::new(&top)))
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
            .map(|rel| pypath::join(&root.to_string_lossy(), &rel));
    }
    if let Some(c) = common {
        let n = pypath::normpath(&c);
        let p = Path::new(&n);
        if p.file_name().is_some_and(|b| b == ".git") {
            return pypath::realpath(p.parent().unwrap_or(Path::new("/")));
        }
    }
    let top = git_out(git, env, root, &["rev-parse", "--show-toplevel"]);
    pypath::realpath(Path::new(
        &top.unwrap_or_else(|| root.to_string_lossy().into_owned()),
    ))
}

fn normalize_remote(url: &str, base: &Path, env: &Env) -> String {
    let u = pystr::strip_ws(url);
    let mut path: Option<String> = None;
    let mut key = String::new();
    if pystr::lower(u).starts_with("file://") {
        path = Some(u.chars().skip(7).collect());
    } else if let Some((_, rest)) = u.split_once("://") {
        let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let (mut host, mut port) = (authority, "");
        if authority.matches(':').count() == 1 {
            (host, port) = authority.split_once(':').unwrap_or((authority, ""));
        }
        if ["22", "443", "80"].contains(&port) {
            port = "";
        }
        let _ = &mut host;
        key = format!(
            "{host}{}/{tail}",
            if port.is_empty() {
                String::new()
            } else {
                format!(":{port}")
            }
        );
    } else if ["/", "./", "../", "~"].iter().any(|p| u.starts_with(p)) {
        path = Some(u.to_string());
    } else {
        // `^(?:[^@/\s]+@)?([^:/\s]+):(.*)$`
        let scp = (|| {
            let (user, rest) = match u.split_once('@') {
                Some((a, b))
                    if !a.is_empty() && !a.contains('/') && !a.chars().any(pystr::is_space) =>
                {
                    (Some(a), b)
                }
                _ => (None, u),
            };
            let _ = user;
            let (host, tail) = rest.split_once(':')?;
            if host.is_empty()
                || host.contains('/')
                || host.chars().any(pystr::is_space)
                || tail.contains('\n')
            {
                return None;
            }
            Some(format!("{host}/{tail}"))
        })();
        match scp {
            Some(k) => key = k,
            None => path = Some(u.to_string()),
        }
    }
    if let Some(p) = path {
        let expanded = match p.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", paths::home(env).to_string_lossy())
            }
            _ => p,
        };
        let full = pypath::realpath(Path::new(&pypath::join(&base.to_string_lossy(), &expanded)));
        let mut full = full.to_string_lossy().into_owned();
        if let Some(f) = full.strip_suffix(".git") {
            full = f.to_string();
        }
        return format!("path:{full}");
    }
    let mut collapsed = String::new();
    for c in key.chars() {
        if !(c == '/' && collapsed.ends_with('/')) {
            collapsed.push(c);
        }
    }
    let mut k = collapsed.trim_end_matches('/').to_string();
    if let Some(s) = k.strip_suffix(".git") {
        k = s.to_string();
    }
    pystr::lower(k.trim_end_matches('/'))
}

/// SWAP(MS1): `repo::repo_key`.
pub fn repo_key(root: &Path, env: &Env, git: &dyn Git) -> String {
    let out = git_out(git, env, root, &["remote"]);
    let mut names: Vec<String> = out
        .map(|o| pystr::split_ws(&o).into_iter().map(String::from).collect())
        .unwrap_or_default();
    names.sort();
    if !names.is_empty() {
        let name = if names.iter().any(|n| n == "origin") {
            "origin".to_string()
        } else {
            names[0].clone()
        };
        let url = git_out(git, env, root, &["remote", "get-url", &name]).or_else(|| {
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

fn encode_project_path(p: &Path) -> String {
    pypath::realpath(p)
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// SWAP(MS1): `repo::legacy_dirs`.
pub fn legacy_dirs(root: &Path, config: &Path, env: &Env, git: &dyn Git) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for base in [root.to_path_buf(), main_root(root, env, git)] {
        let base = pypath::realpath(&base);
        if seen.contains(&base) {
            continue;
        }
        out.push(
            config
                .join("projects")
                .join(encode_project_path(&base))
                .join("memory"),
        );
        seen.push(base);
    }
    out
}

/// SWAP(MS1): `repo::UnsafeTarget`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsafeTarget {
    pub path: PathBuf,
    pub reason: String,
}

/// SWAP(MS1): `repo::unsafe_dir` (`memory_sync._unsafe_dir`).
pub fn unsafe_dir(path: &Path) -> Option<UnsafeTarget> {
    let meta = fs::symlink_metadata(path).ok()?;
    let bad = meta.file_type().is_symlink() || !path.is_dir();
    bad.then(|| UnsafeTarget {
        path: path.to_path_buf(),
        reason: "a symlink or not a folder".into(),
    })
}

// --- approve (MS1) -------------------------------------------------------------------------

fn approved_path(env: &Env) -> PathBuf {
    paths::anthology_home(env).join("approved-repos.json")
}

/// SWAP(MS1): `approve::load_approved` (the `repos` object).
pub fn load_approved(env: &Env) -> Map {
    let parsed = fs::read(approved_path(env))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| json::loads(&t).ok());
    parsed
        .and_then(|v| v.into_object())
        .and_then(|mut m| m.remove("repos"))
        .and_then(|r| r.into_object())
        .unwrap_or_default()
}

/// SWAP(MS1): `approve::is_approved`.
pub fn is_approved(key: &str, env: &Env) -> bool {
    load_approved(env).contains_key(key)
}

/// SWAP(MS1): `approve::ensure_union_attrs`.
pub fn ensure_union_attrs(root: &Path) -> Result<bool, PyExc> {
    let path = root.join(".gitattributes");
    let existing = String::from_utf8_lossy(&fsx::read_regular(&path, None)?).into_owned();
    let have: Vec<String> = existing
        .lines()
        .map(|l| pystr::split_ws(l).join(" "))
        .collect();
    let mut changed = false;
    for line in [
        ".claude/memory/MEMORY.md merge=union",
        ".claude/agent-memory/*/MEMORY.md merge=union",
    ] {
        if !have.iter().any(|h| h == line) {
            fsx::append_line(&path, line)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// SWAP(MS1): `approve::approve`.
pub fn approve(root: &Path, env: &Env, git: &dyn Git) -> Result<String, PyExc> {
    let key = repo_key(root, env, git);
    let mut repos = load_approved(env);
    if !repos.contains_key(&key) {
        let mut entry = Map::new();
        entry.insert("approved".into(), Value::Str(clock::today_local().iso()));
        repos.insert(key.clone(), Value::Object(entry));
        let mut data = Map::new();
        data.insert("version".into(), json::loads("1").expect("int"));
        data.insert("repos".into(), Value::Object(repos));
        let text = json::dumps(&Value::Object(data), &json::INDENT2_SORTED) + "\n";
        fsx::atomic_write(
            &approved_path(env),
            text.as_bytes(),
            0o644,
            ".tmp-",
            ".part",
        )?;
    }
    ensure_union_attrs(root)?;
    Ok(key)
}

// --- render (MS1) --------------------------------------------------------------------------

/// SWAP(MS1): `render::Budget`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub lines: usize,
    pub bytes: usize,
}
/// SWAP(MS1): `render::DEFAULT_BUDGET`.
pub const DEFAULT_BUDGET: Budget = Budget {
    lines: 150,
    bytes: 20000,
};
/// SWAP(MS1): `render::CONTEXT_SECTION_BUDGET`.
pub const CONTEXT_SECTION_BUDGET: Budget = Budget {
    lines: 40,
    bytes: 2500,
};
/// SWAP(MS1): `render::GENERATED`.
pub const GENERATED: &str =
    "<!-- Generated by anthology from the notes' frontmatter; edit the notes, not this file. -->";

/// One listed note: its file name and parsed info.
pub struct Listed {
    pub file: String,
    pub info: NoteInfo,
}

/// SWAP(MS1): `render::list_notes` (`memory_sync._list_notes`).
pub fn list_notes(mem_dir: &Path) -> Vec<Listed> {
    let mut out = Vec::new();
    if !mem_dir.is_dir() {
        return out;
    }
    let Ok(rd) = fs::read_dir(mem_dir) else {
        return out;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !name.ends_with(".md") || name == "MEMORY.md" {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        if let Some(info) = parse_note(&e.path()) {
            out.push(Listed { file: name, info });
        }
    }
    out
}

fn entry_line(n: &Listed) -> String {
    let mut line = format!(
        "- [{}]({})",
        pystr::split_ws(&n.info.name).join(" "),
        quote::quote(&n.file, "/")
    );
    let desc = pystr::split_ws(&n.info.description).join(" ");
    if !desc.is_empty() {
        line.push_str(" — ");
        line.push_str(&desc);
    }
    if pystr::char_len(&line) > 200 {
        line = format!("{}…", pystr::char_slice(&line, 0, 199));
    }
    line
}

fn blen(s: &str) -> usize {
    s.len() + 1
}

fn pointer(hidden: usize, suffix: &str) -> String {
    format!("- {hidden} more — use `anthology-mem search`{suffix}")
}

fn collapsed_pointer(hidden: usize, suffix: &str) -> String {
    format!("- {hidden} more in other types — use `anthology-mem search`{suffix}")
}

const TYPE_ORDER: [&str; 4] = ["user", "feedback", "project", "reference"];

/// SWAP(MS1): `render::render_index`.
pub fn render_index(
    mem_dir: &Path,
    budget: Budget,
    suffix: &str,
    exclude_types: &[&str],
) -> Result<Option<String>, PyExc> {
    let notes = list_notes(mem_dir);
    if notes.is_empty() {
        return Ok(None);
    }
    let excluded: Vec<String> = exclude_types.iter().map(|t| normalize_type(t)).collect();
    let mut notes: Vec<Listed> = notes
        .into_iter()
        .filter(|n| !excluded.contains(&normalize_type(n.info.kind.as_deref().unwrap_or("other"))))
        .collect();
    if notes.is_empty() {
        return Ok(Some(format!("{GENERATED}\n")));
    }
    notes.sort_by(|a, b| (&a.info.name, &a.file).cmp(&(&b.info.name, &b.file)));
    let mut by_type: Vec<(String, Vec<&Listed>)> = Vec::new();
    for n in &notes {
        let t = n.info.kind.clone().unwrap_or_else(|| "other".into());
        match by_type.iter_mut().find(|(k, _)| *k == t) {
            Some(slot) => slot.1.push(n),
            None => by_type.push((t, vec![n])),
        }
    }
    let rank = |t: &str| match TYPE_ORDER.iter().position(|x| *x == t) {
        Some(i) => (i, String::new()),
        None => (TYPE_ORDER.len(), t.to_string()),
    };
    by_type.sort_by_key(|(t, _)| rank(t));
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (t, items) in by_type {
        let mut dated: Vec<&Listed> = items
            .iter()
            .copied()
            .filter(|n| n.info.updated.is_some())
            .collect();
        dated.sort_by(|a, b| b.info.updated.cmp(&a.info.updated));
        let undated = items.iter().copied().filter(|n| n.info.updated.is_none());
        groups.push((
            t,
            dated.into_iter().chain(undated).map(entry_line).collect(),
        ));
    }
    let cost = |g: &(String, Vec<String>), k: usize| {
        let lines = 1 + k + usize::from(k < g.1.len());
        let mut size =
            blen(&format!("## {}", g.0)) + g.1[..k].iter().map(|e| blen(e)).sum::<usize>();
        if k < g.1.len() {
            size += blen(&pointer(g.1.len() - k, suffix));
        }
        (lines, size)
    };
    let fits = |l: usize, b: usize| l <= budget.lines && b <= budget.bytes;
    let mut counts: Vec<usize> = groups.iter().map(|g| g.1.len()).collect();
    let (mut base_l, mut base_b) = (1usize, blen(GENERATED));
    let costs: Vec<(usize, usize)> = groups
        .iter()
        .zip(&counts)
        .map(|(g, k)| cost(g, *k))
        .collect();
    let total_l = base_l + costs.iter().map(|c| c.0).sum::<usize>();
    let total_b = base_b + costs.iter().map(|c| c.1).sum::<usize>();
    let mut collapsed = None;
    if !fits(total_l, total_b) {
        let zero: Vec<(usize, usize)> = groups.iter().map(|g| cost(g, 0)).collect();
        let mut keep = groups.len();
        while keep > 0 {
            let hidden: usize = groups[keep..].iter().map(|g| g.1.len()).sum();
            let extra = if keep < groups.len() {
                (1, blen(&collapsed_pointer(hidden, suffix)))
            } else {
                (0, 0)
            };
            if fits(
                base_l + extra.0 + zero[..keep].iter().map(|c| c.0).sum::<usize>(),
                base_b + extra.1 + zero[..keep].iter().map(|c| c.1).sum::<usize>(),
            ) {
                break;
            }
            keep -= 1;
        }
        if keep < groups.len() {
            let hidden: usize = groups[keep..].iter().map(|g| g.1.len()).sum();
            let c = collapsed_pointer(hidden, suffix);
            base_l += 1;
            base_b += blen(&c);
            collapsed = Some(c);
            groups.truncate(keep);
        }
        counts = vec![0; groups.len()];
        let mut costs: Vec<(usize, usize)> = groups.iter().map(|g| cost(g, 0)).collect();
        let mut tl = base_l + costs.iter().map(|c| c.0).sum::<usize>();
        let mut tb = base_b + costs.iter().map(|c| c.1).sum::<usize>();
        let mut live: Vec<usize> = (0..groups.len()).collect();
        while !live.is_empty() {
            for gi in live.clone() {
                if counts[gi] >= groups[gi].1.len() {
                    live.retain(|x| *x != gi);
                    continue;
                }
                let new = cost(&groups[gi], counts[gi] + 1);
                let nl = tl - costs[gi].0 + new.0;
                let nb = tb - costs[gi].1 + new.1;
                if !fits(nl, nb) {
                    live.retain(|x| *x != gi);
                    continue;
                }
                counts[gi] += 1;
                costs[gi] = new;
                tl = nl;
                tb = nb;
            }
        }
    }
    let mut lines = vec![GENERATED.to_string()];
    for (g, k) in groups.iter().zip(&counts) {
        lines.push(format!("## {}", g.0));
        lines.extend(g.1[..*k].iter().cloned());
        if *k < g.1.len() {
            lines.push(pointer(g.1.len() - k, suffix));
        }
    }
    if let Some(c) = collapsed {
        lines.push(c);
    }
    Ok(Some(lines.join("\n") + "\n"))
}

/// SWAP(MS1): `render::regenerate_index`.
pub fn regenerate_index(
    mem_dir: &Path,
    budget: Budget,
    exclude_types: &[&str],
) -> Result<bool, PyExc> {
    let Some(text) = render_index(mem_dir, budget, "", exclude_types)? else {
        return Ok(false);
    };
    let target = mem_dir.join("MEMORY.md");
    let regular = fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_file());
    if regular && fsx::read_regular(&target, None).is_ok_and(|b| b == text.as_bytes()) {
        return Ok(false);
    }
    fsx::atomic_write(&target, text.as_bytes(), 0o644, ".tmp-", ".part")?;
    Ok(true)
}
