# Anthology — Plan

> Opus plans, Sonnet builds, Haiku scouts — a model-tiered agent team for Claude Code.

Status: **approved — building** · rev. 6 · 2026-11-20 · Owner: rbonweb
The owner approved the build on 2026-11-20 and asked for the repo to be **public from creation**, with no further questions: the lead makes every remaining decision and records it in Part 3.

This is the single plan file:
- **Part 1** is the design (what and why).
- **Part 2** is the build (how, task by task).
- **Part 3** collects the decisions recorded while building.

Every choice here favors the long-term: years of maintenance, upstream changes, Claude Code updates and teams of 4–5 people. A choice is never made just because it's the quickest way to a first release.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement Part 2 task-by-task, with the model assignments in **Build team**. Steps use checkbox (`- [ ]`) syntax for tracking.

---

## Part 1 — Design

### 1. Goal

A Claude Code plugin marketplace that gives every project:

1. A **roster of agents** with fixed models, tools, and knowledge, so work is split by model strength.
2. **Agent-owned knowledge**: every skill we ship belongs to an agent. There are no loose skills. The main session sees only `anthology-superpowers` and three user-only commands (`/anthology:setup`, `/anthology:export`, `/anthology:import`). Each agent reads only its own knowledge.
3. **One house style, preconfigured**: opinionated defaults with no user-facing modes or levels. Every agent writes in one fixed terse style (adapted from caveman).
4. **Guardrails** that stop agents from installing browser tooling on the host, touching the user's Chrome, using any browser other than BrowserOS Neo, or writing secrets into memory.
5. A **routing guide** that tells the main session when to delegate, to whom, and under which rules. A session-start hook delivers it, so it always matches the installed version (§7).
6. **Token-saving command output (rtk)**, always on: a Claude-only build of the owner's rtk fork rewrites Bash commands (`git status` → `rtk git status`), so the main session and every agent get condensed output (§12).
7. **Memory that lives in the repo**: plain Markdown notes committed with the project, so they survive a re-clone, a folder move and a team merge. A local SQLite graph and search index is built from them. Personal notes stay outside the repo, and export/import moves everything between machines (§13).
8. **Built to be maintained**: versioned releases that reach installed users, CI, behaviour evals, reproducible vendoring, an upstream watcher, and clean update and uninstall paths (§14).

It is published as an open-source repo, `rbonweb/anthology`. **Installing the one `anthology` plugin brings everything else with it** (guardrails, anthology-superpowers). Users who install it follow its rules. There are no options to configure. **Supported platforms: macOS and Linux.**

### 2. Non-goals

