# Contributing

This file has two parts: how to work on the repo, and a runbook for maintainers.

For what the product is and why it is built this way, read the [README](README.md) and [DECISIONS.md](DECISIONS.md). Report security problems privately, as described in [SECURITY.md](SECURITY.md).

## Ground rules

- **The repo is public.** Never commit a secret, a token, a `.env` file, an absolute home path (use `~/.claude/...`), a machine name or an email address. A test and gitleaks check this.
- **No options.** Anthology has no user-facing modes, levels or toggles. Do not add any.
- **Vendored content is generated.** `plugins/anthology/knowledge/**` and `plugins/anthology-superpowers/**` are produced by `anthology-dev vendor` (run as [`scripts/dev.sh`](scripts/dev.sh) `vendor`) from pinned upstream commits. Never edit them by hand: change the patch or the transform and regenerate. CI regenerates everything and fails on any difference.
- **BrowserOS Neo is the only browser.** Nothing we ship recommends another browser or launches Chrome, and nothing installs browser tooling on the host.
- **One language: Rust.** Product code lives in `crates/` and ships as one static `anthology` binary (`crates/anthology-cli`), pinned per plugin by `anthology.lock`. The plugin side is POSIX `sh` only: the shims in `hooks/` and the wrappers in `bin/`. Maintainer tools are `anthology-dev` subcommands. The only other code is shell glue around docker, git and `claude` in `scripts/` and `ci/jobs/`. Do not add another language; the vendored superpowers files are upstream's and stay as they are.
- **Behaviour is pinned.** The frozen golden files (`crates/<crate>/tests/golden/`) record what each ported surface must answer, and `cargo test` replays them. Change one only on purpose, with `scripts/dev.sh golden bless`, and say why in the PR. The test ledger in `port/ledger/` maps every test the port inherited to a Rust test or a reviewed drop reason; `ci/jobs/ledger.sh` (the `ledger` job) checks every row against the tests cargo really runs.
- **The shipped set needs a binary release.** A change to the crates the binary is built from (their `src/` and `Cargo.toml`, their `Cargo.lock` entries, `rust-toolchain.toml`, the release profile) reaches users only through a new `anthology-bin--v*` release and a new `anthology.lock` (see [Releasing the anthology binary](#releasing-the-anthology-binary)).
- **Every change to a plugin bumps that plugin's version.** CI fails when a released plugin's folder changes with no higher `version` in its `.claude-plugin/plugin.json`. Add a line under that plugin's `[Unreleased]` heading in [CHANGELOG.md](CHANGELOG.md) too.
- **Memory changes get their own commits,** `chore(memory): …`.
- **Models are aliases** (`opus`, `sonnet`, `haiku`), never version-pinned ids.

## Repository layout

| Path | What it holds |
|---|---|
| `.claude-plugin/marketplace.json` | The marketplace `anthology` and its three plugins |
| `plugins/anthology/` | Agents, generated knowledge, the three skills (`setup`, `export`, `import`), the `sh` hooks in `hooks/`, the `bin/` wrappers, `rtk.lock`, `anthology.lock`, the routing guide |
| `plugins/anthology-guardrails/` | The `PreToolUse` guard shim (`hooks/guard.sh`), its bootstrap and `anthology.lock`; the rules are compiled into the binary |
| `plugins/anthology-superpowers/` | Generated from `vendor/`, renamed from superpowers |
| `crates/` | The Rust workspace: `anthology-cli` (the `anthology` binary) and the libraries it is built from (`anthology-core`, `anthology-guard`, `anthology-shell`, `anthology-memory`, `anthology-runtime`, `anthology-settings`, `anthology-pycompat`); and the tools that never ship: `anthology-dev` (the maintainer tools), `difftest` (the golden replayer), `anthology-testkit` and `repo-tests` (the repository checks) |
| `vendor/` | `sources.json` (every pin), `patches/`, `snapshots/`, `licenses/` |
| `rtk/` | The rtk source, a git subtree with its history |
| `scripts/` | Shell entry points: `dev.sh` (runs `anthology-dev` in the pinned Rust image), `rust-test.sh`, `sync-rtk.sh`, `rtk-trim.sh`, `rtk-test.sh`, `rtk-net-test.sh`, `release.sh`, `run-evals.sh` |
| `evals/` | Behaviour evals (one `case.yaml` per case) and the mocked BrowserOS Neo server in `mocks/`; run with `scripts/run-evals.sh`, which also runs the smoke cases (`anthology-dev smoke`) |
| `port/` | The record of the port: the test ledger in `port/ledger/`, the difftest divergences and seeds, and the replay reports |
| `fuzz/` | The fuzz targets, run weekly by `fuzz.yml` |
| `ci/` | Pinned Docker images, the pinned Claude Code, and one script per CI job in `ci/jobs/` |
| `.github/workflows/` | `ci.yml`, `evals.yml`, `upstream-watch.yml`, `fuzz.yml`, `rtk-release.yml`, `anthology-release.yml` |

`skills/` holds only the three user commands. Every knowledge folder must be read by at least one agent, and a test enforces it.

## Development setup

You need git, Docker and Node 22 or newer. You do not need Rust, Chrome or any browser tooling on the host: the tests and the maintainer tools run in the pinned Rust image. Only a local eval run builds on the host (see [Evals and the baseline](#evals-and-the-baseline)).

1. Clone the repo and work on a branch.
2. Run the checks from the repo root, in the pinned image, the way CI does:

   ```sh
   scripts/rust-test.sh                     # fmt and clippy, then cargo test --workspace --locked
   scripts/rust-test.sh test -p repo-tests  # only tests; extra arguments go to cargo test
   ```

   [`scripts/rust-test.sh`](scripts/rust-test.sh) reads the image from `ci/images/rust.Dockerfile`, installs a few test-only tools inside the container (`git`, `curl`, `procps`, `busybox`, `nodejs`) and runs cargo as a non-root user. Named volumes keep the cargo registry and a target folder per checkout. `cargo test` also replays the frozen golden files and runs the shim tests.
3. Run a maintainer tool with [`scripts/dev.sh`](scripts/dev.sh) `<command>`, which builds and runs `anthology-dev` in the same image; its stdout is the tool's own output. The commands are `vendor`, `version-bump`, `release-edit`, `memory-diff`, `startup-budget`, `eval-baseline`, `upstream-watch`, `rtk-trim`, `smoke`, `lock`, `seed`, `ledger`, `golden` and `replay`, plus the table generators `gen-unicode` and `gen-errno`. `scripts/dev.sh --help` lists them with their subcommands.
4. Run the rtk tests with [`scripts/rtk-test.sh`](scripts/rtk-test.sh). It runs `cargo test --locked` for `rtk/` in the same image, as a non-root user; extra arguments go to `cargo test`. It needs Docker and network access, because it installs a few test-only tools (`git`, `curl`, `jq`, `procps`, `ca-certificates`) inside the container only. [`scripts/rtk-net-test.sh`](scripts/rtk-net-test.sh) builds the release binary in the same image and runs it with `docker run --network none`.
5. Check the vendored content with `scripts/dev.sh vendor check`.
6. Validate a plugin with the Claude Code version CI uses: `npm ci --prefix ci/claude-code`, then `ci/claude-code/node_modules/.bin/claude plugin validate --strict plugins/<name>`.
7. Try your checkout in Claude Code: `claude plugin marketplace add <path to your checkout>` loads the marketplace in place, then install `anthology@anthology` as in the README. The plugins then install the released binary that `anthology.lock` pins, not one built from your checkout. The evals and smokes test a local build, on a scratch copy of `plugins/` that `anthology-dev seed` prepares.

### What CI runs

| Check | What it proves |
|---|---|
| `rust-lint` | `cargo fmt --check` and `clippy -D warnings` |
| `rust-test-linux` and `rust-test-macos` | `cargo test --workspace --locked` in the pinned Rust image and on macOS: the unit tests, the repository checks, the shim tests and the golden replay |
| `deny` | `cargo deny check` (advisories, bans, licenses, sources; `deny.toml`), and that `serde_json` stays out of the shipped binary |
| `shellcheck` and `shims-distros` | The `sh` shims pass `shellcheck -s sh`, and the shim tests pass on Alpine, Debian slim and Ubuntu, with each system's own shell and downloaders |
| `ledger` (`ci/jobs/ledger.sh`) | Every row of the test ledger names a Rust test that cargo really runs, or a reviewed drop reason |
| `fuzz-build` | Every fuzz target builds and survives a short run (`fuzz.yml` runs them longer, weekly) |
| `rtk-test` job (`scripts/rtk-test.sh`, then `scripts/rtk-net-test.sh`) | rtk passes its tests, including one named test per change the fork makes, and the release binary works with the network off |
| `rtk-release` and `anthology-release` workflows, on a pull request | A change to the workflow or the toolchain pin builds and tests all four release targets, without publishing |
| `vendor-check` (`scripts/dev.sh vendor check`) | Every vendored tree and license regenerates exactly from its pin |
| `version-bump` (`scripts/dev.sh version-bump`) | A changed plugin has a higher version than at the merge base |
| `plugin-validate` (`claude plugin validate --strict`) | Each plugin is valid, using the pinned Claude Code in `ci/claude-code/` |
| gitleaks | No secrets in the history, using `.gitleaks.toml` |
| `memory-diff` (`scripts/dev.sh memory-diff`, pull requests) | Lists changed files under `.claude/memory/` and `.claude/agent-memory/` in the job summary, so a memory note is reviewed like code |
| `startup-budget` (`scripts/dev.sh startup-budget`, then the budget tests) | The per-agent startup budgets and the session-start output budget (9,000 characters) hold |
| `cli-syntax` (`ci/jobs/cli-syntax.sh`) | The `claude plugin` subcommands and flags the plugins and evals rely on exist in the pinned Claude Code. No model is called |

Images are pinned by digest in `ci/images/*.Dockerfile`, Rust crates by `Cargo.lock`, the toolchain by `rust-toolchain.toml`, and GitHub Actions by commit SHA. Dependabot proposes the bumps, and CI and the evals gate them.

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
   scripts/dev.sh vendor sync <name> --ref <new tag, branch or commit>
   ```

   `sync` re-pins the entry, rebuilds its target, refreshes its license file, and never commits. It exits 1 and prints `REJECT` lines when a patch no longer applies.
3. If a patch was rejected, update `vendor/patches/<name>.patch` for the new upstream and run `scripts/dev.sh vendor build <name>`. `scripts/dev.sh vendor diff <name>` prints the difference between pristine-plus-transforms and the shipped tree as a patch, which is a starting point.
4. Run `scripts/dev.sh vendor check`. It must be clean.
5. Review the diff. If the output changed, bump the plugin version it belongs to. For `anthology-superpowers`, which is versioned independently of upstream superpowers, bump `plugin_version` in the `superpowers` entry of `sources.json` (and `upstream_version` is updated by `sync`).
6. Run the tests. A new knowledge file can push an agent over its startup budget (see below).
7. Open a PR. CI and the evals are the gate.

Notes for particular sources:

- **`design-taste-frontend`** has a machine-readable port map, `vendor/patches/design-taste-frontend.portmap.json`, that lists each upstream script as keep, port or drop. `vendor` fails when upstream adds a script the map does not list, and when any shipped file contains a Chrome launch. When that happens, decide for the new script and add it to the map.
- **`ui-taste`** has no git upstream. It is pinned by the sha256 of its `CHECKSUMS.sha256` manifest, and a pristine copy lives in `vendor/snapshots/ui-taste/`. To move it, set `checksum` in `sources.json`, then run `scripts/dev.sh vendor fetch ui-taste && scripts/dev.sh vendor build ui-taste`.
- **`vendor/licenses/`** carries each source's license text, unchanged, refreshed by `fetch` and `sync`.
- **Licenses inside the plugin.** Claude Code installs a plugin directory on its own, so `build` also copies each source's license, unchanged, into its target folder (a source with no license file of its own gets the fixed text as `LICENSE`). An upstream `NOTICE` outside the source folder must be listed in the entry's `license_extra` (caveman ships `NOTICE` and `LICENSE-MIT` this way), or `build` fails. `check` verifies the copies byte for byte.

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
2. `rtk-trim.sh` removes upstream's BUSL-1.1 `engine/`, `plugin/` and `proxy/`, so that what ships is the Apache-2.0 edition, and keeps rtk Claude Code only and free of telemetry. It is idempotent and stops with exit 1, naming the file and the anchor, when an edit's expected text is missing, which means an upstream refactor to look at. A removal whose target upstream renamed is not noticed that way; the allowlist tests below catch it. The allowlist test `crates/repo-tests/tests/rtk_claude_only.rs` fails when upstream adds a hook integration module or a Cargo dependency that is not on the list. Adding one is a deliberate decision, not a test fix.
3. Each of the fork's eight remaining changes has a permanent named test (`fork_1_…` to `fork_7_…` and `fork_9_…`). They must still pass. The ninth, custom `[upstream.*]` headers, lived only in the removed BUSL engine, so its test is gone.
4. Review and commit the pin and the trim's changes, run the tests (`scripts/rust-test.sh`, as in the development setup), then open a PR.

#### Releasing rtk binaries

1. Set the `rtk/Cargo.toml` version to `<upstream version>-anthology.<n>` (for example `1.2.1-anthology.2`), update the `rtk` package entry in `rtk/Cargo.lock` to match (the builds use `--locked`), and merge that.
2. Push a tag `rtk--v<version>` on the commit to release. `rtk-release.yml` fails unless the tag equals `rtk--v` plus the `Cargo.toml` version. It builds `aarch64` and `x86_64` for macOS (`apple-darwin`) and Linux (`unknown-linux-musl`), each on a native runner, with the toolchain in `rtk/rust-toolchain.toml`, running `cargo test --locked` before each `cargo build --release --locked`. It attests the archives and publishes `rtk-<target>.tar.gz` and `checksums.txt` to a new release; it fails rather than overwrite an existing release. To publish from an existing tag, run the workflow by hand with the tag as input.
3. Write the version, the tag and the sha256 of each target from `checksums.txt` into `plugins/anthology/rtk.lock`. `crates/repo-tests/tests/rtk_lock.rs` checks that the lock matches `Cargo.toml` and has a 64-hex hash for all four targets.
4. Verify one asset: `gh attestation verify <file> --repo rbonweb/anthology`, and compare its hash.
5. The lock change is a change to `plugins/anthology`, so release a new `anthology` version (below). Installed users get the new binary at their next session start.

### Releasing the anthology binary

The `anthology` binary has its own releases, separate from the plugin versions. Both plugins pin the same one in `anthology.lock` (ADR-029).

1. Set `version` under `[workspace.package]` in the root `Cargo.toml`, update the workspace entries in `Cargo.lock` to match (the builds use `--locked`), and merge that.
2. Push a tag `anthology-bin--v<version>` on the commit to release. [`anthology-release.yml`](.github/workflows/anthology-release.yml) fails unless the tag equals `anthology-bin--v` plus that version. On a native runner per target it runs `cargo deny check` and the tests of the crates the binary is built from, builds `anthology-cli` with `--release --locked` (static musl on Linux), checks `anthology --version`, packs `anthology-<target>.tar.gz` (the binary and `LICENSE`), attests the archives and publishes them with `checksums.txt` to a new release. It fails rather than overwrite an existing release; to publish from an existing tag, run the workflow by hand with the tag as input.
3. Pin it in both plugins: save `gh release view <tag> --json assets > <file>` in the checkout, then run `scripts/dev.sh lock write --tag <tag> --checksums <checksums.txt> --release-json <file>`. It takes the four hashes from `checksums.txt`, refuses any that differ from GitHub's own digest of the asset, and writes both `anthology.lock` files in the fixed layout the shims read. `scripts/dev.sh lock check` confirms the two are valid and identical.
4. Verify one asset: `gh attestation verify anthology-<target>.tar.gz --repo rbonweb/anthology`, and compare its hash with the lock.
5. The lock changes both `plugins/anthology` and `plugins/anthology-guardrails`, so release both (below), `anthology-guardrails` first.

`scripts/release.sh` runs `scripts/dev.sh lock check-fresh` before it releases a plugin with an `anthology.lock`: it compares the shipped set (the `src/` and `Cargo.toml` of every crate `anthology-cli` is built from, their `Cargo.lock` entries, `rust-toolchain.toml` and the release profile) between the lock's tag and `HEAD`, and stops when it changed, because the pinned binary would then not match the source. Changes to tests, `anthology-dev`, the difftest and the golden files never need a binary release.

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
- every other plugin that depends on this one has a version range that accepts `<version>`. If one does not, widen the range in that plugin's `plugin.json` first, which is its own change with its own bump;
- for a plugin that ships `anthology.lock`, `lock check-fresh` passes (see [Releasing the anthology binary](#releasing-the-anthology-binary)).

Its edits run through `scripts/dev.sh`, so it needs Docker, or `ANTHOLOGY_DEV` naming an `anthology-dev` binary. Then it bumps `plugin.json` (and `plugin_version` in `vendor/sources.json` for `anthology-superpowers`), moves the `[Unreleased]` entries into a dated version section, commits as `release: <plugin> <version>`, and runs `claude plugin tag`. It never pushes without `--push`; without it, it prints the two `git push` commands to publish. `--dry-run` does all of this in a throwaway worktree, shows the commit and the CHANGELOG diff, and leaves your checkout, index and tags untouched.

A plugin's `version` is what moves installed users to new code, so a change that is not released does not reach them. Release a dependency before the plugin that needs its new version.

### Upgrading from 0.1.x

After updating from 0.1.x, restart any running Claude Code sessions.

A session that started on 0.1.x keeps the old session-start hook. That hook looks for the 0.1.x guardrails layout, which no longer exists, so it wrongly reports "anthology-guardrails is not installed". The new hooks also do not load until the session restarts. The warning goes away in a new session.

### Evals and the baseline

The behaviour evals in [`evals/`](evals/) catch a model update or a Claude Code update that silently breaks the product. Each case is `evals/<case>/case.yaml`. They sit at the repo root, because a case can only load plugins inside its containment root. Each case lists all three plugins as `../../plugins/anthology`, `../../plugins/anthology-guardrails` and `../../plugins/anthology-superpowers`. They cover:

- routing (the main session delegates to the right `anthology:*` agent);
- knowledge-read order (each agent reads its Startup files before any other tool);
- the G1, G4 and G6 blocks;
- BrowserOS-only browsing, against a mocked `browseros-neo` MCP server in `evals/mocks/` and with no Chrome launch;
- rtk condensing `git status` output, and G1 still blocking a Playwright command that rtk would rewrite;
- the routing guide being in the main session, and absent from agents;
- the platform checks recorded in [DECISIONS.md](DECISIONS.md) that a case can express (agent fields, memory paths, hooks reaching subagents, skills hidden from the main session).

The checks a case cannot express are smoke cases, run by `anthology-dev smoke` (`crates/anthology-dev/src/smoke/`): the Claude Code CLI syntax, the eval cost ceiling, the session-start output cap, a missing hook interpreter, a missing `anthology` binary (the shims fail closed and say why), a stalled guard, the worktree base, the model each agent runs on, rtk's rewrite, and the knowledge-read, agent-memory-path, auto-memory-directory and memory-commit checks for writers. `cli-syntax` and `binary-missing` call no model.

Run everything from the repo root with `scripts/run-evals.sh`. It runs the evals, then the smoke cases, against a scratch copy of `plugins/` with a locally built `anthology` binary seeded in (`anthology-dev seed`). It builds that binary and `anthology-dev` with `cargo build --release --locked`, so a local run needs the Rust toolchain from `rust-toolchain.toml` on the host, unless `ANTHOLOGY_BIN` and `ANTHOLOGY_DEV` name binaries you built. Under the hood it calls one `claude plugin eval` with a fixed `--allow-tools` list:

```sh
claude plugin eval . --trust-plugin --ablation none --no-publish --threshold 0 --runs 1 \
  --max-cost-usd <ceiling> --json <result.json> --output-dir <dir> --allow-tools <grants>
```

A tool that is not granted is withheld from the model. The grants are the few harmless commands the guard and rtk cases need (listed in the script), so a broken guard still cannot download anything; the guard and rtk cases only work through the script, so don't run the bare command and read its failures as regressions. `--case GLOB` and `--tag TAG` run part of the suite (the comparison then tolerates missing cases, the smokes are skipped and `--record` is refused); `--no-smoke` skips the smokes. Results go to `evals/results/<UTC timestamp>/`.

The cost ceiling defaults to $20 for the evals (`--max-cost-usd`) plus $10 for all smokes together (`--smoke-total-budget-usd`). When the evals hit their ceiling the run is partial, and the comparison fails it with exit code 2, so raise the ceiling deliberately. The exit codes are the comparison's when it is not 0 (1 regression, 2 invalid or partial result, 3 no baseline), else 4 when a smoke failed, else 0. A smoke that skips because there are no credentials is not a failure.

**The baseline** is `evals/baseline.json`, the per-case scores of a full, non-partial run. The committed one is recorded from the CI run, so that the baseline and every later comparison come from the same environment: dispatch `evals.yml` once with no baseline committed, download its `evals-results` artifact and commit `candidate-baseline.json` as `evals/baseline.json` with the `candidate` key removed. `scripts/run-evals.sh --record` writes a baseline from a local full run, for experiments.

[`evals.yml`](.github/workflows/evals.yml) runs weekly, on demand, and when the upstream watcher sees a new Claude Code release. It installs the pinned Claude Code from the `ci/claude-code` lockfile and runs `ci/jobs/evals.sh`, which builds the binaries with the pinned toolchain and then calls `scripts/run-evals.sh --ci`. It needs the `ANTHROPIC_API_KEY` repository secret (it skips with a notice while that secret is missing). It has no pull-request or push trigger, so it never runs for a fork; to evaluate a Claude Code bump, dispatch it on the Dependabot branch. The run fails when:

- a baseline case scores below its baseline score;
- a baseline case is missing from the run;
- the run is partial (the cost ceiling was hit) or the result is invalid;
- `evals/baseline.json` is missing (the job says so and the candidate is in the artifact);
- a smoke case fails.

To change the baseline on purpose, re-record it from a CI run as above and explain why in the PR.

### Startup budgets

Each agent's startup reading (its body, the Startup files it must read, and its preloaded skills) has a byte budget in the `BUDGET` table in `crates/anthology-dev/src/startup_budget.rs` (the tests are in `crates/repo-tests/tests/roster.rs`), set at the measured size plus 15%. This makes upstream growth show up in a sync PR instead of silently raising the cost of every call. `scripts/dev.sh startup-budget [agent ...]` prints each agent's current size against its budget. Upstream-watch issues for a source that an agent reads at startup carry a "Startup budget" section with the current sizes; run that command after `scripts/dev.sh vendor sync` to get the sizes after the sync.

When a sync fails a budget:

1. Check what grew: compare the failing agent's measured size (in the failure message) with its budget, and look at what the sync changed in its Startup files.
2. If the growth is not worth it, trim the patch or drop the file from the agent's Startup list.
3. If it is worth it, raise that one number in `BUDGET`. The failure message gives the value to use, `ceil(measured * 1.15)`. Set it and explain the growth in the same commit and in the PR description. Never raise a budget to get a build green without that review.

### The upstream watcher

[`upstream-watch.yml`](.github/workflows/upstream-watch.yml) runs weekly and can be started by hand. It runs `anthology-dev upstream-watch` (`cargo run --locked --release -p anthology-dev -- upstream-watch` on the runner, because it needs the job's GitHub token), which compares every pin in `vendor/sources.json`, the rtk upstream release and the latest Claude Code release against upstream. For each change it opens one GitHub issue, labelled `upstream-watch`, with a summary of the diff and the command to run (`scripts/dev.sh vendor sync …` or `scripts/sync-rtk.sh …`). It updates an existing issue when the target moves again, and never closes one: close it when the sync merges. It never changes the repo itself.

A new Claude Code release also starts the evals. To bump the Claude Code that CI validates with, set the version in `ci/claude-code/package.json`, run `npm install --package-lock-only` in `ci/claude-code`, and let `plugin-validate` and the evals run on the PR.

To see what it would report without touching GitHub:

```sh
scripts/dev.sh upstream-watch --dry-run
```
