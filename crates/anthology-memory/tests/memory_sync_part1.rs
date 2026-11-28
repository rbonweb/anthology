//! Port of `tests/test_memory_sync.py`, part 1 (MS1): remote normalization, repo keys,
//! approval, the union merge attributes and the generated `MEMORY.md` index. The split with
//! MS2 is recorded in `port/ledger/test_memory_sync.tsv`.
//!
//! Python patched `os.environ` (`MemCase`); here every call gets the case's `Env` and the real
//! git (`RealGit`) explicitly. Cases that need git return early when it is missing, as Python
//! skips them.

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anthology_core::Env;
use anthology_core::git::RealGit;
use anthology_memory::approve::{UNION_LINES, approve, ensure_union_attrs, is_approved, unapprove};
use anthology_memory::render::{Budget, DEFAULT_BUDGET, GENERATED, regenerate_index};
use anthology_memory::repo::{normalize_remote, repo_key};
use anthology_pycompat::json;
use anthology_pycompat::path::realpath;
use anthology_testkit::mem::{MemCase, Note, needs_git, try_git, write};

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn lines_of(p: &Path) -> Vec<String> {
    read(p).lines().map(str::to_string).collect()
}

fn note(name: &str, type_: &str, desc: &str, updated: Option<&str>) -> String {
    let mut n = Note::new(name).type_(type_).desc(desc);
    if let Some(u) = updated {
        n = n.updated(u);
    }
    n.text()
}

fn path_key(p: &Path) -> String {
    format!("path:{}", p.display())
}

/// `^- (\d+)<tail>$`: the count of an "N more" pointer line with exactly that tail.
fn pointer_count(line: &str, tail: &str) -> Option<usize> {
    let rest = line.strip_prefix("- ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || &rest[digits.len()..] != tail {
        return None;
    }
    digits.parse().ok()
}

/// `re.match(r"^- (\d+) more", l)`.
fn more_prefix(line: &str) -> Option<usize> {
    let rest = line.strip_prefix("- ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with(" more") {
        return None;
    }
    digits.parse().ok()
}

const MORE: &str = " more — use `anthology-mem search`";

fn set_mtime(p: &Path, t: SystemTime) {
    fs::File::options()
        .write(true)
        .open(p)
        .and_then(|f| f.set_modified(t))
        .unwrap_or_else(|e| panic!("utime {}: {e}", p.display()));
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).and_then(|m| m.modified()).expect("mtime")
}

mod normalize_remote_test {
    use super::*;

    fn norm(url: &str) -> String {
        normalize_remote(url, None, &Env::default())
    }

    #[test]
    fn test_forms_that_name_the_same_repo() {
        for url in [
            "git@github.com:Acme/Repo.git",
            "https://github.com/acme/repo",
            "ssh://git@github.com/acme/repo.git/",
            "https://user:tok@github.com/acme/repo",
            "http://github.com/ACME/REPO.git",
            "git://github.com/acme/repo",
            "ssh://git@github.com:22/acme/repo.git",
            "https://github.com:443/acme/repo/",
            "github.com:acme/repo",
        ] {
            assert_eq!(norm(url), "github.com/acme/repo", "{url}");
        }
    }

    #[test]
    fn test_credentials_never_survive() {
        let key = norm("https://me:s3cret-token@git.example.invalid/a/b.git");
        assert_eq!(key, "git.example.invalid/a/b");
        assert!(!key.contains("s3cret"));
        assert!(!key.split('/').next().expect("host").contains("me"));
    }

    #[test]
    fn test_non_default_port_is_kept() {
        assert_eq!(
            norm("ssh://git@host.example.invalid:2222/a/b.git"),
            "host.example.invalid:2222/a/b"
        );
    }

