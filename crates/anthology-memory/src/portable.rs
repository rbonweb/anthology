//! Memory export and import (MX1): `memory_portable.py` and `anthology memory export|import`
//! (GC-12, GC-5, GC-4). Import validation, which never extracts the archive, is [`load`].
//!
//! A plan is the JSON object `plan_import` returns (`json::Value::Object`, the Python dict,
//! key for key): `apply_import` takes any value, as Python takes any object, and re-validates
//! it, so hand-forged plans are tested the way Python tests them.
//!
//! Python reads `os.environ` where these functions take an [`Env`]: `HOME` for the privacy
//! scan, and the environment git runs with. `apply_import` has no environment argument
//! (Python's reads `os.environ`), so [`apply_import_env`] takes one; `apply_import` is it with
//! an empty environment. Paths are shown as `pathlib` prints them.

pub mod load;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use anthology_core::git::Git;
use anthology_core::{Env, clock, fsx};
use anthology_pycompat::argparse::{self, Arg, Spec, Subcommands};
use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::path::realpath;
use anthology_pycompat::pyerr::PyExc;

use crate::privacy::{self, Finding, PrivacyDeps};
use crate::tar_write::{gzip, pax_archive};

pub use crate::{approve, notes, render, repo};

/// `memory_portable.SECRET_PATTERNS`: the one shared list (`memory_privacy`).
pub use crate::privacy::SECRET_PATTERNS;

/// `SCOPES`.
pub const SCOPES: [&str; 3] = ["personal", "repo", "agent"];

/// What the Python functions raise, by class.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// `ImportRejected(msg)`.
    Rejected(String),
    /// `ExportRefused(msg)`.
    Refused(String),
    /// `ValueError(msg)`.
    Value(String),
    /// An `OSError`.
    Os(PyExc),
}

impl From<load::Rejected> for Error {
    fn from(r: load::Rejected) -> Self {
        Error::Rejected(r.0)
    }
}

impl From<PyExc> for Error {
    fn from(e: PyExc) -> Self {
        Error::Os(e)
    }
}

fn rejected(msg: impl Into<String>) -> Error {
    Error::Rejected(msg.into())
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

/// `str(pathlib.PurePosixPath(p))`: repeated and trailing `/` and `.` parts dropped (`..` kept),
/// a leading `//` (exactly two) kept, an empty path `.`.
fn pathlib_str(p: &[u8]) -> Vec<u8> {
    let (root, rest): (&[u8], &[u8]) = if p.starts_with(b"//") && !p.starts_with(b"///") {
        (b"//", &p[2..])
    } else if p.starts_with(b"/") {
        (b"/", p)
    } else {
        (b"", p)
    };
    let parts: Vec<&[u8]> = rest
        .split(|&b| b == b'/')
        .filter(|s| !s.is_empty() && *s != b".")
        .collect();
    let mut out = root.to_vec();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `str(Path(p))`.
fn shown(p: &Path) -> String {
    String::from_utf8_lossy(&pathlib_str(p.as_os_str().as_bytes())).into_owned()
}

/// `Path(p)`, normalised as `pathlib` holds it.
fn pure(p: &Path) -> PathBuf {
    PathBuf::from(shown(p))
}

/// An `OSError` naming `path`, as `os.*(path)` raises it.
fn os_error(e: io::Error, path: &Path) -> PyExc {
    match PyExc::from(e) {
        PyExc::OSError { errno, .. } => PyExc::OSError {
            errno,
            filename: Some(path.to_string_lossy().into_owned()),
        },
        other => other,
    }
}

fn is_exists_error(e: &PyExc) -> bool {
    matches!(e, PyExc::OSError { errno: 17, .. })
}

fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// `p.is_dir() and not p.is_symlink()`.
fn real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// `Path(p).mkdir(parents=True, exist_ok=True)` (CPython 3.12 `pathlib`), so the error names
/// the folder that failed.
fn mkdir_parents(p: &Path) -> Result<(), PyExc> {
    match fs::create_dir(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = match p.parent() {
                Some(q) if !q.as_os_str().is_empty() => q,
                Some(_) => Path::new("."),
                None => return Err(os_error(e, p)),
            };
            if parent == p {
                return Err(os_error(e, p));
            }
            mkdir_parents(parent)?;
            match fs::create_dir(p) {
                Ok(()) => Ok(()),
                Err(_) if p.is_dir() => Ok(()),
                Err(e) => Err(os_error(e, p)),
            }
        }
        Err(_) if p.is_dir() => Ok(()),
        Err(e) => Err(os_error(e, p)),
    }
}

/// `str(today)[:10]`, or today's date when `None` (`_iso`).
fn iso(today: Option<&str>) -> String {
    match today {
        None => clock::today_local().iso(),
        Some(s) => s.chars().take(10).collect(),
    }
}

/// `_iso(plan.get("date"))`: `str(value)[:10]` of a JSON value.
fn iso_value(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => iso(None),
        Some(Value::Str(s)) => iso(Some(s)),
        Some(Value::Bool(b)) => iso(Some(if *b { "True" } else { "False" })),
        Some(other) => iso(Some(&json::dumps(other, &json::PY_DEFAULT))),
    }
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn int(n: usize) -> Value {
    Value::Int(Int::Small(n as i64))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn findings_json(findings: &[Finding]) -> Value {
    Value::Array(
        findings
            .iter()
            .map(|f| obj(vec![("kind", s(&f.kind)), ("line", int(f.line))]))
            .collect(),
    )
}

fn array_mut<'a>(m: &'a mut Map, key: &str) -> &'a mut Vec<Value> {
    m.get_mut(key)
        .and_then(Value::as_array_mut)
        .expect("the key holds a list")
}

