# Contributing

This file has two parts: how to work on the repo, and a runbook for maintainers.

For what the product is and why it is built this way, read the [README](README.md) and [DECISIONS.md](DECISIONS.md). Report security problems privately, as described in [SECURITY.md](SECURITY.md).

## Ground rules

- **The repo is public.** Never commit a secret, a token, a `.env` file, an absolute home path (use `~/.claude/...`), a machine name or an email address. A test and gitleaks check this.
- **No options.** Anthology has no user-facing modes, levels or toggles. Do not add any.
- **Vendored content is generated.** `plugins/anthology/knowledge/**` and `plugins/anthology-superpowers/**` are produced by [`scripts/vendor.py`](scripts/vendor.py) from pinned upstream commits. Never edit them by hand: change the patch or the transform and regenerate. CI regenerates everything and fails on any difference.
- **BrowserOS Neo is the only browser.** Nothing we ship recommends another browser or launches Chrome, and nothing installs browser tooling on the host.
- **Runtime scripts use only the Python standard library** and run on Python 3.9 or newer (macOS ships 3.9 as `/usr/bin/python3`). Start each module with `from __future__ import annotations`, and do not use `match` or 3.10+ APIs.
- **Every change to a plugin bumps that plugin's version.** CI fails when a released plugin's folder changes with no higher `version` in its `.claude-plugin/plugin.json`. Add a line under that plugin's `[Unreleased]` heading in [CHANGELOG.md](CHANGELOG.md) too.
- **Memory changes get their own commits,** `chore(memory): …`.
- **Models are aliases** (`opus`, `sonnet`, `haiku`), never version-pinned ids.

## Repository layout

| Path | What it holds |
|---|---|
| `.claude-plugin/marketplace.json` | The marketplace `anthology` and its three plugins |
| `plugins/anthology/` | Agents, generated knowledge, the three skills (`setup`, `export`, `import`), hooks, `bin/` wrappers, `rtk.lock`, the routing guide, the Python scripts |
| `plugins/anthology-guardrails/` | The `PreToolUse` hook (`guard.py`, `guard_rules.py`) |
| `plugins/anthology-superpowers/` | Generated from `vendor/`, renamed from superpowers |
| `vendor/` | `sources.json` (every pin), `patches/`, `snapshots/`, `licenses/` |
| `rtk/` | The rtk source, a git subtree with its history |
| `scripts/` | Maintainer tools: `vendor.py`, `sync-rtk.sh`, `rtk-trim.sh`, `rtk-test.sh`, `rtk-net-test.sh`, `release.sh`, `check_version_bump.py`, `upstream_watch.py`, `memory_diff.py`, `run-evals.sh`, `eval_baseline.py` |
| `evals/` | Behaviour evals (one `case.yaml` per case), the mocked BrowserOS Neo server in `mocks/`, and the smoke cases in `smoke/`; run with `scripts/run-evals.sh` |
| `tests/` | Unit tests (`unittest`) |
| `ci/` | Pinned Docker images and the pinned Claude Code used by CI |
| `.github/workflows/` | `ci.yml`, `evals.yml`, `upstream-watch.yml`, `rtk-release.yml` |

`skills/` holds only the three user commands. Every knowledge folder must be read by at least one agent, and a test enforces it.

## Development setup

You need git, Docker, Python 3 and Node 22 or newer. You do not need Rust, Chrome or any browser tooling on the host: tests run in pinned Docker images.

1. Clone the repo and work on a branch.
2. Run the tests from the repo root, in the pinned image, the way CI does:

   ```sh
   docker run --rm -e PYTHONDONTWRITEBYTECODE=1 -v "$PWD":/w -w /w "$(awk '$1 == "FROM" {print $2}' ci/images/python.Dockerfile)" \
     sh -c "pip install -q --require-hashes -r tests/requirements.txt && python -m unittest discover -s tests -v"
   ```

   `PYTHONDONTWRITEBYTECODE=1` keeps the container, which runs as root, from leaving root-owned `__pycache__` folders in your checkout. Do not add `-u`: a guard test needs the running user to exist in the container's passwd file.

   Then again with `ci/images/python39.Dockerfile`, because the runtime scripts must work on Python 3.9.

