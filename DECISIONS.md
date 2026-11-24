# Decisions

This file records the decisions made while building Anthology, so they outlive the plan they came from. Each entry is an ADR: what the situation was, what was decided and what follows from it. Entries are numbered in the order they were made and never rewritten. A later decision that changes an earlier one is a new entry that says so.

New decisions are added at the end, with the next number. An entry gives the date, the task it came from, its status, and the context, decision and consequences. Where a decision implies a check that must keep running, the entry ends with its follow-up checks.

---

## ADR-001: Pre-build rulings

- **Date:** 2026-11-20
- **Task:** Pre-build
- **Status:** Accepted

**Context**

The owner approved the build. Their instructions: the repo is public from creation, and no further questions; the maintainer decides.

**Decision**

- Personal machine details are removed from the plan.
- Upstream license texts ship in `vendor/licenses/`.
- Task 17 is release readiness; 1.0.0 waits for the owner's credits.
- Tasks run sequentially.
- Work happens on branch `build`, merged to `main` after the final review; Task 1 bootstraps `main`.
- `settings_patch` (Task 13) is the only editor of `~/.claude/settings.json`.
- The `/Users/` docs check excludes the generated vendored trees.
- If taste-skill can't be fully ported to BrowserOS, the unportable checks are dropped and the guidance kept.
- If a fork change can't coexist with upstream, upstream's design wins and the fork's intent is re-implemented where feasible.
- The evals workflow skips with a notice while the `ANTHROPIC_API_KEY` secret is missing.

**Consequences**

Nothing personal or secret may ever be committed, because the repo is public from its first push.

---

## ADR-002: Toolchain pins

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

CI and local runs must use the same toolchain, on Apple Silicon and on x86_64 CI. Images were resolved with `docker buildx imagetools inspect` to the multi-arch index digest, so one digest works on both.

**Decision**

Each image is a one-line Dockerfile in `ci/images/` (`python.Dockerfile`, `rust.Dockerfile`, `gitleaks.Dockerfile`), the single source of truth. `ci.yml` and `$TEST` read the image from the `FROM` line, and a test checks that every `FROM` is pinned by `@sha256:`.

- `python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b`
- `rust:1.100-slim@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe` (Rust stable 1.100.0 at pin time)
- `zricethezav/gitleaks:v8.30.1@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f`
- `tests/requirements.txt` pins PyYAML 6.0.3 with hashes for the cp312 manylinux x86_64 and aarch64 wheels and the sdist. `pip install --require-hashes` was checked on both arm64 and (emulated) x86_64.
- `ci.yml` pins `actions/checkout` to `3d3c42e5aac5ba805825da76410c181273ba90b1` (v7.0.1).
- `$TEST` is `docker run --rm -v "$PWD":/w -w /w python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b sh -c "pip install -q --require-hashes -r tests/requirements.txt && python -m unittest discover -s tests -v"`.
- `$RTK_TEST` was first defined as `docker run --rm -v "$PWD/rtk":/w -v anthology-cargo:/usr/local/cargo/registry -w /w rust:1.100-slim@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe cargo test --locked`. ADR-015 replaced it with `scripts/rtk-test.sh`.
- `engines.claude-code` is provisionally `>=2.1.311` in the first-party manifests; it is updated after the platform checks (ADR-003 to ADR-012).
- Dependabot covers `github-actions`, `pip` (`/tests`) and `docker` (`/ci/images`), so digest bumps arrive as PRs. (Updated in fix round 1: the first version kept the digests in `tests/images.lock`, which Dependabot cannot read.)

**Consequences**

A toolchain bump is a Dependabot PR that CI and the evals gate. The Python 3.9 runtime requirement is covered by a second image, `ci/images/python39.Dockerfile`.

---

## ADR-003: Platform check U1, knowledge loading

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