    #[test]
    fn test_local_paths_become_path_keys() {
        let tmp = realpath(Path::new(
            &std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into()),
        ));
        let srv = path_key(&realpath(Path::new("/srv/x")));
        assert_eq!(norm("/srv/x.git"), srv);
        assert_eq!(norm("file:///srv/x.git"), srv);
        assert!(norm(&format!("{}/y.git", tmp.display())).starts_with("path:"));
    }

    #[test]
    fn test_scp_form_with_absolute_path_is_not_a_local_path() {
        assert_eq!(
            norm("git@host.example.invalid:/srv/x.git"),
            "host.example.invalid/srv/x"
        );
    }
}

mod repo_key_test {
    use super::*;

    fn key(case: &MemCase, root: &Path) -> String {
        repo_key(root, &case.env, &RealGit)
    }

    #[test]
    fn test_origin_url_is_normalised() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", Some("https://user:tok@github.com/Acme/Repo.git"));
        assert_eq!(key(&case, &root), "github.com/acme/repo");
    }

    #[test]
    fn test_origin_wins_over_other_remotes() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", Some("https://github.com/acme/repo"));
        case.git(
            &root,
            &["remote", "add", "aaa", "https://github.com/other/thing"],
        );
        assert_eq!(key(&case, &root), "github.com/acme/repo");
    }

    #[test]
    fn test_alphabetically_first_remote_without_origin() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", None);
        case.git(&root, &["remote", "add", "zeta", "https://github.com/z/z"]);
        case.git(&root, &["remote", "add", "alpha", "https://github.com/a/a"]);
        assert_eq!(key(&case, &root), "github.com/a/a");
    }

    #[test]
    fn test_no_remote_is_the_realpath() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", None);
        assert_eq!(key(&case, &root), path_key(&root));
    }

    #[test]
    fn test_linked_worktree_shares_the_key() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", None);
        write(&root.join("f"), "x", None);
        case.commit_all(&root, "c");
        let wt = case.tmp.join("wt");
        let wt_arg = wt.to_str().expect("utf-8 temp path");
        case.git(&root, &["worktree", "add", "-q", wt_arg, "-b", "wt"]);
        assert_eq!(key(&case, &wt), key(&case, &root));
        assert_eq!(key(&case, &wt), path_key(&root));
    }

    #[test]
    fn test_not_a_repo_falls_back_to_the_path() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let plain = case.tmp.join("plain");
        fs::create_dir(&plain).expect("mkdir");
        assert_eq!(key(&case, &plain), path_key(&plain));
    }
}

mod approval_test {
    use super::*;

    fn new_root(case: &MemCase) -> PathBuf {
        let root = case.tmp.join("r");
        fs::create_dir(&root).expect("mkdir");
        root
    }

    #[test]
    fn test_approve_records_key_and_adds_union_attrs() {
        let case = MemCase::new();
        let root = new_root(&case);
        let key = approve(&root, &case.env, &RealGit).expect("approve");
        assert_eq!(key, path_key(&root));
        assert!(is_approved(&key, &case.env));
        let data = json::loads(&read(&case.anth.join("approved-repos.json"))).expect("json");
        let data = data.as_object().expect("object");
        assert_eq!(
            data.get("version"),
            Some(&json::Value::Int(json::Int::Small(1)))
        );
        let date = data
            .get("repos")
            .and_then(json::Value::as_object)
            .and_then(|r| r.get(&key))
            .and_then(json::Value::as_object)
            .and_then(|e| e.get("approved"))
            .and_then(json::Value::as_str)
            .expect("approved date");
        let shape: Vec<bool> = date.chars().map(|c| c.is_ascii_digit()).collect();
        assert_eq!(
            shape,
            [true, true, true, true, false, true, true, false, true, true],
            "{date}"
        );
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        let attrs = lines_of(&root.join(".gitattributes"));
        for line in UNION_LINES {
            assert!(attrs.iter().any(|a| a == line), "{line}");
        }
    }

    #[test]
    fn test_unapprove() {
        let case = MemCase::new();
        let root = new_root(&case);
        let key = approve(&root, &case.env, &RealGit).expect("approve");
        assert!(unapprove(&key, &case.env).expect("unapprove"));
        assert!(!is_approved(&key, &case.env));
        assert!(!unapprove(&key, &case.env).expect("unapprove"));
    }