3. Some tests need `git`, `node` or network access, and skip themselves in those images. CI runs them on the runner. To run one file on your machine, install the test requirements in a virtual environment and run, for example, `python -m unittest tests.test_release -v`.
4. Run the rtk tests with [`scripts/rtk-test.sh`](scripts/rtk-test.sh). It reads the Rust image from `ci/images/rust.Dockerfile` and runs `cargo test --locked` in a container as a non-root user; extra arguments go to `cargo test`. It needs Docker and network access, because it installs a few test-only tools (`git`, `curl`, `jq`, `procps`, `ca-certificates`) inside the container only. [`scripts/rtk-net-test.sh`](scripts/rtk-net-test.sh) builds the release binary in the same image and runs it with `docker run --network none`.
5. Check the vendored content with `python3 scripts/vendor.py check`.
6. Validate a plugin with the Claude Code version CI uses: `npm ci --prefix ci/claude-code`, then `ci/claude-code/node_modules/.bin/claude plugin validate --strict plugins/<name>`.
7. Try your checkout in Claude Code: `claude plugin marketplace add <path to your checkout>` loads the marketplace in place, then install `anthology@anthology` as in the README.

### What CI runs

| Check | What it proves |
|---|---|
| `test` and `test-py39` | The unit tests on the pinned Python 3.12 and 3.9 images, including the per-agent startup budgets |
| `memory-git`, `setup-git`, `version-bump`, `taste-bridge`, `vendor-check` | The tests that need `git`, `node` or network, which skip in the Docker images and run on the runner (memory, session-start and setup, release and version-bump, the BrowserOS Neo bridge, vendoring) |
| `rtk-test` job (`scripts/rtk-test.sh`, then `scripts/rtk-net-test.sh`) | rtk passes its tests, including one named test per change the fork makes, and the release binary works with the network off |
| `rtk-release` workflow, on a pull request | A change to the workflow or the toolchain pin builds and tests all four release targets, without publishing |
| `vendor.py check` | Every vendored tree and license regenerates exactly from its pin |
| `check_version_bump.py` | A changed plugin has a higher version than at the merge base |
| `claude plugin validate --strict` | Each plugin is valid, using the pinned Claude Code in `ci/claude-code/` |
| gitleaks | No secrets in the history, using `.gitleaks.toml` |
| `memory-diff` job (`scripts/memory_diff.py`, pull requests) | Lists changed files under `.claude/memory/` and `.claude/agent-memory/` in the job summary, so a memory note is reviewed like code |
| `startup-budget` job | The per-agent startup budgets and the session-start output budget (9,000 characters) hold |
| `cli-syntax` job | The `claude plugin` subcommands and flags the plugins and evals rely on exist in the pinned Claude Code. No model is called |

Images are pinned by digest in `ci/images/*.Dockerfile`, test dependencies by hash in `tests/requirements.txt`, and GitHub Actions by commit SHA. Dependabot proposes the bumps, and CI and the evals gate them.

### Pull requests