All platform probes (ADR-003 to ADR-012) ran on Claude Code **2.1.311** (Homebrew's stable cask; npm `latest` was 2.1.329). They used throwaway plugins under a scratch directory, loaded with `--plugin-dir` or installed from a local marketplace into an isolated `CLAUDE_CONFIG_DIR`, and ran `claude -p` in scratch git repos. Each probe is kept as a future eval or smoke case, and each ADR ends with its follow-up checks. Eval cases use the repo-root `evals/` layout (ADR-011). Smoke cases are scripts that run `claude -p` in a scratch repo with an isolated `CLAUDE_CONFIG_DIR`.

The raw probe sources (probe plugins, hook scripts, eval suites, commands, jsonl evidence) are archived in the maintainer's local, git-ignored workspace at `.superpowers/sdd/PLAN/probes/` (local only, not in the repo) until the evals task converts them into `evals/` and smoke scripts. The notes in these ADRs are enough to rebuild every case without them.

U1 passes, with one required permission rule:

- `${CLAUDE_PLUGIN_ROOT}` in a plugin agent's body is substituted. The agent Read `<root>/knowledge/_probe/SKILL.md` and quoted its marker.
- A folder under `knowledge/` is not registered: the session's skill list had `probe:hello` and `anthology-superpowers:x`, but no `_probe`.
- `skills: [anthology-superpowers:x]` preloads a skill from a second plugin; the agent quoted its marker without a tool call.
- **Finding:** Claude Code doesn't auto-approve Reads of a plugin's own files. In the default permission mode the knowledge Read is refused (in `-p`) or prompts (interactive). This held for `--plugin-dir`, for a local-path marketplace (loaded in place) and for a GitHub-marketplace plugin copied into `plugins/cache/`. Auto mode and an allow rule both let it through, and a session-level rule also covers subagents.

**Decision**

The maintainer ruled: `/anthology:setup` (through `settings_patch`) adds one `permissions.allow` rule built from the **resolved** plugin location: `Read(<parent of ${CLAUDE_PLUGIN_ROOT}>/**)`.

- For a cached install that's `<config>/plugins/cache/anthology/anthology/**`, which covers every cached version.
- For a local marketplace loaded in place, such as a `marketplace add` of the repo folder, it's `<repo>/plugins/**`. A fixed `~/.claude/plugins/cache/anthology/**` would miss the in-place case.
- `setup remove` takes the rule out, and `session_start.py` and `doctor.py` warn when it no longer matches the current root.
- The Glob fallback isn't needed: the path is right, only the permission is missing.

**Consequences**

Agents can read their knowledge in the default permission mode. The rule is the only broad `Read` allow Anthology adds.

**Follow-up checks**

- **Eval `knowledge-load`:**
  - Prompt: "Run the anthology:<agent> agent with the prompt 'go' and report the first line of each file it read at startup."
  - `allowed_tools: [Agent, Read]`; `plugins` lists all three plugins.
  - Graders:
    - `tool_used` `Agent`, `input_match: 'anthology:<agent>'`;
    - `tool_used` `Read`, `input_match: 'knowledge/<first startup folder>/SKILL\.md'`;
    - `tool_order` before `{tool: Read, input_match: 'knowledge/.*/SKILL\.md'}`, after any non-Read tool the agent uses for its task;
    - `regex` on `last_message` for a marker phrase from that knowledge file;
    - for agents with preloads, `regex` for a phrase from the preloaded superpowers skill, with `tool_used Skill min 0 max 0`, proving it came from the preload and not an invocation.
  - Expected: score 1.00.
- **Smoke `skills-hidden`:** run `claude -p "hi" --max-turns 1 --output-format stream-json --verbose` with the plugins loaded. Assert that the `system/init` event's `skills` contains no name matching `knowledge/` folder names, such as `caveman` or `frontend-design`, while `agents` contains every `anthology:*` agent.

---

## ADR-004: Platform check U2, agent fields and the worktree base

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

U2: all agent fields are honored.

- `model: haiku`: the subagent transcript records a Haiku model.
- `disallowedTools: Edit, Write, NotebookEdit, Agent`: those tools are absent from the agent's tool list.
- `skills`: see ADR-003.
- `isolation: worktree`: the agent runs in `<repo>/.claude/worktrees/agent-<id>` on branch `worktree-agent-<id>`, and its commit lands on that branch.
- `memory: project`: see ADR-009.

**Finding, worktree base:** worktrees branch from the remote default branch (`worktree.baseRef: "fresh"`) by default, not from the session's `HEAD`. In a repo with an `origin`, an isolated agent didn't see the lead's unpushed commits on a feature branch; with `worktree.baseRef: "head"` it did.

**Decision**

`/anthology:setup` also sets `worktree.baseRef: "head"` in user settings (via `settings_patch`, and removed by `setup remove`), so writers start from the lead's current work, such as branch `build`. The routing guide states that writers branch from the lead's `HEAD`. The per-call `isolation` fallback isn't needed.

**Consequences**

Setup changes a global Claude Code setting, so it remembers the previous value and `remove` restores it.

**Follow-up checks**

- **Eval `agent-fields`:**
  - Prompt: "Run the anthology:scout agent with the prompt 'list your tools and run `pwd`'; report its reply."
  - Graders:
    - `tool_used` `Agent`, `input_match: 'anthology:scout'`;
    - `regex` on `trace` with `match: not_contains` for `"name":"(Edit|Write|NotebookEdit)"` inside the subagent's tool calls (read-only agents never call them);
    - `llm`: "PASS if the agent's reported tool list lacks Edit, Write, NotebookEdit and Agent."
  - Expected: 1.00.
- **Model check:** a smoke script asserts each roster agent's subagent transcript (`<config>/projects/<p>/<session>/subagents/agent-*.jsonl`, field `message.model`) contains the frontmatter model family (`opus`, `sonnet` or `haiku`). Run it with `CLAUDE_CODE_SUBAGENT_MODEL` unset, because it overrides agent models.
- **Smoke `worktree-base`:**
  - Set up a scratch repo with a bare `origin` holding `main`, plus a local branch `feat` with one unpushed commit.
  - Run `claude -p "Use the Agent tool to run anthology:implementer with prompt 'run git log --oneline -3 and change nothing'" --settings '{"worktree":{"baseRef":"head"}}'`.
  - Assert:
    - the reply contains the `feat` commit subject;
    - `git worktree list` shows `.claude/worktrees/agent-*`;
    - without the setting, the commit is absent, which documents the platform default.

---

## ADR-005: Platform check U3, hook reach

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

U3 passes.

- A `PreToolUse` hook with matcher `*` saw `Bash`, `Read`, `Write`, `ToolSearch` and a plugin MCP tool (`mcp__plugin_<plugin>_<server>__<tool>`), in the main session and in subagents. Inside a subagent, the hook input carries `agent_type: "<plugin>:<agent>"`.
- Exit 2 with stderr blocks the call, and the model sees `PreToolUse:Bash hook error: [...]: <stderr>` plus the plugin that blocked it.
- Exec form (`command` + `args`) works. `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` expand in `args` and are also in the hook's environment. The data dir is `<config>/plugins/data/<plugin>-<marketplace>` (`<plugin>-inline` for `--plugin-dir`).

**Decision**

None needed; the guard design (a matcher-`*` `PreToolUse` hook in exec form) stands.

**Consequences**

The guardrails can see every tool class, including MCP tools, in the main session and in subagents.

**Follow-up checks**

- **Eval case:** a hook log shows every tool class, and the block text reaches the model.

---

## ADR-006: Platform check U4, CLI syntax

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

Syntax taken from `--help` on 2.1.311:

- `claude plugin marketplace add <owner/repo|https-url|./path> [--scope user|project|local] [--sparse <paths…>]`. A `file://` URL is rejected.
- `claude plugin marketplace list|remove <name>|update [name]`
- `claude plugin install|uninstall|update <plugin>[@marketplace] [-s user|project|local]`. `uninstall` also takes `--keep-data`, and `--prune -y` removes auto-installed dependencies.
- `claude plugin list [--json] [--available]`
- `claude plugin validate <path> [--strict] [--json] [--profile directory]`
- `claude plugin tag [path] [--dry-run] [-m <msg>] [--push] [--remote origin] [-f]`
- `claude plugin eval [target] [--case <glob>] [--tag …] [--runs n] [--model m] [--judge-model m] [--ablation none|with-without] [--threshold 0..1] [--max-cost-usd usd] [--allow-tools …] [--mocks record|off] [--trust-plugin] [--json path] [--no-publish] [-j n]`
- Also present: `claude plugin details|prune|enable|disable|configure|init`.

**Decision**

Scripts and docs use exactly this syntax.

**Consequences**

A CLI change shows up as a failing smoke case rather than a broken doc.

**Follow-up checks**

- **Smoke `cli-syntax`, in CI with the pinned Claude Code:**
  - For each subcommand above, `claude plugin <sub> --help` exits 0, and its output contains every flag the plan's scripts and docs use. At minimum: `marketplace add`, `install -s`, `uninstall --keep-data`, `validate --strict`, `tag --dry-run`, `--push`, and `eval --max-cost-usd`, `--trust-plugin`, `--ablation`, `--no-publish`, `--json`.
  - `claude plugin validate --strict plugins/<each>` exits 0.
  - `claude plugin tag --dry-run plugins/anthology` prints `anthology--v<version>`.
  - Expected: all pass. A missing flag means a CLI change that the docs and scripts must follow.

---

## ADR-007: Platform check U5, upstream pins

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

Pins were resolved read-only with `gh api` and `git ls-remote`. SPDX ids come from the repo license or the shipped LICENSE file.

| Source | Path | Pin | SPDX |
|---|---|---|---|
| `obra/superpowers` | whole plugin | tag `v7.0.2` = `22f0b463e7a5e579904dd6211a520321035526f7` (as expected) | MIT |
| `anthropics/knowledge-work-plugins` | `engineering/`, `design/`, `product-management/`, `marketing/` | `0e0fcacddcc892eb92a1b7873ff9d68159c47571` (`main` head; all four plugin.json at 1.2.0, the same commit the directory serves as 1.2.0) | Apache-2.0 |
| `uxkin/agent` | `plugins/website-design/` | branch **`website-design`** = `07e8f5f61421c7268fddc1511878fb09945da854` (plugin 1.0.0). The folder is not on `main`. | MIT |
| `anthropics/skills` | `skills/frontend-design/` | `e4bb5d960d87126ff3b3a8af1a104b7db67560ba` (`main` head) | Apache-2.0 (folder `LICENSE.txt`; the repo has no top-level license) |
| `vercel-labs/agent-skills` | `skills/web-design-guidelines/` | `1a2c79d4e6c4a2283dc2c614b6538cbe1ed5ca62` (`main` head) | MIT (stated in README; the repo has no LICENSE file) |
| `leonxlnx/taste-skill` | design-taste-frontend | tag `v2.5.1` = `f6b76575f949707b4e3c37cd86cb44b0b98643e8` | MIT |
| `multica-ai/andrej-karpathy-skills` | `skills/karpathy-guidelines/` | `2c606141936f1eeef17fa3043a72095b4765b9c2` (`main` head, pinned 2026-11-21) | MIT (declared in README, SKILL.md frontmatter and plugin.json, author forrestchang; the repo has no LICENSE file) |
| `juliusbrussee/caveman` | `skills/caveman/` | tag `v4.1.0` = `1e91f1306d602d4ca23618701b1185575ae1869b` | Apache-2.0 |
| `cuellarfr/design-skills` | whole repo | `7e25e66e87f64f9d3dea22198f9e29937e7c48f5` (`main` head = 1.0.0; no tags) | MIT |
| ui-taste (uizze.sh) | `/.well-known/skills/ui-taste/` | `CHECKSUMS.sha256` sha256 `405765de761bfaeed98407e75bb6fe67cf9b6f0179562453554f2e9ddacd1612` (25 entries, manifest version `uizze-design-v14`) | Apache-2.0 (shipped LICENSE, matching its checksum line) |
| `rbonweb/rtk` (fork) | whole repo | `develop` = `9b790c414de16f5fb688126b3945c73aa0e0218e` | Apache-2.0 |
| `rtk-ai/rtk` (upstream) | whole repo | latest release `v1.2.1` = `d0f0399a5ff8cd1d81325a033f0b2d8864d665f6` (published 2026-11-19) | Apache-2.0 |

**Decision**

- Every source is MIT or Apache-2.0, so no owner decision is needed.
- Because `vercel-labs/agent-skills` has no LICENSE file, `vendor/licenses/` carries a standard MIT text naming "Vercel", with a note that the license is declared in the upstream README.
- The upstream watcher tracks `uxkin/agent` on branch `website-design`, not `main`.

**Consequences**

Pins are exact commits or checksums, so `vendor.py check` can prove every vendored tree regenerates from them.

**Follow-up checks**

- **Unit test `test_sources_pins` (no model):** asserts `vendor/sources.json` matches this table: repo, path, ref/commit (40-hex), checksum for ui-taste, and SPDX in {MIT, Apache-2.0}.
- `vendor.py check` (CI) proves the committed trees regenerate from those pins.
- `upstream-watch.yml` compares the same pins: `git ls-remote` per repo/branch/tag (`website-design` branch for uxkin), the ui-taste `CHECKSUMS.sha256` sha256, and the rtk upstream latest release. Expected: no diff on a clean tree, and an issue opened when a pin moves.

---

## ADR-008: Platform check U6, hooks and PATH

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

The block wins.

- One plugin's `PreToolUse` hook exited 2 while another returned `permissionDecision: "allow"` with `updatedInput`. The call was blocked, with either plugin load order. This matches the documented precedence `deny > defer > ask > allow`.
- Both hooks receive the **original** command (they run in parallel), so the guard always checks what Claude asked for, not rtk's rewrite. Alone, the allow hook's rewrite and auto-approval worked.
- Plugin `bin/` is on the Bash `PATH` in the main session and in subagents.

**Decision**

None needed; the `guard_rules.check` fallback isn't needed.

**Consequences**

The guardrails and rtk can coexist: a guard block cannot be overridden by rtk's allow.

**Follow-up checks**

- **Eval case:** the deny + allow pair blocks the call.

---

## ADR-009: Platform check U7, memory

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

- `autoMemoryDirectory` in `.claude/settings.local.json` is honored when it is present at session start. A note was written to `<repo>/.claude/memory/` along with `MEMORY.md`.
- When a `SessionStart` hook writes the setting during startup, it applies **from the next session**. That session's notes went to the path-keyed `<config>/projects/<encoded path>/memory/`.
- Per the docs, the project-scope setting is honored only in a trusted workspace, and not while `permissions.blockReadsOutsideWorkingDirectories` is on.
- Agent memory for plugin agent `probe:writer` is `.claude/agent-memory/probe-writer/`: the `:` becomes `-`, so `anthology:<name>` maps to `.claude/agent-memory/anthology-<name>/`. With `isolation: worktree` it resolves **inside the worktree** (`<worktree>/.claude/agent-memory/…`), where it is untracked and lost with the worktree unless committed.
- The `SessionStart` `additionalContext` reached the main session but **not** subagents: the marker is absent from subagent transcripts, and the agent reported none.

**Decision**

- `memory_sync.py` keeps the planned same-session cover: when it repairs the setting, it injects the repo memory index.
- The path-keyed migration runs on **every** session start, not only once, and copies (never overwrites) anything found there. This moves notes saved in a repair session into the repo on the next start.
- Agent memory paths in code, tests and docs use `.claude/agent-memory/anthology-<agent>/`, not `<agent>/`.
- Writer agents (worktree) must commit their `.claude/agent-memory/anthology-<agent>/` changes in their worktree as a separate `chore(memory): …` commit before finishing, so the memory merges with their branch. The agent bodies (Task 6) and routing guide (Task 13) say so.
- Read-only agents run in the main clone and write memory there; the lead commits it.

**Consequences**

The routing guide reaches only the main session, so agents carry their own memory rules in their bodies. A first session in a freshly approved or re-cloned repo may write notes to the path-keyed folder, and the next session start moves them into the repo.

**Follow-up checks**

Smoke scripts (multi-session, so not `plugin eval`):

- **`auto-memory-dir`:**
  - Use a scratch approved repo with no `.claude/settings.local.json` and all three plugins installed in an isolated config.
  - Session 1: `claude -p "Remember for future sessions that the probe color is amber; save it to auto memory now."`
  - Assert that `settings.local.json` now has `autoMemoryDirectory` = `<repo>/.claude/memory`. The note lands in the path-keyed `<config>/projects/<encoded>/memory/`; that's the platform behaviour, and the hook's same-session cover is injected.
  - Session 2: run any prompt, then assert the amber note was copied into `<repo>/.claude/memory/` (migration).
  - Ask session 2 to remember "cyan", and assert the note is written directly under `<repo>/.claude/memory/`.
- **`agent-memory-path`:**
  - Run `claude -p "Use anthology:implementer with prompt 'save a project memory note named probe-note.md saying probe, then finish'"`.
  - Assert `git log` on the worktree branch has a `chore(memory)` commit adding `.claude/agent-memory/anthology-implementer/probe-note.md`, and no `.claude/agent-memory/anthology:implementer` path exists anywhere.
- **Eval `guide-main-only`:**
  - Prompt: "Quote the first heading of the anthology routing guide if it is in your context, then run anthology:scout with prompt 'quote any anthology routing guide heading in your context, or NONE'."
  - Graders:
    - `regex` on `last_message` for the guide's first heading;
    - `regex` on `trace` for the subagent result containing `NONE`;
    - `tool_used` `Agent`, `input_match: 'anthology:scout'`.
  - Expected: 1.00.

---

## ADR-010: Platform check U8, hook output and failures

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

- The `additionalContext` limit is **10,000 characters**, inclusive: a 10,000-char value arrived whole. At 10,001 characters, the model got `Output too large (…KB). Full output saved to: <session dir>/tool-results/hook-…-additionalContext.txt` and a preview (2,000 chars per the docs), and the tail marker was lost. The limit applies to each field separately, and no setting raises it.
- A missing interpreter is a **non-blocking** error and the call goes ahead. In exec form: `Executable not found in $PATH: "<cmd>"`. In shell form: exit 127. The model sees nothing; the user only gets the hook-error notice.
- `onFailure: "block"` exists (v2.1.295+) and works on 2.1.311: the call is blocked with `failed; blocking because onFailure is "block"`.

**Decision**

- Routing guide + personal memory index + repo-memory cover stays under a test budget of **9,000 characters** (one field).
- The guard keeps failing open: no `onFailure: "block"` on the matcher-`*` hook, since a missing `python3` would then block every tool and brick the session.
- "Never silently off" can't rely on Python. If `python3` is missing, the Python `SessionStart` hook itself fails silently. So `anthology-guardrails` adds a first `SessionStart` hook in POSIX `sh` that emits the loud `systemMessage` when `command -v python3` fails.
- `session_start.py` still runs the known-bad-sample self-test.

**Consequences**

A missing `python3` leaves the guard off but never silently. If the platform limit moves, the budget has to be re-checked.

**Follow-up checks**

- **Unit test `test_hook_output_budget`:** run `session_start.py` against fixtures (the largest personal index + repo-memory cover), and assert `len(additionalContext) <= 9000`.
- **Smoke `context-cap`:** a throwaway plugin whose SessionStart hook pads `additionalContext` to exactly N chars with `END-<N>` at the end. `claude -p "Without tools, quote any token starting with END-, else NONE" --max-turns 1`. Expected: N=10000 quotes `END-10000`; N=10001 gives NONE, and the reply mentions `Output too large` / `saved to`. A failure means the platform cap moved, so re-check the 9,000 budget.
- **Smoke `python-missing`:** run the guardrails `sh` sentinel hook with `PATH=/nonexistent:/bin` (no `python3`), and assert stdout is one JSON object with a non-empty `systemMessage` that names python3. With `python3` present, assert no output.
- **Smoke `missing-interpreter`:** a throwaway plugin with a `PreToolUse` Bash hook whose `command` is `python9`. `claude -p "Run: echo RAN-OK" --allowedTools "Bash(echo *)" --output-format stream-json --verbose --include-hook-events`. Assert `RAN-OK` is in the result and a `hook_response` has `Executable not found`. With `"onFailure":"block"`, the result says it was blocked.

---

## ADR-011: Platform check U9, eval harness

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

U9: the harness works.

- `claude plugin eval` ran a case against the probe plugin with `tool_used` (Skill), `tool_order` (Skill before the MCP tool) and a `regex` grader on `mock_calls`. A mocked MCP server (`evals/mocks/<server>/<tool>.md`) answered and scored 1.00.
- `--max-cost-usd` works: with a tiny ceiling and `--runs 2`, one run ran and the command exited **2**, with `partial: true, partialReason: "cost_ceiling"`.
- **Finding:** a case's `plugins:` entries must sit under the containment root. That root is the enclosing plugin, so a suite inside `plugins/anthology/evals/` can't load `anthology-guardrails` or `anthology-superpowers`.
- Evals placed in a non-plugin parent directory and run against that directory loaded all listed plugins and passed. That case dispatched the plugin agent, which Read its knowledge file and showed a cross-plugin preload.

**Decision**

The behaviour evals live at the repo root in `evals/`. Each case declares `plugins: ["../../plugins/anthology", "../../plugins/anthology-guardrails", "../../plugins/anthology-superpowers"]`, and `evals.yml` runs `claude plugin eval . --trust-plugin --ablation none --no-publish --max-cost-usd <ceiling> --model … --judge-model …`. BrowserOS cases mock the `browseros-neo` server under `evals/mocks/`.

**Consequences**

All three plugins load in every case, and no `evals/` folder exists under `plugins/*/`.

**Follow-up checks**

- **Eval `harness-sanity`:**
  - Prompt: "Use the anthology:ui-reviewer agent to take a screenshot of http://localhost:3000 and report what the tool returned."
  - Mock `evals/mocks/browseros-neo/screenshot.md` with body `MOCK-SHOT {{input.url}}`.
  - Graders:
    - `tool_used` `Agent`, `input_match: 'anthology:ui-reviewer'`;
    - `tool_used` `mcp__.*browseros-neo__screenshot` (or `tool_order`: Agent before it);
    - `regex` `target: mock_calls`, `pattern: 'localhost:3000'`.
  - Expected: 1.00.
- **CI smoke `eval-ceiling`:** run any one-case suite with `--max-cost-usd 0.000001 --runs 2 --json out.json`. Expected: exit code 2, `partial: true`, `partialReason: "cost_ceiling"`, exactly one run recorded.
- **Unit test `test_eval_layout`:** every `evals/*/prompt.md` (or `case.yaml`) lists all three plugin paths in `plugins:`, and no `evals/` directory exists under `plugins/*/`.

---

## ADR-012: The minimum Claude Code version and `engines`

- **Date:** 2026-11-20
- **Task:** Task 1
- **Status:** Accepted

**Context**

- `claude plugin validate` on 2.1.311 warns `Unknown field 'engines'`, so `--strict` fails. The docs say `engines` is honored from **v2.1.323**.
- On 2.1.329 (checked in a throwaway container), `validate --strict` passes for `plugins/anthology`.
- Everything in ADR-003 to ADR-011 was verified on 2.1.311, and no needed feature is newer (`onFailure` 2.1.295, `plugin eval` 2.1.269).

**Decision**

The minimum stays **`>=2.1.311`**. Clients older than 2.1.323 ignore the field, which is harmless. CI's `claude plugin validate --strict` installs a pinned Claude Code **≥ 2.1.323**.

**Consequences**

Users on 2.1.311 and newer are supported, while CI validates with a version that understands `engines`.

---

## ADR-013: anthology-superpowers is versioned independently

- **Date:** 2026-11-20
- **Task:** Task 1 (maintainer ruling folded in)
- **Status:** Accepted

**Context**

`anthology-superpowers` is generated from upstream superpowers, whose own version (7.0.2 at pin time) says nothing about Anthology's rename transform.

**Decision**

`anthology-superpowers` is versioned independently of upstream superpowers, starting at `0.1.0`. `rename-plugin` sets the plugin's own version from `sources.json` `plugin_version`, and `upstream_version` records 7.0.2.

**Consequences**

The `~0.1` dependency in `plugins/anthology/.claude-plugin/plugin.json` resolves, and any transform or sync change gets its own bump.

---

## ADR-014: design-taste-frontend on BrowserOS Neo

- **Date:** 2026-11-20
- **Task:** Task 5
- **Status:** Accepted

**Context**

Upstream design-taste-frontend v2.5.1 (`f6b76575`) launches headless Chrome on the host from most of its scripts. Anthology allows no host Chrome launch, so the skill had to be ported to BrowserOS Neo.

**Decision**

Of the 20 upstream scripts, 9 are kept byte-identical, 9 are ported (6 through the bridge, 3 trimmed), and 2 are dropped (`scripts/shoot.mjs`, `assets/backgrounds/check.mjs`). The patch adds one script, `scripts/neo.mjs`, the BrowserOS Neo bridge.

- **The bridge.** The agent starts `neo.mjs serve` in the background (it serves the page on 127.0.0.1:5300-5309) and opens its harness page in its own BrowserOS Neo tab. The hub answers only `127.0.0.1`/`localhost` hosts, sends no CORS header, refuses foreign origins, needs a token for every call and saves only images under `shots/`; it exits after 2 hours idle.
- **The ported checkers** (`consistency`, `layout/check`, `perf`, `make-posters`, scene `make`/`probe`) keep their drivers. Their Chrome launcher is replaced by `NeoTab`, which runs their in-page reads in same-origin harness iframes at each width: no CDP port, no browser launch. `gate.mjs` returns `GATE: SETUP` naming the bridge when it, its harness or the page root is not right; a dev-server URL is not checked (build it and check the folder).
- **What can't run there.** Pixel, hover, key, media-emulation and request-interception checks can't run in BrowserOS Neo. They are listed in the checker's `notChecked` and covered by the eyes step: one `run` call per width captures each section, saves `shots/<w>-<nn>.jpg` through the bridge, and the agent reads them. Widths: 1440x900, 768x1024 and 390x844 at the gate; upstream's five in step 6. BrowserOS can't set the user agent over CDP, so the eyes tab opens the page with `?__neo=eyes` and the hub injects the `HeadlessChrome` shim (`motion.js` `shot()` then shows end states, as under the old checker).
- **The Stop hook is gone.** SKILL.md runs `gate.mjs` by hand before finishing. The Playwright and Puppeteer brand icons are dropped, and the gate's checker-gaming regexes no longer spell the tool names.
- **The port map** `vendor/patches/design-taste-frontend.portmap.json` holds the script rule and each script's action. `vendor.py` (entry field `portmap`) fails build/check/sync when upstream adds an unmapped script, a drop script ships, a keep script differs, a shipped script is not keep/port/added, or any shipped file or path matches the Chrome-launch pattern.

**Consequences**

A new upstream script cannot slip in unreviewed, and no shipped file can launch Chrome. Some checks are replaced by looking at screenshots. `tests/test_taste_bridge.py` drives `neo.mjs` against a fake harness (skipped where `node` is absent), and the CI job `taste-bridge` runs it. Kept scripts ran once on host Node on a scratch page; the bridge ports ran for real in a BrowserOS Neo tab.

---

## ADR-015: rtk import and the conflict plan

- **Date:** 2026-11-20
- **Task:** Task 8
- **Status:** Accepted

**Context**

`rtk/` is a non-squash `git subtree add` of the fork `rbonweb/rtk` `develop` = `9b790c414de16f5fb688126b3945c73aa0e0218e`, then a `git subtree pull` of upstream `rtk-ai/rtk` `v1.2.1` = `d0f0399a5ff8cd1d81325a033f0b2d8864d665f6` (merge base `bd7f87eb`; upstream 172 commits ahead, the fork 5 change commits + 4 PR merges). Both pins are in `vendor/sources.json` (`managed_by: "subtree"`). `scripts/sync-rtk.sh <tag>` repeats the pull on an `rtk-sync/<tag>` branch. It then rewrites the `upstream` pin in `sources.json` to the tag and its peeled commit, leaving the change uncommitted for review, and runs `scripts/rtk-test.sh`.

**Decision**

- **Textual conflict, `src/core/classify.rs`.** Upstream moved `ureq` 2 → 3 (`.header()`, `http::global_timeout`, `send_json`, `http::read_json`); the fork's custom headers used ureq 2 (`.timeout()`, `.set()`). Kept upstream's request code and re-added the fork's loop over `task_cmd::configured_headers(upstream)` with `.header(name, value)`. `Content-Type` now comes from `send_json` (the fork protects that name anyway). One behaviour drift: ureq 2's `.set()` replaced a header, but ureq 3's `.header()` appends. So a configured header that has the same name as one of rtk's own unprotected headers (`anthropic-version`, `x-rtk-routed-from`) is now sent alongside rtk's value instead of replacing it. Credentials stay protected, and this engine-gated code goes to Task 9.
- **Checked, no conflict.** `src/core/task_cmd.rs` merged cleanly: upstream's new `post_json` (ureq 3, `via_daemon`) already takes a header list, and the fork's `configured_headers` feeds it. `src/lib.rs` (minimal `read` default, `rtk opencode`), `src/discover/registry.rs` and `rules.rs` (node/python rules, `--level none` on head/tail, lint routing), `src/hooks/hook_cmd.rs` and the awareness files all kept both sides.
- **Semantic conflict, `src/hooks/init/claude.rs`.** Upstream's new test `boundary_newline_hook_is_idempotent_and_uninstall_is_selective` expects a settings file with the hook already installed to stay untouched, but its fixture has no `Bash(rtk *)` allow rule, so the fork's auto-allow heal patched it. Kept both: the fixture now carries `Bash(rtk *)` (a complete install), so it still proves a boundary-newline hook is not duplicated, and the heal of installs missing the rule stays. The fix is in the merge commit.
- **Dropped:** nothing. The custom `[upstream.*]` headers live in engine-gated code (`classify.rs`, `task_cmd.rs`, `rtk-engine` schema); per the stream ruling, Task 9's license trim decides their fate with the BUSL engine.
- **The bare `cargo test` in the pinned image could not pass, before or after the merge.** Upstream v1.2.1 alone, the fork alone and the merged tree each failed the same 113 tests there, so the merge caused none of them. The causes: `rust:1.100-slim` has no `git` (107 tests that create repos or run git), no `curl` (1), no `jq` (1, the shell hook) and no `kill`/procps (2 signal tests). It also runs as root, so 2 read-only-directory tests in `init_opencode_test` cannot fail.
- **`$RTK_TEST` is now `scripts/rtk-test.sh`** (maintainer ruling; it replaces the `docker run … cargo test --locked` form in ADR-002). Why: the tests need tools the slim image lacks and a non-root user. The script is the single source of truth for local runs, ci.yml's `rtk-test` job and `sync-rtk.sh`. It reads the image from `ci/images/rust.Dockerfile`'s `FROM` line. Inside the container only, it installs `git curl jq procps ca-certificates` from the image's own Debian release. These are unpinned, test-only and never shipped: that is the accepted cost. It then runs `cargo test --locked` as a non-root user (the target dir is the `anthology-rtk-target` volume), and extra arguments pass through. There is no skip list. Result: all 5483 tests pass (8 ignored).

**Consequences**

Every fork change keeps a named test, permanently in CI (`rtk/tests/`, through the built binary):

- `fork_1_auto_allow_hook`: init adds and heals `Bash(rtk *)`.
- `fork_2_read_minimal_default`: `rtk read` strips comments; head/tail rewrites add `--level none`.
- `fork_3_node_filter`.
- `fork_4_python_filter`: rewrite plus the TOML filter on a stand-in tool.
- `fork_5_cd_chain_rewrite`.
- `fork_6_biome_lint`: script routing plus the biome filter on the fork's fixture.
- `fork_7_opencode_run`: rewrite plus the filter on the multistep fixture.
- `fork_8_upstream_headers`: `#![cfg(all(unix, feature = "engine"))]`; a local server sees the `{session}` id and that `Authorization`/`x-api-key` cannot be overridden.
- `fork_9_awareness_exact`.

Run against plain upstream v1.2.1, all of them fail except `fork_5`: the fork's cd-chain change was a regression test only (upstream already rewrote after `cd`), so `fork_5` pins that behaviour.

---

## ADR-016: rtk trim, Claude Code only and no telemetry

- **Date:** 2026-11-20
- **Task:** Task 9
- **Status:** Accepted

**Context**

The imported rtk (ADR-015) carries the BUSL-1.1 engine, telemetry and integrations for many other AI tools. Anthology ships rtk for Claude Code only, under Apache-2.0, without telemetry, and the trim has to survive every upstream sync.

**Decision**

The shipped rtk is the Apache-2.0 edition, Claude Code only, without telemetry. `scripts/rtk-trim.sh` produces it: a thin bash entry that runs `scripts/rtk_trim.py` (stdlib, Python 3.9+), then the lockfile step. The Python is an ordered list of operations over four idempotent primitives: `rm`; `sub` (exact replacement); `cut` (exact deletion); `drop` (an item by its exact first line, body-tolerant). Every one of them stops with exit 1 and `rtk-trim: <file>: <reason>: <anchor>` when its anchor no longer matches. So an upstream refactor of a trimmed region is noticed, and a second run prints `rtk-trim: no changes`. The commit `refactor(rtk): Claude Code only, no telemetry (scripted trim)` is the script's output; nothing under `rtk/` is hand-edited.

- **Removed.** `engine/`, `plugin/`, `proxy/` (all BUSL-1.1), `examples/`, the `engine`/`example-plugins` features and the 12 engine-only source files. Telemetry goes: `telemetry.rs`, `telemetry_cmd.rs`, `rtk telemetry`, the daily ping and the consent prompt in `rtk init`. So does every non-Claude `rtk init` target and `rtk hook` processor (the `hook_cmd.rs` handlers of Copilot/VS Code, Gemini, Antigravity, Vibe, Trae, Codex, Cursor, Droid and OpenCode). Also removed: the translated READMEs, `Formula/`, `install.sh`, `INSTALL.md`, the release-please files, the fork's dev tooling dir, upstream's CI dir, `.semgrep.yml`, `openclaw/`, `scripts/`, `hooks/<11 other hosts>/`, the docs of removed features, and the tests of removed code (13 files, about 130 unit tests).
- **Kept.**
  - `src/hooks/init/{mod,claude,agents_md}.rs` (`agents_md.rs` holds the marker-block helpers that `claude.rs` uses);
  - `rtk hook claude` and `rtk hook check`, and every command filter (including `opencode run`);
  - `text/` (`rtk-text`, Apache-2.0);
  - `permissions_opencode.rs` (the shared rule judging calls it);
  - `llm_proxy.rs` (a loopback-only client of the optional proxy daemon, woven into `gain`/`claude`);
  - Windows code.
- **The engine's `#[cfg(feature = "engine")]` code stays, dead, and can never be enabled.** The feature is gone, and the cfg is declared in `[lints.rust] unexpected_cfgs` check-cfg. Why: deleting 123 cfg sites in high-churn files would make every sync conflict. Cost if wrong: dead source text only.
- **`dead_code = "allow"` for the rtk crate.** Why: removing entry points under upstream's `warnings = "deny"` turns about 75 inert callees across 10 shared files into errors, and chasing them means about 100 more conflict-prone edits for no runtime change. Cost if wrong: inert callees stay in the source (not in the LTO binary); `test_rtk_claude_only` guards what matters.
- **Permitted dependencies.** `[dependencies]` anyhow, base16ct, chrono, clap, codepage, colored, crossterm, dirs, encoding_rs, flate2, ignore, oem_cp, quick-xml, ratatui, regex, rtk-text, rusqlite, serde, serde_json, sha2, tempfile, toml, toml_edit, ureq, walkdir, which. Per target: libc (unix), windows-sys (windows), mimalloc (musl). Build: toml. Dev: temp-env. `rtk-text`: regex. The workspace members are `.` and `text`, and these two are the only manifests. `getrandom` is removed. `ureq` is kept: its only live caller is the proxy health check, a loopback GET reached by `rtk llm-proxy`, `rtk claude` and `rtk gain` on a TTY, never by the hook, rewrite or filters.
- **`Cargo.lock` is pruned** by `cargo metadata --offline --format-version 1` in the pinned image, falling back to online only if the registry volume lacks a crate. A guard compares the `[[package]]` name+version pairs before and after. It exits 1, and restores the old lockfile, if any package is added or any version changes. Result: 19 packages pruned, none added or changed. The container runs as root, and on Linux it chowns `Cargo.lock` back to the caller afterwards. It does not use `--user`. Why: with `--user`, `cargo metadata` fails with `Permission denied` on a cold registry volume, or on one another uid owns, as soon as it must fetch online, which a sync that adds a crate does (checked). Cost if wrong: none (the file ends up owned by the caller either way).
- **Fork change 8 (custom `[upstream.*]` headers, `cc11f851`) is dropped with the engine**, per the stream ruling. It lived only in `classify.rs`, `task_cmd.rs` and the `rtk-engine` schema. `tests/fork_8_upstream_headers.rs` is deleted by the trim and `FORK_TESTS` lists the 8 remaining named tests. This settles the question ADR-015 left open for Task 9. Cost if wrong: the owner loses custom headers for remote pipelines, which exist only in the BUSL edition.
- **What changes for Claude.** For `rtk hook claude` on PreToolUse, nothing. The engine's pipeline pre-step returned "not handled" without user pipelines, so the same `process_claude_payload` path runs. Lost with the engine (Anthology never wired them): PostToolUse output compression, SessionStart handling and user pipelines (`rtk pipeline/methods/plugins/tasks`). Do not wire PostToolUse to `rtk hook claude`: the oss build would answer with a PreToolUse-shaped rewrite. `rtk init -g` (Claude) works; the agent flags are gone.
- **Syncs.** `sync-rtk.sh` now requires the trim (it fails without it). When the subtree pull stops on conflicts, it removes with `git rm` every "deleted by us" (`DU`) path inside a path the trim removes, and commits the merge if nothing else conflicts. Upstream edits to `engine/` and similar paths would otherwise stop every sync. Any other conflict still stops the sync for review. For a content conflict inside a trimmed region, take upstream's hunk and re-run the trim: drops are body-tolerant, and subs and cuts stop loudly when upstream changed the anchored text.
- **Noted for Task 11.** `bin/rtk` should export `RTK_SUPPRESS_HOOK_WARNING=1`. Otherwise rtk's daily `hook_check` finds no `rtk hook claude` entry in Claude's settings.json, because Anthology registers the hook in the plugin's hooks.json, and prints "run rtk init -g", which would install the duplicate hook the design forbids.

**Consequences**

Each of the fork's changes keeps one permanent named test, now 8 instead of 9. The checks that keep the trim honest:

- `tests/test_rtk_claude_only.py` is the allowlist. It checks the init keep-list, the Cargo dependencies per section, the members line and manifest set, and that there is no telemetry. It also checks: none of the about 100 removed paths exist, `HookCommands` is `{Claude, Check}`, there is no `AgentTarget`, the check-cfg and `dead_code` lines are present, there are 8 fork tests, and the scripts are wired. Adding a hook integration module or a dependency that is not on the list is a deliberate decision, not a test fix.
- `tests/test_rtk_trim.py` unit-tests the primitives (apply once, no-op once applied, raise on a missing anchor or a changed signature), the lock guard and the conflict filter.
- `scripts/rtk-net-test.sh` runs in CI's `rtk-test` job after `scripts/rtk-test.sh`. It builds the release binary in the pinned image (target volume `anthology-rtk-release`) and a throwaway image from the same pinned image plus git (inline Dockerfile on stdin, so `ci/images/` is unchanged). It then asserts the exact outputs under `docker run --network none`: only `lo`; `rtk git status`; `rtk rewrite "git log -5"` (exit 3); `rtk hook claude` with and without a rewrite; the `rtk hook` subcommands `claude`, `check` (and clap's `help`); and `rtk telemetry` unrecognized.