- No MCP connectors to SaaS tools (Slack, Notion, Jira, Uizze's paid MCP, …).
- No standalone skills besides the user-only commands `/anthology:setup`, `/anthology:export` and `/anthology:import`. Knowledge is never exposed to the main session.
- No user-facing modes, levels or toggles (e.g. no `/caveman ultra`, no "stop caveman").
- No hand edits to vendored content. Every change to upstream material is a reproducible patch or transform (§6, §14).
- No custom worktree location. The built-in `.claude/worktrees/` is used.
- No sandbox/OS-level enforcement. The guardrails are a hook plus Claude Code's native permission deny rules, not a jail.
- **No browser provider other than BrowserOS Neo.** agent-browser, Playwright MCP, chrome-devtools MCP and Claude in Chrome are not used. Nothing we ship launches Chrome itself.
- **No vector embeddings.** An embedding model would need a cloud API key (a SaaS connector) or a large local download. The memory index is a SQLite graph plus full-text search, derived from the notes (§13).
- **rtk supports Claude Code only.** No Cursor, Copilot, Codex, Gemini, opencode-as-host, Hermes, OpenClaw or other AI-tool integrations. No telemetry.
- **No Windows support** (for now). Setup refuses on Windows with a clear message, and no Windows rtk binary ships. Adding Windows later is a deliberate project: its own CI job, PowerShell-aware guardrails, and native wrappers.
- **No credit text during the build.** The owner adds third-party credits after the build and before publishing. The build does record each source's SPDX license id as metadata, so redistribution is checked up front (§14).
- Nothing in `~/.claude/CLAUDE.md` is ever written by anthology.

### 3. Repository layout

```text
anthology/
├── .claude-plugin/marketplace.json        marketplace "anthology", 3 plugins
├── plugins/
│   ├── anthology/
│   │   ├── .claude-plugin/plugin.json     dependencies: anthology-guardrails, anthology-superpowers (version ranges); engines
│   │   ├── agents/                        11 agent files
│   │   ├── knowledge/                     agent-only knowledge (NOT skills; see §6), generated from vendor/
│   │   ├── skills/{setup,export,import}/  the only skills (user-invoked only)
│   │   ├── hooks/hooks.json               PreToolUse(Bash) → rtk; SessionStart → session_start.py
│   │   ├── bin/rtk, bin/anthology-mem     wrappers on the Bash PATH (resolve paths via ~/.claude/anthology/state.json)
│   │   ├── rtk.lock                       pinned rtk version + sha256 per platform
│   │   ├── templates/routing-guide.md     the guide text (delivered by the hook)
│   │   └── scripts/                       session_start.py, doctor.py, rtk_install.py, memory_sync.py,
│   │                                      memory_index.py, memory_portable.py, settings_patch.py
│   ├── anthology-guardrails/
│   │   ├── .claude-plugin/plugin.json
│   │   ├── hooks/hooks.json               PreToolUse(*) → guard.py (exec form)
│   │   └── scripts/guard.py, guard_rules.py
│   └── anthology-superpowers/             superpowers, generated from vendor/ (renamed by transform)
├── vendor/
│   ├── sources.json                       every upstream: repo, path, commit/checksum, SPDX license, target, transform
│   ├── patches/<name>.patch               anthology's changes on top of pristine upstream
│   └── snapshots/ui-taste/                pristine copy (no git upstream exists)
├── rtk/                                   rtk source, git subtree with full history (fork + upstream merges), trimmed by script
├── scripts/vendor.py                      fetch / regenerate / check / sync for everything in vendor/
├── scripts/sync-rtk.sh, scripts/rtk-trim.sh
├── .github/workflows/                     ci.yml · evals.yml · upstream-watch.yml · rtk-release.yml
├── .github/dependabot.yml
├── evals/                                 behaviour eval cases (claude plugin eval; repo root so all 3 plugins load — U9)
├── tests/                                 stdlib unittest (+ pinned test deps), run in pinned Docker images
├── PLAN.md · README.md · CHANGELOG.md · SECURITY.md · CONTRIBUTING.md · DECISIONS.md · LICENSE (Apache-2.0)
```

**Dependencies (one-plugin install).** `anthology`'s `plugin.json` declares `anthology-guardrails` and `anthology-superpowers` as dependencies, with version ranges on the current major. Bare names resolve inside this same marketplace, so no cross-marketplace allowlist is needed. Disabling a dependency disables `anthology` too.

**Why superpowers is vendored and renamed to `anthology-superpowers`.**
- The main session uses it for brainstorming, writing plans and subagent-driven development.
- Shipping it under the original name would clash with the official copy many users already have: when two enabled plugins share a name, Claude Code drops one copy's hooks and the commands collide.
- The rename is a deterministic transform over pristine upstream. It changes the plugin name and every `superpowers:<skill>` reference to `anthology-superpowers:<skill>`. So syncing upstream means re-fetching and re-running the transform, never re-editing by hand.
- `/anthology:setup` recommends uninstalling the official superpowers. Both copies would work side by side, but their instructions would load twice.

**Why design-skills lives in knowledge:** only agents use it. It's copied unchanged into `knowledge/design-skills/`, and its 10 skills are shared out to agents (§5).

### 4. Model roles

| Model | Role | Good for | Not for |
|---|---|---|---|
| Opus | Lead / researcher / reviewer | Ambiguous problems, research, architecture, decomposition, root-causing unknown bugs, high-stakes review | Mechanical edits, bulk searching |
| Sonnet | Builder | Implementing a clear plan, multi-file edits, tests, scoped refactors, reproducible bugs, docs, UI builds | Deciding *what* to build |
| Haiku | Scout / runner | Finding code, summarizing output, running tests and reporting, renames, boilerplate | Anything needing judgment |

The main session runs on Opus and orchestrates. Small tasks (a handful of steps) are done inline, not delegated. The main session talks to the user in normal prose; the terse house style applies to agents. Model aliases (`opus`, `sonnet`, `haiku`) follow new models automatically, and the behaviour evals (§14) catch regressions when they change.

### 5. Agent roster (`plugins/anthology/agents/`)

Every agent also reads `caveman` (the house style). `ds/x` = `knowledge/design-skills/skills/x`. "Preloaded" skills come from `anthology-superpowers`.

| Agent | Model | Edits | Worktree | Knowledge (agent-only) | Preloaded |
|---|---|---|---|---|---|
| researcher | opus | no | no | competitive-brief, synthesize-research, research-synthesis, ds/ux-research, ds/journey-mapping | — |
| architect | opus | no | no | system-design, architecture, testing-strategy | writing-plans |
| implementer | sonnet | yes | yes | — | test-driven-development, verification-before-completion |
| debugger | sonnet | yes | yes | — | systematic-debugging |
| reviewer | opus | no | no | code-review-checklist, tech-debt, deploy-checklist | — |
| scout | haiku | no | no | — | — |
| docs-writer | sonnet | yes | yes | documentation, ds/ux-writing | — |
| ui-designer | sonnet | yes | yes | frontend-design, design-taste-frontend, ui-taste, landing-page, web-design-system, design-handoff, ds/design-systems, ds/interaction-design, ds/design-elevation | — |
| ui-reviewer | opus | no | no | web-design-guidelines, web-review, ds/design-critique, ds/accessibility-audit | — |
| product-planner | opus | docs only | no | write-spec, sprint-planning, roadmap-update, product-brainstorming, ds/ux-strategy, ds/design-ops | — |
| marketer | sonnet | docs only | no | seo-audit, campaign-plan, content-creation, email-sequence | — |

- **Every knowledge folder is used by at least one agent.** There's no loose knowledge, and a test enforces it.
- **How an agent loads its knowledge:**
  - The agent body starts with a *Startup* section that lists `${CLAUDE_PLUGIN_ROOT}/knowledge/<x>/SKILL.md` paths. Claude Code substitutes `${CLAUDE_PLUGIN_ROOT}` in agent bodies.
  - The agent must Read every listed file before any other work.
  - Paths inside a knowledge file are relative to that file's folder. A reference like `design-skills:<x>` means `knowledge/design-skills/skills/<x>/SKILL.md`.
  - The evals check that every agent does this (§14).
  - Outside auto mode a plugin agent may not Read its own plugin files without permission (U1). So `/anthology:setup` adds one narrow allow rule built from the **resolved** plugin location: the parent of `${CLAUDE_PLUGIN_ROOT}` + `/**`. That covers every cached version, and a local marketplace that loads in place. `session_start.py` warns if the rule no longer matches the current root (re-run setup).
- **Startup budget.** Each agent's startup reading (its body, the Startup files and its preloads) has a byte budget enforced by a test. Upstream growth shows up in sync PRs instead of silently raising every call's cost.
- **ui-designer: which knowledge leads.** For websites and landing pages, `design-taste-frontend` leads. For app screens (web or iOS), `ui-taste` leads. `frontend-design` is the baseline for everything else. The rest are supporting.
- **ui-reviewer** is split out from ui-designer so the builder and the critic are different models (Sonnet builds, Opus reviews). It uses BrowserOS Neo to look at the rendered page.
- **Edit permissions.** "No edits" = `disallowedTools: Edit, Write, NotebookEdit`. MCP tools such as BrowserOS stay inherited. Every agent also disallows `Agent`, so the main session stays the single orchestrator.
- **Memory.** Every agent has `memory: project`. Its notes live in the approved repo under `.claude/agent-memory/<agent>/` (§13). Its body says:
  - check memory before starting;
  - afterwards, save project-specific learnings with their *why*;
  - fix or delete wrong notes;
  - never save personal details or secrets.
- **Security work** routes to the existing `claude-security` plugin. No new agent.

### 6. Knowledge (`plugins/anthology/knowledge/`)

Knowledge folders keep the normal skill layout (`SKILL.md` plus `references/` etc.), but they live outside `skills/`, so Claude Code doesn't register them as skills.

**Every folder is generated, never hand-edited.** `vendor/sources.json` pins each source:
- repo and path;
- commit (or checksum, for ui-taste);
- SPDX license;
- target folder;
- transform.

`scripts/vendor.py` fetches the pinned upstream from **git** (never from a plugin cache), applies the transform and/or `vendor/patches/<name>.patch`, and writes the target. `vendor.py check` regenerates everything into a temp dir and diffs it against the committed tree, and CI runs that check on every PR.

| Folder(s) | Source | Transform |
|---|---|---|
| architecture, system-design, code-review → **code-review-checklist**, testing-strategy, tech-debt, documentation, deploy-checklist | `anthropics/knowledge-work-plugins` (engineering) | `strip-connectors` (scripted removal of connector sections and `~~` blocks) + rename |
| design-handoff, research-synthesis | same repo (design) | `strip-connectors` |
| write-spec, sprint-planning, roadmap-update, competitive-brief, synthesize-research, product-brainstorming | same repo (product-management) | `strip-connectors` |
| seo-audit, campaign-plan, content-creation, email-sequence | same repo (marketing) | `strip-connectors` |
| landing-page, web-review, web-design-system | `uxkin/agent` (website-design) | `strip-connectors` |
| frontend-design | `anthropics/skills` | none (unchanged) |
| web-design-guidelines | `vercel-labs/agent-skills` | none. It downloads its rulebook through WebFetch at runtime, which is allowed. |
| ui-taste | uizze.sh `/.well-known/skills/ui-taste/` (no git; pristine snapshot + checksums kept in `vendor/snapshots/`) | patch: paid Uizze MCP sections and `find_ui_references`/`find_ui_materials` removed |
| design-taste-frontend | `leonxlnx/taste-skill` | patch: ported to **BrowserOS Neo** (below) |
| caveman | `juliusbrussee/caveman` (`skills/caveman` only) | patch: **one fixed style** (below) |
| design-skills | `cuellarfr/design-skills` | none (whole repo, unchanged) |

**General rules:**
- `strip-connectors` removes only connector setup sections and `If ~~<category> is connected` blocks.
- `code-review` is renamed `code-review-checklist` to avoid confusion with Claude Code's built-in `/code-review`.
- No credit text and no comments inside bodies; credits are the owner's job (§11).

**design-taste-frontend → BrowserOS Neo.** Upstream, its scripts launch headless Chrome on the host (temporary profile, `--remote-debugging-port`), and its frontmatter declares a hook that runs `gate.mjs`. The patch:
- removes the frontmatter hook;
- for each **Chrome-launching script** (`shoot`, `perf`, `effects`, `icons`, `moments`, `consistency`, the browser parts of `gate`, the `assets/*/check.mjs` and `make-posters` helpers), either:
  - ports it, so its in-page logic runs through BrowserOS Neo (`navigate`, `screenshot`, `evaluate_in_page`/`run`) against the page served on localhost, or
  - removes it and rewrites the SKILL.md step as a BrowserOS Neo step;
- keeps the **Chrome-free scripts** (`start`, `copy-lint`, `deal`, `pick`, `photos`, …), which run with the host's Node (v22+);
- moves the screenshots at three widths and the visual rubric to BrowserOS Neo.

The **port map is machine-readable** (`vendor/patches/design-taste-frontend.portmap.json`: script → keep / port / drop). A test fails when upstream adds a script that isn't in the map, so a new Chrome-launching script can't slip in on sync. After the patch, no file in the folder may contain a Chrome launch: no `remote-debugging`, `user-data-dir`, Chrome binary paths, `CHROME_PATH`, `playwright` or `puppeteer`.

**caveman → one fixed style.**
- **Kept:** the *Why*, *Rules*, *When to break the rules* and *Pre-send check* sections.
- **Dropped:** the *Persistence* toggles, the `/caveman ultra|wenyan|status` aliases, the trigger phrases, and the ultracave/megacave levels. Upstream hooks aren't copied.
- **New description:** "the anthology house style for every agent's chat and reports".
- **Unaffected:** files, code, commits and docs stay in normal prose, per caveman's own rule 5.

**Dropped:** engineering debug/standup/incident-response; design's other 5 (covered by design-skills); PM metrics-review/stakeholder-update; marketing draft-content/performance-report/competitive-brief.

### 7. Routing guide (delivered at session start)

`templates/routing-guide.md` is injected into the **main session only** by the `SessionStart` hook (`session_start.py`), as `additionalContext`. So it:
- updates automatically with the plugin;
- disappears on uninstall;
- never loads into subagents (no cost or conflicting "delegate" rules for agents);
- can be covered by the behaviour evals.

The total hook output, meaning the guide plus the personal memory index plus any repo-memory cover, stays under the hook-output size limit. A test enforces a budget below that limit (the exact limit is checked as U8).

Sections:
1. **Model roles**, plus the "do small tasks inline" rule.
2. **Which agent for which job**: the roster as a lookup table, and security → `claude-security`. For UI work, ui-designer builds and then ui-reviewer checks.
3. **Blocked agents**: subagents can't ask the user. A blocked agent stops and reports the question. The main session answers or asks the user, then resumes the same agent.
4. **Worktrees**:
   - Only built-in isolation; never `git worktree add`. Setup sets `worktree.baseRef: "head"` (U2), so agents branch from the lead's current commit rather than from origin's default branch.
   - Agent memory written inside a worktree is untracked there (U7); writers commit it as `chore(memory): …` before finishing.
   - Writers commit before finishing.
   - Parallel writers get disjoint file areas.
   - The main session reviews and merges.
5. **Docker**:
   - Tests, builds and E2E test suites run in containers mounting only the current worktree.
   - No host-level installs (`brew`, `npm -g`, global `pip`, Playwright/Puppeteer browser downloads).
   - Project-level installs (`npm install` etc.) are allowed.
   - Docker unavailable → stop and tell the user.
6. **Web**:
   - `WebFetch`/`WebSearch` are fine for reading.
   - **All real-browser work goes through BrowserOS Neo (MCP). It is the only browser.** Each agent uses its own tab.
   - Never use agent-browser, Playwright/Puppeteer on the host, chrome-devtools MCP, or Claude in Chrome. Never launch Chrome on the host, directly or through a script.
   - BrowserOS Neo unavailable → stop and ask. Never install a browser.
7. **Command output (rtk)**: the fork's awareness text.
   - Output is condensed; treat it as complete.
   - Exact text stays exact: `cat`, `grep -o/-c`, `jq -r`, `curl -o/-w`.
   - Use `rtk bypass <cmd>` only when a result is unusable.
8. **Memory**:
   - **What goes where:** `project`, `feedback` and `reference` notes go to the repo's `.claude/memory/` (approved repos only). `user` notes go to personal memory. Personal memory's index is injected by the same hook.
   - **Naming:** notes are named `<type>-<topic-slug>.md`. Link related notes with `[[note-name]]`; links are the graph.
   - **Search:** use `anthology-mem search|related` when the index doesn't answer.
   - **Self-learning:** save corrections and confirmed approaches with their *why*; fix or delete wrong notes. Commit memory changes as separate `chore(memory): …` commits.
   - **Never save secrets.** Repo memory is visible to everyone who can see the repo.

### 8. Browser

**BrowserOS Neo is compulsory and the only browser provider.**

| | BrowserOS Neo |
|---|---|
| Interface | MCP tools (`mcp__browseros-neo__*`) |
| Logins | User's real signed-in accounts (agent profile) |
| Parallel | Shared browser, per-agent tab groups |
| Use for | All browser work: logged-in work, forms, dashboards, local dev-server checks, UI review, design checks, human help |
| Install | User (agents never install it) |

Automated E2E suites that a project already has (e.g. Playwright tests) still run, but only inside Docker (§7.5), never on the host.

### 9. Guardrails (`anthology-guardrails`)

There are **two layers**:

**Layer 1: native deny rules.** `/anthology:setup` adds `permissions.deny` `Read(...)` rules for the Chrome/Chromium profile roots (macOS and Linux) to `~/.claude/settings.json`, shown as a diff first. These cover the built-in file tools and the Bash file commands Claude Code recognises. A hook can't override them. `/anthology:setup remove` takes them out.

**Layer 2: a `PreToolUse` hook** (matcher `*`, exec form, Python 3 stdlib) runs `guard.py`. It blocks with exit code 2 and a message naming the alternative.

| ID | Blocks | Exempt |
|---|---|---|
| G1 | Host browser-automation installs/runs: `playwright install\|test\|codegen\|open`, puppeteer installs, `pip install playwright`, `brew install chromium\|google-chrome\|playwright` | Shell segments whose program is `docker` / `docker-compose` |
| G2 | Launching Chrome/Chromium on the host: `Google Chrome.app`, `open -a "Google Chrome"`, `open -a Chromium`, `google-chrome*`, `chromium*`, any `--remote-debugging-port` | Docker segments; `open -a BrowserOS` is allowed |
| G3 | Any tool touching a Chrome/Chromium profile directory **or its parent** (`…/Application Support/Google`, `…/Application Support/Chromium`, `~/.config/google-chrome`, `~/.config/chromium`), with `~`/`$HOME`/absolute paths, case-insensitive | — |
| G4 | **Any agent-browser command**: `agent-browser …`, `npx agent-browser …`, installs (`npm i -g agent-browser`, `brew install agent-browser`) | — |
| G5 | MCP tools whose names contain `playwright`, `puppeteer`, `chrome-devtools`, or `claude-in-chrome` | — |
| G6 | Secret-looking content written into `.claude/memory/` or `.claude/agent-memory/`, via `Write`/`Edit` **or** Bash redirection/`tee`. Patterns: `AKIA…`, `ghp_`/`github_pat_`, `sk-…`, `xox[bp]-`, `-----BEGIN … PRIVATE KEY-----`, `password\s*[:=]`, `api[_-]?key\s*[:=]` | — |

- Command parsing strips `rtk` / `rtk bypass` / `rtk proxy` prefixes the same way as `sudo`, so `rtk npx playwright install` is still G1.
- When the guard blocks a call, the block wins over an rtk rewrite/allow (checked as U6).
- Rules match **commands**, not arbitrary text. `grep playwright package.json`, `grep -r agent-browser .` and `git commit -m "drop playwright"` are allowed. Every block message points to BrowserOS Neo (or Docker, for test suites).

**Never silently off.** The `SessionStart` entry is a POSIX `sh` wrapper (`session-start.sh`) because a hook whose interpreter is missing fails silently (U8): if `python3` is missing it prints a JSON `systemMessage` itself; otherwise it execs `session_start.py`, which runs `guard.py` on a known-bad sample. If the guard can't run (e.g. `python3` is missing) or doesn't block, the user gets a loud `systemMessage`. The guard still fails open on malformed *input*, so it never bricks a session, but a broken guard is always visible.

**Known limit:** a hook can't see a Chrome launch hidden inside a script. That's why the shipped knowledge itself must never launch Chrome (§6), and why the deny layer exists.

### 10. Owner's machine setup

The owner's machine-specific details are kept in private notes, not in this public repo. The generic steps:

- Uninstall the official `superpowers` and `design-skills` (avoids duplicate instructions and loose design skills).
- `marketplace add ~/Desktop/anthology`, then install `anthology`. The guardrails and anthology-superpowers come in as dependencies.
- Run `/anthology:setup`. It does three things, each shown as a diff first:
  - downloads and verifies rtk;
  - adds `Bash(rtk *)` and the Chrome deny rules to `~/.claude/settings.json`;
  - approves the current repo for memory.
- Turn on auto-update for the `anthology` marketplace (third-party marketplaces default to off).
- Worktrees: built-in default (`worktree.baseRef: "fresh"`), so no setting change.
- Add `.claude/worktrees/` to `~/.config/git/ignore`. Git reads this file by default, and it doesn't exist yet.
- After verification, uninstall the connector-heavy plugins whose skills anthology replaces.
- Make sure BrowserOS Neo's MCP endpoint listens on loopback only.
- agent-browser is not installed and should stay that way.
- Archive the owner's old fork of rtk, or mark it non-canonical. From now on the source of truth is `anthology/rtk/`.

### 11. Publishing

- The GitHub repo `rbonweb/anthology` is created **public** at the start of the build (the owner's decision: free CI minutes, open from day one).
- The repo ships with its own `LICENSE` (Apache-2.0) from the start.
- **The owner adds third-party credits after the build.** Because the repo is public from day one, each source's own license text is shipped unchanged in `vendor/licenses/<source>.txt` from its first commit. MIT and Apache-2.0 require this for redistribution. It is license compliance, not credit prose.
- Before the switch:
  - the owner's credits are in place;
  - PLAN.md, `.claude/memory/` and `.claude/agent-memory/` are reviewed (they become public);
  - secret/personal-data scan;
  - fresh-install test.
- The first public release is `1.0.0` for each plugin, tagged per §14. Internal dependency ranges move to `^1.0`.
- rtk binaries come from the public GitHub releases over plain HTTPS.

### 12. rtk (token-saving command output)

**What:** the owner's fork `rbonweb/rtk` of `rtk-ai/rtk` (Rust, Apache-2.0). It's a CLI proxy that condenses common dev-command output, wired into Claude Code as a `PreToolUse` hook (`rtk hook claude`) that rewrites Bash commands. The fork adds 9 commits:
- auto-allow hook
- minimal read by default
- node/python filters
- `cd` chain rewrite
- biome lint fix
- an `opencode run` output filter
- custom upstream HTTP headers
- "what stays exact" awareness text

**Source lives in anthology (`rtk/`), with real history.**
- **Import:** a non-squash `git subtree add --prefix=rtk` of the fork, then a `git subtree pull` merge of upstream's latest release tag (v1.2.1 at planning time).
- **Why history matters:** the histories share ancestry, so every future sync is a true merge with a merge base.
- **Recorded pins:** `vendor/sources.json` records the upstream tag/commit and the fork commit.
- **`scripts/sync-rtk.sh <tag>`** runs: subtree pull, then `rtk-trim.sh`, then the tests, and leaves a branch for review.

**Claude-only trim is a script (`scripts/rtk-trim.sh`)**, idempotent and re-run after every sync, so upstream additions can't quietly reintroduce what we removed:
- **Removes:**
  - every non-Claude AI-tool integration: `src/hooks/init/*` except `claude.rs` and shared helpers, `hooks/<other hosts>/`, `openclaw/`, and the Copilot/Gemini/VS Code handlers in `hook_cmd.rs`;
  - the translated READMEs, `Formula/`, `install.sh`, `.release-please*`, and the fork's `.claude/` dev tooling;
  - telemetry and the `rtk telemetry` command.
- **Keeps:** every **command** filter, including `opencode run`.
- **Decided by an investigator in Task 9:** keep or drop `engine/`, `proxy/` and `plugin/`.
- **Tests:**
  - `test_rtk_claude_only` is an **allowlist**: the exact set of `hooks/init` modules and the permitted Cargo dependencies, so anything new fails.
  - A `--network none` runtime test proves rtk makes no network calls.
  - **Each of the fork's 9 changes keeps one named test, permanently in CI.**

**Build and ship:**
- **Tests:** Docker with a pinned Rust toolchain (`rtk/rust-toolchain.toml`).
- **Binaries:** GitHub Actions (`rtk-release.yml`, on tag `rtk--v<version>`) builds `aarch64`/`x86_64-apple-darwin` and `x86_64`/`aarch64-unknown-linux-musl`, publishes them with `checksums.txt`, and adds **build-provenance attestations**.
- **Pin:** `plugins/anthology/rtk.lock` pins the version and the sha256 per target.

**Wiring:**
- **Hook:** `hooks/hooks.json` `PreToolUse` (matcher `Bash`) runs the installed binary as `rtk hook claude`. If the binary is missing, it exits 0.
- **Install:** binaries are installed per version in `${CLAUDE_PLUGIN_DATA}/rtk/<version>/rtk`, so a session still on the old plugin version keeps a matching binary.
- **Path lookup:** `session_start.py` (which receives `CLAUDE_PLUGIN_DATA`) writes the resolved binary path to `~/.claude/anthology/state.json`, and `bin/rtk` reads it. Nothing hard-codes the data dir.
- **Reconcile:** at every session start, if `rtk.lock` differs from what's installed, it downloads and verifies the pinned binary (with a timeout); on failure, it shows a warning.
- **No `rtk init`:** it isn't used, because it would add a duplicate hook. Defaults are preconfigured, and there are no user options.

**Permissions — accepted risk (owner's decision):**
- Setup adds a blanket `Bash(rtk *)` allow, so rewritten commands never prompt.
- Because `rtk bypass <cmd>` runs any command, this also auto-approves `rtk bypass <anything>`.
- The guardrails and deny rules still apply to the wrapped command.
- The README states the risk plainly, and `/anthology:setup remove` takes the allow out again.

### 13. Memory (in the repo, survives re-clone, team-safe)

**Principle: the notes are the books; SQLite is the library catalog.**
- Markdown notes in git are the only source of truth: readable, reviewable, mergeable by 4–5 people, and independent of any tool or database format.
- Everything else (the `MEMORY.md` indexes and the SQLite graph/search index) is **derived** from the notes and can always be rebuilt.
- A database as the source of truth was rejected: git can't merge a binary file, so concurrent teammates would lose writes.

| Whose memory | Where | Mechanism |
|---|---|---|
| Main session | `<repo>/.claude/memory/` (committed) | Claude Code's built-in auto memory. `autoMemoryDirectory` must be absolute, so `memory_sync.py` writes this clone's path into `<repo>/.claude/settings.local.json` (per-clone, git-ignored). |
| Each roster agent | `<repo>/.claude/agent-memory/<agent>/` (committed) | Built-in subagent `memory: project` |
| Personal (`user` notes) | `~/.claude/anthology/personal-memory/` (never in a repo) | Its index is injected by the `SessionStart` hook. It isn't tied to a folder path, so it survives re-clones. |

**Approved repos only (trust boundary).**
- Memory is active only in repos the user approved by running `/anthology:setup` inside them.
- Approvals live in `~/.claude/anthology/approved-repos.json`, keyed by the normalized git remote URL (the repo root path if there's no remote). So **re-cloning the same repo into any folder works automatically**.
- In an unapproved repo, nothing is written and nothing is injected. If it contains `.claude/memory/`, the user gets one line: "this repo has anthology memory; run /anthology:setup to use it". A stranger's committed notes never load on their own.

**`memory_sync.py`** (run by `session_start.py`; stdlib; never fails a session):
- **Skip** if the folder isn't an approved git repo, or if auto memory is off (`autoMemoryEnabled: false` / `CLAUDE_CODE_DISABLE_AUTO_MEMORY`).
- **Repair after a re-clone or move:**
  - set `autoMemoryDirectory` in `settings.local.json`, keeping its other keys;
  - make sure that file is git-ignored, through `git rev-parse --git-path info/exclude` (works in linked worktrees);
  - when it had to repair, inject the repo's memory index as same-session cover, because a settings change may only apply from the next session.
- **Derived indexes:** regenerate `.claude/memory/MEMORY.md` and each `.claude/agent-memory/*/MEMORY.md` from the notes' frontmatter (`name`, `description`, `metadata.type`), sorted and grouped by type, within a budget below the 200-line / 25 KB load limit.
  - If a memory outgrows the budget, the index lists the most recently updated notes per type plus "N more — use `anthology-mem search`".
  - Notes past the cut are therefore never silently invisible.
  - It writes only when the content changed.
- **Team merges:** the `.gitattributes` lines `.claude/memory/MEMORY.md merge=union` and `.claude/agent-memory/*/MEMORY.md merge=union` keep merges from conflicting. The next session start regenerates the index, which cleans up anything union left behind.
  - These lines are added once, when the repo is approved.
  - GitHub's web merge button ignores merge drivers, so a PR may still show an index conflict there. Keeping both sides' lines, or merging locally, is always correct, since the index is regenerated anyway.
- **Note collisions:** two people editing the same note is a genuine conflict, resolved like code. Notes are named `<type>-<topic-slug>.md`, so accidental collisions are rare.
- **Migration:** if the old path-keyed folder `~/.claude/projects/<encoded path>/memory/` has notes, they're copied once:
  - `type: user` notes go to personal memory;
  - the rest go to the repo;
  - nothing is ever overwritten.

**SQLite graph and search index (`memory_index.py`, `bin/anthology-mem`):**
- **Where:** built from the repo's notes, agent notes and personal notes into `${CLAUDE_PLUGIN_DATA}/memory-index/<repo-key>.db`. It's never committed, carries a schema version, and is rebuilt on a version mismatch or when notes change (mtime/hash check at session start and before each query).
- **Schema:**
  - `nodes`: `name`, `type`, `scope` (repo/agent/personal), `file`, `description`, `updated`.
  - `edges`: `src`, `dst`, `kind`. `kind` is `links` for `[[note-name]]` links, or a typed relation such as `supersedes`/`depends_on` declared in note frontmatter `metadata.relations`.
  - `notes_fts`: an FTS5 full-text index over the note bodies.
- **Commands** (on the Bash PATH):
  - `anthology-mem search "<terms>"`
  - `anthology-mem related <note> --depth N`
  - `anthology-mem orphans` (notes nothing links to)
  - `anthology-mem stale` (superseded or old notes)
  - `anthology-mem rebuild`
- **No embeddings (§2).** If semantic search is ever wanted, vectors can be added to this same derived index without changing the notes.

**Self-learning:**
- The main session saves `feedback` whenever the user corrects it or confirms an approach that wasn't obvious, always with the *why*.
- Every agent checks its memory before working and saves learnings after.
- The lead's report step for each delegated task includes "anything worth remembering?".
- Wrong notes are fixed or marked `supersedes`, never stacked.
- Memory changes are committed separately as `chore(memory): …`, so they never ride along with unrelated work.

**Portable between machines: `/anthology:export` and `/anthology:import`** (`memory_portable.py`; `disable-model-invocation: true`)
- **Export** writes `anthology-memory-<date>.tar.gz` containing:
  - `manifest.json`, with the format version, the date and note counts (no machine names or paths);
  - `personal/`;
  - `repo/memory/` and `repo/agent-memory/` for the current repo.

  It never uploads anything.
- **Import** first shows a dry run: new notes, identical ones (skipped), and conflicts (kept side by side as `<name>.imported-<date>.md` for Claude to merge with the user). Indexes are regenerated afterwards, and nothing is overwritten silently.
- **Import safety:**
  - members are extracted with `tarfile`'s `data` filter (no absolute paths, `..`, links or devices);
  - only the three memory locations can be written;
  - there's a size limit;
  - secret-looking content is reported.

**Governance for teams.** The README recommends `CODEOWNERS` entries for `.claude/memory/` and `.claude/agent-memory/`, and CI lists memory diffs in each PR. That way a contributor's memory note is reviewed like code before it loads into everyone's sessions.

### 14. Long-term operations

**Versioning and releases.**
- Each plugin uses semver in `plugin.json`. A release is tagged `<plugin>--v<version>` with `claude plugin tag`, and `CHANGELOG.md` (Keep a Changelog) records it.
- **Why this matters:** a plugin's `version` is what moves installed users to new code.
- **CI fails when a plugin directory changed without a version bump.** The internal dependencies use version ranges.
- The README tells users to turn on auto-update for the marketplace.

**CI (`ci.yml`, on every PR and push to `main`):**
- `$TEST` and `$RTK_TEST`;
- `claude plugin validate --strict` for each plugin;
- `vendor.py check`, which proves vendored content is reproducible;
- the version-bump check;
- the per-agent startup-budget test;
- gitleaks;
- the memory-diff listing.

**Behaviour evals** (repo-root `evals/`, `claude plugin eval` with all three plugins loaded; `evals.yml` runs weekly, on demand, and when the upstream watcher sees a new Claude Code release), with a cost ceiling. The cases cover:
- routing (the main session delegates to the right `anthology:*` agent);
- knowledge-read order (each agent Reads its Startup files before any other tool);
- G1/G4/G6 blocks;
- BrowserOS-only browsing (a mocked `browseros-neo` MCP server, no Chrome launch);
- rtk rewrite;
- the routing guide's presence.

The one-time platform probes become permanent eval or smoke cases. This is how a model update or a Claude Code update that silently breaks the product gets noticed.

**Upstream watcher (`upstream-watch.yml`, weekly).**
- It compares every pin in `vendor/sources.json` (superpowers, the knowledge sources, taste-skill, caveman, design-skills, ui-taste's checksum, rtk upstream) and the latest Claude Code release against upstream.
- For each change, it opens an issue with the diff summary and the startup-budget delta.
- The sync itself runs through `vendor.py sync` / `sync-rtk.sh` on a branch, with CI and evals as the gate.

**Compatibility contract.**
- `plugin.json` declares `"engines": {"claude-code": ">=<lowest version verified in Task 1>"}`.
- `doctor.py` and `session_start.py` check the assumptions the design depends on, and say so loudly when one breaks:
  - `${CLAUDE_PLUGIN_DATA}` and the PATH wrappers;
  - BrowserOS tool names;
  - the agent-memory path.

**Update and uninstall paths.** The routing guide updates with the plugin. rtk is reconciled at session start. The memory index format is versioned. `/anthology:setup remove` takes out everything setup added outside the plugin:
- the settings rules;
- the repo approvals (optional; memory files stay in the repo);
- `~/.claude/anthology/state.json`.

**Pinned toolchain.**
- Docker images are pinned by digest.
- `tests/requirements.txt` pins hashes.
- rtk pins its toolchain in `rust-toolchain.toml`.
- GitHub Actions are pinned by commit SHA.
- Dependabot proposes the bumps, and CI and evals gate them.

**License metadata (not credits).** Every entry in `vendor/sources.json` has an SPDX `license`. A test requires it, and the build stops if a source isn't redistributable (anything other than MIT/Apache-2.0 needs the owner's decision). Credit text remains the owner's job.

**Open-source files:**
- `SECURITY.md`, with how to report a guardrail bypass, and what the `Bash(rtk *)` allow and the memory trust model mean.
- `CONTRIBUTING.md`, including the maintainer runbook: syncs, releases, evals and the startup budget.
- `DECISIONS.md`, where Part 3 of this plan moves at the docs task, so decisions outlive PLAN.md.

---

## Part 2 — Build

**Goal:** Build the `anthology` marketplace as designed in Part 1, install it on this machine, and prepare it for a long-lived open-source life.

**Architecture:**
- A git repo at `~/Desktop/anthology` that is a Claude Code marketplace with 3 plugins, plus `vendor/` (pins and patches) and `rtk/` (a git subtree with history).
- Generated content (`knowledge/`, `plugins/anthology-superpowers/`) is always reproducible from `vendor/`.
- Logic is Python 3 stdlib scripts plus the rtk binary.
- Structure is enforced by unit tests; behaviour by `claude plugin eval`.

**Tech stack:**
- Claude Code plugins/marketplace format and `claude plugin eval`.
- Python 3 stdlib for runtime scripts; `unittest` plus pinned test deps, run in pinned Docker.
- Rust/cargo, only inside Docker and GitHub Actions.
- Docker via OrbStack; Node on the host (for taste-skill's Chrome-free scripts); BrowserOS Neo MCP.
- `git` + `gh` (account `rbonweb`); GitHub Actions.

### Build team

The roster agents don't exist until Task 6 is built and Task 16 installs them, so the build uses `general-purpose` subagents with a **model override per role**. The main session (Opus) only orchestrates: it dispatches, answers blocked agents (asking the user when needed), merges, and reports. It doesn't write task code.

| Role | Model | Does |
|---|---|---|
| Lead (main session) | opus | Dispatch, answer questions, merge, keep the checkboxes and Part 3 current |
| Investigator | opus | Work that needs judgment: platform checks, the taste-skill port map, the rtk conflict plan and trim script, fallback decisions |
| Builder | sonnet | Implements a task's steps (TDD, commits) |
| Reviewer | opus | After every task: spec-compliance review, then code-quality review. Findings go back to the builder |
| Runner | haiku | Runs `$TEST`, `$RTK_TEST`, `vendor.py check`, gitleaks, `gh` checks, and lookups; reports output verbatim |

| Task | Investigator | Builder | Runner | Review focus |
|---|---|---|---|---|
| 1 Skeleton, CI baseline, platform checks | U1–U9 probes | Steps 1–4, 6–7 | `$TEST`, `gh` | Manifest shape, CI runs |
| 2 Vendor tooling + unchanged copies + anthology-superpowers | — | `vendor.py`, transforms | `vendor.py check` | Reproducible; rename complete |
| 3 Adapt 22 knowledge folders | — | 2 builders in parallel worktrees | `vendor.py check`, `$TEST` | Only connector blocks removed |
| 4 ui-taste + caveman | — | patches | `vendor.py check` | No MCP, no modes |
| 5 taste-skill → BrowserOS Neo | port map (Step 1) | patch | `$TEST`, `pgrep` check | Review Focus 8 |
| 6 Roster (11 agents) | — | agent files | `$TEST` | Every folder owned; startup budget |
| 7 Guardrails | — | TDD | `$TEST` | Review Focus 1–4 |
| 8 rtk import with history + upstream merge | conflict plan (Step 2) | subtree + merge | `$RTK_TEST` | Review Focus 9 |
| 9 rtk trim script | trim decisions (Step 1) | `rtk-trim.sh` + tests | `$RTK_TEST` | Allowlist; no network |
| 10 rtk release CI + pin | — | workflow, lock | `gh run watch` | Attestations; checksums |
| 11 rtk wiring | — | hook, wrappers, installer (TDD) | `$TEST` | Fail-open; per-version install; Review Focus 10 |
| 12 Memory | — | sync, index, export/import (TDD) | `$TEST`, git merge scenario | Review Focus 11, 12 |
| 13 Session start + setup/remove | — | `session_start.py`, guide, settings patcher | `$TEST` | Output budget; self-test; clean removal |
| 14 Long-term ops | — | release tooling, evals, workflows | `claude plugin eval`, `gh run watch` | Review Focus 13 |
| 15 Docs | — | README, SECURITY, CONTRIBUTING, CHANGELOG, DECISIONS, LICENSE | `$TEST`, gitleaks | No personal data; risks stated |
| 16 Install on this machine | evidence checks | — | `claude plugin list` | Done by the lead with the user, since it changes `~/.claude` |
| 17 Release readiness | — | — | final CI + evals | Lead only; 1.0.0 waits for the owner's credits |

**Order:**
- Tasks run sequentially (3, 4, 5 and 8 all edit `vendor/sources.json`).
- Task 6 needs 3–5.
- Tasks 9 → 10 → 11 follow 8, in that order.
- Task 12 needs 11 (`state.json`).
- Task 13 needs 6, 7, 11 and 12.
- Task 14 needs 13.
- Everything else is sequential.

### Global Constraints

- **Repo:** root `~/Desktop/anthology` (lowercase). GitHub `rbonweb/anthology`, created **public** (owner's decision). Because it's public from the first push, nothing personal or secret may ever be committed.
- **Names:**
  - no "Claude" in the product or repo name; the description may say "for Claude Code";
  - marketplace `anthology`;
  - plugins `anthology`, `anthology-guardrails`, `anthology-superpowers`.
- **Dependencies:** `anthology` declares `anthology-guardrails` and `anthology-superpowers` as dependencies, with version ranges.
- **Platforms:** macOS and Linux only. Windows is refused by setup and doctor.
- **No loose skills:** `plugins/anthology/skills/` contains only `setup`, `export` and `import` (all `disable-model-invocation: true`). Every knowledge folder is read by at least one agent.
- **No user-facing modes, levels or toggles.**
- **Generated, never hand-edited:** `knowledge/**` and `plugins/anthology-superpowers/**` must equal `vendor.py` output (`vendor.py check` in CI).
- **Models:** agent models use aliases only: `opus`, `sonnet`, `haiku`.
- **Runtime scripts:** Python 3 **stdlib only**, compatible with **Python 3.9+** (macOS ships 3.9 as `/usr/bin/python3`): use `from __future__ import annotations`, and no `match` or 3.10+ APIs. CI runs the unit tests on pinned Python 3.9 and 3.12 images. Test-only deps are pinned with hashes in `tests/requirements.txt`.
- **`$TEST`**, the canonical test command (image pinned by digest, recorded in Task 1):
  `docker run --rm -v "$PWD":/w -w /w python:3.12-slim@sha256:<digest> sh -c "pip install -q --require-hashes -r tests/requirements.txt && python -m unittest discover -s tests -v"`
- **`$RTK_TEST`**:
  `docker run --rm -v "$PWD/rtk":/w -v anthology-cargo:/usr/local/cargo/registry -w /w rust:<pinned>@sha256:<digest> cargo test --locked`
- **No host installs** of Playwright, Puppeteer, Chrome, Chromium, agent-browser or Rust. macOS rtk binaries come from GitHub Actions.
- **BrowserOS Neo is the only browser.** Nothing shipped recommends another browser or launches Chrome.
- **rtk:** Claude Code only, no telemetry.
- **Credits:** no credit text (no NOTICE/THIRD_PARTY, nothing in bodies); the owner adds credits after the build. `vendor/sources.json` records SPDX license ids as metadata. The repo's own `LICENSE` (Apache-2.0) is added in Task 15; rtk's `LICENSE` stays in `rtk/`.
- **No personal data in tracked files:** no `/Users/…` paths outside `rtk/` fixtures, no `.env`, no tokens.
- **Commits:**
  - every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`;
  - memory changes go in separate `chore(memory): …` commits;
  - every plugin change bumps that plugin's version.

### Review Focus

1. **False positives:** ordinary commands that merely mention a blocked word must be allowed: `grep playwright package.json`, `grep -r agent-browser .`, `git commit -m "drop puppeteer"`, `open -a BrowserOS`. Task 7.
2. **Wrapped or chained commands** must be blocked: `cd app && npx playwright install`, `CI=1 npx playwright install`, `bash -c "npx playwright install"`, `sudo npx …`, `cd x && agent-browser open y`, `rtk bypass npx playwright install`. Task 7.
3. **Docker exemption bypass:** `docker ps; npx playwright install` must still block the second segment, while `docker run --rm mcr.microsoft.com/playwright:v1.49.0 npx playwright test` is allowed. G4 never exempts Docker. Task 7.
4. **Chrome profile path variants:** `~`, `$HOME`, absolute, mixed case, Linux `.config` paths and parent folders. All must block, on every file tool and in Bash, and the native deny rules cover the same roots. Tasks 7 and 13.
5. **Duplicates:** if the official `superpowers` or `design-skills` is still installed, `/anthology:setup` must recommend uninstalling it. Task 13.
6. **One-plugin install:** installing `anthology` alone brings in the guardrails and anthology-superpowers. Task 16.
7. **Knowledge loading:** agents really Read their knowledge first, and the main session's skill list shows no knowledge. Tasks 1 (U1), 14 (evals) and 16.
8. **taste-skill never launches Chrome:** no Chrome launch strings, no frontmatter hook, the port map covers every upstream script, and no Chrome process appears during a real run. Tasks 5 and 16.
9. **rtk fork changes survive:** each of the 9 has a permanent named test, and `sync-rtk.sh` re-runs the trim. Tasks 8–9.
10. **Guardrails beat rtk:** a blocked command stays blocked even when rtk would rewrite or allow it. Tasks 1 (U6), 7 and 16.
11. **Memory survives a re-clone and a team merge:**
    - a fresh clone of an approved repo in a new folder sees its memory in the first session;
    - two branches that each add a note merge cleanly, and the regenerated index lists both;
    - import never overwrites.

    Tasks 12 and 16.
12. **Memory trust boundary:** an unapproved repo is never modified and its notes are never injected. Task 12.
13. **Upgrade path:** with an older installed version of each plugin, a new release reaches the user: the guide updates, rtk reconciles, the memory index rebuilds, and `remove` leaves nothing behind. Tasks 13, 14 and 16.

---

#### Task 1: Repo skeleton, CI baseline, platform checks, public GitHub repo

**Files:**
- Create: `.claude-plugin/marketplace.json`, `plugins/{anthology,anthology-guardrails}/.claude-plugin/plugin.json`, `.gitignore`, `.gitattributes`, `tests/_repo.py`, `tests/test_repo_layout.py`, `tests/requirements.txt`, `.github/workflows/ci.yml`, `.github/dependabot.yml`
- Modify: Part 3 of `PLAN.md`

**Interfaces:**
- `tests/_repo.py` produces:
  - `ROOT`, `KNOWLEDGE`;
  - `load_frontmatter(path)`;
  - `knowledge_path(ref)`, where `x` → `knowledge/x/SKILL.md` and `ds/x` → `knowledge/design-skills/skills/x/SKILL.md`;
  - `superpowers_skill_path(name)`.
- Part 3 of `PLAN.md` gets the results of U1–U9.

- [ ] **Step 1:** `git init -b main` in `~/Desktop/anthology` (`PLAN.md` already exists there). The `.gitignore` covers `.env*`, `__pycache__/`, `.DS_Store` and `.claude/worktrees/`.
- [ ] **Step 2: Pin the toolchain.**
  - Resolve and record the digests for `python:3.12-slim`, the Rust image and gitleaks.
  - Write `tests/requirements.txt` (PyYAML) with hashes.
  - Record `$TEST`/`$RTK_TEST` with the real digests in Part 3.
- [ ] **Step 3: Failing test** `test_marketplace`:
  - The marketplace `name == "anthology"`.
  - The plugin names are `{"anthology","anthology-guardrails","anthology-superpowers"}`.
  - Each `source` has a `plugin.json` with a matching name, a semver `version` and `license: Apache-2.0` (first-party).
  - `anthology` has dependencies on both others, with version ranges, and an `engines.claude-code` range.

  Run `$TEST` → FAIL.
- [ ] **Step 4:** Write the marketplace and the first-party manifests (`version` `0.1.0`, ranges `~0.1`, provisional `engines` `>=2.1.311`, updated after Step 5). Run `$TEST`: only the anthology-superpowers assertions fail; mark them `expectedFailure` until Task 2. Write `ci.yml`, which runs `$TEST` (more jobs are added by later tasks), with Actions pinned by SHA, and `dependabot.yml` (Actions, Docker, pip).
- [ ] **Step 5: Platform checks.** Use a throwaway agent, knowledge folder, skill and hooks, install the marketplace locally, and record each finding in Part 3. **Keep each probe as a future eval/smoke case (Task 14); don't just delete it.**
  - **U1: knowledge loading.**
    - `${CLAUDE_PLUGIN_ROOT}` in an agent body is substituted; the agent Reads `knowledge/_probe/SKILL.md` first and quotes a marker.
    - The main session's skill list doesn't include `_probe`.
    - `skills: [anthology-superpowers:x]` preloads cross-plugin.
    - *Fallback: a Glob for `**/anthology/knowledge/<x>/SKILL.md` under the plugins dir.*
  - **U2: agent fields.** A plugin agent honors `model`, `disallowedTools`, `skills`, `isolation: worktree` and `memory: project`. *Fallback for isolation: the routing guide passes `isolation: "worktree"` per call.*
  - **U3: hook reach.** A `PreToolUse` hook with matcher `*` sees Bash, file tools and MCP tools. Exit 2 plus stderr blocks the call and reaches the model. Exec-form hooks work, and `${CLAUDE_PLUGIN_ROOT}`/`${CLAUDE_PLUGIN_DATA}` expand.
  - **U4: CLI syntax.** The exact syntax for `claude plugin marketplace add|install|uninstall|update|list|validate|tag|eval`.
  - **U5: upstream pins.** For every source in Part 1 §6, plus rtk:
    - the fork head and upstream's latest release tag/commit (v1.2.1 at planning time);
    - superpowers 7.0.2 = `22f0b463e7a5e579904dd6211a520321035526f7`;
    - the knowledge-work-plugins commit matching upstream version 1.2.0;
    - ui-taste's `CHECKSUMS.sha256`;
    - each source's SPDX license.
  - **U6: hooks and PATH.**
    - When one `PreToolUse` hook exits 2 and another returns `updatedInput` + `permissionDecision: allow`, the call is blocked.
    - Plugin `bin/` is on the Bash PATH in the main session and in subagents.

    *Fallback if the block doesn't win: the rtk hook entry first runs `guard_rules.check`.*
  - **U7: memory.**
    - `autoMemoryDirectory` set in `settings.local.json` is honored; record whether it applies in the same session or from the next.
    - Record the exact `.claude/agent-memory/<dir>` for `anthology:<name>`, and whether it resolves to the worktree or the main clone.
    - `SessionStart` `additionalContext` reaches the main session and **not** subagents.

    *Fallbacks:*
    - *If `autoMemoryDirectory` is ignored, `memory_sync.py` mirrors the repo memory into the built-in path-keyed folder both ways at session start, with the repo as the source of truth.*
    - *If agent memory resolves to the main clone, agents don't write memory inside worktrees, and the lead commits it from the main clone.*
  - **U8: hook output and failures.** Record the `SessionStart` `additionalContext` size limit and what happens beyond it. Record what happens when a hook's interpreter is missing, and whether an `onFailure`-style option exists.
  - **U9: eval harness.** `claude plugin eval` runs one trivial case against the local plugin (`tool_used`/`tool_order` graders, a mocked MCP server), and the cost ceiling flag works.
- [ ] **Step 6:** Delete the probe files (their cases are noted in Part 3 for Task 14). Commit with `chore: marketplace skeleton, CI baseline, platform decisions`.
- [ ] **Step 7:** `gh repo create rbonweb/anthology --public --source . --push`. Verify `PUBLIC`, and verify the CI run is green.

#### Task 2: Vendor tooling, unchanged copies, anthology-superpowers

**Files:**
- Create: `vendor/sources.json`, `scripts/vendor.py`, `vendor/licenses/<source>.txt`, `plugins/anthology-superpowers/**`, `plugins/anthology/knowledge/{design-skills,frontend-design,web-design-guidelines}/**`
- Test: `tests/test_vendor.py`, `tests/test_repo_layout.py` (copies section)

**Interfaces:**
- `vendor/sources.json` has one entry per source: `name`, `repo` (or `url`), `path`, `ref`, `commit` (or `checksum`), `license` (SPDX), `target`, and `transform` (a list of named transforms and/or `patch: vendor/patches/<name>.patch`).
- `vendor.py` (stdlib + `git`):
  - `fetch NAME`: a shallow clone at the pinned commit into a cache dir. It also copies the source repo's license file, unchanged, to `vendor/licenses/<source>.txt` (one per upstream repo).
  - `build NAME [--out DIR]`: pristine → transforms → patch → target.
  - `check`: builds all into a temp dir and diffs against the committed tree; exits non-zero on any difference.
  - `sync NAME --ref REF`: re-pins, rebuilds, reports patch rejects for review, and never commits.
  - `diff NAME`: pristine vs shipped, used to regenerate a patch after an intentional change.
- Named transforms:
  - `rename-plugin(old,new)`: rewrites the `plugin.json` name and every `old:<skill>` reference, and sets the plugin's **own** version from `sources.json` (`plugin_version`, starting `0.1.0`). The upstream version stays recorded as `upstream_version`. anthology-superpowers is versioned independently, so the `~0.1` dependency range resolves and any transform or sync change gets its own bump. Its `license` stays the upstream one (MIT).
  - `strip-connectors` (Task 3).

- [ ] **Step 1: Failing tests:**
  - `sources.json` validates; every entry has a 40-hex commit or a checksum, plus an SPDX license in `{MIT, Apache-2.0}` (anything else fails, and the owner must decide).
  - `vendor.py check` passes on a fixture.
  - The `rename-plugin` transform on a fixture rewrites all `superpowers:` references and nothing else.
  - The generated `plugins/anthology-superpowers/.claude-plugin/plugin.json` has `name == "anthology-superpowers"`.
  - No file under it contains `superpowers:` without the `anthology-` prefix.
  - Every preload in `EXPECTED_ROSTER` resolves.
  - Every distinct upstream repo in `sources.json` has a non-empty `vendor/licenses/<source>.txt` (`vendor.py check` verifies it matches upstream).

  Run → FAIL.
- [ ] **Step 2:** Implement `vendor.py`. Add the entries for superpowers (`rename-plugin`), design-skills, frontend-design and web-design-guidelines (no transform), with the U5 pins.
- [ ] **Step 3:** Run `vendor.py build` for all four, then `vendor.py check` → no differences. Remove the Task 1 `expectedFailure` markers. Add a `vendor.py check` job to `ci.yml`. Run `$TEST` → PASS.
- [ ] **Step 4:** Commit with `feat: reproducible vendoring; anthology-superpowers; unchanged copies`.

#### Task 3: Adapt the 22 knowledge folders

**Files:**
- Modify: `vendor/sources.json` (22 entries), `scripts/vendor.py` (`strip-connectors`)
- Create: generated `plugins/anthology/knowledge/<name>/**` for the 22 names in Part 1 §6 (rows 1–5)
- Test: `tests/test_knowledge.py`

- [ ] **Step 1: Failing tests:**
  - **Fixture tests for `strip-connectors`:** a connector setup section and a `~~` block are removed; nothing else changes; running it twice gives the same result.
  - **`test_knowledge` (shared with Tasks 4–6):**
    - the top-level knowledge folders equal `EXPECTED_KNOWLEDGE` (the Task 4–5 ones are `expectedFailure` until they land);
    - each `SKILL.md` has `name == folder` and a `description`;
    - no adapted file contains `~~`, `CONNECTORS.md` or `uxkin.com/mcp`.

  Run → FAIL.
- [ ] **Step 2:** Implement `strip-connectors`. Add the 22 entries, fetched from **git** (`anthropics/knowledge-work-plugins` at the U5 commit, `uxkin/agent`), with the `code-review` → `code-review-checklist` rename as a transform. Build them.
- [ ] **Step 3:** Run `vendor.py check` and `$TEST` → PASS. Spot-check `vendor.py diff` for 3 folders: only connector removals (and the rename).
- [ ] **Step 4:** Commit with `feat: 22 connector-free knowledge folders (generated)`.

#### Task 4: ui-taste and caveman

**Files:**
- Create: `vendor/snapshots/ui-taste/**`, `vendor/patches/{ui-taste,caveman}.patch`, generated `knowledge/{ui-taste,caveman}/**`
- Test: `tests/test_knowledge.py`

- [ ] **Step 1: Failing tests:**
  - **ui-taste:**
    - no file contains `find_ui_references`, `find_ui_materials` or `Uizze MCP`;
    - the snapshot matches its recorded `CHECKSUMS.sha256`;
    - the license is `Apache-2.0`;
    - `scripts/verify-visual-evidence.mjs` contains no Chrome launch (or it has been ported).
  - **caveman:**
    - none of these appear: `stop caveman`, `normal mode`, `/caveman`, `ultracave`, `megacave`, `wenyan`;
    - the *Rules*, *When to break the rules* and *Pre-send check* headings remain.

  Run → FAIL.
- [ ] **Step 2: ui-taste.**
  - Download every file listed in `https://uizze.sh/.well-known/skills/index.json` for `ui-taste` and verify it against `CHECKSUMS.sha256` (stop on any mismatch).
  - Commit the pristine copy to `vendor/snapshots/ui-taste/`.
  - Make the paid-MCP removal a patch.
  - Add the entry (`checksum=`, `url=`).
- [ ] **Step 3: caveman.** Take `skills/caveman/SKILL.md` at the U5 commit and apply the Part 1 §6 changes as a patch. Copy no hooks and no other skills.
- [ ] **Step 4:** Run `vendor.py check` and `$TEST` → PASS. Commit with `feat: ui-taste (no paid MCP) and fixed caveman house style`.

#### Task 5: design-taste-frontend on BrowserOS Neo

**Files:**
- Create: `vendor/patches/design-taste-frontend.patch`, `vendor/patches/design-taste-frontend.portmap.json`, generated `knowledge/design-taste-frontend/**`
- Modify: Part 3 of `PLAN.md`
- Test: `tests/test_taste_browseros.py`

- [ ] **Step 1 (Investigator, Opus): port map.**
  - Read upstream at the U5 commit.
  - For every script and asset helper, decide **keep** (Chrome-free; host Node), **port** (in-page logic through BrowserOS Neo `evaluate_in_page`/`run` against the page on localhost; screenshots via `screenshot`), or **drop** (the step is rewritten as a BrowserOS Neo step).
  - Decide how the page is served (a host Node static server, or the project's dev server).
  - Probe BrowserOS Neo once: `evaluate_in_page` returns data, and `screenshot` captures three widths.
  - **If the finish gate or the three-width screenshots can't be done with BrowserOS Neo, stop and report to the user.**
  - Write `portmap.json`, and summarize it in Part 3.
- [ ] **Step 2: Failing test** `test_taste_browseros`:
  - no file matches `remote-debugging|user-data-dir|CHROME_PATH|Google Chrome|chrome\.exe|/usr/bin/(google-chrome|chromium)|playwright|puppeteer` (case-insensitive);
  - the frontmatter has no `hooks`;
  - `SKILL.md` mentions BrowserOS Neo;
  - **every script present in the pristine upstream appears in `portmap.json`.**

  Run → FAIL.
- [ ] **Step 3:** Write the patch per the port map. Build it; `vendor.py check` → clean.
- [ ] **Step 4:** Run `$TEST` → PASS. Run each **kept** script once with host Node on a scratch page in `/tmp`. `pgrep -fl "remote-debugging|Google Chrome --headless"` shows nothing new.
- [ ] **Step 5:** Commit with `feat: design-taste-frontend adapted to BrowserOS Neo`.

#### Task 6: Agent roster (11 agents)

**Files:**
- Create: `plugins/anthology/agents/{researcher,architect,implementer,debugger,reviewer,scout,docs-writer,ui-designer,ui-reviewer,product-planner,marketer}.md`
- Test: `tests/test_roster.py`

- [ ] **Step 1: Failing test** `test_roster`, with `EXPECTED_ROSTER` encoding Part 1 §5 exactly:
  - The file set matches; `name == stem`; `model` matches.
  - `"Agent" in disallowedTools` for all agents; no-edit agents also disallow `Edit`, `Write` and `NotebookEdit`.
  - Writers have `isolation: worktree`, unless U2 says otherwise.
  - Every agent has `memory: project`.
  - `skills` equals the expected `anthology-superpowers:` preloads.
  - *Startup* lists exactly caveman plus the expected knowledge paths, and each one exists.
  - **No loose knowledge:** the union of the agents' knowledge covers every folder in `EXPECTED_KNOWLEDGE` (all 10 design-skills).
  - No agent mentions `agent-browser`.
  - **Startup budget:** each agent's body + Startup files + preloads ≤ its budget. Set each budget at measured size + 15% and record it in the test, so growth is a deliberate change.
- [ ] **Step 2:** Write the 11 files. Each `description` says *when* to delegate. Each body is ≤ 60 lines, with these sections:
  - **Startup**, including the relative-path and `design-skills:<x>` rules.
  - **Role** · **Do** · **Don't**.
  - **Memory:** check before; save learnings with their *why*; fix or delete wrong notes; no personal data or secrets; commit memory separately.
  - **When blocked:** stop and return the question.
  - **Report:** Summary / Changes with commit SHA / Verification command + result / Open questions / Worth remembering.

  Additional rules:
  - Writers commit before finishing, run tests through Docker, and never `git worktree add`.
  - Browser-capable agents (ui-designer, ui-reviewer, researcher, marketer) get the Part 1 §7.6 web rule.
  - ui-designer gets the "which knowledge leads" rule.
- [ ] **Step 3:** Run `$TEST` → PASS. Commit with `feat: model-tiered agent roster with agent-only knowledge and project memory`.

#### Task 7: Guardrails

**Files:**
- Create: `plugins/anthology-guardrails/hooks/hooks.json`, `plugins/anthology-guardrails/scripts/guard_rules.py`, `plugins/anthology-guardrails/scripts/guard.py`
- Test: `tests/test_guard_rules.py`, `tests/test_guard_cli.py`

**Interfaces:**
- `guard_rules.check(tool_name, tool_input, home) -> (rule_id, message) | None`.
- `guard_rules.MESSAGES` is keyed `G1`–`G6`.
- `guard_rules.SELF_TEST_SAMPLE` is a known-bad payload that must block.
- `guard.py` reads stdin JSON. On a block it prints the message to stderr and exits 2; otherwise it exits 0; malformed input → exit 0.
- `hooks.json`: `PreToolUse`, matcher `*`, **exec form** with the `python3` interpreter and `${CLAUDE_PLUGIN_ROOT}/scripts/guard.py` as args.

- [ ] **Step 1: Failing tests**, table-driven with one `subTest` per row. Bash cases use `tool_input={"command": …}`; file tools use `file_path`/`path`/`pattern`; Write/Edit use `content`/`new_string`.
  - **Blocked** (expected rule):
    - **G1:** `npx playwright install` · `npx -y playwright install chromium` · `pnpm exec playwright test` · `npm i puppeteer` · `pip install playwright` · `brew install --cask chromium`
    - **G2:** `open -a "Google Chrome" https://x.dev` · `"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless` · `chromium --remote-debugging-port=9222` · `google-chrome-stable`
    - **G3:**
      - `cat ~/Library/Application\ Support/Google/Chrome/Default/Cookies`
      - Read `file_path` under `$HOME/Library/Application Support/Google/Chrome`
      - Grep `path` `~/.config/google-chrome`
      - mixed-case `~/library/application support/google/chrome`
      - **the parent folder:** `tar czf x.tgz ~/Library/Application\ Support/Google` · `find ~/Library/Application\ Support/Google -name Cookies`
    - **G4:** `agent-browser open http://localhost:3000` · `agent-browser --session qa1 snapshot` · `npx agent-browser open x` · `npm i -g agent-browser` · `brew install agent-browser` · `agent-browser install` · `docker run --rm img agent-browser open x`
    - **G5:** `mcp__playwright__browser_navigate` · `mcp__chrome-devtools__click` · `mcp__claude-in-chrome__navigate`
    - **G6:**
      - Write `.claude/memory/project-deploy.md` containing `token: ghp_abc123…`
      - Edit `.claude/agent-memory/x/MEMORY.md` whose `new_string` contains `-----BEGIN OPENSSH PRIVATE KEY-----`
      - Bash `echo "AKIAIOSFODNN7EXAMPLE" >> .claude/memory/reference-aws.md`
      - Bash `printf 'password: x' | tee .claude/agent-memory/r/n.md`
    - **Chains, wrappers and rtk prefixes:** `cd app && npx playwright install` · `CI=1 npx playwright install` · `bash -c "npx playwright install"` · `sudo npx playwright install` · `cd x && agent-browser open y` · `docker ps; npx playwright install` · `rtk npx playwright install` · `rtk bypass open -a "Google Chrome"` · `rtk proxy agent-browser open x`
  - **Allowed** (`None`):
    - `grep -r playwright package.json` · `grep -r agent-browser .` · `git commit -m "drop puppeteer and playwright install"`
    - `open -a BrowserOS` · `npm install` · `npm i puppeteer-core`
    - `docker run --rm mcr.microsoft.com/playwright:v1.49.0 npx playwright test` · `docker compose run e2e npx playwright test`
    - `rtk git status` · `rtk bypass git log`
    - Read `src/chrome-extension/manifest.json` · `mcp__browseros-neo__navigate` · `WebFetch`
    - Write `.claude/memory/feedback-tests.md` mentioning "password reset flow"
    - Write `src/config.ts` containing `ghp_` (G6 only covers memory folders)
  - `SELF_TEST_SAMPLE` blocks.

  Run `$TEST` → FAIL.
- [ ] **Step 2: Implement `check`.**
  - **Normalize:** expand `~`/`$HOME`/`${HOME}`, convert `\` → `/`, and lowercase for path matching.
  - **Split** on `;`, `&&`, `||`, `|`, `&` and newlines, respecting quotes (`shlex`).
  - **Strip prefixes:** `VAR=val`, `sudo`, `env`, `command`, `exec`, `time`, `nohup`, `rtk`, `rtk bypass`, `rtk proxy`.
  - **Recurse** into `bash|sh|zsh -c`.
  - **Docker exemption:** only G1 and G2.
  - **G4:** scan every token.
  - **G3 roots:** the profile dirs and their parents.
  - **G6:** for Write/Edit content, and for Bash redirections (`>`, `>>`) and `tee` targets under the memory folders, scan the content written by `echo`/`printf`/heredoc.
  - **Messages:**
    - G1 → "run it inside Docker (OrbStack); for browser work use BrowserOS Neo"
    - G2 → "never launch Chrome on the host; use BrowserOS Neo"
    - G3 → "Chrome profiles are off-limits"
    - G4 → "agent-browser is not used here; use the BrowserOS Neo MCP tools"
    - G5 → "use the BrowserOS Neo MCP tools"
    - G6 → "memory is committed to the repo; never store secrets there"
- [ ] **Step 3:** Run `$TEST` → PASS.
- [ ] **Step 4: CLI test.** Piping a G1 payload to `guard.py` exits 2 with the message; an allowed command exits 0; `not json` exits 0. Implement `guard.py` and the exec-form `hooks.json`. Run `$TEST` → PASS.
- [ ] **Step 5:** Commit with `feat: guardrails G1–G6`.

#### Task 8: rtk import with history and upstream merge

**Files:**
- Create: `rtk/**` (subtree), `scripts/sync-rtk.sh`
- Modify: `vendor/sources.json` (rtk entry: upstream tag/commit, fork commit, license), Part 3 of `PLAN.md`
- Test: `$RTK_TEST`, `tests/test_rtk_layout.py`

- [ ] **Step 1:** `git subtree add --prefix=rtk https://github.com/rbonweb/rtk <U5 fork commit>`, with no squash, so the full history comes in. List the 9 fork commits, and name a test for each (an existing one, or a new one written in Step 4).
- [ ] **Step 2 (Investigator, Opus): conflict plan.**
  - On a branch, `git subtree pull --prefix=rtk https://github.com/rtk-ai/rtk <U5 upstream tag>`.
  - For each conflict, decide how to keep both upstream's change and the fork's intent, and record it in Part 3. Likely files: `src/discover/registry.rs`, `src/discover/rules.rs`, `src/hooks/init/claude.rs`, `src/lib.rs`.
  - **If a fork change can't coexist with upstream, stop and report to the user.**
- [ ] **Step 3 (Builder):** Resolve per the plan and commit the merge. `$RTK_TEST` → PASS.
- [ ] **Step 4:** Add a permanent named test for each fork change (`tests/fork_<n>_<slug>.rs` or equivalent) → PASS.
- [ ] **Step 5:** Write `scripts/sync-rtk.sh <tag>`: a branch, the subtree pull, `rtk-trim.sh` (Task 9), `$RTK_TEST`, and a summary of conflicts. It never pushes or merges. `tests/test_rtk_layout.py` checks the `sources.json` rtk entry. Add a `$RTK_TEST` job to `ci.yml`.
- [ ] **Step 6:** Commit (the subtree history plus the merge commit) with `feat: import rtk with history, merged with upstream <tag>`.

#### Task 9: rtk Claude-only trim (scripted)

**Files:**
- Create: `scripts/rtk-trim.sh`
- Modify: files under `rtk/` (via the script only); Part 3 of `PLAN.md`
- Test: `$RTK_TEST`, `tests/test_rtk_claude_only.py`

- [ ] **Step 1 (Investigator, Opus): trim decisions.**
  - The exact keep-list for `src/hooks/init/` (`claude.rs` plus shared helpers).
  - The edits to module registration and to `hook_cmd.rs`.
  - Which files to remove: translated READMEs, `Formula/`, `install.sh`, `.release-please*`, the fork's `.claude/`, `openclaw/`, `hooks/<non-claude>/`, and telemetry.
  - Whether to keep `engine/`, `proxy/` and `plugin/`.
  - The permitted Cargo dependency set.

  Record all of it in Part 3.
- [ ] **Step 2: Failing tests:**
  - **`test_rtk_claude_only` (an allowlist):**
    - the `src/hooks/init/*.rs` set equals the keep-list;
    - the Cargo dependency set equals the permitted set;
    - no `telemetry` module;
    - none of the removed paths exist.
  - **Network test:** a `--network none` Docker run of the release binary on sample commands (`rtk git status` in a fixture repo, `rtk rewrite "git log -5"`, `rtk hook claude` with a sample payload) succeeds with correct output.

  Run → FAIL.
- [ ] **Step 3:** Write `rtk-trim.sh`: idempotent, it fails loudly if an expected anchor is missing (so an upstream refactor is noticed), and running it twice changes nothing. Run it; `$RTK_TEST` → PASS; commit in small steps.
- [ ] **Step 4:** Run `sync-rtk.sh` against the same tag on a scratch branch → no changes. Run `$TEST` and `$RTK_TEST` → PASS.
- [ ] **Step 5:** Commit with `refactor(rtk): Claude Code only, no telemetry (scripted trim)`.

#### Task 10: rtk release CI and pin

**Files:**
- Create: `.github/workflows/rtk-release.yml`, `rtk/rust-toolchain.toml`, `plugins/anthology/rtk.lock`
- Modify: `rtk/Cargo.toml` version (`<upstream>-anthology.<n>`)
- Test: `tests/test_rtk_lock.py`

- [ ] **Step 1:** Write the workflow (Actions pinned by SHA). It runs on tag `rtk--v*` or by hand, with this matrix:
  - `aarch64-apple-darwin` and `x86_64-apple-darwin` (macOS runner);
  - `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.

  Each job runs `cargo test --locked` and then `cargo build --release --locked`. It publishes `rtk-<target>.tar.gz` plus `checksums.txt` to the tag's release, with **build-provenance attestations**.
- [ ] **Step 2:** Tag, then `gh run watch` until all 4 targets publish.
- [ ] **Step 3: Failing test** `test_rtk_lock`: the `version` equals `Cargo.toml`, plus `tag`, plus a 64-hex sha256 for each of the 4 targets. Write `rtk.lock` from `checksums.txt`. Verify one asset with `gh attestation verify` and a hash check. `$TEST` → PASS.
- [ ] **Step 4:** Commit with `ci(rtk): attested release binaries, pinned in rtk.lock`.

#### Task 11: rtk wiring in the plugin

**Files:**
- Create: `plugins/anthology/hooks/hooks.json`, `plugins/anthology/bin/rtk`, `plugins/anthology/scripts/rtk_install.py`, `plugins/anthology/scripts/state.py`
- Test: `tests/test_rtk_install.py`

**Interfaces:**
- **`state.py`:**
  - `state_path()` → `${CLAUDE_CONFIG_DIR:-~/.claude}/anthology/state.json`.
  - `read()`.
  - `write(**kv)`: atomic.
- **`rtk_install`:**
  - `target(system, machine)`: macOS/Linux only; raises otherwise, with a "Windows isn't supported" message.
  - `verify(path, sha256)`.
  - `install(data_dir, lock, fetch) -> Path`: writes `<data>/rtk/<version>/rtk` (mode 755) only after the hash matches; idempotent; prunes versions not used in 30 days.
  - `reconcile(data_dir, lock, fetch, timeout)`: installs when it's missing or mismatched, then calls `state.write(rtk_path=…)`.
  - CLI `--data-dir PATH [--dry-run]`, fetching over HTTPS from the public release.
- **`hooks.json`:** `PreToolUse` with matcher `Bash`, exec form: a resolver that reads `state.json` and execs `rtk hook claude`, or exits 0 if rtk is missing. Use the U6 fallback if needed.
- **`bin/rtk`:** a POSIX sh wrapper that reads `rtk_path` from `state.json` and execs it. If it's missing, it prints "run /anthology:setup" and exits 127.

- [ ] **Step 1: Failing tests:**
  - **targets:** darwin/arm64, darwin/x86_64, linux/x86_64 and linux/aarch64 map to the right targets; windows raises.
  - **install:** with a fake fetch, a good hash installs per version; a bad hash leaves nothing behind; a second run doesn't fetch.
  - **reconcile:** after a lock change, it installs the new version, keeps the old one and updates `state.json`; when the fetch fails, it returns a warning, not an exception.
  - **files:** `hooks.json` shape; `bin/rtk` is executable.

  Run `$TEST` → FAIL.
- [ ] **Step 2:** Implement (stdlib). Run `$TEST` → PASS.
- [ ] **Step 3:** Commit with `feat: wire rtk into the anthology plugin (versioned, reconciled)`.

#### Task 12: Memory (approved repos, derived indexes, SQLite graph, export/import)

**Files:**
- Create: `plugins/anthology/scripts/{memory_sync,memory_index,memory_portable}.py`, `plugins/anthology/bin/anthology-mem`, `plugins/anthology/skills/{export,import}/SKILL.md`
- Test: `tests/test_memory_sync.py`, `tests/test_memory_index.py`, `tests/test_memory_portable.py`

**Interfaces:**
- **`memory_sync.py`** (explicit paths; testable in temp git repos):
  - **Repo identity and approval:** `repo_key(root)` (normalized remote URL, else the path); `is_approved(key)`; `approve(root)`, which also runs `ensure_union_attrs`; `unapprove(key)`.
  - **Repairs:**
    - `ensure_memory_dir(root, settings_local) -> (dict, changed)`;
    - `ensure_ignored(root)`, via `git rev-parse --git-path info/exclude`;
    - `ensure_union_attrs(root)`.
  - **Indexes:** `regenerate_index(mem_dir, budget) -> changed`, built from the notes' frontmatter, grouped by type, newest first within the budget, with an "N more" pointer.
  - **Migration:** `migrate(old_dir, repo_mem, personal_mem)`.
  - **Entry point:** `run(cwd, env) -> (context_text, messages)`.
- **`memory_index.py`** (sqlite3 + FTS5):
  - `SCHEMA_VERSION`;
  - `build(sources, db_path)`, incremental by mtime/hash and fully rebuilt on a schema change;
  - `search(q)`; `related(name, depth)`; `orphans()`; `stale()`.
  - Edges come from `[[links]]` and from `metadata.relations` (`supersedes`, `depends_on`, `relates_to`).
- **`bin/anthology-mem`:** reads `data_dir` from `state.json`, works out the repo key, ensures the index is fresh, and runs the subcommand.
- **`memory_portable.py`:** `export`, `plan_import`, `apply_import` (as in Part 1 §13), and a CLI.

- [ ] **Step 1: Failing tests** (temp git repos, temp HOME):
  - **Trust boundary:**
    - in an unapproved repo with committed `.claude/memory/`, nothing is written or injected and one hint message is shown;
    - after `approve`, the repair happens.
  - **Re-clone:**
    - in an approved repo cloned into a new path (same remote), `settings.local.json` gets the new absolute path, the context includes the index, and the file is git-ignored;
    - in a linked worktree, `ensure_ignored` writes to the correct exclude file;
    - a second run is a no-op.
  - **Disabled:** with the env var or the setting, it's a no-op.
  - **Derived index:**
    - it's regenerated from frontmatter, deterministically;
    - 300 notes → the index stays within budget and contains the "N more" pointer;
    - an unchanged run writes nothing.
  - **Team merge:**
    - two branches each add a note: the merge succeeds with the union attributes and conflicts without them (the control);
    - after `regenerate_index`, both notes are listed exactly once.
  - **Migrate:** `type: user` notes go to personal memory and others to the repo; nothing is ever overwritten.
  - **Index:**
    - `search` finds a term in a note body;
    - `related` follows links and typed relations to depth 2;
    - `orphans` and `stale` work;
    - a schema-version bump triggers a full rebuild;
    - the DB lives in the data dir, never in the repo.
  - **Portable:**
    - export → import round trip into an empty HOME gives identical notes;
    - importing into an existing memory: identical notes are skipped, a conflict becomes `.imported-<date>.md`, and `--dry-run` writes nothing;
    - malicious archives (`../`, absolute paths, symlinks, outside the roots, over 50 MB) are rejected, with nothing written;
    - secret-looking content is reported.
  - **Skills:** `export`/`import` have `disable-model-invocation: true`.

  Run `$TEST` → FAIL.
- [ ] **Step 2:** Implement (stdlib; `tarfile` `filter="data"`). Apply the U7 fallbacks if they were needed.
- [ ] **Step 3: Write the two skills.**
  - `/anthology:export` writes the archive, reports the counts and the path, and reminds the user that it contains personal notes.
  - `/anthology:import <file>` does a dry run, waits for confirmation, applies, then merges each `.imported-*` with the user and deletes it once merged.
- [ ] **Step 4:** Run `$TEST` → PASS. Commit with `feat: repo memory with trust boundary, derived indexes, SQLite graph search, export/import`.

#### Task 13: Session start, routing guide, setup and remove

**Files:**
- Create: `plugins/anthology/templates/routing-guide.md`, `plugins/anthology/scripts/session-start.sh`, `plugins/anthology/scripts/{session_start,doctor,settings_patch}.py`, `plugins/anthology/skills/setup/SKILL.md`
- Modify: `plugins/anthology/hooks/hooks.json` (add `SessionStart`, exec form)
- Test: `tests/test_session_start.py`, `tests/test_doctor.py`, `tests/test_settings_patch.py`

**Interfaces:**
- **`session_start.run(env, stdin) -> stdout_json`.** In order:
  1. **Platform check:** on Windows, return one `systemMessage` and stop.
  2. **Guard self-test:** run `guard.py` on `SELF_TEST_SAMPLE`. If it doesn't block, or the interpreter is missing, show a loud `systemMessage`. Also warn if the U1 plugin-read allow rule no longer matches the parent of the current `${CLAUDE_PLUGIN_ROOT}` (re-run setup).
  3. **`rtk_install.reconcile`**, and write `state.json` (`data_dir`, `rtk_path`).
  4. **`memory_sync.run`.**
  5. **`additionalContext`** = the routing guide, then the personal memory index, then the repo-memory cover (only when it repaired). It's capped at the U8 budget, and lower-priority parts are truncated with pointers.
  6. **Never fails the session:** on any exception, exit 0 with a short `systemMessage`.
- **`doctor.check_env()`** checks `python3`, `node`, `docker`, `browseros`, `rtk` (hash matches the lock) and `platform_supported`. **`doctor.find_duplicates(installed)`** returns the official `superpowers` and any `design-skills` installs.
- **`settings_patch.apply(settings, ops) / revert(settings, ops) -> (new, diff)`.** Ops:
  - add/remove `Bash(rtk *)`;
  - add/remove the Chrome/Chromium `Read(...)` deny rules for the macOS/Linux profile roots and their parents;
  - add/remove the `Read(<parent of CLAUDE_PLUGIN_ROOT>/**)` allow rule (U1; covers cached versions and local in-place marketplaces);
  - set `worktree.baseRef` to `"head"`, remembering the previous value so `remove` restores it (U2);
  - always keep unrelated keys, byte-stable where possible.

- [ ] **Step 1: Failing tests:**
  - **Output:**
    - the output contains the guide; the total stays ≤ the budget even with a 300-note personal index (truncation pointer present);
    - no guide text is written to any `CLAUDE.md`;
    - an injected exception still exits 0.
  - **Guard self-test:** a broken guard (an interpreter path that doesn't exist) → the `systemMessage` warns. A stale U1 allow rule (root moved) → warning.
  - **Platform:** Windows → refusal message, nothing else.
  - **Settings:** `settings_patch` apply then revert = the original; apply twice = applied once.
  - **Doctor:** `find_duplicates` on a sample `installed_plugins.json`.
  - **Guide content:** the guide names BrowserOS Neo as the only browser and doesn't recommend agent-browser.
  - **Skills folder:** `skills/` = `{setup, export, import}`.

  Run `$TEST` → FAIL.
- [ ] **Step 2:** Implement. Write `routing-guide.md` with Part 1 §7 sections 1–8, within the budget, using `anthology:<agent>` IDs.
- [ ] **Step 3: Write `/anthology:setup`** (`disable-model-invocation: true`). It runs `doctor` and reports:
  - unsupported platform → stop;
  - missing Docker, BrowserOS Neo or Node → explain;
  - duplicates → the U4 uninstall commands.

  Then it shows every change as a diff and applies them only after the user confirms:
  - rtk download;
  - the `settings_patch` diff, with one line saying `Bash(rtk *)` also auto-approves `rtk bypass <cmd>`;
  - approving the current repo for memory;
  - creating the personal-memory folder.

  `/anthology:setup remove` reverts all of it: the settings ops, `state.json`, and optionally the repo approvals. Memory files are never deleted.
- [ ] **Step 4:** Run `$TEST` → PASS. Commit with `feat: session-start guide delivery, guard self-test, setup and remove`.

#### Task 14: Long-term operations (releases, evals, upstream watch)

**Files:**
- Create: `CHANGELOG.md`, `scripts/release.sh`, `scripts/check_version_bump.py`, `evals/**`, `.github/workflows/{evals,upstream-watch}.yml`
- Modify: `.github/workflows/ci.yml`
- Test: `tests/test_version_bump.py`, `tests/test_evals_layout.py`

- [ ] **Step 1: Version-bump check.** Failing test first: `check_version_bump.py <base> <head>` fails when files under `plugins/<p>/` changed without a `version` change in `plugins/<p>/.claude-plugin/plugin.json`, and passes otherwise. Implement it and add it to `ci.yml`, together with `claude plugin validate --strict` for each plugin (CI installs a pinned Claude Code ≥ 2.1.323, because older versions reject `engines` — U4), gitleaks (pinned), the memory-diff listing and the startup-budget test.
- [ ] **Step 2: Release script.**
  - `release.sh <plugin> <version>` bumps the version, moves the `CHANGELOG.md` *Unreleased* entries, commits, and runs `claude plugin tag`.
  - It never pushes without `--push`.
  - Dry-run it.
- [ ] **Step 3: Evals.**
  - **The cases** (repo-root `evals/`, each `case.yaml` listing `plugins: [plugins/anthology, plugins/anthology-guardrails, plugins/anthology-superpowers]` — U9):
    - routing, one case per agent: `tool_used: Agent` with `input_match` `anthology:<name>`;
    - knowledge-read order (`tool_order`: the Startup Reads come before any other tool);
    - G1/G4/G6 blocks;
    - BrowserOS-only browsing (a mocked `browseros-neo` MCP server, and no Bash Chrome launch);
    - rtk rewrite;
    - the routing guide's presence;
    - the converted U1–U9 probes.
  - **Run them locally** with the U9 syntax and a cost ceiling, and record the baseline in Part 3.
  - **Write `evals.yml`:** weekly, on demand, and on an upstream-watch trigger. It uses the `ANTHROPIC_API_KEY` repo secret (the owner adds it) and fails on regression versus the baseline.
- [ ] **Step 4: `upstream-watch.yml`.**
  - Weekly, for each `sources.json` entry, the rtk upstream and the latest Claude Code release, it compares the pinned version against upstream and opens one issue per change (or updates an existing one).
  - Each issue has a diff summary and the `vendor.py sync` / `sync-rtk.sh` command to run.
  - A new Claude Code release also triggers `evals.yml`.
  - Test the comparison logic with fixtures.
- [ ] **Step 5:** Run `$TEST` → PASS; CI green. Commit with `ci: release tooling, version-bump gate, behaviour evals, upstream watcher`.

#### Task 15: Docs

**Files:**
- Create: `README.md`, `SECURITY.md`, `CONTRIBUTING.md`, `DECISIONS.md`, `LICENSE` (Apache-2.0)
- Test: `tests/test_docs.py`

- [ ] **Step 1: Failing test** `test_docs`:
  - `LICENSE` starts with "Apache License";
  - no tracked file outside `rtk/` and the generated vendored trees (`plugins/anthology-superpowers/`, `plugins/anthology/knowledge/`, `vendor/snapshots/`) contains a real home path (regex `/Users/[A-Za-z0-9._-]+/` or `/home/[A-Za-z0-9._-]+/`), and none anywhere contains the owner's home path;
  - no `.env*` files;
  - `SECURITY.md` mentions `Bash(rtk *)` and the memory trust model;
  - `CONTRIBUTING.md` links `vendor.py`, `sync-rtk.sh`, `release.sh` and the evals.

  Run → FAIL.
- [ ] **Step 2: README sections.**
  - **What it is.** Opinionated and preconfigured: install it and follow its rules. macOS and Linux.
  - **Install.** `/plugin marketplace add rbonweb/anthology`, then `/plugin install anthology@anthology` (which brings the rest), **turn on auto-update for the marketplace**, then `/anthology:setup` in each repo you want memory in.
  - **The product:** the roster and knowledge; model roles; guardrails (two layers, and their limits); BrowserOS Neo as the only browser.
  - **rtk:** what it does, that it's always on and Claude-only, no telemetry, and **the `Bash(rtk *)` risk**.
  - **Memory:**
    - repo vs personal, and approved repos;
    - surviving re-clones; team merges; CODEOWNERS for memory;
    - `anthology-mem`; export/import;
    - **"repo memory is visible to everyone who can see the repo"**.
  - **Updating and removing:** `/anthology:setup remove`.
  - **Requirements:** Python 3, Node 22+, Docker, BrowserOS Neo.
  - **Replacing the official superpowers/design-skills.**
  - **Credits:** no credits section; the owner adds it.
- [ ] **Step 3: The other files.**
  - **`SECURITY.md`:** the reporting route, the threat model, the guardrail limits, the rtk allow, and memory trust.
  - **`CONTRIBUTING.md`:** setup for development; the maintainer runbook (syncing a vendored source, syncing rtk, releasing, evals and the baseline, adjusting startup budgets).
  - **`DECISIONS.md`:** Part 3 so far, as ADR-style entries.
- [ ] **Step 4:** Run `$TEST` → PASS; gitleaks clean. Commit with `docs: README, security, contributing, decisions, license`, then push.

#### Task 16: Install on this machine, verify, clean up

No repo files. This changes `~/.claude` only. The lead does this with the user.

- [ ] **Step 1:** Using the U4 syntax: uninstall `superpowers@anthropic-plugin-directory` and `design-skills@anthropic-plugin-directory`. Add the marketplace from `~/Desktop/anthology` and install **only** `anthology`. `claude plugin list` shows that `anthology-guardrails` and `anthology-superpowers` came in automatically. (If they didn't, install them explicitly and record it in Part 3 and the README.)
- [ ] **Step 2:** Run `/anthology:setup` inside `~/Desktop/anthology`, review the diffs, and confirm. Append `.claude/worktrees/` to `~/.config/git/ignore`, and verify with `git check-ignore -v .claude/worktrees/x`.
- [ ] **Step 3: Verify in a fresh session.** Each check needs evidence:
  - (a) the 11 `anthology:*` agents are listed;
  - (b) the main skill list has `anthology:setup/export/import` and anthology-superpowers, with **no knowledge**;
  - (c) `anthology:ui-designer` quotes the first headings of `design-taste-frontend` and `caveman`, and `anthology:reviewer` quotes `code-review-checklist`;
  - (d) an agent's report is in the house style;
  - (e) `npx playwright install` is blocked by G1, `agent-browser open http://localhost:3000` by G4, and `rtk bypass npx playwright install` by G1 (Review Focus 10);
  - (f) a small ui-designer task on a static page in `/tmp` uses BrowserOS Neo, and `pgrep -fl "remote-debugging|Google Chrome --headless"` stays empty;
  - (g) `/doctor` and `claude --debug` show no plugin or agent warnings, and `~/.claude/CLAUDE.md` is unchanged;
  - (h) `git status` from the main session and from `anthology:scout` runs as `rtk git status`; `cat` output is exact; the binary hash matches `rtk.lock`;
  - (i) the routing guide is present in the main session and absent from a subagent (ask scout to quote it: it can't);
  - (j) re-clone:
    - save a project fact in a scratch approved repo, commit it, and `git clone` it into another folder: the first session there knows the fact;
    - an agent note lands in `.claude/agent-memory/…`;
    - an unapproved clone of a different repo with `.claude/memory/` gets only the hint;
  - (k) `anthology-mem search` and `related` return the expected notes;
  - (l) `/anthology:export`, then `/anthology:import` into a temp HOME restores the personal notes; a second import reports everything identical;
  - (m) upgrade path: bump a plugin version locally and update; the guide change appears in the next session, and rtk reconciles.
- [ ] **Step 4: Migrate this project's existing memory** (from `~/.claude/projects/<this project's encoded path>/memory/`):
  - `anthology-machine-facts`, `anthology-user-workflow` and `open-decisions-as-questions` → personal memory;
  - `anthology-status` → `~/Desktop/anthology/.claude/memory/` (no personal details in it), committed as `chore(memory): …`;
  - confirm with the user.
- [ ] **Step 5:** After Step 3 passes:
  - uninstall `design`, `engineering`, `marketing`, `product-management` and `website-design` (`@anthropic-plugin-directory`);
  - delete leftover caches of plugins that were already uninstalled;
  - re-check (a)–(c), (e), (h) and (j).
- [ ] **Step 6:** Report to the user, including the manual items:
  - make sure BrowserOS listens on loopback only;
  - turn on marketplace auto-update;
  - add the `ANTHROPIC_API_KEY` repo secret for evals;
  - archive or mark the `rbonweb/rtk` fork.

#### Task 17: Release readiness (the repo is already public)

- [ ] **Step 1:** CI and evals are green on `main`; the gitleaks scan is clean.
- [ ] **Step 2:** Set the repo description ("Opus plans, Sonnet builds, Haiku scouts — a model-tiered agent team for Claude Code") and the topics (`claude-code`, `claude-code-plugin`, `agents`, `skills`).
- [ ] **Step 3:** Release the current `0.x` version of each plugin with `release.sh` (tags pushed).
- [ ] **Step 4:** Verify from a clean environment (a temp `HOME`/`CLAUDE_CONFIG_DIR`):
  - `/plugin marketplace add rbonweb/anthology` lists the 3 plugins;
  - installing `anthology` brings the other two;
  - `/anthology:setup` downloads rtk over plain HTTPS, and `gh attestation verify` passes on the downloaded asset.
- [ ] **Step 5:** `1.0.0` **waits for the owner**: they add the third-party credits and review `PLAN.md` and the repo's memory notes. Then `release.sh` bumps to `1.0.0`, with internal ranges `^1.0`. Report this as the one open item.

---

## Part 3 — Build decisions

Filled in during the build:
- the toolchain digests (Task 1);
- the platform checks U1–U9 (Task 1);
- the taste-skill port map summary (Task 5);
- the rtk conflict plan (Task 8);
- the rtk trim decisions (Task 9);
- the eval baseline (Task 14);
- any fallback taken.

Each entry gives the date, the task, what was found, and what was decided. These entries move to `DECISIONS.md` in Task 15 and are added there from then on.

- **2026-11-20, pre-build (lead):** the owner approved the build. Their instructions: the repo is public from creation, and no further questions; the lead decides. Rulings:
  - Personal machine details are removed from PLAN.md.
  - Upstream license texts ship in `vendor/licenses/`.
  - Task 17 = release readiness; 1.0.0 waits for the owner's credits.
  - Tasks run sequentially.
  - Work happens on branch `build`, merged to `main` after the final review; Task 1 bootstraps `main`.
  - `settings_patch` (Task 13) is the only editor of `~/.claude/settings.json`.
  - The `/Users/` docs check excludes the generated vendored trees.
  - If taste-skill can't be fully ported to BrowserOS, the unportable checks are dropped and the guidance kept.
  - If a fork change can't coexist with upstream, upstream's design wins and the fork's intent is re-implemented where feasible.
  - The evals workflow skips with a notice while the `ANTHROPIC_API_KEY` secret is missing.
- **2026-11-20, Task 1 (toolchain pins):** images resolved with `docker buildx imagetools inspect` to the multi-arch index digest, so one digest works on Apple Silicon and on x86_64 CI. Each is a one-line Dockerfile in `ci/images/` (`python.Dockerfile`, `rust.Dockerfile`, `gitleaks.Dockerfile`), the single source of truth: `ci.yml` and `$TEST` read the image from the `FROM` line, and a test checks that every `FROM` is pinned by `@sha256:`.
  - `python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b`
  - `rust:1.100-slim@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe` (Rust stable 1.100.0 at pin time)
  - `zricethezav/gitleaks:v8.30.1@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f`
  - `tests/requirements.txt` pins PyYAML 6.0.3 with hashes for the cp312 manylinux x86_64 and aarch64 wheels and the sdist; `pip install --require-hashes` was checked on both arm64 and (emulated) x86_64.
  - `ci.yml` pins `actions/checkout` to `3d3c42e5aac5ba805825da76410c181273ba90b1` (v7.0.1).
  - `$TEST` = `docker run --rm -v "$PWD":/w -w /w python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b sh -c "pip install -q --require-hashes -r tests/requirements.txt && python -m unittest discover -s tests -v"`
  - `$RTK_TEST` = `docker run --rm -v "$PWD/rtk":/w -v anthology-cargo:/usr/local/cargo/registry -w /w rust:1.100-slim@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe cargo test --locked`
  - `engines.claude-code` is provisionally `>=2.1.311` in the first-party manifests; it is updated after the platform checks (U1–U9).
  - Dependabot covers `github-actions`, `pip` (`/tests`) and `docker` (`/ci/images`), so digest bumps arrive as PRs (updated in fix round 1; the first version kept the digests in `tests/images.lock`, which Dependabot cannot read).
- **2026-11-20, Task 1 (platform checks U1–U9):** all probes ran on Claude Code **2.1.311** (Homebrew's stable cask; npm `latest` was 2.1.329). They used throwaway plugins under a scratch directory, loaded with `--plugin-dir` or installed from a local marketplace into an isolated `CLAUDE_CONFIG_DIR`, and ran `claude -p` in scratch git repos. Each probe is kept as a future eval/smoke case for Task 14, and each check below ends with a **→ Task 14 case** note. Eval cases use the repo-root `evals/` layout from U9. Smoke cases are scripts that run `claude -p` in a scratch repo with an isolated `CLAUDE_CONFIG_DIR`.
  - **Probe archive:** the raw probe sources (probe plugins, hook scripts, eval suites, commands, jsonl evidence) are archived in the lead's local, git-ignored workspace at `.superpowers/sdd/PLAN/probes/` (local only, not in the repo) until Task 14 converts them into `evals/` and smoke scripts. The notes below are enough to rebuild every case without them.
  - **U1 — knowledge loading: passes, with one required permission rule.**
    - `${CLAUDE_PLUGIN_ROOT}` in a plugin agent's body is substituted. The agent Read `<root>/knowledge/_probe/SKILL.md` and quoted its marker.
    - A folder under `knowledge/` is not registered: the session's skill list had `probe:hello` and `anthology-superpowers:x`, but no `_probe`.
    - `skills: [anthology-superpowers:x]` preloads a skill from a second plugin; the agent quoted its marker without a tool call.
    - **Finding:** Claude Code doesn't auto-approve Reads of a plugin's own files. In the default permission mode the knowledge Read is refused (in `-p`) or prompts (interactive). This held for `--plugin-dir`, for a local-path marketplace (loaded in place) and for a GitHub-marketplace plugin copied into `plugins/cache/`. Auto mode and an allow rule both let it through, and a session-level rule also covers subagents.
    - **Decision (lead ruling):** `/anthology:setup` (through `settings_patch`) adds one `permissions.allow` rule built from the **resolved** plugin location: `Read(<parent of ${CLAUDE_PLUGIN_ROOT}>/**)`. For a cached install that's `<config>/plugins/cache/anthology/anthology/**`, which covers every cached version. For a local marketplace loaded in place, such as a `marketplace add` of the repo folder, it's `<repo>/plugins/**`. A fixed `~/.claude/plugins/cache/anthology/**` would miss the in-place case. `setup remove` takes the rule out, and `session_start.py`/`doctor.py` warn when it no longer matches the current root. The Glob fallback isn't needed: the path is right, only the permission is missing.
    - **→ Task 14 case (eval `knowledge-load`):**
      - Prompt: "Run the anthology:<agent> agent with the prompt 'go' and report the first line of each file it read at startup."
      - `allowed_tools: [Agent, Read]`; `plugins` lists all three plugins.
      - Graders:
        - `tool_used` `Agent`, `input_match: 'anthology:<agent>'`;
        - `tool_used` `Read`, `input_match: 'knowledge/<first startup folder>/SKILL\.md'`;
        - `tool_order` before `{tool: Read, input_match: 'knowledge/.*/SKILL\.md'}`, after any non-Read tool the agent uses for its task;
        - `regex` on `last_message` for a marker phrase from that knowledge file;
        - for agents with preloads, `regex` for a phrase from the preloaded superpowers skill, with `tool_used Skill min 0 max 0`, proving it came from the preload and not an invocation.
      - Expected: score 1.00.
      - A smoke case `skills-hidden` runs `claude -p "hi" --max-turns 1 --output-format stream-json --verbose` with the plugins loaded. It asserts that the `system/init` event's `skills` contains no name matching `knowledge/` folder names, such as `caveman` or `frontend-design`, while `agents` contains every `anthology:*` agent.
  - **U2 — agent fields: all honored.**
    - `model: haiku`: the subagent transcript records a Haiku model.
    - `disallowedTools: Edit, Write, NotebookEdit, Agent`: those tools are absent from the agent's tool list.
    - `skills`: see U1.
    - `isolation: worktree`: the agent runs in `<repo>/.claude/worktrees/agent-<id>` on branch `worktree-agent-<id>`, and its commit lands on that branch.
    - `memory: project`: see U7.
    - **Finding — worktree base:** worktrees branch from the remote default branch (`worktree.baseRef: "fresh"`) by default, not from the session's `HEAD`. In a repo with an `origin`, an isolated agent didn't see the lead's unpushed commits on a feature branch; with `worktree.baseRef: "head"` it did.
    - **Decision:** `/anthology:setup` also sets `worktree.baseRef: "head"` in user settings (via `settings_patch`, and removed by `setup remove`), so writers start from the lead's current work, such as branch `build`. The routing guide states that writers branch from the lead's `HEAD`. The per-call `isolation` fallback isn't needed.
    - **→ Task 14 cases:**
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
  - **U3 — hook reach: passes.**
    - A `PreToolUse` hook with matcher `*` saw `Bash`, `Read`, `Write`, `ToolSearch` and a plugin MCP tool (`mcp__plugin_<plugin>_<server>__<tool>`), in the main session and in subagents. Inside a subagent, the hook input carries `agent_type: "<plugin>:<agent>"`.
    - Exit 2 with stderr blocks the call, and the model sees `PreToolUse:Bash hook error: [...]: <stderr>` plus the plugin that blocked it.
    - Exec form (`command` + `args`) works. `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` expand in `args` and are also in the hook's environment. The data dir is `<config>/plugins/data/<plugin>-<marketplace>` (`<plugin>-inline` for `--plugin-dir`).
    - **Decision:** none needed; the guard design (§9) stands. **Eval case:** a hook log shows every tool class, and the block text reaches the model.
  - **U4 — CLI syntax (from `--help` on 2.1.311):**
    - `claude plugin marketplace add <owner/repo|https-url|./path> [--scope user|project|local] [--sparse <paths…>]`. A `file://` URL is rejected.
    - `claude plugin marketplace list|remove <name>|update [name]`
    - `claude plugin install|uninstall|update <plugin>[@marketplace] [-s user|project|local]`. `uninstall` also takes `--keep-data`, and `--prune -y` removes auto-installed dependencies.
    - `claude plugin list [--json] [--available]`
    - `claude plugin validate <path> [--strict] [--json] [--profile directory]`
    - `claude plugin tag [path] [--dry-run] [-m <msg>] [--push] [--remote origin] [-f]`
    - `claude plugin eval [target] [--case <glob>] [--tag …] [--runs n] [--model m] [--judge-model m] [--ablation none|with-without] [--threshold 0..1] [--max-cost-usd usd] [--allow-tools …] [--mocks record|off] [--trust-plugin] [--json path] [--no-publish] [-j n]`
    - Also present: `claude plugin details|prune|enable|disable|configure|init`.
    - **→ Task 14 case (smoke `cli-syntax`, in CI with the pinned Claude Code):**
      - For each subcommand above, `claude plugin <sub> --help` exits 0, and its output contains every flag the plan's scripts and docs use. At minimum: `marketplace add`, `install -s`, `uninstall --keep-data`, `validate --strict`, `tag --dry-run`, `--push`, and `eval --max-cost-usd`, `--trust-plugin`, `--ablation`, `--no-publish`, `--json`.
      - `claude plugin validate --strict plugins/<each>` exits 0.
      - `claude plugin tag --dry-run plugins/anthology` prints `anthology--v<version>`.
      - Expected: all pass. A missing flag means a CLI change that the docs and scripts must follow.
  - **U5 — upstream pins** (resolved read-only with `gh api` / `git ls-remote`; SPDX from the repo license or the shipped LICENSE file):

    | Source | Path | Pin | SPDX |
    |---|---|---|---|
    | `obra/superpowers` | whole plugin | tag `v7.0.2` = `22f0b463e7a5e579904dd6211a520321035526f7` (as expected) | MIT |
    | `anthropics/knowledge-work-plugins` | `engineering/`, `design/`, `product-management/`, `marketing/` | `0e0fcacddcc892eb92a1b7873ff9d68159c47571` (`main` head; all four plugin.json at 1.2.0, the same commit the directory serves as 1.2.0) | Apache-2.0 |
    | `uxkin/agent` | `plugins/website-design/` | branch **`website-design`** = `07e8f5f61421c7268fddc1511878fb09945da854` (plugin 1.0.0). The folder is not on `main`. | MIT |
    | `anthropics/skills` | `skills/frontend-design/` | `e4bb5d960d87126ff3b3a8af1a104b7db67560ba` (`main` head) | Apache-2.0 (folder `LICENSE.txt`; the repo has no top-level license) |
    | `vercel-labs/agent-skills` | `skills/web-design-guidelines/` | `1a2c79d4e6c4a2283dc2c614b6538cbe1ed5ca62` (`main` head) | MIT (stated in README; the repo has no LICENSE file) |
    | `leonxlnx/taste-skill` | design-taste-frontend | tag `v2.5.1` = `f6b76575f949707b4e3c37cd86cb44b0b98643e8` | MIT |
    | `juliusbrussee/caveman` | `skills/caveman/` | tag `v4.1.0` = `1e91f1306d602d4ca23618701b1185575ae1869b` | Apache-2.0 |
    | `cuellarfr/design-skills` | whole repo | `7e25e66e87f64f9d3dea22198f9e29937e7c48f5` (`main` head = 1.0.0; no tags) | MIT |
    | ui-taste (uizze.sh) | `/.well-known/skills/ui-taste/` | `CHECKSUMS.sha256` sha256 `405765de761bfaeed98407e75bb6fe67cf9b6f0179562453554f2e9ddacd1612` (25 entries, manifest version `uizze-design-v14`) | Apache-2.0 (shipped LICENSE, matching its checksum line) |
    | `rbonweb/rtk` (fork) | whole repo | `develop` = `9b790c414de16f5fb688126b3945c73aa0e0218e` | Apache-2.0 |
    | `rtk-ai/rtk` (upstream) | whole repo | latest release `v1.2.1` = `d0f0399a5ff8cd1d81325a033f0b2d8864d665f6` (published 2026-11-19) | Apache-2.0 |

    - **Decisions:**
      - Every source is MIT or Apache-2.0, so no owner decision is needed.
      - Because `vercel-labs/agent-skills` has no LICENSE file, `vendor/licenses/` carries a standard MIT text naming "Vercel", with a note that the license is declared in the upstream README.
      - The upstream watcher tracks `uxkin/agent` on branch `website-design`, not `main`.
    - **→ Task 14 case (unit test, no model):** `test_sources_pins` asserts `vendor/sources.json` matches this table: repo, path, ref/commit (40-hex), checksum for ui-taste, and SPDX in {MIT, Apache-2.0}. `vendor.py check` (CI) proves the committed trees regenerate from those pins. `upstream-watch.yml` compares the same pins: `git ls-remote` per repo/branch/tag (`website-design` branch for uxkin), the ui-taste `CHECKSUMS.sha256` sha256, and the rtk upstream latest release. Expected: no diff on a clean tree, and an issue opened when a pin moves.
  - **U6 — hooks and PATH: the block wins.**
    - One plugin's `PreToolUse` hook exited 2 while another returned `permissionDecision: "allow"` with `updatedInput`. The call was blocked, with either plugin load order. This matches the documented precedence `deny > defer > ask > allow`.
    - Both hooks receive the **original** command (they run in parallel), so the guard always checks what Claude asked for, not rtk's rewrite. Alone, the allow hook's rewrite and auto-approval worked.
    - Plugin `bin/` is on the Bash `PATH` in the main session and in subagents.
    - **Decision:** none needed; the `guard_rules.check` fallback isn't needed. **Eval case:** the deny + allow pair blocks the call.
  - **U7 — memory.**
    - `autoMemoryDirectory` in `.claude/settings.local.json` is honored when it is present at session start. A note was written to `<repo>/.claude/memory/` along with `MEMORY.md`.
    - When a `SessionStart` hook writes the setting during startup, it applies **from the next session**. That session's notes went to the path-keyed `<config>/projects/<encoded path>/memory/`.
    - Per the docs, the project-scope setting is honored only in a trusted workspace, and not while `permissions.blockReadsOutsideWorkingDirectories` is on.
    - Agent memory for plugin agent `probe:writer` is `.claude/agent-memory/probe-writer/`: the `:` becomes `-`, so `anthology:<name>` maps to `.claude/agent-memory/anthology-<name>/`. With `isolation: worktree` it resolves **inside the worktree** (`<worktree>/.claude/agent-memory/…`), where it is untracked and lost with the worktree unless committed.
    - The `SessionStart` `additionalContext` reached the main session but **not** subagents: the marker is absent from subagent transcripts, and the agent reported none.
    - **Decisions:**
      - `memory_sync.py` keeps the planned same-session cover: when it repairs the setting, it injects the repo memory index.
      - The path-keyed migration runs on **every** session start, not only once, and copies (never overwrites) anything found there. This moves notes saved in a repair session into the repo on the next start.
      - Agent memory paths in code, tests and docs use `.claude/agent-memory/anthology-<agent>/`, not `<agent>/`.
      - Writer agents (worktree) must commit their `.claude/agent-memory/anthology-<agent>/` changes in their worktree as a separate `chore(memory): …` commit before finishing, so the memory merges with their branch. The agent bodies (Task 6) and routing guide (Task 13) say so.
      - Read-only agents run in the main clone and write memory there; the lead commits it.
    - **→ Task 14 cases** (smoke scripts: multi-session, so not `plugin eval`):
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
  - **U8 — hook output and failures.**
    - The `additionalContext` limit is **10,000 characters**, inclusive: a 10,000-char value arrived whole. At 10,001 characters, the model got `Output too large (…KB). Full output saved to: <session dir>/tool-results/hook-…-additionalContext.txt` and a preview (2,000 chars per the docs), and the tail marker was lost. The limit applies to each field separately, and no setting raises it.
    - A missing interpreter is a **non-blocking** error and the call goes ahead. In exec form: `Executable not found in $PATH: "<cmd>"`. In shell form: exit 127. The model sees nothing; the user only gets the hook-error notice.
    - `onFailure: "block"` exists (v2.1.295+) and works on 2.1.311: the call is blocked with `failed; blocking because onFailure is "block"`.
    - **Decisions:**
      - Routing guide + personal memory index + repo-memory cover stays under a test budget of **9,000 characters** (one field).
      - The guard keeps failing open: no `onFailure: "block"` on the matcher-`*` hook, since a missing `python3` would then block every tool and brick the session.
      - "Never silently off" can't rely on Python. If `python3` is missing, the Python `SessionStart` hook itself fails silently. So `anthology-guardrails` adds a first `SessionStart` hook in POSIX `sh` that emits the loud `systemMessage` when `command -v python3` fails.
      - `session_start.py` still runs the known-bad-sample self-test.
    - **→ Task 14 cases:**
      - **Unit test `test_hook_output_budget`:** run `session_start.py` against fixtures (the largest personal index + repo-memory cover), and assert `len(additionalContext) <= 9000`.
      - **Smoke `context-cap`:** a throwaway plugin whose SessionStart hook pads `additionalContext` to exactly N chars with `END-<N>` at the end. `claude -p "Without tools, quote any token starting with END-, else NONE" --max-turns 1`. Expected: N=10000 quotes `END-10000`; N=10001 gives NONE, and the reply mentions `Output too large` / `saved to`. A failure means the platform cap moved, so re-check the 9,000 budget.
      - **Smoke `python-missing`:** run the guardrails `sh` sentinel hook with `PATH=/nonexistent:/bin` (no `python3`), and assert stdout is one JSON object with a non-empty `systemMessage` that names python3. With `python3` present, assert no output.
      - **Smoke `missing-interpreter`:** a throwaway plugin with a `PreToolUse` Bash hook whose `command` is `python9`. `claude -p "Run: echo RAN-OK" --allowedTools "Bash(echo *)" --output-format stream-json --verbose --include-hook-events`. Assert `RAN-OK` is in the result and a `hook_response` has `Executable not found`. With `"onFailure":"block"`, the result says it was blocked.
  - **U9 — eval harness: works.**
    - `claude plugin eval` ran a case against the probe plugin with `tool_used` (Skill), `tool_order` (Skill before the MCP tool) and a `regex` grader on `mock_calls`. A mocked MCP server (`evals/mocks/<server>/<tool>.md`) answered and scored 1.00.
    - `--max-cost-usd` works: with a tiny ceiling and `--runs 2`, one run ran and the command exited **2**, with `partial: true, partialReason: "cost_ceiling"`.
    - **Finding:** a case's `plugins:` entries must sit under the containment root. That root is the enclosing plugin, so a suite inside `plugins/anthology/evals/` can't load `anthology-guardrails` or `anthology-superpowers`.
    - Evals placed in a non-plugin parent directory and run against that directory loaded all listed plugins and passed. That case dispatched the plugin agent, which Read its knowledge file and showed a cross-plugin preload.
    - **Decision:** the behaviour evals live at the repo root in `evals/`. Each case declares `plugins: ["../../plugins/anthology", "../../plugins/anthology-guardrails", "../../plugins/anthology-superpowers"]`, and `evals.yml` runs `claude plugin eval . --trust-plugin --ablation none --no-publish --max-cost-usd <ceiling> --model … --judge-model …`. BrowserOS cases mock the `browseros-neo` server under `evals/mocks/`.
    - **→ Task 14 cases:**
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
  - **`engines` and the minimum version.**
    - `claude plugin validate` on 2.1.311 warns `Unknown field 'engines'`, so `--strict` fails. The docs say `engines` is honored from **v2.1.323**.
    - On 2.1.329 (checked in a throwaway container), `validate --strict` passes for `plugins/anthology`.
    - Everything above was verified on 2.1.311, and no needed feature is newer (`onFailure` 2.1.295, `plugin eval` 2.1.269). So the minimum stays **`>=2.1.311`**. Clients older than 2.1.323 ignore the field, which is harmless.
    - **Decision:** CI's `claude plugin validate --strict` (Task 14) installs a pinned Claude Code **≥ 2.1.323**.
  - **Lead ruling folded in — anthology-superpowers versioning:** anthology-superpowers is versioned independently of upstream superpowers, starting at `0.1.0`. `rename-plugin` sets the plugin's own version from `sources.json` `plugin_version`, and `upstream_version` records 7.0.2. So the `~0.1` dependency in `plugins/anthology/.claude-plugin/plugin.json` resolves, and any transform or sync change gets its own bump.
- **2026-11-20, Task 8 (rtk conflict plan):** `rtk/` is a non-squash `git subtree add` of the fork `rbonweb/rtk` `develop` = `9b790c414de16f5fb688126b3945c73aa0e0218e`, then a `git subtree pull` of upstream `rtk-ai/rtk` `v1.2.1` = `d0f0399a5ff8cd1d81325a033f0b2d8864d665f6` (merge base `bd7f87eb`; upstream 172 commits ahead, the fork 5 change commits + 4 PR merges). Both pins are in `vendor/sources.json` (`managed_by: "subtree"`); `scripts/sync-rtk.sh <tag>` repeats the pull on an `rtk-sync/<tag>` branch.
  - **Textual conflict, `src/core/classify.rs`:** upstream moved `ureq` 2 → 3 (`.header()`, `http::global_timeout`, `send_json`, `http::read_json`); the fork's custom headers used ureq 2 (`.timeout()`, `.set()`). Kept upstream's request code and re-added the fork's loop over `task_cmd::configured_headers(upstream)` with `.header(name, value)`. `Content-Type` now comes from `send_json` (the fork protects that name anyway).
  - **Checked, no conflict:** `src/core/task_cmd.rs` merged cleanly: upstream's new `post_json` (ureq 3, `via_daemon`) already takes a header list, and the fork's `configured_headers` feeds it. `src/lib.rs` (minimal `read` default, `rtk opencode`), `src/discover/registry.rs` and `rules.rs` (node/python rules, `--level none` on head/tail, lint routing), `src/hooks/hook_cmd.rs` and the awareness files all kept both sides.
  - **Semantic conflict, `src/hooks/init/claude.rs`:** upstream's new test `boundary_newline_hook_is_idempotent_and_uninstall_is_selective` expects a settings file with the hook already installed to stay untouched, but its fixture has no `Bash(rtk *)` allow rule, so the fork's auto-allow heal patched it. Kept both: the fixture now carries `Bash(rtk *)` (a complete install), so it still proves a boundary-newline hook is not duplicated, and the heal of installs missing the rule stays. The fix is in the merge commit.
  - **Dropped:** nothing. The custom `[upstream.*]` headers live in engine-gated code (`classify.rs`, `task_cmd.rs`, `rtk-engine` schema); per the stream ruling, Task 9's license trim decides their fate with the BUSL engine.
  - **Named tests** (`rtk/tests/`, through the built binary): `fork_1_auto_allow_hook` (init adds and heals `Bash(rtk *)`), `fork_2_read_minimal_default` (`rtk read` strips comments; head/tail rewrites add `--level none`), `fork_3_node_filter`, `fork_4_python_filter` (rewrite plus the TOML filter on a stand-in tool), `fork_5_cd_chain_rewrite`, `fork_6_biome_lint` (script routing plus the biome filter on the fork's fixture), `fork_7_opencode_run` (rewrite plus the filter on the multistep fixture), `fork_8_upstream_headers` (`#![cfg(feature = "engine")]`: a local server sees the `{session}` id and that `Authorization`/`x-api-key` cannot be overridden), `fork_9_awareness_exact`. Run against plain upstream v1.2.1, all fail except `fork_5`: the fork's cd-chain change was a regression test only (upstream already rewrote after `cd`), so `fork_5` pins that behaviour.
  - **`$RTK_TEST` does not pass in the pinned image, before or after the merge.** With `--no-fail-fast` the merged tree has 5370 passed, 113 failed; upstream v1.2.1 alone fails exactly the same 113 tests in the same image, and the fork alone fails 113 as well. None is caused by the merge. Causes: `rust:1.100-slim` has no `git` (107 tests that create repos or run git), `curl` (1), `jq` (1, the shell hook) or `kill`/procps (2 signal tests), and the container runs as root, so 2 read-only-directory tests in `init_opencode_test` cannot fail. The canonical command stops after the lib unit tests (4857 passed, 5 failed, all git), so the integration tests do not run. The new `rtk-test` CI job runs the canonical command and will fail until the image question is settled (lead decision).
