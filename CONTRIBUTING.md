# Contributing

This file has two parts: how to work on the repo, and a runbook for maintainers.

For what the product is and why it is built this way, read the [README](README.md) and [DECISIONS.md](DECISIONS.md). Report security problems privately, as described in [SECURITY.md](SECURITY.md).

## Ground rules

- **The repo is public.** Never commit a secret, a token, a `.env` file, an absolute home path (use `~/.claude/...`), a machine name or an email address. A test and gitleaks check this.
- **No options.** Anthology has no user-facing modes, levels or toggles. Do not add any.
- **Vendored content is generated.** `plugins/anthology/knowledge/**` and `plugins/anthology-superpowers/**` are produced by `anthology-dev vendor` (run as [`scripts/dev.sh`](scripts/dev.sh) `vendor`) from pinned upstream commits. Never edit them by hand: change the patch or the transform and regenerate. CI regenerates everything and fails on any difference. The one exception is a first-party knowledge folder, which no source targets and which is written by hand (`dashboard-copy`; see [Syncing a vendored source](#syncing-a-vendored-source)).
- **BrowserOS Neo is the only browser.** Nothing we ship recommends another browser or launches Chrome, and nothing installs browser tooling on the host.
- **One language: Rust.** Product code lives in `crates/` and ships as one static `anthology` binary (`crates/anthology-cli`), pinned per plugin by `anthology.lock`. The plugin side is POSIX `sh` only: the shims in `hooks/` and the wrappers in `bin/`. Maintainer tools are `anthology-dev` subcommands. The only other code is shell glue around docker, git and `claude` in `scripts/` and `ci/jobs/`. Do not add another language; the vendored superpowers files are upstream's and stay as they are.
- **Behaviour is pinned.** The frozen golden files (`crates/<crate>/tests/golden/`) record what each ported surface must answer, and `cargo test` replays them. Change one only on purpose, with `scripts/dev.sh golden bless`, and say why in the PR. The test ledger in `port/ledger/` maps every test the port inherited to a Rust test or a reviewed drop reason; `ci/jobs/ledger.sh` (the `ledger` job) checks every row against the tests cargo really runs.
- **The shipped set needs a binary release.** A change to the crates the binary is built from (their `src/` and `Cargo.toml`, their `Cargo.lock` entries, `rust-toolchain.toml`, the release profile) reaches users only through a new `anthology-bin--v*` release and a new `anthology.lock` (see [Releasing the anthology binary](#releasing-the-anthology-binary)).
- **Every change to a plugin bumps that plugin's version.** CI fails when a released plugin's folder changes with no higher `version` in its `.claude-plugin/plugin.json`. Add a changelog entry too: a fragment file `changelog.d/<plugin>/<name>.md` (see [changelog.d/README.md](changelog.d/README.md)), so two PRs do not edit the same lines of [CHANGELOG.md](CHANGELOG.md). Writing under that plugin's `[Unreleased]` heading still works.
- **Dev-only helpers go in dev crates.** Code that only tests or maintainers use belongs in `anthology-dev`, `anthology-testkit`, `difftest` or `repo-tests`, never behind a Cargo feature of a crate the binary is built from: such a feature changes the lock file and the shipped set, and the next release then needs a binary release for no user benefit.
- **Memory changes get their own commits,** `chore(memory): …`.
- **Models are aliases** (`opus`, `sonnet`, `haiku`), never version-pinned ids.

## Repository layout

| Path | What it holds |
|---|---|
| `.claude-plugin/marketplace.json` | The marketplace `anthology` and its three plugins |
| `plugins/anthology/` | Agents, generated knowledge, the three skills (`setup`, `export`, `import`), the `sh` hooks in `hooks/`, the `bin/` wrappers, `atl.lock`, `anthology.lock`, the routing guide |
| `plugins/anthology-guardrails/` | The `PreToolUse` guard shim (`hooks/guard.sh`), its bootstrap and `anthology.lock`; the rules are compiled into the binary |
| `plugins/anthology-superpowers/` | Generated from `vendor/`, renamed from superpowers |
| `crates/` | The Rust workspace: `anthology-cli` (the `anthology` binary) and the libraries it is built from (`anthology-core`, `anthology-guard`, `anthology-shell`, `anthology-memory`, `anthology-runtime`, `anthology-settings`, `anthology-pycompat`); and the tools that never ship: `anthology-dev` (the maintainer tools), `difftest` (the golden replayer), `anthology-testkit` and `repo-tests` (the repository checks) |
| `changelog.d/` | Changelog fragments, `changelog.d/<plugin>/*.md`, which `scripts/release.sh` compiles into `CHANGELOG.md` and deletes |
| `vendor/` | `sources.json` (every pin), `patches/`, `snapshots/`, `licenses/` |
| `atl/` | The source of ATL (Anthology Token Layer), a fork of rtk by RTK AI Labs, as a git subtree with its history. `atl/LICENSE` is upstream's Apache-2.0 license, kept as it is, and `atl/NOTICE` records the origin and the changes; both ship in every archive |
| `scripts/` | Shell entry points: `dev.sh` (runs `anthology-dev` in the pinned Rust image), `rust-test.sh`, `remote-build.sh` (the same checks on a remote build host), `sync-atl.sh`, `atl-trim.sh`, `atl-test.sh`, `atl-net-test.sh`, `release.sh`, `run-evals.sh` |
| `evals/` | Behaviour evals (one `case.yaml` per case) and the mocked BrowserOS Neo server in `mocks/`; run with `scripts/run-evals.sh`, which also runs the smoke cases (`anthology-dev smoke`) |
| `port/` | The record of the port: the test ledger in `port/ledger/`, the difftest divergences and seeds, and the replay reports |
| `fuzz/` | The fuzz targets, run weekly by `fuzz.yml` |
| `ci/` | Pinned Docker images, the pinned Claude Code, and one script per CI job in `ci/jobs/` |
| `.github/workflows/` | `ci.yml`, `evals.yml`, `upstream-watch.yml`, `fuzz.yml`, `atl-release.yml`, `anthology-release.yml` |

`skills/` holds only the three user commands. Every knowledge folder must be read by at least one agent, or linked from one it reads, and a test enforces it.

## Development setup

You need git, Docker and Node 22 or newer. You do not need Rust, Chrome or any browser tooling on the host: the tests and the maintainer tools run in the pinned Rust image. Only a local eval run builds on the host (see [Evals and the baseline](#evals-and-the-baseline)).

1. Clone the repo and work on a branch.
2. Run the checks from the repo root, in the pinned image, the way CI does:

   ```sh
   scripts/rust-test.sh                     # fmt and clippy, then cargo test --workspace --locked
   scripts/rust-test.sh test -p repo-tests  # only tests; extra arguments go to cargo test
   ```

   [`scripts/rust-test.sh`](scripts/rust-test.sh) reads the image from `ci/images/rust.Dockerfile`, installs a few test-only tools inside the container (`git`, `curl`, `procps`, `busybox`, `nodejs`) and runs cargo as a non-root user. Named volumes keep the cargo registry and a target folder per checkout. `cargo test` also replays the frozen golden files and runs the shim tests.
3. Run a maintainer tool with [`scripts/dev.sh`](scripts/dev.sh) `<command>`, which builds and runs `anthology-dev` in the same image; its stdout is the tool's own output. The commands are `vendor`, `version-bump`, `release-edit`, `memory-diff`, `startup-budget`, `eval-baseline`, `upstream-watch`, `atl-trim`, `smoke`, `lock`, `seed`, `ledger`, `golden` and `replay`, plus the table generators `gen-unicode` and `gen-errno`. `scripts/dev.sh --help` lists them with their subcommands.
4. Run the atl tests with [`scripts/atl-test.sh`](scripts/atl-test.sh). It runs `cargo test --locked` for `atl/` in the same image, as a non-root user; extra arguments go to `cargo test`. It needs Docker and network access, because it installs a few test-only tools (`git`, `curl`, `jq`, `procps`, `ca-certificates`) inside the container only. [`scripts/atl-net-test.sh`](scripts/atl-net-test.sh) builds the release binary in the same image and runs it with `docker run --network none`.
5. Check the vendored content with `scripts/dev.sh vendor check`.
6. Validate a plugin with the Claude Code version CI uses: `npm ci --prefix ci/claude-code`, then `ci/claude-code/node_modules/.bin/claude plugin validate --strict plugins/<name>`.
7. Try your checkout in Claude Code: `claude plugin marketplace add <path to your checkout>` loads the marketplace in place, then install `anthology@anthology` as in the README. The plugins then install the released binary that `anthology.lock` pins, not one built from your checkout. The evals and smokes test a local build, on a scratch copy of `plugins/` that `anthology-dev seed` prepares.

### Building on a remote host

To keep a long Rust build off your own machine, [`scripts/remote-build.sh`](scripts/remote-build.sh) runs `scripts/rust-test.sh` on a Linux build host over ssh. The host needs Docker (your user in the `docker` group), `bash` 4 or newer and `tar`; `rsync` is used when both ends have it. Key-based ssh login must already work: the script uses `ssh -o BatchMode=yes`, never prompts, never uses `sudo`, and changes nothing about the host's ssh or auth setup. Give it the host's ssh alias in the environment, never in a file of the repo:

```sh
ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh test -p anthology-guard
ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh lint
ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean
ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-ours
```

The arguments are the ones `scripts/rust-test.sh` takes. The script copies the checkout (tracked files, plus uncommitted and new files that are not ignored; never `target/` or `.claude/worktrees`) to `~/anthology-remote/<hash>/` on the host, runs the checks there, streams the output and exits with their exit code. The `<hash>` names the checkout, so each worktree has its own folder and its own target volume. Run one build per checkout at a time: a new run removes the containers that an earlier run of the same checkout left.

The host is shared, so the script leaves nothing of ours on it that can fill its disk:

- **Clean by default.** When a run ends, passed or failed, and also on Ctrl-C or a dropped connection, the checkout's folder, its `anthology-rust-target-*` volume and its containers are removed. `ANTHOLOGY_KEEP_REMOTE=1` keeps the folder and volume, so the next run sends only what changed and builds warm. Use it for one working session, then run `--clean`.
- **Only our own items.** The script removes only what it created: the folders under `~/anthology-remote/`; the `anthology-rust-target-*`, `anthology-cargo`, `anthology-sccache` and `anthology-rustup-*` volumes, which it creates itself with the label `anthology.remote-build=1` and treats as ours only when the name matches and the label is there (a same-named volume without the label was on the host before us and is never touched); the containers that mount those volumes; and the Rust image, only if it was not on the host when our run started (the script records that in `~/anthology-remote/.pulled-images`) and only when no container that is not ours uses it. It never prunes, never forces a removal and touches nothing outside `~/anthology-remote`.
- **Self-heal at the start.** Before each run the script removes our own leftovers that were not used for 12 hours: folders, `anthology-rust-target-*` volumes and our stopped containers. When nothing of ours has run for 48 hours (the time of `~/anthology-remote/.last-used`, written at the start and the end of each run), it also removes the `anthology-cargo`, `anthology-sccache` and `anthology-rustup-*` caches and the pulled image. A volume that a running container uses stays.
- **Disk guard.** The script refuses to start, and says so, when the host has less than 20 GB free on its home disk or its Docker data disk.
- **`--clean`** removes this checkout's folder, volume and containers. **`--clean-ours`** removes everything of ours, as defined above, and nothing else. The caches and the image stay between runs so that a run starts quickly; run `--clean-ours` when you are done for the day.

The remote folder is a copy without git history, so checks that read it (for example `scripts/dev.sh lock check-fresh`) belong on your own machine or in CI.

### What CI runs

| Check | What it proves |
|---|---|
| `rust-lint` | `cargo fmt --check` and `clippy -D warnings` |
| `rust-test-linux` and `rust-test-macos` | `cargo test` in the pinned Rust image and on macOS: the unit tests, the repository checks, the shim tests and the golden replay (one test per difftest surface). They are the required checks and gate on parallel `-shard` jobs (plain and `--all-features`), which pass when `ci/jobs/rust-scope.sh` found nothing for them to run. See [What a pull request runs](#what-a-pull-request-runs) |
| `deny` | `cargo deny check` (advisories, bans, licenses, sources; `deny.toml`), and that `serde_json` stays out of the shipped binary |
| `shellcheck` and `shims-distros` | The `sh` shims pass `shellcheck -s sh`, and the shim tests pass on Alpine, Debian slim and Ubuntu, with each system's own shell and downloaders |
| `ledger` (`ci/jobs/ledger.sh`) | Every row of the test ledger names a Rust test that cargo really runs, or a reviewed drop reason |
| `fuzz-build` | Every fuzz target builds and survives a short run (`fuzz.yml` runs them longer, weekly) |
| `atl-test` job (`scripts/atl-test.sh`, then `scripts/atl-net-test.sh`) | atl passes its tests, including one named test per change the fork makes, and the release binary works with the network off |
| `atl-test-macos` (`ci/jobs/atl-test-macos.sh`) | atl passes its tests on macOS too, where temporary folders sit behind `/private/var` symlinks |
| `atl-release` and `anthology-release` workflows, on a pull request | A change to the workflow or the toolchain pin builds and tests all four release targets, without publishing |
| `vendor-check` (`scripts/dev.sh vendor check`) | Every vendored tree and license regenerates exactly from its pin |
| `version-bump` (`scripts/dev.sh version-bump`) | A changed plugin has a higher version than at the merge base |
| `plugin-validate` (`claude plugin validate --strict`) | Each plugin is valid, using the pinned Claude Code in `ci/claude-code/` |
| gitleaks | No secrets in the history, using `.gitleaks.toml` |
| `memory-diff` (`scripts/dev.sh memory-diff`, pull requests) | Lists changed files under `.claude/memory/` and `.claude/agent-memory/` in the job summary, so a memory note is reviewed like code |
| `startup-budget` (`scripts/dev.sh startup-budget`, then the budget tests) | The per-agent startup budgets and the session-start output budget (9,000 characters) hold |
| `cli-syntax` (`ci/jobs/cli-syntax.sh`) | The `claude plugin` subcommands and flags the plugins and evals rely on exist in the pinned Claude Code. No model is called |

### What a pull request runs

A pull request runs only the Rust tests its change can affect, so a round takes minutes. `ci/jobs/rust-scope.sh` (rules in `ci/scope-rules.tsv`) reads the changed files and `cargo metadata`, and picks:

- **Crates.** The changed crates plus every crate that depends on them, through a normal, build or dev dependency. The `rest` shards run their tests and are split by the weights in `ci/test-weights.tsv`.
- **Golden surfaces.** The golden replay is one test per surface (`golden_replay::<surface>` in `crates/difftest/tests/golden.rs`). A pull request replays the surfaces of the crates it reaches, in one shard, plus the surfaces a rule names (an agent prompt replays `dev.startup-budget`). A change to `difftest` replays all of them.
- **Docs, plugins, scripts, evals, memory notes, release metadata.** The repository checks (`repo-tests`) only.
- **atl and fuzz.** `atl-test` runs for a change to `atl/` or a crate atl builds, `atl-test-macos` for a change to `atl/` itself, and `fuzz-build` for a change to `fuzz/` or a crate the targets build.
- **macOS.** The macOS shards run on a pull request only for platform-sensitive paths (the `macos` rules: shims, hooks, the guard, code that spawns processes or reads the platform), and then only for the affected crates. The `macos` rules include the hooks and scripts `repo-tests` runs under the macOS shell (the shims, the superpowers `session-start` hook, the mods' truth table).
- **Repository jobs.** `rust-lint`, `ledger`, `shims-distros`, `cli-syntax`, `startup-budget`, `mods`, `plugin-validate`, `version-bump`, `vendor-check`, `memory-diff` and `lock-fresh` run only when a path they read, or a crate they build or run, changed (`job:` rules and `crate:` rows in `ci/scope-rules.tsv`; the `jobs` output of `rust-scope`). A skipped job shows as skipped. `gitleaks`, `deny` and `shellcheck` run on every pull request.

Everything runs, as before, on every push to `build` and `main`, on a manual run of the workflow, and on a pull request that has the `full-ci` label or a commit with `[full-ci]` in its message (a trailer line or the subject). It also runs for a change to the test machinery itself (`ci.yml`, `ci/jobs/rust-*`, `ci/scope-rules.tsv`, the Rust image, the toolchain, `Cargo.toml` or `Cargo.lock` beyond a version) and for a path no rule places, so a new top-level folder needs a row in `ci/scope-rules.tsv`.

The Linux shards keep the cargo registry, the toolchain and the dependency build in a cache (`ci/jobs/rust-cache.sh`): a push to `build` or `main` saves it and a pull request reads it.

Images are pinned by digest in `ci/images/*.Dockerfile`, Rust crates by `Cargo.lock`, the toolchain by `rust-toolchain.toml`, and GitHub Actions by commit SHA. Dependabot proposes the bumps, and CI and the evals gate them.

### Pull requests

- Say what changed and why.
- Bump the plugin version, and add a changelog fragment `changelog.d/<plugin>/<name>.md` (see [changelog.d/README.md](changelog.d/README.md)), for every plugin you touched.
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
- **`drop-paths`** is a transform that leaves upstream files or folders out of the vendored tree: `{"name": "drop-paths", "paths": ["evals", "references/a.md"]}`, each path relative to the source folder. It fails when a path is not in the source, so an upstream rename cannot leave a file behind by accident. Use it, not a deletion hunk in a patch, which would copy the whole file into the patch and break on every upstream edit.
- **First-party knowledge** (`dashboard-copy`) is hand-written. No `sources.json` entry targets it, `anthology-dev vendor` never touches it, and `vendor check` ignores it. The folders are listed in `FIRST_PARTY_KNOWLEDGE` in `crates/repo-tests/src/repo.rs`, and `test_first_party_folders_are_hand_written` in `crates/repo-tests/tests/knowledge.rs` fails when a source targets one, and when a knowledge folder is neither pinned by a source nor on that list. A new hand-written folder goes on the list in the same change. Nothing checks hand-written text against an upstream license, so check the license of any third-party material before you use it, and keep its attribution in the folder (`dashboard-copy/SKILL.md` carries the GOV.UK and Carbon notices).
- **Support folders** (`blog`, listed in `SUPPORT_KNOWLEDGE`) are vendored but have no `SKILL.md` and no agent reads them. They hold shared references that other knowledge files link to.
- **`vendor/licenses/`** carries each source's license text, unchanged, refreshed by `fetch` and `sync`.
- **Licenses inside the plugin.** Claude Code installs a plugin directory on its own, so `build` also copies each source's license, unchanged, into its target folder (a source with no license file of its own gets the fixed text as `LICENSE`). An upstream `NOTICE` outside the source folder must be listed in the entry's `license_extra` (caveman ships `NOTICE` and `LICENSE-MIT` this way), or `build` fails. `check` verifies the copies byte for byte.

### Syncing atl

ATL lives in `atl/` as a git subtree with full history: the owner's fork merged with upstream (`rtk-ai/rtk`). Both pins are in the upstream `rtk-ai/rtk` entry of `sources.json`, whose `target` is `atl`. [`scripts/sync-atl.sh`](scripts/sync-atl.sh) moves to a new upstream release:

```sh
scripts/sync-atl.sh <upstream tag>
```

Give it an upstream release tag like `v1.2.3`. It refuses to run on a dirty tree. In order, it:

1. creates the branch `atl-sync/<tag>` from `HEAD`;
2. runs `git subtree pull` for that tag. Upstream edits to files the trim deletes (`engine/` and similar) are resolved by deleting them again. On any other conflict it prints the conflicted files and **stops**, leaving the branch for you to resolve;
3. records the tag and its commit as the upstream pin in `vendor/sources.json`;
4. runs `scripts/atl-trim.sh`;
5. runs `scripts/atl-test.sh`;
6. prints a summary.

The pin and the trim's changes are left uncommitted, for review with the branch. It never pushes and never merges.

What you do:

1. If it stopped on conflicts, resolve them so that both upstream's change and the fork's intent survive. If a fork change cannot coexist with upstream, upstream's design wins and the intent is re-implemented where feasible. Record the decision in `DECISIONS.md`. Then re-run the remaining steps by hand (the pin, `scripts/atl-trim.sh`, `scripts/atl-test.sh`).
2. `atl-trim.sh` removes upstream's BUSL-1.1 `engine/`, `plugin/` and `proxy/`, so that what ships is the Apache-2.0 edition, and keeps atl Claude Code only and free of telemetry. It is idempotent and stops with exit 1, naming the file and the anchor, when an edit's expected text is missing, which means an upstream refactor to look at. A removal whose target upstream renamed is not noticed that way; the allowlist tests below catch it. The allowlist test `crates/repo-tests/tests/atl_claude_only.rs` fails when upstream adds a hook integration module or a Cargo dependency that is not on the list. Adding one is a deliberate decision, not a test fix.
3. Each of the fork's eight remaining changes has a permanent named test (`fork_1_…` to `fork_7_…` and `fork_9_…`). They must still pass. The ninth, custom `[upstream.*]` headers, lived only in the removed BUSL engine, so its test is gone.
4. Review and commit the pin and the trim's changes, run the tests (`scripts/rust-test.sh`, as in the development setup), then open a PR.

#### Releasing atl binaries

1. Set the `atl/Cargo.toml` version to `<upstream version>-anthology.<n>` (for example `1.2.1-anthology.2`), update the `atl` package entry in `atl/Cargo.lock` to match (the builds use `--locked`), and merge that.
2. Push a tag `atl--v<version>` on the commit to release. `atl-release.yml` fails unless the tag equals `atl--v` plus the `Cargo.toml` version. It builds `aarch64` and `x86_64` for macOS (`apple-darwin`) and Linux (`unknown-linux-musl`), each on a native runner, with the toolchain in `atl/rust-toolchain.toml`, running `cargo test --locked` before each `cargo build --release --locked`. It attests the archives and publishes `atl-<target>.tar.gz` and `checksums.txt` to a new release; it fails rather than overwrite an existing release. To publish from an existing tag, run the workflow by hand with the tag as input.
3. Write the version, the tag and the sha256 of each target from `checksums.txt` into `plugins/anthology/atl.lock`. `crates/repo-tests/tests/atl_lock.rs` checks that the lock matches `Cargo.toml` and has a 64-hex hash for all four targets.
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
scripts/release.sh <plugin> <version> --push      # also push the branch and the tag, in one atomic push
                                                  # (--remote NAME picks the remote; default origin)
```

It refuses to run (and changes nothing) unless:

- the working tree is clean. Untracked files under `.claude/agent-memory/` and `.claude/memory/` do not count; any other untracked file, and any changed tracked file, does;
- `<version>` is higher than the current one, or equal to it when that tag does not exist yet (a first release);
- the tag does not exist;
- the plugin's `[Unreleased]` section in `CHANGELOG.md` has content, or `changelog.d/<plugin>/` holds at least one fragment with an entry;
- every other plugin that depends on this one has a version range that accepts `<version>`. If one does not, widen the range in that plugin's `plugin.json` first, which is its own change with its own bump;
- for a plugin that ships `anthology.lock`, `lock check-fresh` passes (see [Releasing the anthology binary](#releasing-the-anthology-binary)).

Its edits run through `scripts/dev.sh`, so it needs Docker, or `ANTHOLOGY_DEV` naming an `anthology-dev` binary. Then it bumps `plugin.json` (and `plugin_version` in `vendor/sources.json` for `anthology-superpowers`), moves the `[Unreleased]` entries, followed by the fragments in filename order, into a dated version section, deletes those fragments, commits as `release: <plugin> <version>`, and runs `claude plugin tag` (no push). With `--push` it then pushes the branch and the tag together with `git push --atomic`, so the remote gets both or neither. If that push fails, or anything else fails or is interrupted after the tag was made, it deletes the local tag again; after a failed push it also prints the commands to finish by hand. It never pushes without `--push`; without it, it prints the `git push --atomic` command to publish. The last line it prints is always `RELEASE OK <tag>` or `RELEASE FAILED: <step>`, where the step is `arguments`, `preconditions`, `edit`, `commit`, `tag` or `push`. A dry run ends with `RELEASE OK <tag> (dry run: nothing was released)`. `--dry-run` does all of this in a throwaway worktree, shows the commit and the CHANGELOG diff, and leaves your checkout, index and tags untouched.

A plugin's `version` is what moves installed users to new code, so a change that is not released does not reach them. Release a dependency before the plugin that needs its new version.

### Evals and the baseline

The behaviour evals in [`evals/`](evals/) catch a model update or a Claude Code update that silently breaks the product. Each case is `evals/<case>/case.yaml`. They sit at the repo root, because a case can only load plugins inside its containment root. Each case lists all three plugins as `../../plugins/anthology`, `../../plugins/anthology-guardrails` and `../../plugins/anthology-superpowers`. They cover:

- routing (the main session delegates to the right `anthology:*` agent);
- knowledge-read order (each agent reads its Startup files before any other tool);
- the G1, G4 and G6 blocks;
- BrowserOS-only browsing, against a mocked `browseros-neo` MCP server in `evals/mocks/` and with no Chrome launch;
- atl condensing `git status` output, and G1 still blocking a Playwright command that atl would rewrite;
- the routing guide being in the main session, and absent from agents;
- the platform checks recorded in [DECISIONS.md](DECISIONS.md) that a case can express (agent fields, memory paths, hooks reaching subagents, skills hidden from the main session).

The checks a case cannot express are smoke cases, run by `anthology-dev smoke` (`crates/anthology-dev/src/smoke/`): the Claude Code CLI syntax, the eval cost ceiling, the session-start output cap, a missing hook interpreter, a missing `anthology` binary (the shims fail closed and say why), a stalled guard, the worktree base, the model each agent runs on, atl's rewrite, and the knowledge-read, agent-memory-path, auto-memory-directory and memory-commit checks for writers. `cli-syntax` and `binary-missing` call no model.

Run everything from the repo root with `scripts/run-evals.sh`. It runs the evals, then the smoke cases, against a scratch copy of `plugins/` with a locally built `anthology` binary seeded in (`anthology-dev seed`). It builds that binary and `anthology-dev` with `cargo build --release --locked`, so a local run needs the Rust toolchain from `rust-toolchain.toml` on the host, unless `ANTHOLOGY_BIN` and `ANTHOLOGY_DEV` name binaries you built. Under the hood it calls one `claude plugin eval` with a fixed `--allow-tools` list:

```sh
claude plugin eval . --trust-plugin --ablation none --no-publish --threshold 0 --runs 1 \
  --max-cost-usd <ceiling> --json <result.json> --output-dir <dir> --allow-tools <grants>
```

A tool that is not granted is withheld from the model. The grants are the few harmless commands the guard and atl cases need (listed in the script), so a broken guard still cannot download anything; the guard and atl cases only work through the script, so don't run the bare command and read its failures as regressions. `--case GLOB` and `--tag TAG` run part of the suite (the comparison then tolerates missing cases, the smokes are skipped and `--record` is refused); `--no-smoke` skips the smokes. Results go to `evals/results/<UTC timestamp>/`.

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

**Conditional Startup lines.** A Startup line whose last path is followed by `` `, only when …``, `` `, only for …`` or `` `, only to …`` is conditional: the agent reads it only for that kind of job. Write it after all the always lines, in exactly this form, or `test_startup_lines_follow_the_grammar` fails:

```text
- `${CLAUDE_PLUGIN_ROOT}/knowledge/<folder>/SKILL.md` and `${CLAUDE_PLUGIN_ROOT}/knowledge/<other>/SKILL.md`, only when <the kind of job>.
```

The budget counts the body, every always file, the preloaded skills and the **largest conditional file**, not the sum of the conditional ones, because a brief needs one deliverable's files at a time. The smoke cases and the knowledge-read evals demand only the always files. The cost: a brief that spans several deliverables reads more than the budget says. Every conditional path still counts as read for the "every knowledge folder has a reader" check, so the tools still see it.

When a sync fails a budget:

1. Check what grew: compare the failing agent's measured size (in the failure message) with its budget, and look at what the sync changed in its Startup files.
2. If the growth is not worth it, trim the patch or drop the file from the agent's Startup list.
3. If it is worth it, raise that one number in `BUDGET`. The failure message gives the value to use, `ceil(measured * 1.15)`. Set it and explain the growth in the same commit and in the PR description. Never raise a budget to get a build green without that review.

### The upstream watcher

[`upstream-watch.yml`](.github/workflows/upstream-watch.yml) runs weekly and can be started by hand. It runs `anthology-dev upstream-watch` (`cargo run --locked --release -p anthology-dev -- upstream-watch` on the runner, because it needs the job's GitHub token), which compares every pin in `vendor/sources.json`, the upstream release of rtk (`rtk-ai/rtk`, which ATL forks) and the latest Claude Code release against upstream. For each change it opens one GitHub issue, labelled `upstream-watch`, with a summary of the diff and the command to run (`scripts/dev.sh vendor sync …` or `scripts/sync-atl.sh …`). It updates an existing issue when the target moves again, and never closes one: close it when the sync merges. It never changes the repo itself.

A new Claude Code release also starts the evals. To bump the Claude Code that CI validates with, set the version in `ci/claude-code/package.json`, run `npm install --package-lock-only` in `ci/claude-code`, and let `plugin-validate` and the evals run on the PR.

To see what it would report without touching GitHub:

```sh
scripts/dev.sh upstream-watch --dry-run
```
