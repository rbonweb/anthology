//! Port of `tests/test_memory_portable.py` (MX1): `ExportTest`, `SecretsTest`, `ImportTest`,
//! `DestinationTest`, `ImportPrivacyTest` and `CliTest`, one module per class.
//! `MaliciousArchiveTest` is `memory_portable_malicious.rs`; `SkillsTest` is CO2's.
//!
//! Python patched `os.environ` (`MemCase`); here [`MemCase::env`] is that environment: it is
//! what `plan_import` and `export` get when Python passed `env=self.env`, and what
//! `apply_import_env` gets for Python's `apply_import(plan)`, which reads `os.environ`. Git is
//! the real one (`needs_git` where Python has it), except for `ImportPrivacyTest`, whose
//! `fake_repo` becomes a fake [`Git`] that answers for one folder. The CLI tests run the built
//! `anthology memory …` with exactly the case's environment.
//!
//! Secret-looking fixtures are assembled at run time, so no literal sits in this file.

use std::fs;
use std::io::Read;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::{Git, GitOut, RealGit};
use anthology_memory::portable::{self, Error, approve, repo};
use anthology_pycompat::json::{self, Map, Value};
use anthology_testkit::mem::{MemCase, Note, needs_git, write};
use anthology_testkit::mx::{Spec, make_archive, tree};
use anthology_testkit::run::{Output, run_bin};

/// One forged-plan mutation of `test_forged_plan_entries_are_refused`.
type Mutation<'a> = Box<dyn Fn(&mut Map) + 'a>;
/// One case of `test_symlinked_destinations_are_refused_and_nothing_is_written`.
type Setup<'a> = Box<dyn Fn() + 'a>;

/// `PortableCase`.
struct Case {
    m: MemCase,
    cwd: PathBuf,
    out: PathBuf,
    personal: PathBuf,
}

impl Case {
    fn new() -> Case {
        let m = MemCase::new();
        let cwd = m.tmp.join("plain");
        fs::create_dir(&cwd).expect("mkdir plain");
        let out = m.tmp.join("out");
        fs::create_dir(&out).expect("mkdir out");
        let personal = m.anth.join("personal-memory");
        Case {
            m,
            cwd,
            out,
            personal,
        }
    }

    fn env(&self) -> &Env {
        &self.m.env
    }

    fn tmp(&self) -> &Path {
        &self.m.tmp
    }

    /// `second_machine(name)`: an environment with its own empty `HOME` and config folder,
    /// and that machine's anthology home.
    fn second_machine(&self, name: &str) -> (Env, PathBuf) {
        let home = self.tmp().join(name).join("home");
        fs::create_dir_all(&home).expect("mkdir second home");
        let env = self
            .env()
            .clone()
            .with_os("HOME", &home)
            .with_os("CLAUDE_CONFIG_DIR", home.join(".claude"));
        (env, home.join(".claude").join("anthology"))
    }

    fn seed_personal(&self) {
        write(
            &self.personal.join("user-me.md"),
            &note("user-me", "user", "About me", "2026-01-02"),
            None,
        );
        write(
            &self.personal.join("feedback-x.md"),
            &note("feedback-x", "feedback", "Do x", "2026-01-03"),
            None,
        );
    }

    /// `archive()`: the seeded personal notes exported to `out` on 2026-04-05.
    fn archive(&self) -> (PathBuf, Map) {
        self.seed_personal();
        let res = export(self, &self.cwd, Some(&self.out), "2026-04-05");
        (PathBuf::from(text(&res, "path")), res)
    }

    /// `run_cli(env, *args)`.
    fn run_cli(&self, env: &Env, args: &[&str]) -> Output {
        let mut argv = vec!["memory"];
        argv.extend_from_slice(args);
        run_bin(&argv, b"", env, &self.cwd)
    }
}

/// `note_text(name, type_, desc, updated)`.
fn note(name: &str, type_: &str, desc: &str, updated: &str) -> String {
    Note::new(name)
        .type_(type_)
        .desc(desc)
        .updated(updated)
        .text()
}

fn p(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 test path")
}

fn export(c: &Case, cwd: &Path, out: Option<&Path>, today: &str) -> Map {
    portable::export(cwd, out, Some(today), c.env(), &RealGit).expect("export")
}

fn plan_with(archive: &Path, cwd: &Path, today: Option<&str>, env: &Env, git: &dyn Git) -> Map {
    portable::plan_import(archive, cwd, today, env, git).expect("plan_import")
}

fn plan(archive: &Path, cwd: &Path, today: Option<&str>, env: &Env) -> Map {
    plan_with(archive, cwd, today, env, &RealGit)
}

/// `apply_import(plan)` under the case's `os.environ`.
fn try_apply(c: &Case, plan: &Map, git: &dyn Git) -> Result<Map, Error> {
    portable::apply_import_env(&Value::Object(plan.clone()), c.env(), git)
}

fn apply(c: &Case, plan: &Map) -> Map {
    try_apply(c, plan, &RealGit).expect("apply_import")
}

fn text<'a>(m: &'a Map, key: &str) -> &'a str {
    m.get(key).and_then(Value::as_str).expect("a str")
}

fn list<'a>(m: &'a Map, key: &str) -> &'a [Value] {
    m.get(key).and_then(Value::as_array).expect("a list")
}

fn object(v: &Value) -> &Map {
    v.as_object().expect("an object")
}

/// `[e["member"] for e in plan[key]]`.
fn members(plan: &Map, key: &str) -> Vec<String> {
    list(plan, key)
        .iter()
        .map(|e| text(object(e), "member").to_string())
        .collect()
}

fn strs(m: &Map, key: &str) -> Vec<String> {
    list(m, key)
        .iter()
        .map(|v| v.as_str().expect("a str").to_string())
        .collect()
}

fn count(m: &Map, key: &str) -> Vec<(String, i64)> {
    let c = object(m.get(key).expect("counts"));
    // Python compares the whole dict: no key beyond these three.
    assert_eq!(c.len(), 3, "{c:?}");
    ["personal", "repo", "agent"]
        .iter()
        .map(|k| {
            (
                k.to_string(),
                c.get(k).and_then(Value::as_i64).expect("int"),
            )
        })
        .collect()
}

fn counts(personal: i64, repo: i64, agent: i64) -> Vec<(String, i64)> {
    vec![
        ("personal".into(), personal),
        ("repo".into(), repo),
        ("agent".into(), agent),
    ]
}

fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v.clone());
    }
    Value::Object(m)
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn sorted_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `sorted(p.name for p in dir.glob(pattern))` for a `<prefix>*<suffix>` pattern.
fn glob(dir: &Path, prefix: &str, suffix: &str) -> Vec<String> {
    sorted_names(dir)
        .into_iter()
        .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
        .collect()
}

