---
name: reference-project-map
description: How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing.
metadata:
  type: reference
---

## Overview
Anthology is a Claude Code marketplace of three plugins backed by one static Rust binary, `anthology`. ATL (Anthology Token Layer), a fork in `atl/`, shortens command output. Plugins are POSIX `sh` plus TypeScript mods; all else is Rust in `crates/`. Work lands on `build`; tests run in Docker; CI is the gate.

## Pieces
- `plugins/anthology` - agents, `templates/routing-guide.md`, `hooks/` and `bin/` shims (`anthology`, `anthology-mem`, `atl`, `atl-hook`), `atl.lock`, `anthology.lock`. All 11 `agents/*.md` share a byte-identical `## Shell` section (`every_agent_has_the_same_shell_section`, `roster.rs`).
- `plugins/anthology/hooks/mods/` - Claude Code mods: `index.ts` (the one module `hooks.json` names under `"modules"`), `util.ts`, one file per feature. State keys `plugins/anthology/types/anthology.d.ts`. `delegation.ts`: `isEmbeddedHost` (ADR-037) skips the refusal and the coordinator section.
- `plugins/anthology-guardrails` - `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh` and `anthology.lock` are byte-identical to the anthology plugin's: edit one, edit both.
- `plugins/anthology-superpowers`, `plugins/anthology/knowledge/**` - generated from `vendor/sources.json` by `anthology-dev vendor`. Never hand-edit: change `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin. The `hooks/session-start` host check (ADR-037) comes from `vendor/patches/superpowers.patch`.
- `crates/anthology-cli` - the `anthology` binary; `src/cmd/perm.rs` and `perm/` are `anthology perm classify`, run by the permissions mod. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance, ATL builds it too) <- `guard` <- `memory`, `settings`, `runtime`.
- `anthology-guard/src/host.rs`: `embedded(entrypoint, attended, mode)`, `embedded_from_env()` (ADR-037; read by `cmd/guard.rs` and session start). `check.rs`: `check_explained` gives the verdict plus a G0 cause mark; `hook.rs` builds the G0 hints and `decide_for`.
- `anthology-memory`: `save.rs` (`save_repo_note_with`, `reindex`, `ReindexTarget`, `write_note`), `index.rs` (`run_reindex`). `anthology-runtime/src/setup.rs`: `memory.repo` gets `repos_below[_more]` outside a repo; setup `SKILL.md` asks which and passes `--cwd '<abs repo path>'`.
- `crates/anthology-dev` (`scripts/dev.sh`; `version_bump.rs`: `release_verdict`, `highest_release`, run through `gate()`), `difftest` (golden replayer), `repo-tests` - never ship.
- `atl/`, `fuzz/` - own workspaces with their own `Cargo.lock`; ATL's gate is `src/hooks/anthology_gate.rs`.
- `vendor/` - `sources.json` (every pin), `patches/`, `snapshots/`; sync with `scripts/dev.sh vendor sync`.
- `evals/` (`scripts/run-evals.sh` and the smoke runner's `build_env` export `ANTHOLOGY_HOST_MODE=full`), `ci/` (pinned images, one script per job in `ci/jobs/`), `changelog.d/<plugin>/*.md` (fragments that `release-edit changelog-check|changelog-move` read and delete).

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` checks the binary against `anthology.lock`, downloads it if missing -> it injects the guide, installs ATL. An embedded host gets only the self-test and bootstrap lines; the nag shows only at `startup`; compact and resume warn on a stale loaded anthology plugin.
- Bash call: `bin/atl-hook`, then `hooks/no-attribution.sh`; the guardrails `guard.sh` -> `anthology guard`. All run with `onFailure: block`, timeout 60. At a prompt, the permissions mod sends the command to `anthology perm classify` and allows it only when proven read-only.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves `[Unreleased]` entries, commits, tags, then `git push --atomic`; last line `RELEASE OK|FAILED`. Release a dependency plugin first; `lock check-fresh` refuses a lock older than the workspace.

## Checks
- Everything: `scripts/rust-test.sh`. One crate: `scripts/rust-test.sh test -p <crate>` (`rust_test_script.rs`). ATL: `scripts/atl-test.sh`.
- CI (`.github/workflows/ci.yml`): one job per `ci/jobs/` script (`shellcheck.sh` covers `release.sh`). Other workflows: `evals`, `fuzz`, `upstream-watch`, releases. A PR in merge conflict runs no CI: merge `origin/build` and push.
- `lock-fresh.sh`: `dev lock check-fresh`, always exits 0, warns only (`ANTHOLOGY_DEV_SH` replaces dev.sh in tests). `fuzz.sh`: `cargo metadata --locked` guards the fuzz lock (`fuzz_build_is_locked`, `repo-tests/tests/ci.rs`).
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`. Ledger: `ci/jobs/ledger.sh`; `port/ledger/*.tsv` maps each inherited test to a Rust test or a drop reason; a change needs its row.
- Budgets: agent `BUDGET` in `startup_budget.rs`; guide at most 3800 characters, session-start output at most 9000 (`anthology-runtime/tests/session_start.rs`).

## Cleanup duties
- Workspace version bump: also update `Cargo.lock` and `fuzz/Cargo.lock` (`ci/jobs/fuzz.sh` fails on a stale one). Then follow Binary release.
- Change to `crates/*/src` or `Cargo.toml` of a crate in the `anthology-cli` graph, or `rust-toolchain.toml`: needs a new `anthology-bin--v*` tag and `lock write`, or `release.sh` stops.
- Change under a plugin folder: bump `version` in its `plugin.json` above its latest release tag (a bump already on `build` counts, ADR-036) and add a line under its `[Unreleased]` in `CHANGELOG.md`, or `version-bump` fails.
- Mods: one `modules` entry in `hooks.json`; `index.ts` registers each shared event once, each part in `isolated`; every hook keeps its `.catch`. A new `$.state` key goes in `types/anthology.d.ts`.
- Change to the output of `anthology perm classify`: change `classifiedPaths` in `hooks/mods/permissions.ts` too.
- `evals/baseline.json`: record it only from the CI run of `evals.yml`, never from a laptop.
- Change to `atl.lock`, `bin/*` or `hooks/*`: release `anthology`. Edit `hooks.json`: update its pins in `crates/repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs` and `crates/anthology-runtime/tests/atl_install.rs`.
- Agent prompts: run `scripts/dev.sh startup-budget`, raise `BUDGET` only with a stated reason. A changed `setup`, session start, doctor or memory sync output: `scripts/dev.sh golden bless --surface X`.
- Add, remove or rewire a piece: update this map in the same commit.
- Memory notes get their own `chore(memory): ...` commit.