// ---------------------------------------------------------------------------------------------
// Secret detection (reports labels only, never the matched text)
// ---------------------------------------------------------------------------------------------

/// `scan_secrets(text)`: sorted unique labels of the secret patterns found in `text`.
pub fn scan_secrets(text: &str) -> Vec<String> {
    let mut labels: BTreeSet<&str> = BTreeSet::new();
    for &(label, py, ignorecase) in SECRET_PATTERNS {
        if !labels.contains(label) && privacy::search_any(&[(py, ignorecase)], text) {
            labels.insert(label);
        }
    }
    labels.into_iter().map(str::to_string).collect()
}

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

/// `_flat_notes(directory)`: `[(filename, bytes)]` of the exportable notes in a folder (flat
/// `*.md`, no symlinks, UTF-8, sane names). A folder that cannot be listed is an `OSError`.
fn flat_notes(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, PyExc> {
    let mut out = Vec::new();
    if !real_dir(dir) {
        return Ok(out);
    }
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)
        .map_err(|e| os_error(e, dir))?
        .flatten()
        .collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !load::ok_note_name(&name) {
            continue;
        }
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(data) = fs::read(entry.path()) else {
            continue;
        };
        if std::str::from_utf8(&data).is_err() {
            continue;
        }
        out.push((name, data));
    }
    Ok(out)
}

/// `_inside_git_work_tree(path)`: a `.git` entry (of any kind) in the folder or above it.
fn inside_git_work_tree(path: &Path) -> bool {
    let mut probe = realpath(path);
    loop {
        if lexists(&probe.join(".git")) {
            return true;
        }
        match probe.parent() {
            Some(parent) if parent != probe => probe = parent.to_path_buf(),
            _ => return false,
        }
    }
}

/// `repo_root(cwd)` when that repo is approved (`repo_ok`).
fn approved_root(cwd: &Path, env: &Env, git: &dyn Git) -> Option<PathBuf> {
    let root = repo::repo_root(cwd, env, git)?;
    approve::is_approved(&repo::repo_key(&root, env, git), env).then_some(root)
}

