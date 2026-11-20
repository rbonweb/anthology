# Anthology in Rust: design

- **Date:** 2026-11-20
- **Status:** Proposed (architect), **revision 2**. Revision 2 applies the independent review `.superpowers/sdd/RUST/plan-review.md` and the controller's rulings on it. §18 maps each finding to where it is fixed.
- **Decisions:** the owner's binding decisions are in §1.2. Every other choice is marked **architect's call**, with its reason and its cost if wrong. No owner decision is open.
- **Baseline:** `build` at `fedabad2`. The port branches from `build` once fix lane F1 (`fix-f1`: rtk `anthology_gate.rs`) has merged (§15.0).
- **Companion plan:** [`docs/superpowers/plans/2026-11-20-rust-port.md`](../plans/2026-11-20-rust-port.md). Its *Global Constraints* (GC-1 to GC-19) list every value kept verbatim. **The plan is authoritative for lane facts** (ids, owners, dependencies, waves). This design gives the reasons.

---

## 1. Intent and goals

### 1.1 Intent

Anthology should be maintained in one language, Rust. All first-party Python (~12k lines of product and tooling, ~19k lines of tests) is replaced by Rust that behaves the same, ships as one attested binary, and is proven equivalent by a differential oracle before any Python is deleted.

### 1.2 The owner's binding decisions

1. **One language.** All of the following are ported:
   - `plugins/anthology/scripts/*.py` (14 modules, 5.4k lines);
   - `plugins/anthology-guardrails/scripts/*.py` (1.5k lines);
   - `scripts/*.py` (8 tools, 4.0k lines);
   - `evals/smoke/smoke.py` (966 lines);
   - `tests/*.py` (35 test modules and 4 helpers, 19.1k lines, 1,513 `def test_`; the last full run was 1,445 OK with 152 skipped).
