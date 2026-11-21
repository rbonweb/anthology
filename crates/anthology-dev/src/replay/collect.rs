//! `replay` part `collect`: the local corpus from Claude Code's transcripts (design §13.7).
//!
//! - **Source:** every `*.jsonl` below `<config>/projects/` (recursively, so
//!   `*/subagents/*.jsonl` too), in sorted order. Symlinks are not followed.
//! - **Calls:** every `tool_use` block of a line's `message.content`, deduplicated by its
//!   canonical form ([`Call::canonical`]) with a count. The first occurrence's session id,
//!   transcript path, `cwd`, permission mode (the file's last `permissionMode`, else `default`)
//!   and block id become the envelope fields; its `input` bytes are kept raw.
//! - **Texts:** every text field, deduplicated with a count, for the privacy comparison:
//!   `message.content` when it is a string; `text` and `thinking` blocks; every string in a
//!   `tool_use` input; `tool_result` content (a string, or its text blocks); every string in a
//!   line's `toolUseResult`. Lone surrogates are already U+FFFD (F-19), for both sides.
//! - **Output** (local only, never in a repository): `<replay>/replay-corpus-calls.jsonl`,
//!   `replay-corpus-texts.jsonl` and `replay-corpus-manifest.json`, mode 0600, written through
//!   temporary files and renamed; plus `hmac.key` on first use. The replay folder is
//!   `~/.claude/anthology/replay/` (GC-1), created 0700, and refused when it lies inside an
//!   anthology checkout or any other git work tree (a dotfiles repository could commit it).
//!   Temporary files are named so `.gitignore`'s `replay-corpus*` covers them too.
//!
//! Memory stays bounded by the number of distinct items (one hash each): records stream to the
//! temporary files and get their counts in a final pass.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use anthology_core::{Env, paths};
use anthology_pycompat::json::{self, INDENT2_SORTED, Int, Map, Value};

use super::envelope::{Call, Step, span};
use super::hmac;

pub const CALLS_FILE: &str = "replay-corpus-calls.jsonl";
pub const TEXTS_FILE: &str = "replay-corpus-texts.jsonl";
pub const MANIFEST_FILE: &str = "replay-corpus-manifest.json";

/// `~/.claude/anthology/replay` (GC-1: the real home, not `<config>`).
pub fn default_dir(env: &Env) -> PathBuf {
    paths::home(env).join(".claude/anthology/replay")
}

/// `<config>/projects`.
pub fn default_projects(env: &Env) -> PathBuf {
    paths::config_dir(env).join("projects")
}

/// Whether `dir` is an anthology checkout (or worktree) root.
fn is_checkout(dir: &Path) -> bool {
    dir.join("port/difftest").is_dir() || dir.join("port/replay-reports").is_dir()
}

/// `path` with its deepest existing ancestor resolved (symlinks included).
fn resolved(path: &Path, cwd: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut rest = Vec::new();
    let mut cur = abs.as_path();
    loop {
        if let Ok(real) = fs::canonicalize(cur) {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return abs,
        }
    }
}

/// Refuses a replay folder inside an anthology checkout or any git work tree: the corpus must
/// never be committed.
pub fn refuse_in_checkout(dir: &Path, cwd: &Path) -> Result<(), String> {
    let real = resolved(dir, cwd);
    if let Some(top) = real.ancestors().find(|a| is_checkout(a)) {
        return Err(format!(
            "refusing to use {} for replay data: it is inside the anthology checkout {}; \
             the corpus is local only",
            dir.display(),
            top.display()
        ));
    }
    if let Some(top) = real.ancestors().find(|a| a.join(".git").exists()) {
        return Err(format!(
            "refusing to use {} for replay data: it is inside the git work tree {}; the corpus \
             is local only (pass --dir outside any repository)",
            dir.display(),
            top.display()
        ));
    }
    Ok(())
}

