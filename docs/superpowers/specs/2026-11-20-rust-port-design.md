# Anthology in Rust: design

- **Date:** 2026-11-20
- **Status:** Proposed (architect), **revision 4**. Revision 4 applies `plan-rereview-2.md` (N2-I1, m2-1 to m2-12).
  - Revision 2 applied the independent review `.superpowers/sdd/2026-11-20-rust-port/plan-review.md`.
  - Revision 3 applies the scoped re-review `plan-rereview-1.md` (N-I1 to N-I5, m-R1 to m-R15) and the controller's rulings on it.
  - §18 maps each finding to where it is fixed.
- **Decisions:** the owner's binding decisions are in §1.2. Every other choice is marked **architect's call**, with its reason and its cost if wrong. No owner decision is open.
- **Baseline (N-I3):** branch `rust` **as it stands when a lane starts**.
  - `rust` was created from `build@1e00f832`. Pending `build` work (fix-f1, FW-R, FW-Q) arrives through `build`→`rust` merges.
  - Lane instructions never cite a fixed SHA. Test counts come from `port/python-test-ids.txt` (the ledger), not from literals in this document.
  - Controller ruling: F1 starts before fix-f1 lands, and RG1 wires rtk once fix-f1 is on `rust`.
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
   - `tests/*.py`: 35 test modules and 4 helpers, about 19k lines. There were 1,528 `def test_` on `rust@1e00f832`; the authoritative and current count is `port/python-test-ids.txt`.
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
| `guard_rules.py` lexer + expansion (`_MAX_DEPTH` through `_analyze`, :48-~1000 at `rust@1e00f832`) | ~950 | **`anthology-shell`** (shared with rtk) | guard, rtk gate |
| `guard_rules.py` rules G1–G6 and paths (from `_HOME_VAR` on, :1004-1554 at `rust@1e00f832`) | ~550 | `anthology-guard::rules` | guard |
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
| guardrails `PreToolUse *`: `python3 scripts/guard.py` | `anthology guard hook --shim` (run by the shim); plain `anthology guard hook` keeps the Python contract (difftest, replay runner 2) | `sh ${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh`, `"timeout": 60`, `"onFailure": "block"` | stdin is hook JSON. Plain mode: exit 0 allows, exit 2 blocks with stderr `Blocked by anthology-guardrails (<id>): <msg>\n` and empty stdout. `--shim` mode is the same, except that an allow also writes the **allow token** `anthology-guard:allow:v1\n` to stderr (N2-I1, GC-17). |
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

- **Port scope.** A direct port of `guard_rules.py` from `_MAX_DEPTH` through `_analyze` (:48-~1000 at `rust@1e00f832`; names, not lines, are the identifiers): `_Lexer`, `_scan_paren`, `_find_heredoc_end`, `_lenient_segments`, `_strip_prefixes`, `_rtk_command`, `_parse_shell_args`, `_add_command`, `_collect`, `_collect_find_exec`, `_reads_stdin_as_shell`, `_echo_text` and `_analyze`. The limits keep their values. The types are in plan §3.4.
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
- **CI.**
  - `rtk-test.sh` and `rtk-net-test.sh` mount the repo root.
  - The existing live `rtk-test` job already runs on every push (it has no path filter), so changes to `crates/anthology-shell/**` are covered without a filter.
  - RG1's edits to the two scripts go through a controller plumbing PR, together with the Python `test_rtk_layout` assertions and their Rust port (plan §0.2).

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
  <version>/anthology                    # 0755 regular, non-empty file that passed the --version probe at install
  <version>/.bin-sha256                  # "<sha256 of the extracted binary>\n" (written before .sha256)
  <version>/.sha256                      # "<tarball sha256>\n", written LAST (install marker)
  <version>/.last-used                   # touched by SessionStart bootstraps
  .broken-<version>-<epoch>-<pid>/       # a quarantined install (§9.2); <epoch> is the quarantine time
  .tmp-XXXXXX/                           # per-attempt staging, removed on exit
  .attempt                               # rate-limit stamp, created with noclobber (§8.4)
  anthology-<target>.tar.gz              # optional, user-seeded for offline installs (used only if it matches the pin)
