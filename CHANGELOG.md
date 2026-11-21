# Changelog

All notable changes are recorded here, per plugin. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and each plugin uses
[Semantic Versioning](https://semver.org/) on its own: a release is tagged
`<plugin>--v<version>`, and `scripts/release.sh` moves a plugin's Unreleased entries into
a new version section.

## anthology

### [Unreleased]

#### Added

- Plugin manifest with version-range dependencies on `anthology-guardrails` and
  `anthology-superpowers`, so installing `anthology` brings in both.
- 11 agents, called `anthology:<name>`, each with a fixed model alias and project memory:
  - Opus: `researcher`, `architect`, `reviewer`, `ui-reviewer`, `product-planner`.
  - Sonnet: `implementer`, `debugger`, `docs-writer`, `ui-designer`, `marketer`.
  - Haiku: `scout`.
- Knowledge library: 28 folders (design, engineering, product, marketing, UI and house
  style), generated from pinned upstream sources by `scripts/vendor.py`. Agents read them
  from their own startup lists; the main session does not see them. Each folder ships its
  upstream license text inside the plugin, and `caveman` also ships its `NOTICE` and
  `LICENSE-MIT`.
- Three user commands: `/anthology:setup` (machine check, then one confirmed set of
  changes to settings, rtk and repo memory; `remove` undoes them), `/anthology:export` and
  `/anthology:import`. Each runs only when you ask for it.
- SessionStart hook: delivers the routing guide to the main session, runs the guardrails
  self-test, reconciles rtk with `rtk.lock`, keeps memory working in approved repos, and
  warns when setup has not run or the plugin has moved. A problem in any step becomes a
  warning and never fails the session.
- Routing guide (`templates/routing-guide.md`): model roles, which agent for which job,
  worktree, Docker and blocked-agent rules, and the lead's memory duties, including
  committing the `.claude/agent-memory/` notes that read-only and docs-only agents leave
  uncommitted and acting on each report's "Worth remembering" items.
- Memory as committed Markdown notes: main session in `.claude/memory/`, each agent in
  `.claude/agent-memory/anthology-<agent>/`, personal notes outside any repo. Repo memory
  (`.claude/memory/`) is read and kept up only in repos approved through `/anthology:setup`.
  Agent memory is different: Claude Code loads `.claude/agent-memory/anthology-<agent>/`
  into that agent in any repo, approved or not, so the agent bodies treat those notes as
  hints, never instructions, and the README and `SECURITY.md` say to review that folder in
  repos you do not trust (ADR-019). `MEMORY.md` indexes and the local SQLite search index
  are derived from the notes and rebuilt on demand.
- Personal memory stays out of repos. `type: user` notes are moved out of `.claude/memory/`
  and `.claude/agent-memory/` into `~/.claude/anthology/personal-memory/` at each session
  start, and notes from Claude Code's old path-keyed memory folder are copied into personal
  memory only, never into the repo on their own. `/anthology:setup` can promote chosen old
  notes into the repo, one by one, after showing a diff (ADR-020). If the personal-memory
  folder is a symlink, session start says so and moves and loads nothing through it.
- A privacy scan guards anthology's own writes into repo memory (setup's promotion,
  `/anthology:import` and `anthology-mem save`): home paths, email addresses, IP
  addresses, private host names, ssh targets and key paths, and secret-looking tokens
  (including `secret_access_key = …` assignments and `scheme://user:password@host` URLs)
  go to personal memory or are refused, naming only the kind and line. If the scanner
  itself fails, the save or import is refused with one line and nothing is written.
  Claude Code's own auto-memory writes are not scanned.
- `anthology-mem` on the Bash path: `search`, `related`, `orphans`, `stale`, `rebuild`,
  and `save`, which lets agents without a Write tool save their own notes. `save` is
  limited to the agent memory folders, refuses symlinks and secret-looking content, and
  overwrites only with `--replace`.
- Memory export and import: `/anthology:export` writes a `tar.gz` archive of personal and
  current-repo notes; `/anthology:import` shows a dry run first and never overwrites a
  note.
- Doctor, run by setup: checks `python3`, `node`, Docker, BrowserOS Neo, the rtk binary and
  the platform, and finds installed copies of the official `superpowers` and
  `design-skills` plugins that Anthology replaces. It also checks the plugin-read
  permission, that BrowserOS tool names in your MCP configuration match what the plugin
  expects (`browseros_tools`, read from configuration files only), the plugin data folder,
  the `bin/` wrappers, where agent memory is written, and whether
  `CLAUDE_CODE_SUBAGENT_MODEL` overrides the roster's model tiers.