fn is_empty_dir(dir: &Path) -> bool {
    sorted_names(dir).is_empty()
}

/// The members of a `.tar.gz`, in order: `(name, header)`.
fn tar_members(path: &Path) -> Vec<(String, tar::Header, Vec<u8>)> {
    let file = fs::File::open(path).expect("open the archive");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut out = Vec::new();
    for entry in archive.entries().expect("entries") {
        let mut entry = entry.expect("entry");
        let name = entry.path().expect("path").to_string_lossy().into_owned();
        let header = entry.header().clone();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read");
        out.push((name, header, data));
    }
    out
}

fn tar_names(path: &Path) -> Vec<String> {
    tar_members(path).into_iter().map(|(n, _, _)| n).collect()
}

/// `fake_repo(name)`: a folder that `memory_sync` treats as a git work tree with the remote
/// key `example.invalid/acme/app`, without running git.
struct FakeGit {
    root: PathBuf,
}

impl Git for FakeGit {
    fn run(&self, _env: &Env, _cwd: &Path, args: &[&str]) -> Option<GitOut> {
        let stdout = match args {
            ["rev-parse", "--show-toplevel"] => self.root.to_string_lossy().into_owned(),
            ["remote"] => "origin".into(),
            ["remote", "get-url", "origin"] => "https://example.invalid/acme/app.git".into(),
            _ => {
                return Some(GitOut {
                    code: 1,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
            }
        };
        Some(GitOut {
            code: 0,
            stdout: format!("{stdout}\n").into_bytes(),
            stderr: Vec::new(),
        })
    }
}

/// `secret_samples()`: secret-looking strings assembled at run time.
fn secret_samples() -> Vec<(&'static str, String)> {
    vec![
        (
            "private-key",
            ["-----BEGIN ", "RSA PRIVATE KEY-----\nabc\n"].concat(),
        ),
        (
            "aws-access-key",
            ["id ", "AKIA", "ABCDEFGHIJKLMNOP"].concat(),
        ),
        ("github-token", ["ghp", "_", &"a1".repeat(20)].concat()),
        ("slack-token", ["xox", "b-", "1234567890-abcdef"].concat()),
        ("api-key", ["sk", "-", "abcdefghij0123456789"].concat()),
        (
            "credential-assignment",
            ["pass", "word: hunter22hunter"].concat(),
        ),
        (
            "jwt",
            [
                "eyJ",
                "hbGciOiJIUzI1NiJ9",
                ".eyJ",
                "zdWIiOiIxMjM0In0",
                ".",
                "c2lnbmF0dXJlLXg",
            ]
            .concat(),
        ),
        ("google-api-key", ["AI", "za", &"B".repeat(35)].concat()),
        (
            "stripe-live-key",
            ["rk", "_live_", "abcdefghij0123"].concat(),
        ),
        (
            "bearer-token",
            ["Authorization: Bear", "er ", "abcdefghij0123456789xyz"].concat(),
        ),
    ]
}

fn sample(label: &str) -> String {
    secret_samples()
        .into_iter()
        .find(|(l, _)| *l == label)
        .expect("a sample")
        .1
}

mod export_test {
    use super::*;

    #[test]
    fn test_archive_shape_and_manifest() {
        let c = Case::new();
        let (path, res) = c.archive();
        assert_eq!(
            path.file_name().unwrap(),
            "anthology-memory-2026-04-05.tar.gz"
        );
        assert_eq!(count(&res, "counts"), counts(2, 0, 0));
        assert_eq!(res.get("repo_included"), Some(&Value::Bool(false)));
        let members = tar_members(&path);
        let names: Vec<&str> = members.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "manifest.json",
                "personal/feedback-x.md",
                "personal/user-me.md"
            ]
        );
        for (_, h, _) in &members {
            assert!(h.entry_type().is_file());
            let uname = h.username().ok().flatten().unwrap_or_default();
            let gname = h.groupname().ok().flatten().unwrap_or_default();
            assert_eq!(
                (
                    h.mode().unwrap(),
                    h.uid().unwrap(),
                    h.gid().unwrap(),
                    uname,
                    gname,
                    h.mtime().unwrap()
                ),
                (0o644, 0, 0, "", "", 0)
            );
        }
        let raw = String::from_utf8(members[0].2.clone()).expect("utf-8");
        let manifest = json::loads(&raw).expect("json");
        let want = obj(&[
            ("format", Value::Int(json::Int::Small(1))),
            ("date", s("2026-04-05")),
            (
                "counts",
                obj(&[
                    ("personal", Value::Int(json::Int::Small(2))),
                    ("repo", Value::Int(json::Int::Small(0))),
                    ("agent", Value::Int(json::Int::Small(0))),
                ]),
            ),
        ]);
        assert_eq!(manifest, want);
        let tmp_name = c.tmp().file_name().unwrap().to_string_lossy().into_owned();
        for private in [p(c.tmp()), p(&c.m.home), tmp_name.as_str(), "home"] {
            assert!(!raw.contains(private), "{private}");
        }
    }

    #[test]
    fn test_deterministic() {
        let c = Case::new();
        c.seed_personal();
        let a = export(&c, &c.cwd, Some(&c.tmp().join("o1")), "2026-04-05");
        let b = export(&c, &c.cwd, Some(&c.tmp().join("o2")), "2026-04-05");
        assert_eq!(
            fs::read(text(&a, "path")).unwrap(),
            fs::read(text(&b, "path")).unwrap()
        );
    }

    #[test]
    fn test_never_overwrites_an_existing_archive() {
        let c = Case::new();
        let (first, _) = c.archive();
        let before = fs::read(&first).unwrap();
        let second = export(&c, &c.cwd, Some(&c.out), "2026-04-05");
        assert!(text(&second, "path").ends_with("/anthology-memory-2026-04-05-2.tar.gz"));
        assert_eq!(fs::read(&first).unwrap(), before);
        let third = export(&c, &c.cwd, Some(&c.out), "2026-04-05");
        assert!(text(&third, "path").ends_with("/anthology-memory-2026-04-05-3.tar.gz"));
    }

