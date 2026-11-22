---
name: 'perm classify: where the path and git check live and how they are tested'
description: 'anthology perm classify sits in anthology-cli (cmd/perm.rs, cmd/perm/{rules,paths,git,words}.rs); review rulings from #172 rounds 1 and 2 and how tests prove each trap'
metadata:
  type: project
  modified: 2026-11-22
---

anthology perm classify (P4, ADR-034; renamed from ADR-033 because another PR took that number) lives in crates/anthology-cli/src/cmd/perm.rs with rules.rs (Read() matcher, anchors read wide, a rule with braces or a bang refuses), paths.rs (resolve, must exist, containment folded on macOS, glob refused, walk with a 20000-entry budget; any symlink or hard-linked file in a walk refuses because GNU and Apple diff follow links), git.rs (asks git itself: rev-parse and config --list --includes --show-origin -z side by side; key allowlist safe_key; osxkeychain allowed only from Apple own gitconfig by path; object store must resolve under a root) and words.rs (refusals on analyze prog and args: git signature atoms, history forms such as show, log -p, diff --cached, diff or blame with a revision; GREP_OPTIONS and RIPGREP_CONFIG_PATH). It is not in anthology-guard because another PR held that crate at the time.
Why: the security review asked for proof per trap, so each test first runs the real tool and shows the marker ran or the secret printed, then asserts no decision. Some git versions need -p next to --word-diff or --color-words before they print a patch, so demos add it.
How to apply: a PATH entry under a root, the cwd or a cd target refuses every line, and the git asked is resolved once and must lie outside those, so tests that put a fake program on PATH must place it outside the fixture root unless they test that refusal. The json parser keeps the last duplicate key, so perm.rs scans the top-level keys itself. Leave workspace versions alone in feature PRs; the release branch sets them. Mutation-check each guard by adding && false, run the perm test, then git checkout the src folder.