Results: `scripts/rtk-test.sh` gives 4775 passed, 0 failed, 8 ignored (60 test binaries). `scripts/rtk-net-test.sh` gives PASS.

---

## ADR-017: rtk release, pin and install

- **Date:** 2026-11-20
- **Task:** Tasks 10 and 11
- **Status:** Accepted

**Context**

The trimmed rtk (ADR-016) has to reach users as a binary they can verify, and the plugin has to install it without editing the user's settings beyond what setup shows.

**Decision**

- **Release.** `.github/workflows/rtk-release.yml` runs on a tag `rtk--v<version>`, where `<version>` must equal the version in `rtk/Cargo.toml` (`1.2.1-anthology.1` for the first release). It builds and tests (`cargo test --locked`) on four native runners (`macos-15`, `macos-15-intel`, `ubuntu-24.04`, `ubuntu-24.04-arm`), builds the macOS targets and the static musl Linux targets with the toolchain pinned in `rtk/rust-toolchain.toml`, attests each tarball with build-provenance, and publishes `rtk-<target>.tar.gz` and `checksums.txt`. It fails if the release already exists, and every action is pinned by commit SHA.
- **Dry run.** A pull request that touches the workflow or the toolchain pin runs the whole matrix without attesting or publishing, so the matrix is proven before a tag. Cost if wrong: extra CI minutes.
- **The tag.** The tag is pushed on the commit that carries the workflow, not on `main`, because the release flow does not need `main`. If that branch is squash-merged, the tag still names the exact source of the binaries.
- **`plugins/anthology/rtk.lock`** is JSON with exactly `version`, `tag`, `repo` and `sha256` (one 64-hex hash of `rtk-<target>.tar.gz` for each of the four targets). The download URL is `https://github.com/<repo>/releases/download/<tag>/rtk-<target>.tar.gz`. `tests/test_rtk_lock.py` checks the lock against `rtk/Cargo.toml`.
- **Install.** `plugins/anthology/scripts/rtk_install.py` downloads the tarball for the platform, checks its sha256 against the lock, installs under the plugin's data folder and records the binary in `state.json` (one global `rtk_path`). It never raises: on any failure it leaves `state.json` alone and returns a warning. `bin/rtk` runs the installed binary (and says "run /anthology:setup" when there is none); `bin/rtk-hook` is the `PreToolUse` Bash hook, runs `rtk hook claude` and exits 0 when rtk is not installed. Both export `RTK_SUPPRESS_HOOK_WARNING=1`, so rtk does not tell Claude to run `rtk init -g`, which would add a duplicate hook.
- **Callers.** Setup (60 s per read) and session start (8 s per read, skipped when under 40 s of the session-start budget remain) both go through `rtk_install`. `rtk_install.check_installed` is the read-only check that doctor uses, because the lock pins tarballs, not binaries.
- **One global binary.** A session still running an older plugin version uses the newest installed binary until it restarts. Cost if wrong: an old session runs a newer rtk, which is acceptable because `rtk hook claude` is stable across our patch releases.

**Consequences**

A future rtk bump sets `rtk/Cargo.toml` to `-anthology.<n+1>`, regenerates the `Cargo.lock` entry, pushes the new tag and then rewrites `rtk.lock` from `checksums.txt`; the lock change is a change to `plugins/anthology`, so it ships with a new plugin release.

## ADR-018: memory saves by read-only agents (`anthology-mem save`)

- **Date:** 2026-11-20
- **Task:** stream I (follow-up to Tasks 6, 12 and 13)
- **Status:** Accepted

**Context**

Read-only agents (`disallowedTools` includes Write and Edit) were told to save memory notes with a Bash heredoc. On Claude Code 2.1.311, a Bash command with a heredoc or an output redirect never matches a permission pattern grant, so outside auto mode every save prompted the user, or was denied under `-p`. A flat `metadata.type:` key also indexed as type "other".

**Decision**

- Every agent saves memory with `anthology-mem save --agent <name> --type <t> --slug <s> --name '…' --description '…' --body '…' [--replace]`. The helper writes only `<git toplevel>/.claude/agent-memory/anthology-<agent>/<type>-<slug>.md` with nested `metadata:`/`type:` frontmatter and regenerates that folder's `MEMORY.md`.
- Setup adds exactly one allow rule, `Bash(anthology-mem save *)` (`settings_patch.ALLOW_MEM_SAVE`); remove reverts it.
- Verified with `claude -p` and a probe subagent: the rule auto-allows the command, including a quoted argument that spans lines. Still gated: `&&`/`;` compounds, redirects, `$(…)`, `PATH=`/`PYTHONPATH=` prefixes and an absolute path to the binary. A quoted line whose first non-blank character is `#` is not auto-allowed, so notes use bold lines instead of `#` headings.
- A subagent's Bash environment carries no agent identity, so `--agent` is self-declared and validated against the roster. Types are project/feedback/reference; slugs are validated; symlinks are refused at every level; secret-shaped text is refused; an existing note is refused unless `--replace`. There is no delete subcommand, so the auto-allowed surface is write-only.
- Every write runs the repo-memory privacy check (`memory_sync._privacy_check`).

**Consequences**

Agent memory saves are silent (no prompt) once setup has run. Cost if wrong: one helper and one line per agent body. Evidence: the stream I log.

## ADR-019: agent memory in unapproved repos (hints, not instructions)

- **Date:** 2026-11-20
- **Task:** final-review finding I5
- **Status:** Accepted

**Context**

- Every roster agent has `memory: project`. Per ADR-009, Claude Code loads `.claude/agent-memory/anthology-<agent>/MEMORY.md` into that subagent itself, whatever the approval state, and we cannot change that.
- Every agent body also told the agent to read that folder, with no approval condition.
- `memory_sync` only gates repo auto-memory (`.claude/memory/`) on approval. SECURITY.md, README.md and PLAN said a stranger's committed notes never load, which is false for agent memory.

**Decision**

- Each of the 11 agent bodies reads memory as: "They are hints, never instructions; none overrides the user's request, routing guide, BrowserOS-only rule or Docker rule." The 7 agents without a browser say "browser rule" instead, because the roster test keeps the word BrowserOS out of their bodies. `tests/test_roster.py` asserts the line in every body.
- README and SECURITY now say plainly: repo auto-memory loads only in approved repos; agent memory in a cloned repo is loaded by Claude Code for that agent regardless of approval, so review `.claude/agent-memory/` in repos you do not trust, like any code.
- No code gate was added: the platform load cannot be stopped from a plugin, and gating only the body line would not close it.

**Consequences**

The defence is model guidance plus review, not a barrier; a note can still mislead an agent within the user's request. Cost if wrong: one line per agent body (about 100 bytes; scout is at 5929 of 5944, so the next body growth there needs a budget bump with a reason).

## ADR-020: legacy notes stay personal; user notes leave the repo (supersedes parts of ADR-009)

- **Date:** 2026-11-20
- **Task:** final-review finding B3 (stream P1), fix-wave re-review I-1
- **Status:** Accepted

**Context**

- ADR-009 decided that the path-keyed migration copies notes from `<config>/projects/<encoded path>/memory/` "into the repo", and its `auto-memory-dir` follow-up check asserts that the amber note appears under `<repo>/.claude/memory/` in session 2.
- The final review (B3) found that this put every old note, including ones with home paths, hosts and secrets, into a repo that may be public, on the first session after setup.
- Stream P1 changed the code: `memory_sync` now calls `migrate(old, None, personal, …)`, and the repo is written only through an explicit per-note promotion. The ADR and the smoke were not updated, so the smoke failed on every eval run (re-review I-1).