    #[test]
    fn test_only_plain_notes_are_exported() {
        let c = Case::new();
        c.seed_personal();
        write(&c.personal.join("MEMORY.md"), "- generated\n", None);
        write(&c.personal.join("notes.txt"), "x", None);
        write(&c.personal.join(".hidden.md"), "x", None);
        write(&c.personal.join("has space.md"), "x", None);
        fs::write(c.personal.join("bad.md"), b"\xff\xfe not utf-8").unwrap();
        let outside = c.tmp().join("outside.md");
        write(&outside, "secret outside\n", None);
        symlink(&outside, c.personal.join("link.md")).unwrap();
        fs::create_dir(c.personal.join("sub")).unwrap();
        write(&c.personal.join("sub").join("deep.md"), "x", None);
        let res = export(&c, &c.cwd, Some(&c.out), "2026-04-05");
        assert_eq!(
            tar_names(Path::new(text(&res, "path"))),
            [
                "manifest.json",
                "personal/feedback-x.md",
                "personal/user-me.md"
            ]
        );
    }

    #[test]
    fn test_default_folder_is_under_the_anthology_home_not_the_cwd() {
        let c = Case::new();
        c.seed_personal();
        let res = portable::export(&c.cwd, None, Some("2026-04-05"), c.env(), &RealGit).unwrap();
        let exports = c.m.anth.join("exports");
        assert_eq!(
            Path::new(text(&res, "path")).parent(),
            Some(exports.as_path())
        );
        let mode = fs::metadata(&exports).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o700);
        assert!(is_empty_dir(&c.cwd));
    }

    #[test]
    fn test_explicit_folder_inside_a_git_work_tree_is_refused() {
        let c = Case::new();
        c.seed_personal();
        let work = c.tmp().join("work");
        fs::create_dir_all(work.join(".git")).unwrap(); // a marker is enough: no git needed
        for target in [work.clone(), work.join("sub").join("deeper")] {
            let before = tree(c.tmp());
            let err =
                portable::export(&c.cwd, Some(&target), Some("2026-04-05"), c.env(), &RealGit)
                    .unwrap_err();
            let Error::Refused(msg) = err else {
                panic!("not ExportRefused: {err:?}");
            };
            assert!(
                msg.contains("export refuses to write personal notes inside a git repo"),
                "{msg}"
            );
            assert_eq!(tree(c.tmp()), before);
        }
    }

    #[test]
    fn test_cli_export_refuses_a_folder_in_a_git_repo_with_exit_2() {
        let c = Case::new();
        c.seed_personal();
        let work = c.tmp().join("work");
        fs::create_dir_all(work.join(".git")).unwrap();
        let r = c.run_cli(c.env(), &["export", "--out", p(&work)]);
        assert_eq!(r.code, 2, "{}", r.stdout_text());
        assert!(
            r.stderr_text()
                .contains("export refuses to write personal notes inside a git repo"),
            "{}",
            r.stderr_text()
        );
        assert_eq!(sorted_names(&work), [".git"]);
    }

    #[test]
    fn test_exporting_from_inside_a_repo_does_not_write_into_it() {
        if !needs_git() {
            return;
        }
        let c = Case::new();
        let root = c.m.make_repo("repo", None);
        approve::approve(&root, c.env(), &RealGit).unwrap();
        write(
            &root.join(".claude/memory/project-r.md"),
            &Note::new("project-r").text(),
            None,
        );
        let before = tree(&root);
        let res = portable::export(&root, None, Some("2026-04-05"), c.env(), &RealGit).unwrap();
        assert_eq!(tree(&root), before);
        assert_eq!(
            Path::new(text(&res, "path")).parent(),
            Some(c.m.anth.join("exports").as_path())
        );
        assert_eq!(count(&res, "counts")[1].1, 1);
    }

    #[test]
    fn test_creates_the_output_folder() {
        let c = Case::new();
        c.seed_personal();
        let res = export(&c, &c.cwd, Some(&c.tmp().join("new/dir")), "2026-04-05");
        assert!(Path::new(text(&res, "path")).is_file());
    }

    #[test]
    fn test_repo_and_agent_notes_only_from_an_approved_repo() {
        if !needs_git() {
            return;
        }
        let c = Case::new();
        let root = c.m.make_repo("repo", None);
        write(
            &root.join(".claude/memory/project-r.md"),
            &Note::new("project-r").text(),
            None,
        );
        write(&root.join(".claude/memory/MEMORY.md"), "generated\n", None);
        write(
            &root.join(".claude/agent-memory/anthology-w/feedback-w.md"),
            &Note::new("feedback-w").type_("feedback").text(),
            None,
        );
        write(&root.join(".claude/agent-memory/bad dir/x.md"), "x", None);
        let res = export(&c, &root, Some(&c.out), "2026-04-05");
        assert_eq!(res.get("repo_included"), Some(&Value::Bool(false)));
        assert_eq!(count(&res, "counts"), counts(0, 0, 0));
        approve::approve(&root, c.env(), &RealGit).unwrap();
        let res = export(&c, &root, Some(&c.out), "2026-04-06");
        assert_eq!(res.get("repo_included"), Some(&Value::Bool(true)));
        assert_eq!(count(&res, "counts"), counts(0, 1, 1));
        assert_eq!(
            tar_names(Path::new(text(&res, "path"))),
            [
                "manifest.json",
                "repo/agent-memory/anthology-w/feedback-w.md",
                "repo/memory/project-r.md"
            ]
        );
    }
}

mod secrets_test {
    use super::*;

    #[test]
    fn test_each_pattern_has_a_label() {
        for (label, text) in secret_samples() {
            assert_eq!(portable::scan_secrets(&text), [label], "{label}");
        }
        // an Anthropic key is also an `sk-` key
        assert_eq!(
            portable::scan_secrets(&["sk", "-ant-", "abcdefghij0123456789"].concat()),
            ["anthropic-key", "api-key"]
        );
        assert_eq!(
            portable::SECRET_PATTERNS,
            anthology_memory::privacy::SECRET_PATTERNS
        );
        assert_eq!(
            portable::scan_secrets(&["github_pat", "_", &"A".repeat(25)].concat()),
            ["github-token"]
        );
        assert!(
            portable::scan_secrets("Just a normal note about passwords in general.").is_empty()
        );
        assert!(portable::scan_secrets("token: short").is_empty());
    }

    #[test]
    fn test_export_reports_labels_not_values() {
        let c = Case::new();
        let key = sample("aws-access-key");
        write(
            &c.personal.join("user-keys.md"),
            &Note::new("user-keys")
                .type_("user")
                .body(&format!("{key}\n"))
                .text(),
            None,
        );
        write(
            &c.personal.join("user-ok.md"),
            &Note::new("user-ok").type_("user").text(),
            None,
        );
        let res = export(&c, &c.cwd, Some(&c.out), "2026-04-05");
        assert_eq!(
            list(&res, "secrets"),
            [obj(&[
                ("file", s("personal/user-keys.md")),
                ("pattern", s("aws-access-key"))
            ])]
        );
        let dumped = json::dumps(&Value::Object(res), &json::PY_DEFAULT);
        assert!(!dumped.contains(&key["id AKIA".len()..]));
    }

