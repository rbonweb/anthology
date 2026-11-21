//! The repo approval the auto-memory smokes need, with MS1's published signature (plan §3.6,
//! `anthology_memory::approve::approve`).
//!
//! SWAP(MS1): MS1 has not landed, so this is a minimal private stand-in that follows
//! `memory_sync.approve` (repo key, `approved-repos.json`, the union merge lines). Once MS1
//! merges, `approve` becomes `pub use anthology_memory::approve::approve;` and the rest of this
//! file goes.

use std::fs;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::Git;
use anthology_core::{clock, fsx, paths};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;

const UNION_LINES: [&str; 2] = [
    ".claude/memory/MEMORY.md merge=union",
    ".claude/agent-memory/*/MEMORY.md merge=union",
];
const DEFAULT_PORTS: [&str; 3] = ["22", "443", "80"];

fn git_out(git: &dyn Git, env: &Env, root: &Path, args: &[&str]) -> Option<String> {
    let out = git.run(env, root, args)?;
    if out.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn realpath(p: &Path) -> PathBuf {
    anthology_pycompat::path::realpath(p)
}

/// `normalize_remote(url, base)`: `host/owner/repo` lowercased, credentials and default ports
/// dropped; a local path becomes `path:<realpath>`.
fn normalize_remote(url: &str, base: &Path) -> String {
    let u = url.trim();
    let mut path: Option<String> = None;
    let mut key = String::new();
    if u.to_ascii_lowercase().starts_with("file://") {
        path = Some(u[7..].to_string());
    } else if let Some((_, rest)) = u.split_once("://") {
        let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let (mut host, mut port) = (authority, "");
        if authority.matches(':').count() == 1
            && let Some((h, p)) = authority.split_once(':')
        {
            (host, port) = (h, p);
        }
        if DEFAULT_PORTS.contains(&port) {
            port = "";
        }
        key = format!(
            "{host}{}/{tail}",
            if port.is_empty() {
                String::new()
            } else {
                format!(":{port}")
            }
        );
    } else if u.starts_with('/')
        || u.starts_with("./")
        || u.starts_with("../")
        || u.starts_with('~')
    {
        path = Some(u.to_string());
    } else {
        // `^(?:[^@/\s]+@)?([^:/\s]+):(.*)$`
        let scp = regex::Regex::new(r"^(?:[^@/\s]+@)?([^:/\s]+):(.*)$").expect("fixed pattern");
        match scp.captures(u) {
            Some(c) => key = format!("{}/{}", &c[1], &c[2]),
            None => path = Some(u.to_string()),
        }
    }
    if let Some(p) = path {
        let full = realpath(&base.join(p));
        let full = full.to_string_lossy();
        let full = full.strip_suffix(".git").unwrap_or(&full);
        return format!("path:{full}");
    }
    let slashes = regex::Regex::new("/+").expect("fixed pattern");
    let key = slashes.replace_all(&key, "/");
    let key = key.trim_end_matches('/');
    let key = key.strip_suffix(".git").unwrap_or(key);
    key.trim_end_matches('/').to_lowercase()
}

/// `_main_root(root)`: the main work tree, so a linked worktree maps to its clone.
fn main_root(root: &Path, env: &Env, git: &dyn Git) -> PathBuf {
    let common = git_out(
        git,
        env,
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .or_else(|| {
        git_out(git, env, root, &["rev-parse", "--git-common-dir"])
            .map(|rel| root.join(rel).to_string_lossy().into_owned())
    });
    if let Some(common) = common {
        let norm = anthology_pycompat::path::normpath(&common);
        let norm = Path::new(&norm);
        if norm.file_name().is_some_and(|n| n == ".git")
            && let Some(parent) = norm.parent()
        {
            return realpath(parent);
        }
    }
    let top = git_out(git, env, root, &["rev-parse", "--show-toplevel"]);
    realpath(Path::new(
        &top.unwrap_or_else(|| root.to_string_lossy().into_owned()),
    ))
}

/// `repo_key(root)`: the normalised remote URL, else `path:<main work tree>`.
fn repo_key(root: &Path, env: &Env, git: &dyn Git) -> String {
    let mut names: Vec<String> = git_out(git, env, root, &["remote"])
        .map(|o| o.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    names.sort();
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
            return normalize_remote(&url, root);
        }
    }
    format!("path:{}", main_root(root, env, git).display())
}

/// `ensure_union_attrs(root)`: the union merge lines in `.gitattributes`, once each.
fn ensure_union_attrs(root: &Path) -> Result<(), PyExc> {
    let path = root.join(".gitattributes");
    let existing = match fs::symlink_metadata(&path) {
        Err(_) => String::new(),
        Ok(m) if m.is_file() => String::from_utf8_lossy(&fsx::read_regular(&path, None)?).into(),
        Ok(_) => {
            return Err(PyExc::Other(
                "UnsafeTarget",
                format!(
                    "refusing to use {}: it is not a regular file",
                    path.display()
                ),
            ));
        }
    };
    let have: Vec<String> = existing
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    for line in UNION_LINES {
        if !have.iter().any(|h| h == line) {
            fsx::append_line(&path, line)?;
        }
    }
    Ok(())
}

/// `memory_sync.approve(root)`: records the repo's key in `<ah>/approved-repos.json`, then adds
/// the union merge lines. Returns the key.
pub fn approve(root: &Path, env: &Env, git: &dyn Git) -> Result<String, PyExc> {
    let key = repo_key(root, env, git);
    let path = paths::anthology_home(env).join("approved-repos.json");
    let mut repos = fs::read_to_string(&path)
        .ok()
        .and_then(|t| json::loads(&t).ok())
        .and_then(|mut d| d.as_object_mut().and_then(|o| o.remove("repos")))
        .and_then(Value::into_object)
        .unwrap_or_default();
    if repos.get(&key).is_none() {
        let mut entry = Map::new();
        entry.insert(
            "approved".to_string(),
            Value::Str(clock::today_local().iso()),
        );
        repos.insert(key.clone(), Value::Object(entry));
        let mut doc = Map::new();
        doc.insert("version".to_string(), Value::Int(json::Int::Small(1)));
        doc.insert("repos".to_string(), Value::Object(repos));
        let mut text = json::dumps(&Value::Object(doc), &json::INDENT2_SORTED);
        text.push('\n');
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(PyExc::from)?;
        }
        fsx::atomic_write(&path, text.as_bytes(), 0o644, ".tmp-", ".part")?;
    }
    ensure_union_attrs(root)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_normalise_like_memory_sync() {
        let base = Path::new("/");
        assert_eq!(
            normalize_remote("https://example.invalid/smoke/repo.git", base),
            "example.invalid/smoke/repo"
        );
        assert_eq!(
            normalize_remote("https://u:p@Host.EXAMPLE:443//A/B.git/", base),
            "host.example/a/b"
        );
        assert_eq!(
            normalize_remote("ssh://git@host:2222/a/b", base),
            "host:2222/a/b"
        );
        assert_eq!(
            normalize_remote("git@github.com:A/B.git", base),
            "github.com/a/b"
        );
        assert_eq!(normalize_remote("/srv/x.git", base), "path:/srv/x");
    }
}