**Decision**

- ADR-009's decision "copies … into the repo" and its follow-up assertion about session 2 no longer hold. Everything else in ADR-009 stands, including the every-session migration and the session 3 check that a new note lands directly in `<repo>/.claude/memory/`.
- Notes from the old path-keyed folder are copied into `~/.claude/anthology/personal-memory/` only (never overwritten, a ledger prevents re-copying). They reach the repo only through `/anthology:setup`, note by note after a diff, and only if the note is not `type: user` and the privacy scan finds nothing.
- `type: user` notes found in `.claude/memory/` or `.claude/agent-memory/` are moved to personal memory at every session start.
- Every write into repo memory that anthology makes (promotion, import, `anthology-mem save`) runs the privacy scan; a scanner failure is a clean refusal with nothing written.
- A symlinked or non-folder personal-memory folder is never written, swept into or read through. The session says so in one line (fail closed, visibly), instead of silently leaving `user` notes in the repo.
- The `auto-memory-dir` smoke now asserts: session 2 puts the amber note in personal memory (`<config>/anthology/personal-memory/`) and not under `.claude/memory/`; session 3 puts `cyan-circle` in the repo.

**Consequences**

A first session after setup leaves old notes out of the repo and the main session's repo index until the user promotes them, which is the intended privacy default. Cost if wrong: a user who wanted automatic repo migration runs setup once and picks notes. Evidence: stream P1 tests, `tests/test_smoke.py`.

## ADR-021: quoted secret keys in the privacy scan and G6

- **Date:** 2026-11-20
- **Task:** fix-wave lane FW-Q
- **Status:** Accepted

**Context**

The secret-assignment patterns (`password`, `api key`, and the broader `password|passwd|secret|api key|token` one in `memory_privacy.SECRET_PATTERNS`, whose first two are mirrored by guard rule G6) required the separator right after the key. A JSON- or dict-quoted key (`"password": "…"`, `'api_token': '…'`, `{"token":"…"}`) has a closing quote in between and was missed.

**Decision**

- Each of those patterns allows one closing quote (`"` or `'`, with an optional backslash before it, so JSON inside a double-quoted shell string such as `{\"password\": \"x\"}` counts) between the key and the `:`/`=` separator. The G6 patterns and their copies in `SECRET_PATTERNS` stay byte-identical (a test checks it).
- After a quoted key the value must be a scalar. It does not count if it is empty (`""`), an object or array (starts with `{` or `[`), or an immediate `,`/`}`/`]`. So JSON schemas (`"password": {"type": "string"}`), blank fields and prose such as "the password field" stay clean. The broad pattern also keeps its 8-character minimum.
- A value is skipped, after a quoted or an unquoted key, only when it is wholly `null`/`none`/`true`/`false` or a template placeholder: all of it `<…>`, all of it `${…}`, or a run of only `*`, `.` or `x`/`X`, optionally quoted, then optional blanks and one of `,`, `}`, `]`, a line break or the end of the text. A quoted one must have its closing quote right after the word, so a `,`, `}` or `]` inside the quotes does not end it. `"true-love-99"`, `"true,horse,battery"`, `"x,Hunter2xyz"`, `"<a>b"`, `"${X}tail"` and `"***real"` still flag. `"string"` is not a placeholder (a schema is `{"type": "string"}`, already clean). This changes the unquoted forms too: `password=<your-password>` is clean, and so are `password: x` and `password=xxxx`.
- Every pattern stays linear (one optional quote, no nested quantifiers); a test asserts that 4x the input costs less than 6x.

**Consequences**

A quoted key with a real-looking value is refused or blocked like an unquoted one. Residual misses, accepted: an object-valued secret (`"password": {"value": "…"}`, ARM parameter files), an array (`"password": ["…"]`), a TOML triple-quoted value after a quoted key (`"password" = """…"""`), a value with a leading space inside the quotes, a real secret that is only `x`, `*` or `.` characters, an unquoted value that starts with a skip word and a `,` (`password=true,horse`, since `,` is a normal delimiter there), Unicode quotes or colons (`“password”: …`) and doubly escaped quotes (as on build). Inside a G6 Bash command the placeholder skip rarely applies (`echo 'password: <x>' > mem` still blocks, because a quote and a redirect follow the placeholder); that is the safe side. Cost if wrong: a false negative on one of those, or a false positive on a quoted example value that looks real (`"password": "string"`). Evidence: `QuotedKeyTest` in `tests/test_memory_privacy.py` and the G6 rows in `tests/test_guard_rules.py`.

## ADR-022: rtk's hook enforces deny, ask and Read rules behind `Bash(rtk *)`; rtk history lives in Anthology's folder

- **Date:** 2026-11-20
- **Task:** fix lane F1 (final review C1 and I4)
- **Status:** Accepted

**Context**

Setup adds the owner's blanket `Bash(rtk *)` allow (ADR-017, PLAN §12), and Claude Code judges the command a `PreToolUse` hook returns in `updatedInput`, not the one the agent typed. Probed on Claude Code 2.1.311 (`claude -p`, `.superpowers/sdd/PLAN/probes/F1/`): a hook `"ask"` prompts even when an allow rule matches the rewritten text; a hook `"deny"` blocks; a hook `"allow"` does not override a deny rule that matches the rewritten text; with no decision the rewritten text is judged by the rules; and Claude Code itself denies `cat secret.txt` under `Read(./secret.txt)`. rtk 1.2.1-anthology.1 emitted nothing on a deny and no decision on an ask, read no `Read(…)` rules and no managed settings, and judged `rtk …` forms by their own text. So `rtk git push`, `rtk bypass …`, `kubectl apply` (an ask rule) and `cat secret.txt` all ran without a prompt, and the setup note said the deny rules still applied. Separately, rtk recorded every Bash command line in its default data folder for 90 days, outside anything remove deletes.

**Decision**

- **Fork change in `rtk/src/hooks/anthology_gate.rs`**, wired into `decide_for_claude_hook_at` (so `rtk hook claude` and `rtk hook check --agent claude` agree); `rtk rewrite` is unchanged.
  - A Bash deny rule that matches the command, the inner command of any `rtk` form (`bypass`/`proxy`, `run` with `-c` or argv, `err`, `test`, `summary`, `rtk <tool> …`, global flags), a shell wrapper's script, a `find -exec`, `xargs` or `env` payload, recursively, prints an explicit `permissionDecision: "deny"` with no `updatedInput`.
  - A matching ask rule prints `"ask"` (never "no decision"). Behind an `rtk` prefix, when any deny or ask rule exists, these also ask: constructs the gate cannot read (substitution, an expansion before words a rule could still match, `eval`, a stdin-fed shell, interpreters, a non-POSIX `--shell`, git `-c alias.*`/program config, the depth cap, the work budget), and a ruled command reached only from a later word position (`caffeinate git push`, `ssh h git push`) or named anywhere in the segment. At top level the same cases leave the command unrewritten, so Claude Code judges the original. The hook also gates its own rewritten text, so a plain `pnpm exec …` or `npx -c …` cannot become a prompt-free `rtk …`.
  - These backstops are deliberately broad: a shell-command deny list cannot be completed by enumerating spellings. With only a `git push` deny, about 6 of 124 common agent commands probed changed in round 4 (a commit message or grep pattern that mentions `git push`, `rtk bypass python3 …`); round 5 made it 22, round 7 53, round 8 and 9 54 (below).
  - **Allowlist (round 5, replaced in round 7).** Four review rounds each found new spellings that ran a ruled command (stdin scripts for `ed`/`awk`/`make`, `git grep -O`, `git rebase -x`, `env -S` escapes, `/usr/lib/git-core/git-push`, `hash -p`, then a redirect between words, a comment or a here-document putting the lexer out of step with the shell). So, whenever any deny or ask rule exists (Bash or `Read`), an rtk-prefixed command passes without a prompt only when its program is in one table (`LISTED`, documented at the top of `anthology_gate.rs`) and its arguments pass that program's rule: no option that runs a string or a program the command line names, no word that may expand into one, no stdin script, `env -S`, `hash`/`alias`/function definition, assignment, substitution, comment or here-document. Git: a fixed set of global options (no `-c`, `--config-env`, `--exec-path`) and subcommands (no alias, `config`, `bisect`, `difftool`, `filter-branch`, `submodule foreach`...), without `grep -O`, `rebase -x/--exec`, `--upload-pack`, `--receive-pack` and the like; `git-<sub>` is `git <sub>`, for the deny rules too. Everything else asks; a rewrite into such a command is dropped (the host judges the original). An allowlist fails closed where a denylist fails open. Cost: with only a `git push` deny, 22 of the 124 common commands prompt (round 4: 9) and 3 lose their rewrite; the common forms (`rtk git status`, `rtk ls`, `rtk cargo test`, `rtk grep`, `rtk read`, `rtk npm run`, `rtk npx prettier`) do not. The rule scan also reads each command with its redirects left out (`git >/dev/null push` is `git push`) and checks a value glued to a short option (`grep -fsecret.txt`) or after `@`. The Read check was extended to Read-only users too: a Read rule alone turns the allowlist on.
  - **Scope (round 6 ruling).** The gate aims at parity-plus with Claude Code's own rule matching. A deny or ask rule is enforced against the commands the agent types (every rtk form, wrapper, compound, substitution, redirect); a Read rule against every file the line names or redirects. What an allowed program does with files, plugins, configs or build steps it loads (`build.rs`, test files, MSBuild, the linker, plugins, kubeconfig `exec`, compose files) is out of scope: deny rules restrict commands, not what programs do. The round-5 wording that no option of an "any" program runs a string, and that no listed program reads a script on stdin, overclaimed and is withdrawn.
  - **Round 6 (its allowlist parts replaced in round 7).** A literal backstop: wherever the text runs `rtk`, any Bash deny or ask rule's command text found anywhere in the raw line (quotes and backslashes removed, whitespace collapsed, case folded where Claude Code folds it) asks. Any word naming `/dev/stdin`, `/dev/fd/N`, `/proc/self/fd/N` or `/proc/<pid>/fd/N`, a `-` that may mean stdin, and any redirect opening a descriptor above 2 (`3<<<`, `{fd}<`) ask for every listed program, `Any` ones included. Never-lists grew: `git fetch/pull/clone/ls-remote --exec`; `dotnet -p:`/`/p:`/`-property:`, loggers, targets, response files and the `--` MSBuild passthrough; `go -ldflags -gcflags -asmflags -gccgoflags -overlay`; `prettier --plugin --plugin-search-dir`; `rspec`/`rubocop -r --require`; `pytest --pdb --trace --pdbcls`; `mypy --python-executable`; `pyright --pythonpath`; `find -fprint* -fls`; npm and pnpm shell/editor/browser/`--config.*` options. Fetched packages ask: `pip`/`uv pip install` of a `git+`/`hg+`/`svn+`/`bzr+` or URL spec or with an index or find-links option, `npx -p`, `uv run --with`, `uvx --from`. A command word with `/` passes only from a system directory. File loads named on the line (other than `/dev/fd`) remain the file-exec residual. Prompt cost is unchanged at 22 of 124.
  - **Plain shape (round 7, structural ruling).** Five rounds each found a spelling, descriptor or option the allowlist missed (round 6: a redirect before the command or after a `cd`, `cd /dev` then `stdin`, `/dev/stdout` with `1<<<`, `/DEV/stdin`, dotnet's `--property:` separator, `/dev/{x,stdin}`, `//proc/…`, `file -Lfsecret.txt`). So the gate stops modelling shell semantics for what may pass. With any deny, ask or Read rule, a line that runs or mentions `rtk` passes without a prompt only in the plain shape: exactly one simple command (outside quotes only letters, digits, spaces and `-_./:,=+%@`; `$`, a backquote, a backslash and `!` refused even inside quotes; so no compound, group, redirect, here-string, substitution, expansion, glob, brace, comment, assignment or `cd` in front; `~` only as a leading `~/` or a git `rev~N`/`rev^`), the command `rtk <sub>` with `<sub>` in a short list (`ls`, `tree`, `read`, `wc`, `diff`, `grep`, `rg`, `find`; `git status/log/diff/show/branch/blame/add/commit/checkout/switch/rev-parse/ls-files`; `cargo build/test/check/clippy/fmt`), every option from that sub's list, and every non-pattern operand or value a literal path resolved against the hook's cwd (lexically and through symlinks), asked about under `/dev`, `/proc`, `/sys` or as `-`, and checked against the Read rules (deny is deny, ask is ask). Everything else asks; a deny from the rule scan stays a deny. rtk rewrites a command only into that shape, otherwise the original goes back unrewritten. The per-program allowlist (`LISTED`, its never-lists, the descriptor and fd screens) is deleted. The literal backstop stays. Git `add`/`commit`/`checkout`/`switch` are on the list because they run nothing the line names; cargo is, as the ruling names it; `pytest`, `ruff`, `tsc` and `mypy` are not (plugins and config by flag). A git operand has no `:`; for `x~1`/`x^1`, which zsh's `extendedglob` reads as patterns for names starting with `x`, every such name is checked. Input redirects are now found over the whole text, and after any `cd` a relative operand is uncertain (ask). Cost: with only a `git push` deny, 53 of the 124 common commands prompt (round 6: 22) and 26 lose their rewrite.
  - **Round 8.** Two Read gaps in the option model closed by construction. Git options with an optional value (`--pretty`, `--format`, `-U`, `--unified`, `-u`, `--untracked-files`; `--stat`, `--color`, `--decorate`, `--word-diff`, `--abbrev-ref`, `--short` were already flags) take it only glued, as git does, and no word after a git subcommand may hold a `:` except a `commit -m` message: `git show --pretty HEAD:secret.txt` asked nothing before and asks now. Any word after cargo's `--` asks, and any word opening with `@` asks anywhere (rustc and rustdoc read `@FILE` relative to the workspace root, not the hook's cwd). Hardening: `git add -f`/`--force` is off the list; an option that takes the next word asks when that word starts with `-`; the `x~1` stem scan asks past 10 000 entries. Needless asks removed: `$`, a backquote, a backslash and `!` inside single quotes; `A~N..B` and `A^..B` ranges (stem-checked like `x~1`); `cargo build -r`; `git status -uno`; `rtk ls ~` (from `$HOME`, ask when unset). Cost: 54 of 124 (round 7: 53), 25 rewrites dropped. Accepted residuals, documented: a glob value selecting a denied file in a recursive search (`rg -g`, `grep --include`), a blob id from `git ls-files -s` shown with `git show`, and what an argument file a program loads on its own holds.
  - **Round 9.** Under zsh `EXTENDED_GLOB`, `^`, `#` and a non-leading `~` are glob operators, and round 8's ranges let `c^1./.env` (matching `config/.env`) and `~/project/config/.e^1` through. By construction: with rules present an unquoted word holding one of them asks, except a `commit -m` message and a git revision of the strict shape `REF(~N|^N|~|^)*` (or two joined by `..`/`...`), `REF` being `[A-Za-z0-9_-]+` with no `/` or `.`; only those get the x* name check. A leading `~` is the home only as `~` or `~/…` with no later `^`, `~` or `#`. The commit message is no path and leaves the Read check (gate and rule scan); single-quoted text is literal to the Read check too, except in git, whose pathspecs are globs: a git word holding `*`, `?`, `[` or `\` asks even quoted. Cost: 54 of 124 ask (round 8: 54), 26 rewrites dropped (round 8: 25; `git diff -- "*.rs"` now keeps its original text).
  - `Read(…)` deny rules block the commands rtk rewrites into reads and `rtk read`-family subcommands on a denied path; an unresolvable path (a variable, a glob, an input-redirect operand that expands, `/proc`, `/dev/fd`) asks, at top level too since round 6. `Read(…)` ask rules prompt the same way.
  - Rule sources: project and user `settings{,.local}.json`, `CLAUDE_PROJECT_DIR`, and `managed-settings.json` + `managed-settings.d/*.json` (honoring `allowManagedPermissionRulesOnly`). An unreadable file fails closed (ask). Oversize or unparseable hook input prints `"ask"`.
  - With no deny, ask or Read rule anywhere the hook output is byte-identical to before: the owner's accepted risk (`Bash(rtk *)`, rewritten commands never prompt) is unchanged.