    #[test]
    fn test_import_plan_reports_labels_not_values() {
        let c = Case::new();
        let token = sample("github-token");
        let path = make_archive(
            &c.tmp().join("s.tar.gz"),
            &[(
                "personal/user-keys.md",
                Spec::File(format!("x\n{token}\n").into_bytes()),
            )],
            true,
            false,
        );
        let plan = plan(&path, &c.cwd, None, c.env());
        assert_eq!(
            list(&plan, "secrets"),
            [obj(&[
                ("file", s("personal/user-keys.md")),
                ("pattern", s("github-token"))
            ])]
        );
        let dumped = json::dumps(&Value::Object(plan), &json::PY_DEFAULT);
        assert!(!dumped.contains(&token));
    }
}

mod import_test {
    use super::*;

    #[test]
    fn test_round_trip_into_an_empty_home_is_byte_identical() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let plan = plan(&path, &c.cwd, Some("2026-04-06"), &env_b);
        assert_eq!(
            members(&plan, "new"),
            ["personal/feedback-x.md", "personal/user-me.md"]
        );
        for key in ["identical", "conflicts", "skipped_repo"] {
            assert!(list(&plan, key).is_empty(), "{key}");
        }
        assert!(tree(&anth_b).is_empty()); // planning wrote nothing
        let result = apply(&c, &plan);
        assert_eq!(list(&result, "written").len(), 2);
        for name in ["user-me.md", "feedback-x.md"] {
            assert_eq!(
                fs::read(anth_b.join("personal-memory").join(name)).unwrap(),
                fs::read(c.personal.join(name)).unwrap()
            );
        }
        let index = fs::read_to_string(anth_b.join("personal-memory/MEMORY.md")).unwrap();
        assert!(index.contains("[user-me](user-me.md)"), "{index}");
    }

    #[test]
    fn test_identical_skipped_conflict_kept_side_by_side() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let mem_b = anth_b.join("personal-memory");
        let mine = fs::read_to_string(c.personal.join("user-me.md")).unwrap();
        write(&mem_b.join("user-me.md"), &mine, None); // identical
        write(
            &mem_b.join("feedback-x.md"),
            "my own different note\n",
            None,
        ); // conflict
        let plan1 = plan(&path, &c.cwd, Some("2026-04-06"), &env_b);
        assert_eq!(members(&plan1, "identical"), ["personal/user-me.md"]);
        assert_eq!(members(&plan1, "conflicts"), ["personal/feedback-x.md"]);
        assert!(list(&plan1, "new").is_empty());
        let result = apply(&c, &plan1);
        assert_eq!(
            fs::read_to_string(mem_b.join("feedback-x.md")).unwrap(),
            "my own different note\n"
        );
        let imported = mem_b.join("feedback-x.imported-2026-04-06.md");
        assert_eq!(
            fs::read(&imported).unwrap(),
            fs::read(c.personal.join("feedback-x.md")).unwrap()
        );
        assert_eq!(strs(&result, "conflicts"), [p(&imported)]);
        assert!(list(&result, "written").is_empty());
        // importing again does not pile up copies: the imported copy counts as identical
        let plan2 = plan(&path, &c.cwd, Some("2026-04-07"), &env_b);
        let sizes: Vec<usize> = ["new", "conflicts", "identical"]
            .iter()
            .map(|k| list(&plan2, k).len())
            .collect();
        assert_eq!(sizes, [0, 0, 2]);
        apply(&c, &plan2);
        assert_eq!(
            glob(&mem_b, "feedback-x", ""),
            ["feedback-x.imported-2026-04-06.md", "feedback-x.md"]
        );
    }

    #[test]
    fn test_imported_name_is_made_unique() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let mem_b = anth_b.join("personal-memory");
        write(&mem_b.join("feedback-x.md"), "mine\n", None);
        write(
            &mem_b.join("feedback-x.imported-2026-04-06.md"),
            "an older different import\n",
            None,
        );
        let plan = plan(&path, &c.cwd, Some("2026-04-06"), &env_b);
        apply(&c, &plan);
        assert_eq!(
            fs::read_to_string(mem_b.join("feedback-x.imported-2026-04-06.md")).unwrap(),
            "an older different import\n"
        );
        assert!(mem_b.join("feedback-x.imported-2026-04-06-2.md").is_file());
    }

    #[test]
    fn test_plan_and_dry_run_write_nothing() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        write(
            &anth_b.join("personal-memory/feedback-x.md"),
            "mine\n",
            None,
        );
        let before = tree(c.tmp());
        plan(&path, &c.cwd, None, &env_b);
        assert_eq!(tree(c.tmp()), before);
        let r = c.run_cli(&env_b, &["import", p(&path), "--dry-run"]);
        assert_eq!(r.code, 0, "{}", r.stderr_text());
        assert_eq!(tree(c.tmp()), before);
        let r = c.run_cli(&env_b, &["import", p(&path)]); // no --yes: also only the plan
        assert_eq!(r.code, 0, "{}", r.stderr_text());
        assert!(r.stdout_text().contains("Nothing written"));
        assert_eq!(tree(c.tmp()), before);
        c.run_cli(&env_b, &["import", p(&path), "--yes", "--dry-run"]); // dry-run wins
        assert_eq!(tree(c.tmp()), before);
    }

    #[test]
    fn test_changed_after_planning_becomes_a_conflict_not_an_overwrite() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let plan = plan(&path, &c.cwd, Some("2026-04-06"), &env_b);
        let mem_b = anth_b.join("personal-memory");
        write(&mem_b.join("user-me.md"), "appeared meanwhile\n", None);
        let result = apply(&c, &plan);
        assert_eq!(
            fs::read_to_string(mem_b.join("user-me.md")).unwrap(),
            "appeared meanwhile\n"
        );
        assert!(mem_b.join("user-me.imported-2026-04-06.md").is_file());
        assert_eq!(list(&result, "conflicts").len(), 1);
    }

    #[test]
    fn test_archive_swapped_after_planning_is_refused() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let plan = plan(&path, &c.cwd, None, &env_b);
        // same member names, different content: only the checksum can tell
        make_archive(
            &path,
            &[
                ("personal/feedback-x.md", Spec::File(b"swapped\n".to_vec())),
                ("personal/user-me.md", Spec::File(b"swapped\n".to_vec())),
            ],
            true,
            false,
        );
        match try_apply(&c, &plan, &RealGit) {
            Err(Error::Rejected(m)) => assert!(m.contains("changed after"), "{m}"),
            other => panic!("not rejected: {other:?}"),
        }
        assert!(tree(&anth_b).is_empty());
    }

    #[test]
    fn test_cli_yes_can_be_bound_to_the_dry_run_hash() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let dry = c.run_cli(&env_b, &["import", p(&path), "--dry-run"]);
        let sha = {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(fs::read(&path).unwrap());
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert!(dry.stdout_text().contains(&sha), "{}", dry.stdout_text());
        let before = tree(c.tmp());
        let zeros = "0".repeat(64);
        let r = c.run_cli(&env_b, &["import", p(&path), "--yes", "--sha256", &zeros]);
        assert_eq!(r.code, 2, "{}", r.stdout_text());
        assert_eq!(tree(c.tmp()), before);
        let r = c.run_cli(&env_b, &["import", p(&path), "--yes", "--sha256", &sha]);
        assert_eq!(r.code, 0, "{}", r.stderr_text());
        assert!(anth_b.join("personal-memory/user-me.md").exists());
    }

    /// Lead extra (MX1 review I-1): a `--sha256` that is not UTF-8 never matches, as in
    /// Python, where the surrogateescaped value differs from every hex digest.
    #[test]
    fn a_sha256_that_is_not_utf8_is_refused_and_nothing_is_written() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let before = tree(c.tmp());
        let args = [
            OsString::from("import"),
            path.clone().into_os_string(),
            OsString::from("--yes"),
            OsString::from("--sha256"),
            OsString::from_vec(vec![0xff]),
        ];
        let out = portable::main(&args, &c.cwd, &env_b, &RealGit);
        assert_eq!(out.code, 2, "{}{}", out.stdout, out.stderr);
        assert_eq!(
            out.stderr,
            "memory_portable: rejected: the archive is not the one that was shown (sha256 differs)\n"
        );
        assert_eq!(tree(c.tmp()), before);
        assert!(!anth_b.join("personal-memory").exists());
    }

    #[test]
    fn test_forged_plan_entries_are_refused() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, _anth_b) = c.second_machine("b");
        let before = tree(c.tmp());
        let first = |plan: &mut Map, key: &str, v: Value| {
            let new = plan.get_mut("new").and_then(Value::as_array_mut).unwrap();
            new[0].as_object_mut().unwrap().insert(key.into(), v);
        };
        let tmp = c.tmp().to_path_buf();
        let mutations: Vec<Mutation> = vec![
            Box::new(|p| first(p, "name", s("../escape.md"))),
            Box::new(|p| first(p, "name", s("MEMORY.md"))),
            Box::new(|p| first(p, "scope", s("elsewhere"))),
            Box::new(|p| {
                first(p, "member", s("personal/never-in-archive.md"));
                first(p, "name", s("never-in-archive.md"));
            }),
            Box::new(|p| {
                p.insert(
                    "home".into(),
                    s(&tmp.join("not-anthology").to_string_lossy()),
                )
            }),
            Box::new(|p| {
                p.insert(
                    "archive".into(),
                    s(&tmp.join("missing.tar.gz").to_string_lossy()),
                )
            }),
            Box::new(|p| {
                p.insert("repo_root".into(), s(&tmp.to_string_lossy()));
                let entry = obj(&[
                    ("member", s("repo/memory/a.md")),
                    ("scope", s("repo")),
                    ("agent", Value::Null),
                    ("name", s("a.md")),
                ]);
                p.insert("new".into(), Value::Array(vec![entry]));
            }),
        ];
        for mutate in &mutations {
            let mut plan = plan(&path, &c.cwd, None, &env_b);
            mutate(&mut plan);
            assert!(
                matches!(try_apply(&c, &plan, &RealGit), Err(Error::Rejected(_))),
                "not rejected"
            );
        }
        assert_eq!(tree(c.tmp()), before);
    }

    /// Lead extra (MX1 review M-8, D-8 Rust stricter): a date that would put `/`, a NUL or
    /// `..` into an `.imported-<date>` name is refused, by plan and by apply, before any write.
    /// Python builds the name from it, so a forged `date` could steer the copy elsewhere.
    #[test]
    fn a_date_that_could_leave_the_folder_is_refused() {
        let c = Case::new();
        let (path, _) = c.archive();
        let (env_b, anth_b) = c.second_machine("b");
        let before = tree(c.tmp());
        let refused = Err(Error::Rejected("malformed plan date".into()));
        for bad in ["../../x", "2026/01/01", "2026-01\u{0}01", "..", "a..b"] {
            let mut forged = plan(&path, &c.cwd, None, &env_b);
            forged.insert("date".into(), s(bad));
            assert_eq!(
                try_apply(&c, &forged, &RealGit).map(|_| ()),
                refused,
                "{bad:?}"
            );
            let planned = portable::plan_import(&path, &c.cwd, Some(bad), &env_b, &RealGit);
            assert_eq!(planned.map(|_| ()), refused, "{bad:?}");
        }
        assert_eq!(tree(c.tmp()), before);
        assert!(!anth_b.join("personal-memory").exists());
        // A plain date still works.
        let mut ok = plan(&path, &c.cwd, None, &env_b);
        ok.insert("date".into(), s("2026-04-06"));
        assert!(try_apply(&c, &ok, &RealGit).is_ok());
    }

    #[test]
    fn test_repo_parts_are_skipped_outside_an_approved_repo() {
        let c = Case::new();
        let path = make_archive(
            &c.tmp().join("m.tar.gz"),
            &[
                ("personal/user-p.md", Spec::File(b"personal\n".to_vec())),
                ("repo/memory/project-r.md", Spec::File(b"repo\n".to_vec())),
                (
                    "repo/agent-memory/anthology-w/feedback-w.md",
                    Spec::File(b"agent\n".to_vec()),
                ),
            ],
            true,
            false,
        );
        let (env_b, anth_b) = c.second_machine("b");
        let plan = plan(&path, &c.cwd, None, &env_b);
        let mut skipped = strs(&plan, "skipped_repo");
        skipped.sort();
        assert_eq!(
            skipped,
            [
                "repo/agent-memory/anthology-w/feedback-w.md",
                "repo/memory/project-r.md"
            ]
        );
        assert!(!list(&plan, "messages").is_empty());
        apply(&c, &plan);
        assert_eq!(
            sorted_names(&anth_b.join("personal-memory")),
            ["MEMORY.md", "user-p.md"]
        );
        assert!(is_empty_dir(&c.cwd));
    }

    #[test]
    fn test_folder_members_are_allowed() {
        let c = Case::new();
        let path = make_archive(
            &c.tmp().join("d.tar.gz"),
            &[
                ("personal", Spec::Dir),
                ("personal/a.md", Spec::File(b"a\n".to_vec())),
            ],
            true,
            false,
        );
        let (env_b, anth_b) = c.second_machine("b");
        let plan = plan(&path, &c.cwd, None, &env_b);
        apply(&c, &plan);
        assert_eq!(
            fs::read(anth_b.join("personal-memory/a.md")).unwrap(),
            b"a\n"
        );
    }
}