/// Creates the replay folder (0700) after [`refuse_in_checkout`].
pub fn ensure_dir(dir: &Path, cwd: &Path) -> Result<(), String> {
    refuse_in_checkout(dir, cwd)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let meta = fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot chmod {}: {e}", dir.display()))?;
    refuse_in_checkout(dir, cwd)
}

/// What `collect` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    pub files: u64,
    pub lines: u64,
    pub skipped: u64,
    pub calls_total: u64,
    pub calls_distinct: u64,
    pub texts_total: u64,
    pub texts_distinct: u64,
}

/// Every `*.jsonl` file below `root`, sorted, without following symlinks.
pub fn transcripts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_dir() {
                stack.push(path);
            } else if meta.file_type().is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The calls and texts of one transcript line (module docs). `mode` is the file's current
/// permission mode, updated from the line. `None` when the line is not a JSON object.
pub fn line_items(
    line: &[u8],
    transcript: &str,
    mode: &mut String,
) -> Option<(Vec<Call>, Vec<String>)> {
    let value = json::loads_bytes_lossy(line).ok()?;
    let obj = value.as_object()?;
    if let Some(m) = obj.get("permissionMode").and_then(Value::as_str) {
        *mode = m.to_string();
    }
    let s = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut calls = Vec::new();
    let mut texts = Vec::new();
    let content = obj
        .get("message")
        .and_then(Value::as_object)
        .and_then(|m| m.get("content"));
    match content {
        Some(Value::Str(text)) => texts.push(text.clone()),
        Some(Value::Array(blocks)) => {
            for (i, block) in blocks.iter().enumerate() {
                block_texts(block, &mut texts);
                let Some(b) = block.as_object() else { continue };
                if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let Some(name) = b.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let path = [
                    Step::Key("message"),
                    Step::Key("content"),
                    Step::Index(i),
                    Step::Key("input"),
                ];
                let Some(raw) = span(line, &path) else {
                    continue;
                };
                calls.push(Call {
                    count: 1,
                    tool_name: name.to_string(),
                    session_id: s("sessionId"),
                    transcript_path: transcript.to_string(),
                    cwd: s("cwd"),
                    permission_mode: mode.clone(),
                    tool_use_id: b
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    raw_input: line[raw].to_vec(),
                });
            }
        }
        _ => {}
    }
    if let Some(result) = obj.get("toolUseResult") {
        leaves(result, &mut texts);
    }
    texts.retain(|t| !t.is_empty());
    Some((calls, texts))
}

