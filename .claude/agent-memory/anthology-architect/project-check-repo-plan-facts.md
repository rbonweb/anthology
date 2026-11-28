---
name: 'Facts that shaped the 2026-11-28 check-repo plan'
description: 'yara-x is blocked by the serde_json ban; GuardDog v3 rules are a small YARA subset; guide is at its 4050 cap; skill fenced commands are pinned'
metadata:
  type: project
  modified: 2026-11-28
---

Why: these decided the check-repo design (.superpowers/sdd/2026-11-28-check-repo/) and are not obvious from one file.
- yara-x needs serde_json (required dep) and wasmtime plus cranelift on native targets. crates/repo-tests/tests/no_serde_json.rs bans serde_json in the anthology-cli graph (GC-16), and deny.toml denies duplicate versions. The yr CLI tarball is 8.5-10 MB against 4.4-5 MB for the whole anthology binary.
- GuardDog default branch is v3; guarddog/analyzer/sourcecode holds 55 .yar (11 capability, 44 threat) plus 4 .meta private-rule files, no Semgrep. The rules use only regex and text strings with nocase, two wide or hex strings, counts (#x >= N, #a > #b + #c), N of / any of sets, not, private rule refs and an ecosystem external. Pairing capability with threat is Python (risk_engine.py), not YARA. Apache-2.0 with a Datadog NOTICE.
- regex 1.13.2 already pulls aho-corasick 1.1.5 and regex-syntax 0.8.12, so adding them as direct deps adds no crate.
- On 2026-11-28 plugins/anthology/templates/routing-guide.md is exactly 4050 chars, the cap in anthology-runtime/tests/session_start.rs. PR 331 raises the cap to 4100 and adds about 34 to 43 chars. Unpinned, low-meaning text: ": writers branch from HEAD" and " MCP" after chrome-devtools.
- repo-tests cutover_skills.rs every_skill_command_runs_the_anthology_wrapper requires every fenced command in the setup skill to start with the bin/anthology setup wrapper, so setup must hand off to another skill in prose.
- The hook.session-start golden uses a synthetic guide, so a guide edit needs no bless; a new systemMessage line or state key may.
- The release tarball ships only the binary and LICENSE (anthology-release.yml), so a crate NOTICE does not reach users unless the binary prints it.
- osv-scanner reads osv-scanner.toml from each scanned folder unless --config is given; the research table missed this.
