---
name: reference-project-map
description: "How the pieces of this project connect, and the cleanups each kind of change brings. Read before planning or briefing."
metadata:
  node_type: memory
  type: reference
  modified: 2026-11-28T16:55:55.126Z
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
---

## Overview
Three Claude Code plugins and one static Rust binary, `anthology`. ATL (`atl/`, a fork) shortens command output. Plugins are POSIX `sh` plus TypeScript mods; the rest is Rust in `crates/`. Work lands on `build`, gated by CI.

## Pieces
- `plugins/anthology`: agents, `templates/routing-guide.md`, `hooks/`, `bin/` shims, `atl.lock`, `anthology.lock`. All 12 `agents/*.md` share a byte-identical `## Shell` and one US English line, both pinned in `roster.rs`. A new agent needs: `anthology-core` ROSTER (bin release), `repo.rs`, `smoke.rs`, `helpers.rs`, `session_start.rs` AGENTS, `BUDGET`, evals and a guide row.
- `anthology-slop` (regex rules) -> `anthology slop hook|check`. `hooks/slop-gate.sh` (PreToolUse Write/Edit): only exit 10 blocks, and a 5 s deadline allows. Docs: `docs/slop-gate.md`.
- `anthology-scan` -> `anthology check-repo` (`cmd/check_repo.rs`; skill `skills/check-repo`; `docs/check-repo.md`). The scan reads files as data and never runs them.
  - Modules: `tree`/`files`/`scan` (walk, orchestrator); `shape` (command classes); `rules/*`; `yara/` (GuardDog rules vendored at `rules/guarddog/`); `fetch` (quarantine clone; git modes octal); `tools` (osv, gitleaks, zizmor); `report`/`escape` (verdicts, triage cap 200).
  - In `scan.rs`: reach (`Reach::expand`) follows `npm run` scripts, project-folder variables and folders to files; files a command runs get a second pass as code and `raise_reached_file` (payload and hidden caps). In `tree.rs`: a link at a settings path is read as its target. The severity matrix is `shape::severity`.
  - `anthology-runtime/src/check_hint.rs` gives a once-per-repo session-start hint.
- `plugins/anthology/hooks/mods/`: `index.ts`, the one `modules` entry in `hooks.json`. It registers each shared event once, each part in `isolated`, one file per feature.
  - `delegation.ts`: `isEmbeddedHost`; its edit hook refuses the mask marker.
  - `masking.ts` (`session.append`) -> `anthology guard mask` (`anthology-guard/src/mask.rs`).
  - `nudge.ts` (part of `agent.report`) reads the SDD ledger.
  - A new `$.state` key goes in `types/anthology.d.ts`.
- `plugins/anthology-guardrails`: `hooks/guard.sh`, a fail-closed PreToolUse guard. Its `lib.sh`, `bootstrap.sh` and `anthology.lock` are byte-identical to the anthology plugin's: edit both.
- `plugins/anthology-superpowers` and `plugins/anthology/knowledge/**` are generated from `vendor/sources.json` by `anthology-dev vendor` (`drop-paths` omits upstream files), except `FIRST_PARTY_KNOWLEDGE` in `repo.rs`. Never hand-edit them: change `vendor/`, run `scripts/dev.sh vendor check`, bump the plugin. The superpowers host check lives in `vendor/patches/superpowers.patch`.
- `anthology-cli`: the binary. `cmd/perm.rs` and `perm/` give `anthology perm classify`. Build graph: `pycompat` <- `core` <- `settings`, `memory`, `runtime`; `shell` (no workspace inheritance; ATL builds it too) <- `guard` <- the same three.
- `anthology-guard/src/host.rs`: `embedded()` (ADR-037; truth table in `delegation.test.ts`, run by `repo-tests/tests/host_predicate.rs`). `path_vars.rs` holds `ANTHOLOGY_GUARD_PATH_VARS`, with a twin in `anthology-shell/src/path_vars.rs`.
- `anthology-memory`: `save.rs`, `index.rs`, `commit_scan.rs`.
- `anthology-runtime`:
  - `setup.rs`: outside a repo, `memory.repo` lists `repos_below`, and the setup `SKILL.md` passes `--cwd`.
  - `project.rs`: after the guide come the repo's own agents, skills and `MAP_LINE`, and the `.claude/anthology.json` switches (`anthology switch`). Approved repos only.