2. **Vendored content is untouched.** `plugins/anthology-superpowers/**` keeps its six upstream `.py` files. `plugins/anthology/knowledge/**` has none. `vendor check` keeps both trees byte-identical to upstream.
3. **Delivery.** One `anthology` binary, built, attested and released from this repo's GitHub releases the way rtk is (ADR-017). A tiny POSIX `sh` bootstrap downloads it at the first session and checks it against a pinned sha256. No Python runs at runtime.
4. **Fail closed.** Until the guard binary is installed, the `PreToolUse` guard blocks shell commands, file writes and edits, and MCP tool calls, with a "run /anthology:setup" message. Read-only tools are allowed. This supersedes the fail-open decision in ADR-010 and SECURITY.md:71.
5. **Targets.** The same four as rtk: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`.

### 1.3 Goals

- **G-1 Behaviour parity.** Every hook, command, file write, exit code and message matches the Python at the baseline. The only exceptions are the forced changes in plan GC-15, F-1 to F-20.
- **G-2 One shell parser for security decisions.** The guard and rtk's Anthology gate decide from the same parser and the same "what actually runs" expansion. Disagreement between two parsers has been the main source of guard bypass findings.
- **G-3 Nothing dropped silently.** Every Python test id maps to a Rust test or to a reviewed drop reason, in a machine-checked ledger.
- **G-4 Fail closed, loudly and recoverably.**
  - A missing, corrupt, wrong-architecture or crashing guard binary blocks every non-read-only tool and explains the fix.
  - A binary that fails to run is set aside and reinstalled automatically.
  - Every block message also names the escape that always works: disable anthology-guardrails with `/plugin`.
- **G-5 Same budgets.**
  - Session-start context at most 9,000 characters (platform cap 10,000); `systemMessage` at most 2,000.
  - The session-start hook finishes well inside its 60 s timeout, even on a first session that downloads the binary (§8.7).
  - Agent startup byte budgets are unchanged.
  - The guard's timing tests (0.2 s and 0.5 s) and the privacy scan's linear-scaling test still hold.
- **G-6 Minimal, audited dependencies.** Everything is pinned in `Cargo.lock` and gated by `cargo deny` in CI.

---

## 2. Non-goals

- **No behaviour changes**, except:
  - the fail-closed guard;
  - the removal of Python and every message that names it;
  - changes the port forces. Plan GC-15 lists each one, old → new, with the reason.
- **No new features.** No new rules, settings ops, memory formats or user commands, except the plumbing the shims need: `anthology guard hook`, `anthology hook session-start`, and the bootstrap's own CLI.
- **No change to vendored trees** or to `vendor/sources.json` semantics.
- **No change to rtk's user-facing behaviour.** The one rtk change is lane RG, which moves the fork gate (`anthology_gate.rs`) onto the shared parser. Its verdicts may only get stricter. rtk keeps its own binary, lock and release.
- **No Windows support.** Windows stays refused. With fail-closed, a Windows user who enables anthology-guardrails can still use read-only tools, and everything else is blocked with a message naming `/plugin` (F-18, §9.6).
- **The bash/sh orchestration scripts stay shell**, with no Python calls: `release.sh`, `run-evals.sh`, `rtk-test.sh`, `rtk-net-test.sh`, `rtk-trim.sh`, `sync-rtk.sh`. **Architect's call:** they are glue around docker, git and `claude`, not logic. Cost if wrong: porting them later, each under 300 lines.

---

## 3. What is ported (inventory)

| Python (baseline) | Lines | Rust home | Called by |
|---|---|---|---|
| `anthology-guardrails/scripts/guard.py` | 38 | `anthology-guard::hook` → `anthology guard hook` | guardrails `PreToolUse *` |
| `guard_rules.py` lexer + expansion (:48-911) | ~860 | **`anthology-shell`** (shared with rtk) | guard, rtk gate |
| `guard_rules.py` rules G1–G6 and paths (:914-1461) | ~550 | `anthology-guard::rules` | guard |
| `state.py` | 52 | `anthology-core::state` | all |
| `compat.py` | 112 | `anthology-runtime::compat` | session start, doctor |
| `guardrails_loc.py` | 225 | `anthology-runtime::guardrails_loc` | session start, setup |
| `settings_patch.py` | 687 | `anthology-settings` | setup, session start, doctor |
| `doctor.py` | 368 | `anthology-runtime::doctor` → `anthology doctor` | setup preview |
| `rtk_install.py` | 389 | `anthology-runtime::rtk_install` → `anthology rtk-install` | session start, setup, doctor |
| `session_start.py` + `session-start.sh` | 332+15 | `anthology-runtime::session_start` → `anthology hook session-start` | anthology `SessionStart` |
| `anthology_setup.py` | 487 | `anthology-runtime::setup` → `anthology setup …` | `skills/setup` |
| `memory_privacy.py` | 293 | `anthology-memory::privacy` | sync, portable, save |
| `memory_sync.py` | 1134 | `anthology-memory::{notes,repo,approve,render,sync,save}` | session start, setup, index, portable |
| `memory_index.py` + `bin/anthology-mem` | 673 | `anthology-memory::index` → `anthology mem …` | agents, routing guide |
| `memory_portable.py` | 684 | `anthology-memory::portable` → `anthology memory export\|import` | `skills/export`, `skills/import` |
| `scripts/vendor.py` | 1404 | `anthology-dev vendor …` | CI, maintainers |
| `scripts/check_version_bump.py` | 210 | `anthology-dev version-bump` | CI |
| `scripts/release_edit.py` (+ `release.sh`) | 401 | `anthology-dev release-edit …` | `release.sh` |
| `scripts/memory_diff.py` | 118 | `anthology-dev memory-diff` | CI |
| `scripts/startup_budget.py` | 173 | `anthology-dev startup-budget` | CI, upstream watch |
| `scripts/eval_baseline.py` | 242 | `anthology-dev eval-baseline …` | `run-evals.sh` |
| `scripts/upstream_watch.py` | 819 | `anthology-dev upstream-watch` | `upstream-watch.yml` |
| `scripts/rtk_trim.py` | 649 | `anthology-dev rtk-trim` | `rtk-trim.sh`, `sync-rtk.sh` |
| `evals/smoke/smoke.py` | 966 | `anthology-dev smoke` | CI `cli-syntax`, `run-evals.sh` |
| `tests/test_*.py` | ~18.8k | `crates/*/tests/`, `crates/repo-tests` | CI |
| `tests/_repo.py` | 90 | `repo-tests::repo` | meta tests |
| `tests/_mem.py` (`MemCase`, `git`, `note_text`, `snapshot`, `needs_git`) | 120 | `anthology-testkit::mem` | memory, setup tests |
| `tests/_rtk.py` | 51 | `anthology-testkit::release` | rtk install, shims |
| `tests/_agents.py` (`make_repo`) | 37 | `anthology-dev` test support (`tests/support/agents.rs`) | startup-budget, upstream-watch tests |

---

## 4. Approaches considered

### 4.1 Sharing one shell parser with rtk (the biggest choice)

Recon facts:
- rtk's lexer (`rtk/src/discover/lexer.rs`) is **flat**: tokens with offsets, quotes kept literal, no recursion into `$()` or backticks, no heredoc bodies. `decision.rs` **defers** on exactly those constructs (lexer.rs:517-559).
- The guard needs a structural parse that recurses into substitutions, heredocs, `sh -c`, `eval`, rtk runner forms, `find -exec`, docker shells and `cd` tracking.
- rtk's modules are private; `rtk/src/lib.rs:1-10` makes only `pub fn entry()` public.
- The F1 gate (`fix-f1:rtk/src/hooks/anthology_gate.rs`, 3,353 lines) adds a third partial recursion, and that recursion can disagree with the guard.

| | A. `anthology` depends on rtk and uses rtk's lexer | **B. New crate `anthology-shell`; the guard uses it and the rtk fork depends on it** | C. Two parsers plus a shared conformance corpus |
|---|---|---|---|
| One parser for security decisions | No. The lexer cannot see the constructs the guard must open. | **Yes**: `analyze()` is the single "what runs" answer. | No. Disagreement is reduced, not removed. |
| rtk fork drift | High: `pub` re-exports in `lib.rs` (rtk_trim anchors), and rtk's `[workspace]` removed. | **Low**: one `[dependencies]` line under `# anthology fork:`. | None. |
| Weight | All of rtk's tree in the guard. | `regex` + `fancy-regex` only. rtk already depends on `regex`. | Same as B. |
| Fidelity | Rewrite; ~1,150 rows at risk. | **Direct port**, proven by the difftest. | Same as B for the guard. |

**Choice: B (architect's call).** Cost if wrong: if an upstream rtk restructure makes the path dependency painful, fall back to C. The guard keeps `anthology-shell`, and the gate keeps a conformance test against it.

### 4.2 Where the binary lives and who installs it

| | **A. Shared, versioned `${CLAUDE_CONFIG_DIR:-~/.claude}/anthology/bin/<version>/`; each plugin ships the same lock and bootstrap** | B. Per-plugin `${CLAUDE_PLUGIN_DATA}` | C. Fold rtk into `anthology` (multi-call) |
|---|---|---|---|
| Guardrails installed alone | Works; the path comes from the lock only. | Works, but two downloads when both plugins are installed. | — |
| Depends on `CLAUDE_PLUGIN_DATA` | No. | Yes, on every guard call. | — |
| Cleanup | `remove` deletes it when guardrails is not installed; otherwise it is pruned after 30 days unused. | Claude Code deletes it on uninstall. | — |
| Behaviour change | None beyond the new binary. | None. | **Yes**: rtk path, doctor, lock and release cadence. |

**Choice: A (architect's call).** Cost if wrong: about 6 MB per version left behind after an uninstall that skips `remove`, until it is pruned.

### 4.3 How fail-closed is enforced

| | **A. `sh` shim decides when the binary is missing or broken; `onFailure: "block"` as backstop** | B. `onFailure: "block"` only | C. Separate matchers for read-only and other tools |
|---|---|---|---|
| Read-only tools when missing | Allowed (§9.3 key scan). | All blocked, including `Skill`, so `/anthology:setup` cannot load. | Needs negative lookahead in matchers, which is undocumented. |
| Our message reaches the model | Yes: stderr with exit 2. | No: generic "failed; blocking because onFailure is block". | Yes. |
| Broken installed binary | Quarantined and reinstalled (§9.2). | Blocked forever. | Blocked forever. |

`onFailure` is verified in the 2.1.311 bundle (review "verified"): a `non_blocking_error` (exit other than 0 or 2, or a missing executable) and a timeout both become blocks on `PreToolUse`. A user abort does not.

**Choice: A (architect's call).** Cost if wrong: the key scan mis-reads a payload. It errs toward blocking, so the cost is a spurious block of a read-only tool until the binary installs.

### 4.4 Python-semantics parity layer

| | **A. In-tree `anthology-pycompat`** | B. serde_json, the regex crate and so on, accepting the differences | C. A C library (PCRE2 or CPython-derived) |
|---|---|---|---|
| Guard input with a lone surrogate, NaN or deep nesting | Parsed like `json.loads`. Deep nesting is accepted without a recursion limit, which is stricter (§9.5). | serde_json **rejects** these → exit 0 → **allowed (bypass)**. | — |
| `json.dumps` byte parity | Exact. | Separators, escapes and float repr differ. | — |
| `\s` `\w` `.lower()` `.isdigit()` strerror | Tables generated from CPython 3.12 and per-OS libc. | Differ. | Heavy, and still not CPython. |

**Choice: A (architect's call).** Cost if wrong: about 3k lines in-tree, mostly generated tables. The difftest proves them, and the golden files keep them correct after cutover.

### 4.5 The oracle after Python is gone

**Choice: a live differential run during the port, frozen into golden files at cutover (architect's call).**
- A pinned Python kept in CI forever was rejected, because it violates §1.2.
- A differential-only harness was rejected, because the protection is lost at cutover.

Cost if wrong: golden updates must be blessed deliberately (`anthology-dev golden bless`), which makes each one a visible diff.

### 4.6 Validation on real traffic before cutover

| | **A. Replay of the owner's local transcripts (I-1)** | B. 7-day live shadow run (revision 1) |
|---|---|---|
| Coverage | Every recorded tool call, including subagents. Deterministic and re-runnable after every merge. | Only new calls during the week. |
| Depth | Compares `analyze()` entries and privacy findings, not only verdicts. | Verdicts only. |
| Calendar time | None. | 7 days. |

**Choice: A** (controller's settled override, adopted). Its limit, stated in §13.7: the replay proves parity on real traffic, not bypass resistance. The generators and monotonicity properties prove bypass resistance.

---

## 5. Workspace and crates

```
Cargo.toml                 # [workspace] members = ["crates/*"], exclude = ["rtk", "fuzz"]; ALL deps declared in [workspace.dependencies] by F1
Cargo.lock                 # committed; --locked everywhere; regenerated only by the integrator
rust-toolchain.toml        # channel = "1.100.0" (channel must equal rtk/rust-toolchain.toml's; targets may differ) + musl targets
deny.toml                  # cargo-deny policy (§11)
crates/
  anthology-pycompat/      # json, str (+ generated Unicode tables), re (translation to regex/fancy-regex syntax), difflib, fnmatch,
                           # quote, path, pyerr (+ generated per-OS errno tables), ipaddress, shlex, argparse rules
  anthology-shell/         # structural shell lexer + analyze(); deps: regex, fancy-regex only; SELF-CONTAINED Cargo.toml (no
                           # workspace inheritance) because rtk builds it too; SHARED WITH rtk
  anthology-guard/         # data (PROFILE_ROOTS, PREFIX_VARIANTS, SELF_TEST_SAMPLE), secrets, G1–G6, check(), hook entry
  anthology-core/          # Env, paths, roster, state.json, fsx, git, platform, lock files, clock, sys (only unsafe/libc)
  anthology-settings/      # settings_patch
  anthology-memory/        # privacy, notes, repo, approve, render, sync, save, index (rusqlite bundled), portable
  anthology-runtime/       # guardrails_loc, compat, doctor, rtk_install, fetch, bootstrap caller, session_start, setup
  anthology-cli/           # [[bin]] anthology: argv dispatch, Env and HOME resolution, the only std::env reader
  anthology-testkit/       # dev-only; per-lane helper modules
  anthology-dev/           # [[bin]] anthology-dev: repo tooling, ledger, replay, golden (NOT shipped)
  repo-tests/              # meta tests, shim tests, cutover tests
  difftest/                # port-time differential harness (deleted at cutover)
fuzz/                      # cargo-fuzz targets (own workspace, pinned nightly)
port/                      # ledger/<module>.tsv, python-test-ids.txt, staging/, difftest/{oracle,divergences}/, replay-reports/
```

**Dependency graph** (an arrow means "depends on"):
- `cli` → runtime, memory, guard, core
- runtime → settings, memory, core, guard (data only)
- settings → core, pycompat, **guard (data only: `PROFILE_ROOTS`, `PREFIX_VARIANTS`)**
- memory → core, pycompat, guard (`secrets` only)
- guard → shell, pycompat
- shell → regex, fancy-regex
- rtk (fork) → shell

`anthology-guard` does not depend on core. `HOME` is resolved by `anthology-cli` through `core::sys`, with the passwd fallback, and passed in.

**Profile (architect's call):**
- `[profile.release]`: `lto = "thin"`, `codegen-units = 1`, `strip = true`, `panic = "unwind"`. Unwind is needed so the guard can catch a panic and block (§9.5).
- macOS builds use `MACOSX_DEPLOYMENT_TARGET=11.0`; Linux builds are static musl.
- Expected size is about 6 MB.

Cost if wrong: download size only.

**Crate borders follow lane borders (architect's call).** Shared files are created once, by foundation task F1, and lanes never edit them (plan §0.2). Cost if wrong: some cross-crate plumbing.

---

## 6. Binary and CLI surface

### 6.1 Every Python entry point and hook mapped

| Today | Rust | Wrapper or hook | Contract |
|---|---|---|---|
| guardrails `PreToolUse *`: `python3 scripts/guard.py` | `anthology guard hook` | `sh ${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh`, `"timeout": 60`, `"onFailure": "block"` | stdin is hook JSON. Exit 0 allows; exit 2 blocks with stderr `Blocked by anthology-guardrails (<id>): <msg>\n` and empty stdout. |
| *(new)* guardrails `SessionStart` | — | `sh ${CLAUDE_PLUGIN_ROOT}/hooks/bootstrap.sh session --plugin guardrails` | Installs the pinned binary. Prints one `systemMessage` JSON on failure, otherwise nothing. Always exits 0. |
| anthology `SessionStart`: `sh scripts/session-start.sh` → `session_start.py` | `anthology hook session-start` | `sh ${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh` (`timeout: 60`) | The shim bootstraps first, then runs the binary with `ANTHOLOGY_HOOK_T0` and `ANTHOLOGY_BOOTSTRAP_DOWNLOADED`. Prints one JSON line. Always exits 0. |
| anthology `PreToolUse Bash`: `bin/rtk-hook`; `bin/rtk` | unchanged | unchanged | `rtk_path` stays on its own line in state.json. |
| `bin/anthology-mem` → `memory_index.py` | `anthology mem <search\|related\|orphans\|stale\|rebuild\|save>` | `bin/anthology-mem` (sh) | Same flags, outputs and exit codes; prog name `anthology-mem`. |
| `skills/setup`: `anthology_setup.py {preview,apply,remove}` | `anthology setup {preview,apply,remove}` | `"${CLAUDE_PLUGIN_ROOT}/bin/anthology" setup …` | Same flags; JSON with indent 2; exit 0, 1 or 2. |
| `skills/export`, `skills/import`: `memory_portable.py` | `anthology memory export\|import` | `bin/anthology memory …` | Same. |
| `doctor.py [--json]` | `anthology doctor [--json]` | — | Exit 0, or 2 if the platform is unsupported. |
| `rtk_install.py …` | `anthology rtk-install …` | — | Same. |
| `import guard_rules` (sample, roots, variants) | compiled-in `anthology_guard::data` | — | §6.3 |
| *(new)* | `anthology --version` | — | `anthology <version> (<target>)` |

`bin/anthology` is a new sh wrapper on `PATH`. It runs the binary pinned by **its own** plugin's lock, and runs the bootstrap in `wrapper` mode if that binary is missing.

### 6.2 Argument parsing

Argument parsing uses `lexopt` plus a `pycompat::argparse` rules layer: `--a b` and `--a=b`, unique-prefix abbreviation where Python allowed it (none for `mem save`), repeated `--promote`, and exit 2 on a usage error. Usage text is normalized by the difftest (D-4). **Architect's call.** Cost if wrong: drift in an abbreviation someone relied on, which the ported argparse tests catch.

### 6.3 Guard data compiled in

`SELF_TEST_SAMPLE`, `PROFILE_ROOTS` and `PREFIX_VARIANTS` are constants in `anthology_guard::data`. `ROSTER` is in `anthology_core::roster`. **Foundation task F1 creates all four, filled in**, so settings (wave 1), compat and doctor can use them before the guard lane exists (I-2).

`guardrails_loc` still finds the live guard: the self-test runs **its** `hooks/guard.sh`. **Architect's call.** Cost if wrong: a guardrails plugin pinned to an older binary has a shorter profile list than the deny rules setup writes. The lists only grow, so the result is stricter, never looser.

---

## 7. Shared parser with rtk

### 7.1 Crate contract (`crates/anthology-shell`)

- **Port scope.** A direct port of `guard_rules.py:48-911`: `_Lexer`, `_scan_paren`, `_find_heredoc_end`, `_lenient_segments`, `_strip_prefixes`, `_rtk_command`, `_parse_shell_args`, `_add_command`, `_collect`, `_collect_find_exec`, `_reads_stdin_as_shell`, `_echo_text` and `_analyze`. The limits keep their values. The types are in plan §3.4.
- **Dependencies:** only `regex` and `fancy-regex`. Lookaround and backreference patterns use `fancy-regex`; the lexer's `_HEREDOC_START` has both `(?!<)` and `\1`. `#![forbid(unsafe_code)]`.
- **Self-contained `Cargo.toml`.** No `*.workspace = true` inheritance and explicit versions, because rtk's workspace builds it too. A CI check builds `--manifest-path crates/anthology-shell/Cargo.toml` from inside `rtk/` (m-15).
- **`analyze(command, home) -> Analysis`.**
  - `entries: Vec<Entry>`. Each `Entry` mirrors Python's `_Entry` field for field (`raw`, `redirs`, `prog_path`, `prog`, `args`, `cwd`, `exempt`, `group`, `code`).
  - Plus the additive `opaque: Vec<Opaque>` (constructs the expansion could not see into) and `incomplete: bool` (a regex backtrack limit was hit anywhere, I-9).
  - The guard ignores `opaque`, which keeps parity, but **blocks when `incomplete`** (§9.5). The rtk gate asks on either.

### 7.2 rtk side (lane RG)

- **`rtk/Cargo.toml`.** One line at the end of `[dependencies]`, under `# anthology fork: shared shell parser`: `anthology-shell = { path = "../crates/anthology-shell" }`.
  - rtk keeps its own `[workspace]` and `Cargo.lock`. The lock gains the path package plus `fancy-regex` and `bit-set`/`bit-vec`; `regex` is already there.
  - `rtk_trim.py --lock-guard` gains an allowlist for path packages under `../crates/` and their dependency closure (m-14). RG's acceptance simulates a subtree sync.
- **`anthology_gate.rs`.** Replaces its own recursion with `anthology_shell::analyze`. It keeps its rule sources, `Read()` logic, work budget, and its "opaque behind rtk → ask" policy, now extended to `incomplete`.
- **Verdict policy: only stricter.** Over the gate's tests plus the guard corpus, `new ≥ old` in the order deny > ask > defer/allow.
- **Cross-check test `rtk/tests/anthology_shell_rtk_forms_test.rs`.** It pins rtk's clap subcommand list, so a new upstream subcommand forces a decision about `RTK_PASSTHROUGH`/`RTK_RUNNERS`. The file is not named `fork_*`, so `FORK_TESTS` is untouched.
- **CI.** `rtk-test.sh` and `rtk-net-test.sh` mount the repo root. A path filter runs `rtk-test` and the RG monotonicity corpus whenever `crates/anthology-shell/**` changes.

### 7.3 What stays separate

rtk's upstream `discover::lexer` and `registry` keep doing **rewriting**, which is not a security decision. Both hooks judge the **original** command (ADR-008). RG is **not** on the cutover critical path.

---

## 8. Bootstrap and installation

### 8.1 Files shipped in each plugin

| Plugin | Added files |
|---|---|
| `anthology` | `anthology.lock`, `hooks/{lib.sh,bootstrap.sh,session-start.sh}`, `bin/anthology`, `bin/anthology-mem` (rewritten) |
| `anthology-guardrails` | `anthology.lock`, `hooks/{lib.sh,bootstrap.sh,guard.sh}` |

- `hooks/lib.sh`, `hooks/bootstrap.sh` and `anthology.lock` are byte-identical across the two plugins at `HEAD`, test-enforced.
- The hook shims live in `hooks/`, so they are **not on `PATH`**.
- `plugins/anthology/scripts/` is deleted at cutover.

### 8.2 `anthology.lock`

The format is rtk.lock's: JSON with exactly these keys, in this order, one per line, written only by `anthology-dev lock write`. The fixed line layout lets `sh` read it with `sed`.

```json
{
  "version": "0.1.0",
  "tag": "anthology-bin--v0.1.0",
  "repo": "rbonweb/anthology",
  "sha256": {
    "aarch64-apple-darwin": "<64 hex>",
    "x86_64-apple-darwin": "<64 hex>",
    "x86_64-unknown-linux-musl": "<64 hex>",
    "aarch64-unknown-linux-musl": "<64 hex>"
  }
}
```

**Validation** (`sh` and Rust; plan GC-17):
- `version` matches `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$`;
- `tag` equals `anthology-bin--v` + version;
- `repo` matches `^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$` with no `.` or `..` parts;
- each hash matches `^[0-9a-f]{64}$`.

Anything else counts as an **invalid lock**.

The tag prefix cannot collide with `anthology--v*` plugin tags or `rtk--v*`. The URL is `https://github.com/<repo>/releases/download/<tag>/anthology-<target>.tar.gz`.

### 8.3 Install location and layout

```
<ah> = ${CLAUDE_CONFIG_DIR:-$HOME/.claude}/anthology
<ah>/bin/
  <version>/anthology        # 0755 regular file
  <version>/.bin-sha256      # "<sha256 of the extracted binary>\n" (written before .sha256)
  <version>/.sha256          # "<tarball sha256>\n", written LAST (install marker)
  <version>/.last-used       # touched by SessionStart bootstraps
  .broken-<version>-<epoch>/ # a quarantined install (§9.2), pruned after 24 h
  .tmp-XXXXXX/               # per-attempt staging, removed on exit
  .attempt                   # rate-limit stamp, created with noclobber (§8.4)
  anthology-<target>.tar.gz  # optional, user-seeded for offline installs
```

- **"Installed" (guard hot path).** `<version>` is a real directory, `anthology` is a regular executable, and `.sha256` is non-empty. The hot path does not hash anything; it costs one `sed` and three `test` calls.
- **"Current" (bootstrap, doctor).** Installed, plus `.sha256` equals the lock's hash for this target, plus `.bin-sha256` equals the sha256 of `anthology`.
  - If the binary hash differs, the install is quarantined (renamed to `.broken-…`) and reinstalled (C-1).
  - Doctor reports the mismatch in its `anthology_binary` check.

**Architect's call:** integrity is verified at session start, not per call. Cost if wrong: a binary corrupted mid-session runs until it fails (and is then quarantined, §9.2) or until the next session start.

### 8.4 The bootstrap (`hooks/bootstrap.sh`, POSIX sh)

**CLI (m-3):** `bootstrap.sh <session|guard|wrapper> --plugin <anthology|guardrails> [--quiet] [--budget SECONDS]`

**Modes:**
- `session`: SessionStart. Never rate-limited, and **never writes `.attempt`**, so it cannot delay a later retry (C-1.6).
- `guard`: `guard.sh` self-heal.
- `wrapper`: `bin/*`.
- Both `guard` and `wrapper` are rate-limited through `.attempt`.

**Output:**
- `--quiet`: exactly one stdout line `status=present|installed|failed:<reason>`, with exit 0 for present or installed and 1 for failed.
- Without `--quiet` in `session` mode: nothing on success, and the `--plugin`'s GC-14 `systemMessage` JSON on failure. Exit 0.
- `--plugin` selects the message text, so one byte-identical script serves both plugins.

**Steps:**
1. **Platform.** Map `uname -s`/`uname -m` to the target (the rtk table). Anything else is `unsupported platform <os>/<arch>`.
2. **Lock.** Validate per §8.2; a failure is `anthology.lock is not valid`.
3. **Current?** If the install is current, touch `.last-used` and report `present`. If the binary hash mismatches, quarantine it and continue.
4. **Rate limit** (`guard` and `wrapper` modes only):
   - If `.attempt` is newer than 60 s (`find "$dir" -name .attempt -mmin -1`), stop with `failed:rate-limited`.
   - Otherwise create a fresh stamp under noclobber: `rm` a stale stamp, then `set -C; : > .attempt`. If creation fails because a parallel call just created it, report `rate-limited` too (m-4).
5. **Get the archive** within the budget. The default `--budget` is `BOOTSTRAP_BUDGET` = **20 s** of wall time, including connect. First available wins:
   1. a seeded `<ah>/bin/anthology-<target>.tar.gz`;
   2. **curl:** `curl -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 10 --max-time $B --max-filesize 67108864 -A anthology-bootstrap -o "$tmp/a.tgz" "$url"`;
   3. **GNU wget** (detected by `wget --version 2>/dev/null | grep -q 'GNU Wget'`): `wget --https-only -q -T 10 -U anthology-bootstrap -O … "$url"` under the **watchdog**;
   4. **BusyBox wget** (detected by `wget --help 2>&1 | grep -q BusyBox`): `wget -q -T 10 -O … "$url"` under the watchdog. There is no `--https-only`; the https URL plus the sha256 pin decide what is accepted, so an http redirect cannot change the accepted bytes;
   5. **node** (architect's call; it covers `node:*-slim` hosts where Claude Code was installed with npm): `node -e '<fixed inline https.get script>' "$url" "$tmp/a.tgz"`. The script is https-only on every redirect, allows at most 5 redirects, caps the size at 64 MiB and has its own total timeout. It also runs under the watchdog.
   6. If none applies, the reason is `no downloader (curl, wget or node)`. The message then gives the exact manual install command (GC-14).

   The **watchdog** is POSIX: `"$@" & p=$!; ( sleep "$B"; kill -9 "$p" ) 2>/dev/null & w=$!; wait "$p"; rc=$?; kill "$w" 2>/dev/null`. It enforces a real total timeout, because wget's `-T` is per read (I-12).
6. **Hash.** Use `sha256sum`, else `shasum -a 256`, else `openssl dgst -sha256 -r`; with none, the reason is `no sha256 tool`. A mismatch with the lock is `sha256 does not match anthology.lock`. A seeded file is never deleted.
7. **Extract.** `tar -xzf … -C "$tmp/x" anthology`. The result must be a regular file and not a symlink, else `the archive has no anthology binary`.
8. **Place.**
   1. `mkdir -p <version>`, refusing a symlink;
   2. `mv` the binary in;
   3. write `.bin-sha256` (temp file + `mv`);
   4. write `.sha256` **last**.

   Any write failure is `cannot write the install folder`. `$tmp` is removed by a trap. Concurrent installs each use their own `mktemp -d` and rename atomically.

**Tooling:** POSIX plus `uname`, `sed`, `find`, `mktemp`, `tar` and gzip, one sha tool, and one of curl, wget or node. CI runs `shellcheck -s sh` and runs the shims on **Alpine (BusyBox ash + BusyBox wget), Debian slim (dash, no curl or wget, with node), Ubuntu (dash, curl, GNU wget) and macOS (`/bin/sh`, curl)** (I-12).

### 8.5 Upgrades, pinning and binary freshness

- **Pinning.** Exact version plus a per-target tarball hash; never "latest". An upgrade or downgrade is a plugin upgrade or downgrade.
- **Plugin update.** The new lock names a new `<version>`, which is installed beside the old one. Running sessions keep their own version directory.
- **Prune** (Rust session start): delete `<version>` directories not pinned by either live lock and unused for 30 days; `.tmp-*` and `.broken-*` older than 24 h.
- **Freshness (I-6).** `release.sh` runs `anthology-dev lock check-fresh` before a plugin release. It compares the **shipped set** between the lock's tag commit and `HEAD`. The shipped set is:
  - the `src/` files and `Cargo.toml` of every workspace package in `cargo tree -p anthology-cli -e normal,build --prefix none` (excluding `tests/`, `benches/`, `examples/`);
  - the `Cargo.lock` entries of the packages in that graph;
  - `rust-toolchain.toml`;
  - the root `[profile.release]` and the `[workspace.dependencies]` entries that graph uses.

  Test-only, dev-tool, difftest and golden changes do not force a binary release. CO2 must not touch the shipped set, which also guarantees the binary validated by the replay is the one that ships.

### 8.6 Offline first run

- The bootstrap fails and its `systemMessage` names the reason.
- The guard fails closed: read-only tools work. Each blocked call retries at most once a minute (§9.2), so restored connectivity heals the session.
- **Air-gapped and no-downloader installs:** copy `anthology-<target>.tar.gz` from the release into `<ah>/bin/`. It is verified against the same pin. The GC-14 "no downloader" message prints the exact URL and folder.

### 8.7 Each plugin alone, both together, and the session-start time budget

- **`anthology` alone:**
  - its session start bootstraps and runs;
  - the self-test gives the existing guardrails messages ("…is not installed…" or "…is disabled…");
  - nothing fails closed.
- **`anthology-guardrails` alone:** its own SessionStart bootstraps; the guard runs or fails closed. The messages name "start a new session while online" and `/plugin`, both of which work without the anthology plugin.
- **Both:** each plugin's SessionStart hook is its own process, with its own 60 s timeout. They run in parallel.

**One download per session-start hook (I-7; plan GC-7).** Worst case: about 20 s download + 2 s hash and extract + 10 s self-test, then the rtk step is skipped (below), all well inside the 50 s `DEADLINE` and the 60 s hook timeout.

1. `hooks/session-start.sh` records `ANTHOLOGY_HOOK_T0` and runs `bootstrap.sh session --plugin anthology --quiet --budget 20`.
   - `present` → it runs the binary with `ANTHOLOGY_BOOTSTRAP_DOWNLOADED=0`.
   - `installed` → `=1`.
   - `failed` → it prints the anthology GC-14 `systemMessage` and exits 0. There is no Rust session start, so there is no self-test message (m-21).
2. Rust session start ensures the **live guard's** version only when no download has happened in this hook (`ANTHOLOGY_BOOTSTRAP_DOWNLOADED=0`) and the remaining time allows it:
   - `budget = min(20, DEADLINE − elapsed_since_T0 − GUARD_TIMEOUT − 5)`, skipped when the budget is below 20;
   - it runs `bootstrap.sh session --plugin guardrails --quiet --budget <budget>`.
   - Otherwise it is deferred to the guardrails plugin's own SessionStart hook, the guard shim's self-heal, or the next session.
3. **rtk.** Elapsed time for the rtk check counts from `T0`. A session start that downloaded a binary therefore usually skips the rtk check, with the existing message, and rtk reconciles at the next session (F-20).
4. **Duplicate messages (m-21).** At most two messages appear on an offline first session: the anthology bootstrap's and the guardrails bootstrap's. They say different things, routing guide off and guard blocking. This is accepted and documented in GC-14.

### 8.8 Setup and remove

- **`setup apply`.** Ensures the guardrails-pinned binary through `bootstrap.sh session --plugin guardrails --quiet --budget 60`. The result gains `guard_binary: {status}`.
- **`setup remove`.** Deletes `<ah>/bin/` only if no `anthology-guardrails@*` entry is in `installed_plugins.json`. Otherwise it reports `kept (anthology-guardrails still uses it)`.

---

## 9. The fail-closed guard

### 9.1 Hook registration (guardrails `hooks/hooks.json`)

```json
{"hooks": {
  "SessionStart": [{"hooks": [{"type": "command", "command": "sh",
      "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/bootstrap.sh", "session", "--plugin", "guardrails"], "timeout": 60}]}],
  "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "sh",
      "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh"], "timeout": 60, "onFailure": "block"}]}]
}}
```

### 9.2 `hooks/guard.sh` decision table (C-1)

| State | Tool | Result |
|---|---|---|
| Installed (§8.3) | any | Run `"$bin" guard hook` (no `exec`), stdin inherited. **0 → 0. 2 → 2** (stderr passes through). **Any other exit, including a signal** → **quarantine**, then block with the GC-14 "failed to run (exit N) and was set aside" message. Quarantine renames `<version>` to `.broken-<version>-<epoch>`; the next call takes the "Missing" rows and reinstalls. |
| Missing; ≥ 3 `.broken-<version>-*` directories newer than 24 h | not read-only | Block with the GC-14 "failed 3 times today" message, which names `/plugin`. No further reinstall loop. |
| Missing; lock valid; platform supported | read-only allowlist (§9.4) | Exit 0. |
| Missing; lock valid; platform supported | anything else; `ANTHOLOGY_GUARD_NO_FETCH` unset | Buffer stdin to a temp file. Run `bootstrap.sh guard --plugin guardrails --quiet` (20 s, rate-limited). If installed, run the binary on the buffer and return its verdict, applying the first row's rules. Otherwise block with the not-installed message, or the no-downloader message with the manual command. |
| Missing | anything else; `NO_FETCH` set | Block with the not-installed message. |
| Lock invalid, or platform unsupported | read-only allowlist | **Exit 0** (m-2), so reading works even on Windows. |
| Lock invalid, or platform unsupported | anything else | Block with the specific GC-14 message. |

**Why the current call is blocked rather than retried after a quarantine:** stdin is not buffered on the hot path, to keep it cheap. The next call heals.

**Recovery is always possible:**
- self-heal on the next call, at most once a minute;
- `/anthology:setup`. Its skill says: if a Bash call is blocked with `(setup)`, wait a minute and retry once;
- a new session while online;
- **`/plugin` → disable anthology-guardrails**, which never passes through `PreToolUse`.

Every `(setup)` and `(internal)` message names `/plugin`.

**Architect's calls:**
- self-heal from the guard shim (cost: one ≤20 s pause per minute on blocked calls while offline);
- quarantine on any exit other than 0 or 2 (cost: one re-download after a transient crash such as an OOM kill);
- a cap of 3 quarantines per version per day (cost: after a bad release, users must use `/plugin` until a fixed version ships, which is the honest answer).

**Test matrix (BS1, CO3):**

| Case | Expected |
|---|---|
| truncated file | exec format error 126 → quarantine + heal |
| wrong-arch binary | 126 or "Bad CPU type" → quarantine + heal |
| panic before `catch_unwind` | exit 101 → quarantine + heal |
| panic inside `check` | exit 2 `(internal)` block; no quarantine |
| SIGKILL | 137 → quarantine + heal |
| internal hang | binary watchdog: exit 2 `(internal)` within 8 s |
| hard hang (SIGSTOP) | hook timeout → `onFailure` block; smoke `guard-timeout` |
| deterministic failure | 3 quarantines → `/plugin` message |

### 9.3 Reading `tool_name` in sh

In raw JSON the sequence `"tool_name"` followed by optional whitespace and `:` can only be an object key. The shim joins lines with `tr '\n\r' '  '` and counts those keys. It allows only when there is **exactly one** and its value is an allowlisted string. Every payload has a top-level `tool_name`, so a nested second key makes the count 2, which blocks. Fuzzed in BS1.

### 9.4 Read-only allowlist (when the binary is missing or cannot run here)

`Read`, `Grep`, `Glob`, `LS`, `NotebookRead`, `WebFetch`, `WebSearch`, `TodoWrite`, `TaskCreate`, `TaskUpdate`, `TaskGet`, `TaskList`, `TaskOutput`, `TaskStop`, `Task`, `Agent`, `Skill`, `ToolSearch`, `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`, `BashOutput`, `KillShell`.

- It is an **allowlist** (architect's call): anything else is blocked, including future tools, `mcp__*`, `ListMcpResourcesTool` and `ReadMcpResourceTool`.
- `Skill` lets `/anthology:setup` load. `Task` and `Agent` are allowed because subagent calls are guarded too. `TaskStop` and `KillShell` stop work, which lowers risk (m-1).
- Legacy names (`LS`, `NotebookRead`, `BashOutput`) are harmless and kept.
- **Validation:** runner 4 of the replay (§13.7) reports which real calls would be blocked, by `tool_name`, on Claude Code 2.1.311 and 2.1.329. CO1 reviews that list.

Cost if wrong: a harmless new tool is blocked until the binary installs.

**Documented residual (SECURITY.md).** While the binary is missing, G3 (Chrome profiles) is not enforced for `Read`, `Grep`, `Glob`, `LS`, `NotebookRead` or `WebFetch` (a `file:` URL). The native `Read(…)` deny rules from setup still apply.

### 9.5 Inside the binary

- **Malformed envelope** (not JSON, not an object, or `tool_name` not a string) → exit 0, as today.
- **JSON.** `pycompat::json::loads` matches `json.loads`:
  - lone surrogates → U+FFFD (D-6);
  - NaN and Infinity are accepted;
  - for duplicate keys the last value wins, in the first position;
  - invalid UTF-8 is replaced first.

  It is iterative with **no depth limit**, so Python's `RecursionError` fail-open becomes a real check (F-10, D-1).
- **Panics.** `hook()` installs a **silent panic hook** (m-5), so nothing reaches stderr except our line, and wraps `check` in `catch_unwind`. A panic gives the GC-14 `(internal)` panic block, exit 2.
- **Regex limits.** Any `fancy-regex` backtrack-limit error in shell or guard sets `Analysis.incomplete` or aborts the check. Either way the call is blocked with the GC-14 `(internal)` limit message (I-9).
- **Watchdog.** A thread started by `guard hook` writes the `(internal)` limit block and exits 2 after **8 s**. This stays under the self-test's 10 s timeout.
- **Divergence rule.** These are the only intended guard divergences: **where Python raised or would merely be slow, Rust may block, and it must never allow what Python blocked** (D-1, D-7). **Architect's call** (fail-closed intent). Cost if wrong: a guard bug blocks one call. Fuzzing makes that rare, and the message names `/plugin`.

### 9.6 Unsupported platforms and Windows

Read-only tools are allowed. Everything else is blocked with the "no guard binary for this platform … disable anthology-guardrails (/plugin)" message (F-18). **Architect's call:** consistency with fail-closed. Cost if wrong: those users must disable the plugin.

### 9.7 What does not change

- Rule order, messages, the stderr format and empty stdout.
- "A block wins over rtk" (ADR-008).
- The four guard evals grade `anthology-guardrails \(Gn\)` unchanged.

---

## 10. Release pipeline

The new workflow is `.github/workflows/anthology-release.yml`, cloned from `rtk-release.yml`. Every action is pinned by SHA.

- **Triggers.** A tag `anthology-bin--v*`; `workflow_dispatch` (input `tag`). A PR touching the workflow, `rust-toolchain.toml` or `deny.toml` runs a dry run (no attestation, no publish).
- **`meta` job.** The tag must match `^anthology-bin--v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$` and equal `anthology-bin--v` plus `[workspace.package].version`.
- **`build` job** (native runners, the rtk matrix):
  - `macos-15`, `macos-15-intel`, `ubuntu-24.04` (musl), `ubuntu-24.04-arm` (musl). Linux uses `musl-tools` and `CC_<target>=musl-gcc`, because bundled SQLite compiles C.
  - Steps: `cargo deny check`; `cargo test --locked --target $T -p anthology-pycompat -p anthology-shell -p anthology-guard -p anthology-core -p anthology-settings -p anthology-memory -p anthology-runtime -p anthology-cli`. That is the **shipped graph** only (m-16); shims, difftest and meta tests run in CI.
  - `cargo build --release --locked -p anthology-cli --target $T`; `readelf` must show no `INTERP` or `NEEDED` on Linux; `anthology --version` must match.
  - Package: `COPYFILE_DISABLE=1 tar -czf dist/anthology-$T.tar.gz -C stage anthology LICENSE`. There is no NOTICE, per the PLAN's credits constraint.
- **`publish` job.** Exactly 4 tarballs, then `checksums.txt` (`sha256sum … | LC_ALL=C sort -k2`), then `actions/attest-build-provenance`, then `gh release create --verify-tag`. It fails if the release already exists.
- **Pin.** `anthology-dev lock write --tag <tag> --checksums checksums.txt` rewrites both `anthology.lock` files. The plugins then ship through `release.sh`, which runs `lock check-fresh` (§8.5).
- **"Signed"** means build-provenance attestation plus the sha256 pin in a reviewed commit, as for rtk. Users verify with `gh attestation verify <file> --repo rbonweb/anthology`. There is no Apple codesigning (architect's call; cost if wrong: add codesign and notarize later, with the lock format unchanged).

---

## 11. Dependency policy

**Rules:**
- crates.io only, `Cargo.lock` committed, `--locked` everywhere;
- **all dependencies are declared once in `[workspace.dependencies]` by F1** (plan §0.2), and lanes do not add crates without an integrator plumbing PR that carries a one-line justification;
- Dependabot `cargo` for `/`;
- `deny.toml`:
  - `[advisories]`: deny vulnerabilities, unmaintained and yanked crates;
  - `[licenses]`: allow MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0, MPL-2.0;
  - `[bans]`: deny `openssl-sys` and duplicate versions outside a reviewed skip list;
  - `[sources]`: crates.io only.
- **No `serde_json` in the shipped binary (m-11).** cargo-deny bans are graph-wide, so a `repo-tests` check asserts that `cargo tree -p anthology-cli -e normal` contains no `serde_json`. `ureq` is used without its `json` feature.
- `cargo deny` alone covers RustSec, replacing `cargo audit` (architect's call; no cost).

### 11.1 Regex policy (I-8)

The rule is **`regex` where the Python pattern has no lookaround and no backreference; `fancy-regex` wherever it has either.**

- This holds in every crate, including shell, guard and dev.
- Dynamic patterns built from `HOME` and the hostname (memory_privacy.py:136, :154) use `fancy-regex`.
- `pycompat::re::translate` emits **fancy-regex syntax**, so lookaround passes through.
- The vendor portmap `js_node_regex` patterns contain no lookaround today (checked). DV1 records this and still compiles them through `translate`.
- **Backtrack-limit handling:**
  - **privacy:** the line is reported as a finding of kind `unscannable` (fail closed, D-7);
  - **guard and shell:** the call is blocked (`(internal)`, D-7);
  - **dev tools:** an error exit.

The full table, pattern by pattern, is plan GC-16.

### 11.2 Crates

| Crate | Where | Why |
|---|---|---|
| `regex` | shell, guard, memory, pycompat, dev | Linear time for the lookaround-free patterns |
| `fancy-regex` | shell, guard, memory, pycompat, dev | Lookaround and backreferences with Python-like spans (§11.1) |
| `rusqlite` (`bundled`) | memory::index | See §11.3 |
| `flate2` (`rust_backend`) | memory::portable, runtime::rtk_install | gzip |
| `tar` | rtk_install, portable | **Reading only.** Export uses a custom header writer for CPython PAX byte parity (m-20). |
| `sha2` | core, runtime, memory, dev | sha256 |
| `ureq` (rustls, ring, platform-verifier; **no `json`**) | runtime::fetch, dev | HTTPS with an https-only redirect policy, the OS trust store, and `HTTPS_PROXY`/`NO_PROXY` |
| `libc` | core::sys | `O_NOFOLLOW`, `localtime_r`, `gethostname`, `getpwuid_r` |
| `lexopt` | cli, dev | Argument parsing |
| dev only: `proptest`, `yaml-rust2`, `tempfile`, `rustls` (TLS test server) | tests | Properties, YAML meta tests, temp dirs, the RT2 TLS test server. Test certificates are committed under `anthology-testkit/fixtures/tls/` (no `rcgen`). The plain HTTP test server is std `TcpListener` in testkit (m-12). |

### 11.3 SQLite: `rusqlite` with `bundled` (architect's call)

Reasons:
- FTS5 and `porter unicode61` are always compiled in (verified: `-DSQLITE_ENABLE_FTS5`);
- static musl builds cannot link a system `libsqlite3`;
- one SQLite version gives deterministic `bm25()` and `snippet()`;
- rtk already ships it. Pin the same rusqlite line as rtk (0.34) and bump both together.

Alternatives rejected: system SQLite (static builds break, versions drift), `libsql`/`sqlite` (heavier or less audited), tantivy (different ranking), hand-rolled search (no ranking parity).

**Schema version.** `PRAGMA user_version` goes from 1 to **2**, so indexes built by Python rebuild once (F-11). Python rebuilds on any version other than 1, so a downgrade is also safe.

---

## 12. Python-semantics parity (`anthology-pycompat`)

The rules (also plan GC-16):

- **Authority.** CPython **3.12** in the pinned `python:3.12-slim` image is the oracle. 3.9 runs are a non-gating report of differences (m-7).
- **JSON.** As §9.5 for `loads`. `dumps` matches CPython byte for byte: separators, `ensure_ascii` escapes including surrogate pairs, indent, `sort_keys` by code point, and float `repr`.
- **Strings.**
  - Code-point counting and slicing, unless Python counted bytes.
  - `lower` (including the context-sensitive **Final_Sigma**: `'ΑΣ'.lower() == 'ας'`, m-6), `isspace` (includes `\x1c-\x1f`), `split()`, `strip()`, `isdigit`, `isalpha`, `isalnum` and `unicodedata.category`.
  - Tables are generated from CPython 3.12 (Unicode 15.0) and committed with their generator.
- **Regex.** `pyre::{W,S,D}` classes; `\Z` → `\z`; lookaround via fancy-regex (§11.1); `\b` keeps Rust's meaning (D-3).
- **Paths.** `posixpath.normpath`, `os.path.realpath`, `fnmatch.fnmatchcase`.
- **`ipaddress`.** The parts privacy uses: IPv4 octets, IPv6 parsing with zone stripping, and the `fc00::/7` and `fe80::/10` checks.
- **`shlex`.** `split` and `quote`, for smoke.
- **Error text (I-10).**
  - `PyExc::py_str` renders `[Errno N] <text>: '<path>'` from **committed per-OS errno tables**: Linux from glibc, dumped by the oracle in the 3.12 image; macOS dumped on the macOS runner.
  - It never uses musl's `strerror` or std's `Display`, so users see the same text Python printed on glibc Linux and macOS.
  - **Architect's call.** Cost if wrong: on a musl-only host, Python would have printed musl's wording. That is cosmetic.
- **difflib.** `SequenceMatcher` with autojunk, and `unified_diff(lineterm="", n=0)`.

---

## 13. Test strategy

### 13.1 Layers

1. Unit tests, ported with the same intent.
2. Integration tests driving `anthology` and the shims through `anthology-testkit`.
3. The differential harness, frozen into golden files at cutover.
4. Property tests and fuzzing.
5. Meta tests (`repo-tests`).
6. Smoke and evals.
7. **The replay of real local traffic** (§13.7).

### 13.2 Differential testing (the main oracle)

- **Oracle.** Baseline Python in the pinned 3.12 image. Rust is static musl `anthology` and `difftest` binaries **in the same container**.
- **macOS (m-9).** The CLI-level surfaces also run on the macOS runner against python.org 3.12 installed by a SHA-pinned `actions/setup-python`. This job **gates**, so `/private/var` realpaths, APFS case-insensitivity, BSD tools and macOS errno text are covered.
- **Surfaces.**
  - Library level: a long-lived Python driver (`port/difftest/oracle/<surface>.py`, JSONL) against Rust in-process. Used for `shell.segments`, `shell.analyze`, `guard.check`, `privacy.findings`, `settings.*`, `notes.parse`, `pycompat.*`, `core.state` and `rtk_install.reconcile` (injected fetcher).
  - CLI level: `guard.hook`, `hook.session-start`, `setup`, `doctor`, `mem`, `memory.portable`, `memory.sync` (scenario fixtures), and every `anthology-dev` tool.
  - The surface names are fixed by F1 (plan §0.2).
- **Cases.**
  - Seeds extracted from every Python test's inputs (≥ 1,150 guard rows).
  - Generated variants: the mutation set covers quoting, wrappers, `sh -c`, substitutions, heredocs, `cd`/`-C`, docker, case, `~`/`$HOME`, brace expansion, `file:` URLs, CRLF/NBSP/`\x1c`, NUL, lookaround boundaries for privacy, and hostile repeats.
  - Scenario fixtures from a filesystem-plus-git DSL.
- **Normalization:** `<ROOT>`, `<TODAY>`, durations, D-2 library text, D-4 usage text.
- **Divergences.** Each surface has its own `port/difftest/divergences/<surface>.toml`. Each entry names a GC-16 class; anything else fails.
- **Gate per lane:** zero unexplained diffs on all seeds plus 10⁴ generated per PR. Nightly: 10⁶ for guard and shell, 10⁵ for privacy.
- **Freeze at cutover** into `crates/*/tests/golden/*.jsonl`, replayed by `cargo test` forever.

### 13.3 Porting every Python test: the ledger and test seams

- **Id list.** `port/python-test-ids.txt`, from `python -m unittest discover -s tests -v` (3.12 image with requirements). It includes inherited run ids, and is regenerated whenever a `build`→`rust` merge changes `tests/`.
- **Ledger.** `port/ledger/<python_module>.tsv`, one file per Python module (I-5), each row `python_id  status  rust_test  note`. Statuses:
  - `ported`, `merged:<id>`, `golden:<corpus>`;
  - `dropped:py-compat`, `dropped:python-runtime`, `dropped:superseded:F-n` (n = 1..20).
  - Every `dropped` row carries a note naming its replacement test where one exists.
- **Rows.** Table rows are extracted to `crates/*/tests/data/<module>.jsonl` with `"py"` ids. The Rust test keeps the Python method name, and row counts are checked.
- **Test seams (I-4; plan GC-19).**
  - Python tests patch ~137 internals. Rust has no monkeypatching, so every patched collaborator becomes a field of a per-module `Deps` struct, and every patched constant becomes a `Limits` field. Both have production `Default`s.
  - `fsx` gets a `#[cfg(test)]` fault hook for the `os.open` failure cases.
  - **Ledger rule:** a row may be `ported` only if the Rust test drives the same branch through a seam. A test that cannot is `merged:` into one that does, or reviewed as `dropped`.
