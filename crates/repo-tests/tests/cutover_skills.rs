//! CO2: the skills and agent commands once the Python is gone (plan CO2; GC-15 F-8, GC-14). The
//! replacements the ledger names for test_roster's parser introspection and test_memory_portable
//! `SkillsTest`.

use std::collections::BTreeSet;
use std::ffi::OsString;

use anthology_core::Env;
use anthology_memory::index::{self, MemDeps};
use regex::Regex;
use repo_tests::{frontmatter, repo};

const SKILLS: &str = "plugins/anthology/skills";

/// The `--` options of `anthology mem save --help` (what `bin/anthology-mem save` runs), less
/// `--help`, `--json` and `--repo`. `--repo` saves into the repo's own memory, which only the lead
/// does; no agent's Memory command passes it, so it is not in roster.rs's `SAVE_OPTIONS`.
fn mem_save_options() -> BTreeSet<String> {
    let args: Vec<OsString> = ["save", "--help"].iter().map(OsString::from).collect();
    let out = index::main(&args, &Env::default(), &MemDeps::default());
    assert_eq!(out.code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    let help = String::from_utf8(out.stdout).expect("utf-8 help");
    Regex::new(r"--[a-z]+")
        .unwrap()
        .find_iter(&help)
        .map(|m| m.as_str().to_string())
        .filter(|o| !["--help", "--json", "--repo"].contains(&o.as_str()))
        .collect()
}

/// test_roster `test_memory_command_flags_equal_the_save_subcommand_options`, the parser half:
/// the flags each agent's Memory command passes are the options of `anthology mem save --help`
/// (less `--replace`, which no agent passes), and roster.rs's `SAVE_OPTIONS` is that list.
#[test]
fn the_agent_memory_flags_equal_the_options_of_mem_save_help() {
    let options = mem_save_options();
    let want: BTreeSet<String> = [
        "--agent",
        "--type",
        "--slug",
        "--name",
        "--description",
        "--body",
        "--replace",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(options, want, "roster.rs SAVE_OPTIONS");
    let without_replace: BTreeSet<String> =
        options.into_iter().filter(|o| o != "--replace").collect();
    let command = Regex::new(r"`(anthology-mem save [^`]*)`").unwrap();
    let flag = Regex::new(r"--[a-z]+").unwrap();
    let mut agents = 0;
    for name in repo::list_dir("plugins/anthology/agents") {
        if !name.ends_with(".md") {
            continue;
        }
        let rel = format!("plugins/anthology/agents/{name}");
        let text = repo::read(&rel);
        let Some(m) = command.captures(&text) else {
            continue;
        };
        agents += 1;
        let flags: BTreeSet<String> = flag
            .find_iter(&m[1])
            .map(|f| f.as_str().to_string())
            .collect();
        assert_eq!(flags, without_replace, "{rel}");
    }
    assert!(agents > 0, "no agent runs `anthology-mem save`");
}

fn skill(name: &str) -> (repo_tests::yaml::Yaml, String) {
    frontmatter::load(&format!("{SKILLS}/{name}/SKILL.md"))
}

/// test_memory_portable `SkillsTest.test_skill_bodies_follow_the_flow`.
#[test]
fn test_skill_bodies_follow_the_flow() {
    let (_, export_body) = skill("export");
    for needle in ["personal notes", "counts", "exports"] {
        assert!(export_body.contains(needle), "export: {needle}");
    }
    assert!(
        export_body.to_lowercase().contains("never"),
        "export: never"
    );
    assert!(!export_body.contains("add `--out <folder>`"), "export");
    let (_, import_body) = skill("import");
    for needle in [
        "--dry-run",
        "--yes",
        "--sha256",
        ".imported-",
        "confirmation",
        "delete",
    ] {
        assert!(import_body.contains(needle), "import: {needle}");
    }
    assert!(import_body.find("--dry-run") < import_body.find("--yes"));
}

// ---- The skills after the switch (F-8, GC-14) -----------------------------------------------

/// What every skill command runs after the cutover (F-8): the plugin's `bin/anthology` wrapper.
const ANTHOLOGY: &str = "\"${CLAUDE_PLUGIN_ROOT}/bin/anthology\"";

/// The fenced command lines of a skill body.
fn commands(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if fenced && !line.trim().is_empty() {
            out.push(line.trim().to_string());
        }
    }
    out
}

/// test_memory_portable `SkillsTest.test_export_and_import_skills`: both skills are manual-only
/// and run `anthology memory export|import` through the plugin's wrapper (F-8).
#[test]
fn test_export_and_import_skills() {
    for name in ["export", "import"] {
        let (meta, body) = skill(name);
        assert_eq!(meta["name"].as_str(), Some(name));
        assert!(
            meta["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{name}: description"
        );
        assert_eq!(
            meta["disable-model-invocation"].as_bool(),
            Some(true),
            "{name}"
        );
        let want = format!("{ANTHOLOGY} memory {name} ");
        assert!(
            commands(&body).iter().any(|c| c.starts_with(&want)),
            "{name}: {want}"
        );
    }
}

/// F-8: every command a skill runs is `bin/anthology <subcommand>`, and no skill names Python.
#[test]
fn every_skill_command_runs_the_anthology_wrapper() {
    for (name, sub) in [
        ("setup", "setup"),
        ("export", "memory"),
        ("import", "memory"),
    ] {
        let (_, body) = skill(name);
        let cmds = commands(&body);
        assert!(!cmds.is_empty(), "{name}: no command");
        for c in cmds {
            assert!(c.starts_with(&format!("{ANTHOLOGY} {sub} ")), "{name}: {c}");
        }
        assert!(!body.to_lowercase().contains("python"), "{name}: python");
    }
}

/// GC-14: the setup skill says what to do when a Bash call is blocked with "(setup)".
#[test]
fn the_setup_skill_carries_the_gc_14_guidance() {
    let (_, body) = skill("setup");
    let line = "If a Bash call is blocked with \"(setup)\", wait a minute and run the same command \
                once more: the guard reinstalls its binary. If it is still blocked, tell the user \
                to start a new session while online, or to disable anthology-guardrails with \
                /plugin.";
    assert!(body.lines().any(|l| l == line), "setup: the GC-14 line");
}