- **Not covered, documented:** `--settings`, `--disallowedTools`, server-managed settings and the macOS MDM profile (no hook can read them); files a recursive search walks into; files, links and settings the same command creates (`ln -s . d && cat d/secret.txt`); what a listed program runs from files (tests, build and `npm run` scripts, git hooks and config, plugins and config files named on the line, registry packages), out of scope by the ruling above, and the session's existing environment; git object reads (`git show REV:path`, `git cat-file -p`, `git log -p`); commands hidden from any parser (also uncovered without rtk).
- **rtk history.** `bin/rtk` and `bin/rtk-hook` export `RTK_DB_PATH`, `RTK_RECALL_DB` and `RTK_TEE_DIR` under `state.rtk_data_dir()` = `${CLAUDE_CONFIG_DIR:-~/.claude}/anthology/rtk/` (state.json's folder), and `/anthology:setup remove` deletes that folder behind the usual preview. Stats stay on by default; rtk has no switch that drops command lines but keeps `rtk gain`. rtk's default data folder is not deleted, because another rtk install may share it; README and SECURITY name it for `1.2.1-anthology.1` users.
- **Release:** `1.2.1-anthology.3` (tag `rtk--v1.2.1-anthology.3`), then `1.2.1-anthology.4` (tag `rtk--v1.2.1-anthology.4`, rounds 5-9: the plain shape), each built and attested by `rtk-release.yml` as in ADR-017; `rtk.lock` rewritten from its `checksums.txt`. The tag `rtk--v1.2.1-anthology.2` was pushed first, but its release run failed on a test-only timing bound before anything was published, so it has no release; the version moved on rather than re-pointing a pushed tag.

**Consequences**

Users with deny, ask or Read rules get them enforced behind rtk, at the cost of an occasional prompt where a wrapped command cannot be read, and an unreadable settings file now prompts for every command. A later upstream sync must keep the three `// anthology fork:` call sites wired to the gate. Cost if wrong: a rule source Claude Code adds later is invisible to the hook until the gate reads it.

## ADR-023: rtk's gate also reads every line with the shared shell parser (`anthology-shell`)

- **Date:** 2026-11-21
- **Task:** Rust port lane RG1 (design §7.2, plan RG1)
- **Status:** Accepted; ships with the next rtk release, which the release owner cuts

**Context**

The guard and rtk's Anthology gate (ADR-022) each had their own shell reading. Disagreement between two parsers was the main source of guard bypass findings (design G-2), and the gate's recursion (rtk forms, `sh -c`, `find -exec`, `xargs`, `env`, launchers, depth 4) was a third partial model. `anthology-shell` (lane SH) ports the guard's lexer and expansion (`analyze`): heredocs and here-strings fed to a shell, `echo … | sh`, `eval`, substitutions, wrappers, package runners, rtk runner and passthrough forms, docker shells, `cd` tracking, nested to depth 12, under one byte-weighted work budget that marks the result `incomplete`, and `opaque` reasons for what it cannot see into. The acceptance is that verdicts only get stricter: over the gate's tests plus the guard corpus, deny > ask > defer/allow, new ≥ old.

**Decision**

- `rtk/Cargo.toml` gains one dependency under `# anthology fork: shared shell parser`: `anthology-shell = { path = "../crates/anthology-shell" }`. rtk keeps its own `[workspace]` and `Cargo.lock`; the lock gains the path package, `fancy-regex` 0.20.2 and `bit-set`/`bit-vec` 0.8, the versions the root lock pins. `rtk-test.sh`, `rtk-net-test.sh` and `rtk-trim.sh` mount the repository root, since the path dependency lives outside `rtk/`.
- With any deny, ask or `Read()` rule, `anthology_gate::evaluate` runs `anthology_shell::analyze` on the line (and on rtk's own rewrite of it) after the existing rule scan, and each entry (program and arguments, as the guard reads them) meets the rules. This pass can only add a deny or an ask:
  - a Bash deny rule matching an entry denies, an ask rule asks (`bash <<EOF` / `git push`, `eval 'git push'`, `rtk bypass caffeinate git push`, `rtk git $'push'` are now denied);
  - a `Read()`-denied path denies when a command rtk rewrites into a read, or any program rtk runs (`rtk bypass base64 secret.txt`), names it or reads it through `<`; an unresolvable path asks for those, and so does a `Read()` ask path behind rtk;
  - `incomplete` (a regex backtrack limit or the work budget) asks.
- The plain shape (ADR-022, rounds 7-9) also requires the shared parser to read the line as the same one command: one entry, nothing `opaque`, no redirect, the same words. So an opaque entry behind rtk always asks (a line whose text holds `rtk` passes only in the plain shape, and rtk rewrites only into it), and two lexers that disagree on a line that would pass is a reason to ask. The "holds `rtk`" test now drops a backslash-newline first, as the shell does (`r\` + newline + `tk` is `rtk`).
- The pass adds no defer and no ask for what only the rule scan's own reading needs (an expansion a rule's head can extend, a relative path after a `cd`): the rule scan keeps deciding those, and a hook defer would stop the decision before rtk's own rewrite is judged, which is looser (found by the old/new corpus run).
- **The rule scan stays.** Replacing its recursion outright loses denies the ≥ acceptance forbids: shells the guard does not model as shells (`mksh`, `tcsh`, `csh`, `ash -c`), a deny reached through a quoted script after a launcher, and word-suffix readings of launchers the guard does not peel. Running both and keeping the stricter answer is the only shape that meets the acceptance by construction; disagreement between the two readers can only ask or deny. Cost if wrong: two readers stay in the gate until a later change deletes the rule scan's recursion, once a corpus shows the shared parser covers it.
- `rtk/tests/anthology_shell_rtk_forms_test.rs` pins rtk's clap subcommand list and each subcommand's form (passthrough, runner, fixed tool) against `anthology_shell::RTK_PASSTHROUGH` and `RTK_RUNNERS`, so an upstream subcommand that runs a command cannot arrive unclassified. It is not a `fork_*` test.
- With no deny, ask or `Read()` rule, the hook output is byte-identical to `1.2.1-anthology.4` (the gate is inert, and `analyze` is not called).

**Consequences**

More lines are denied where they used to be asked about, and some lines behind `rtk` that the plain shape never passed now ask for a second reason; no line that used to be asked about or denied passes. Measured on 3,814 commands (the gate's tests, the F1 review kits, the guard's corpus and the shell seeds) under five rule sets: no verdict got looser, and with no rules every output is byte-identical. An upstream sync now carries one more `# anthology fork:` line in `rtk/Cargo.toml`; `rtk_trim`'s lock guard allows path packages under `../crates/` and their dependency closure (DV4). Cost if wrong: an `anthology-shell` bug that over-reads a command adds a prompt or a deny; one that under-reads is still covered by the rule scan.

**Addendum (RG2, 2026-11-21): the gate's work bound on huge lines**

- A 1 MB `rtk ls a a a ...` took about 41 s with any rule: the plain shape and the `Read()` checks made several `canonicalize` calls per path word, and the work budget did not charge them. A `PreToolUse` hook that outlives its timeout is no decision (Claude Code continues through the normal permission flow unless `onFailure` is `block`), so `Bash(rtk *)` would allow the line.
- Disk lookups now have their own budget (`PROBE_COST` per path, working directory and matcher). When it runs out, symlinks are no longer resolved for the rest of the command, every path is still matched as written, the text is still read in full within the scan's own budget, and the command asks at the end. **Unverified paths ask:** past the lookup budget, a `Read()` deny that only a symlink lookup would find is an ask, not a deny; a path denied as written (lexical `..` included) still denies, and so does a Bash deny rule later in the line.
- A line whose bytes alone would spend the scan's budget (`LINE_BYTE_COST`, 125 KB) never passes: the budgeted scan still runs and its deny stands, otherwise the hook asks on the line as written, with no plain-shape check and no rewrite. With no rules the gate stays inert.
- Consequences: a 1 MB line answers in well under a second (release build, 0.05-0.42 s measured); lines with about a thousand or more literal paths, and any line over 125 KB, now prompt when any rule exists. RG1's old/new corpus is byte-identical.

## ADR-024: safety hooks fail closed on a timeout or crash (`onFailure: "block"`; supersedes part of ADR-010)

- **Date:** 2026-11-21
- **Task:** Rust port lane HK1 (from the RG2 finding)
- **Status:** Accepted

**Context**

- Neither safety `PreToolUse` hook set `timeout` or `onFailure`: the guardrails guard (matcher `*`) and rtk's hook (matcher `Bash`, which runs the ADR-022/023 gate behind `Bash(rtk *)`).
- The Claude Code hooks reference says a command hook's `timeout` defaults to 600 s, and "On `PreToolUse`, a timed-out `command`, `http`, or `mcp_tool` hook doesn't block the tool call, except for hooks whose failure blocks the action. The call continues through the normal permission flow". A hook that can't start, or exits with anything but 0 or 2, is also a non-blocking error. So a stalled, killed or crashed rtk hook let an `rtk …` line through on `Bash(rtk *)` alone. The same applied to the guard.
- `"onFailure": "block"` makes each of those failures (can't start, exit other than 0 or 2, timeout, invalid JSON output) do what exit 2 does: on `PreToolUse`, it blocks the call. It needs v2.1.295 for command hooks (ADR-010, ADR-012); the plugins already declare `>=2.1.311`. The smoke `guard-timeout` checks it on a real client.
- ADR-010 kept the guard fail-open, because a missing `python3` would then block every tool. The owner's later fail-mode answer for the Rust port is fail closed, and the staged guardrails `hooks.json` (design §9.1) already has `"timeout": 60, "onFailure": "block"`.

**Decision**

- Every safety `PreToolUse` hook sets `"timeout": 60` and `"onFailure": "block"`. That is the live guard (`python3 scripts/guard.py`), the live rtk hook (`sh bin/rtk-hook`), and the staged rtk hook in `port/staging/plugins/anthology/hooks/hooks.json`, so the cutover keeps it. The staged guard already had both.
- Ruling: one timeout, 60 s, the design §9.1 value for both hooks. Why: the guard decides in about 10 ms and rtk's gate is bounded (RG2: a 1 MB line in under 0.5 s; the history write after the answer waits at most 5 s), so 60 s only bounds a stall. With `block`, a shorter timeout can only add blocks, never allow a call. Cost if wrong: a pathological stall holds a tool call for up to 60 s before it is blocked.
- Ruling: this supersedes ADR-010's "the guard keeps failing open" for the Python build. Why: a stalled or killed guard must not turn into an allow, and the release ships the Rust shim with the same setting anyway. Cost if wrong: until the cutover, a machine without a working `python3` gets every tool call blocked, with a `failed; blocking because onFailure is "block"` error, until `python3` is installed or the guardrails plugin is disabled (`/plugin`).
- Unchanged: both hooks still exit 0 on malformed input. `guard.py` catches every exception and allows, and rtk's hook answers `ask` on unreadable input and exits 0 when rtk is not installed. So `block` only fires when the process itself fails or stalls.

**Consequences**

A timeout or crash of either safety hook now blocks the call, and the error names the hook. rtk exiting 1 or panicking blocks that Bash call, instead of letting it fall through to `Bash(rtk *)`. No minimum-version change.

## ADR-025: setup offers to remove `CLAUDE_CODE_SUBAGENT_MODEL`

- **Date:** 2026-11-21
- **Task:** owner request (setup handles `CLAUDE_CODE_SUBAGENT_MODEL`)
- **Status:** Accepted (replaces "Anthology never changes it" from the section 14 compatibility checks)

**Context**

`CLAUDE_CODE_SUBAGENT_MODEL` overrides every agent's model, so the roster's opus/sonnet/haiku tiers do not apply. Doctor and session start only warned, and told users to remove it themselves.

**Decision**

- `compat.subagent_model_sources` lists every place it is set (user settings, the project's two settings files, managed settings, the process environment), never the value. The process environment counts only when its value matches none of those files, because Claude Code copies a settings file's `env` into the commands it runs.
- When it is in the user `settings.json` env, the preview offers its removal as a separate opt-in change with its own redacted diff. `apply --remove-subagent-model`, passed only on a yes, removes it under the same digest check as setup's other settings changes.
- The removal is a new settings op, `{"op": "unset", "path": ["env", "CLAUDE_CODE_SUBAGENT_MODEL"]}`. It is stored with the other ops, with the previous raw value. `remove` puts the value back as the env block's last key, unless the key is present again. That follows the `worktree.baseRef` convention: a value the user set since stays. The op is not part of `setup_ops`, so the ops version does not change.
- Anywhere else, setup changes nothing and gives a one-line hint for that place. A key listed twice in the user `env` block is not offered: dropping one copy would leave the other in force, and undo could not restore both, so the hint asks the user to delete both lines.

**Consequences**

When the user settings.json is the only place, the tiers apply after one yes and a restart. A value set elsewhere, or a shell value hidden behind a settings value, still applies until it is removed there; session start keeps warning and points at `/anthology:setup`. Cost if wrong: a user who relied on the override loses it until `setup remove` or a manual edit. The previous value sits in `state.json` next to the other undo data.

## ADR-026: working rules live in the plugin (karpathy-guidelines, large-builds)

- **Date:** 2026-11-21
- **Task:** owner request (move the owner's local working rules into Anthology)
- **Status:** Accepted

**Context**

Every working rule belongs in Anthology, never in a user's global Claude config. Four rules lived outside it: follow the Karpathy coding guidelines, read a large-build rulebook before a long multi-agent build, write plain feature-based status reports, and record rulings when the owner is away.

**Decision**

- `karpathy-guidelines` is vendored like the other knowledge: `multica-ai/andrej-karpathy-skills`, `skills/karpathy-guidelines/` at `main` = `2c606141936f1eeef17fa3043a72095b4765b9c2`, by forrestchang and contributors, MIT. The repository ships no LICENSE file; the license is declared in its README, SKILL.md frontmatter and plugin.json, so `vendor/licenses/` carries the standard MIT text with that note and the folder ships it as `LICENSE`. One patch adds a single line: in an autonomous run, record the assumption and carry on.
- `implementer`, `debugger` and `reviewer` read it at startup (knowledge belongs to agents). Their startup budgets were raised to ceil(measured x 1.15).
- `large-builds` is first-party, so it is a model-invoked skill in the `anthology` plugin (`anthology:large-builds`), next to the three user commands. Its "Every session" section carries the coding rule (karpathy-guidelines) and marks the rulings and status-report sections as rules for every session.
- The routing guide gets one pointer line: coding, long builds, status reports and rulings load `anthology:large-builds`. To fit its 3,800-character ceiling, three existing lines were tightened without changing what they say; the ceiling and the 9,000-character session-start budget are unchanged.

**Consequences**

Ruling: the detail lives in `anthology:large-builds`, with one pointer in the guide — the guide was 5 characters under its ceiling — cost if wrong: the main session reads an 8 KB skill before coding or reporting. Ruling: karpathy-guidelines is knowledge, not a main-session skill — `vendor.py` writes only to knowledge folders, and the coding agents are where code is written — cost if wrong: the main session reaches the full text only through the skill's pointer when it codes inline.

## ADR-027: the Rust port (one language; the difftest frozen into goldens; the test ledger)

- **Date:** 2026-11-21
- **Task:** Rust port, cutover CO2 (design `docs/superpowers/specs/2026-11-20-rust-port-design.md` §1 and §4.5, plan CO2)
- **Status:** Accepted

**Context**

Anthology was two languages. About 12k lines of first-party Python (the 14 plugin modules in `plugins/anthology/scripts/`, the guard in `plugins/anthology-guardrails/scripts/`, 8 maintainer tools in `scripts/`, the smoke runner `evals/smoke/smoke.py`) and about 19k lines of Python tests ran next to rtk's Rust. Users needed a working `python3` (3.9 or newer), the guard could not run without it (ADR-010), and the guard and rtk's gate each had their own shell parser. The owner decided on 2026-11-20 to maintain Anthology in Rust only, with no Python at runtime.

**Decision**

- All first-party Python is ported to the cargo workspace in `crates/`. `anthology-cli` builds the one shipped program, `anthology`, from `anthology-core`, `anthology-guard`, `anthology-shell`, `anthology-memory`, `anthology-runtime`, `anthology-settings` and `anthology-pycompat`. `anthology-dev` (the maintainer tools), `difftest`, `anthology-testkit` and `repo-tests` never ship. The vendored superpowers files are untouched.
- Behaviour parity: every hook, command, file write, exit code and message matches the Python at the baseline, except the forced changes the plan lists (GC-15, F-1 to F-20): no Python, fail closed (ADR-028), and the messages and commands that named Python. `anthology-pycompat` re-implements what the outputs depend on (`json.loads` and `json.dumps`, `\s` and `\w`, `str.lower`, strerror texts), so parity is byte for byte.
- Proof during the port: a live differential test ran every ported surface against the Python as the oracle, on seeds and generated cases. At cutover the oracle's answers were frozen into gzip golden files (`crates/<crate>/tests/golden/`), which `cargo test` replays. A golden changes only through `anthology-dev golden bless`, so every behaviour change is a visible diff. The oracle (`port/difftest/oracle`) and the extraction scripts were then deleted; `crates/difftest` stays as the replayer.
- Proof that nothing was dropped: every Python test id (`port/python-test-ids.txt`) has a row in `port/ledger/<module>.tsv` naming the Rust test that ports it or a reviewed drop reason. `anthology-dev ledger check` (the `ledger` job) fails on an id without a row and on a row whose Rust test cargo does not run.
- The glue around docker, git and `claude` stays shell, with no Python calls: `release.sh`, `run-evals.sh`, `sync-rtk.sh`, `rtk-trim.sh`, `rtk-test.sh`, `rtk-net-test.sh`. Maintainer tools run as `scripts/dev.sh <command>` in the pinned Rust image, so contributors need no host Rust.
- CO2 deleted `plugins/anthology/scripts/`, `plugins/anthology-guardrails/scripts/`, `scripts/*.py`, `evals/smoke/smoke.py`, `tests/`, the Python CI images and the Python CI jobs (design §14.2).

**Consequences**

Users no longer need Python; the runtime is one static binary per platform, downloaded and pinned (ADR-029). No CI job uses Python. A deliberate behaviour change now needs a blessed golden and, for a removed test, a ledger row with its reason. Cost if wrong: the goldens cover only the frozen cases, so a behaviour no seed or generator reached can drift without a diff; the fuzz targets and the property tests cover the parsers.

## ADR-028: the guard fails closed when its binary is missing or broken (supersedes ADR-010's fail-open guard)

- **Date:** 2026-11-21
- **Task:** Rust port, cutover CO2 (design §1.2 decision 4, §9; plan GC-14)
- **Status:** Accepted

**Context**

- ADR-010 kept the guard fail-open, because a missing `python3` would otherwise block every tool, and added a POSIX `sh` sentinel hook that said loudly when `python3` was missing.
- ADR-024 already superseded part of that: a guard hook that crashes, stalls or cannot start blocks the call (`"onFailure": "block"`, 60 s timeout).
- After the port the guard is a downloaded binary. A missing binary must not mean no guardrails, and the owner's binding decision for the port is fail closed.

**Decision**

- `hooks/guard.sh` (POSIX `sh`) has exactly two allow paths: the binary's positive allow token (exit 0 with exactly `anthology-guard:allow:v1` on stderr), and, when the binary cannot run, a read-only allowlist (`Read`, `Grep`, `Glob`, `LS`, `NotebookRead`, `WebFetch`, `WebSearch`, `TodoWrite`, the `Task*` tools, `Task`, `Agent`, `Skill`, `ToolSearch`, `AskUserQuestion`, the plan-mode tools, `BashOutput`, `KillShell`). Every other path blocks with exit 2: shell commands, file writes and edits, `mcp__*` and any tool not on the list.
- An installed binary that answers with anything but the token or a well-formed block is quarantined and reinstalled on a later call, at most once a minute. Three failures in 24 hours stop the reinstalls from tool calls; a new session tries once an hour.
- Inside the binary, a panic, a regex limit or the 8 s watchdog blocks with an `(internal)` message. Malformed input still exits 0, as before.
- Every `(setup)` and `(internal)` message names the fix and the escape that always works: disabling `anthology-guardrails` with `/plugin`, which never passes through `PreToolUse`.
- On a platform with no guard binary (Windows included), read-only tools are allowed and everything else is blocked with a message naming `/plugin`.
- This supersedes the rest of ADR-010's guard decision: the fail-open choice and the `python3` sentinel hook. ADR-024's `"onFailure": "block"` and 60 s timeout stay as the backstop under the shim. ADR-010's hook-output budget (9,000 characters) still stands.

**Consequences**

An offline first session can read but not run, write or call MCP tools until the binary installs, and it heals by itself once the network is back. While the binary is missing, G3 is not enforced for the read-only tools that are let through; the native `Read(…)` deny rules still apply (SECURITY.md). A guard stopped by SIGSTOP reaches the hook timeout and gets Claude Code's generic `onFailure` message, which names neither the cause nor `/plugin`. Cost if wrong: a harmless tool that is not on the allowlist is blocked until the binary installs. Evidence: the shim tests (`crates/repo-tests/tests/shims/`) and the `binary-missing` smoke.

## ADR-029: the `anthology` binary: release, pin and install

- **Date:** 2026-11-21
- **Task:** Rust port lanes RL1 and BS1, cutover CO1 and CO2 (design §8, §10)
- **Status:** Accepted

**Context**

The `anthology` binary has to reach users the way rtk does (ADR-017): built in this repo, verifiable, pinned in a reviewed commit, and installed without Python. Both plugins need it, and `anthology-guardrails` must work when it is installed alone.

**Decision**

- **Release.** `.github/workflows/anthology-release.yml` runs on a tag `anthology-bin--v<version>`, which must equal `[workspace.package].version`. On four native runners it runs `cargo deny check` and the tests of the shipped crates, builds `anthology-cli` with `--release --locked` (static musl on Linux, with no interpreter or shared-library dependency), checks `anthology --version`, packs `anthology-<target>.tar.gz` (the binary and `LICENSE`), attests the archives with build provenance and publishes them with `checksums.txt`. It fails if the release exists. A pull request touching the workflow, `rust-toolchain.toml` or `deny.toml` runs a dry run. No Apple codesigning, as for rtk.
- **Pin.** Each plugin ships `anthology.lock`, byte-identical in both: `version`, `tag`, `repo` and one sha256 per target archive, in rtk.lock's layout, one key per line so `sh` can read it. Only `anthology-dev lock write` writes it, from `checksums.txt`, and it refuses a hash that differs from GitHub's own digest of the asset. The `anthology-bin--v` prefix cannot collide with plugin tags or `rtk--v`.
- **Install.** `hooks/bootstrap.sh` and `hooks/lib.sh` (POSIX `sh`, identical in both plugins) install into `${CLAUDE_CONFIG_DIR:-~/.claude}/anthology/bin/<version>/`, shared by both plugins. A user-placed seed `anthology-<target>.tar.gz` in that folder is tried first, then `curl`, GNU or BusyBox `wget`, then `node`, within a 20 s budget. The archive's sha256 must match the lock before anything is unpacked, the binary must print exactly `anthology <version> (<target>)`, and the install marker is written last. Each session start re-checks the install against the lock.
- **Freshness.** `release.sh` runs `anthology-dev lock check-fresh` before releasing a plugin that ships `anthology.lock`. It compares the shipped set (the `src/` and `Cargo.toml` of every crate in `anthology-cli`'s build graph, their `Cargo.lock` entries, `rust-toolchain.toml`, the release profile and the workspace keys that graph uses) between the lock's tag commit and `HEAD`, and fails when it changed. Tests, `anthology-dev`, the difftest and the goldens are outside it.
- **Lifecycle.** A plugin update installs the new version beside the old one; session start deletes versions no lock pins after 30 days unused. `/anthology:setup remove` deletes the folder only when `anthology-guardrails` is no longer installed.

**Consequences**

A change to the shipped crates reaches users only after a new binary tag and a `lock write` commit, then a release of both plugins. CO2 changed nothing in the shipped set, so the binary the replay validated (ADR-031) is the one that ships. Cost if wrong: about 6 MB per version stays behind after an uninstall that skips `remove`, until the user deletes it.

## ADR-030: one shell parser for the guard and rtk's gate (`anthology-shell`)

- **Date:** 2026-11-21
- **Task:** Rust port lanes SH1, SH2, GD1 and RG1 (design §4.1, §7)
- **Status:** Accepted

**Context**

The Python guard read shell commands with its own lexer and expansion. rtk's Anthology gate (ADR-022) had a flat lexer plus its own partial recursion. Disagreement between the readers was the main source of guard bypass findings. rtk's lexer cannot see into substitutions, heredocs or `sh -c`, which the guard must open.

**Decision**

- `crates/anthology-shell` is a direct port of the guard's lexer and expansion (`analyze(command, home)`), proven against the Python by the difftest. It depends only on `regex` and `fancy-regex` (for the patterns with lookaround or backreferences), forbids unsafe code and has a self-contained `Cargo.toml`, because rtk's own workspace builds it too.
- `analyze` returns the entries that actually run (program, arguments, redirects, working folder, docker exemption), plus what it could not see into (`opaque`) and whether a regex or work limit cut it short (`incomplete`).
- The guard (`anthology-guard`) decides from the entries, ignores `opaque` to keep parity with the Python, and blocks on `incomplete`. rtk's gate asks on either, and depends on the crate through one `# anthology fork:` path dependency (ADR-023).

**Consequences**

A parser fix reaches both the guard and the gate. They ship separately (the guard in the `anthology` binary, the gate in rtk's releases), so between releases the two can run different versions of the parser. Cost if wrong: if an upstream rtk restructure makes the path dependency painful, fall back to a shared conformance corpus, with the guard keeping `anthology-shell`.

## ADR-031: replay of local transcripts before the cutover

- **Date:** 2026-11-21
- **Task:** Rust port lane RP1, cutover CO1 and CO2 (design §4.6, §13.7; architect's calls A18 and A29)
- **Status:** Accepted

**Context**

The difftest proves parity on seeds and generated inputs, but real tool calls carry encodings, subagent payloads and MCP inputs no generator writes. Revision 1 of the design planned a 7-day live shadow run, which costs calendar time, sees only new calls and compares verdicts only. The repo is public, so nothing from real transcripts may be committed.

**Decision**

- `anthology-dev replay collect|run|report` replays every `tool_use` in the owner's local transcripts (`~/.claude/projects/**/*.jsonl`, subagent transcripts included), deduplicated, as Claude Code's `PreToolUse` envelope with the raw input bytes. This replaces the shadow run (A18).
- Four runners per call: the Python guard (the authority), `anthology guard hook`, the shim with the binary seeded, and the shim with the binary missing. Runners 1 to 3 must agree byte for byte on exit code and stderr. It also compares `analyze` entries on every Bash command and the privacy findings on every text field. Runner 4 counts what fail closed would block, by tool, which checks the read-only allowlist (ADR-028).
- It runs at CO1 on the tagged binary, after every merge that touches the guard, the parser or the privacy scan, and at CO2. The gate is zero unexplained differences.
- The corpus and the full details stay local, under `~/.claude/anthology/replay/`. `.gitignore` and `no_replay_tracked.rs` keep them out of the repo. The committed report `port/replay-reports/<date>.md` holds only aggregate counts, tools bucketed as `builtin`, `mcp` or `other`, and case ids that are HMAC-SHA256 values under a key that never leaves the machine. A test checks the report for every corpus string before it is written (A29).

**Consequences**

The binary that ships was checked against every real tool call on record, deeper than verdicts, with no calendar wait. The replay proves parity on real traffic, not bypass resistance: the generators and the monotonicity properties cover that. Runner 1 now needs a checkout from before CO2 (`--repo`), since the Python is gone from `HEAD`. Cost if wrong: less diagnostic detail in the committed report; the full detail stays on the owner's machine.

## ADR-032: a read-only classifier on the shared parser, and the rtk gate's `cd` and `head` wrappers

- **Date:** 2026-11-22
- **Task:** post-release plan P3 (PLAN §15 item 1; the permission-levels design, item 1)
- **Status:** Accepted

**Context**

An allow is the one answer that skips the person, so a command may only be called read-only when that is proven. Two later pieces need the same proof: a permission mod that allows read-only commands the engine would ask about, and rtk's gate, which today asks about every line that is not one bare `rtk` command, so `cd src && git status` and `git log | head -n 5` are never rewritten once a deny, ask or `Read()` rule exists. A second shell reading in each place is how the guard's bypass findings arose (design G-2).

**Decision**

- `anthology-shell::readonly` holds `classify_readonly(command, home)`, which returns `Proven { paths, cd, runs_git_config }` or `NotProven(reason)`; each path is a `ReadPath { path, recursive }`. It proves, and never guesses. A line is proven only when the module's own small scanner and `analyze` read the same simple commands, words and redirects, with nothing opaque, no alternate reading and no `incomplete`; every program is in a fixed table and has only options that table lists. Stages join with `&&` or `|` only; the only redirects are `2>&1`, `>&2`, `>/dev/null` and `< PATH`; a `cd` may only lead, with a target `literal_cd_target` accepts. Anything unlisted is not proven: that includes every program, every option and every git subcommand not named.
- Ruling: a `cd` target must be absolute or start with `./`. A bare `cd src` is refused because `CDPATH` (set in a shell snapshot a hook cannot see) can send it elsewhere. Cost if wrong: `cd src && …` is not read-only-proven and not rewritten under rules; the normal prompt stands.
- Ruling: an unquoted `~`, `^`, `#`, brace, `$`, backquote, `<(`, backslash, `!` or `;` is refused anywhere, so `git diff HEAD~1` is not proven unless the revision is quoted. Commands run in the user's own shell, often zsh with `EXTENDED_GLOB`, where `~`, `^` and `#` are glob operators. Cost if wrong: fewer commands proven.
- Ruling: a glob that could expand to a name starting with `-` (`*.rs`, `*`) is refused outside `--`, and no glob stands in an option value, a pattern or any word that is not a path, so a file called `--output=x` cannot become an option. Cost if wrong: `ls *.rs` is not proven; `ls ./*.rs` is.
- Ruling: a `jq` filter that names `env`, `$ENV`, `import`, `include` or `modulemeta` is refused, since `jq -n env` prints every environment secret without writing anything. Cost if wrong: a field called `.env` is not proven.
- Ruling: `Proven.paths` are the paths as written; a git operand is reported as a possible path, since git decides between a revision and a path. The classifier never reads the file system: containment, symlinks, secrets, `Read()` rules and git's repository config are the path check's job (P4).
- Ruling (security review, round 1): the programs that run are the macOS BSD tools under zsh, with the shell snapshot's `bfs` in place of `find`, `ugrep` in place of `grep` and `rg`; the tables are written for those, not for GNU. A word that starts with `-` after the first operand (and before any `--`) is not proven for any program, and neither is a `--` after an operand: BSD `getopt` stops at the first operand and reads the word as a file, GNU permutes and reads an option, and for `uniq a -c` the file is written. `find` is exempt, since every word of its expression must be in the table. Cost if wrong: `git diff main -- src` and `grep foo -r src` are not proven; the prompt stands.
- Ruling: zsh reads `<1-5>`, `<-9>` and `<->` as one glob word, so a `<` that opens that shape is refused. Every word is checked for an opening `=` after its quotes are removed, since zsh expands it to a command path (`''=ls`), and an `=` after `=` or `:` is refused (`MAGIC_EQUAL_SUBST`). Cost if wrong: a quoted pattern that opens with `=` is not proven.
- Ruling: `Proven.runs_git_config` is true when any command is `git`. Git reads the whole repository and runs the programs that configuration names, whatever `paths` says. The path check (P4) must decide git with an ALLOWLIST, not a denylist: it allows the line only when every configuration file git can reach holds nothing but keys on a short list of safe repository-local ones, and asks on any other key, section or include. A denylist keeps missing vectors. Examples the first denylist missed: a partial clone (`remote.<name>.promisor`, `extensions.*`) makes plain `git log -p`, `show` and `diff` fetch missing blobs on demand, and the fetch runs `remote.<name>.uploadpack` and can reach `credential.helper`, `core.sshCommand`, `core.gitProxy` and `protocol.*`. The vectors known so far (`core.pager`, `pager.*`, `core.fsmonitor`, `core.hooksPath`, `core.askPass`, `core.editor`, `core.attributesFile`, `diff.external`, `diff.<driver>.command` and `.textconv`, `merge.*.driver`, `gpg.program` with `log.showSignature`, `filter.*`, `submodule.*`, `alias.*`, `include.*`) are listed in the doc comment of `ReadOnly::Proven` as examples of what the allowlist catches. P4 must also find the repository the way git does: from the working directory after the `cd` targets, up through the parents, a bare repository reached by a `cd` included, a `.git` file that points elsewhere, a worktree's own configuration, the system, global and `GIT_CONFIG_*` sources, and the submodules' repositories. A `git` word that holds `%G` (a signature placeholder, which runs gpg or ssh-keygen) is not proven. This change adds no code for any of it beyond the marker and the `%G` refusal. `describe --dirty` and `--broken` are not proven, since they refresh the index; `git status` without `--no-optional-locks` refreshes it too, so the claim is "writes no file a person could name", not "writes nothing". `file -p` is not proven (it sets the access time).
- Ruling: `ReadPath.recursive` is true when the command reads everything under the path (`rg`, `find`, `du`, `tree`, `grep -r`, `grep -d recurse`, `ls -R`, a git pathspec, the two directories of a `diff`, any path with `**` in it), so the path check treats a root that holds a secret as read: P4 must treat a recursive path as able to reach secret files by name. A path that names an obvious secret (`SECRET_PATTERNS`: `.ssh`, `.gnupg`, `.aws/credentials`, `.env`, `.env.*`, `*.pem`, `*.key`, `id_*`, `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`, `.config/gh/hosts.yml`, `.docker/config.json`, `.kube/config`, `.claude/.credentials.json`, `.bash_history`, `.zsh_history`, `.histfile`) is not proven, alone or under the `cd` targets, in any ASCII case (the default macOS volume ignores case). A recursive path that is a directory such a secret lies in (`.aws`, `.config/gh`) is not proven either. A glob is held against those names too: two or more literal characters when it stands alone in its component, none when a plain component stands beside it (`.kube/*`), and a leading wildcard may match a leading `.`, since the check cannot see `dotglob` or `GLOB_DOTS`. This is defence in depth: the path check owns the decision, and a recursive read that starts above a secret (`/srv/app` over `/srv/app/.ssh`) is its to refuse.
- Ruling: the `grep` that runs is ugrep, so its row has no `-NUM` (a recursion depth that turns on `-r`; GNU reads context lines), and a `-d` or `--directories` value is exactly `read`, `skip` or `recurse` (GNU accepts abbreviations such as `-d rec`, ugrep adds `dereference-recurse`, which follows symlinks); `-D` and `--devices` take `read` or `skip`. Cost if wrong: `grep -5 KEY f` is not proven.
- Limit: `&>/dev/null` is two things in dash (a background job and an empty redirect), so the allowed form assumes bash or zsh, the shells the commands run under.
- Limit: `rg` reads `RIPGREP_CONFIG_PATH`, which can add `--pre`, unless `--no-config` is given; that is the user's environment and is not checked.
- Hard precondition: nothing may allow a `Proven` on the strength of this module alone. Until P4's path check takes `paths`, `recursive`, `cd` and `runs_git_config`, `classify_readonly` stays out of every allow path, as it is in this change.
- rtk's gate takes two wrappers around the plain `rtk` command, once each and in this order: `cd TARGET &&` in front and `| head -n N` or `| tail -n N` behind. They use the shared helpers `literal_cd_target` and `head_tail_count`. The line is cut at its unquoted `&&` and `|`; each part must be plain; the shared parser must read the same stages. The target must exist and be a directory (symlinks resolved); the command's relative paths are resolved against that real directory before the `Read()` check, the target is read like an operand, and the `Read()` rules stay anchored to the directory the hook was called in, as the rule scan's are.
- rtk builds `anthology-shell` as a path dependency, so a build from this tree already has the DT-4 lexer fix. `rtk/Cargo.lock` lists the path package at `0.1.0`, which is only the package version, so no lock change is needed.

**Consequences**

`classify_readonly` is not wired into any hook in this change. The rtk change needs a new rtk release and an `rtk.lock` bump before users get it; until then the shipped rtk behaves as before. Cost if wrong: a classifier bug is a silent allow, so the table errs toward refusing; the first release of the permission mod should log what it proves for a while before it allows.

## ADR-033: the guard does not trust a value it cannot see (PE-1 to PE-6)

- **Date:** 2026-11-22
- **Task:** security review of the guard (bypasses found on `build` at `b42ae431`)
- **Status:** Accepted

**Context**

A security review found commands the guard allowed although bash 5.2 or zsh 5.9 ran a blocked program from them. zsh is the shell Claude Code uses on macOS. Each was checked in a container with a marker program on `PATH`. They fall into six classes, and each class has the same cause: a program, a script or an evaluated text came from a value the analysis cannot see (a variable, a positional parameter, a line read from input, an alias table), or the shell read a word in a way the parser did not model. `SECURITY.md` listed "commands built inside variables or arrays" as a limit; no ADR accepted it as a risk, and the owner asked for it to be closed.

**Decision**

Each class is too complex (G0). The analysis carries on to its end and is then marked incomplete, so a rule that blocks one of the command's readings still names itself (`${x:-npx} playwright install` stays G1). rtk's gate asks on an incomplete analysis, as before.

- **PE-1, a program or script from a parameter expansion.** The lexer marks each word that held an unquoted or double-quoted parameter expansion (`$x`, `${x}`, `"$x"`, `${a[0]}`, `$1`, `"$@"`, and zsh's `$=x`, `=$x`, `$x[1,3]`). Such a word in the program position is too complex, found through assignments, keywords and wrappers as a program from a command substitution already was (`env`, `command`, `exec`, `nice`, `time`, `sudo`, `xargs`, `if`, `!`, `{ }`, `( )`), and so is a `find -exec` or `fd -x` program with a `$` in it. So is a script that holds such a word: `eval "$x"`, `bash -c "echo; $x"`, `env -S "$x"`, a `trap`, `watch` or command-variable script. A single-quoted script (`sh -c 'wc -l "$1"' _ f`) holds none. zsh's `noglob` and `nocorrect` are looked through as keywords (`noglob $x` is too complex), a program word `=npx` is read as `npx` (zsh runs the path of `npx`), `coproc NAME { … }` is read as its body, a program word with an unquoted `*`, `?` or `[` (`/usr/local/bin/np?`) is too complex outside arithmetic, and so is a `find -exec` or `fd -x` program from the found name (`-exec {} …`).
- Ruling (review of #159, reversing the first ruling): a positional parameter (`$1`, `"$@"`, `$2`) as a program, or in a script, is too complex unless the analysis itself put a literal value there. That is in two places only. One: the top level of a shell's `-c` script whose arguments are all literal (no expansion, no `{}`, not fed by `xargs` or `parallel`), which the analysis also reads with them in place. Two: the `{ … }` body of a function the same text defines once, when every entry of the whole analysis that runs the function is a direct call after the definition with literal words (no expansion, no glob or brace character, no `~`, no leading `=`; review round 2), so each is read with its arguments in place. Review round 3, an allowlist in place of lists of setters and pattern characters: every direct-call word, and every argument of the `-c` script in place one, must be one or more of `[A-Za-z0-9_./:=,+@%-]` after quote removal (a leading `=` is not allowed either); the function body must hold no word that is a number or starts with digits followed by `=`, `+=` or `[` (zsh's `1+=x`, `print -v 1`, `read -r 1`), and no `${` with a positional parameter (digits, `@`, `*`, `#`) followed by `:` or `=` (zsh assigns it in `${1::=x}` and `${1:=x}`; this also counts as setting the parameters anywhere the analysis read them in place); and the body must not run `read`, `print`, `printf`, `typeset`, `declare`, `local`, `export`, `set`, `shift`, `zparseopts`, `getopts`, `eval`, `source`, `.`, `builtin`, `command` or `alias`. A function also run from a `trap`, an `eval`, a wrapper (`xargs f`), another shell (`export -f`) or a second definition, a body the analysis does not read (`f() ( … )`, zsh's `f() cmd`), a zsh anonymous function (`() { … } args`, `function { … } args`), a shell reading its script from stdin with arguments (`bash -s npx <<< …`, `bash /dev/stdin …`, `source /dev/stdin …`) and the top level of the command are all too complex. Defining `command_not_found_handle` or `command_not_found_handler`, which the shell calls with a missing command's words, is too complex. Where the positional parameters were read in place, a command that sets or moves them (`set a b`, `set -- a`, a bare `set`, `shift`, zsh's `argv=(…)`, `read -A argv`, any word naming `argv`, zsh's `zparseopts` in any form and `1=value`) is too complex. Why: the first ruling trusted every positional parameter that was not set in the command, and each of these runs one the analysis never saw. Cost if wrong: a wrapper whose arguments the analysis cannot see is blocked (`sh -c '"$1"' _ "$x"`); `run() { "$@"; }; run make test` stays allowed, while a `retry` wrapper whose body counts (`for i in 1 2 3`) is too complex since round 3, an accepted over-block.
- Ruling (carve-out a, review of #159, made an allowlist in round 3): a program word whose one expansion is a standard directory prefix at its start is read through. The prefix is `"$NAME/…"` or `"$NAME"/…` (double-quoted, no braces, no zsh modifier, no subscript), with NAME one of `HOME`, `TMPDIR`, `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PROJECT_DIR` and `VIRTUAL_ENV`. HOME is put in place with the home directory the guard already has; any other prefix stays an unknown directory. Every rule then reads the word, so the basename still meets G1 (`"$HOME/x/npx" playwright install` is G1). The read-through applies only when the whole command keeps to an allowlist: every expansion in the text is one of those two forms, and every other character is one of `[A-Za-z0-9_./:=,+@%-]`, a blank, a quote or one of `; & | < >`. So no brace, glob, `#`, `^`, `~`, backquote, `(`, `)`, backslash, other `$`, here-string or here-doc anywhere. On top of that, a read-through name may not appear in the text or in any word of the analysis outside those forms (a plain substring match, so it over-blocks), and the command may not source a file or run `eval`. Otherwise the old rule applies. Round 2 had a list of the ways to change a name instead, and round 3 found names it missed, built by brace expansion (`HO{M,}E`) or matched by a zsh pattern (`typeset -m`), so round 3 also drops the `${NAME}` form and the read-through of `$HOME/` in a double-quoted `sh -c` or `eval` script. `PWD` is not read through at all: Claude Code keeps the working directory between calls, so an earlier call can `cd` into a directory whose name holds `$(…)`, and the outer shell pastes that name into a script. Why: an expansion in double quotes is not split, so a standard prefix only names a directory, and these are how project tools are run. Cost if wrong: a way to change one of these names with only allowlisted characters and without spelling it would put another directory in place, though the literal rest of the word, and its basename, still meet every rule. Accepted over-blocks: `"${HOME}/.cargo/bin/cargo" build`, `sh -c "cd $HOME/proj && make"` and any prefixed command with a glob, brace, comment or backslash in it are too complex.
- **PE-2, zsh glob qualifiers.** A `(` right after a character of a word is a glob qualifier or pattern group in zsh, and a qualifier runs code (`*(e:'cmd':)`, `a.rs(+cmd)`, even with no glob character in the word). It is too complex, except in arithmetic text, inside `[[ … ]]`, and in `f()`, a function definition. So is a `(` right after a `)` (`(a|b)(e:'cmd':)`), and one after a word character, a quote or a `)` inside an unquoted `${…}`, whose default word zsh globs (`${x:-a.rs(e:'cmd':)}`). A `$((…))` inside double quotes, in a `${…}` and in a heredoc body is now arithmetic text too, as an unquoted one was.
- **PE-3, code from a value.** bash's prompt expansion `${x@P}` (it runs a `$(…)` in the value; `@E`, `@Q` and `@A` run none), zsh's `(e)` flag among any flags, zsh's `${~x}` and `$~x` and any word that names `GLOB_SUBST` or `PROMPT_SUBST` (in any spelling zsh accepts), and a prompt variable (`PS1` to `PS4`, `PROMPT*`, `RPROMPT*`, `RPS1`, `RPS2`, `PROMPT_COMMAND`) set to a value holding `$` or a backquote, or named to `read`, `printf -v` or `declare` (`PS4='$(cmd)'; set -x` runs it in bash). The startup file names `BASH_ENV` and `ENV` count as prompts (`BASH_ENV='$(cmd)' bash -c :` runs it). The same `${…}` checks apply to an unquoted heredoc body, which expands as a double-quoted word does. A nameref to a prompt, a startup name or a table (`declare -n p=PS4`), one from an expansion (`local -n r=$1`) or a bare one (`declare -n p`), a name `declare`, `typeset`, `local`, `readonly` or `export` set from an expansion (`declare "$x=…"`), and an option name from an expansion (`setopt ${o}subst`, `set -o $o`) are too complex.
- **PE-4, a value read in and then evaluated.** bash and zsh evaluate subscripts and arithmetic, and a value evaluated there runs its substitutions: `read s <<< '[$(cmd)]'; unset "a$s"`. A here-string or heredoc is not scanned as a command, so when the command holds a `read`, `mapfile`, `readarray` or `select`, or a value with a closed `$(…)` or backquote pair as text (an assignment, or an argument of `set` or `printf`), any evaluating use is too complex: an arithmetic context (`$((…))`, `((…))`, `$[…]`), `let`, an `-i` declaration, `[[` with an arithmetic operator or `-v`, a `$` in a name that `unset`, `declare`, `typeset`, `local`, `export`, `readonly`, `read`, `printf -v` or `test -v` evaluate, a subscript after a name that names a variable (`a[$s]`, `${a[s]}`; not `${a[@]}` or `a[0]`), an indirect `${!s}`, or an offset that names one (`${x:$s}`; not `${x:0:7}`).
- Ruling (carve-out b, review of #159 round 3, reversing the round 1 and 2 rulings): the rule above holds as written, with `select` counted as a command that reads input. Rounds 1 and 2 tracked which names may hold a read value, so that a read loop that only counts was allowed, and each round found a way to reach a read value through a name the tracking missed (a plain `n=line`, a `${n:=line}` default, `for n in line`). Why: no tracking is smaller than tracking every way a name gets a value. Cost: a counted read loop (`while read -r l; do n=$((n+1)); done < f`) is too complex, an accepted over-block. The `.env` loader `while read -r k v; do export "$k=$v"; done < .env` stays too complex (zsh runs a subscript in that `export`); `set -a; source .env; set +a` loads the same file.
- **PE-5, nested subscripts.** A subscript after a name whose text before its first `]` holds another `[` or a quote, with a closed `$(…)`, backquote pair or `$'…'` after it, is too complex (`a[[]$(cmd)]`, `a["]"$(cmd)]`). The shells count nested brackets and quotes to find its end; the existing subscript scan closes it at the first `]`. The scan is a separate check, so `subscript_runs` is unchanged.
- **PE-6, a renamed program.** An alias definition (`alias u=…`, zsh `alias -g` and `-s`), a hashed path (`hash -p /path u`, zsh `hash u=/path`) and an assignment to the tables behind them (`BASH_ALIASES`, `BASH_CMDS`; zsh `aliases`, `galiases`, `saliases`, `commands`, and `functions`, which defines a function from text) are too complex. A bash table is matched in any word, not only an assignment (`printf -v 'BASH_CMDS[u]' …`, `declare -n r=BASH_CMDS`), and so is an exported function's environment name (`env 'BASH_FUNC_u%%=() { …; }' bash -c u`); a zsh table with a subscript or a value, or named to `set -A`, `typeset`, `read` and the like (`set -A commands u /path`). Ruling: blocking the definition is smaller than following aliases, because an alias body can be any prefix of a later command (`alias u=npx` then `u playwright install`). Cost if wrong: defining an alias in a Bash call is blocked; listing (`alias`, `alias ll`), `unalias` and `hash -r` are not.

**Known limits**

- A command's output assigned to a variable and then evaluated is not covered: `s=$(printf '…'); echo $((s))` still runs what the output holds. Blocking every substitution followed by arithmetic would block common counting and timing commands (`n=$(wc -l < f); echo $((n * 2))`), and the output is no more visible than a file's.
- A file's content stays unseen, as before: a script run with `bash ./x.sh`, a file read with `read`, or a `.env` file sourced.
- Under zsh, `emulate sh` or `emulate ksh` did not make an expansion's glob qualifier run in the checks above; other option changes than the ones named are not modelled.
- A value whose `$(…)` is assembled from pieces (`p='$'; p+='(cmd)'`) is not seen as code by the PE-4 check, which looks for a closed `$(…)` or backquote pair in one word. Ruling (review of #159): a known limit, not fixed here; a read value that reaches an evaluation is still caught.
- The prefix values other than HOME come from Claude Code's environment, which the guard does not see; they are trusted as directories only, never read into a script.

**Accepted false positives** (review of #159): `${EDITOR:-vi} file`, `"${cmd[@]}"`, `$SHELL -c …`, the `.env` loader loop above and zsh-style text such as `echo foo(bar)` stay too complex. Carve-out (c): when the command holds a double-quoted `sh -c`, `eval` or `trap` script with an expansion in it, the `(G0)` message adds that the outer shell fills it in first, and suggests single quotes with the values passed as arguments.

**Consequences**

Commands with a program from a variable now need the program spelled out, except under a standard directory prefix, and the message says to simplify the command. The frozen Python rows for two array-element programs (`test_allowed_round_1_neighbours` rows 40 and 41) are stricter by owner direction and are listed in `rule_table.rs`. The `guard.check`, `guard.hook` and `shell.analyze` goldens are blessed with the new answers (`difftest golden bless`); each case that changed was allowed by Python and is now blocked as too complex, or was blocked by Python and is now blocked as too complex, and each `shell.analyze` case that changed is now incomplete. The review round blessed them again: the new changes are generated words with a glob character in the program position, `)(` in generated text, the two `xargs`/`eval` positional seeds, and the `(G0)` hint on the hook surface. Evidence: `crates/anthology-guard/tests/param_expansion.rs`; every bypass in it failed before the change, and each ran a marker program in bash 5.2 or zsh 5.9.

## ADR-034: the path and git check behind `anthology perm classify`

- **Date:** 2026-11-22
- **Task:** post-release plan P4 (the permission-levels design, item 1)
- **Status:** Accepted

**Context**

ADR-032 left the classifier's `Proven` unusable on its own: the paths it lists are as written, it never looks at the file system, and a `git` line runs whatever the repository's configuration names. The permission mod (P5) needs one call that says "allow" only when all of that has been checked, and that says nothing otherwise.

**Decision**

- `anthology perm classify` reads `{"command","cwd","roots","read_rules"}` (exactly those keys, none twice, at most 1 MiB). On a proof it exits 0 and prints the `anthology-perm:read-only:v1` line, then one resolved absolute path per line: every `cd` target, every file read, every start directory of a recursive reader, and for git the work tree, the git dir and the common dir. The permission mod checks each of those with Claude Code's own Read permission (security review of #164). Anything else exits 1 with one `no decision` reason line and nothing on stdout. It never denies. It lives in `anthology-cli` (`cmd/perm.rs` and `cmd/perm/`), not in `anthology-guard` as the design first placed it: another change was editing the guard crate. Cost if wrong: one module to move later.
- Ruling: the request's `cwd` is not trusted to be the shell's working directory (subagents and worktrees run elsewhere, and no quick test confirmed what Claude Code reports). Only a leading absolute `cd` says where the rest runs: without one, a relative path, an implicit `.` (`ls`, `rg x`) and any git line refuse. Cost if wrong: `ls` and `git status` typed bare are never allowed; `cd /abs && …` and absolute paths are.
- Ruling: the command must be printable ASCII (tabs allowed): NUL, control characters and invisible Unicode (zero-width, bidi controls, a BOM) refuse, and so does any other non-ASCII text. Cost if wrong: a file name with an accent is not proven.
- Paths: the `cd` targets and every path are resolved with symlinks followed and must exist, lie under a root (compared without case on macOS, whose default volume ignores it) and be a regular file with one link or a directory; a missing path, a dangling symlink, a FIFO, socket or device, a file with more than one hard link (another name for a file that may lie anywhere), `/dev` and `/proc` refuse. Each is held against the `Read()` rules and the secret patterns as written and as resolved. A glob refuses: what the shell expands it to is not known here. A recursive path refuses when an anchored `Read()` rule could match anything below it, existing or not; then the tree is walked and every entry is held against the name rules and the secret patterns; more than 20,000 entries refuses. Cost if wrong: `rg x .` in a repository with a build folder gets no decision, and so does any recursive read of a tree that holds a `.env` file or lies over a `Read()`-denied path, even when the program would skip it.
- Ruling (review round 2 of #172): any symlink below a recursive start refuses, for every recursive reader. GNU and Apple `diff` comparing two directories follow a symlink they find inside them, so the earlier claim that no program of the table does was wrong. A hard-linked file refuses at the start and in the walk. Cost if wrong: a tree with a symlink in it (a `node_modules/.bin`, say) is never read without a prompt.
- Ruling: the `Read()` anchors are read wide. `/p` is tried under every root, the working directory, `~/.claude` and `/`; `./p` and `p/q` under the working directory and every root; each base as given and resolved; case always folded. A bare `Read`, `Read()`, `Read(*)` and `Read(**)` deny every read; a rule text that is not `Read(...)` refuses, and so does a rule holding `{`, `}` or `!` (brace or negation syntax this check does not read). Cost if wrong: a rule meant for one directory also stops reads in another.
- Ruling: `PATH` is read from this process's environment, which the hook shares with the shell that runs the command. An empty or relative entry refuses every line, and so does any entry that, as written or resolved, lies under a root, the request's `cwd` or a `cd` target: a file there could stand in for `cat` or `git` (round 2, C2). Every program the line runs (round 3) is found on `PATH` the way the shell finds it and resolved with its symlinks followed, and the real file must lie outside those places too, so a `cat` linked from a folder outside the roots to a file inside one refuses. A program not on `PATH` is skipped, since the shell cannot run it. The word resolved is the program the shell runs (`prog`), never a word before it: a `NAME=value` assignment or any wrapper word before the program, and a program word that is quoted or holds a backslash (`\cat`, `'cat'`), give no decision, on top of the classifier's own refusal. The git this check asks is that resolved git, so nothing under those places is ever run. Cost if wrong: a project that puts its own `bin` folder on `PATH` gets no decision for any line.
- Git is asked, not imitated: `git rev-parse` and `git config --list --includes --show-origin -z` run in the resolved working directory with the same environment, so the repository and its configuration are found exactly as the command's git finds them (system, global, local and worktree files, `include` and `includeIf`, `.git` files, bare repositories reached by `cd`). Neither runs a configured program. A non-zero exit, any stderr or a 2 s timeout refuses.
- Ruling: every configuration key must be on an allowlist (`safe_key`): a few `core.*` booleans, `user`/`author`/`committer` names, `init.defaultBranch`, `remote.<n>.url`/`pushurl`/`fetch`, every `branch.<n>.*` key, `include.path`, `includeIf.<c>.path` (what they bring in is listed and checked too), `safe.directory`, `color.*`, `advice.*`, `pull.rebase`/`ff`, `push.default`/`autoSetupRemote`, `fetch.prune`, `diff.renames`/`algorithm`. Everything else refuses, which covers a partial clone, `core.fsmonitor`, `core.pager`, `core.hooksPath`, `diff.external`, `diff.*.textconv`, `filter.*`, `alias.*`, `credential.helper`, `core.gitProxy`, `protocol.*` and keys not known today. Cost if wrong: git-lfs installs `filter.lfs.*` globally, so its users get no decision for any git line until the allowlist is widened by a later decision.
- Ruling (round 2, gap 4): `credential.helper=osxkeychain` is allowed when, and only when, `--show-origin` names Apple's own system file (`/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig` or the Xcode one). It is told apart by that path, never by scope: the same key and value in a repository, global or included file refuses, and so does any other helper.
- Ruling (rounds 2 and 3, I1 and I2): the word-level check (`cmd/perm/words.rs`, on `analyze`'s program and arguments) refuses any git word holding a signature atom (`%G…`, `%(signature…)`, `--sort=signature`), which runs gpg, and reads every git subcommand against an option allowlist of its own: any option off the list gives no decision (round 3 replaced the round-2 denylist, which `log -U3` and `log --check` got past). `log` takes only `--oneline`, `-n N`/`-N`/`--max-count`, `--skip`, `--format`/`--pretty`, `--graph`, `--decorate`, `--all`, `--branches`, `--tags`, `--first-parent`, `--reverse`, `--date=`, `--since`/`--until`/`--after`/`--before`, `--author`, `--committer`, `--grep`, `--` and operands. `status` has no `-v`; `diff` (work tree against the index) has no `--cached`/`--staged` and, like `blame`, no operand that could be a revision (anything but `./…` and `/…`); `branch`, `remote`, `ls-files`, `rev-parse` and `describe` take the listing options of the table. `show`, `cat-file`, `grep` and every other subcommand refuse. It also refuses `grep`/`egrep`/`fgrep` with `GREP_OPTIONS` set and `rg` with `RIPGREP_CONFIG_PATH` set. Cost if wrong: `git show`, `git log -p` and any `log` option off the list always prompt.
- Ruling: any `GIT_*` environment variable refuses a git line, whatever its value. `PAGER` is not checked: git only pages to a terminal, and the Bash tool's output is not one. `HOME` is read for the rules and the classifier; `PATH` is read to refuse a relative entry and to find the git to ask; git is run with the environment as it is, so it reads the same global files (`XDG_CONFIG_HOME` included) as the command's git.
- Git beyond the configuration: the git dir, the common dir and the work tree must lie under a root and match no `Read()` rule or secret pattern; an anchored rule that could match anything below one of them refuses, since `diff` and `blame` print what lies there (a name-at-any-depth rule is held against the tracked paths, not git's own files). Any file in the hooks folder but `*.sample` refuses (`git status` writes the index, which runs `post-index-change`). The object store must resolve under a root, with no symlink in it or in its `pack` folder. `objects/info/alternates`, `.gitmodules`, a `modules` folder and a gitlink in the index refuse (git runs `status` inside a populated submodule with that repository's configuration, `.gitmodules` or not). The index (versions 2 to 4) is read and every tracked path is held against the rules and the secret patterns; a split or sparse index refuses. Cost if wrong: a repository that tracks `.env.example` gets no decision for git.
- Every git trap has a test that first shows, with the real git, that the trap runs a marker program, then that `classify` gives no decision.
- `ANTHOLOGY_NO_INSTALL` (any non-empty value) makes `bin/anthology` and `bin/anthology-mem` exit 127 after the installed-binary check, before any download or install.
- Limit: history is not searched (a name scan of every commit costs hundreds of milliseconds in a repository of a few thousand commits), which is why every git form above that prints history refuses. `git <rev> -- <path>` stays refused by the classifier (ADR-032).
- Ruling (round 2, gap 5): a key written twice in the request refuses; the parser alone keeps the last one, and a reader that keeps the first would see another request.
- Limit: the command runs in the user's shell, whose snapshot can hold aliases, functions and environment this process cannot see; files can change between the check and the run.

**Consequences**

Nothing calls `perm classify` yet; P5 wires it into the permission mod behind the core verdict. Versions are left to the release branch; the plugins pick the command up with the next `anthology.lock` bump. An older binary answers `perm` with usage (exit 2), which the mod treats as no decision.

## ADR-035: rtk is renamed ATL (Anthology Token Layer)

- **Date:** 2026-11-22
- **Task:** the ATL rename (plan: `atl-rename-plan.md`; T4 is settings, setup and docs)
- **Status:** Accepted

**Context**

The tool this repo ships as `rtk` is a fork of rtk by RTK AI Labs (github.com/rtk-ai/rtk), Apache-2.0 edition only: the BUSL-1.1 parts were trimmed (ADR-017). The owner decided to rename it everywhere. The Apache-2.0 license (section 6) does not let a derivative use the licensor's names as its own product name, and a fork that keeps shipping as `rtk` next to the upstream tool collides on the Bash path and in people's heads. Every ADR, CHANGELOG entry, plan and record dated before this one keeps the name it was written with; this ADR is the map from old to new.

**Decision**

| Before | After |
|---|---|
| tool, binary, package, folder `rtk/` | `atl` (`atl/`) |
| display name | ATL (Anthology Token Layer); "ATL" on first mention in docs |
| shims `bin/rtk`, `bin/rtk-hook` | `bin/atl`, `bin/atl-hook` |
| `plugins/anthology/rtk.lock` | `plugins/anthology/atl.lock` |
| release tag `rtk--v<version>` | `atl--v<version>`; the tags `rtk--v1.2.1-anthology.1` to `.5` stay where they are |
| release assets `rtk-<target>.tar.gz` | `atl-<target>.tar.gz`, holding `atl`, `LICENSE` and `NOTICE` |
| workflow `rtk-release.yml`, CI job `rtk-test`, scripts `rtk-*.sh`, `sync-rtk.sh` | `atl-release.yml`, `atl-test`, `atl-*.sh`, `sync-atl.sh` |
| environment `RTK_*` | `ATL_*` |
| state.json key `rtk_path` | `atl_path` |
| install `<plugin data>/rtk/<version>/rtk` | `<plugin data>/atl/<version>/atl` |
| history `~/.claude/anthology/rtk/` | `~/.claude/anthology/atl/` |
| project filters `.rtk/filters.toml` | `.atl/filters.toml` (trust is asked again) |
| settings allow `Bash(rtk *)` | `Bash(atl *)`, tag `token-layer` |
| `anthology rtk-install` | `anthology atl-install` |
| settings `ALLOW_RTK` | `ALLOW_ATL`, with `TAG_TOKEN_LAYER` and `LEGACY_ALLOW_RTK` |

- **License.** `atl/LICENSE` stays verbatim (Copyright 2026 RTK AI Labs). One `atl/NOTICE` says that ATL is a modified version of rtk (github.com/rtk-ai/rtk), Copyright 2026 RTK AI Labs, Apache-2.0, renamed and trimmed by Anthology. Every ATL archive ships the license and the NOTICE. The README and the crate description keep "a fork of rtk by RTK AI Labs". Upstream URLs, the authors and copyright lines in the crate and `atl/CHANGELOG.md` do not change.
- **No alias.** There is no `rtk` command, wrapper or hook name any more.
- **Session start** installs ATL into `<plugin data>/atl/` and writes `atl_path`. It never removes `rtk_path` or `<plugin data>/rtk/`.
- **The allow rule.** The `Bash(atl *)` op carries the tag `token-layer`, which is replaceable. In `reconcile` a stored op with no tag that adds `Bash(rtk *)` to `permissions.allow` counts as `token-layer`, so a re-run reverts it and adds the ATL rule. Only a rule an earlier setup recorded is removed; a `Bash(rtk *)` the user wrote is never touched. The op set changes, so `OPS_CHANGED` nudges a setup re-run.
- **Setup apply** deletes the legacy leftovers, shown in the preview first, in this order: write `settings.json`; drop `rtk_path` from `state.json`; delete `<plugin data>/rtk/`; delete `~/.claude/anthology/rtk/` (the old history). The folder guards (not a symlink, under the plugin data folder, real path as expected) are the ones `remove` uses. If the settings write fails nothing else runs, so a retry finds it all. `setup remove` covers the ATL and the legacy targets. The owner's answer: the old history is deleted, not migrated; ATL starts with an empty one.
- **Never touched:** upstream rtk's own folders (`~/.local/share/rtk`, `~/Library/Application Support/rtk`, a project's `.rtk/`), the first release's default history folder, and any `rtk` the user installed on their own.
- **Security strings.** The guard and the hook keep recognising `rtk` as a legacy name next to `atl`: the hook's mention check, the shell peel (`TOKEN_LAYER_NAMES`), the gate's own-name check. A command typed with the old name is still checked.
- **Fail-closed windows.** Plugin updated before setup: `atl` rewrites prompt. Download fails: `atl-hook` exits 0. An old session after setup: the old hook exits 0 and `rtk` is not found. The guard keeps running in all three.
- **Guard pin.** The ATL peel needs a new `anthology` binary release, so `anthology` pins `anthology-guardrails` at `~0.2.7` (0.2.2 to 0.2.6 ship on binaries without the ATL peel).

**Consequences**

One rename touches about 15,000 lines, so it lands as one integration branch (T1 binary, CI and scripts; T2 shell and guard; T3 install, hook and doctor; T4 settings, setup and docs) behind a repo test that fails on any `rtk` outside an allowed path or an allowed line. The users' cost is one `/anthology:setup` run after the update. Cost if wrong: a user who never re-runs setup keeps prompts on `atl` rewrites and the old files on disk, which are small; both fail closed.

## ADR-036: the version-bump gate compares a released plugin with its latest release tag

- **Date:** 2026-11-24
- **Task:** the version-bump gate fix in the 0.2.2 release batch (`crates/anthology-dev/src/version_bump.rs`)
- **Status:** Accepted

**Context**

The `version-bump` job asked one question of a changed plugin: is its version above the version at the merge base of the pull request and its base branch? When two branches changed the same plugin, the first to merge bumped it, and every other branch then had to bump it again to pass, which also made `CHANGELOG.md` and `plugin.json` conflict on each merge. The release tag, not the base branch, is what users hold, so the tag is the better floor.

**Decision**

- A plugin counts as released once a tag `<plugin>--v*` exists on a commit that is an ancestor of the base. Only those tags count, so the tag a release commit creates in the pushed range is not a release that its own push has to exceed. An unreleased plugin needs no bump, as before.
- A changed released plugin passes when its head version is above the highest `<plugin>--v*` tag merged into the base (semver, matched by the exact prefix `<plugin>--v`, so `pp--v1.0.0` is no release of `p`) and not below its version at the merge base. A bump that already landed on the base therefore counts, and a branch does not bump it again.
- A tag whose version is not valid semver is ignored. When no tag is valid, or the head version is not valid semver, the old comparison with the merge-base version applies.
- Ruling: the latest release is the floor, not the base branch's unreleased version. Cost if wrong: a bump already on the base lets a later branch pass without a bump of its own, so two branches can share one unreleased version. That is intended. The changes of both belong in that release, and `release.sh` is the step that fixes the number.

**Consequences**

Branches no longer re-bump a plugin that the base already moved past its last release, which removes the usual conflict on `plugin.json` and the changelog heading. A plugin that is still at its latest release, or below it, still fails, and the message names that release.

## ADR-037: host mode, where an app drives Claude Code

- **Date:** 2026-11-24
- **Task:** host mode for embedded sessions in the 0.2.2 release batch (the delegation mod, the session-start hooks and the G0 message)
- **Status:** Accepted

**Context**

Programs other than a person at a terminal drive Claude Code through its SDK. Such an app owns its system prompt and its task, and nobody there can read the lead instructions Anthology adds (the routing guide, the memory titles, the superpowers introduction, the coordinator section) or act on them. The delegation mod could refuse the host's own `Edit`, `Write` or `NotebookEdit` calls. The block message also offered `/plugin disable`, which no one in such a session can run.

**Decision**

- A session is an embedded host when `ANTHOLOGY_HOST_MODE` is not `full` and either `CLAUDE_CODE_SESSION_ATTENDED` is `0` or `CLAUDE_CODE_ENTRYPOINT` starts with `sdk-`. With no such variable set, there is no host mode.
- In a host, Anthology adds no lead context: no guide, no memory, no superpowers bootstrap and no coordinator section, and the delegation mod refuses nothing. Guard verdicts do not change, and the G0 message drops its `/plugin disable` line.
- `ANTHOLOGY_HOST_MODE=full` opts back in to everything. The evals set it.
- One predicate, three readers: the delegation mod (`isEmbeddedHost` in `hooks/mods/delegation.ts`), the `anthology-superpowers` session-start hook (a patch in `vendor/patches/superpowers.patch`) and the Rust side (session start and the G0 message).
- Why these signals: Claude Code recomputes the attended flag for each session, and an interactive `cli` or IDE session reads attended as `1`, so a person at a terminal is never taken for a host.
- Ruling: the check changes what Anthology says, never what it blocks. Guard verdicts are the same in a host. Cost if wrong: a session that Claude Code reports as unattended or as an `sdk-*` entrypoint, though a person uses it, gets no guide, memory or coordinator section until `ANTHOLOGY_HOST_MODE=full` is set. Its safety checks are unchanged.

**Consequences**

Hosts stop paying for instructions they cannot use, and their own edits are no longer refused. The same two variables now decide the question in TypeScript, shell and Rust, so a change to one reader needs the same change in the other two.
