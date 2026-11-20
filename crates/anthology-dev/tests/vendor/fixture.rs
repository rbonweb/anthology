//! Shared helpers of the vendor tests (tests/test_vendor.py's module helpers and `FixtureCase`).
#![allow(dead_code, reason = "each test module uses its own subset")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_dev::vendor::{self, Entry, VendorError};
use anthology_pycompat::json::{self, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const MIT_TEXT: &str = "MIT License\n\nCopyright (c) Someone\n";
pub const FIXED_MIT: &str = "Note: the license is declared in the upstream README.\n\nMIT License\n\nCopyright (c) Vercel\n";

/// The repository root (`tests/_repo.py` ROOT).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A str value.
pub fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

/// A value from JSON text.
pub fn jv(text: &str) -> Value {
    json::loads(text).unwrap_or_else(|e| panic!("bad test JSON {text}: {e:?}"))
}

/// `write(base, rel, text, exec_bit)`: parents created, mode 755 or 644.
pub fn write(base: &Path, rel: &str, text: impl AsRef<[u8]>, exec_bit: bool) -> PathBuf {
    let p = base.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text.as_ref()).unwrap();
    let mode = if exec_bit { 0o755 } else { 0o644 };
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    p
}

/// `entry(name, **kw)`.
pub fn entry(name: &str, kw: &[(&str, Value)]) -> Value {
    let mut e = json::Map::new();
    e.insert("name".into(), s(name));
    e.insert("repo".into(), s(&format!("https://github.com/acme/{name}")));
    e.insert("path".into(), s(""));
    e.insert("ref".into(), s("main"));
    e.insert("commit".into(), s(SHA_A));
    e.insert("license".into(), s("MIT"));
    e.insert("license_file".into(), s("LICENSE"));
    e.insert(
        "target".into(),
        s(&format!("plugins/anthology/knowledge/{name}")),
    );
    e.insert("transform".into(), jv("[]"));
    for (k, v) in kw {
        e.insert((*k).to_string(), v.clone());
    }
    Value::Object(e)
}

/// `del e[key]`.
pub fn del(e: &mut Value, key: &str) {
    e.as_object_mut().unwrap().remove(key);
}

/// `dict(e, **kw)`.
pub fn with(e: &Value, kw: &[(&str, Value)]) -> Value {
    let mut m = e.as_object().unwrap().clone();
    for (k, v) in kw {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

/// `vendor.validate_sources([e, …])`.
pub fn validate(entries: &[Value]) -> Vec<String> {
    vendor::validate_sources(entries)
}

pub fn sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The files of a url source: rel → bytes.
pub type Files = BTreeMap<String, Vec<u8>>;

pub fn files(pairs: &[(&str, &[u8])]) -> Files {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.to_vec()))
        .collect()
}

pub fn checksums_bytes(files: &Files) -> Vec<u8> {
    files
        .iter()
        .map(|(rel, data)| format!("{}  {rel}\n", sha(data)))
        .collect::<String>()
        .into_bytes()
}

pub fn url_files() -> Files {
    files(&[
        ("SKILL.md", b"---\nname: ui\ndescription: d\n---\nbody\n"),
        ("LICENSE", b"Apache License\nVersion 2.0\n"),
        ("ref/a.md", b"a\n"),
        ("scripts/x.mjs", b"export {};\n"),
    ])
}

pub fn url_base(name: &str) -> String {
    format!("https://example.test/.well-known/skills/{name}/")
}

/// `url_entry(name, files=URL_FILES, **kw)`.
pub fn url_entry_with(name: &str, files: &Files, kw: &[(&str, Value)]) -> Value {
    let mut e = json::Map::new();
    e.insert("name".into(), s(name));
    e.insert("url".into(), s(&url_base(name)));
    e.insert("path".into(), s(""));
    e.insert("ref".into(), s("v1"));
    e.insert("checksum".into(), s(&sha(&checksums_bytes(files))));
    e.insert("license".into(), s("Apache-2.0"));
    e.insert("license_file".into(), s("LICENSE"));
    e.insert(
        "target".into(),
        s(&format!("plugins/anthology/knowledge/{name}")),
    );
    e.insert("transform".into(), jv("[]"));
    for (k, v) in kw {
        e.insert((*k).to_string(), v.clone());
    }
    Value::Object(e)
}

pub fn url_entry(name: &str, kw: &[(&str, Value)]) -> Value {
    url_entry_with(name, &url_files(), kw)
}

/// `write_snapshot(root, name, files)`.
pub fn write_snapshot(root: &Path, name: &str, files: &Files) -> PathBuf {
    let base = root.join("vendor").join("snapshots").join(name);
    for (rel, data) in files {
        write(&base, rel, data, false);
    }
    write(&base, "CHECKSUMS.sha256", checksums_bytes(files), false);
    base
}