/// `export(cwd, out_dir, today, env)`: `{"path", "counts", "secrets", "repo_included"}`.
pub fn export(
    cwd: &Path,
    out_dir: Option<&Path>,
    today: Option<&str>,
    env: &Env,
    git: &dyn Git,
) -> Result<Map, Error> {
    let date = iso(today);
    let home = pure(&repo::anthology_home(env));
    let out = match out_dir {
        None => {
            let out = home.join("exports");
            if !out.exists() {
                mkdir_parents(&home)?;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&out)
                    .map_err(|e| os_error(e, &out))?;
            }
            out
        }
        Some(o) => {
            if inside_git_work_tree(o) {
                return Err(Error::Refused(format!(
                    "export refuses to write personal notes inside a git repo: {}",
                    shown(o)
                )));
            }
            pure(o)
        }
    };
    let root = approved_root(cwd, env, git);
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let (mut personal, mut repo_n, mut agent_n) = (0, 0, 0);
    for (fname, data) in flat_notes(&home.join("personal-memory"))? {
        files.insert(format!("personal/{fname}"), data);
        personal += 1;
    }
    if let Some(root) = &root {
        for (fname, data) in flat_notes(&root.join(".claude").join("memory"))? {
            files.insert(format!("repo/memory/{fname}"), data);
            repo_n += 1;
        }
        let agents = root.join(".claude").join("agent-memory");
        if real_dir(&agents) {
            let mut dirs: Vec<PathBuf> = fs::read_dir(&agents)
                .map_err(|e| os_error(e, &agents))?
                .flatten()
                .map(|e| e.path())
                .collect();
            dirs.sort();
            for d in dirs {
                let Some(name) = d.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                    continue;
                };
                if load::name_ok(&name) && real_dir(&d) {
                    for (fname, data) in flat_notes(&d)? {
                        files.insert(format!("repo/agent-memory/{name}/{fname}"), data);
                        agent_n += 1;
                    }
                }
            }
        }
    }
    if files.values().map(|d| d.len() as u64).sum::<u64>() > load::MAX_TOTAL {
        return Err(Error::Value(format!(
            "the notes are too large to export (limit {} MiB)",
            load::MAX_TOTAL >> 20
        )));
    }
    let mut secrets = Vec::new();
    for (name, data) in &files {
        for label in scan_secrets(&String::from_utf8_lossy(data)) {
            secrets.push(obj(vec![("file", s(name)), ("pattern", s(&label))]));
        }
    }
    let counts = || {
        obj(vec![
            ("personal", int(personal)),
            ("repo", int(repo_n)),
            ("agent", int(agent_n)),
        ])
    };
    let manifest = obj(vec![
        ("format", Value::Int(Int::Small(load::FORMAT))),
        ("date", s(&date)),
        ("counts", counts()),
    ]);
    let mut text = json::dumps(&manifest, &json::INDENT2_SORTED);
    text.push('\n');
    files.insert("manifest.json".into(), text.into_bytes());
    let members: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    let blob = gzip(&pax_archive(&members));
    mkdir_parents(&out)?;
    let mut n = 1;
    let path = loop {
        let suffix = if n == 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        let path = out.join(format!("anthology-memory-{date}{suffix}.tar.gz"));
        match fsx::create_excl(&path, &blob, 0o666, false) {
            Ok(()) => break path,
            Err(e) if is_exists_error(&e) => n += 1,
            Err(e) => return Err(Error::Os(e)),
        }
    };
    let mut res = Map::new();
    res.insert("path".into(), s(&shown(&path)));
    res.insert("counts".into(), counts());
    res.insert("secrets".into(), Value::Array(secrets));
    res.insert("repo_included".into(), Value::Bool(root.is_some()));
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// Import: plan and apply
// ---------------------------------------------------------------------------------------------

/// `_location(scope, agent, home, root)`: the trusted base folder and the folders below it.
fn location(scope: &str, agent: Option<&str>, home: &Path, root: &Path) -> (PathBuf, Vec<String>) {
    match scope {
        "personal" => (pure(home), vec!["personal-memory".into()]),
        "repo" => (pure(root), vec![".claude".into(), "memory".into()]),
        _ => (
            pure(root),
            vec![
                ".claude".into(),
                "agent-memory".into(),
                agent.unwrap_or_default().to_string(),
            ],
        ),
    }
}

fn joined(base: &Path, parts: &[String]) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in parts {
        p.push(part);
    }
    p
}

/// `_entry_for(member)`: `(scope, agent, name)`.
fn entry_for(member: &str) -> (&'static str, Option<&str>, &str) {
    let parts: Vec<&str> = member.split('/').collect();
    if parts[0] == "personal" {
        ("personal", None, parts[1])
    } else if parts[1] == "memory" {
        ("repo", None, parts[2])
    } else {
        ("agent", Some(parts[2]), parts[3])
    }
}

/// `_is_plain_file(path)`: a regular file (followed) that is not a symlink.
fn is_plain_file(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())
}

/// `_same_as_existing(directory, name, data)`: identical to the note itself, or to an earlier
/// `.imported-*` copy of it. Reading the note itself may fail (`OSError`); a failure while
/// looking at the copies ends the search.
fn same_as_existing(dir: &Path, name: &str, data: &[u8]) -> Result<bool, PyExc> {
    let dest = dir.join(name);
    if is_plain_file(&dest) && fs::read(&dest).map_err(|e| os_error(e, &dest))? == data {
        return Ok(true);
    }
    let prefix = format!("{}.imported-", &name[..name.len() - 3]);
    let Ok(rd) = fs::read_dir(dir) else {
        return Ok(false);
    };
    let mut others: Vec<OsString> = Vec::new();
    for entry in rd {
        match entry {
            Ok(e) => others.push(e.file_name()),
            Err(_) => return Ok(false),
        }
    }
    others.sort();
    for other in others {
        let b = other.as_bytes();
        if b.starts_with(prefix.as_bytes()) && b.ends_with(b".md") {
            let p = dir.join(&other);
            if is_plain_file(&p) {
                match fs::read(&p) {
                    Ok(d) if d == data => return Ok(true),
                    Ok(_) => {}
                    Err(_) => return Ok(false),
                }
            }
        }
    }
    Ok(false)
}

/// The date that goes into `.imported-<date>` names, refused when it could leave the folder
/// or cut the name: a `/`, a NUL or a `..` (D-8: Python builds the name from any date).
fn checked_date(date: String) -> Result<String, Error> {
    if date.contains('/') || date.contains('\0') || date.contains("..") {
        return Err(rejected("malformed plan date"));
    }
    Ok(date)
}