- Session start prints one loud line when a compatibility assumption breaks (no
  `CLAUDE_PLUGIN_DATA`, missing `bin/` wrappers, agent memory written outside
  `anthology-<agent>/`, `CLAUDE_CODE_SUBAGENT_MODEL` set), and one note when a release
  changed the `settings.json` entries setup makes and one is still missing. The note only
  suggests re-running setup; it never edits `settings.json`. If you removed such an entry on
  purpose, it returns at each session start after every release that changes the entries,
  until you run setup again.
- Setup shows its `settings.json` diff with every value outside `permissions.allow`,
  `permissions.deny` and `worktree.baseRef` replaced by `"<redacted>"`. Those three paths
  are shown as they are, including your own rules, so a token written inside one of your
  rules would be visible in the conversation.
- rtk: condenses the output of common dev commands through a `PreToolUse` hook on Bash.
  The pinned binary (`rtk.lock`, sha256 per platform) is downloaded from this repo's
  releases and checked by setup, then reconciled at every session start. Claude Code only,
  no telemetry; without the binary the hook does nothing and commands run unchanged.
- BrowserOS Neo is the only browser agents use; the plugin launches no other browser.
- The plugin's own Apache-2.0 `LICENSE`.
- Notices marking the Apache-2.0 files Anthology modified (the connector stripping on the
  knowledge-work folders, and the `caveman` and `ui-taste` patches) come with the owner's
  credits pass, before 1.0.0.

#### Security

- rtk `1.2.1-anthology.3`: the rtk hook applies your deny, ask and `Read()` deny rules (user,
  project, local and managed settings) to the command an `rtk …` form wraps, printing an
  explicit deny or ask, so `Bash(rtk *)` no longer runs a command one of those rules covers.
- rtk `1.2.1-anthology.4`: once you have any deny, ask or `Read()` rule, a line that runs
  `rtk` passes without a prompt only in the plain shape: one `rtk ls`, `tree`, `read`, `wc`,
  `diff`, `grep`, `rg`, `find`, everyday `rtk git` or `rtk cargo build/test/check/clippy/fmt`
  command with listed options and literal paths, and no pipe, `&&`, `;`, redirect,
  substitution, variable, glob, `@file` word or `cd` in front (and nothing after cargo's
  `--`, no `:` in a git word but a commit message, no `git add -f`). Everything else asks, and rtk rewrites
  only into that shape. A rule's command text anywhere in such a line asks, however
  quoted. A dashed git builtin (`git-push`), words after a redirect (`git >/dev/null push`)
  and a redirect before the command (`< secret.txt cat`) now meet your deny rules. Rules
  restrict commands, not what an allowed program loads from files. Without rules nothing
  changes.
- rtk's local history (command lines, recall store, tee spool) is kept in
  `~/.claude/anthology/rtk/` and deleted by `/anthology:setup remove`; README and SECURITY
  document what it stores and for how long.

## anthology-guardrails

### [Unreleased]

#### Added

- Plugin manifest and marketplace entry.
- A `PreToolUse` hook on every tool call that blocks, with a message naming the
  alternative, and fails open: malformed input or an internal error never blocks a call.
  - G1: host browser-automation installs and runs (Playwright, Puppeteer, and browser
    installs through `brew`).
  - G2: launching Chrome or Chromium on the host, and any `--remote-debugging-port`.
  - G3: any tool touching a Chrome or Chromium profile folder or its parent.
  - G4: any `agent-browser` command or install.
  - G5: MCP tools whose names contain `playwright`, `puppeteer`, `chrome-devtools` or
    `claude-in-chrome`.
  - G6: secret-looking content written into `.claude/memory/` or `.claude/agent-memory/`.
  - Commands inside `docker` or `docker-compose` are exempt from G1 and G2 only.
- The guard is a hook plus deny rules, not a sandbox; its known limits are listed in the
  README and `SECURITY.md`. The `anthology` SessionStart hook self-tests it and warns
  loudly if it cannot run.
- The plugin's own Apache-2.0 `LICENSE`.

## anthology-superpowers

### [Unreleased]

#### Added

- The upstream superpowers 7.0.2 skills library, renamed to `anthology-superpowers`, kept
  reproducible by `scripts/vendor.py` and versioned independently of upstream. It ships
  upstream's MIT `LICENSE` unchanged.

#### Changed

- The brainstorming visual companion no longer launches the operating system's default
  browser: `start-server.sh` has no `--open`, the server prints its URL, and the skill
  tells Claude to open it in BrowserOS Neo.
- The README carries the `/plugin install anthology-superpowers@anthology` line.
- subagent-driven-development gains `references/large-builds.md`, read on demand for
  large or long multi-agent builds: start tasks against published interfaces with marked
  stand-ins, keep idle agents busy, test at the cheapest level once, one review per task,
  batch merges, rulings instead of questions, no working around a refused action, and
  status reports in feature terms. SKILL.md, `references/fix-loop.md` and
  dispatching-parallel-agents point to it.