    #[test]
    fn test_invalid_file_means_nothing_approved_and_approve_recovers() {
        let case = MemCase::new();
        let file = case.anth.join("approved-repos.json");
        write(&file, "{not json", None);
        assert!(!is_approved("anything", &case.env));
        write(&file, r#"["list"]"#, None);
        assert!(!is_approved("anything", &case.env));
        let root = new_root(&case);
        let key = approve(&root, &case.env, &RealGit).expect("approve");
        assert!(is_approved(&key, &case.env));
    }

    #[test]
    fn test_no_temp_files_left_behind() {
        let case = MemCase::new();
        let root = new_root(&case);
        approve(&root, &case.env, &RealGit).expect("approve");
        let names: Vec<String> = fs::read_dir(&case.anth)
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["approved-repos.json"]);
    }

    #[test]
    fn test_credentials_are_never_stored() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", Some("https://user:tok3n@github.com/Acme/Repo.git"));
        let key = approve(&root, &case.env, &RealGit).expect("approve");
        assert_eq!(key, "github.com/acme/repo");
        assert!(!read(&case.anth.join("approved-repos.json")).contains("tok3n"));
    }
}

mod union_attrs_test {
    use super::*;

    fn new_root(case: &MemCase) -> PathBuf {
        let root = case.tmp.join("r");
        fs::create_dir(&root).expect("mkdir");
        root
    }

    #[test]
    fn test_creates_and_is_idempotent() {
        let case = MemCase::new();
        let root = new_root(&case);
        assert!(ensure_union_attrs(&root).expect("attrs"));
        let first = read(&root.join(".gitattributes"));
        assert_eq!(first.lines().collect::<Vec<_>>(), UNION_LINES);
        assert!(first.ends_with('\n'));
        assert!(!ensure_union_attrs(&root).expect("attrs"));
        assert_eq!(read(&root.join(".gitattributes")), first);
    }

    #[test]
    fn test_keeps_existing_content_and_adds_missing_newline() {
        let case = MemCase::new();
        let root = new_root(&case);
        write(&root.join(".gitattributes"), "*.png binary", None);
        assert!(ensure_union_attrs(&root).expect("attrs"));
        let mut want = vec!["*.png binary"];
        want.extend(UNION_LINES);
        assert_eq!(lines_of(&root.join(".gitattributes")), want);
    }

    #[test]
    fn test_adds_only_the_missing_line() {
        let case = MemCase::new();
        let root = new_root(&case);
        write(
            &root.join(".gitattributes"),
            &format!("{}\n", UNION_LINES[0]),
            None,
        );
        assert!(ensure_union_attrs(&root).expect("attrs"));
        assert_eq!(lines_of(&root.join(".gitattributes")), UNION_LINES);
    }
}

fn regen(d: &Path) -> bool {
    regenerate_index(d, DEFAULT_BUDGET, &[]).expect("regenerate_index")
}

mod regenerate_index_test {
    use super::*;

    fn mem(case: &MemCase, notes: &[(&str, String)]) -> PathBuf {
        let d = case.tmp.join("mem");
        for (fname, text) in notes {
            write(&d.join(fname), text, None);
        }
        d
    }

