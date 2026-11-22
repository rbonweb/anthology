---
name: 'perm classify: where the path and git check live and how they are tested'
description: 'anthology perm classify sits in anthology-cli (cmd/perm.rs, cmd/perm/{rules,paths,git}.rs); output lists resolved paths; git traps proven with real git; review rulings'
metadata:
  type: project
  modified: 2026-11-22
---

anthology perm classify (P4, ADR-033) lives in crates/anthology-cli/src/cmd/perm.rs with rules.rs (Read() matcher, anchors read wide, anchored_reaches for rules below a directory), paths.rs (readonly_paths: resolve, must exist, containment folded on macOS, glob refused, walk with a 20000-entry budget) and git.rs (asks git itself: rev-parse and config --list --includes -z run side by side; key allowlist safe_key; hooks, alternates, submodule, index checks). It is not in anthology-guard because another PR held that crate at the time.
Contract after the #164 review: exit 0 prints the token line and then one resolved absolute path per line (cd targets, files, recursive starts, git work tree, git dir, common dir); the mod checks each with Claude Code Read permission. The request cwd is not trusted: without a leading absolute cd, relative paths, implicit dot and git refuse. Any GIT_* env var refuses a git line. The command must be printable ASCII.
Tests: crates/anthology-cli/tests/perm.rs sends cd CWD && COMMAND through Fx::classify; fixtures run git with GIT_CONFIG_NOSYSTEM but the binary never gets it. Every git trap test first asserts a marker program ran with the real git. Apple git carries credential.helper in its own config, so on macOS every git line is refused and the allow tests only check the refusal (Fx::platform_keys); Linux CI checks the allows. The container needs git, and procps for anthology-runtime bootstrap tests (they call ps). The stderr-non-empty check in run_git is not pinned by a test.
Why: the security review and P5 extend this, and a mutant that survives here is a silent allow.
