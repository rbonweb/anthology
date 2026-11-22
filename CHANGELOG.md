# Changelog

All notable changes are recorded here, per plugin. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and each plugin uses
[Semantic Versioning](https://semver.org/) on its own: a release is tagged
`<plugin>--v<version>`, and `scripts/release.sh` moves a plugin's Unreleased entries into
a new version section.

## anthology

### [Unreleased]

#### Added

- Attention alert: a `Notification` hook pops a desktop alert with a sound when Claude Code
  shows a permission prompt or waits for your input (macOS `osascript`, Linux `notify-send`
  plus `paplay` or `canberra-gtk-play`, else a terminal bell). Set `ANTHOLOGY_NOTIFY=0` to
  turn it off.
- Project map and after-work check. The architect builds `.claude/memory/reference-project-map.md`
  from a new template (`templates/project-map.md`, with a size budget and a `## Cleanup duties`
  section), and the routing guide tells the lead to read it before planning. Writing agents
  update the map in their final commit when a change adds, removes or rewires a piece. The
  reviewer gets a fixed after-work checklist: connected pieces updated, nothing left behind,
  cleanups done, map updated.
- Attribution rule: commits, PRs and code comments carry only the git config author. The
  writing agents (implementer, debugger, ui-designer, docs-writer) and the routing guide say
  never to add `Co-Authored-By` or "Generated with" lines and never to mention the assistant
  or AI. A new `PreToolUse` hook (`hooks/no-attribution.sh`) enforces it: it denies a `git commit`
  (amend included) or a `gh pr create|edit` command whose text carries a `Co-Authored-By:` trailer
  that names the assistant or its maker, or a "Generated with" line that names the assistant.
  Every other command is allowed. The check catches the usual habit and is not a guarantee:
  `--trailer`, `-F` and `--body-file` are not checked.
- `anthology-mem save --body-file <path>` takes the note body from a file (`-` for stdin) and
  applies the same checks as `--body`. Claude Code's delete safety check reads the command line,
  so a note that merely mentioned a delete command was refused when passed inline. Exactly one of
  `--body` and `--body-file` is required.
- `anthology-mem save` warns on stderr, and still saves, when the agent already has a note with
  a very similar name or description, and suggests updating it with `--replace`.

#### Fixed

- `rtk-hook` no longer asks in bypass mode. A hook "ask" prompts in every permission mode, so
  a command whose text mentioned `rtk` still prompted users who had chosen no prompts. The
  hook now drops rtk's ask when the top-level `permission_mode` of the hook input is
  `bypassPermissions`, and the command runs as typed. Auto, default, acceptEdits and plan modes
  behave as before (auto keeps the ask, so `Bash(rtk *)` cannot skip auto mode's classifier),
  and a deny is never dropped.

#### Changed

- Writing agents (implementer, debugger, ui-designer, docs-writer) and the routing guide now
  delete only literal paths and never `rm -rf $VAR`, because Claude Code asks the user about
  removals it cannot prove safe, even in bypass mode.
- Routing guide: the lead hands off all making and changing to the matching agent and keeps
  only reads, lookups, answering the user and a fix of a few lines in one file. This rule was
  already in 0.2.0 but was not listed in its changelog.
- Memory indexes are capped at 8 KB (was 20 KB). Agents' `MEMORY.md` files had grown to 17-20 KB
  with 88-138 notes, many of them per-task logs. Past the cap an index keeps the newest notes of
  each type and ends each cut type with "N more — use `anthology-mem search`".

### [0.2.0] - 2026-11-22

#### Added

- Plugin manifest with version-range dependencies on `anthology-guardrails` and
  `anthology-superpowers`, so installing `anthology` brings in both.
- 11 agents, called `anthology:<name>`, each with a fixed model alias and project memory:
  - Opus: `researcher`, `architect`, `reviewer`, `ui-reviewer`, `product-planner`.
  - Sonnet: `implementer`, `debugger`, `docs-writer`, `ui-designer`, `marketer`.
  - Haiku: `scout`.
- Knowledge library: 29 folders (design, engineering, product, marketing, UI and house
  style), generated from pinned upstream sources by `anthology-dev vendor`. Agents read them
  from their own startup lists; the main session does not see them. Each folder ships its
  upstream license text inside the plugin, and `caveman` also ships its `NOTICE` and
  `LICENSE-MIT`.
- Three user commands: `/anthology:setup` (machine check, then one confirmed set of
  changes to settings, rtk and repo memory; `remove` undoes them), `/anthology:export` and
  `/anthology:import`. Each runs only when you ask for it.
- SessionStart hook: delivers the routing guide to the main session, runs the guardrails
  self-test, reconciles rtk with `rtk.lock`, keeps memory working in approved repos, and
  warns when setup has not run or the plugin has moved. A problem in any step becomes a
  warning and never fails the session. A subagent that compacts (its hook input carries
  `agent_id`) gets no routing guide, and the "set autoMemoryDirectory" notice stays quiet
  after a compaction.
- Routing guide (`templates/routing-guide.md`): model roles, which agent for which job,
  worktree, Docker and blocked-agent rules, the build skill to load for multi-task builds,
  and the lead's memory duties, including committing the `.claude/agent-memory/` notes that
  read-only and docs-only agents leave uncommitted (once per merge batch or at task end)
  and acting on each report's "Worth remembering" items.
- Working rules ship with the plugin instead of living in a user's global config
  (ADR-026). `karpathy-guidelines` (multica-ai/andrej-karpathy-skills by forrestchang and
  contributors, MIT) is a knowledge folder read by the implementer, debugger and reviewer.
  `anthology:large-builds` is a skill for long multi-agent builds, status reports and
  rulings while the user is away, and it carries the coding rule; the routing guide points to
  it in one line.
- Worktree agents (implementer, debugger, docs-writer, ui-designer) change files with
  Edit/Write and keep one plain command per Bash call, because Claude Code refuses
  commands in a worktree that it cannot check; a multi-step script goes in a file run
  with `bash`. The reviewer reads tech-debt and deploy-checklist only when they apply.
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
- Doctor, run by setup: checks the guardrails' `anthology` binary, `node`, Docker, BrowserOS
  Neo, the rtk binary and the platform, and finds installed copies of the official `superpowers` and
  `design-skills` plugins that Anthology replaces. It also checks the plugin-read
  permission, that BrowserOS tool names in your MCP configuration match what the plugin
  expects (`browseros_tools`, read from configuration files only), the plugin data folder,
  the `bin/` wrappers, where agent memory is written, and whether
  `CLAUDE_CODE_SUBAGENT_MODEL` overrides the roster's model tiers. `rtk_savings` reports, from
  rtk's own history, how many of the commands the hook saw went through rtk and how many
  output tokens that saved, so you can judge whether rtk is worth keeping.
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
- Setup handles `CLAUDE_CODE_SUBAGENT_MODEL`, which overrides every agent's model: when it
  is set in your user `settings.json`, setup offers to remove it as a separate question and
  `/anthology:setup remove` puts it back; anywhere else it says where it is set and how to
  remove it there.
- rtk: condenses the output of common dev commands through a `PreToolUse` hook on Bash.
  The pinned binary (`rtk.lock`, sha256 per platform) is downloaded from this repo's
  releases and checked by setup, then reconciled at every session start. Claude Code only,
  no telemetry; without the binary the hook does nothing and commands run unchanged.
  File reads (`cat`, `head`, `tail`) are not rewritten to `rtk read`: it saved about 3% on
  them and changes the text Claude reads, so they run as typed.
- BrowserOS Neo is the only browser agents use; the plugin launches no other browser.
- The plugin's own Apache-2.0 `LICENSE`.
- Notices marking the Apache-2.0 files Anthology modified (the connector stripping on the
  knowledge-work folders, and the `caveman` and `ui-taste` patches) come with the owner's
  credits pass, before 1.0.0.

#### Changed

- The plugin's code is one static `anthology` binary written in Rust, in place of the Python
  scripts. The SessionStart hook, setup, doctor, export and import, `anthology-mem` and
  `bin/anthology` run it through small POSIX `sh` shims in `hooks/` and `bin/`. Behaviour,
  files and messages are unchanged except where they named Python.
- The binary is downloaded on first use from this repo's `anthology-bin--v<version>` release,
  checked against the sha256 for your platform in `anthology.lock` before it is unpacked, and
  installed in `~/.claude/anthology/bin/<version>/`, shared with `anthology-guardrails`.
  Offline, copy the release archive into `~/.claude/anthology/bin/`. If it cannot be
  installed, session start says why in one line, and the routing guide, memory and rtk stay
  off until it is. `/anthology:setup remove` keeps the binaries while `anthology-guardrails`
  is installed.
- New requirements, for the first download only: network access to GitHub, one of `curl`,
  `wget` or `node`, one of `sha256sum`, `shasum` or `openssl`, and `tar`.

#### Removed

- The Python requirement: Anthology no longer needs `python3`, and
  `plugins/anthology/scripts/` is gone.

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
- The rtk hook no longer prompts for commands that do not mention `rtk` (`node --version;
  bun --version`), also in bypass and auto mode: it drops rtk's ask, and the command runs
  as typed under your own mode and rules, as without rtk. An ask on a line that mentions
  `rtk`, a deny and an rtk failure still pass through. On other lines your ask rules get only
  Claude Code's own matching (not `git -C . push` or `bash -c '…'`); deny rules still apply
  in every form, so use a deny rule for anything that must never run. A `$'` that ends a
  quoted regex (`grep -v '^a$'`) is not a mention, and neither is a `$'...'` string, nor is `rtk` inside a longer word (`insertKey`).
- rtk's local history (command lines, recall store, tee spool) is kept in
  `~/.claude/anthology/rtk/` and deleted by `/anthology:setup remove`; README and SECURITY
  document what it stores and for how long.

## anthology-guardrails

### [Unreleased]

### [0.2.0] - 2026-11-22

#### Added

- Plugin manifest and marketplace entry.
- A `PreToolUse` hook on every tool call that blocks, with a message naming the
  alternative. Malformed input never blocks a call.
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

#### Changed

- The guard is part of the `anthology` binary (Rust), run by the POSIX `sh` shim
  `hooks/guard.sh`. Rules and block messages are unchanged. The plugin ships the same
  `anthology.lock` as `anthology` and installs the pinned binary itself at session start, so
  it also works on its own.
- New requirements, for the first download only: network access to GitHub, one of `curl`,
  `wget` or `node`, one of `sha256sum`, `shasum` or `openssl`, and `tar`.

#### Removed

- The Python guard (`scripts/guard.py`, `scripts/guard_rules.py`) and the `python3`
  requirement.

#### Security

- The guard fails closed. While its binary is missing, broken or cannot run on this
  machine, only read-only tools (`Read`, `Grep`, `Glob`, `WebFetch`, `Skill`, `Task` and the
  like) are allowed; shell commands, file edits and MCP tools are blocked with a message that
  names the fix and the way out, disabling `anthology-guardrails` with `/plugin`. A blocked
  call retries the install at most once a minute, a binary that does not work is set aside
  and reinstalled, and after three failures in 24 hours only a new session retries.
- An error inside the guard (a crash, a pattern limit, a check that runs longer than 8
  seconds) now blocks the call instead of allowing it.

## anthology-superpowers

### [Unreleased]

### [0.2.0] - 2026-11-22

#### Added

- The upstream superpowers 7.0.2 skills library, renamed to `anthology-superpowers`, kept
  reproducible by `anthology-dev vendor` and versioned independently of upstream. It ships
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