```

- **"Installed" (guard hot path).** `<version>` is a real directory, and `anthology` is a regular, executable, **non-empty** file (`test -f`, `test -x`, `test -s`, all shell builtins). `.sha256` is non-empty. There is no hashing on the hot path.
- **"Current" (bootstrap, doctor).** Installed, plus `.sha256` equals the lock's hash for this target, plus `.bin-sha256` equals the sha256 of `anthology`.
  - If the binary hash differs, the install is quarantined and reinstalled (C-1).
  - Doctor reports the mismatch.
- **Allow token (N2-I1).** The shim allows only on the binary's positive allow token (§9.2). No file that is not our binary can produce it, whatever the shell does with it: an empty, NUL-filled or whitespace-only file, or a script.
- **Install-time `--version` probe (N-I1).** Before any marker is written, the staged binary must print exactly `anthology <version> (<target>)` and exit 0. A wrong-arch, truncated or garbage file therefore never becomes "installed". On dash and BusyBox, such a file would otherwise be run as a shell script.
- **Quarantine names (m-R1).** The `<epoch>` in `.broken-<version>-<epoch>-<pid>` is the quarantine time. It is the only clock used for both the 3-strike count ("in the last 86,400 s") and pruning ("older than 24 h"); directory mtimes are never used. `<pid>` keeps two quarantines in the same second from nesting.

**Architect's call:** integrity is verified at session start, not per call. Cost if wrong: a binary corrupted mid-session runs until it fails, and is then quarantined by the exit classification (§9.2).

### 8.4 The bootstrap (`hooks/bootstrap.sh`, POSIX sh)

**CLI (m-3):** `bootstrap.sh <session|guard|wrapper> --plugin <anthology|guardrails> [--quiet] [--budget SECONDS]`

**Modes:**
- `session`: SessionStart, setup and `ensure_live_guard_binary`. It is never rate-limited and **never writes `.attempt`** (C-1.6).
  - **3-strike cap (m2-6).** `session` mode honours the cap, except for **one fresh attempt per hour**. The hour is tracked by `<ah>/bin/.session-retry`, created with noclobber; a stale stamp is retired as in step 5.
  - **Architect's call.** SessionStart fires on startup, resume, `/clear` **and auto-compaction**, so a session start is not necessarily a deliberate retry.
  - Cost: during a bad release, at most one download per hour per plugin hook (two hooks, so up to two per hour) is spent on the fresh attempt. Setup cannot reach this path while the guard is capped, because its Bash call is blocked. The way out stays a new session after the hour, or `/plugin`.
- `guard`: `guard.sh` self-heal. Rate-limited, and honours the cap.
- `wrapper`: `bin/*`. Rate-limited, and honours the cap.

**Output:**
- `--quiet`: exactly one stdout line `status=present|installed|failed:<reason>`. Exit 0 for present or installed, 1 for failed.
- Without `--quiet` in `session` mode: nothing on success, and the `--plugin`'s GC-14 `systemMessage` JSON on failure. Exit 0.

**Steps:**
1. **Platform.** Map `uname -s`/`uname -m` to the target (the rtk table). Anything else is `unsupported platform <os>/<arch>`.
2. **Lock.** Validate per §8.2; a failure is `anthology.lock is not valid`.
3. **Install folder (m-R2).** `mkdir -p <ah>/bin`, refusing a symlink at `<ah>/bin`. A failure is `cannot write the install folder`.
4. **Current?** If the install is current, touch `.last-used` and report `present`. A binary-hash mismatch quarantines it and continues.
5. **Rate limit and cap** (`guard` and `wrapper` only):
   1. If 3 or more `.broken-<version>-<epoch>-*` directories have `<epoch>` within the last 86,400 s, stop with `failed:three-strikes`.
   2. If `.attempt` is newer than 60 s (`find "$dir" -name .attempt -mmin -1`), stop with `failed:rate-limited`.
   3. Otherwise retire a stale stamp **atomically**, with `mv -f .attempt .attempt.stale.$$` (only one racer's rename finds it), then `( set -C; : > "$dir/.attempt" )`.
   4. If the create fails, it is `rate-limited` **only if `.attempt` exists and is newer than 60 s** (re-run the `-mmin -1` test). A stale stamp in an unwritable folder is therefore `cannot write the install folder`, never `rate-limited` forever (m2-3).
   5. Concurrent calls may still both download in a narrow race. That is harmless: each uses its own `mktemp -d`, and renames are atomic (m2-4).
6. **Get the archive** within the budget. The default is `BOOTSTRAP_BUDGET` = **20 s** of wall time, including connect. Sources are tried in this order:
   1. **Seed.** `<ah>/bin/anthology-<target>.tar.gz`, if present. If its sha256 does not match the pin, it is **not used and not deleted**. The network downloaders below are tried, and if they also fail, the reason names the seed (`seed file anthology-<target>.tar.gz does not match anthology.lock`), so a stale seed never blocks for good (m-R3).
   2. **curl:** `curl -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 10 --max-time $B --max-filesize 67108864 -A anthology-bootstrap -o "$tmp/a.tgz" "$url"`.
   3. **GNU wget** (`wget --version 2>/dev/null | grep -q 'GNU Wget'`): `wget --https-only -q -T 10 -U anthology-bootstrap -O … "$url"`, under the watchdog.
   4. **BusyBox wget** (`wget --help 2>&1 | grep -q BusyBox`): `wget -q -T 10 -O … "$url"`, under the watchdog. There is no https-only flag; the https URL plus the sha256 pin decide which bytes are accepted.
   5. **node** (architect's call): `NODE_USE_ENV_PROXY=1 node -e "$NODE_GET" "$url" "$tmp/a.tgz"`, under the watchdog. `$NODE_GET` is a **fixed constant** in `lib.sh`, and a test checks it byte for byte. It is https-only on every hop, follows at most 5 redirects, caps the size at 64 MiB and has its own total timeout. Caveats, documented in README and SECURITY:
      - `https.get` honours `HTTPS_PROXY`/`NO_PROXY` **only on node ≥ 24 with `NODE_USE_ENV_PROXY=1`**, which the bootstrap sets. On older node behind a proxy, the result is `download failed`.
      - Claude Code from the native installer does not guarantee `node` on `PATH`. node is a last resort that mainly helps npm installs on slim images.
   6. If nothing worked: `no downloader (curl, wget or node)` with the manual command (GC-14), or the seed reason above.

   **The watchdog (m-R4)** cannot hold the caller's stdout or outlive a fast download:
   ```sh
   "$@" </dev/null & p=$!
   ( exec </dev/null >/dev/null 2>&1
     sleep "$B" & s=$!
     trap 'kill "$s" 2>/dev/null; exit 0' TERM
     wait "$s"; kill -9 "$p" 2>/dev/null ) & w=$!
   wait "$p"; rc=$?
   kill "$w" 2>/dev/null; wait "$w" 2>/dev/null
   ```
   A download that finishes in 2 s returns in about 2 s, and no orphaned `sleep` keeps a pipe open.
7. **Hash.** Use `sha256sum`, else `shasum -a 256`, else `openssl dgst -sha256 -r`; with none, `no sha256 tool`. A mismatch is `sha256 does not match anthology.lock`.
8. **Extract.** `tar -xzf … -C "$tmp/x" anthology`. The result must be a regular, non-empty file and not a symlink, else `the archive has no anthology binary`.
9. **Probe (N-I1).** `"$tmp/x/anthology" --version </dev/null` runs **under the watchdog** (m2-12). It must exit 0 and print exactly `anthology <version> (<target>)`.
   - Otherwise the staged binary is moved to `.broken-<version>-<epoch>-<pid>`, so a probe failure **counts as a strike** (m2-1), and the reason is `the binary does not run on this machine`.
   - A wrong architecture, a `noexec` home folder or a seccomp policy therefore stops after three downloads a day, not after unlimited ones.
10. **Place.**
    1. `mkdir -p <version>`, refusing a symlink;
    2. `mv` the binary in;
    3. write `.bin-sha256` (temp file + `mv`);
    4. write `.sha256` **last**.

    `$tmp` is removed by a trap. Concurrent installs each use their own `mktemp -d`.

    **On a failed placement (m2-2):**
    - `cannot write the install folder` is reported only if `test -w <ah>/bin` fails.
    - Any other placement failure, such as `<version>` being moved away by a parallel quarantine, retries the placement once, then reports `install interrupted by a parallel change`.

**Tooling and test platforms.**
- POSIX, plus `uname`, `sed`, `find`, `mktemp`, `tar`, gzip, one sha tool, and one of curl, wget or node.
- CI runs `shellcheck -s sh`, and runs the shims, with their **real** downloaders against the testkit TLS server (§13.4), on:
  - Alpine (BusyBox ash + BusyBox wget);
  - Debian slim (dash; `nodejs` installed at job start, no curl or wget);
  - Ubuntu 24.04 (dash; `curl` and `wget` installed at job start, because the base image has neither);
  - macOS (`/bin/sh`, system curl).

### 8.5 Upgrades, pinning and binary freshness

- **Pinning.** Exact version plus a per-target tarball hash; never "latest". An upgrade or downgrade is a plugin upgrade or downgrade.
- **Plugin update.** The new lock names a new `<version>`, which is installed beside the old one. Running sessions keep their own version directory.
- **Prune** (Rust session start): delete `<version>` directories not pinned by either live lock and unused for 30 days; `.tmp-*` and `.broken-*` older than 24 h.
- **Freshness (I-6).** `release.sh` runs `anthology-dev lock check-fresh` before a plugin release. It compares the **shipped set** between the lock's tag commit and `HEAD`. The shipped set is:
  - the `src/` files and `Cargo.toml` of every workspace package in `cargo tree -p anthology-cli -e normal,build --prefix none` (excluding `tests/`, `benches/`, `examples/`);
  - the `Cargo.lock` entries of the packages in that graph;
  - `rust-toolchain.toml`;
  - the root `[profile.release]`, `[workspace.package]` (version, edition) and the `[workspace.dependencies]` entries that graph uses;
  - every `build.rs` of those packages, and any file they pull in with `include_*!` from outside `src/`.

  Test-only, dev-tool, difftest and golden changes do not force a binary release. CO2 must not touch the shipped set, which also guarantees the binary validated by the replay is the one that ships.

### 8.6 Offline first run

- The bootstrap fails and its `systemMessage` names the reason.
- The guard fails closed: read-only tools work. Each blocked call retries at most once a minute (§9.2), so restored connectivity heals the session.
- **Air-gapped and no-downloader installs:** copy `anthology-<target>.tar.gz` from the release into `<ah>/bin/`. It is verified against the same pin, and a seed that does not match is ignored with a named reason (m-R3). The GC-14 "no downloader" message prints the exact URL and folder.

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

### 9.2 `hooks/guard.sh` decision table (C-1, N-I1)

**Exit classification on the hot path (N2-I1).** There are no temp files:

```sh
err=$("$bin" guard hook --shim 2>&1 >/dev/null); rc=$?   # stdin inherited; the binary's stdout is always empty
```

| `rc` | `$err` (trailing newline stripped by `$(…)`) | Meaning |
|---|---|---|
| 0 | **exactly** `anthology-guard:allow:v1` | Allow (exit 0) |
| 2 | starts with `Blocked by anthology-guardrails (` | A real block: `printf '%s\n' "$err" >&2; exit 2` |
| anything else | anything | **The binary did not work** → quarantine. This includes exit 0 with **empty** or any other stderr: a NUL-filled file that BusyBox runs as an empty script, a whitespace-only file, a script, environment noise. It also includes exit 2 with foreign stderr and any other exit code or signal. |

**The fail-closed invariant (m2-12).**
- `guard.sh` has exactly **two** `exit 0` sites: the token-classified allow, and the read-only allowlist.
- Every other path, including an unexpected bootstrap output, a `mktemp` failure for the stdin buffer and an unknown state, falls through to a final block.
- A bootstrap result of `present` **or** `installed` both lead to running the binary.
- BS1 tests this by counting the `exit 0` sites and by fault injection.

| State | Tool | Result |
|---|---|---|
| Installed (§8.3: `-f`, `-x`, **`-s`**, marker) | any | Classify as above. On "did not work", **quarantine**: `mv <version> .broken-<version>-<epoch>-<pid>`, then block with the GC-14 "set aside" message. That message becomes the three-strikes text when this quarantine is the third (m2-5). If the `mv` fails **and `<version>` still exists**, block with the "could not be set aside" message. If `<version>` is already gone (a parallel call quarantined it), use the "set aside" message (m2-2). |
| Present but empty (`test -s` fails) or not executable | not read-only | Quarantine as above. A 0-byte file is **never executed**, so it cannot allow the call (N-I1). |
| Missing; 3 strikes in 86,400 s (by `<epoch>`) | not read-only | Block with the GC-14 three-strikes message (names `/plugin` and "start a new session"). |
| Missing; lock valid; platform supported | read-only allowlist (§9.4) | Exit 0. |
| Missing; lock valid; platform supported | anything else; `ANTHOLOGY_GUARD_NO_FETCH` unset | Buffer stdin to a temp file; if `mktemp` fails, block. Run `bootstrap.sh guard --plugin guardrails --quiet` (20 s, rate-limited, capped). On `present` or `installed`, run the binary on the buffer and apply the classification. Otherwise block with the message for the bootstrap's reason: not installed, rate-limited, no downloader with the manual command, a stale seed, or cannot write. |
| Missing | anything else; `NO_FETCH` set | Block with the not-installed message. |
| Lock invalid, or platform unsupported | read-only allowlist | Exit 0 (m-2). |
| Lock invalid, or platform unsupported | anything else | Block with the specific GC-14 message. |

**Why the current call is blocked rather than retried after a quarantine:** the crashed binary consumed stdin, and the hot path does not buffer it. A later call heals. The "set aside" message says so accurately, because of the rate limit (m-R14): "it is reinstalled on a later call (at most once a minute)".

**Recovery is always possible:**
- self-heal on a later call;
- `/anthology:setup`. Its skill says: wait a minute, then retry once;
- a new session while online. This makes one fresh attempt per hour even when capped (A28);
- **`/plugin` → disable anthology-guardrails**, which never passes through `PreToolUse`.

Every `(setup)` and `(internal)` message names `/plugin`.

**Hard hang (m-R6).**
- `guard hook` starts its watchdog thread **before reading stdin**.
- Only a process that cannot run its own threads (SIGSTOP, kernel stall) reaches the 60 s hook timeout. That gives Claude Code's generic `onFailure` block ("timed out; blocking because onFailure is block"), which names neither the cause nor `/plugin`.
- This residual is documented in SECURITY.md.

**Architect's calls:**
- self-heal from the shim (cost: a ≤20 s pause, at most once a minute while offline);
- quarantine on anything other than a clean allow or a well-formed block (cost: one re-download after a transient crash);
- the 3-strike cap for tool calls (cost: a broken release needs a new session or `/plugin`).

**Test matrix (BS1, CO3).** Every row runs under dash, BusyBox sh, `bash --posix` and macOS `/bin/sh` unless noted:

Outcomes only; tests never assert a shell's intermediate exit code (m2-9):

| Case | Expected |
|---|---|
| 0-byte binary | `test -s` fails → quarantine; **never allows** |
| **NUL-filled, non-empty** (4 KB, 6 MB, NULs mixed with `\n`) | quarantine; **never allows** (N2-I1: BusyBox runs it as an empty script, exit 0, no token) |
| **whitespace-only** binary | quarantine; never allows |
| **empty script** (`#!/bin/sh` only, `:`, `exit 0`) | quarantine; never allows |
| random bytes | quarantine (the shell may report 126 or a syntax error) |
| ELF for the other architecture | after install → quarantine; at install → the probe fails, which is a strike (`the binary does not run on this machine`) |
| a binary that exits 2 with foreign stderr | quarantine |
| exit 0 with empty stderr, or with stderr other than the token | quarantine |
| not executable (for example a `noexec` mount) | quarantine; reinstall; the probe fails, a strike each time; capped after 3 (m2-1) |
| parallel calls on one broken binary | one quarantine wins; the others give the "set aside" message, never "not writable" (m2-2) |
| stale `.attempt` in an unwritable `<ah>/bin` | `cannot write`, not `rate-limited` (m2-3) |
| panic before `catch_unwind` (101) | quarantine + heal |
| SIGKILL (137), SIGSEGV (139) | quarantine + heal |
| panic inside `check` | exit 2 `(internal)`; no quarantine |
| internal hang | watchdog → `(internal)` within 8 s |
| SIGSTOP | hook timeout → `onFailure` block (smoke `guard-timeout`) |
| deterministic failure | 3 strikes → three-strikes message; a new session retries once |
| `mv` of the quarantine fails (read-only `<ah>/bin`) | "could not be set aside" message, no loop |
| stale seed | network fallback; named reason if that also fails |

### 9.3 Reading `tool_name` in sh

In raw JSON the sequence `"tool_name"` followed by optional whitespace and `:` can only be an object key. The shim joins lines with `tr '\n\r' '  '` and counts those keys. It allows only when there is **exactly one** and its value is an allowlisted string. Every payload has a top-level `tool_name`, so a nested second key makes the count 2, which blocks. Fuzzed in BS1.

### 9.4 Read-only allowlist (when the binary is missing or cannot run here)

`Read`, `Grep`, `Glob`, `LS`, `NotebookRead`, `WebFetch`, `WebSearch`, `TodoWrite`, `TaskCreate`, `TaskUpdate`, `TaskGet`, `TaskList`, `TaskOutput`, `TaskStop`, `Task`, `Agent`, `Skill`, `ToolSearch`, `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`, `BashOutput`, `KillShell`.

- It is an **allowlist** (architect's call): anything else is blocked, including future tools, `mcp__*`, `ListMcpResourcesTool` and `ReadMcpResourceTool`.
- `Skill` lets `/anthology:setup` load. `Task` and `Agent` are allowed because subagent calls are guarded too. `TaskStop` and `KillShell` stop work, which lowers risk (m-1).
- Legacy names (`LS`, `NotebookRead`, `BashOutput`) are harmless and kept.
- **Validation (m2-7).** Runner 4 of the replay (§13.7) writes a **local-only** list of the real calls fail-closed would block, by `tool_name`, to `~/.claude/anthology/replay/runner4-<date>.tsv`, on Claude Code 2.1.311 and 2.1.329. CO1 reviews that list. Only bucket counts reach the committed report.

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
- **Watchdog.** A thread started by `guard hook` **before it reads stdin** writes the `(internal)` limit block and exits 2 after **8 s**. This stays under the self-test's 10 s timeout.
- **Divergence rule.** These are the only intended guard divergences: **where Python raised or would merely be slow, Rust may block, and it must never allow what Python blocked** (D-1, D-7). **Architect's call** (fail-closed intent). Cost if wrong: a guard bug blocks one call. Fuzzing makes that rare, and the message names `/plugin`.

### 9.6 Unsupported platforms and Windows

Read-only tools are allowed. Everything else is blocked with the "no guard binary for this platform … disable anthology-guardrails (/plugin)" message (F-18). **Architect's call:** consistency with fail-closed. Cost if wrong: those users must disable the plugin.

### 9.7 What does not change

- Rule order, messages, the stderr format and empty stdout.
- "A block wins over rtk" (ADR-008).
- The four guard evals grade `anthology-guardrails \(Gn\)` unchanged.

### 9.8 Shell analysis: known limits (SH-F2 to SH-F4)

The guard is one layer of defence. It stops accidental installs and launches by cooperative agents, not a determined bypass. Most commands it cannot model are too complex (G0); the routes below are not, and are allowed. The shell-gap chase stopped at SH-F4 (controller ruling). These routes are known and stay open. Each one needs a value that an earlier command computes, or a shell feature that is not modelled:
- **Plain variables.** A value assigned before it is used is not read into the word: `x=i; npm $x puppeteer`, `: ${x:=i}; npm $x …`, `read l <<< "i puppeteer"; npm $l`, `echo i puppeteer | while read l; do npm $l; done`, and a function that copies a literal argument before running it (`f() { local c=$1; eval "$c"; }; f "npx …"`).
- **Subscripts split across words** (G0-FP). A word is too complex when it holds a whole subscript that runs a substitution: a `[` (not after a backslash), a closed `$(…)`, backtick pair or `$'…'`, then a `]`. A subscript assembled from pieces of several words or values (`x='[$(id'; unset "a${x})]"`) is not read, as one split at its `[` never was.
- **Files written, then run** (G0-FP review P-1; post-release). The "shell running a file the text wrote" check only matches `sh FILE` (any shell, with its options) and `source FILE` / `. FILE`, where FILE is a redirect target of the same text. It does not catch:
  - `chmod +x x && ./x`, `bash < x` and `. ./x` (a different spelling of the path);
  - `BASH_ENV=f bash …`;
  - a file written by `tee`, or by any means other than a redirect;
  - a file written in an earlier tool call.
- **A script's arguments** (G0-FP review M-1). A shell given a script file is checked on that file only, not on the words after it, which are the script's positional parameters. So `printf X > p.sh; sh run.sh p.sh` is allowed, even when `run.sh` runs or sources its argument (`. "$1"`). This is the same as `sh run.sh` alone, whose content is never read.
- **Substitution output.** What a `$(…)` prints is not read as a command or a word: `sh -c "$(echo npx …)"`, `for l in $(echo npx …); do eval "$l"; done`, `npm ${x:-$(echo i)} puppeteer`.
- **`xargs -0`.** NUL-separated output from a literal producer is not split into arguments (`printf 'a\0b\0' | xargs -0 …`), in the pipe form as in the `<(…)` form.
- **Contrived stdin-to-shell routes** (review-sh-f3 M-1):
  - `printf -v x %s "$l"; eval "$x"`;
  - `command exec <<< X; xargs …`;
  - `for` over the positional parameters inside `sh -c` (`sh -c 'for a; do eval "$a"; done' _ "$l"`);
  - `set -- $l; "$@"`, and `shift`;
  - a double `eval` (`eval "eval \$$n"`);
  - a function defined inside `eval` (`eval 'f() { sh; }'; echo X | f`).
- **Function calls the body reading does not follow** (review-sh-f4 M-1, M-2). A call's body is read once, with its arguments in place, only for a call among the text's own commands:
  - a wrapper that calls a wrapper (`g() { f "$@"; }; f() { npx "$@"; }; g playwright install`, and deeper chains);
  - a call inside `eval` (`f() { npx "$@"; }; eval 'f playwright install'`);
  - a subshell body (`f() ( npx "$@" ); f playwright install`).
- **Heredocs on the lenient path** (review-sh-f3 M-3). After a broken later line, the lenient split is too complex for `<<<` and for a heredoc right after a compound closer. A heredoc after a redirect word (`done 0<<EOF`, `done 2>/dev/null <<EOF`), on a function call or on `exec` is not matched. The lenient reading of the heredoc body still blocks every such form that has been tried (G1).

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
- **Pin.** `anthology-dev lock write --tag <tag> --checksums checksums.txt --release-json <file>` rewrites both `anthology.lock` files; make the file with `gh release view <tag> --json assets > <file>`. The plugins then ship through `release.sh`, which runs `lock check-fresh` (§8.5).
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
- Dynamic patterns built from `HOME` and the hostname (`memory_privacy._home_literal` and the hostname patterns) use `fancy-regex`.
- **Secret patterns (N-I3).** Once FW-Q merges into `rust`, the quoted-key `password`/`api key`/credential-assignment patterns gain look-aheads, both in the G6 `_SECRET_PATTERNS` and in privacy. `anthology_guard::secrets` and `privacy::SECRET_PATTERNS` are therefore compiled with `pycompat::re::compile`, which picks fancy-regex when it is needed, and the shared-patterns test compares them as `(pattern, flags)`.
- **Inventory.** F4a commits `port/regex-inventory.tsv`, listing every Python pattern with lookaround or backreferences. `ledger check` fails if a merge from `build` adds one that is not in it.
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
- **macOS (m-9).** The CLI-level surfaces also run on the macOS runner. The oracle there is pinned to CPython 3.12.16, the same patch release as the Linux image: a sha256-pinned python-build-standalone build, because `actions/setup-python` stops at 3.12.10 on macOS and tarfile changed in the 3.12 security releases (PR #93). On macOS the `memory.portable` path limit is 1024 bytes, and the oracle's temp dir is `/private/tmp` (`/tmp` resolved through macOS's link). This job **gates**, so `/private/var` realpaths, APFS case-insensitivity, BSD tools and macOS errno text are covered.
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
- the §9.2 test matrix: fake binaries for each exit code; 0-byte, garbage and wrong-arch files; a hang; foreign stderr;
- seeded archives, including a stale seed;
- **real downloaders against the testkit TLS server (m-R5)**: curl and wget via `SSL_CERT_FILE`, node via `NODE_EXTRA_CA_CERTS`, BusyBox wget via its CA bundle path. The real tools parse the real flags, plus slow and never-ending server modes. `PATH` stubs are used only for the "tool absent" cases;
- the session-start budget test: a downloader that never finishes, where `session-start.sh` must print one JSON line within **45 s**;
- a fast download (< 2 s) returns in under 5 s, which proves the watchdog releases stdout (m-R4).

It runs on the four platforms in §8.4. The `tool_name` key scan is fuzzed with proptest.

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
  - `.gitignore` gains `/port/replay/`, `replay-corpus*` and `hmac.key` (any directory; m2-8).
  - A `repo-tests` assertion (`no_replay_tracked.rs`) fails if any tracked path matches `replay-corpus`, `port/replay/`, `runner4-*.tsv` or a file named `hmac.key`.
- **Envelope.**
  - `{"session_id","transcript_path","cwd","permission_mode","hook_event_name":"PreToolUse","tool_name","tool_input","tool_use_id"}`, serialized the way Claude Code does (JS `JSON.stringify`: compact, raw non-ASCII, `\uXXXX` only for control characters and lone surrogates).
  - `tool_input` is **spliced as the raw bytes** from the transcript line, so real encodings reach `loads_bytes_lossy`.
  - `HOME` is the owner's real home.
- **Four runners per call:**
  1. Python `guard.py` (the authority);
  2. `anthology guard hook`;
  3. staged `hooks/guard.sh` with the binary seeded;
  4. staged `hooks/guard.sh` with the binary **missing**.

  Runners 1-3 must agree byte for byte on exit code and stderr. Runner 4 reports bucket counts in the committed report, plus the local-only per-`tool_name` list (§9.4) that never goes into `port/replay-reports/`.

  **Isolation (m-R7).** Runners 3 and 4 run with a **scratch `CLAUDE_CONFIG_DIR`**, so they never read or install into the owner's `~/.claude/anthology/bin`. Runner 3's scratch dir is seeded with the tagged binary. Runner 4 sets `ANTHOLOGY_GUARD_NO_FETCH=1`, so it stays "missing". `HOME` is still the real home, because the commands reference it.
- **Deeper than verdicts.**
  - `shell.analyze` is compared on every Bash `command` and `guard.check` on every call (`Entry` minus `opaque`).
  - `privacy.findings` runs over **every text field** in the corpus (tool inputs, assistant text, tool results), Python against Rust.
- **When it runs:**
  - at CO1, on the tagged binary;
  - after every `build`→`rust` merge that touches `guard_rules.py`, `memory_privacy.py` or `crates/anthology-{shell,guard,pycompat,memory}`;
  - at CO2.
- **Gate.** Zero unexplained differences.
- **Report privacy rule (N-I2, the controller's ruling, verbatim in intent).** The committed report `port/replay-reports/<date>.md` holds **only aggregate counts and HMAC-keyed case ids**:
  - **Case ids** are `HMAC-SHA256(key, canonical envelope)[:16]` hex. The key is 32 random bytes generated once and kept **locally** at `~/.claude/anthology/replay/hmac.key`, never in the repo; `.gitignore` lists `hmac.key` in any directory, and `no_replay_tracked.rs` checks it. Short commands cannot be recovered by guessing without the key.
  - **Never included:** command text, paths, `HOME`, user or host names, transcript content of any kind, tool names, or MCP server names.
  - **Tools are bucketed** as `builtin`, `mcp` or `other`. Every `mcp__*` tool counts as `mcp`, unknown names as `other`. Counts are per bucket and per runner.
  - **Also allowed:** both commit SHAs, the sha256 of the corpus manifest, totals, distinct counts, divergence-class counts (D-1 to D-7), and the case ids of each explained or unexplained difference.
  - **The corpus-string test** greps the report for every corpus string longer than 3 characters. Before searching it removes the report's fixed vocabulary: the template words, the bucket names `builtin`/`mcp`/`other`, the divergence class names, and hex ids. Any hit fails RP1.
- Disagreements become difftest seeds only after manual scrubbing, in a local session; the seed text never goes through the report.
- **Limit (stated).** The replay proves parity on real traffic, not bypass resistance. Generators and monotonicity cover bypass resistance.

---

## 14. CI changes

### 14.1 During the port

**Existing live jobs are untouched until CO2,** including `rtk-test`, `vendor-check` and the Python jobs. F1 adds new jobs only.

| Job | Owner | State after F1 | Runs |
|---|---|---|---|
| `rust-lint` | F1 | **real** | fmt, clippy `-D warnings` |
| `rust-test-linux` | F1 | **real** | `cargo test --workspace --locked` (rust image + git + busybox + node) |
| `rust-test-macos` | F1 | **real** | the same on `macos-15` |
| `deny` | F1 | **real** | `cargo deny check`, plus the `serde_json` tree check |
| `fuzz` (`.github/workflows/fuzz.yml`, weekly + dispatch) | F1 creates; target lanes add their targets' corpora | **real** (runs the targets that exist) | cargo-fuzz, 10 min per target, pinned nightly |
| `shellcheck` | BS1 | placeholder → real | `shellcheck -s sh` on the shims |
| `shims-distros` | BS1 | placeholder → real | `repo-tests` shim suite on alpine, debian-slim and ubuntu (packages installed at job start) |
| `difftest-linux`, `difftest-macos` | F4 | placeholder → real | static musl binaries in the python 3.12 image; CLI surfaces on macOS against a pinned CPython 3.12.16, as on Linux (gating, m-9) |
| `ledger` | F4a | placeholder → real | `anthology-dev ledger check --report`, blocking for the merging lane's files, plus the regex-inventory check |
| `anthology-release` dry run | RL1 | real when added | on PRs touching the workflow or the toolchain |

Placeholders exist only for jobs a named lane fills, and each lane edits only `ci/jobs/<its job>.sh`. Local runs use `scripts/{rust-test,dev,difftest}.sh` in docker. There is no host Rust (owner constraint).

### 14.2 After cutover (no Python anywhere in CI)

- **Removed:**
  - jobs `test`, `test-py39`, `memory-git`, `setup-git`, the Python part of `taste-bridge`, `difftest-*`;
  - `ci/images/python*.Dockerfile`, `tests/requirements.txt`;
  - Dependabot `pip` (F-16).
- **Rewritten to `anthology-dev`:**
  - `vendor-check`, `version-bump`, `memory-diff`, `cli-syntax` (`smoke … cli-syntax`), `upstream-watch.yml`, `evals.yml`;
  - **`startup-budget`** = `anthology-dev startup-budget` plus `cargo test -p repo-tests --test roster startup_budget` (the `test_roster` ports; without `--test roster` the filter also picks a timeout pin in `ci.rs`) plus `cargo test -p anthology-runtime budget` (the `test_session_start -k budget` ports) (m-18).
- **`vendor check`** still needs git and network, as today, and runs no Python: vendored `.py` files are compared as bytes.
- **Evals and smoke** use `anthology-dev seed` on a scratch copy of `plugins/`. The probe U10 decides which config dir `claude plugin eval` gives hooks.

---

## 15. Migration and cutover order

The plan is authoritative for the lane ids and graph (m-10).

**0. Preconditions.**
- `rust` exists (from `build@1e00f832`). F1 starts now; fix-f1, FW-R and FW-Q arrive through `build`→`rust` merges, and lanes always work from `rust` as it stands (N-I3).
- The id list and row corpora are extracted.
- **Double-implementation rule:** a Python behaviour change on `build` needs its Rust equivalent, ledger rows and seeds before the next `build`→`rust` merge goes green (architect's call).

**1. Foundation (F1–F4).** F1 pre-declares **every** shared file, dependency, subcommand, module stub, difftest surface, fuzz target and CI job (plan §0.2). After F1, lanes only add files they own.

**2. Wave 1** (parallel after F): P1, SH1 → SH2 (SH2 after P1), MP1\*, ST1\*, RT1a, RT2, BS1, RL1, DV1\*, DV2, DV4, EV1a, and MT1, which merges **last** in wave 1. Starred lanes code against P1's signatures and merge after it.

**3. Wave 2:** GD1 → GD2 (after SH2); MS1 → MS2; RT1b (doctor, after RT1a, RT2 and ST1; `ROSTER` comes from F1); DV3 (after DV2); RG1 (after SH2).

**4. Wave 3:** MI1 and MX1 (after MS2); RT3 (after RT1a, RT2, ST1, MS2, GD2, BS1); RT4 (after RT1a, RT1b, RT2, ST1, MS2, BS1); RP1, the replay tool (after GD2, MP1, BS1, F4); **EV1b**, the live smokes and the dry eval (after BS1, GD2, RT3, RT4, MI1, MS1, RL1) (N-I4).

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
6. **Merge collisions across 13 parallel lanes.** F1 pre-declaration, per-lane files and serial merges.
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
| A21 | Quarantine on anything but a token allow or a well-formed block; probe failures are strikes; `.bin-sha256`; 3-strikes cap | Recovery without user action; no reinstall loops (m2-1) | One extra download after a transient crash |
| A22 | Bootstrap downloaders: curl, GNU wget, BusyBox wget, node; watchdog total timeout | Alpine, slim and npm installs work | Larger sh surface, covered by the distro matrix |
| A23 | Per-OS errno text tables | Musl static binary prints glibc/macOS text | Cosmetic on musl hosts |
| A24 | One download per session-start hook; `BOOTSTRAP_BUDGET` 20 s; T0-based rtk skip (F-20) | Always inside the 60 s hook timeout | rtk reconciles one session later after a first install |
| A25 | RT1 split: loc+compat (wave 1), doctor (wave 2) | Matches the real import graph | None |
| A26 | Merge discipline: F1 pre-declares shared files; serial merges | Avoids lock and dispatch collisions | Integrator plumbing PRs |
| A27 | Positive allow token (`guard hook --shim` writes `anthology-guard:allow:v1`); block only on 2 with our prefix; `test -s`; `--version` probe at install | Closes the ENOEXEC fail-open on every shell, including BusyBox's NUL-stripping (N-I1, N2-I1) | Environment noise on stderr quarantines a good binary, loudly, until it is capped |
| A28 | `session` mode honours the 3-strike cap, except for one fresh attempt per hour (`.session-retry`) | SessionStart also fires on resume, `/clear` and auto-compaction, so it is not a deliberate retry; an hourly attempt still self-heals a transient bad state (m2-6) | Up to two downloads per hour (one per plugin hook) during a bad release |
| A29 | Replay report: aggregate counts and HMAC-keyed ids only; tools bucketed | Public repo; no personal data | Less diagnostic detail in the committed report (full detail stays local) |

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
| **Re-review 1** | |
| N-I1 | §8.3, §8.4 step 9, §9.2 classification and matrix; plan GC-17, BS1 |
| N-I2 | §13.7 report privacy rule; plan RP1 |
| N-I3 | Header baseline; §11.1 secrets + inventory; plan GC-16, F4a, ledger-based counts |
| N-I4 | §15 (EV1a/EV1b); plan RL1, EV1a, EV1b |
| N-I5 | §7.2, §14.1; plan F1 |
| m-R1 to m-R15 | All accepted; see plan §6. None rejected. |
| **Re-review 2** | |
| N2-I1 | §6.1, §8.3, §9.2 (token classification and matrix), A27; plan GC-5, GC-17, §3.5, GD2, BS1 |
| m2-1 to m2-12 | All accepted; see plan §6. None rejected. |