/// `DestinationTest` (`@needs_git`): an approved repo and a three-scope archive.
struct Dest {
    c: Case,
    repo: PathBuf,
    archive: PathBuf,
    outside: PathBuf,
}

impl Dest {
    fn new() -> Option<Dest> {
        if !needs_git() {
            return None;
        }
        let c = Case::new();
        let repo = c.m.make_repo("repo", None);
        approve::approve(&repo, c.env(), &RealGit).unwrap();
        let archive = make_archive(
            &c.tmp().join("m.tar.gz"),
            &[
                ("personal/user-p.md", Spec::File(b"personal\n".to_vec())),
                ("repo/memory/project-r.md", Spec::File(b"repo\n".to_vec())),
                (
                    "repo/agent-memory/anthology-w/feedback-w.md",
                    Spec::File(b"agent\n".to_vec()),
                ),
            ],
            true,
            false,
        );
        let outside = c.tmp().join("outside");
        fs::create_dir(&outside).unwrap();
        Some(Dest {
            c,
            repo,
            archive,
            outside,
        })
    }

    fn plan(&self) -> Map {
        plan(&self.archive, &self.repo, Some("2026-04-06"), self.c.env())
    }
}

mod destination_test {
    use super::*;

    #[test]
    fn test_round_trip_into_an_approved_repo() {
        let Some(d) = Dest::new() else { return };
        let plan = d.plan();
        assert_eq!(list(&plan, "new").len(), 3);
        assert!(list(&plan, "skipped_repo").is_empty());
        let result = apply(&d.c, &plan);
        assert_eq!(list(&result, "written").len(), 3);
        let mem = d.repo.join(".claude/memory");
        assert_eq!(fs::read(mem.join("project-r.md")).unwrap(), b"repo\n");
        let agent = d.repo.join(".claude/agent-memory/anthology-w");
        assert_eq!(fs::read(agent.join("feedback-w.md")).unwrap(), b"agent\n");
        assert_eq!(
            fs::read(d.c.personal.join("user-p.md")).unwrap(),
            b"personal\n"
        );
        for dir in [&mem, &agent, &d.c.personal] {
            assert!(dir.join("MEMORY.md").is_file(), "{}", dir.display());
        }
        let index = fs::read_to_string(mem.join("MEMORY.md")).unwrap();
        assert!(index.contains("[project-r](project-r.md)"), "{index}");
    }

