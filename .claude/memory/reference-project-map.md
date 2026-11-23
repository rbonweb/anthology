---
name: reference-project-map
description: How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing.
metadata:
  type: reference
---

## Overview
Anthology is a Claude Code marketplace of three plugins backed by one static Rust binary, `anthology`. ATL (Anthology Token Layer), a fork in `atl/`, shortens command output. Plugins are POSIX `sh` plus TypeScript mods; all else is Rust in `crates/`. Work lands on `build`; tests run in the pinned Docker image; CI is the gate.

## Pieces
- `plugins/anthology` - agents, `templates/routing-guide.md`, `hooks/` and `bin/` shims (`anthology`, `anthology-mem`, `atl`, `atl-hook`), `atl.lock`, `anthology.lock`. `plugin.json` ranges depend on the other two plugins.
- `plugins/anthology/hooks/mods/` - Claude Code mods: `index.ts` (the one module `hooks.json` names under `"modules"`), `util.ts`, one file per feature. Tests `plugins/anthology/tests/mods/`, state keys `plugins/anthology/types/anthology.d.ts`, CI `ci/jobs/mods.sh`.
- `plugins/anthology-guardrails` - `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh` and `anthology.lock` are byte-identical to the anthology plugin's.
- `plugins/anthology-superpowers`, `plugins/anthology/knowledge/**` - generated from `vendor/sources.json` by `anthology-dev vendor`. Never hand-edit.
- `crates/anthology-cli` - the `anthology` binary; `src/cmd/perm.rs` and `perm/` are `anthology perm classify`, run by the permissions mod. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance, ATL builds it too) <- `guard` <- `memory`, `settings`, `runtime`.
- `crates/anthology-dev` (maintainer tools, `scripts/dev.sh`), `difftest` (golden replayer), `anthology-testkit`, `repo-tests` (repo, shim, docs checks) - never ship.
- `atl/` - subtree fork with its own workspace, `Cargo.lock` and toolchain pin; `src/hooks/anthology_gate.rs` is the gate; `scripts/atl-trim.sh` drops non-Apache parts.
- `fuzz/` - own workspace and `Cargo.lock`, outside the root workspace.
- `vendor/` - `sources.json` (every pin, ATL upstream too), `patches/`, `snapshots/`, `licenses/`. Sync: `scripts/dev.sh vendor sync <name>`, `scripts/sync-atl.sh <tag>`.
- `port/ledger/*.tsv` - every inherited test maps to a Rust test or a drop reason. `port/difftest/`: divergences, seeds.
- `evals/` (cases, `mocks/`, `baseline.json`), `ci/` (pinned images and Claude Code, one script per job in `ci/jobs/`).

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` checks the binary against `anthology.lock` (sha256), downloads it if missing -> it injects the guide, installs ATL per `atl.lock`.
- Bash call: `bin/atl-hook`, then `hooks/no-attribution.sh`; the guardrails `guard.sh` -> `anthology guard`. All run with `onFailure: block`, timeout 60. At a prompt, the permissions mod sends the command to `anthology perm classify` and allows it only when proven read-only.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` (the lock PR) -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves `[Unreleased]` entries, commits, runs `claude plugin tag`. Release a dependency plugin first; it checks dependents' ranges and refuses a lock older than the workspace (`lock check-fresh`).

## Checks
- Everything: `scripts/rust-test.sh` (fmt, clippy, `cargo test --workspace --locked`). One crate: `scripts/rust-test.sh test -p repo-tests`. ATL: `scripts/atl-test.sh`. Mods: `ci/jobs/mods.sh`.
- CI (`.github/workflows/ci.yml`): rust-lint, rust-test-linux and -macos, deny, shellcheck, shims-distros, ledger, fuzz-build, atl-test, vendor-check, version-bump, plugin-validate, mods, gitleaks, startup-budget, cli-syntax, memory-diff. Other workflows: `evals`, `fuzz`, `upstream-watch`, two releases. A PR in merge conflict runs no CI.
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`, replayed by `the_committed_goldens_replay_clean`. Ledger: `ci/jobs/ledger.sh`.
- Budgets: agent `BUDGET` in `crates/anthology-dev/src/startup_budget.rs`; guide at most 3800 characters, session-start output at most 9000 (`anthology-runtime/tests/session_start.rs`).
- `hooks.json` shape is pinned in `crates/repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs` and `crates/anthology-runtime/tests/atl_install.rs`.

## Cleanup duties
- Workspace version bump: also update `Cargo.lock` and `fuzz/Cargo.lock` (PR CI skips fuzz; `ci/jobs/fuzz.sh` fails on a stale one). Then follow Binary release.
- Change to `crates/*/src` or `Cargo.toml` of a crate in the `anthology-cli` graph, or `rust-toolchain.toml`: needs a new `anthology-bin--v*` tag and `lock write`, or `release.sh` stops.
- Change under a plugin folder: bump `version` in its `plugin.json` and add a line under its `[Unreleased]` in `CHANGELOG.md`, or `version-bump` fails. A lock PR touches both plugins, so it bumps both above the base branch's version, even if that is unreleased.
- PR in conflict with `build`: merge `origin/build` and push; CI starts only then.
- New `$.state` key in a mod: declare it in `types/anthology.d.ts` (one `PluginState.anthology` object), or `validate` fails.
- Mods have one `modules` entry in `hooks.json`; a second fails the file. `index.ts` registers each shared event once, each part in `isolated`. Every hook keeps its `.catch`.
- Change to the output of `anthology perm classify` (token line, path lines): change `classifiedPaths` in `hooks/mods/permissions.ts` too.
- `evals/baseline.json`: record it only from the CI run of `evals.yml` (its candidate artifact), never from a laptop.
- The old name `rtk` fails `atl_rename.rs`, except in dated notes under `.claude/agent-memory/` and `.claude/memory/`.
- Edit `lib.sh`, `bootstrap.sh` or a lock in one of the two shim plugins: make the same edit in the other.
- Change to `atl.lock`, `bin/*` or `hooks/*`: release `anthology`. Edit `hooks.json`: update the three pinned tests above.
- Agent prompts: run `scripts/dev.sh startup-budget`, raise `BUDGET` only with a stated reason, re-bless `dev.startup-budget`.
- Output of `setup`, session start, doctor or memory sync: `scripts/dev.sh golden bless --surface X`, reason in the PR.
- Ported tests keep their Python names. Add, drop or rename one only with its `port/ledger/*.tsv` row.
- Vendored trees: change the patch or transform in `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin.
- Add or move a doc: `docs.rs` and `cutover_docs.rs` in `repo-tests` check it (the word "Python" is allowed only in listed places).
- Add, remove or rewire a piece: update this map in the same commit.
- Memory notes get their own `chore(memory): ...` commit.