- **`anthology-dev ledger check`** fails on:
  - a missing row;
  - a missing or `#[ignore]` Rust target (checked on macOS and Linux);
  - a lower row count;
  - a `todo` row.

  It runs per lane for the lane's files, and over everything at CO2.

### 13.4 Shell shims

`crates/repo-tests/tests/shims.rs` drives `hooks/*.sh` and `bin/*` with:
- fake binaries (exits 0, 2, 101, 126, 137 and 139, a hang, and a deterministic failure);
- seeded archives;
- `PATH` stubs: no curl, GNU wget only, BusyBox wget only, node only, no downloader, a slow or never-ending server, no sha tool, no tar;
- the §9.2 matrix;
- the session-start budget test: a stubbed downloader that never finishes, where `session-start.sh` must print one JSON line within **45 s**.

It runs under dash, `bash --posix` and busybox sh in Linux CI, on the four distribution images in §8.4, and under `/bin/sh` on macOS. The `tool_name` scan is fuzzed with proptest: allow ⇒ exactly one `tool_name` key and the top-level value is allowlisted.

### 13.5 Fuzzing and properties

- **proptest** (stable, in `cargo test`, fixed seeds):
  - `analyze` terminates without panicking;
  - **wrapping monotonicity** of verdicts;
  - privacy insertion is found on its line;
  - **privacy never shrinks when the backtrack limit is lowered** (an error becomes `unscannable`; I-9);
  - JSON round-trips;
  - the key scan.