    #[test]
    fn test_full_export_import_round_trip_between_two_repos() {
        let Some(d) = Dest::new() else { return };
        let c = &d.c;
        write(
            &d.repo.join(".claude/memory/project-r.md"),
            &note("project-r", "project", "R", "2026-02-02"),
            None,
        );
        write(
            &d.repo
                .join(".claude/agent-memory/anthology-w/feedback-w.md"),
            &Note::new("feedback-w").type_("feedback").desc("W").text(),
            None,
        );
        write(
            &c.personal.join("user-p.md"),
            &Note::new("user-p").type_("user").desc("P").text(),
            None,
        );
        let exported = export(c, &d.repo, Some(&c.out), "2026-04-05");
        let exported = PathBuf::from(text(&exported, "path"));
        let (env_b, anth_b) = c.second_machine("b");
        let repo_b = c.m.make_repo("repo-b", None);
        approve::approve(&repo_b, &env_b, &RealGit).unwrap();
        let plan = plan(&exported, &repo_b, None, &env_b);
        apply(c, &plan);
        let pairs = [
            (
                d.repo.join(".claude/memory/project-r.md"),
                repo_b.join(".claude/memory/project-r.md"),
            ),
            (
                d.repo
                    .join(".claude/agent-memory/anthology-w/feedback-w.md"),
                repo_b.join(".claude/agent-memory/anthology-w/feedback-w.md"),
            ),
            (
                c.personal.join("user-p.md"),
                anth_b.join("personal-memory/user-p.md"),
            ),
        ];
        for (a, b) in pairs {
            assert_eq!(
                fs::read(&a).unwrap(),
                fs::read(&b).unwrap(),
                "{}",
                b.display()
            );
        }
    }

    #[test]
    fn test_symlinked_destinations_are_refused_and_nothing_is_written() {
        let Some(d) = Dest::new() else { return };
        let dot_claude = d.repo.join(".claude");
        let cases: Vec<(&str, Setup)> = vec![
            (
                "memory",
                Box::new(|| {
                    fs::create_dir(&dot_claude).unwrap();
                    symlink(&d.outside, dot_claude.join("memory")).unwrap();
                }),
            ),
            (
                "dot-claude",
                Box::new(|| symlink(&d.outside, &dot_claude).unwrap()),
            ),
            (
                "agent-memory",
                Box::new(|| {
                    fs::create_dir(&dot_claude).unwrap();
                    symlink(&d.outside, dot_claude.join("agent-memory")).unwrap();
                }),
            ),
            (
                "agent-folder",
                Box::new(|| {
                    fs::create_dir_all(dot_claude.join("agent-memory")).unwrap();
                    symlink(&d.outside, dot_claude.join("agent-memory/anthology-w")).unwrap();
                }),
            ),
            (
                "personal",
                Box::new(|| {
                    fs::create_dir_all(&d.c.m.anth).unwrap();
                    symlink(&d.outside, &d.c.personal).unwrap();
                }),
            ),
        ];
        for (name, setup) in cases {
            setup();
            let before = tree(d.c.tmp());
            let plan = d.plan();
            assert!(
                matches!(try_apply(&d.c, &plan, &RealGit), Err(Error::Rejected(_))),
                "{name}"
            );
            assert_eq!(tree(d.c.tmp()), before, "{name}");
            assert!(is_empty_dir(&d.outside), "{name}");
            // undo for the next case
            if fs::symlink_metadata(&d.c.personal).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_file(&d.c.personal).unwrap();
            }
            if fs::symlink_metadata(&dot_claude).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_file(&dot_claude).unwrap();
            }
            let _ = fs::remove_dir_all(&dot_claude);
        }
    }

    #[test]
    fn test_destination_note_that_is_a_symlink_is_never_followed() {
        let Some(d) = Dest::new() else { return };
        let target = d.c.tmp().join("target.md");
        write(&target, "precious\n", None);
        let mem = d.repo.join(".claude/memory");
        fs::create_dir_all(&mem).unwrap();
        symlink(&target, mem.join("project-r.md")).unwrap();
        let plan = d.plan();
        assert_eq!(members(&plan, "conflicts"), ["repo/memory/project-r.md"]);
        apply(&d.c, &plan);
        assert_eq!(fs::read_to_string(&target).unwrap(), "precious\n");
        assert!(mem.join("project-r.imported-2026-04-06.md").is_file());
    }

    #[test]
    fn test_repo_must_still_be_approved_at_apply_time() {
        let Some(d) = Dest::new() else { return };
        let plan = d.plan();
        let key = repo::repo_key(&d.repo, d.c.env(), &RealGit);
        approve::unapprove(&key, d.c.env()).unwrap();
        let before = tree(d.c.tmp());
        assert!(matches!(
            try_apply(&d.c, &plan, &RealGit),
            Err(Error::Rejected(_))
        ));
        assert_eq!(tree(d.c.tmp()), before);
    }

    #[test]
    fn test_unapproved_repo_gets_personal_notes_only() {
        let Some(d) = Dest::new() else { return };
        let other = d.c.m.make_repo("other", None);
        let plan = plan(&d.archive, &other, None, d.c.env());
        assert_eq!(list(&plan, "skipped_repo").len(), 2);
        apply(&d.c, &plan);
        assert!(!other.join(".claude").exists());
        assert!(d.c.personal.join("user-p.md").exists());
    }
}