    #[test]
    fn test_groups_by_type_newest_first() {
        let case = MemCase::new();
        let d = mem(
            &case,
            &[
                (
                    "project-old.md",
                    note("project-old", "project", "Old thing", Some("2026-01-01")),
                ),
                (
                    "project-new.md",
                    note("project-new", "project", "New thing", Some("2026-03-01")),
                ),
                (
                    "user-me.md",
                    note("user-me", "user", "About me", Some("2026-02-01")),
                ),
                (
                    "feedback-x.md",
                    note("feedback-x", "feedback", "Do x", Some("2026-02-02")),
                ),
                ("ref-a.md", note("ref-a", "reference", "Where a", None)),
                ("zed.md", note("zed", "zeta", "Custom type", None)),
                ("alpha.md", note("alpha", "aardvark", "Other custom", None)),
            ],
        );
        assert!(regen(&d));
        let lines = lines_of(&d.join("MEMORY.md"));
        assert!(lines[0].starts_with("<!-- Generated by anthology"));
        let heads: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("## "))
            .map(String::as_str)
            .collect();
        assert_eq!(
            heads,
            [
                "## user",
                "## feedback",
                "## project",
                "## reference",
                "## aardvark",
                "## zeta"
            ]
        );
        let new = "- [project-new](project-new.md) — New thing";
        let old = "- [project-old](project-old.md) — Old thing";
        let at = |s: &str| lines.iter().position(|l| l == s).expect(s);
        assert!(at(new) < at(old));
    }

    #[test]
    fn test_dated_before_undated_ties_by_name() {
        let case = MemCase::new();
        let d = mem(
            &case,
            &[
                ("b.md", note("b", "project", "b", None)),
                ("a.md", note("a", "project", "a", None)),
                ("c.md", note("c", "project", "c", Some("2026-01-01"))),
                ("d.md", note("d", "project", "d", Some("2026-01-01"))),
            ],
        );
        regen(&d);
        let names: Vec<String> = lines_of(&d.join("MEMORY.md"))
            .iter()
            .filter(|l| l.starts_with("- ["))
            .map(|e| e.split(']').next().expect("name")[3..].to_string())
            .collect();
        assert_eq!(names, ["c", "d", "a", "b"]);
    }

    #[test]
    fn test_uses_name_over_stem_and_top_level_fields() {
        let case = MemCase::new();
        let text = Note::new("Pretty Name")
            .type_("feedback")
            .desc("d")
            .updated("2026-05-05")
            .top_level_type()
            .text();
        let d = mem(&case, &[("file-stem.md", text)]);
        regen(&d);
        let text = read(&d.join("MEMORY.md"));
        assert!(text.contains("## feedback"));
        assert!(text.contains("- [Pretty Name](file-stem.md) — d"));
    }

    #[test]
    fn test_unparseable_frontmatter_is_listed_by_stem() {
        let case = MemCase::new();
        let d = mem(
            &case,
            &[
                ("broken.md", "---\nname: [unclosed\n  : :\n".to_string()),
                ("plain.md", "no frontmatter\n".to_string()),
            ],
        );
        regen(&d);
        let text = read(&d.join("MEMORY.md"));
        assert!(text.contains("## other"));
        assert!(text.contains("[broken](broken.md)"));
        assert!(text.contains("[plain](plain.md)"));
    }

    #[test]
    fn test_skips_memory_md_symlinks_and_non_md() {
        let case = MemCase::new();
        let d = mem(&case, &[("a.md", Note::new("a").text())]);
        write(&d.join("notes.txt"), "x", None);
        symlink(d.join("a.md"), d.join("link.md")).expect("symlink");
        write(
            &d.join("MEMORY.md"),
            "stale generated content that is not a note\n",
            None,
        );
        regen(&d);
        let text = read(&d.join("MEMORY.md"));
        assert_eq!(text.matches("](a.md)").count(), 1);
        assert!(!text.contains("link"));
        assert!(!text.contains("notes.txt"));
        assert!(!text.contains("stale generated"));
    }

    #[test]
    fn test_descriptions_are_single_lined_and_lines_capped() {
        let case = MemCase::new();
        let d = mem(
            &case,
            &[
                (
                    "a.md",
                    "---\nname: a\ndescription: \"first\\nsecond   spaced\"\n---\nx\n".to_string(),
                ),
                ("b.md", Note::new("b").desc(&"w".repeat(500)).text()),
            ],
        );
        regen(&d);
        let lines = lines_of(&d.join("MEMORY.md"));
        assert!(lines.iter().all(|l| l.chars().count() <= 200));
        assert!(!lines.iter().any(|l| l.starts_with("second")));
    }

    #[test]
    fn test_deterministic_across_creation_order_and_mtimes() {
        let case = MemCase::new();
        let types = ["project", "feedback", "user"];
        let mut notes: Vec<(String, String)> = (0..30)
            .map(|i| {
                let name = format!("n{i:02}");
                let text = note(
                    &name,
                    types[i % 3],
                    &format!("desc {i}"),
                    Some(&format!("2026-01-{:02}", (i % 28) + 1)),
                );
                (format!("{name}.md"), text)
            })
            .collect();
        notes.sort();
        let a = case.tmp.join("a");
        let b = case.tmp.join("b");
        for (k, text) in &notes {
            write(&a.join(k), text, None);
        }
        for (n, (k, text)) in notes.iter().rev().enumerate() {
            let p = write(&b.join(k), text, None);
            let secs = 1_000_000 + n as u64 * 1000;
            set_mtime(&p, UNIX_EPOCH + Duration::from_secs(secs));
        }
        regen(&a);
        regen(&b);
        assert_eq!(
            fs::read(a.join("MEMORY.md")).expect("a"),
            fs::read(b.join("MEMORY.md")).expect("b")
        );
    }

    #[test]
    fn test_generated_text_has_no_paths_or_times() {
        let case = MemCase::new();
        let d = mem(&case, &[("a.md", Note::new("a").text())]);
        regen(&d);
        assert!(!read(&d.join("MEMORY.md")).contains(&case.tmp.display().to_string()));
    }

    #[test]
    fn test_unchanged_run_writes_nothing() {
        let case = MemCase::new();
        let d = mem(
            &case,
            &[
                ("a.md", Note::new("a").text()),
                ("b.md", Note::new("b").type_("user").text()),
            ],
        );
        assert!(regen(&d));
        let index = d.join("MEMORY.md");
        set_mtime(&index, mtime(&index) - Duration::from_secs(5));
        let before = mtime(&index);
        assert!(!regen(&d));
        assert_eq!(mtime(&index), before);
        write(&d.join("c.md"), &Note::new("c").text(), None);
        assert!(regen(&d));
        assert!(read(&index).contains("](c.md)"));
    }

    #[test]
    fn test_no_notes_writes_nothing() {
        let case = MemCase::new();
        let d = case.tmp.join("empty");
        fs::create_dir(&d).expect("mkdir");
        assert!(!regen(&d));
        assert!(!d.join("MEMORY.md").exists());
        write(&d.join("MEMORY.md"), "keep me\n", None);
        assert!(!regen(&d));
        assert_eq!(read(&d.join("MEMORY.md")), "keep me\n");
        assert!(!regen(&case.tmp.join("missing")));
    }

    #[test]
    fn test_300_notes_stay_within_budget_with_more_pointers() {
        let case = MemCase::new();
        let types = ["feedback", "project", "reference"];
        let notes: Vec<(String, String)> = (0..300)
            .map(|i| {
                let t = types[i % 3];
                let name = format!("{t}-{i:03}");
                let date = format!("2026-{:02}-{:02}", (i / 28) % 12 + 1, i % 28 + 1);
                let text = note(&name, t, &format!("Description number {i}"), Some(&date));
                (format!("{name}.md"), text)
            })
            .collect();
        let refs: Vec<(&str, String)> =
            notes.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let d = mem(&case, &refs);
        regen(&d);
        let raw = fs::read(d.join("MEMORY.md")).expect("index");
        let text = String::from_utf8(raw.clone()).expect("utf-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!((DEFAULT_BUDGET.lines, DEFAULT_BUDGET.bytes), (150, 8000));
        assert!(lines.len() <= DEFAULT_BUDGET.lines);
        assert!(raw.len() <= DEFAULT_BUDGET.bytes);
        let pointers = lines
            .iter()
            .filter(|l| pointer_count(l, MORE).is_some())
            .count();
        assert_eq!(pointers, 3);
        let mut shown: Vec<(String, usize)> = Vec::new();
        let mut hidden: Vec<(String, usize)> = Vec::new();
        let mut current = String::new();
        for l in &lines {
            if let Some(t) = l.strip_prefix("## ") {
                current = t.to_string();
                shown.push((current.clone(), 0));
            } else if l.starts_with("- [") {
                shown.last_mut().expect("a heading").1 += 1;
            }
            if let Some(n) = more_prefix(l) {
                hidden.push((current.clone(), n));
            }
        }
        let get = |v: &[(String, usize)], t: &str| v.iter().find(|(k, _)| k == t).map(|e| e.1);
        for t in types {
            let s = get(&shown, t).expect("shown");
            assert!(s > 0, "{t}");
            assert_eq!(s + get(&hidden, t).expect("hidden"), 100, "{t}");
        }
        let counts: Vec<usize> = shown.iter().map(|e| e.1).collect();
        let (max, min) = (counts.iter().max(), counts.iter().min());
        assert!(max.expect("max") - min.expect("min") < 2);
    }

    #[test]
    fn test_most_recent_notes_survive_truncation() {
        let case = MemCase::new();
        let mut notes: Vec<(String, String)> = (0..60)
            .map(|i| {
                let name = format!("p{i:03}");
                let date = format!("2026-01-{:02}", i % 28 + 1);
                (
                    format!("{name}.md"),
                    note(&name, "project", "d", Some(&date)),
                )
            })
            .collect();
        notes.push((
            "newest.md".into(),
            note("newest", "project", "d", Some("2030-01-01")),
        ));
        let refs: Vec<(&str, String)> =
            notes.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let d = mem(&case, &refs);
        regenerate_index(
            &d,
            Budget {
                lines: 10,
                bytes: 20000,
            },
            &[],
        )
        .expect("regenerate_index");
        let lines = lines_of(&d.join("MEMORY.md"));
        assert!(lines.len() <= 10);
        assert!(lines[2].contains("[newest](newest.md)"));
        assert!(
            lines
                .last()
                .expect("last")
                .ends_with("more — use `anthology-mem search`")
        );
    }

    #[test]
    fn test_byte_budget_is_enforced() {
        let case = MemCase::new();
        let notes: Vec<(String, String)> = (0..120)
            .map(|i| {
                let name = format!("n{i:03}");
                let date = format!("2026-01-{:02}", i % 28 + 1);
                (
                    format!("{name}.md"),
                    note(&name, "project", &"x".repeat(170), Some(&date)),
                )
            })
            .collect();
        let refs: Vec<(&str, String)> =
            notes.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let d = mem(&case, &refs);
        regen(&d);
        let raw = fs::read(d.join("MEMORY.md")).expect("index");
        assert!(raw.len() <= 20000);
        let text = String::from_utf8(raw).expect("utf-8");
        assert!(text.lines().count() <= 150);
        assert!(text.contains("more — use `anthology-mem search`"));
    }
}