/// The 22 connector-free knowledge folders (tests/_repo.py `STRIPPED_KNOWLEDGE`): folder →
/// (upstream repo slug, path inside it).
pub fn stripped_knowledge() -> BTreeMap<String, (String, String)> {
    const KWP: &str = "anthropics-knowledge-work-plugins";
    const UX: &str = "uxkin-agent";
    let mut m = BTreeMap::new();
    let mut add = |n: &str, slug: &str, path: String| {
        m.insert(n.to_string(), (slug.to_string(), path));
    };
    for n in [
        "architecture",
        "system-design",
        "testing-strategy",
        "tech-debt",
        "documentation",
        "deploy-checklist",
    ] {
        add(n, KWP, format!("engineering/skills/{n}"));
    }
    add(
        "code-review-checklist",
        KWP,
        "engineering/skills/code-review".to_string(),
    );
    for n in ["design-handoff", "research-synthesis"] {
        add(n, KWP, format!("design/skills/{n}"));
    }
    for n in [
        "write-spec",
        "sprint-planning",
        "roadmap-update",
        "competitive-brief",
        "synthesize-research",
        "product-brainstorming",
    ] {
        add(n, KWP, format!("product-management/skills/{n}"));
    }
    for n in [
        "seo-audit",
        "campaign-plan",
        "content-creation",
        "email-sequence",
    ] {
        add(n, KWP, format!("marketing/skills/{n}"));
    }
    for n in ["landing-page", "web-review", "web-design-system"] {
        add(n, UX, format!("plugins/website-design/skills/{n}"));
    }
    m
}

/// Asserts `r` failed with a message matching the Python `assertRaisesRegex` pattern `re`.
#[track_caller]
pub fn assert_err_matches<T: std::fmt::Debug>(r: Result<T, VendorError>, pattern: &str) {
    match r {
        Ok(v) => panic!("expected an error matching {pattern:?}, got Ok({v:?})"),
        Err(e) => {
            let re = regex::Regex::new(pattern).unwrap();
            assert!(re.is_match(&e.0), "{:?} does not match {pattern:?}", e.0);
        }
    }
}

/// `FixtureCase`: a scratch repo root plus pristine upstream trees, with fetch stubbed.
pub struct Fixture {
    _dir: TempDir,
    pub tmp: PathBuf,
    pub root: PathBuf,
    pub pristine: RefCell<BTreeMap<String, PathBuf>>,
}

impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::Builder::new()
            .prefix("vendor-test-")
            .tempdir()
            .unwrap();
        // Resolved, as Python's tempfile paths compare against resolved checkouts.
        let tmp = fs::canonicalize(dir.path()).unwrap();
        let root = tmp.join("repo");
        fs::create_dir_all(root.join("vendor")).unwrap();
        Fixture {
            _dir: dir,
            tmp,
            root,
            pristine: RefCell::new(BTreeMap::new()),
        }
    }

    /// `add_upstream(name, files, **entry_kw)`: a pristine checkout (with a `.git` dir that must
    /// never be copied); `.sh` files and files under `hooks/` are executable.
    pub fn add_upstream(&self, name: &str, files: &[(&str, &str)], kw: &[(&str, Value)]) -> Value {
        let raw: Vec<(&str, &[u8])> = files.iter().map(|(k, v)| (*k, v.as_bytes())).collect();
        self.add_upstream_raw(name, &raw, kw)
    }

    pub fn add_upstream_raw(
        &self,
        name: &str,
        files: &[(&str, &[u8])],
        kw: &[(&str, Value)],
    ) -> Value {
        let repo = self.tmp.join("upstream").join(name);
        write(&repo, ".git/HEAD", "ref: refs/heads/main\n", false);
        for (rel, text) in files {
            let exec_bit = rel.ends_with(".sh") || rel.starts_with("hooks/");
            write(&repo, rel, text, exec_bit);
        }
        self.pristine.borrow_mut().insert(name.to_string(), repo);
        entry(name, kw)
    }

    /// The pristine checkout of `name`.
    pub fn upstream(&self, name: &str) -> PathBuf {
        self.pristine.borrow()[name].clone()
    }

    /// `fetch(e)`: the stubbed fetcher.
    pub fn fetch(&self, e: &Entry) -> vendor::Result<PathBuf> {
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        Ok(self.pristine.borrow()[name].clone())
    }

    /// `set_sources(entries)`: `json.dumps({"sources": entries}, indent=2) + "\n"`.
    pub fn set_sources(&self, entries: &[Value]) {
        let mut top = json::Map::new();
        top.insert("sources".into(), Value::Array(entries.to_vec()));
        let text = json::dumps(&Value::Object(top), &json::INDENT2) + "\n";
        fs::write(self.root.join("vendor").join("sources.json"), text).unwrap();
    }
}
