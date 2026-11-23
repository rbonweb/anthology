---
name: 'perm classify: where the path and git check live and how they are tested'
description: 'anthology perm classify sits in anthology-cli (cmd/perm.rs, cmd/perm/{rules,paths,git,words}.rs); review rulings from #172 rounds 1 to 3 and how tests prove each trap'
metadata:
  type: project
  modified: 2026-11-22
---

anthology perm classify (P4, ADR-034; renamed from ADR-033 because another PR took that number) lives in crates/anthology-cli/src/cmd/perm.rs with rules.rs (Read() matcher, anchors read wide, a rule with braces or a bang refuses), paths.rs (resolve, must exist, containment folded on macOS, glob refused, walk with a 20000-entry budget; any symlink or hard-linked file in a walk refuses because GNU and Apple diff follow links), git.rs (asks git itself: rev-parse and config --list --includes --show-origin -z side by side; key allowlist safe_key; osxkeychain allowed only from Apple own gitconfig by path; object store must resolve under a root) and words.rs (git signature atoms; one option ALLOWLIST per git subcommand, never a denylist, because round 3 showed log -U3 and log --check slipping past the round-2 denylist; GREP_OPTIONS and RIPGREP_CONFIG_PATH).
Why: the security review asked for proof per trap, so each test first runs the real tool and shows the marker ran or the secret printed, then asserts no decision. Some git versions need -p next to --word-diff or --color-words before they print a patch, so demos add it.
How to apply: every program a line runs is found on PATH and canonicalized, and must not land under a root, the cwd or a cd target; a program missing from PATH is skipped because the CI images have no rg. A PATH entry under those places refuses every line, so tests that put a fake program on PATH must place it outside the fixture root unless they test that refusal. The json parser keeps the last duplicate key, so perm.rs scans the top-level keys itself. Leave workspace versions alone in feature PRs. The shims test server binds port 443, so call bs::server() only when bs::net_enabled(). Mutation-check each guard by adding && false and running the perm test.
