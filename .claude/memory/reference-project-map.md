---
name: reference-project-map
description: "How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing."
metadata:
  node_type: memory
  type: reference
  modified: 2026-11-27T08:05:34.573Z
---

## Overview
Three Claude Code plugins on one static Rust binary, `anthology`. ATL (`atl/`, a fork) shortens command output. Plugins are POSIX `sh` plus TypeScript mods; the rest is Rust in `crates/`. Work lands on `build`; CI gates it.

## Pieces
- `plugins/anthology` - agents, `templates/routing-guide.md`, `hooks/`, `bin/` shims, `atl.lock`, `anthology.lock`. All 11 `agents/*.md` share a byte-identical `## Shell` section (pinned in `roster.rs`).
- `plugins/anthology/hooks/mods/` - `index.ts` is the one `modules` entry in `hooks.json`; it registers each shared event once, each part in `isolated`; one file per feature. `delegation.ts`: `isEmbeddedHost` skips the refusal and coordinator section. New `$.state` key: `types/anthology.d.ts`.
- `plugins/anthology-guardrails` - `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh`, `anthology.lock` are byte-identical to the anthology plugin's: edit both.
- `plugins/anthology-superpowers`, `plugins/anthology/knowledge/**` - generated from `vendor/sources.json` by `anthology-dev vendor`. Never hand-edit: change `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin. Its `hooks/session-start` host check: `vendor/patches/superpowers.patch`.
- `anthology-cli` - the binary; `src/cmd/perm.rs`, `perm/`: `anthology perm classify`. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance; ATL builds it too) <- `guard` <- the same three.
- `anthology-guard/src/host.rs`: `embedded()` (ADR-037; truth table: `delegation.test.ts`, run by `repo-tests/tests/host_predicate.rs`); `check.rs`, `hook.rs`. `path_vars.rs` there: `ANTHOLOGY_GUARD_PATH_VARS`; `anthology-shell/src/path_vars.rs`.
- `anthology-memory`: `save.rs` (`reindex`), `index.rs`. `anthology-runtime/src/setup.rs`: outside a repo, `memory.repo` lists `repos_below`; setup `SKILL.md` asks which and passes `--cwd`. `anthology-runtime/src/project.rs`: after the guide, session start lists the repo's own agents and skills and `MAP_LINE` (approved repos only); the `.claude/anthology.json` switches `routing_guide` and `delegation` apply in approved repos only (`anthology switch`).
- `anthology-dev` (`version_bump.rs`), `difftest` (golden replayer), `repo-tests` never ship. `atl/`, `fuzz/` have own workspaces and `Cargo.lock`; ATL's gate: `src/hooks/anthology_gate.rs`, then `read_only_proof.rs` (drops an ask when `perm classify` proves the line read-only, via `ATL_PERM_CLASSIFY`; none in auto mode).
- `vendor/`: `sources.json` (all pins), `patches/`, `snapshots/`; `scripts/dev.sh vendor sync`. `evals/`: runners export `ANTHOLOGY_HOST_MODE=full`. `ci/jobs/`. `changelog.d/<plugin>/*.md`: `release-edit` reads and deletes them.

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` checks the binary against `anthology.lock` (downloads if missing) -> injects the guide, installs ATL, warns of an enabled official `superpowers` (`doctor.rs`).
- Bash call: `bin/atl-hook`, then `hooks/no-attribution.sh`; guardrails `guard.sh` -> `anthology guard` (binary missing: a detached `bootstrap.sh fetch` downloads it; `guard.sh` passes only that installer). All `onFailure: block`, timeout 60. At a prompt, the permissions mod allows only a command `anthology perm classify` proves read-only.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves `[Unreleased]` entries, commits, tags, `git push --atomic`; last line `RELEASE OK|FAILED`. Dev calls share one `rust-test.sh` session. Release a dependency plugin first; `lock check-fresh` refuses a lock older than the workspace (`lock-fresh.sh` only warns, exit 0).

## Checks
- All: `scripts/rust-test.sh`; one crate: `... test -p <crate>`. One-shots and `session-stop` drop the target volume (`ANTHOLOGY_KEEP_TARGET=1` keeps it); `session-start` runs `gc`; `anthology-sccache` is shared. Remote: `ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <args>` cleans only its own items.
- CI (`ci.yml`): one job per `ci/jobs/` script (`shellcheck.sh` also checks `release.sh`, `rust-test.sh`, `remote-build.sh`). On a PR `rust-scope.sh` (rules `ci/scope-rules.tsv`, weights `ci/test-weights.tsv`, graph `rust-graph.mjs`) picks crates a change reaches, their golden surfaces (`golden_replay::<surface>`), atl, fuzz, macOS; `rust-test-linux`/`-macos` are the required gates. A push, the `full-ci` label, `[full-ci]` in a commit or a test-machinery change runs all. A PR in merge conflict runs no CI: merge `origin/build`.
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`. Ledger: `port/ledger/*.tsv` maps inherited tests to Rust tests or drop reasons (`ledger.sh`). Budgets: guide max 3800 characters, session-start output max 9000 (`session_start.rs`).

## Cleanup duties
- Workspace version bump: also update `Cargo.lock`, `fuzz/Cargo.lock` (`ci/jobs/fuzz.sh` fails on a stale one: `cargo metadata --locked`). Then follow Binary release.
- Change to `crates/*/src` or `Cargo.toml` in the `anthology-cli` graph, or `rust-toolchain.toml`: new `anthology-bin--v*` tag and `lock write`, or `release.sh` stops.
- Plugin folder change: bump `version` in its `plugin.json` above its latest release tag (one already on `build` counts, ADR-036); add a line under `[Unreleased]` in its `CHANGELOG.md`; or `version-bump` fails.
- `anthology perm classify` output change: update `classifiedPaths` in `hooks/mods/permissions.ts`.
- `evals/baseline.json`: record only from the CI run of `evals.yml`.
- `atl.lock`, `bin/*` or `hooks/*` change: release `anthology`. `hooks.json` edit: update pins in `repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs`, `anthology-runtime/tests/atl_install.rs`.
- Agent prompts: run `scripts/dev.sh startup-budget`; raise `BUDGET` (`startup_budget.rs`) only with a stated reason. Changed `setup`, session start, doctor or memory sync output: `scripts/dev.sh golden bless --surface X`.
- Docs: `README.md` front page, at most 120 lines; detail in `docs/*.md` (`DOC_PAGES`, `repo-tests` `docs.rs`).
- New crate: a row in `ci/test-weights.tsv`. New golden surface: a line in `golden_replay` (`difftest/tests/golden.rs`). New top-level folder: a row in `ci/scope-rules.tsv`, or CI runs everything. `repo-tests/tests/ci_scope.rs` pins all three.
- Add, remove or rewire a piece: update this map in the same commit. Memory notes get their own `chore(memory): ...` commit.