mod team_merge_test {
    use super::*;

    fn scenario(case: &MemCase, with_attrs: bool) -> (PathBuf, PathBuf, i32, String) {
        let root = case.make_repo("repo", None);
        let mem = root.join(".claude").join("memory");
        write(
            &mem.join("project-base.md"),
            &note("project-base", "project", "Base", Some("2026-01-01")),
            None,
        );
        regen(&mem);
        if with_attrs {
            ensure_union_attrs(&root).expect("attrs");
        }
        case.commit_all(&root, "base");
        case.git(&root, &["checkout", "-q", "-b", "one"]);
        write(
            &mem.join("project-one.md"),
            &note("project-one", "project", "One", Some("2026-02-01")),
            None,
        );
        regen(&mem);
        case.commit_all(&root, "one");
        case.git(&root, &["checkout", "-q", "main"]);
        case.git(&root, &["checkout", "-q", "-b", "two"]);
        write(
            &mem.join("project-two.md"),
            &note("project-two", "project", "Two", Some("2026-02-02")),
            None,
        );
        regen(&mem);
        case.commit_all(&root, "two");
        case.git(&root, &["checkout", "-q", "one"]);
        let merge = try_git(&case.env, &root, &["merge", "--no-edit", "two"]);
        let out = format!("{}{}", merge.stdout_text(), merge.stderr_text());
        (root, mem, merge.code, out)
    }