fn block_texts(block: &Value, out: &mut Vec<String>) {
    let Some(b) = block.as_object() else { return };
    let field = |k: &str, out: &mut Vec<String>| {
        if let Some(t) = b.get(k).and_then(Value::as_str) {
            out.push(t.to_string());
        }
    };
    match b.get("type").and_then(Value::as_str) {
        Some("text") => field("text", out),
        Some("thinking") => field("thinking", out),
        Some("tool_use") => {
            if let Some(input) = b.get("input") {
                leaves(input, out);
            }
        }
        Some("tool_result") => match b.get("content") {
            Some(Value::Str(t)) => out.push(t.clone()),
            Some(Value::Array(items)) => {
                for item in items {
                    block_texts(item, out);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

/// Every string value inside `v` (iterative).
fn leaves(v: &Value, out: &mut Vec<String>) {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Value::Str(s) => out.push(s.clone()),
            Value::Array(items) => stack.extend(items.iter().rev()),
            Value::Object(m) => {
                let values: Vec<&Value> = m.iter().map(|(_, v)| v).collect();
                stack.extend(values.into_iter().rev());
            }
            _ => {}
        }
    }
}

/// A new file, mode 0600. `create_new` is `O_CREAT | O_EXCL`, which also refuses a symlink.
fn private_file(path: &Path) -> Result<BufWriter<File>, String> {
    let _ = fs::remove_file(path);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(BufWriter::new)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("cannot write {}: {e}", path.display())
}

/// Builds the corpus in `dir` from the transcripts below `projects`.
pub fn collect(projects: &Path, dir: &Path, cwd: &Path) -> Result<Collected, String> {
    ensure_dir(dir, cwd)?;
    hmac::load_or_create(dir)?;
    let calls_tmp = dir.join("replay-corpus-calls.tmp");
    let texts_tmp = dir.join("replay-corpus-texts.tmp");
    let result = collect_into(projects, dir, &calls_tmp, &texts_tmp);
    let _ = fs::remove_file(&calls_tmp);
    let _ = fs::remove_file(&texts_tmp);
    result
}

fn collect_into(
    projects: &Path,
    dir: &Path,
    calls_tmp: &Path,
    texts_tmp: &Path,
) -> Result<Collected, String> {
    let mut found = Collected::default();
    let mut call_ids: HashMap<[u8; 32], usize> = HashMap::new();
    let mut call_counts: Vec<u64> = Vec::new();
    let mut text_ids: HashMap<[u8; 32], usize> = HashMap::new();
    let mut text_counts: Vec<u64> = Vec::new();
    {
        let mut calls_out = private_file(calls_tmp)?;
        let mut texts_out = private_file(texts_tmp)?;
        for file in transcripts(projects) {
            let Ok(f) = File::open(&file) else { continue };
            found.files += 1;
            let shown = file.to_string_lossy().into_owned();
            let mut mode = "default".to_string();
            let mut reader = BufReader::new(f);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    line.pop();
                }
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                found.lines += 1;
                let Some((calls, texts)) = line_items(&line, &shown, &mut mode) else {
                    found.skipped += 1;
                    continue;
                };
                for mut call in calls {
                    found.calls_total += 1;
                    let key: [u8; 32] = Sha256::digest(call.canonical().as_bytes()).into();
                    match call_ids.entry(key) {
                        Entry::Occupied(e) => call_counts[*e.get()] += 1,
                        Entry::Vacant(e) => {
                            e.insert(call_counts.len());
                            call_counts.push(1);
                            call.count = 0;
                            calls_out
                                .write_all(&call.corpus_line())
                                .and_then(|()| calls_out.write_all(b"\n"))
                                .map_err(io_err(calls_tmp))?;
                        }
                    }
                }
                for text in texts {
                    found.texts_total += 1;
                    let key: [u8; 32] = Sha256::digest(text.as_bytes()).into();
                    match text_ids.entry(key) {
                        Entry::Occupied(e) => text_counts[*e.get()] += 1,
                        Entry::Vacant(e) => {
                            e.insert(text_counts.len());
                            text_counts.push(1);
                            let mut m = Map::new();
                            m.insert("text".into(), Value::Str(text));
                            texts_out
                                .write_all(json::js_stringify(&Value::Object(m)).as_bytes())
                                .and_then(|()| texts_out.write_all(b"\n"))
                                .map_err(io_err(texts_tmp))?;
                        }
                    }
                }
            }
        }
        calls_out.flush().map_err(io_err(calls_tmp))?;
        texts_out.flush().map_err(io_err(texts_tmp))?;
    }
    found.calls_distinct = call_counts.len() as u64;
    found.texts_distinct = text_counts.len() as u64;
    let calls_sha = finish(calls_tmp, &dir.join(CALLS_FILE), &call_counts, |line, n| {
        let mut call = Call::from_corpus_line(line)?;
        call.count = n;
        Some(call.corpus_line())
    })?;
    let texts_sha = finish(texts_tmp, &dir.join(TEXTS_FILE), &text_counts, |line, n| {
        let text = json::loads_bytes_lossy(line)
            .ok()?
            .as_object()?
            .get("text")?
            .clone();
        let mut m = Map::new();
        m.insert("count".into(), int(n));
        m.insert("text".into(), text);
        Some(json::js_stringify(&Value::Object(m)).into_bytes())
    })?;
    let mut m = Map::new();
    m.insert("version".into(), int(1));
    m.insert("files".into(), int(found.files));
    m.insert("lines".into(), int(found.lines));
    m.insert("skipped".into(), int(found.skipped));
    m.insert("calls_total".into(), int(found.calls_total));
    m.insert("calls_distinct".into(), int(found.calls_distinct));
    m.insert("texts_total".into(), int(found.texts_total));
    m.insert("texts_distinct".into(), int(found.texts_distinct));
    m.insert("calls_sha256".into(), Value::Str(calls_sha));
    m.insert("texts_sha256".into(), Value::Str(texts_sha));
    let mut text = json::dumps(&Value::Object(m), &INDENT2_SORTED);
    text.push('\n');
    let manifest = dir.join(MANIFEST_FILE);
    let tmp = dir.join("replay-corpus-manifest.tmp");
    let mut out = private_file(&tmp)?;
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(io_err(&tmp))?;
    drop(out);
    fs::rename(&tmp, &manifest).map_err(io_err(&manifest))?;
    Ok(found)
}

fn int(n: u64) -> Value {
    Value::Int(Int::Small(i64::try_from(n).unwrap_or(i64::MAX)))
}

/// Rewrites `tmp` into `dest` with each line's count; returns `dest`'s sha256.
fn finish(
    tmp: &Path,
    dest: &Path,
    counts: &[u64],
    with_count: impl Fn(&[u8], u64) -> Option<Vec<u8>>,
) -> Result<String, String> {
    let staged = dest.with_extension("staged");
    let mut out = private_file(&staged)?;
    let mut sha = Sha256::new();
    let reader = BufReader::new(File::open(tmp).map_err(|e| format!("{}: {e}", tmp.display()))?);
    let mut n = 0;
    for line in reader.split(b'\n') {
        let line = line.map_err(|e| format!("{}: {e}", tmp.display()))?;
        let count = *counts
            .get(n)
            .ok_or("replay collect: the corpus count is off")?;
        n += 1;
        let mut rec = with_count(&line, count)
            .ok_or_else(|| format!("replay collect: cannot reread {}", tmp.display()))?;
        rec.push(b'\n');
        sha.update(&rec);
        out.write_all(&rec).map_err(io_err(&staged))?;
    }
    if n != counts.len() {
        return Err("replay collect: the corpus count is off".into());
    }
    out.flush().map_err(io_err(&staged))?;
    drop(out);
    fs::rename(&staged, dest).map_err(io_err(dest))?;
    Ok(hmac::hex(&sha.finalize()))
}

/// The distinct calls of the corpus in `dir`, in first-seen order.
pub fn read_calls(dir: &Path) -> Result<Vec<Call>, String> {
    read_lines(&dir.join(CALLS_FILE), Call::from_corpus_line)
}

/// The distinct texts of the corpus in `dir`, with their counts.
pub fn read_texts(dir: &Path) -> Result<Vec<(u64, String)>, String> {
    read_lines(&dir.join(TEXTS_FILE), |line| {
        let v = json::loads_bytes_lossy(line).ok()?;
        let m = v.as_object()?;
        let n = u64::try_from(m.get("count")?.as_i64()?).ok()?;
        Some((n, m.get("text")?.as_str()?.to_string()))
    })
}

fn read_lines<T>(path: &Path, parse: impl Fn(&[u8]) -> Option<T>) -> Result<Vec<T>, String> {
    let f = File::open(path).map_err(|_| {
        format!(
            "no replay corpus at {}; run `anthology-dev replay collect` first",
            path.display()
        )
    })?;
    let mut out = Vec::new();
    for (i, line) in BufReader::new(f).split(b'\n').enumerate() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        out.push(
            parse(&line)
                .ok_or_else(|| format!("{}:{}: not a corpus line", path.display(), i + 1))?,
        );
    }
    Ok(out)
}

