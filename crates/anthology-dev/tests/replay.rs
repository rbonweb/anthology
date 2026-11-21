//! `anthology-dev replay` end to end on a synthetic corpus (plan RP1: "CI runs the whole tool on
//! a synthetic corpus; the real run is local only").
//!
//! The corpus is a made-up `~/.claude/projects` tree in a temporary `HOME`, with a subagent
//! transcript. `collect`, `run` (all four runners and all three surfaces) and `report` run on
//! it; the report must pass the corpus-string test. Runner 1 and the Python surfaces need a
//! `python3` (3.9+) on `PATH`: the macOS runner has one, the Linux Rust image does not, so there
//! the test replays runners 2-4 only and says so.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_dev::replay::{self, collect, report};

fn repo() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).expect("repo root")
}

fn args(a: &[&str]) -> Vec<OsString> {
    a.iter().map(OsString::from).collect()
}

fn have_python() -> bool {
    Command::new("python3")
        .args([
            "-c",
            "import sys; sys.exit(0 if sys.version_info >= (3, 9) else 1)",
        ])
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Corpus {
    _root: tempfile::TempDir,
    home: PathBuf,
    env: Env,
}

/// One transcript line: an assistant message with `blocks`.
fn assistant(session: &str, cwd: &str, blocks: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","cwd":"{cwd}","message":{{"role":"assistant","content":[{blocks}]}}}}"#
    )
}

fn tool_use(id: &str, name: &str, input: &str) -> String {
    format!(r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}"#)
}