    #[test]
    fn test_union_attrs_make_the_merge_succeed() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let (_root, mem, code, out) = scenario(&case, true);
        assert_eq!(code, 0, "{out}");
        let raw = read(&mem.join("MEMORY.md"));
        assert!(raw.contains("](project-one.md)"));
        assert!(raw.contains("](project-two.md)"));
        assert!(!raw.contains("<<<<<<<"));
        regen(&mem);
        let text = read(&mem.join("MEMORY.md"));
        for name in ["project-base", "project-one", "project-two"] {
            assert_eq!(text.matches(&format!("]({name}.md)")).count(), 1, "{name}");
        }
    }

    #[test]
    fn test_without_the_attrs_the_same_merge_conflicts() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let (_root, mem, code, _out) = scenario(&case, false);
        assert_ne!(code, 0);
        assert!(read(&mem.join("MEMORY.md")).contains("<<<<<<<"));
    }

    #[test]
    fn test_regenerate_cleans_up_what_union_leaves_behind() {
        if !needs_git() {
            return;
        }
        let case = MemCase::new();
        let root = case.make_repo("repo", None);
        let mem = root.join(".claude").join("memory");
        write(
            &mem.join("project-a.md"),
            &note("project-a", "project", "A", Some("2026-01-01")),
            None,
        );
        write(
            &mem.join("project-b.md"),
            &note("project-b", "project", "B", Some("2026-01-02")),
            None,
        );
        write(
            &mem.join("MEMORY.md"),
            "<!-- old -->\n## project\n- [project-a](project-a.md) — A\n\
             - [project-b](project-b.md) — B\n- [project-a](project-a.md) — A\n",
            None,
        );
        assert!(regen(&mem));
        let text = read(&mem.join("MEMORY.md"));
        assert_eq!(text.matches("](project-a.md)").count(), 1);
        assert_eq!(text.matches("](project-b.md)").count(), 1);
    }
}