- Never shipped: `anthology-dev` (`version_bump.rs`), `difftest` (the golden replayer) and `repo-tests`. `atl/` and `fuzz/` have their own workspace and `Cargo.lock`.
- ATL gate: `src/hooks/anthology_gate.rs`, then `read_only_proof.rs`. ATL rewrites only a line that `perm classify` proves read-only (via `ATL_PERM_CLASSIFY`) or that user allow rules cover (`user_allow.rs`). None in auto mode.
- These change together: `BYPASS_NOTE` (`anthology-settings/src/ops.rs`), the setup `SKILL.md`, the runtime setup test.
- `vendor/`: `sources.json` (pins), `patches/`, `snapshots/`. `evals/` runners export `ANTHOLOGY_HOST_MODE=full`. `changelog.d/<plugin>/*.md`: `release-edit` reads and deletes them.

## Flows
- Session start: `hooks/session-start.sh` -> `bin/anthology` shim -> `lib.sh` checks the binary against `anthology.lock` (downloads it if missing) -> guide, ATL install, check-repo hint, a warning for the official `superpowers` (`doctor.rs`).
- Bash call, in order; all are `onFailure: block`, timeout 60:
  1. `bin/atl-hook`;
  2. `hooks/no-attribution.sh`;
  3. `hooks/commit-privacy.sh` -> `anthology privacy hook` (fails open; only exit 10 blocks);
  4. guardrails `guard.sh` -> `anthology guard`. A missing binary is fetched by a detached `bootstrap.sh fetch`.

  The permissions mod allows only what `perm classify` proves read-only.
- Binary release: root `Cargo.toml` version -> tag `anthology-bin--v<ver>` -> `anthology-release.yml` -> `scripts/dev.sh lock write` rewrites both `anthology.lock` -> release guardrails, then anthology.
- ATL release: `atl/Cargo.toml` version `<upstream>-anthology.<n>` -> tag `atl--v<ver>` -> `atl-release.yml` -> `atl.lock` -> release anthology.
- Plugin release: `scripts/release.sh <plugin> <ver>` bumps `plugin.json`, moves `[Unreleased]`, commits, tags and runs `git push --atomic`. Its last line is `RELEASE OK|FAILED`. Release a dependency plugin first. `lock check-fresh` refuses a lock older than the workspace.

## Checks
- All: `scripts/rust-test.sh`; one crate: `... test -p <crate>`. Agents run them on the remote host: `ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <args>`, with a folder and volume per run, flock locks and a slot cap.
  - Images and shared caches are kept.
  - `--clean-ours` removes this checkout's dead runs.
  - `--clean-all-ours [--with-caches]` removes all of ours.
- CI (`ci.yml`): one job per `ci/jobs/` script. On a PR, `rust-scope.sh` (`ci/scope-rules.tsv`, `ci/test-weights.tsv`, `rust-graph.mjs`) picks the reached crates, goldens, atl, fuzz, macOS and repo jobs. gitleaks, deny and shellcheck always run. `rust-test-linux` gates; macOS does not block merges. A push, the `full-ci` label or `[full-ci]` runs all.
  - A PR GitHub calls conflicting runs no CI. The conflict is often a false one from MEMORY.md: merge locally.
- Goldens: `crates/<crate>/tests/golden/*.jsonl.gz`. `port/ledger/*.tsv` maps inherited tests. Budgets: guide ≤4100 characters, session-start ≤9000.
- `repo-tests` guards: `us_english.rs` (UK spelling; `ALLOWED` lists contract strings), `docs.rs` (no real home paths), `licenses`, `no_serde_json`, `ci_scope`.

## Cleanup duties
- Workspace version bump: also `Cargo.lock` and `fuzz/Cargo.lock`. Any `crates/*/src`, `Cargo.toml` or `rust-toolchain.toml` change in the `anthology-cli` graph needs a new bin tag plus `lock write`, or `release.sh` stops.
- Plugin folder change: bump `plugin.json` above its latest tag (ADR-036) and add an `[Unreleased]` line or a `changelog.d` fragment.
- `perm classify` output change: update `classifiedPaths` in `hooks/mods/permissions.ts`. `evals/baseline.json` is recorded only from CI `evals.yml`.
- `atl.lock`, `bin/*` or `hooks/*` change: release `anthology`. A `hooks.json` edit means updating the pins in `repo-tests/tests/shims/statics.rs`, `cutover_hooks.rs` and `anthology-runtime/tests/atl_install.rs`.
- Agent prompts: run `scripts/dev.sh startup-budget`, and raise `BUDGET` only with a reason. Setup, session start, doctor or memory output changes: `scripts/dev.sh golden bless --surface X`.
- Docs: `README.md` ≤120 lines; detail goes in `docs/*.md` (`DOC_PAGES`, `docs.rs`).
- New crate: a row in `ci/test-weights.tsv`. New golden surface: `golden_replay`. New top-level folder: a row in `ci/scope-rules.tsv`. `ci_scope.rs` pins all three.
- Adding, removing or rewiring a piece: put the map line in the PR body, and the lead applies it. Memory notes get their own `chore(memory): ...` commit.