- Say what changed and why.
- Bump the plugin version, and add a CHANGELOG entry under `[Unreleased]`, for every plugin you touched.
- Add or update tests. For a bug, write the failing test first.
- If a test fails on a startup budget, read [Startup budgets](#startup-budgets) before touching it.

## Maintainer runbook

### Syncing a vendored source

Every upstream is pinned in [`vendor/sources.json`](vendor/sources.json): repo, path, ref, commit, SPDX license, target folder and transform. Anthology's own changes to upstream material are `vendor/patches/<name>.patch` files or named transforms. Only MIT and Apache-2.0 sources are accepted; anything else needs the owner's decision.

1. Work on a branch.
2. Move the pin:

   ```sh
   python3 scripts/vendor.py sync <name> --ref <new tag, branch or commit>
   ```

   `sync` re-pins the entry, rebuilds its target, refreshes its license file, and never commits. It exits 1 and prints `REJECT` lines when a patch no longer applies.
3. If a patch was rejected, update `vendor/patches/<name>.patch` for the new upstream and run `python3 scripts/vendor.py build <name>`. `python3 scripts/vendor.py diff <name>` prints the difference between pristine-plus-transforms and the shipped tree as a patch, which is a starting point.
4. Run `python3 scripts/vendor.py check`. It must be clean.
5. Review the diff. If the output changed, bump the plugin version it belongs to. For `anthology-superpowers`, which is versioned independently of upstream superpowers, bump `plugin_version` in the `superpowers` entry of `sources.json` (and `upstream_version` is updated by `sync`).
6. Run the tests. A new knowledge file can push an agent over its startup budget (see below).
7. Open a PR. CI and the evals are the gate.

Notes for particular sources:

- **`design-taste-frontend`** has a machine-readable port map, `vendor/patches/design-taste-frontend.portmap.json`, that lists each upstream script as keep, port or drop. `vendor.py` fails when upstream adds a script the map does not list, and when any shipped file contains a Chrome launch. When that happens, decide for the new script and add it to the map.
- **`ui-taste`** has no git upstream. It is pinned by the sha256 of its `CHECKSUMS.sha256` manifest, and a pristine copy lives in `vendor/snapshots/ui-taste/`. To move it, set `checksum` in `sources.json`, then run `python3 scripts/vendor.py fetch ui-taste && python3 scripts/vendor.py build ui-taste`.
- **`vendor/licenses/`** carries each source's license text, unchanged, refreshed by `fetch` and `sync`.

### Syncing rtk

rtk lives in `rtk/` as a git subtree with full history: the owner's fork merged with upstream. Both pins are in the `rtk` entry of `sources.json`. [`scripts/sync-rtk.sh`](scripts/sync-rtk.sh) moves to a new upstream release:

```sh
scripts/sync-rtk.sh <upstream tag>
```

Give it an upstream release tag like `v1.2.3`. It refuses to run on a dirty tree. In order, it:

1. creates the branch `rtk-sync/<tag>` from `HEAD`;
2. runs `git subtree pull` for that tag. Upstream edits to files the trim deletes (`engine/` and similar) are resolved by deleting them again. On any other conflict it prints the conflicted files and **stops**, leaving the branch for you to resolve;
3. records the tag and its commit as the upstream pin in `vendor/sources.json`;
4. runs `scripts/rtk-trim.sh`;
5. runs `scripts/rtk-test.sh`;
6. prints a summary.

The pin and the trim's changes are left uncommitted, for review with the branch. It never pushes and never merges.

What you do:

1. If it stopped on conflicts, resolve them so that both upstream's change and the fork's intent survive. If a fork change cannot coexist with upstream, upstream's design wins and the intent is re-implemented where feasible. Record the decision in `DECISIONS.md`. Then re-run the remaining steps by hand (the pin, `scripts/rtk-trim.sh`, `scripts/rtk-test.sh`).
2. `rtk-trim.sh` removes upstream's BUSL-1.1 `engine/`, `plugin/` and `proxy/`, so that what ships is the Apache-2.0 edition, and keeps rtk Claude Code only and free of telemetry. It is idempotent and stops with exit 1, naming the file and the anchor, when an edit's expected text is missing, which means an upstream refactor to look at. A removal whose target upstream renamed is not noticed that way; the allowlist tests below catch it. The allowlist test `test_rtk_claude_only` fails when upstream adds a hook integration module or a Cargo dependency that is not on the list. Adding one is a deliberate decision, not a test fix.
3. Each of the fork's eight remaining changes has a permanent named test (`fork_1_…` to `fork_7_…` and `fork_9_…`). They must still pass. The ninth, custom `[upstream.*]` headers, lived only in the removed BUSL engine, so its test is gone.
4. Review and commit the pin and the trim's changes, run the unit tests (as in the development setup), then open a PR.

#### Releasing rtk binaries

1. Set the `rtk/Cargo.toml` version to `<upstream version>-anthology.<n>` (for example `1.2.1-anthology.2`), update the `rtk` package entry in `rtk/Cargo.lock` to match (the builds use `--locked`), and merge that.
2. Push a tag `rtk--v<version>` on the commit to release. `rtk-release.yml` fails unless the tag equals `rtk--v` plus the `Cargo.toml` version. It builds `aarch64` and `x86_64` for macOS (`apple-darwin`) and Linux (`unknown-linux-musl`), each on a native runner, with the toolchain in `rtk/rust-toolchain.toml`, running `cargo test --locked` before each `cargo build --release --locked`. It attests the archives and publishes `rtk-<target>.tar.gz` and `checksums.txt` to a new release; it fails rather than overwrite an existing release. To publish from an existing tag, run the workflow by hand with the tag as input.
3. Write the version, the tag and the sha256 of each target from `checksums.txt` into `plugins/anthology/rtk.lock`. `tests/test_rtk_lock.py` checks that the lock matches `Cargo.toml` and has a 64-hex hash for all four targets.
4. Verify one asset: `gh attestation verify <file> --repo rbonweb/anthology`, and compare its hash.
5. The lock change is a change to `plugins/anthology`, so release a new `anthology` version (below). Installed users get the new binary at their next session start.

### Releasing a plugin

Each plugin is versioned on its own (semver). A release is a commit plus a tag `<plugin>--v<version>`, made by [`scripts/release.sh`](scripts/release.sh):

```sh
scripts/release.sh <plugin> <version> --dry-run   # look first
scripts/release.sh <plugin> <version>             # commit and tag locally
scripts/release.sh <plugin> <version> --push      # also push the branch and the tag
                                                  # (--remote NAME picks the remote; default origin)
```

It refuses to run (and changes nothing) unless:

- the working tree is clean;
- `<version>` is higher than the current one, or equal to it when that tag does not exist yet (a first release);
- the tag does not exist;
- the plugin's `[Unreleased]` section in `CHANGELOG.md` has content;
- every other plugin that depends on this one has a version range that accepts `<version>`. If one does not, widen the range in that plugin's `plugin.json` first, which is its own change with its own bump.

Then it bumps `plugin.json` (and `plugin_version` in `vendor/sources.json` for `anthology-superpowers`), moves the `[Unreleased]` entries into a dated version section, commits as `release: <plugin> <version>`, and runs `claude plugin tag`. It never pushes without `--push`; without it, it prints the two `git push` commands to publish. `--dry-run` does all of this in a throwaway worktree, shows the commit and the CHANGELOG diff, and leaves your checkout, index and tags untouched.

A plugin's `version` is what moves installed users to new code, so a change that is not released does not reach them. Release a dependency before the plugin that needs its new version.

### Evals and the baseline

The behaviour evals in [`evals/`](evals/) catch a model update or a Claude Code update that silently breaks the product. Each case is `evals/<case>/case.yaml`. They sit at the repo root, because a case can only load plugins inside its containment root. Each case lists all three plugins as `../../plugins/anthology`, `../../plugins/anthology-guardrails` and `../../plugins/anthology-superpowers`. They cover:

- routing (the main session delegates to the right `anthology:*` agent);
- knowledge-read order (each agent reads its Startup files before any other tool);
- the G1, G4 and G6 blocks;
- BrowserOS-only browsing, against a mocked `browseros-neo` MCP server in `evals/mocks/` and with no Chrome launch;
- rtk condensing `git status` output, and G1 still blocking a Playwright command that rtk would rewrite;
- the routing guide being in the main session, and absent from agents;
- the platform checks recorded in [DECISIONS.md](DECISIONS.md) that a case can express (agent fields, memory paths, hooks reaching subagents, skills hidden from the main session).

The checks a case cannot express are smoke cases in `evals/smoke/smoke.py`: the Claude Code CLI syntax, the eval cost ceiling, the session-start output cap, a missing hook interpreter and a missing `python3`, the worktree base, the model each agent runs on, and the knowledge-read, auto-memory-directory and memory-commit checks for writers. `cli-syntax` and `python-missing` call no model.

Run everything from the repo root with `scripts/run-evals.sh`. It runs the evals, then the smoke cases. Under the hood it calls one `claude plugin eval` with a fixed `--allow-tools` list:

```sh
claude plugin eval . --trust-plugin --ablation none --no-publish --threshold 0 --runs 1 \
  --max-cost-usd <ceiling> --json <result.json> --output-dir <dir> --allow-tools <grants>
```

A tool that is not granted is withheld from the model. The grants are the few harmless commands the guard and rtk cases need (listed in the script), so a broken guard still cannot download anything; the guard and rtk cases only work through the script, so don't run the bare command and read its failures as regressions. `--case GLOB` and `--tag TAG` run part of the suite (the comparison then tolerates missing cases, the smokes are skipped and `--record` is refused); `--no-smoke` skips the smokes. Results go to `evals/results/<UTC timestamp>/`.

The cost ceiling defaults to $20 for the evals (`--max-cost-usd`) plus $10 for all smokes together (`--smoke-total-budget-usd`). When the evals hit their ceiling the run is partial, and the comparison fails it with exit code 2, so raise the ceiling deliberately. The exit codes are the comparison's when it is not 0 (1 regression, 2 invalid or partial result, 3 no baseline), else 4 when a smoke failed, else 0. A smoke that skips because there are no credentials is not a failure.

**The baseline** is `evals/baseline.json`, the per-case scores of a full, non-partial run. The committed one is recorded from the CI run, so that the baseline and every later comparison come from the same environment: dispatch `evals.yml` once with no baseline committed, download its `evals-results` artifact and commit `candidate-baseline.json` as `evals/baseline.json` with the `candidate` key removed. `scripts/run-evals.sh --record` writes a baseline from a local full run, for experiments.

[`evals.yml`](.github/workflows/evals.yml) runs weekly, on demand, and when the upstream watcher sees a new Claude Code release. It installs the pinned Claude Code from the `ci/claude-code` lockfile and needs the `ANTHROPIC_API_KEY` repository secret (it skips with a notice while that secret is missing). It has no pull-request or push trigger, so it never runs for a fork; to evaluate a Claude Code bump, dispatch it on the Dependabot branch. The run fails when:

- a baseline case scores below its baseline score;
- a baseline case is missing from the run;
- the run is partial (the cost ceiling was hit) or the result is invalid;
- `evals/baseline.json` is missing (the job says so and the candidate is in the artifact);
- a smoke case fails.

To change the baseline on purpose, re-record it from a CI run as above and explain why in the PR.

### Startup budgets

Each agent's startup reading (its body, the Startup files it must read, and its preloaded skills) has a byte budget in the `BUDGET` dict in `scripts/startup_budget.py` (the test is `test_roster`), set at the measured size plus 15%. This makes upstream growth show up in a sync PR instead of silently raising the cost of every call. `python3 scripts/startup_budget.py [agent ...]` prints each agent's current size against its budget. Upstream-watch issues for a source that an agent reads at startup carry a "Startup budget" section with the current sizes; run that command after `vendor.py sync` to get the sizes after the sync.

When a sync fails a budget:

1. Check what grew: compare the failing agent's measured size (in the failure message) with its budget, and look at what the sync changed in its Startup files.
2. If the growth is not worth it, trim the patch or drop the file from the agent's Startup list.
3. If it is worth it, raise that one number in `BUDGET`. The failure message gives the value to use, `ceil(measured * 1.15)`. Set it and explain the growth in the same commit and in the PR description. Never raise a budget to get a build green without that review.

### The upstream watcher

[`upstream-watch.yml`](.github/workflows/upstream-watch.yml) runs weekly and can be started by hand. [`scripts/upstream_watch.py`](scripts/upstream_watch.py) compares every pin in `vendor/sources.json`, the rtk upstream release and the latest Claude Code release against upstream. For each change it opens one GitHub issue, labelled `upstream-watch`, with a summary of the diff and the command to run (`vendor.py sync …` or `sync-rtk.sh …`). It updates an existing issue when the target moves again, and never closes one: close it when the sync merges. It never changes the repo itself.

A new Claude Code release also starts the evals. To bump the Claude Code that CI validates with, set the version in `ci/claude-code/package.json`, run `npm install --package-lock-only` in `ci/claude-code`, and let `plugin-validate` and the evals run on the PR.

To see what it would report without touching GitHub:

```sh
python3 scripts/upstream_watch.py --dry-run
```
