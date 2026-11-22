---
name: reference-project-map
description: How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing.
metadata:
  type: reference
---

## Overview
Anthology is a Claude Code marketplace of three plugins backed by one static Rust binary, `anthology`. ATL (Anthology Token Layer), a fork in `atl/`, shortens command output. The plugin side is POSIX `sh` only; all other code is Rust in `crates/`. Work lands on `build`; tests run in the pinned Docker image and CI is the gate.

## Pieces
- `plugins/anthology` - agents, `templates/routing-guide.md`, `hooks/` and `bin/` shims (`anthology`, `anthology-mem`, `atl`, `atl-hook`), `atl.lock`, `anthology.lock`. `plugin.json` ranges depend on the other two plugins.
- `plugins/anthology-guardrails` - `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh` and `anthology.lock` are byte-identical to the anthology plugin's.
- `plugins/anthology-superpowers`, `plugins/anthology/knowledge/**` - generated from `vendor/sources.json` by `anthology-dev vendor`. Never hand-edit.
- `crates/anthology-cli` - the `anthology` binary. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance, ATL builds it too) <- `guard` <- `memory`, `settings`, `runtime`.
- `crates/anthology-dev` (maintainer tools, run by `scripts/dev.sh`), `difftest` (golden replayer), `anthology-testkit`, `repo-tests` (repo, shim and docs checks) - never ship.
- `atl/` - subtree fork with its own workspace, `Cargo.lock` and toolchain pin. `atl/src/hooks/anthology_gate.rs` is the gate. `scripts/atl-trim.sh` removes the non-Apache parts.
- `fuzz/` - own workspace and `Cargo.lock`, outside the root workspace.
- `vendor/` - `sources.json` (every pin, including the ATL upstream), `patches/`, `snapshots/`, `licenses/`.
- `port/ledger/*.tsv` - the port ledger: every inherited test maps to a Rust test or a drop reason. `port/difftest/` holds divergences and seeds.
- `evals/` (cases, `mocks/`), `ci/` (pinned images, pinned Claude Code, one script per job in `ci/jobs/`).
- Mods (Claude Code hook modules in `plugins/anthology/hooks/mods/`, named by `"modules"` in `hooks.json`; tests in `plugins/anthology/tests/mods/`; `ci/jobs/mods.sh`) are in open PRs, not yet on `build`.

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` verifies the binary against `anthology.lock` (sha256), downloads it if missing -> binary injects the guide and installs ATL per `atl.lock`.
- Bash call: `bin/atl-hook`, then `hooks/no-attribution.sh`; the guardrails `guard.sh` -> `anthology guard`. All run with `onFailure: block`, timeout 60.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves CHANGELOG `[Unreleased]` entries, commits, runs `claude plugin tag` (`<plugin>--v<ver>`). Release a dependency plugin first; `release.sh` checks that dependents' ranges accept the version.
- Upstream sync: `scripts/dev.sh vendor sync <name>`; ATL: `scripts/sync-atl.sh <tag>`.

## Checks
- Everything: `scripts/rust-test.sh` (fmt, clippy, `cargo test --workspace --locked`). One crate: `scripts/rust-test.sh test -p repo-tests`. ATL: `scripts/atl-test.sh`, `scripts/atl-net-test.sh`.
- CI (`.github/workflows/ci.yml`): rust-lint, rust-test-linux and -macos, deny, shellcheck, shims-distros, ledger, fuzz-build, atl-test, vendor-check, version-bump, plugin-validate, gitleaks, startup-budget, cli-syntax; memory-diff only lists memory changes. Separate workflows: `evals.yml`, `fuzz.yml`, `upstream-watch.yml`, `atl-release.yml`, `anthology-release.yml`.
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`, replayed by the difftest test `the_committed_goldens_replay_clean`.
- Ledger: `ci/jobs/ledger.sh` checks every row against the tests cargo lists.
- Budgets: agent `BUDGET` in `crates/anthology-dev/src/startup_budget.rs`; guide at most 3800 characters and session-start output at most 9000 (`crates/anthology-runtime/tests/session_start.rs`).
- `hooks.json` shape is pinned in `crates/repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs` and `crates/anthology-runtime/tests/atl_install.rs`.

## Cleanup duties
- Workspace version bump: also update `Cargo.lock` and `fuzz/Cargo.lock` (CI skips fuzz on PRs; `ci/jobs/fuzz.sh` fails on a stale lock). Then follow Binary release.
- Change to `crates/*/src` or `Cargo.toml` of a crate in the `anthology-cli` graph, or `rust-toolchain.toml`: needs a new `anthology-bin--v*` tag and `lock write`, or `release.sh` stops on `lock check-fresh`.
- Change under a plugin folder: bump `version` in its `.claude-plugin/plugin.json` and add a line under that plugin's `[Unreleased]` in `CHANGELOG.md`. `version-bump` fails otherwise.
- Edit `lib.sh`, `bootstrap.sh` or a lock in one of the two shim plugins: make the same edit in the other.
- Change to `atl.lock`, `bin/*` or `hooks/*`: release `anthology`. Edit `hooks.json`: update the three pinned tests above.
- Change to agent prompts: run `scripts/dev.sh startup-budget`, raise `BUDGET` only with a stated reason, re-bless `dev.startup-budget`.
- Change to output of `setup`, session start, doctor or memory sync: re-bless that surface.
- Golden surfaces change only on purpose: `scripts/dev.sh golden bless --surface X`, with the reason in the PR.
- Ported tests keep their Python names (`port/ledger/*.tsv`). Add, drop or rename one only with its ledger row.
- Vendored trees: change the patch or transform in `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin.
- Add or move a doc: `repo-tests` `docs.rs` and `cutover_docs.rs` check them (the word "Python" is allowed only in listed places).
- Add, remove or rewire a piece: update this map in the same commit.
- Docker runs: remove your own containers, images and volumes.
- Memory notes get their own `chore(memory): ...` commit.
