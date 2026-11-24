//! `anthology-mem save --repo` and `anthology-mem reindex`: repo-level notes written through the
//! same checks as an agent note, and the index rebuilt from the notes alone. The tests run the
//! real binary in a throw-away git repository (run `cargo build -p anthology-cli` first when this
//! crate is tested alone).

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_memory::index::notes;
use anthology_testkit::mem::{MemCase, needs_git};
use anthology_testkit::run::Output;

const REPO_NOTE: &str = ".claude/memory/project-eval-probe.md";
const REPO_INDEX: &str = ".claude/memory/MEMORY.md";
const AGENT_INDEX: &str = ".claude/agent-memory/anthology-scout/MEMORY.md";
const CONFLICT: &str = "<<<<<<< HEAD\n- [ours](a.md)\n=======\n- [theirs](b.md)\n>>>>>>> other\n";

fn run_mem(env: &Env, cwd: &Path, args: &[&str]) -> Output {
    let mut argv = vec!["mem"];
    argv.extend_from_slice(args);
    anthology_testkit::run_bin(&argv, b"", env, cwd)
}

struct Case {
    case: MemCase,
    repo: PathBuf,
}

impl Case {
    fn new() -> Case {
        let case = MemCase::new();
        let repo = case.make_repo("repo", None);
        Case { case, repo }
    }

    fn run(&self, args: &[&str]) -> Output {
        run_mem(&self.case.env, &self.repo, args)
    }

    /// `save --repo` with the default text, `kw` replacing a flag's value.
    fn save_repo(&self, kw: &[(&str, &str)]) -> Output {
        let mut flags = vec![
            ("type", "project"),
            ("slug", "eval-probe"),
            ("name", "Eval probe"),
            ("description", "How the probe works"),
            ("body", "Body."),
        ];
        for (k, v) in kw {
            match flags.iter_mut().find(|f| f.0 == *k) {
                Some(f) => f.1 = v,
                None => flags.push((k, v)),
            }
        }
        let mut argv: Vec<String> = vec!["save".into(), "--repo".into()];
        for (k, v) in flags {
            argv.push(format!("--{k}"));
            argv.push(v.to_string());
        }
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        self.run(&argv)
    }

    fn save_agent(&self) -> Output {
        self.run(&[
            "save",
            "--agent",
            "scout",
            "--type",
            "feedback",
            "--slug",
            "terse",
            "--name",
            "Terse",
            "--description",
            "Keep it short",
            "--body",
            "Short.",
        ])
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.repo.join(rel)).expect("read")
    }

    fn overwrite(&self, rel: &str, text: &str) {
        fs::write(self.repo.join(rel), text).expect("write");
    }
}

fn ok(out: Output) -> Output {
    assert_eq!(out.code, 0, "{}", out.stderr_text());
    out
}

#[test]
fn save_repo_writes_note_and_index() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    let p = c.save_repo(&[]);
    assert_eq!(
        (p.code, p.stdout_text(), p.stderr_text()),
        (
            0,
            format!("created {REPO_NOTE} in {}\n", c.repo.display()),
            String::new()
        )
    );
    let n = notes::parse_note(&c.repo.join(REPO_NOTE)).expect("note");
    assert_eq!(
        (n.name.as_str(), n.kind.as_deref(), n.description.as_str()),
        ("Eval probe", Some("project"), "How the probe works")
    );
    assert!(
        c.read(REPO_INDEX)
            .contains("- [Eval probe](project-eval-probe.md) — How the probe works")
    );
    assert!(!c.repo.join(".claude/agent-memory").exists());
    // The same name again needs --replace, as for an agent note.
    let again = c.save_repo(&[]);
    assert_eq!(again.code, 1);
    assert!(again.stderr_text().contains("--replace"));
}

#[test]
fn save_repo_refuses_a_user_note() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    let p = c.save_repo(&[("type", "user")]);
    assert_eq!(p.code, 1);
    assert!(p.stderr_text().contains("personal"), "{}", p.stderr_text());
    assert!(!c.repo.join(".claude").exists());
}

#[test]
fn save_repo_refuses_a_home_path() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    let p = c.save_repo(&[("body", "Kept in /Users/someone/work/notes.")]);
    assert_eq!(p.code, 1);
    assert!(p.stderr_text().contains("home path"), "{}", p.stderr_text());
    assert!(!p.stderr_text().contains("someone"));
    assert!(!c.repo.join(".claude").exists());
}

