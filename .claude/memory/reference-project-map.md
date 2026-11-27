---
name: reference-project-map
description: "How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing."
metadata:
  node_type: memory
  type: reference
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-25T06:25:20.874Z
---

## Overview
Three Claude Code plugins backed by one static Rust binary, `anthology`. ATL (`atl/`, a fork) shortens command output. Plugins are POSIX `sh` plus TypeScript mods; the rest is Rust in `crates/`. Work lands on `build`; CI is the gate.

## Pieces
- `plugins/anthology` - agents, `templates/routing-guide.md`, `hooks/`, `bin/` shims, `atl.lock`, `anthology.lock`. All 11 `agents/*.md` share a byte-identical `## Shell` section (`every_agent_has_the_same_shell_section`, `roster.rs`).
- `plugins/anthology/hooks/mods/` - Claude Code mods: `index.ts` (the one module `hooks.json` names under `"modules"`), `util.ts`, one file per feature. State keys `plugins/anthology/types/anthology.d.ts`. `delegation.ts`: `isEmbeddedHost` (ADR-037) skips the refusal and the coordinator section.
- `plugins/anthology-guardrails` - `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh` and `anthology.lock` are byte-identical to the anthology plugin's: edit one, edit both.
- `plugins/anthology-superpowers`, `plugins/anthology/knowledge/**` - generated from `vendor/sources.json` by `anthology-dev vendor`. Never hand-edit: change `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin. The `hooks/session-start` host check (ADR-037) comes from `vendor/patches/superpowers.patch`.
- `crates/anthology-cli` - the `anthology` binary; `src/cmd/perm.rs` and `perm/` are `anthology perm classify`, run by the permissions mod. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance, ATL builds it too) <- `guard` <- `memory`, `settings`, `runtime`.
- `anthology-guard/src/host.rs`: `embedded()`, `embedded_from_env()` (ADR-037; truth table in `delegation.test.ts`, run by `repo-tests/tests/host_predicate.rs`). `check.rs`: `check_explained` (verdict plus G0 cause: `Analysis.halt_cause`, else an Opaque mark); `hook.rs`: G0 hints, embedded lines, `decide_for`.
- `anthology-shell/src/path_vars.rs`: `PathVars`, `analyze_with_vars`, the carve-out (d) text check. `anthology-guard/src/path_vars.rs`: `HostPathVars`, the `ANTHOLOGY_GUARD_PATH_VARS` name, value and file checks.
- `anthology-memory`: `save.rs` (`save_repo_note_with`, `reindex`, `write_note`), `index.rs` (`run_reindex`). `anthology-runtime/src/setup.rs`: outside a repo, `memory.repo` lists `repos_below[_more]`; setup `SKILL.md` asks which and passes `--cwd`.
- `crates/anthology-dev` (`version_bump.rs`: `release_verdict`, `highest_release`, run through `gate()`), `difftest` (golden replayer), `repo-tests` - never ship.
- `atl/`, `fuzz/` - own workspaces and `Cargo.lock`; ATL's gate: `src/hooks/anthology_gate.rs`.
- `vendor/` - `sources.json` (every pin), `patches/`, `snapshots/`; sync with `scripts/dev.sh vendor sync`.
- `evals/` (runners export `ANTHOLOGY_HOST_MODE=full`), `ci/jobs/` (one script per CI job), `changelog.d/<plugin>/*.md` (fragments `release-edit` reads and deletes).

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` checks the binary against `anthology.lock`, downloads it if missing -> it injects the guide, installs ATL. Embedded hosts get only self-test and bootstrap lines.
- Bash call: `bin/atl-hook`, then `hooks/no-attribution.sh`; the guardrails `guard.sh` -> `anthology guard`. All run with `onFailure: block`, timeout 60. At a prompt, the permissions mod allows a command only when `anthology perm classify` proves it read-only.
- Guard binary missing: a detached `bootstrap.sh fetch` downloads; `guard.sh` lets only the installer its blocks name through.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves `[Unreleased]` entries, commits, tags, then `git push --atomic`; last line `RELEASE OK|FAILED`. Its dev calls share one `anthology-release-*` container (`rust-test.sh session-start|stop`). Release a dependency plugin first; `lock check-fresh` refuses a lock older than the workspace.

## Checks
- Everything: `scripts/rust-test.sh`. One crate: `scripts/rust-test.sh test -p <crate>` (`rust_test_script.rs`).
- CI (`ci.yml`): one job per `ci/jobs/` script (`shellcheck.sh` checks `release.sh`, `rust-test.sh` as bash). On a PR `rust-scope.sh` (rules `ci/scope-rules.tsv`, weights `ci/test-weights.tsv`, graph `rust-graph.mjs`) picks the crates a change reaches, their golden surfaces (`golden_replay::<surface>` tests, `difftest/tests/golden.rs`), atl, atl on macOS, fuzz and macOS; `rust-test-linux`/`-macos` are the required gates over its `-shard` matrices. A push, the `full-ci` label, `[full-ci]` in a commit or a change to the test machinery runs everything. A PR in merge conflict runs no CI: merge `origin/build`.
- `lock-fresh.sh` warns only (exit 0). `fuzz.sh`: `cargo metadata --locked` guards the fuzz lock.
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`. Ledger: `port/ledger/*.tsv` maps each inherited test to a Rust test or a drop reason (`ci/jobs/ledger.sh`).
- Budgets: agent `BUDGET` in `startup_budget.rs`; guide at most 3800 characters, session-start output at most 9000 (`session_start.rs`).

## Cleanup duties
- Workspace version bump: also update `Cargo.lock` and `fuzz/Cargo.lock` (`ci/jobs/fuzz.sh` fails on a stale one). Then follow Binary release.
- Change to `crates/*/src` or `Cargo.toml` of a crate in the `anthology-cli` graph, or `rust-toolchain.toml`: needs a new `anthology-bin--v*` tag and `lock write`, or `release.sh` stops.
- Change under a plugin folder: bump `version` in its `plugin.json` above its latest release tag (a bump already on `build` counts, ADR-036) and add a line under its `[Unreleased]` in `CHANGELOG.md`, or `version-bump` fails.
- Mods: one `modules` entry in `hooks.json`; `index.ts` registers each shared event once, each part in `isolated`. A new `$.state` key goes in `types/anthology.d.ts`.
- Change to the output of `anthology perm classify`: change `classifiedPaths` in `hooks/mods/permissions.ts` too.
- `evals/baseline.json`: record it only from the CI run of `evals.yml`.
- Change to `atl.lock`, `bin/*` or `hooks/*`: release `anthology`. Edit `hooks.json`: update its pins in `crates/repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs` and `crates/anthology-runtime/tests/atl_install.rs`.
- Agent prompts: run `scripts/dev.sh startup-budget`, raise `BUDGET` only with a stated reason. A changed `setup`, session start, doctor or memory sync output: `scripts/dev.sh golden bless --surface X`.
- Docs: `README.md` is the front page (at most 120 lines); detail lives in `docs/*.md`, listed in `DOC_PAGES` in `repo-tests/tests/docs.rs`.
- New crate: a row in `ci/test-weights.tsv`. New golden surface: a line in `golden_replay` (`difftest/tests/golden.rs`). New top-level folder: a row in `ci/scope-rules.tsv`, or CI runs everything for it. `repo-tests/tests/ci_scope.rs` pins all three.
- Add, remove or rewire a piece: update this map in the same commit.
- Memory notes get their own `chore(memory): ...` commit.