/// `ImportPrivacyTest`: a fake approved repo and an archive with one clean repo note, one with
/// an email address and one agent note with a private IP address.
struct Privacy {
    c: Case,
    repo: PathBuf,
    repo_mem: PathBuf,
    archive: PathBuf,
    git: FakeGit,
}

/// `MAIL`.
fn mail() -> String {
    ["alice", "@", "mail.acme.zz"].concat()
}

impl Privacy {
    fn new() -> Privacy {
        let c = Case::new();
        let repo = c.m.fake_repo("repo");
        let git = FakeGit { root: repo.clone() };
        approve::approve(&repo, c.env(), &git).unwrap();
        let repo_mem = repo.join(".claude/memory");
        let archive = make_archive(
            &c.tmp().join("p.tar.gz"),
            &[
                (
                    "repo/memory/project-clean.md",
                    Spec::File(b"Use Docker.\n".to_vec()),
                ),
                (
                    "repo/memory/reference-contact.md",
                    Spec::File(format!("Ask {}\n", mail()).into_bytes()),
                ),
                (
                    "repo/agent-memory/anthology-w/reference-ip.md",
                    Spec::File(b"DB at 10.1.2.3\n".to_vec()),
                ),
            ],
            true,
            false,
        );
        Privacy {
            c,
            repo,
            repo_mem,
            archive,
            git,
        }
    }

    fn plan(&self) -> Map {
        self.plan_of(&self.archive, Some("2026-04-06"))
    }

    fn plan_of(&self, archive: &Path, today: Option<&str>) -> Map {
        plan_with(archive, &self.repo, today, self.c.env(), &self.git)
    }

    fn apply(&self, plan: &Map) -> Map {
        try_apply(&self.c, plan, &self.git).expect("apply_import")
    }

    /// The archive of `test_user_notes_*`.
    fn user_archive(&self, members: &[(&str, String)]) -> PathBuf {
        let specs: Vec<(&str, Spec)> = members
            .iter()
            .map(|(n, t)| (*n, Spec::File(t.clone().into_bytes())))
            .collect();
        make_archive(&self.c.tmp().join("u.tar.gz"), &specs, true, false)
    }
}

/// `plan["new"]` with the redirect dropped, and `plan["blocked"]` emptied: a forged plan.
fn forge(plan: &mut Map) {
    plan.insert("blocked".into(), Value::Array(Vec::new()));
    for entry in plan
        .get_mut("new")
        .and_then(Value::as_array_mut)
        .unwrap()
        .iter_mut()
    {
        entry.as_object_mut().unwrap().remove("redirected");
    }
}

mod import_privacy_test {
    use super::*;