/// `_imported_name(directory, name, date)`: `<stem>.imported-<date>[-N].md`, the first free.
fn imported_name(dir: &Path, name: &str, date: &str) -> String {
    let stem = &name[..name.len() - 3];
    let mut n = 1;
    loop {
        let suffix = if n == 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        let candidate = format!("{stem}.imported-{date}{suffix}.md");
        if !lexists(&dir.join(&candidate)) {
            return candidate;
        }
        n += 1;
    }
}

/// `_repo_block(name, data)`: `(findings, reason)` why a note may not be written into repo or
/// agent memory, or `([], None)`.
fn repo_block(name: &str, data: &[u8], pdeps: &PrivacyDeps) -> (Vec<Finding>, Option<String>) {
    if notes::is_user_note(&notes::note_info(name, data)) {
        return (Vec::new(), Some(notes::USER_REASON.to_string()));
    }
    let findings = privacy::privacy_findings(&String::from_utf8_lossy(data), None, None, pdeps);
    if findings.is_empty() {
        return (findings, None);
    }
    let reason = format!("the privacy scan found {}", privacy::describe(&findings));
    (findings, Some(reason))
}

/// `plan_import(archive, cwd, today, env)`: the plan object. Writes nothing.
pub fn plan_import(
    archive: &Path,
    cwd: &Path,
    today: Option<&str>,
    env: &Env,
    git: &dyn Git,
) -> Result<Map, Error> {
    let loaded = load::load_archive(archive)?;
    let home = pure(&repo::anthology_home(env));
    let root = approved_root(cwd, env, git);
    let date = checked_date(iso(today))?;
    let pdeps = PrivacyDeps::from_env(env);
    let mut plan = Map::new();
    plan.insert("archive".into(), s(&realpath(archive).to_string_lossy()));
    plan.insert("sha256".into(), s(&loaded.sha256));
    plan.insert("date".into(), s(&date));
    plan.insert("home".into(), s(&shown(&home)));
    plan.insert(
        "repo_root".into(),
        root.as_ref().map_or(Value::Null, |r| s(&shown(r))),
    );
    for key in [
        "new",
        "identical",
        "conflicts",
        "secrets",
        "skipped_repo",
        "blocked",
        "messages",
    ] {
        plan.insert(key.into(), Value::Array(Vec::new()));
    }
    let no_root = PathBuf::new();
    for (member, data) in &loaded.contents {
        if member == "manifest.json" {
            continue;
        }
        for label in scan_secrets(&String::from_utf8_lossy(data)) {
            let finding = obj(vec![("file", s(member)), ("pattern", s(&label))]);
            array_mut(&mut plan, "secrets").push(finding);
        }
        let (scope, agent, name) = entry_for(member);
        if scope != "personal" && root.is_none() {
            array_mut(&mut plan, "skipped_repo").push(s(member));
            continue;
        }
        let root = root.as_deref().unwrap_or(&no_root);
        let (base, parts) = location(scope, agent, &home, root);
        let mut directory = joined(&base, &parts);
        let mut dest = directory.join(name);
        let mut entry = Map::new();
        entry.insert("member".into(), s(member));
        entry.insert("scope".into(), s(scope));
        entry.insert("agent".into(), agent.map_or(Value::Null, s));
        entry.insert("name".into(), s(name));
        entry.insert("dest".into(), s(&shown(&dest)));
        if scope != "personal" && !(lexists(&dest) && same_as_existing(&directory, name, data)?) {
            let (findings, reason) = repo_block(name, data, &pdeps);
            if let Some(reason) = reason {
                let b = obj(vec![
                    ("file", s(member)),
                    ("findings", findings_json(&findings)),
                    ("reason", s(&reason)),
                ]);
                array_mut(&mut plan, "blocked").push(b);
                entry.insert("redirected".into(), s("personal"));
                let (base, parts) = location("personal", None, &home, root);
                directory = joined(&base, &parts);
                dest = directory.join(name);
                entry.insert("dest".into(), s(&shown(&dest)));
            }
        }
        let key = if !lexists(&dest) {
            "new"
        } else if same_as_existing(&directory, name, data)? {
            "identical"
        } else {
            "conflicts"
        };
        if key == "conflicts" {
            let imported = directory.join(imported_name(&directory, name, &date));
            entry.insert("imported_as".into(), s(&shown(&imported)));
        }
        array_mut(&mut plan, key).push(Value::Object(entry));
    }
    let skipped = array_mut(&mut plan, "skipped_repo").len();
    let blocked = array_mut(&mut plan, "blocked").len();
    let messages = array_mut(&mut plan, "messages");
    if skipped > 0 {
        messages.push(s(&format!(
            "this folder is not an approved repo (run /anthology:setup in it): {skipped} repo \
             note(s) skipped"
        )));
    }
    if blocked > 0 {
        messages.push(s(&format!(
            "{blocked} repo note(s) are user notes or look private (privacy scan); they go to \
             personal memory instead of the repo"
        )));
    }
    Ok(plan)
}

