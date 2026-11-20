//! `tests/support/agents.rs` (the port of `tests/_agents.py`) builds the same repository.
#![forbid(unsafe_code)]

mod support;

use support::agents::make_repo;

#[test]
fn make_repo_writes_the_three_agents_and_their_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    make_repo(root);
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).expect(rel);
    assert_eq!(
        read("plugins/anthology/knowledge/alpha/SKILL.md"),
        "a".repeat(100)
    );
    assert_eq!(
        read("plugins/anthology/knowledge/beta/SKILL.md"),
        "b".repeat(200)
    );
    assert_eq!(
        read("plugins/anthology-superpowers/skills/plan/SKILL.md"),
        "p".repeat(400)
    );
    // Byte for byte what `AGENT.format(...)` gives in tests/_agents.py (printed by CPython).
    assert_eq!(
        read("plugins/anthology/agents/reader.md"),
        "---\nname: reader\ndescription: \"Use when testing.\"\nmodel: haiku\n---\n\n## \
         Startup\nBefore any other work, Read every file below, in order:\n- \
         `${CLAUDE_PLUGIN_ROOT}/knowledge/alpha/SKILL.md`\n- \
         `${CLAUDE_PLUGIN_ROOT}/knowledge/beta/SKILL.md`\n\n## Role\nTest agent.\n"
    );
    assert_eq!(
        read("plugins/anthology/agents/skilled.md"),
        "---\nname: skilled\ndescription: \"Use when testing.\"\nmodel: haiku\nskills:\n  - \
         anthology-superpowers:plan\n---\n\n## Startup\nBefore any other work, Read every file \
         below, in order:\n- `${CLAUDE_PLUGIN_ROOT}/knowledge/beta/SKILL.md`\n\n## Role\nTest \
         agent.\n"
    );
    assert_eq!(
        read("plugins/anthology/agents/bare.md"),
        "---\nname: bare\ndescription: \"Use when testing.\"\nmodel: haiku\n---\n\n## \
         Startup\nBefore any other work, Read every file below, in order:\n\n\n## Role\nTest \
         agent.\n"
    );
}
