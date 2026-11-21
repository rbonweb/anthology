//! test_vendor.UrlDownloadTest: `vendor fetch` of a url source downloads into a temporary
//! directory, verifies everything, and only then replaces the committed snapshot.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anthology_dev::vendor::{self, Entry, VendorError};
use anthology_pycompat::json::Value;

use crate::fixture::{
    Files, Fixture, assert_err_matches, checksums_bytes, entry, files, names, s, set, sha,
    tree_files, url_base, url_entry, url_files, write_snapshot,
};

/// `FakeWeb`: a getter serving one skill (files, CHECKSUMS.sha256, index.json) from memory.
struct FakeWeb {
    base: String,
    index_url: String,
    pages: RefCell<BTreeMap<String, Vec<u8>>>,
    calls: RefCell<Vec<String>>,
}

impl FakeWeb {
    fn new(name: &str) -> FakeWeb {
        FakeWeb::with(name, &url_files(), None, None)
    }

    fn with(
        name: &str,
        files: &Files,
        sums: Option<Vec<u8>>,
        index_files: Option<Vec<String>>,
    ) -> FakeWeb {
        let base = url_base(name);
        let cut = base.rfind(&format!("{name}/")).unwrap();
        let index_url = format!("{}index.json", &base[..cut]);
        let mut pages: BTreeMap<String, Vec<u8>> = files
            .iter()
            .map(|(rel, data)| (format!("{base}{rel}"), data.clone()))
            .collect();
        pages.insert(
            format!("{base}CHECKSUMS.sha256"),
            sums.unwrap_or_else(|| checksums_bytes(files)),
        );
        let listed = index_files.unwrap_or_else(|| {
            let mut v: Vec<String> = files.keys().cloned().collect();
            v.push("CHECKSUMS.sha256".into());
            v
        });
        let listed = listed
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let index = format!(
            r#"{{"skills": [{{"name": "other", "files": ["x"]}}, {{"name": "{name}", "files": [{listed}]}}]}}"#
        );
        pages.insert(index_url.clone(), index.into_bytes());
        FakeWeb {
            base,
            index_url,
            pages: RefCell::new(pages),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn get(&self, url: &str) -> vendor::Result<Vec<u8>> {
        self.calls.borrow_mut().push(url.to_string());
        self.pages
            .borrow()
            .get(url)
            .cloned()
            .ok_or_else(|| VendorError(format!("HTTP 404: {url}")))
    }

    fn page(&self, rel: &str, data: &[u8]) {
        self.pages
            .borrow_mut()
            .insert(format!("{}{rel}", self.base), data.to_vec());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

struct Case {
    f: Fixture,
    e: Value,
    snap: PathBuf,
}

fn setup() -> Case {
    let f = Fixture::new();
    let e = url_entry("ui", &[]);
    f.set_sources(std::slice::from_ref(&e));
    let snap = f.root.join("vendor").join("snapshots").join("ui");
    Case { f, e, snap }
}

impl Case {
    fn download(&self, web: &FakeWeb) -> vendor::Result<PathBuf> {
        vendor::download_snapshot(
            &self.f.root,
            self.e.as_object().unwrap(),
            Some(&|u: &str| web.get(u)),
        )
    }

    #[track_caller]
    fn assert_refused_and_untouched(&self, web: &FakeWeb, pattern: &str) {
        write_snapshot(
            &self.f.root,
            "ui",
            &files(&[("SKILL.md", b"keep\n"), ("LICENSE", b"keep\n")]),
        );
        let before = tree_files(&self.snap);
        assert_err_matches(self.download(web), pattern);
        assert_eq!(tree_files(&self.snap), before);
        assert_eq!(names(self.snap.parent().unwrap()), ["ui"]);
    }
}

fn everything() -> Vec<String> {
    let mut v: Vec<String> = url_files().into_keys().collect();
    v.push("CHECKSUMS.sha256".into());
    v
}

#[test]
fn test_download_writes_a_verified_snapshot() {
    let c = setup();
    let web = FakeWeb::new("ui");
    assert_eq!(c.download(&web).unwrap(), c.snap);
    let mut want = url_files();
    want.insert("CHECKSUMS.sha256".into(), checksums_bytes(&url_files()));
    assert_eq!(tree_files(&c.snap), want);
    assert_eq!(
        vendor::snapshot_fetcher(&c.f.root)(c.e.as_object().unwrap()).unwrap(),
        c.snap
    );
    assert_eq!(names(c.snap.parent().unwrap()), ["ui"]);
    assert_eq!(web.calls()[0], format!("{}CHECKSUMS.sha256", web.base));
}

#[test]
fn test_download_replaces_an_older_snapshot() {
    let c = setup();
    write_snapshot(
        &c.f.root,
        "ui",
        &files(&[
            ("SKILL.md", b"old\n"),
            ("LICENSE", b"old\n"),
            ("stale.md", b"s\n"),
        ]),
    );
    c.download(&FakeWeb::new("ui")).unwrap();
    assert!(!c.snap.join("stale.md").exists());
    assert_eq!(
        fs::read(c.snap.join("SKILL.md")).unwrap(),
        url_files()["SKILL.md"]
    );
}

#[test]
fn test_a_wrong_pin_refuses_before_downloading_any_file() {
    let mut c = setup();
    set(&mut c.e, "checksum", s(&"0".repeat(64)));
    let web = FakeWeb::new("ui");
    c.assert_refused_and_untouched(&web, "checksum");
    assert_eq!(web.calls(), [format!("{}CHECKSUMS.sha256", web.base)]);
}

#[test]
fn test_a_file_that_does_not_match_its_checksum_refuses() {
    let c = setup();
    let web = FakeWeb::new("ui");
    web.page("ref/a.md", b"tampered\n");
    c.assert_refused_and_untouched(&web, "ref/a.md");
}

#[test]
fn test_a_missing_file_refuses() {
    let c = setup();
    let web = FakeWeb::new("ui");
    web.pages
        .borrow_mut()
        .remove(&format!("{}ref/a.md", web.base));
    c.assert_refused_and_untouched(&web, "404");
}

#[test]
fn test_an_index_that_lists_another_file_set_refuses() {
    let c = setup();
    let mut more = everything();
    more.push("more.md".into());
    c.assert_refused_and_untouched(
        &FakeWeb::with("ui", &url_files(), None, Some(more)),
        "more.md",
    );
    let fewer: Vec<String> = everything()
        .into_iter()
        .filter(|f| f != "ref/a.md")
        .collect();
    c.assert_refused_and_untouched(
        &FakeWeb::with("ui", &url_files(), None, Some(fewer)),
        "ref/a.md",
    );
}

#[test]
fn test_an_index_without_the_skill_refuses() {
    let c = setup();
    let web = FakeWeb::new("ui");
    web.pages
        .borrow_mut()
        .insert(web.index_url.clone(), br#"{"skills": []}"#.to_vec());
    c.assert_refused_and_untouched(&web, "index");
}

#[test]
fn test_a_malformed_index_refuses() {
    let c = setup();
    let web = FakeWeb::new("ui");
    web.pages
        .borrow_mut()
        .insert(web.index_url.clone(), b"not json".to_vec());
    c.assert_refused_and_untouched(&web, "index");
}

#[test]
fn test_path_traversal_in_checksums_refuses_and_requests_nothing_else() {
    let mut c = setup();
    for bad in ["../evil.md", "/etc/passwd", "a/../../b.md"] {
        let mut sums = checksums_bytes(&url_files());
        sums.extend(format!("{}  {bad}\n", sha(b"x")).into_bytes());
        set(&mut c.e, "checksum", s(&sha(&sums)));
        let web = FakeWeb::with("ui", &url_files(), Some(sums), None);
        c.assert_refused_and_untouched(&web, "unsafe");
        assert_eq!(
            web.calls(),
            [format!("{}CHECKSUMS.sha256", web.base)],
            "{bad}"
        );
    }
    assert!(!c.f.root.join("vendor").join("evil.md").exists());
    assert!(!c.f.tmp.join("evil.md").exists());
}

#[test]
fn test_the_getter_is_https_only() {
    for bad in [
        "http://example.test/x",
        "file:///etc/passwd",
        "ftp://example.test/x",
        "//x/y",
    ] {
        assert!(vendor::http_get(bad).is_err(), "{bad}");
    }
}

#[test]
fn test_an_invalid_entry_is_refused_before_any_request() {
    let mut c = setup();
    set(
        &mut c.e,
        "url",
        s("http://example.test/.well-known/skills/ui/"),
    );
    c.f.set_sources(std::slice::from_ref(&c.e));
    let web = FakeWeb::new("ui");
    assert!(c.download(&web).is_err());
    assert!(web.calls().is_empty());
}

#[test]
fn test_fetch_entry_downloads_url_entries_and_delegates_the_rest() {
    let c = setup();
    let web = FakeWeb::new("ui");
    let get = |u: &str| web.get(u);
    assert_eq!(
        vendor::fetch_entry(&c.f.root, c.e.as_object().unwrap(), None, Some(&get)).unwrap(),
        c.snap
    );
    let g = entry("g", &[]);
    let fetch =
        |e: &Entry| Ok(Path::new("/x").join(e.get("name").and_then(Value::as_str).unwrap()));
    assert_eq!(
        vendor::fetch_entry(&c.f.root, g.as_object().unwrap(), Some(&fetch), None).unwrap(),
        PathBuf::from("/x/g")
    );
}