    #[test]
    fn test_a_note_with_an_email_is_redirected_and_a_clean_one_goes_to_the_repo() {
        let t = Privacy::new();
        let plan = t.plan();
        let finding = |kind: &str| {
            Value::Array(vec![obj(&[
                ("kind", s(kind)),
                ("line", Value::Int(json::Int::Small(1))),
            ])])
        };
        assert_eq!(
            list(&plan, "blocked"),
            [
                obj(&[
                    ("file", s("repo/agent-memory/anthology-w/reference-ip.md")),
                    ("findings", finding("private-ip")),
                    (
                        "reason",
                        s("the privacy scan found private IP address (line 1)")
                    ),
                ]),
                obj(&[
                    ("file", s("repo/memory/reference-contact.md")),
                    ("findings", finding("email")),
                    ("reason", s("the privacy scan found email address (line 1)")),
                ]),
            ]
        );
        let dumped = json::dumps(&Value::Object(plan.clone()), &json::PY_DEFAULT);
        assert!(!dumped.contains(&mail()));
        let dest = |member: &str| {
            list(&plan, "new")
                .iter()
                .map(object)
                .find(|e| text(e, "member") == member)
                .map(|e| text(e, "dest").to_string())
                .expect("planned")
        };
        assert_eq!(
            dest("repo/memory/reference-contact.md"),
            p(&t.c.personal.join("reference-contact.md"))
        );
        assert_eq!(
            dest("repo/memory/project-clean.md"),
            p(&t.repo_mem.join("project-clean.md"))
        );
        let result = t.apply(&plan);
        assert_eq!(
            glob(&t.repo_mem, "", ".md"),
            ["MEMORY.md", "project-clean.md"]
        );
        assert!(!t.repo.join(".claude/agent-memory").exists());
        assert_eq!(
            fs::read_to_string(t.c.personal.join("reference-contact.md")).unwrap(),
            format!("Ask {}\n", mail())
        );
        assert!(t.c.personal.join("reference-ip.md").is_file());
        let mut files: Vec<String> = list(&result, "blocked")
            .iter()
            .map(|b| text(object(b), "file").to_string())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "repo/agent-memory/anthology-w/reference-ip.md",
                "repo/memory/reference-contact.md"
            ]
        );
    }

    #[test]
    fn test_apply_rescans_instead_of_trusting_the_plan() {
        let t = Privacy::new();
        let mut plan = t.plan();
        forge(&mut plan);
        t.apply(&plan);
        assert!(!t.repo_mem.join("reference-contact.md").exists());
        assert!(t.c.personal.join("reference-contact.md").is_file());
    }

    #[test]
    fn test_redirected_conflicts_get_the_imported_name_in_personal_memory() {
        let t = Privacy::new();
        write(&t.c.personal.join("reference-contact.md"), "mine\n", None);
        let plan = t.plan();
        assert_eq!(
            members(&plan, "conflicts"),
            ["repo/memory/reference-contact.md"]
        );
        t.apply(&plan);
        assert_eq!(
            fs::read_to_string(t.c.personal.join("reference-contact.md")).unwrap(),
            "mine\n"
        );
        assert!(
            t.c.personal
                .join("reference-contact.imported-2026-04-06.md")
                .is_file()
        );
        assert!(!t.repo_mem.join("reference-contact.md").exists());
    }

    #[test]
    fn test_identical_repo_notes_are_skipped_not_blocked() {
        let t = Privacy::new();
        write(
            &t.repo_mem.join("reference-contact.md"),
            &format!("Ask {}\n", mail()),
            None,
        );
        let plan = t.plan();
        assert_eq!(
            members(&plan, "identical"),
            ["repo/memory/reference-contact.md"]
        );
        let blocked: Vec<&str> = list(&plan, "blocked")
            .iter()
            .map(|b| text(object(b), "file"))
            .collect();
        assert!(!blocked.contains(&"repo/memory/reference-contact.md"));
    }

    #[test]
    fn test_user_notes_are_never_written_into_the_repo() {
        let t = Privacy::new();
        let path = t.user_archive(&[
            (
                "repo/memory/user-x.md",
                Note::new("user-x").type_("user").desc("Me").text(),
            ),
            (
                "repo/agent-memory/anthology-w/user-z.md",
                Note::new("user-z")
                    .type_("User # personal")
                    .desc("Z")
                    .text(),
            ),
            (
                "repo/memory/project-y.md",
                Note::new("project-y").type_("project").desc("Y").text(),
            ),
        ]);
        let plan = t.plan_of(&path, None);
        let mut blocked: Vec<(String, String)> = list(&plan, "blocked")
            .iter()
            .map(|b| {
                let b = object(b);
                (text(b, "file").to_string(), text(b, "reason").to_string())
            })
            .collect();
        blocked.sort();
        let reason = portable::notes::USER_REASON.to_string();
        assert_eq!(
            blocked,
            [
                (
                    "repo/agent-memory/anthology-w/user-z.md".to_string(),
                    reason.clone()
                ),
                ("repo/memory/user-x.md".to_string(), reason),
            ]
        );
        let result = t.apply(&plan);
        assert_eq!(list(&result, "blocked").len(), 2);
        assert_eq!(glob(&t.repo_mem, "", ".md"), ["MEMORY.md", "project-y.md"]);
        assert!(!t.repo.join(".claude/agent-memory").exists());
        assert!(t.c.personal.join("user-x.md").is_file());
        assert!(t.c.personal.join("user-z.md").is_file());
        let index = fs::read_to_string(t.repo_mem.join("MEMORY.md")).unwrap();
        assert!(index.contains("[project-y]"), "{index}");
        assert!(!index.contains("user-x"), "{index}");
    }

    #[test]
    fn test_apply_redirects_a_user_note_the_plan_let_through() {
        let t = Privacy::new();
        let path = t.user_archive(&[(
            "repo/memory/user-x.md",
            Note::new("user-x").type_("user").desc("Me").text(),
        )]);
        let mut plan = t.plan_of(&path, None);
        forge(&mut plan);
        t.apply(&plan);
        assert!(!t.repo_mem.join("user-x.md").exists());
        assert!(t.c.personal.join("user-x.md").is_file());
    }

    #[test]
    fn test_dry_run_output_names_the_blocked_notes_without_their_content() {
        let t = Privacy::new();
        let out = portable::print_plan(&t.plan());
        assert!(out.contains("blocked from the repo (written to personal memory instead)"));
        assert!(out.contains(
            "repo/memory/reference-contact.md: the privacy scan found email address (line 1)"
        ));
        assert!(out.contains("(to personal memory: blocked)"));
        assert!(!out.contains(&mail()));
    }
}

mod cli_test {
    use super::*;

    #[test]
    fn test_export_then_import_through_the_cli() {
        let c = Case::new();
        c.seed_personal();
        let token = sample("slack-token");
        write(
            &c.personal.join("user-keys.md"),
            &Note::new("user-keys")
                .type_("user")
                .body(&format!("{token}\n"))
                .text(),
            None,
        );
        let r = c.run_cli(c.env(), &["export", "--out", p(&c.out)]);
        assert_eq!(r.code, 0, "{}", r.stderr_text());
        let stdout = r.stdout_text();
        assert!(stdout.contains("personal: 3"), "{stdout}");
        assert!(stdout.contains("slack-token"), "{stdout}");
        assert!(!(stdout.clone() + &r.stderr_text()).contains(&token));
        assert!(stdout.contains("personal notes"), "{stdout}");
        let archive = c.out.join(
            glob(&c.out, "anthology-memory-", ".tar.gz")
                .first()
                .expect("an archive"),
        );
        // default out: <anthology home>/exports, never the cwd
        let r = c.run_cli(c.env(), &["export", "--json"]);
        let data = json::loads(&r.stdout_text()).expect("json");
        let data = object(&data);
        assert_eq!(
            Path::new(text(data, "path")).parent(),
            Some(c.m.anth.join("exports").as_path())
        );
        assert!(is_empty_dir(&c.cwd));
        assert_eq!(count(data, "counts")[0].1, 3);
        let (env_b, anth_b) = c.second_machine("b");
        let r = c.run_cli(&env_b, &["import", p(&archive), "--json"]);
        let both = json::loads(&r.stdout_text()).expect("json");
        let plan = object(object(&both).get("plan").expect("plan"));
        assert_eq!(list(plan, "new").len(), 3);
        assert!(!r.stdout_text().contains(&token));
        assert!(!anth_b.join("personal-memory").exists());
        let r = c.run_cli(&env_b, &["import", p(&archive), "--yes"]);
        assert_eq!(r.code, 0, "{}", r.stderr_text());
        assert_eq!(
            fs::read(anth_b.join("personal-memory/user-me.md")).unwrap(),
            fs::read(c.personal.join("user-me.md")).unwrap()
        );
    }

    #[test]
    fn test_missing_file_is_an_error_not_a_traceback() {
        let c = Case::new();
        let r = c.run_cli(c.env(), &["import", p(&c.tmp().join("nope.tar.gz"))]);
        assert_eq!(r.code, 2);
        assert!(!r.stderr_text().contains("Traceback"));
    }
}