/// `_check_chain(base, parts)`: refuse a symlink (or non-folder) anywhere below `base` on the
/// way to the folder.
fn check_chain(base: &Path, parts: &[String]) -> Result<(), Error> {
    let mut cur = base.to_path_buf();
    for part in parts {
        cur.push(part);
        if is_symlink(&cur) {
            return Err(rejected(format!(
                "refusing to write through a symlink: {}",
                shown(&cur)
            )));
        }
        if lexists(&cur) && !cur.is_dir() {
            return Err(rejected(format!("not a folder: {}", shown(&cur))));
        }
    }
    Ok(())
}

/// `_make_dir(base, parts)`.
fn make_dir(base: &Path, parts: &[String]) -> Result<PathBuf, Error> {
    mkdir_parents(base)?;
    let mut cur = base.to_path_buf();
    let through = |cur: &Path| {
        rejected(format!(
            "refusing to write through a symlink: {}",
            shown(cur)
        ))
    };
    for part in parts {
        cur.push(part);
        if is_symlink(&cur) {
            return Err(through(&cur));
        }
        if !cur.exists() {
            fs::create_dir(&cur).map_err(|e| os_error(e, &cur))?;
            if is_symlink(&cur) {
                return Err(through(&cur));
            }
        }
    }
    if realpath(&cur) != joined(&realpath(base), parts) {
        return Err(rejected(format!(
            "the destination is not where it should be: {}",
            shown(&cur)
        )));
    }
    Ok(cur)
}

/// `_create(directory, name, data)`: `O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW`, mode 0644.
fn create(dir: &Path, name: &str, data: &[u8]) -> Result<(), PyExc> {
    fsx::create_excl(&dir.join(name), data, 0o644, true)
}

/// `for x in plan.get(key, [])`: a list's items; a str's characters or an object's keys are
/// never entries (`None`). A value Python cannot iterate (`TypeError`) is a malformed plan.
fn py_iter(v: Option<&Value>) -> Result<Vec<Option<&Value>>, Error> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.iter().map(Some).collect()),
        Some(Value::Str(t)) => Ok(vec![None; t.chars().count()]),
        Some(Value::Object(m)) => Ok(vec![None; m.len()]),
        Some(_) => Err(rejected("malformed plan")),
    }
}

/// `_checked_entry(entry)`: the fields apply uses, re-validated (`dest` in a plan is
/// informational only): `(scope, agent, name, member)`.
fn checked_entry(entry: Option<&Value>) -> Result<(String, Option<String>, String, String), Error> {
    let Some(Value::Object(e)) = entry else {
        return Err(rejected("malformed plan entry"));
    };
    let scope = match e.get("scope") {
        Some(Value::Str(t)) if SCOPES.contains(&t.as_str()) => t.clone(),
        _ => return Err(rejected("malformed plan entry")),
    };
    let name = match e.get("name") {
        Some(Value::Str(n)) if load::ok_note_name(n) => n.clone(),
        _ => return Err(rejected("malformed plan entry name")),
    };
    let agent = if scope == "agent" {
        match e.get("agent") {
            Some(Value::Str(a)) if load::name_ok(a) => Some(a.clone()),
            _ => return Err(rejected("malformed plan entry folder")),
        }
    } else {
        None
    };
    let member = match (scope.as_str(), &agent) {
        ("personal", _) => format!("personal/{name}"),
        ("repo", _) => format!("repo/memory/{name}"),
        (_, a) => format!(
            "repo/agent-memory/{}/{name}",
            a.as_deref().unwrap_or_default()
        ),
    };
    if e.get("member").and_then(Value::as_str) != Some(member.as_str()) {
        return Err(rejected("malformed plan entry member"));
    }
    Ok((scope, agent, name, member))
}

/// `apply_import(plan)` with an empty `os.environ` (see [`apply_import_env`]).
pub fn apply_import(plan: &Value, git: &dyn Git) -> Result<Map, Error> {
    apply_import_env(plan, &Env::default(), git)
}

/// A destination folder's key: `(scope, agent)`.
type Place = (String, Option<String>);

/// One planned write: `(key, scope, agent, name, data)`.
struct Work<'a> {
    key: &'static str,
    scope: String,
    agent: Option<String>,
    name: String,
    data: &'a [u8],
}

