---
name: 'Per-repo switches, project agents section and delegation exemptions'
description: 'Where .claude/anthology.json, the own-agents section, MAP_LINE and the delegation exemptions live, and the test stubs and golden effect'
metadata:
  type: project
  modified: 2026-11-27
---

Session start (crates/anthology-runtime/src/project.rs, session_start.rs): after the guide it adds the repo own agents and skills (.claude/agents/*.md, .claude/skills/*/SKILL.md, a CLAUDE.md table or list under a heading with agent in it, then the user config folder agents and skills), at most 12 entries, parsed with anthology_memory::notes::parse_text_ok. MAP_LINE is added only for a repo approved in /anthology:setup (anthology_memory repo_root, repo_key, approve::is_approved). The extras exist only when the guide was read; .claude/anthology.json routing_guide false skips the guide and the extras. The same file with delegation false turns off hooks/mods/delegation.ts (read with $.fs.read from $.session.root()).
Delegation mod: whole Claude config folder and any file with no .git in its folder or above are exempt; a pure deletion (30 lines, 4 KB) and a config-file edit (15 lines) are tiny fixes; code stays at 5 lines. A failing fs.exists counts as project code.
Tests: delegation.test.ts engine stub answers fs.exists as true for every /.git unless a test passes repos, so existing paths count as in a repo; fs.read answers only <root>/.claude/anthology.json from repoConfig. In Rust, tests use Fx.go_cwd with SessionCase fake_repo for a plain folder and make_repo plus approve for an approved one.
Golden: the hook.session-start golden changed in 256 cases, all a pure insertion of MAP_LINE for an approved repo; compare old and new gz per case id to prove it.
Why: the plugin used to override a user own per-repo agents; these are the pieces to touch when that behavior changes again.