mod many_types_budget_test {
    use super::*;

    #[test]
    fn test_sixty_types_stay_within_the_budget() {
        let case = MemCase::new();
        let d = case.tmp.join("mem");
        for t in 0..60 {
            for n in 0..2 {
                let name = format!("t{t:02}-{n}");
                write(
                    &d.join(format!("{name}.md")),
                    &note(
                        &name,
                        &format!("type{t:02}"),
                        "d",
                        Some(&format!("2026-01-{:02}", n + 1)),
                    ),
                    None,
                );
            }
        }
        let budget = Budget {
            lines: 30,
            bytes: 20000,
        };
        regenerate_index(&d, budget, &[]).expect("regenerate_index");
        let lines = lines_of(&d.join("MEMORY.md"));
        assert!(lines.len() <= 30);
        assert_eq!(lines[0], GENERATED);
        assert_eq!(lines[1], "## type00");
        let tail = lines.last().expect("tail");
        let hidden = pointer_count(tail, " more in other types — use `anthology-mem search`")
            .unwrap_or_else(|| panic!("collapsed pointer: {tail}"));
        let shown_types = lines.iter().filter(|l| l.starts_with("## ")).count();
        let pointers: usize = lines[..lines.len() - 1]
            .iter()
            .filter(|l| more_prefix(l).is_some() && l.contains(" more — use"))
            .map(|l| more_prefix(l).expect("count"))
            .sum();
        let entries = lines.iter().filter(|l| l.starts_with("- [")).count();
        assert_eq!(hidden + pointers + entries, 120);
        assert_eq!(hidden, 2 * (60 - shown_types));
        regenerate_index(
            &d,
            Budget {
                lines: 150,
                bytes: 600,
            },
            &[],
        )
        .expect("regenerate_index");
        assert!(fs::read(d.join("MEMORY.md")).expect("index").len() <= 600);
    }