/// `apply_import(plan)`: `{"written", "conflicts", "skipped", "blocked", "indexes"}`. `env` is
/// Python's `os.environ`: `HOME` for the privacy scan, and git's environment (with
/// `CLAUDE_CONFIG_DIR` set next to the plan's `home`, as Python's `penv`).
pub fn apply_import_env(plan: &Value, env: &Env, git: &dyn Git) -> Result<Map, Error> {
    let Value::Object(p) = plan else {
        return Err(rejected("malformed plan"));
    };
    let (Some(home), Some(archive)) = (
        p.get("home").and_then(Value::as_str),
        p.get("archive").and_then(Value::as_str),
    ) else {
        return Err(rejected("malformed plan"));
    };
    let home = Path::new(home);
    if home.file_name().and_then(|n| n.to_str()) != Some("anthology") {
        return Err(rejected("malformed plan"));
    }
    let loaded = load::load_archive(Path::new(archive))?;
    if p.get("sha256").and_then(Value::as_str) != Some(loaded.sha256.as_str()) {
        return Err(rejected("the archive changed after it was planned"));
    }
    let parent = match home.parent() {
        Some(q) if !q.as_os_str().is_empty() => q,
        _ => Path::new("."),
    };
    let penv = env.clone().with_os("CLAUDE_CONFIG_DIR", shown(parent));
    let date = checked_date(iso_value(p.get("date")))?;
    // `{b.get("file") for b in ... if isinstance(b, dict)}`: a list or dict `file` is
    // unhashable, a TypeError before anything is written (D-1: "malformed plan").
    let mut planned_blocked: BTreeSet<&str> = BTreeSet::new();
    for b in py_iter(p.get("blocked"))?.into_iter().flatten() {
        match b.as_object().and_then(|o| o.get("file")) {
            Some(Value::Array(_) | Value::Object(_)) => return Err(rejected("malformed plan")),
            Some(Value::Str(f)) => {
                planned_blocked.insert(f);
            }
            _ => {}
        }
    }
    let pdeps = PrivacyDeps::from_env(env);
    let mut work: Vec<Work> = Vec::new();
    let mut blocked: Vec<Value> = Vec::new();
    for key in ["new", "conflicts", "identical"] {
        for entry in py_iter(p.get(key))? {
            let (mut scope, mut agent, name, member) = checked_entry(entry)?;
            let Some(data) = loaded.contents.get(&member) else {
                return Err(rejected(format!("plan entry not in the archive: {member}")));
            };
            if scope != "personal" && key != "identical" {
                let (findings, reason) = repo_block(&name, data, &pdeps);
                if reason.is_some() || planned_blocked.contains(member.as_str()) {
                    let reason = reason.unwrap_or_else(|| "blocked in the plan".into());
                    blocked.push(obj(vec![
                        ("file", s(&member)),
                        ("findings", findings_json(&findings)),
                        ("reason", s(&reason)),
                    ]));
                    scope = "personal".into();
                    agent = None;
                }
            }
            work.push(Work {
                key,
                scope,
                agent,
                name,
                data,
            });
        }
    }
    let root = p.get("repo_root").and_then(Value::as_str).map(Path::new);
    if work.iter().any(|w| w.scope != "personal") {
        let approved = root.is_some_and(|r| {
            r.is_dir()
                && repo::repo_root(r, &penv, git) == Some(realpath(r))
                && approve::is_approved(&repo::repo_key(r, &penv, git), &penv)
        });
        if !approved {
            return Err(rejected("the repo is not an approved repo"));
        }
    }
    let no_root = PathBuf::new();
    let root = root.unwrap_or(&no_root);
    // Pre-flight every destination before the first write.
    let mut targets: Vec<(Place, (PathBuf, Vec<String>))> = Vec::new();
    for w in &work {
        let (base, parts) = location(&w.scope, w.agent.as_deref(), home, root);
        check_chain(&base, &parts)?;
        let k = (w.scope.clone(), w.agent.clone());
        if !targets.iter().any(|(t, _)| *t == k) {
            targets.push((k, (base, parts)));
        }
    }
    let (mut written, mut conflicts, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
    let mut dirs: Vec<(Place, PathBuf)> = Vec::new();
    for w in &work {
        if w.key == "identical" {
            skipped.push(s(&w.name));
            continue;
        }
        let k = (w.scope.clone(), w.agent.clone());
        let directory = match dirs.iter().find(|(d, _)| *d == k) {
            Some((_, d)) => d.clone(),
            None => {
                let (base, parts) = &targets
                    .iter()
                    .find(|(t, _)| *t == k)
                    .expect("every work item has a target")
                    .1;
                let d = make_dir(base, parts)?;
                dirs.push((k, d.clone()));
                d
            }
        };
        if w.key == "new" {
            match create(&directory, &w.name, w.data) {
                Ok(()) => {
                    written.push(s(&shown(&directory.join(&w.name))));
                    continue;
                }
                // Appeared since planning: treated as a conflict below.
                Err(e) if is_exists_error(&e) => {}
                Err(e) => return Err(Error::Os(e)),
            }
        }
        if same_as_existing(&directory, &w.name, w.data)? {
            skipped.push(s(&w.name));
            continue;
        }
        let imported = loop {
            let imported = imported_name(&directory, &w.name, &date);
            match create(&directory, &imported, w.data) {
                Ok(()) => break imported,
                Err(e) if is_exists_error(&e) => continue,
                Err(e) => return Err(Error::Os(e)),
            }
        };
        conflicts.push(s(&shown(&directory.join(imported))));
    }
    let mut indexes = Vec::new();
    for ((scope, _), directory) in &dirs {
        let exclude: &[&str] = if scope == "personal" {
            &[]
        } else {
            notes::REPO_EXCLUDE
        };
        render::regenerate_index(directory, render::DEFAULT_BUDGET, exclude)?;
        indexes.push(s(&shown(directory)));
    }
    let mut result = Map::new();
    result.insert("written".into(), Value::Array(written));
    result.insert("conflicts".into(), Value::Array(conflicts));
    result.insert("skipped".into(), Value::Array(skipped));
    result.insert("blocked".into(), Value::Array(blocked));
    result.insert("indexes".into(), Value::Array(indexes));
    Ok(result)
}

// ---------------------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------------------

/// What `main` printed and returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

fn list<'a>(m: &'a Map, key: &str) -> &'a [Value] {
    m.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text<'a>(m: &'a Map, key: &str) -> &'a str {
    m.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Int(Int::Small(n))) => *n != 0,
        Some(Value::Float(f)) => *f != 0.0,
        Some(Value::Str(t)) => !t.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Int(Int::Big(_))) => true,
    }
}