/// A temporary `HOME` with a synthetic transcripts tree (module docs).
fn corpus() -> Corpus {
    let root = tempfile::tempdir().expect("tempdir");
    let home = fs::canonicalize(root.path()).expect("real").join("home");
    let h = home.to_str().expect("utf8").to_string();
    let project = home.join(".claude/projects/-work-synthetic-project");
    let session = "5e55a1d0-0000-4000-8000-000000000001";
    fs::create_dir_all(project.join(session).join("subagents")).expect("mkdir");
    let cwd = format!("{h}/work/synthetic-project");
    let main = [
        r#"{"type":"user","permissionMode":"acceptEdits","message":{"role":"user","content":"please look around the synthetic project"}}"#.to_string(),
        assistant(
            session,
            &cwd,
            &[
                r#"{"type":"text","text":"Looking at the files now."}"#.to_string(),
                tool_use("toolu_01", "Bash", r#"{"command":"ls -la ~/work/synthetic-project","description":"List"}"#),
            ]
            .join(","),
        ),
        // The same call spelled differently: one distinct case, counted twice.
        assistant(session, &cwd, &tool_use("toolu_02", "Bash", r#"{ "description": "List", "command": "ls -la ~/work/synthetic-project" }"#)),
        assistant(session, &cwd, &tool_use("toolu_03", "Bash", r#"{"command":"npx playwright install chromium"}"#)),
        assistant(session, &cwd, &tool_use("toolu_04", "Read", &format!(r#"{{"file_path":"{h}/work/synthetic-project/README.md"}}"#))),
        assistant(session, &cwd, &tool_use("toolu_05", "mcp__synthsrv__lookup", r#"{"query":"quarterly widgets"}"#)),
        assistant(session, &cwd, &tool_use("toolu_06", "SomeFutureTool", r#"{"flag":true}"#)),
        assistant(session, &cwd, &tool_use("toolu_07", "Bash", r#"{"command":"echo café \ud800"}"#)),
        // Deep nesting: Python's json raises RecursionError, guard.py fails open (D-1 territory).
        assistant(
            session,
            &cwd,
            &tool_use("toolu_08", "Bash", &format!(r#"{{"command":"echo deep","x":{}{}}}"#, "[".repeat(3000), "]".repeat(3000))),
        ),
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":"total 0\nreach me at widget.maker@synthetic-corp.test"}]},"toolUseResult":{"stdout":"total 0","stderr":""}}"#.to_string(),
        "this line is not JSON".to_string(),
    ]
    .join("\n");
    fs::write(project.join(format!("{session}.jsonl")), main + "\n").expect("write");
    let sub = [
        assistant(session, &cwd, &tool_use("toolu_11", "Bash", &format!(r#"{{"command":"cat '{h}/Library/Application Support/Google/Chrome/Default/Cookies'"}}"#))),
        assistant(session, &cwd, &tool_use("toolu_12", "Glob", r#"{"pattern":"**/*.rs"}"#)),
    ]
    .join("\n");
    fs::write(project.join(session).join("subagents/agent-a1.jsonl"), sub).expect("write");
    let mut env = Env::default().with("HOME", &h).with("USER", "synthuser");
    if let Some(path) = std::env::var_os("PATH") {
        env = env.with_os("PATH", path);
    }
    Corpus {
        _root: root,
        home,
        env,
    }
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).expect("meta").permissions().mode() & 0o777
}

#[test]
fn collect_reads_subagents_deduplicates_and_keeps_the_corpus_private() {
    let c = corpus();
    let out = replay::run_in(&args(&["collect"]), &c.env, &c.home);
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with(
            "replay collect: 2 transcripts, 13 lines (1 skipped), 10 tool calls (9 distinct)"
        ),
        "{stdout}"
    );
    let dir = c.home.join(".claude/anthology/replay");
    assert_eq!(mode(&dir), 0o700);
    for f in [
        collect::CALLS_FILE,
        collect::TEXTS_FILE,
        collect::MANIFEST_FILE,
        "hmac.key",
    ] {
        assert_eq!(mode(&dir.join(f)), 0o600, "{f}");
    }
    let calls = collect::read_calls(&dir).expect("calls");
    // Sorted paths: the session folder (its subagents) before the session file.
    let ids: Vec<&str> = calls.iter().map(|c| c.tool_use_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "toolu_11", "toolu_12", "toolu_01", "toolu_03", "toolu_04", "toolu_05", "toolu_06",
            "toolu_07", "toolu_08"
        ]
    );
    let by_id = |id: &str| calls.iter().find(|c| c.tool_use_id == id).expect("call");
    let ls = by_id("toolu_01");
    assert_eq!(ls.count, 2, "the two spellings of the ls call are one case");
    assert_eq!(ls.permission_mode, "acceptEdits");
    assert_eq!(
        ls.raw_input,
        br#"{"command":"ls -la ~/work/synthetic-project","description":"List"}"#.to_vec()
    );
    assert_eq!(by_id("toolu_11").permission_mode, "default");
    assert!(
        by_id("toolu_11")
            .transcript_path
            .ends_with("subagents/agent-a1.jsonl")
    );
    assert!(
        calls
            .iter()
            .all(|c| c.count == 1 || c.tool_use_id == "toolu_01")
    );
    let texts = collect::read_texts(&dir).expect("texts");
    assert!(
        texts
            .iter()
            .any(|(_, t)| t.contains("widget.maker@synthetic-corp.test"))
    );
    assert!(
        texts
            .iter()
            .any(|(_, t)| t == "please look around the synthetic project")
    );
    // A second collect keeps the key and rebuilds the same corpus.
    let key = fs::read(dir.join("hmac.key")).expect("key");
    let manifest = collect::manifest_sha256(&dir).expect("manifest");
    assert_eq!(replay::run_in(&args(&["collect"]), &c.env, &c.home).code, 0);
    assert_eq!(fs::read(dir.join("hmac.key")).expect("key"), key);
    assert_eq!(collect::manifest_sha256(&dir).expect("manifest"), manifest);
}

#[test]
fn nothing_is_written_inside_the_repository() {
    let c = corpus();
    let repo = repo();
    for (sub, dir) in [
        ("collect", "port/replay"),
        ("collect", "target/replay-probe"),
        ("report", "port/replay"),
    ] {
        let inside = repo.join(dir);
        let out = replay::run_in(
            &args(&[sub, "--dir", inside.to_str().expect("utf8")]),
            &c.env,
            &c.home,
        );
        assert_eq!(out.code, 2, "{sub} {dir}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("inside the anthology checkout"), "{stderr}");
        assert!(!inside.exists(), "{dir} was created");
    }
}

#[test]
fn the_whole_tool_runs_on_a_synthetic_corpus() {
    let c = corpus();
    let repo = repo();
    let dir = c.home.join(".claude/anthology/replay");
    assert_eq!(replay::run_in(&args(&["collect"]), &c.env, &c.home).code, 0);
    let bin = anthology_testkit::run::anthology_bin();
    let python = have_python();
    if !python {
        // rust-test-macos is the one CI job where the authority comparisons run: never skip there.
        assert!(
            !(cfg!(target_os = "macos") && std::env::var_os("CI").is_some()),
            "macOS CI must have a python3 3.9+ for the replay test"
        );
        eprintln!("replay: no python3 3.9+ here; runner 1 and the Python surfaces are skipped");
    }
    let (runners, surfaces) = if python {
        ("1,2,3,4", "analyze,check,privacy")
    } else {
        ("2,3,4", "")
    };
    let out = replay::run_in(
        &args(&[
            "run",
            "--runners",
            runners,
            "--surfaces",
            surfaces,
            "--bin",
            bin.to_str().expect("utf8"),
            "--repo",
            repo.to_str().expect("utf8"),
            "--jobs",
            "2",
            "--date",
            "2026-01-02",
            "--rust-commit",
            &"0".repeat(40),
        ]),
        &c.env,
        &c.home,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.code == 0 || out.code == 1,
        "exit {}: {stderr}",
        out.code
    );
    let summary = report::Summary::from_json(
        &fs::read_to_string(dir.join("run-2026-01-02.json")).expect("run file"),
    )
    .expect("summary");
    assert_eq!((summary.calls_distinct, summary.calls_total), (9, 10));
    assert_eq!(summary.bucket_distinct, [7, 1, 1]);
    // Runner 4 (binary missing): only the read-only allowlist passes.
    assert_eq!(
        summary.runners.get(&4),
        Some(&[[2, 5, 0], [0, 1, 0], [0, 1, 0]])
    );
    let tsv = fs::read_to_string(dir.join("runner4-2026-01-02.tsv")).expect("runner4 list");
    assert!(
        tsv.contains("\nmcp__synthsrv__lookup\tmcp\t1\t1\t0\t1\t0\n"),
        "{tsv}"
    );
    assert!(tsv.contains("\nBash\tbuiltin\t5\t6\t0\t5\t0\n"), "{tsv}");
    assert_eq!(mode(&dir.join("runner4-2026-01-02.tsv")), 0o600);
    if python {
        assert_eq!(
            summary.runners.get(&1).map(|r| r[0][1]),
            Some(2),
            "Python blocks G1 and G3"
        );
        let privacy = summary
            .compared
            .get("privacy")
            .copied()
            .expect("privacy compared");
        assert_eq!(
            privacy,
            (summary.texts_distinct, summary.texts_distinct),
            "privacy is clean"
        );
        assert_eq!(summary.compared.get("check").map(|c| c.0), Some(9));
        assert_eq!(summary.compared.get("analyze").map(|c| c.0), Some(5));
        let details = fs::read_to_string(dir.join("replay-corpus-details-2026-01-02.jsonl"))
            .expect("details");
        assert!(!details.contains("oracle error"), "{details}");
    }
    assert_eq!(summary.unexplained(), 0, "{stderr}");

    let out_md = c.home.join("report.md");
    let out = replay::run_in(
        &args(&[
            "report",
            "--date",
            "2026-01-02",
            "--out",
            out_md.to_str().expect("utf8"),
        ]),
        &c.env,
        &c.home,
    );
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    let text = fs::read_to_string(&out_md).expect("report");
    assert!(text.starts_with("# Replay report 2026-01-02\n"), "{text}");
    let strings = report::corpus_strings(
        &dir,
        vec![c.home.to_string_lossy().into_owned(), "synthuser".into()],
    )
    .expect("strings");
    for s in &strings {
        // Strings made only of report words and numbers ("total 0") are what the test removes.
        if s.chars().count() > 3 && !report::scan(s).0.is_empty() {
            assert!(
                !text.contains(s.as_str()),
                "the report holds the corpus string {s:?}"
            );
        }
    }
    for leak in [
        "Bash",
        "synthsrv",
        "SomeFutureTool",
        "synthetic",
        "toolu_",
        "/",
    ] {
        assert!(!text.contains(leak), "the report names {leak:?}");
    }
    // An injected leak fails the corpus-string test, and nothing is written.
    let mut leaky = report::Summary::from_json(
        &fs::read_to_string(dir.join("run-2026-01-02.json")).expect("run"),
    )
    .expect("summary");
    leaky.python_commit = "x".into();
    let rendered = report::render(&leaky) + "* quarterly widgets\n";
    let check = report::check_report(&rendered, &mut strings.clone().into_iter());
    assert!(
        check.hits.contains(&"quarterly widgets".to_string()),
        "{check:?}"
    );
}

/// A stand-in `anthology` (a shell script) that allows every call: plain mode exits 0 silently,
/// `--shim` writes the allow token. It proves runner 3's seeding (the shim takes the hot path and
/// runs the seeded copy) and the runner 2 vs 3 comparison without waiting for GD2.
#[test]
fn runner_3_runs_the_seeded_binary_and_matches_runner_2() {
    let c = corpus();
    let repo = repo();
    let dir = c.home.join(".claude/anthology/replay");
    assert_eq!(replay::run_in(&args(&["collect"]), &c.env, &c.home).code, 0);
    let bin = c.home.join("fake-anthology");
    fs::write(
        &bin,
        "#!/bin/sh\ncase $1 in\n--version) echo 'anthology 0.1.0 (test)'; exit 0 ;;\n\
         guard) cat >/dev/null; [ \"$3\" = --shim ] && printf 'anthology-guard:allow:v1\\n' >&2; exit 0 ;;\n\
         esac\nexit 64\n",
    )
    .expect("write");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
    let out = replay::run_in(
        &args(&[
            "run",
            "--runners",
            "2,3,4",
            "--surfaces",
            "",
            "--bin",
            bin.to_str().expect("utf8"),
            "--repo",
            repo.to_str().expect("utf8"),
            "--jobs",
            "2",
            "--date",
            "2026-01-03",
        ]),
        &c.env,
        &c.home,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.code, 0, "{stderr}");
    assert!(!stderr.contains("quarantined"), "{stderr}");
    let summary = report::Summary::from_json(
        &fs::read_to_string(dir.join("run-2026-01-03.json")).expect("run file"),
    )
    .expect("summary");
    assert_eq!(summary.compared.get("runners"), Some(&(9, 9)));
    assert_eq!(
        summary.runners.get(&3),
        Some(&[[7, 0, 0], [1, 0, 0], [1, 0, 0]])
    );
    assert!(summary.diffs.is_empty());
}