    #[test]
    fn test_all_types_fit_means_no_collapsed_pointer() {
        let case = MemCase::new();
        let d = case.tmp.join("mem");
        for t in 0..5 {
            write(
                &d.join(format!("t{t}.md")),
                &Note::new(&format!("t{t}"))
                    .type_(&format!("type{t}"))
                    .text(),
                None,
            );
        }
        regen(&d);
        assert!(!read(&d.join("MEMORY.md")).contains("other types"));
    }
}

mod special_memory_md_test {
    use super::*;

    #[cfg(target_os = "macos")]
    const O_NONBLOCK: i32 = 0x0004;
    #[cfg(not(target_os = "macos"))]
    const O_NONBLOCK: i32 = 0o4000;

    struct Setup {
        _case: MemCase,
        agent: PathBuf,
        outside: PathBuf,
    }

    fn set_up() -> Setup {
        let case = MemCase::new();
        let agent = case.tmp.join("agent");
        write(
            &agent.join("project-a.md"),
            &Note::new("project-a").desc("Thing A").text(),
            None,
        );
        let outside = case.tmp.join("outside");
        fs::create_dir(&outside).expect("mkdir");
        Setup {
            _case: case,
            agent,
            outside,
        }
    }

    fn mkfifo(p: &Path) {
        let ok = std::process::Command::new("mkfifo")
            .arg(p)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "mkfifo {}", p.display());
    }

    /// Lets a stuck reader finish, so its thread does not outlive the test.
    fn unblock(fifo: &Path) {
        let _ = fs::OpenOptions::new()
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(fifo);
    }

    /// `run_with_timeout` + `assertPromptlyRegular`: `regenerate_index` finishes within 5 s
    /// without an error and leaves a regular `MEMORY.md` listing the note; returns its result.
    fn promptly_regular(s: &Setup, fifo: Option<&Path>) -> bool {
        let (tx, rx) = mpsc::channel();
        let agent = s.agent.clone();
        std::thread::spawn(move || {
            let _ = tx.send(regenerate_index(&agent, DEFAULT_BUDGET, &[]));
        });
        let result = rx.recv_timeout(Duration::from_secs(5));
        if let Some(f) = fifo {
            unblock(f);
        }
        let result = result
            .expect("regenerating MEMORY.md blocked on a special file")
            .expect("no error");
        let target = s.agent.join("MEMORY.md");
        let meta = fs::symlink_metadata(&target).expect("lstat");
        assert!(meta.file_type().is_file());
        assert!(read(&target).contains("[project-a](project-a.md)"));
        result
    }

    #[test]
    fn test_a_fifo_at_memory_md_is_replaced_without_blocking() {
        let s = set_up();
        let fifo = s.agent.join("MEMORY.md");
        mkfifo(&fifo);
        assert!(promptly_regular(&s, Some(&fifo)));
    }

    #[test]
    fn test_a_symlink_to_a_fifo_is_replaced_without_blocking() {
        let s = set_up();
        let fifo = s.outside.join("pipe");
        mkfifo(&fifo);
        symlink(&fifo, s.agent.join("MEMORY.md")).expect("symlink");
        assert!(promptly_regular(&s, Some(&fifo)));
    }

    #[test]
    fn test_a_symlink_to_a_file_with_the_same_text_is_still_replaced() {
        let s = set_up();
        regen(&s.agent);
        let text = fs::read(s.agent.join("MEMORY.md")).expect("index");
        let victim = write(&s.outside.join("victim.md"), "", None);
        fs::write(&victim, &text).expect("victim");
        fs::remove_file(s.agent.join("MEMORY.md")).expect("unlink");
        symlink(&victim, s.agent.join("MEMORY.md")).expect("symlink");
        assert!(promptly_regular(&s, None));
        assert_eq!(fs::read(&victim).expect("victim"), text);
    }

    #[test]
    fn test_a_symlink_to_a_different_file_leaves_the_target_alone() {
        let s = set_up();
        let victim = write(&s.outside.join("victim.md"), "precious", None);
        symlink(&victim, s.agent.join("MEMORY.md")).expect("symlink");
        promptly_regular(&s, None);
        assert_eq!(read(&victim), "precious");
    }
}
