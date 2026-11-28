---
name: reference-project-map
description: "How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing."
metadata:
  node_type: memory
  type: reference
  modified: 2026-11-28T11:34:52.774Z
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
---

## Overview
Three Claude Code plugins, one static Rust binary, `anthology`; ATL (`atl/`, a fork) shortens command output. Plugins are POSIX `sh` plus TypeScript mods; the rest is Rust in `crates/`. Work lands on `build`; CI gates it.

## Pieces
- `plugins/anthology` - agents, `templates/routing-guide.md`, `hooks/`, `bin/` shims, `atl.lock`, `anthology.lock`. All 12 `agents/*.md` share a byte-identical `## Shell` section (pinned in `roster.rs`). New agent: `anthology-core` ROSTER (bin release), `repo.rs`, `smoke.rs`, `helpers.rs`, `session_start.rs` AGENTS, `BUDGET`, evals, a guide row.
- `anthology-slop` (regex rules) -> `anthology slop hook|check`; `hooks/slop-gate.sh` (PreToolUse Write/Edit): only exit 10 blocks, a 5 s deadline allows; `docs/slop-gate.md`.
- `plugins/anthology/hooks/mods/` - `index.ts`, the one `modules` entry in `hooks.json`, registers each shared event once, each part in `isolated`; one file per feature. `delegation.ts`: `isEmbeddedHost` skips refusal and coordinator section; its edit hook refuses the mask marker. `masking.ts` (`session.append` tool-result) -> `anthology guard mask` (`anthology-guard/src/mask.rs`); `nudge.ts` (part of `agent.report`) reads the SDD ledger. New `$.state` key: `types/anthology.d.ts`.
- `plugins/anthology-guardrails` - `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh`, `anthology.lock` are byte-identical to the anthology plugin's: edit both.
- `plugins/anthology-superpowers`, `plugins/anthology/knowledge/**` - generated from `vendor/sources.json` by `anthology-dev vendor` (`drop-paths` omits upstream files), except `FIRST_PARTY_KNOWLEDGE` in `repo.rs`. Never hand-edit: change `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin. Its `hooks/session-start` host check: `vendor/patches/superpowers.patch`.
- `anthology-cli` - the binary; `src/cmd/perm.rs`, `perm/`: `anthology perm classify`. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance; ATL builds it too) <- `guard` <- the same three.
- `anthology-guard/src/host.rs`: `embedded()` (ADR-037; truth table: `delegation.test.ts`, run by `repo-tests/tests/host_predicate.rs`); `path_vars.rs` there: `ANTHOLOGY_GUARD_PATH_VARS`; `anthology-shell/src/path_vars.rs`.
- `anthology-memory`: `save.rs`, `index.rs`. `anthology-runtime/src/setup.rs`: outside a repo, `memory.repo` lists `repos_below`; setup `SKILL.md` asks which and passes `--cwd`. `anthology-runtime/src/project.rs`: after the guide, the repo's own agents, skills and `MAP_LINE`; `.claude/anthology.json` `routing_guide`/`delegation` switches (`anthology switch`); both in approved repos only.
- `anthology-dev` (`version_bump.rs`), `difftest` (golden replayer), `repo-tests` never ship. `atl/`, `fuzz/` have own workspaces and `Cargo.lock`; ATL's gate: `src/hooks/anthology_gate.rs`, then `read_only_proof.rs` (ATL never widens permissions: it rewrites only a line `perm classify` proves read-only, via `ATL_PERM_CLASSIFY`, or user allow rules cover, `user_allow.rs`; none in auto mode; typed `atl` folder searches ask). `BYPASS_NOTE` (`anthology-settings/src/ops.rs`), the setup `SKILL.md` and the runtime setup test change together.
- `vendor/`: `sources.json` (all pins), `patches/`, `snapshots/`; `scripts/dev.sh vendor sync`. `evals/`: runners export `ANTHOLOGY_HOST_MODE=full`. `changelog.d/<plugin>/*.md`: `release-edit` reads and deletes them.

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` checks the binary against `anthology.lock` (downloads if missing) -> guide, ATL install, warning for official `superpowers` (`doctor.rs`).
- Bash call: `bin/atl-hook`, then `hooks/no-attribution.sh`, then `hooks/commit-privacy.sh` -> `anthology privacy hook` (`cmd/privacy.rs`, `anthology-memory/src/commit_scan.rs`; fails open, only exit 10 blocks); guardrails `guard.sh` -> `anthology guard` (binary missing: a detached `bootstrap.sh fetch` downloads it; `guard.sh` passes only that installer). All `onFailure: block`, timeout 60. The permissions mod allows only what `perm classify` proves read-only.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves `[Unreleased]` entries, commits, tags, `git push --atomic`; last line `RELEASE OK|FAILED`. Release a dependency plugin first; `lock check-fresh` refuses a lock older than the workspace (`lock-fresh.sh` only warns).

## Checks
- All: `scripts/rust-test.sh`; one crate: `... test -p <crate>`. One-shots keep the checkout's target volume (`ANTHOLOGY_KEEP_TARGET=0` drops it); `session-stop` drops it; `session-start` runs `gc`, which removes only gone worktrees' volumes; `anthology-sccache` is shared. Images and shared caches are kept, trimmed only under disk pressure. Remote: `ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <args>`: one folder and volume per run, flock locks, a cap on concurrent builds; `--clean-ours` cleans this checkout's dead runs, `--clean-all-ours` all of ours (refuses while a run is live; `--with-caches` also wipes caches and image).
- CI (`ci.yml`): one job per `ci/jobs/` script (`shellcheck.sh` also: `release.sh`, `rust-test.sh`, `remote-build.sh`). On a PR `rust-scope.sh` (rules, `job:` and `crate:` rows: `ci/scope-rules.tsv`; weights `ci/test-weights.tsv`; graph `rust-graph.mjs`) picks the crates a change reaches, their golden surfaces, atl, fuzz, macOS and the repo jobs that read it; gitleaks, deny, shellcheck always run. `rust-test-linux`/`-macos` gate. A push, the `full-ci` label, `[full-ci]` in a commit or a test-machinery change runs all. A PR in merge conflict runs no CI: merge `origin/build`.
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`. Ledger: `port/ledger/*.tsv` maps inherited tests to Rust tests or drop reasons. Budgets: guide ≤4100 characters, session-start output ≤9000 (`session_start.rs`).

## Cleanup duties
- Workspace version bump: also update `Cargo.lock`, `fuzz/Cargo.lock` (else `ci/jobs/fuzz.sh` fails). Then follow Binary release.
- Change to `crates/*/src` or `Cargo.toml` in the `anthology-cli` graph, or `rust-toolchain.toml`: new `anthology-bin--v*` tag and `lock write`, or `release.sh` stops.
- Plugin folder change: bump `version` in its `plugin.json` above its latest release tag (one already on `build` counts, ADR-036); an `[Unreleased]` line in its `CHANGELOG.md`; or `version-bump` fails.
- `anthology perm classify` output change: update `classifiedPaths` in `hooks/mods/permissions.ts`.
- `evals/baseline.json`: record only from the CI run of `evals.yml`.
- `atl.lock`, `bin/*` or `hooks/*` change: release `anthology`. `hooks.json` edit: update pins in `repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs`, `anthology-runtime/tests/atl_install.rs`.
- Agent prompts: `scripts/dev.sh startup-budget` (a `, only …` line adds its largest file); raise its `BUDGET` with a reason. Changed `setup`, session start, doctor or memory sync output: `scripts/dev.sh golden bless --surface X`.
- Docs: `README.md` front page, ≤120 lines; detail in `docs/*.md` (`DOC_PAGES`, `repo-tests` `docs.rs`).
- New crate: a row in `ci/test-weights.tsv`. New golden surface: a line in `golden_replay` (`difftest/tests/golden.rs`). New top-level folder: a row in `ci/scope-rules.tsv`, or CI runs everything. `repo-tests/tests/ci_scope.rs` pins all three.
- Add, remove or rewire a piece: put the map line in the PR body; the lead applies it. Memory notes get their own `chore(memory): ...` commit.