/// `{"kind", "line"}` objects back into findings (for `describe`).
fn findings_of(v: Option<&Value>) -> Vec<Finding> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let f = f.as_object()?;
                    Some(Finding {
                        kind: f.get("kind")?.as_str()?.to_string(),
                        line: usize::try_from(f.get("line")?.as_i64()?).ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `_print_plan(plan)`: what it prints.
pub fn print_plan(plan: &Map) -> String {
    let mut out = String::new();
    let archive = text(plan, "archive");
    let base = archive.rsplit('/').next().unwrap_or_default();
    out.push_str(&format!(
        "Import plan for {base} (sha256 {})\n",
        text(plan, "sha256")
    ));
    let conflicts_title = format!(
        "conflicts (kept side by side as <name>.imported-{}.md)",
        text(plan, "date")
    );
    for (key, title) in [
        ("new", "new"),
        ("identical", "identical (skipped)"),
        ("conflicts", conflicts_title.as_str()),
    ] {
        out.push_str(&format!("  {title}: {}\n", list(plan, key).len()));
        for e in list(plan, key) {
            let e = e.as_object().cloned().unwrap_or_default();
            let tail = if truthy(e.get("redirected")) {
                " (to personal memory: blocked)"
            } else {
                ""
            };
            out.push_str(&format!("    - {}{tail}\n", text(&e, "member")));
        }
    }
    for message in list(plan, "messages") {
        out.push_str(&format!(
            "  note: {}\n",
            message.as_str().unwrap_or_default()
        ));
    }
    if truthy(plan.get("blocked")) {
        out.push_str("  blocked from the repo (written to personal memory instead):\n");
        for b in list(plan, "blocked") {
            let b = b.as_object().cloned().unwrap_or_default();
            let reason = if truthy(b.get("reason")) {
                text(&b, "reason").to_string()
            } else {
                privacy::describe(&findings_of(b.get("findings")))
            };
            out.push_str(&format!("    - {}: {reason}\n", text(&b, "file")));
        }
    }
    if !list(plan, "secrets").is_empty() {
        out.push_str("  secret-looking content found (labels only, contents not shown):\n");
        for f in list(plan, "secrets") {
            let f = f.as_object().cloned().unwrap_or_default();
            out.push_str(&format!(
                "    - {}: {}\n",
                text(&f, "file"),
                text(&f, "pattern")
            ));
        }
    }
    out
}

fn parser(env: &Env) -> Spec {
    let export = Spec::command()
        .arg(Arg::option(&["--out"]).help(
            "folder for the archive, outside any git repo (default: <anthology home>/exports)",
        ))
        .arg(Arg::option(&["--json"]).store_true());
    let import = Spec::command()
        .arg(Arg::positional("file"))
        .arg(
            Arg::option(&["--dry-run"])
                .store_true()
                .help("only print the plan"),
        )
        .arg(Arg::option(&["--yes"]).store_true().help("apply the plan"))
        .arg(
            Arg::option(&["--sha256"])
                .help("only apply if the archive has this sha256 (as printed by the dry run)"),
        )
        .arg(Arg::option(&["--json"]).store_true());
    let sub = Subcommands::new()
        .dest("command")
        .required()
        .parser(
            "export",
            Some("write anthology-memory-<date>.tar.gz"),
            export,
        )
        .parser(
            "import",
            Some("import an archive (prints the plan unless --yes)"),
            import,
        );
    Spec::new("anthology memory")
        .description("Export or import Anthology memory notes.")
        .subcommands(sub)
        .width(argparse::help_width(env.get("COLUMNS"), None))
}

fn line(out: &mut String, l: &str) {
    out.push_str(l);
    out.push('\n');
}

fn run(p: &argparse::Parsed, cwd: &Path, env: &Env, git: &dyn Git) -> Result<String, Error> {
    let mut out = String::new();
    let as_json = p.flag("json");
    if p.str("command") == Some("export") {
        let out_dir = p.os_str("out").map(Path::new);
        let res = export(cwd, out_dir, None, env, git)?;
        if as_json {
            line(
                &mut out,
                &json::dumps(&Value::Object(res), &json::INDENT2_SORTED),
            );
        } else {
            let c = res
                .get("counts")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let n = |k: &str| c.get(k).and_then(Value::as_i64).unwrap_or_default();
            line(&mut out, &format!("Wrote {}", text(&res, "path")));
            line(
                &mut out,
                &format!(
                    "  personal: {}, repo: {}, agent: {}",
                    n("personal"),
                    n("repo"),
                    n("agent")
                ),
            );
            if !truthy(res.get("repo_included")) {
                line(
                    &mut out,
                    "  (this folder is not an approved repo: repo notes not included)",
                );
            }
            for f in list(&res, "secrets") {
                let f = f.as_object().cloned().unwrap_or_default();
                line(
                    &mut out,
                    &format!(
                        "  secret-looking content ({}) in {}",
                        text(&f, "pattern"),
                        text(&f, "file")
                    ),
                );
            }
            line(
                &mut out,
                "The archive contains your personal notes; keep it private.",
            );
        }
        return Ok(out);
    }
    let file = Path::new(p.os_str("file").unwrap_or_default());
    let plan = plan_import(file, cwd, None, env, git)?;
    // A value that is not UTF-8 never equals the hex digest (Python compares the
    // surrogateescaped str), so it is refused too.
    if let Some(want) = p.os_str("sha256").filter(|w| !w.is_empty())
        && want
            .to_str()
            .is_none_or(|w| anthology_pycompat::str::lower(w) != text(&plan, "sha256"))
    {
        return Err(rejected(
            "the archive is not the one that was shown (sha256 differs)",
        ));
    }
    let result = if p.flag("yes") && !p.flag("dry_run") {
        Some(apply_import_env(&Value::Object(plan.clone()), env, git)?)
    } else {
        None
    };
    if as_json {
        let both = obj(vec![
            ("plan", Value::Object(plan)),
            ("result", result.map_or(Value::Null, Value::Object)),
        ]);
        line(&mut out, &json::dumps(&both, &json::INDENT2_SORTED));
        return Ok(out);
    }
    out.push_str(&print_plan(&plan));
    match result {
        None => line(&mut out, "Nothing written. Re-run with --yes to apply."),
        Some(r) => {
            line(
                &mut out,
                &format!(
                    "Wrote {} note(s); {} kept side by side:",
                    list(&r, "written").len(),
                    list(&r, "conflicts").len()
                ),
            );
            for path in list(&r, "conflicts") {
                line(
                    &mut out,
                    &format!("    {}", path.as_str().unwrap_or_default()),
                );
            }
            let blocked = list(&r, "blocked");
            if !blocked.is_empty() {
                line(
                    &mut out,
                    &format!(
                        "{} repo note(s) blocked from the repo, written to personal memory:",
                        blocked.len()
                    ),
                );
                for b in blocked {
                    let b = b.as_object().cloned().unwrap_or_default();
                    line(&mut out, &format!("    {}", text(&b, "file")));
                }
            }
        }
    }
    Ok(out)
}

/// `main(argv)` with `prog` `memory_portable.py` replaced as the CLI shows it; `cwd` is
/// `os.getcwd()`.
pub fn main(args: &[OsString], cwd: &Path, env: &Env, git: &dyn Git) -> Output {
    let parsed = match argparse::parse(&parser(env), args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stdout: e.stdout,
                stderr: e.stderr,
                code: e.code,
            };
        }
    };
    match run(&parsed, cwd, env, git) {
        Ok(stdout) => Output {
            stdout,
            stderr: String::new(),
            code: 0,
        },
        Err(e) => {
            let (msg, code) = match e {
                Error::Rejected(m) => (format!("rejected: {m}"), 2),
                Error::Refused(m) => (m, 2),
                Error::Value(m) => (m, 1),
                Error::Os(e) => (e.py_str(), 1),
            };
            Output {
                stdout: String::new(),
                stderr: format!("memory_portable: {msg}\n"),
                code,
            }
        }
    }
}