- **cargo-fuzz** (pinned nightly, weekly `fuzz.yml`, 10 minutes per target): `shell_segments`, `shell_analyze`, `guard_hook`, `privacy_findings`, `json_loads`, `portable_import`, `settings_apply`. Corpora are committed; crashes become regression tests.

### 13.6 Performance

- Ported bounds: guard 0.5 s and 0.2 s; the 5,000-deep input returns `None`.
- Privacy: `BOUND=1.0` s and `cost(160k) < max(6·cost(40k), 0.2 s)`.
- Session start: ≤ 9,000 characters context, ≤ 2,000 `systemMessage`, and the 45 s shim budget test (§13.4).
- Agent startup budgets: unchanged.
- Guard hook median ≤ 20 ms over 50 runs (not a contract).

### 13.7 Replay of local transcripts (I-1; run by CO1, tool built by RP1)

- **Corpus.**
  - Every `tool_use` block in `~/.claude/projects/**/*.jsonl`, **including `*/subagents/*.jsonl`**, deduplicated by `(tool_name, canonical tool_input)` with a count.
  - It is stored under `~/.claude/anthology/replay/`. It is **local only, never committed**.
  - `.gitignore` gains `/port/replay/` and `replay-corpus*` in case someone copies it into the repo, and a `repo-tests` assertion fails if any tracked path matches `replay-corpus` or `port/replay/`.