#[test]
fn save_repo_refuses_a_symlinked_dir() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    let elsewhere = c.case.tmp.join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    fs::create_dir(c.repo.join(".claude")).expect("mkdir");
    symlink(&elsewhere, c.repo.join(".claude/memory")).expect("symlink");
    let p = c.save_repo(&[]);
    assert_eq!(p.code, 1);
    assert!(p.stderr_text().contains("symlink"), "{}", p.stderr_text());
    assert_eq!(fs::read_dir(&elsewhere).expect("ls").count(), 0);
}

#[test]
fn reindex_rebuilds_a_conflicted_index() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    ok(c.save_repo(&[]));
    let good = c.read(REPO_INDEX);
    c.overwrite(REPO_INDEX, CONFLICT);
    let p = ok(c.run(&["reindex", "--repo"]));
    assert_eq!(p.stdout_text(), format!("reindexed {REPO_INDEX}\n"));
    assert_eq!(c.read(REPO_INDEX), good);
    // Nothing left to fix the second time.
    let p = ok(c.run(&["reindex", "--repo"]));
    assert_eq!(p.stdout_text(), format!("unchanged {REPO_INDEX}\n"));
}

#[test]
fn reindex_agent_rebuilds_only_that_agent() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    ok(c.save_repo(&[]));
    ok(c.save_agent());
    let good = c.read(AGENT_INDEX);
    c.overwrite(AGENT_INDEX, CONFLICT);
    c.overwrite(REPO_INDEX, CONFLICT);
    ok(c.run(&["reindex", "--agent", "scout"]));
    assert_eq!(c.read(AGENT_INDEX), good);
    assert_eq!(c.read(REPO_INDEX), CONFLICT);
    let p = c.run(&["reindex", "--agent", "nobody"]);
    assert_eq!(p.code, 1);
    assert!(p.stderr_text().contains("unknown agent"));
}

#[test]
fn reindex_all_covers_agents_and_repo() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    ok(c.save_repo(&[]));
    ok(c.save_agent());
    let (repo_good, agent_good) = (c.read(REPO_INDEX), c.read(AGENT_INDEX));
    c.overwrite(REPO_INDEX, CONFLICT);
    c.overwrite(AGENT_INDEX, CONFLICT);
    let p = ok(c.run(&["reindex", "--all"]));
    assert_eq!(
        p.stdout_text(),
        format!("reindexed {REPO_INDEX}\nreindexed {AGENT_INDEX}\n")
    );
    assert_eq!(c.read(REPO_INDEX), repo_good);
    assert_eq!(c.read(AGENT_INDEX), agent_good);
}

#[test]
fn reindex_refuses_a_symlinked_dir() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    let elsewhere = c.case.tmp.join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    fs::write(
        elsewhere.join("project-x.md"),
        "---\nname: x\ndescription: d\nmetadata:\n  type: project\n---\n\nb\n",
    )
    .expect("write");
    fs::create_dir(c.repo.join(".claude")).expect("mkdir");
    symlink(&elsewhere, c.repo.join(".claude/memory")).expect("symlink");
    let p = c.run(&["reindex", "--repo"]);
    assert_eq!(p.code, 1);
    assert!(p.stderr_text().contains("symlink"), "{}", p.stderr_text());
    assert!(!elsewhere.join("MEMORY.md").exists());
}

#[test]
fn agent_and_repo_are_exclusive() {
    if !needs_git() {
        return;
    }
    let c = Case::new();
    let p = c.run(&[
        "save",
        "--agent",
        "scout",
        "--repo",
        "--type",
        "project",
        "--slug",
        "x",
        "--name",
        "n",
        "--description",
        "d",
        "--body",
        "b",
    ]);
    assert_eq!(p.code, 2);
    assert!(
        p.stderr_text().contains("not allowed with"),
        "{}",
        p.stderr_text()
    );
    // One of them is needed.
    let p = c.run(&[
        "save",
        "--type",
        "project",
        "--slug",
        "x",
        "--name",
        "n",
        "--description",
        "d",
        "--body",
        "b",
    ]);
    assert_eq!(p.code, 2);
    for args in [
        &["reindex", "--agent", "scout", "--repo"][..],
        &["reindex", "--all", "--repo"],
        &["reindex"],
    ] {
        assert_eq!(c.run(args).code, 2, "{args:?}");
    }
    assert!(!c.repo.join(".claude").exists());
}