/// The sha256 of the corpus manifest (the report names it).
pub fn manifest_sha256(dir: &Path) -> Result<String, String> {
    let path = dir.join(MANIFEST_FILE);
    let bytes = fs::read(&path).map_err(|_| {
        format!(
            "no replay corpus at {}; run `anthology-dev replay collect` first",
            path.display()
        )
    })?;
    Ok(hmac::hex(&Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_gives_its_calls_and_every_text_field() {
        let line = br#"{"type":"assistant","sessionId":"s1","cwd":"/w","message":{"role":"assistant","content":[
            {"type":"text","text":"hello there"},
            {"type":"thinking","thinking":"hmm","signature":"sig"},
            {"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls  -la","nested":{"x":["deep"]}}},
            {"type":"image","source":{"type":"base64","data":"AAAA"}}]}}"#;
        let mut mode = "default".to_string();
        let (calls, texts) = line_items(line, "/t.jsonl", &mut mode).expect("object");
        assert_eq!(calls.len(), 1);
        let c = &calls[0];
        assert_eq!(
            (c.tool_name.as_str(), c.session_id.as_str(), c.cwd.as_str()),
            ("Bash", "s1", "/w")
        );
        assert_eq!(
            (c.tool_use_id.as_str(), c.permission_mode.as_str()),
            ("toolu_1", "default")
        );
        assert_eq!(
            c.raw_input,
            br#"{"command":"ls  -la","nested":{"x":["deep"]}}"#.to_vec()
        );
        assert_eq!(c.transcript_path, "/t.jsonl");
        assert_eq!(texts, ["hello there", "hmm", "ls  -la", "deep"]);
    }

    #[test]
    fn tool_results_and_the_permission_mode_are_read() {
        let mut mode = "default".to_string();
        let user = br#"{"type":"user","permissionMode":"plan","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"out 1"},{"type":"image"}]},
            {"type":"tool_result","tool_use_id":"toolu_2","content":"out 2"}]},
            "toolUseResult":{"stdout":"out 3","stderr":"","interrupted":false}}"#;
        let (calls, texts) = line_items(user, "/t", &mut mode).expect("object");
        assert!(calls.is_empty());
        assert_eq!(texts, ["out 1", "out 2", "out 3"]);
        assert_eq!(mode, "plan");
        let plain = br#"{"type":"user","message":{"role":"user","content":"a prompt"}}"#;
        assert_eq!(
            line_items(plain, "/t", &mut mode).expect("object").1,
            ["a prompt"]
        );
        let next = br#"{"message":{"content":[{"type":"tool_use","name":"Read","input":{}}]}}"#;
        let (calls, _) = line_items(next, "/t", &mut mode).expect("object");
        assert_eq!(calls[0].permission_mode, "plan");
        assert_eq!(calls[0].tool_use_id, "");
        assert!(line_items(b"[1, 2]", "/t", &mut mode).is_none());
        assert!(line_items(b"{not json", "/t", &mut mode).is_none());
    }

    #[test]
    fn a_folder_inside_a_checkout_is_refused() {
        let root = tempfile::tempdir().expect("tempdir");
        let checkout = root.path().join("repo");
        fs::create_dir_all(checkout.join("port/difftest")).expect("mkdir");
        let inside = checkout.join("a/b/replay");
        let err = ensure_dir(&inside, root.path()).expect_err("inside");
        assert!(err.contains("inside the anthology checkout"), "{err}");
        assert!(!checkout.join("a").exists(), "nothing was created");
        // Relative, and through a symlink, are refused too.
        assert!(refuse_in_checkout(Path::new("x"), &checkout).is_err());
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&checkout, &link).expect("symlink");
        assert!(refuse_in_checkout(&link.join("replay"), root.path()).is_err());
        // Any other git work tree is refused too.
        let dotfiles = root.path().join("dotfiles");
        fs::create_dir_all(dotfiles.join(".git")).expect("mkdir");
        let err =
            refuse_in_checkout(&dotfiles.join(".claude/replay"), root.path()).expect_err("git");
        assert!(err.contains("inside the git work tree"), "{err}");
        let outside = root.path().join("home/.claude/anthology/replay");
        ensure_dir(&outside, root.path()).expect("outside");
        let mode = fs::metadata(&outside).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