- **Envelope.**
  - `{"session_id","transcript_path","cwd","permission_mode","hook_event_name":"PreToolUse","tool_name","tool_input","tool_use_id"}`, serialized the way Claude Code does (JS `JSON.stringify`: compact, raw non-ASCII, `\uXXXX` only for control characters and lone surrogates).
  - `tool_input` is **spliced as the raw bytes** from the transcript line, so real encodings reach `loads_bytes_lossy`.
  - `HOME` is the owner's real home.
- **Four runners per call:**
  1. Python `guard.py` (the authority);
  2. `anthology guard hook`;
  3. staged `hooks/guard.sh` with the binary seeded;
  4. staged `hooks/guard.sh` with the binary **missing**.

  Runners 1-3 must agree byte for byte on exit code and stderr. Runner 4 is a report: how many real calls fail-closed would block, by `tool_name`. It validates §9.4.
- **Deeper than verdicts.**
  - `shell.analyze` is compared on every Bash `command` and `guard.check` on every call (`Entry` minus `opaque`).
  - `privacy.findings` runs over **every text field** in the corpus (tool inputs, assistant text, tool results), Python against Rust.
- **When it runs:**
  - at CO1, on the tagged binary;
  - after every `build`→`rust` merge that touches `guard_rules.py`, `memory_privacy.py` or `crates/anthology-{shell,guard,pycompat,memory}`;
  - at CO2.
