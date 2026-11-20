//! Port of `tests/_agents.py` (F4, m-13): a tiny agent repository for the startup-budget tests.

use std::path::{Path, PathBuf};

/// The agent file template: `{name}`, `{skills}` and `{files}` are filled in by [`agent`].
pub const AGENT: &str = "---
name: {name}
description: \"Use when testing.\"
model: haiku
{skills}---

## Startup
Before any other work, Read every file below, in order:
{files}

## Role
Test agent.
";

/// `AGENT.format(name=..., skills=..., files=...)`.
pub fn agent(name: &str, skills: &str, files: &str) -> String {
    AGENT
        .replace("{name}", name)
        .replace("{skills}", skills)
        .replace("{files}", files)
}

/// `write(root, rel, text)`: creates the parent folders and writes UTF-8 text.
pub fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(&path, text).expect("write");
    path
}

/// `make_repo(root)`: three agents, `reader` (two Startup files), `skilled` (one file, one
/// preload) and `bare`.
pub fn make_repo(root: &Path) {
    write(
        root,
        "plugins/anthology/knowledge/alpha/SKILL.md",
        &"a".repeat(100),
    );
    write(
        root,
        "plugins/anthology/knowledge/beta/SKILL.md",
        &"b".repeat(200),
    );
    write(
        root,
        "plugins/anthology-superpowers/skills/plan/SKILL.md",
        &"p".repeat(400),
    );
    let line = |k: &str| format!("- `${{CLAUDE_PLUGIN_ROOT}}/knowledge/{k}/SKILL.md`");
    let agents = "plugins/anthology/agents/";
    write(
        root,
        &format!("{agents}reader.md"),
        &agent(
            "reader",
            "",
            &format!("{}\n{}", line("alpha"), line("beta")),
        ),
    );
    write(
        root,
        &format!("{agents}skilled.md"),
        &agent(
            "skilled",
            "skills:\n  - anthology-superpowers:plan\n",
            &line("beta"),
        ),
    );
    write(root, &format!("{agents}bare.md"), &agent("bare", "", ""));
}