- **Gate.** Zero unexplained differences.
  - A committed report with **no commands or text** goes to `port/replay-reports/<date>.md`: corpus size, distinct calls, per-tool counts, runner-4 block counts, both commit SHAs and the sha256 of the corpus manifest.
  - Disagreements become difftest seeds only after manual scrubbing.
- **Limit (stated).** The replay proves parity on real traffic, not bypass resistance. Generators and monotonicity cover bypass resistance.

---

## 14. CI changes

### 14.1 During the port

F1 creates every job, some as no-op placeholders, so lanes never edit `ci.yml`.

| Job | Runs |
|---|---|
| `rust-lint` | fmt, clippy `-D warnings` |
| `rust-test-linux` | `cargo test --workspace --locked` (rust image + git + busybox + node) |
| `rust-test-macos` | the same on `macos-15` |
| `shims-distros` | `repo-tests` shim suite in `alpine`, `debian:*-slim` (+node), `ubuntu:24.04` images (new one-line pinned Dockerfiles `ci/images/{alpine,debian-slim,ubuntu}.Dockerfile`) |
| `deny` | `cargo deny check`, plus the `serde_json` tree check |
| `shellcheck` | pinned image; `shellcheck -s sh` on all shims |
| `difftest-linux` | static musl binaries in the python 3.12 image; `--all --gen 10000` |
| `difftest-macos` | CLI surfaces against setup-python 3.12 (gating) |
| `ledger` | `anthology-dev ledger check --report` (blocking per lane for the merging lane's files) |
| `rtk-test` | root mount; path filter adds `crates/anthology-shell/**` |
| `anthology-release` dry run | on PRs touching the workflow or the toolchain |
| `fuzz` (weekly) | cargo-fuzz |
| existing Python jobs | unchanged until cutover |

Local runs use `scripts/{rust-test,dev,difftest}.sh` in docker. There is no host Rust (architect's call, the owner's existing constraint; cost: a slower local loop).

### 14.2 After cutover (no Python anywhere in CI)

- **Removed:**
  - jobs `test`, `test-py39`, `memory-git`, `setup-git`, the Python part of `taste-bridge`, `difftest-*`;
  - `ci/images/python*.Dockerfile`, `tests/requirements.txt`;
  - Dependabot `pip` (F-16).
- **Rewritten to `anthology-dev`:**
  - `vendor-check`, `version-bump`, `memory-diff`, `cli-syntax` (`smoke … cli-syntax`), `upstream-watch.yml`, `evals.yml`;
  - **`startup-budget`** = `anthology-dev startup-budget` plus `cargo test -p repo-tests startup_budget` (the `test_roster` ports) plus `cargo test -p anthology-runtime budget` (the `test_session_start -k budget` ports) (m-18).
- **`vendor check`** still needs git and network, as today, and runs no Python: vendored `.py` files are compared as bytes.
- **Evals and smoke** use `anthology-dev seed` on a scratch copy of `plugins/`. The probe U10 decides which config dir `claude plugin eval` gives hooks.

---

## 15. Migration and cutover order

The plan is authoritative for the lane ids and graph (m-10).

**0. Preconditions.**
- `fix-f1` is merged into `build`, then `rust` is created.
- The id list and row corpora are extracted.
- **Double-implementation rule:** a Python behaviour change on `build` needs its Rust equivalent, ledger rows and seeds before the next `build`→`rust` merge goes green (architect's call).

**1. Foundation (F1–F4).** F1 pre-declares **every** shared file, dependency, subcommand, module stub, difftest surface, fuzz target and CI job (plan §0.2). After F1, lanes only add files they own.

**2. Wave 1** (parallel after F): P1, SH1 → SH2 (SH2 after P1), MP1\*, ST1\*, RT1a, RT2, BS1, RL1, DV1\*, DV2, DV4, EV1, and MT1, which merges **last** in wave 1. Starred lanes code against P1's signatures and merge after it.

**3. Wave 2:** GD1 → GD2 (after SH2); MS1 → MS2; RT1b (doctor, after RT1a, RT2 and ST1; `ROSTER` comes from F1); DV3 (after DV2); RG1 (after SH2).

**4. Wave 3:** MI1 and MX1 (after MS2); RT3 (after RT1a, RT1b, RT2, ST1, MS2, GD2, BS1); RT4 (after RT1a, RT1b, RT2, ST1, MS2, BS1); RP1, the replay tool (after GD2, MP1, BS1, F4).

**Merges are serial**, through the controller, one lane at a time. Every other open lane merges `rust` back in and re-runs its definition of done before it merges (plan §0.2).

**5. CO1: first binary release and replay.** Tag `anthology-bin--v0.1.0` on `rust`, publish, run `lock write` into staging, then run the full replay (§13.7) with a clean report.

**6. CO2: cutover.**
- Move staging into place; update skills, docs and ADRs; run `difftest freeze`; delete the Python; switch CI; bump plugin versions.
- **It must not touch the shipped set (§8.5).**
- **Gates:** the ledger is complete, the golden replay passes, a replay rerun is clean, the evals and smokes pass, and no CI job uses Python.

**7. CO3: release.** Merge `rust` into `build`, run `release.sh` for both plugins, then the clean-profile checks (offline fail-closed, heal, and the exit-139 quarantine case).

**8. RG** releases rtk whenever it is ready.

---

## 16. Risks (ranked)

1. **Python-semantics drift in the guard becomes a bypass.**
   - *Mitigation:* pycompat tables; the 10⁶ nightly difftest; monotonicity properties; the replay at `analyze()` level; D-1 and D-7 only ever stricter.
2. **The fail-closed shims brick sessions** (portability, a broken install, the downloader).
   - *Mitigation:* quarantine and self-heal; `/plugin` named in every message; four distribution images; the BusyBox, GNU and node paths; a real total timeout; the key-scan fuzz.
3. **The privacy regex port loses findings or goes quadratic.**
   - *Mitigation:* fancy-regex keeps the semantics; a limit error is reported as `unscannable`; hostile and scaling tests; the replay over all transcript text; a hand-written fallback behind one function.
4. **Port debt from `build`.**
   - *Mitigation:* the double-implementation rule; per-module ledgers regenerated on each merge.
5. **Plugin and binary skew.**
   - *Mitigation:* the shipped-set `check-fresh`; lock equality and format tests; dev seeding for evals; CO2 may not touch the shipped set.

Further risks:
6. **Merge collisions across 14 parallel lanes.** F1 pre-declaration, per-lane files and serial merges.
7. **Session-start timeout on first runs.** One download per hook, a 20 s budget, and the 45 s shim test.
8. Byte parity of gzip, tar and SQLite. D-5 plus a custom tar writer.
9. ureq versus urllib behind proxies. An env-proxy integration test.
10. A conflict on the one-line rtk `Cargo.toml` dependency during upstream sync. The lock-guard allowlist and a simulated sync.
11. No host Rust slows the owner's loop.
12. The credits policy versus MIT/Apache attribution in a binary. An existing owner policy, unchanged.

---

## 17. Architect's calls (summary)

| # | Call | Reason | Cost if wrong |
|---|---|---|---|
| A1 | `anthology-shell` (regex and fancy-regex only); rtk depends on it | One parser for security decisions | Fall back to a conformance corpus |
| A2 | Shared `<ah>/bin/<version>/` | Deterministic; guardrails alone works | ~6 MB until pruned |
| A3 | sh shim plus `onFailure` backstop | Read-only tools and our message survive | A spurious block of a read-only tool |
| A4 | Read-only **allowlist**, validated by replay runner 4 | New tools blocked by default | A harmless tool blocked until install |
| A5 | Self-heal from the guard shim, once a minute | Makes "run /anthology:setup" actionable | ≤20 s pause per minute offline |
| A6 | Panic, regex limit or 8 s watchdog → block (D-1, D-7) | Fail-closed intent | One call blocked, with `/plugin` named |
| A7 | Guard data and `ROSTER` compiled in, created in F1 | No cross-binary import; unblocks wave 1 | Stricter-only mismatch across versions |
| A8 | `anthology-pycompat` in-tree | Parity; no JSON bypass | ~3k lines |
| A9 | fancy-regex wherever Python uses lookaround or backreferences | Same spans as `re` | Hand-written matchers later |
| A10 | rusqlite bundled; `user_version` 2 | FTS5; static builds | One rebuild per user |
| A11 | `cargo deny` alone; `serde_json` banned from the shipped graph by a tree check | One RustSec tool; no JSON bypass | None |
| A12 | `lexopt` plus argparse rules | Minimal; exact exit codes | Abbreviation drift |
| A13 | `anthology-dev` never shipped | Keeps network and API code out of the runtime | None |
| A14 | sh/bash glue stays shell | Glue, not logic | Port later |
| A15 | No Apple codesign | Matches rtk | Add later |
| A16 | Live difftest, then golden freeze | The oracle survives | Blessed updates |
| A17 | Fail closed on unsupported platforms (read-only allowed) | Consistency | Those users disable the plugin |
| A18 | Replay of local transcripts (replaces the shadow run) | Real inputs, deeper than verdicts, no calendar time | None |
| A19 | Double-implementation rule | No silent port debt | Slower `build` changes |
| A20 | Keep "no host Rust" | Owner constraint | Slower local loop |
| A21 | Quarantine on exit ∉ {0,2}; `.bin-sha256`; 3-strikes cap | Recovery without user action; no reinstall loops | One extra download after a transient crash |
| A22 | Bootstrap downloaders: curl, GNU wget, BusyBox wget, node; watchdog total timeout | Alpine, slim and npm installs work | Larger sh surface, covered by the distro matrix |
| A23 | Per-OS errno text tables | Musl static binary prints glibc/macOS text | Cosmetic on musl hosts |
| A24 | One download per session-start hook; `BOOTSTRAP_BUDGET` 20 s; T0-based rtk skip (F-20) | Always inside the 60 s hook timeout | rtk reconciles one session later after a first install |
| A25 | RT1 split: loc+compat (wave 1), doctor (wave 2) | Matches the real import graph | None |
| A26 | Merge discipline: F1 pre-declares shared files; serial merges | Avoids lock and dispatch collisions | Integrator plumbing PRs |

---

## 18. Review traceability (plan-review.md → fix)

| Finding | Fixed in |
|---|---|
| C-1 | §1.3 G-4, §8.3, §8.4 (step 3, `session` writes no stamp), §9.2, §9.5; plan GC-14, BS1, GD2, CO2 skill text, CO3 |
| I-1 | §4.6, §13.7, §15; plan RP1, CO1, CO2 |
| I-2 | §5 graph, §6.3; plan F1, lane table (RT1a/RT1b) |
| I-3 | Plan §3.6 (the full memory API) |
| I-4 | §13.3; plan GC-19 |
| I-5 | §5, §15; plan §0.2 |
| I-6 | §8.5; plan RL1, CO2 |
| I-7 | §8.7; plan GC-7, RT3, BS1 |
| I-8 | §11.1; plan GC-16 table |
| I-9 | §7.1, §9.5, §11.1; plan GC-12, MP1 |
| I-10 | §12; plan P1, GC-16 |
| I-11 | Plan GC-15 F-16 to F-20, GC-16 D-6; MI1 mapping; DV3 uses F-17 |
| I-12 | §8.4, §13.4, §14.1; plan BS1 |
| m-1 to m-22 | All accepted; see plan §6 for where each is fixed. None rejected. |
