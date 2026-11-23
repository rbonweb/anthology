# Changelog

All notable changes are recorded here, per plugin. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and each plugin uses
[Semantic Versioning](https://semver.org/) on its own: a release is tagged
`<plugin>--v<version>`, and `scripts/release.sh` moves a plugin's Unreleased entries into
a new version section.

## anthology

### [Unreleased]

#### Changed

- rtk is now ATL (Anthology Token Layer), a fork of rtk by RTK AI Labs. Everything that
  carried the old name is renamed: the `atl/` folder, the `atl` command and its `atl-hook`,
  `atl.lock`, the `atl--v<version>` release tags and `atl-<target>.tar.gz` archives, the
  `ATL_*` environment variables, the `atl_path` key in `state.json`, the install under
  `<plugin data>/atl/`, the history under `~/.claude/anthology/atl/`, project filters in
  `.atl/filters.toml` (they need `atl trust` again), `anthology atl-install`, and the allow
  rule `Bash(atl *)`. There is no `rtk` alias. ATL stays under the Apache License 2.0:
  `atl/NOTICE` credits upstream and ships with `atl/LICENSE` in every release archive, and the
  tool's README, DISCLAIMER and manifests describe the fork. Its bug-report link, its hook's
  install hint and its package maintainer now point at Anthology. `scripts/atl-rename.sh`
  applies the rename rule again, and `scripts/sync-atl.sh` merges a new upstream release over
  the renamed base. Tags already published under the old name stay as they are. The guard and
  the hook still recognise the old `rtk` name, so a command typed with it is checked as
  before. Routing guide section 7 is now "Command output: ATL (Anthology Token Layer)".
  `anthology` now needs `anthology-guardrails` `~0.2.7`. The binary is `1.2.1-anthology.6`.
  See ADR-035.
- The session-start guard self-test also runs an ATL runner form
  (`atl run -c 'npx playwright install'`) and expects it blocked, so a guard binary too old
  to look inside `atl` commands warns "guardrails may be OFF" instead of passing quietly.
  `scripts/release.sh` refuses to tag `anthology` or `anthology-guardrails` while the
  plugin's `anthology.lock` pins an `anthology-bin` older than the workspace version.
- Upgrading: after updating, re-run `/anthology:setup`, then restart your sessions. Session
  start installs ATL beside the old files and removes nothing. Until you run setup, `atl`
  commands prompt, and session start asks you to run it. Setup replaces the `Bash(rtk *)`
  allow it added earlier with `Bash(atl *)` (an `rtk` allow you wrote yourself is never
  touched), and shows, then deletes after you confirm, what the old install left: the
  `rtk_path` entry in `state.json`, `<plugin data>/rtk/` and the old history in
  `~/.claude/anthology/rtk/`. The old history is deleted, not migrated. Remove covers both the
  ATL and the old files. Upstream rtk's own folders are never touched. A session started
  before setup keeps the old hook: it exits cleanly, and `rtk` commands there fail until you
  restart the session.
- Downgrading to 0.2.x: that release's `/anthology:setup remove` does not know about ATL, so
  it leaves `<plugin data>/atl/` and the ATL history in `~/.claude/anthology/atl/` in place.
  Delete both folders by hand.
- pin anthology-bin 0.2.0 and atl 1.2.1-anthology.6.

#### Added

- `large-builds` skill: three rules for watching CI. A writer's report is not done until CI on
  its PR is green, or the report names the exact failing job (the lead says so in the brief and
  checks `gh pr checks` before accepting). At every status check the lead reads CI for every
  open PR, including integration branches, and sends each red job back to its owner the same
  hour. When an integration branch merges part PRs, the lead closes those part PRs with a
  comment pointing to the integration PR.
- `anthology perm classify` (nothing calls it yet): reads a Bash command, its working
  directory, the roots and the `Read()` rules as JSON on stdin. Only when the command is
  proven read-only does it print `anthology-perm:read-only:v1`, followed by the resolved path of
  everything the command reads, one per line. Every such path must exist under a root, be a
  file with one link or a directory, and match no `Read()` rule or secret pattern; recursive
  trees are walked and refuse on any symlink inside, and globs, non-ASCII text, relative paths
  without a leading absolute `cd` and a `PATH` entry inside the project give no decision.
  A line that runs git is allowed only when the repository's whole configuration holds nothing
  but a short allowlist of keys, no `GIT_*` variable, hook, alternates or submodule is in play,
  no tracked file is a secret or `Read()`-denied, and the git form prints no history
  (`show`, `log -p`, `diff --cached` and the like give no decision). Anything else gives no
  decision, never a deny.
- `bin/anthology` and `bin/anthology-mem` exit 127 at once, without downloading or installing,
  when `ANTHOLOGY_NO_INSTALL` is set to any non-empty value.

#### Fixed

- Session start no longer says the guardrails are "failing closed" when they are not. The guard
  self-test runs the guard shim with fetching switched off, so a binary that was not yet in
  place when the shim looked (a parallel session was still installing it, or this hook had
  deferred its own install) came back as "the guard binary is not installed (not attempted) ...
  shell commands, file edits and MCP tools are blocked", although the guard installs the
  binary on its first call and the binary was in the install folder a moment later. Session
  start now checks the install folder itself: when the pinned binary is there it reports that the
  self-test could not tell, and says the guard still runs on every command. It keeps the
  failing-closed warning when the binary really is missing, unusable (empty marker, symlinked or
  another version's folder) or the guard really refuses.
- A guard self-test that times out with the binary installed is now reported as "skipped the
  guard self-test (it timed out, the machine may be busy); the guard still runs on every
  command; if commands hang or are blocked, run /anthology:setup" instead of "the guardrails
  may be OFF". When less time is left than the self-test may take, it is not run: the install
  folder is checked instead, and the line says session start ran out of time only when the
  binary is installed, otherwise the failing-closed warning stands. A guardrails plugin that is
  disabled or missing is still named in that case.

### [0.2.9] - 2026-11-22

#### Added

- Attribution off at the source. Claude Code's own instructions tell the model to add a
  `Co-Authored-By` trailer to commits, so the no-attribution hook denied almost every first commit
  attempt. `/anthology:setup` now also writes Claude Code's `"attribution": {"commit": "", "pr": ""}`
  setting into an approved repo's committed `.claude/settings.json`, next to `autoMemoryEnabled`
  (same state record, revert on `remove`, size cap and preview listing). A value you set yourself for
  either key is never overwritten: the preview lists it in `notes` and leaves it. User-wide settings
  are not touched; setup's output says the same object can be added to `~/.claude/settings.json` by
  hand. The hook stays as the backstop, and its block message now says how to turn attribution off.

#### Fixed

- `rtk-hook` no longer rewrites a command into an rtk call that cannot run. Claude Code's shell
  snapshot defines `rg`, `grep` and `find` as shell functions, so a typed `rg -n word a b` can
  work with no `rg` file on PATH. rtk rewrote it to `rtk rg ...`, which execs a real `rg`, and
  the user saw `rtk: search failed: Failed to execute command: No such file or directory` and
  exit 1. After rtk answers with a plain rewrite, the hook now drops it, and the command runs as
  typed, in two cases. First, the typed command's first word (after leading `NAME=value`
  assignments) is `rg`, `grep`, `ag`, `find` or `fd` and is not a real executable on the hook's
  PATH (`command -v` in the hook's own `sh`, which has no snapshot functions). Second, the rewrite
  holds an `rtk rg|grep|ag|find|fd` call for a program that is not on PATH, even when the first
  word cannot be read (quoted, expanded, escaped, a path) or is a builtin such as `cd`. Other
  programs (cargo, npm, docker, git) always keep their rewrite, because they are never snapshot
  functions and a hook PATH poorer than the Bash tool's must not cost them it. A deny and a kept
  ask are never dropped.
- `anthology-mem save` run in a folder that is not a git repo now names the repos below it.
  When Claude Code is started in a parent folder that holds several repos, every save used to
  stop at "not inside a git work tree" with no way forward. The message now adds a line that
  lists up to five of the repos directly below the folder (sorted, then "and N more") and says
  to run the command from inside one, for example `cd <repo> && anthology-mem save ...`, or to
  start Claude Code inside the repo. It uses the same one-level scan, capped at 200 entries, as
  the session-start warning, and the exit code is unchanged. With no repo below, or a folder
  that cannot be listed, the message is the old single line. Folder names are untrusted text:
  each is shown in quotes, cut at 60 characters, with control characters, quotes and
  backslashes escaped, and the `cd` example is left out when the first name is not plain UTF-8
  or needed escaping or cutting.
- `anthology-mem search` accepts `--query <words...>` as an alias of the positional words, so
  `anthology-mem search --query tokens colour output` works as well as
  `anthology-mem search tokens colour output`. Giving the words both ways, or neither way, is a
  usage error (exit 2) that says where the words go. The help text shows both forms.

#### Changed

- The closing pointer that session start adds when it cuts the memory index now reads
  `anthology-mem search <words>` in place of `<query>`, since the command takes words. The
  length is the same, so the session budget is unchanged.
- pin anthology-bin 0.1.4.

### [0.2.7] - 2026-11-22

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
  `--body` and `--body-file` is required. The file must be a regular file of at most 16 KB (a
  larger one is refused, never cut), inside the repo or the system temp folder, and not at an
  obvious secret location (the guard's own list: `.ssh`, `.env`, keys, credentials, shell
  history); symlinks are resolved before the check.
- `anthology-mem save` warns on stderr, and still saves, when the agent already has a note with
  a very similar name or description, and suggests updating it with `--replace`.
- Every agent that saves notes except the scout (its startup budget has 4 bytes free) is told
  to pass a long note body, or one holding `$(`, backticks or delete-like words, through
  `anthology-mem save --body-file <path>` (a file inside the repo or temp folder) or
  `--body-file -`.
- When session start has to cut the memory index to fit its budget, the closing pointer now
  says only to use `anthology-mem search <query>`; it no longer lists the full `MEMORY.md`
  paths, because the index is capped and those files are not the way to read every note.

#### Fixed

- `rtk-hook` no longer asks in bypass mode. A hook "ask" prompts in every permission mode, so
  a command whose text mentioned `rtk` still prompted users who had chosen no prompts. The
  hook now drops rtk's ask when the top-level `permission_mode` of the hook input is
  `bypassPermissions`, and the command runs as typed. Auto, default, acceptEdits and plan modes
  behave as before (auto keeps the ask, so `Bash(rtk *)` cannot skip auto mode's classifier),
  and a deny is never dropped.
- Session start no longer injects the lead's rules into subagents. When a subagent compacts,
  Claude Code runs SessionStart inside it, and it used to get every lead-only warning too: the
  setup nag, the guardrails self-test and its "anthology-guardrails is not installed" warning
  (false when guardrails is installed for the project, because the subagent's folder can be
  outside it), the rtk and data-folder checks, and the once-per-repo agent-memory note, which a
  subagent could use up before the lead saw it. A subagent now gets its memory index and nothing
  else: no guide, no warnings, no state writes.
- Session start warns when Claude Code was started in a folder that is not a git repo but holds
  git repos directly below it. Anthology's agents need worktrees, so every dispatch of a writing
  agent failed there and the lead fell back to agents without Anthology's rules. The warning
  says to start Claude Code inside the repo you are working on. It looks one level down and at
  no more than 200 entries, and it is not shown in subagents.

#### Changed

- Writing agents (implementer, debugger, ui-designer, docs-writer) and the routing guide now
  delete only literal paths and never `rm -rf $VAR`, because Claude Code asks the user about
  removals it cannot prove safe, even in bypass mode.
- Routing guide: the lead hands off all making and changing to the matching agent and keeps
  only reads, lookups, answering the user and a fix of a few lines in one file. This rule was
  already in 0.2.0 but was not listed in its changelog.
- Every generated memory index (personal, repo, agent, imported) is capped at 8 KB (was 20 KB).
  Agents' `MEMORY.md` files had grown to 17-20 KB with 88-138 notes, many of them per-task
  logs. Past the cap an index keeps the newest notes of each type and ends each cut type with
  "N more — use `anthology-mem search`". The session-start pointers no longer name the
  `MEMORY.md` file, since it is capped too.
- A repo that uses Anthology memory has one memory: the repo's committed `.claude/memory/`
  plus personal memory. When `/anthology:setup` approves a repo, it merges
  `"autoMemoryEnabled": false` into that repo's committed `.claude/settings.json`, so Claude
  Code's built-in auto memory does not run beside it. The file is committed, so the switch
  applies to everyone who uses the repo. The user-wide settings are not touched for this, and
  repos that do not use Anthology keep auto memory. The preview shows the diff, apply records
  the edit before writing it, and `/anthology:setup remove` puts the file back (deleting it
  only if setup created it and nothing else is in it). A symlinked, non-JSON or oversized
  file is left alone.
- Claude Code's subagent memory (`memory: project`) is part of auto memory, so in such a repo
  it stops preloading each agent's `.claude/agent-memory/anthology-<agent>/MEMORY.md`. Every
  agent's prompt tells it to read that file before starting, and `anthology-mem save` still
  writes the notes. Injecting each agent's memory at spawn is planned as a follow-up.
- With that setting in a repo's committed settings, session start keeps the memory step
  working in approved repos: it no longer reads the committed `false` as an opt-out there (a
  user, local or environment opt-out still is) and injects the repo's memory index every
  session, because Claude Code no longer loads it.
- `/anthology:setup` moves the old notes of Claude Code's auto memory by type, after you
  confirm (`apply --move-old-notes`): project, feedback and reference notes go into
  `.claude/memory/` as `<type>-<slug>.md`, user notes into personal memory. Notes bound for
  the repo go through every check `anthology-mem save` runs; a refused note, or one without a
  type, is listed and not moved. The originals are left in place and never deleted. Without
  the confirmation the notes are still copied only into personal memory, as before.
- The routing guide says repo memory is where the lead writes its notes, because Claude's
  auto memory is off in approved repos.
- Field fixes from an audit of a long multi-agent session:
  - Writing agents edit files with Edit/Write only (no heredocs, `sed -i` or file-writing
    scripts) and run one git command per Bash call, because Claude Code refuses commands it
    cannot verify. They also never use a long `sleep`, load deferred tools such as Monitor
    through ToolSearch before calling them, and put a timeout on every test run, reporting a
    hang instead of waiting.
  - Writing agents also tag Docker images and name containers per worktree, commit with
    `git commit -m "subject" -m "body"` (never `-F`, which the attribution hook cannot see) and
    merge with single commands.
  - Every agent that saves notes (architect, researcher, reviewer, ui-reviewer, marketer,
    product-planner and the four writers) saves only lasting facts, searches with
    `anthology-mem search` first and updates an existing note instead of adding a duplicate.
    The scout is left alone because its startup budget has 4 bytes free.
  - Routing guide: merge conflicts go to an implementer, long checks run in the background or
    go to an agent, plans hold no lead coding tasks, agents are briefed with a plan path and
    task ID, a user's stop or pause beats the plan, and Docker images and containers are named
    per worktree so parallel agents do not test each other's code.
  - `large-builds` skill: the detail behind those rules (lead and user, waiting and hangs,
    per-worktree Docker names, small memory).
  - The routing guide dropped its model-to-agent list (each agent already fixes its own model)
    to stay within its size budget.
- The plugin now pins rtk `1.2.1-anthology.5` (tag `rtk--v1.2.1-anthology.5`) in `rtk.lock`,
  and the session-start reconcile installs it. It is built on the shared `anthology-shell`
  parser with the `$((` lexer fix, so a program word hidden inside `$((…))` is no longer let
  through, and its gate has the change below.
- rtk fork, in rtk `1.2.1-anthology.5`: with any deny, ask or `Read()` rule, a line still passes
  without a prompt, and is still rewritten, when it has a `cd TARGET &&` in front of the one
  plain `rtk` command or a `| head -n N` or `| tail -n N` behind it. `TARGET` must be an
  absolute or `./` path that exists (no `..`, `~`, `-`, variable or bare `src`, which
  `CDPATH` could redirect), and the command's relative paths are resolved against it before
  your `Read()` rules, so `cd ./secret && cat k.txt` is still denied. The count must be digits
  only (no variable, substitution, file or `-f`). A second pipe, `;`, `||`, `&`, a newline, a
  second `cd` or a redirect still asks. Everything else about the plain shape is unchanged.
  The plain shape also refuses a word that opens with `=` once its quotes are gone (`''=ls`,
  which zsh turns into a command path), and a word that starts with `-` after the first
  operand (`rtk wc a -l`), which the macOS tools read as a file.
- New in `anthology-shell` (used by the guard, rtk and the later permission levels):
  `classify_readonly`, which proves that a Bash command only reads, and is conservative by
  default: anything it does not prove is not read-only. It is not wired into a hook yet.
  It is written for the macOS tools under zsh: a word that starts with `-` after the first
  operand is not proven for any program (BSD tools read it as a file, and `uniq a -c` writes
  one), a zsh numeric glob such as `<1-5>` is not proven, nor is a word that opens with `=`
  once its quotes are gone. A path that names an obvious secret (`.ssh`, `.env`, `*.pem`,
  `id_*`, `.aws/credentials` and so on) is never proven. A proof says, for each path,
  whether everything under it is read, and whether `git` is in the line, so the later path
  check can look at git's configuration. The `grep` there is the snapshot's ugrep, so `grep
  -5` and any `-d` value other than `read`, `skip` or `recurse` are not proven; `diff` of two
  directories, a `**` glob and a `find`, `rg`, `du` or `tree` read everything under their
  path, and a directory that holds a secret by name is not proven for them. Secret names are
  matched in any case and now include `.config/gh/hosts.yml`, `.docker/config.json`,
  `.kube/config`, `.claude/.credentials.json` and the shell history files. A `git` format
  with `%G` (it runs gpg) is not proven.
- pin anthology-bin 0.1.3.

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

#### Changed

- The guard reads `atl`, the new name of the command-output tool (ATL, Anthology Token Layer),
  the way it read `rtk`: it looks through `atl run -c '…'`, `atl err …` and the other ATL
  runners and checks the command inside. The old `rtk` name is still read the same way, so a
  command typed with it cannot hide what it runs. Messages name ATL. `anthology` 0.3.0 needs
  this release (`~0.2.7`). The change ships in the next `anthology` binary (0.2.0).
- The too-complex message (G0) now says how to fix a search pattern in double quotes that holds
  one backtick, such as `grep -n "foo\|^export const X = <backtick>" file`. Inside double quotes
  a backtick starts a command substitution, here one that never ends, so the block is right but
  agents retried the same command. The message adds: "A backtick inside double quotes runs a
  command; put the pattern in single quotes". The verdict does not change, and a pattern in
  single quotes was and stays allowed. This ships in the next `anthology` binary (0.1.5).
- pin anthology-bin 0.2.0.

### [0.2.4] - 2026-11-22

#### Security

- The guard no longer trusts a value it cannot see (ADR-033). Each of these ran a blocked
  program in bash or zsh and was allowed; each is now blocked as too complex to check (G0):
  - a program or a script that comes from a variable or an array element, also behind
    `env`, `command`, `exec`, `nice`, `time`, `xargs`, zsh's `noglob` and `nocorrect`,
    `find -exec` and the like, and through `eval`, `bash -c` or `env -S`;
  - a positional parameter as a program or in a script, unless the guard filled it in with
    a literal value itself (a `sh -c '…' _ args` script, or a function only called directly):
    a shell reading its script from stdin with arguments, `find -exec sh -c … {}`, a function
    run from `trap` or `eval`, zsh's anonymous functions, `shift`, and a
    `command_not_found_handle(r)` definition;
  - a zsh glob qualifier, which runs code (`*(e:…:)`, `file(+cmd)`, after a group, in a
    `${x:-…}` default), and a program word with a glob character or from `find -exec {}`;
  - an expansion that runs code held in a value: bash's `${x@P}`, zsh's `(e)` flag and
    `${~x}` (in heredoc bodies too), the zsh options `GLOB_SUBST` and `PROMPT_SUBST` (also
    named by a variable), a prompt variable or `BASH_ENV`/`ENV` that holds a substitution,
    and a nameref or a variable name that could point at one;
  - a value from `read`, `mapfile`, `readarray` or `select`, or text that holds a
    substitution, with anything the shell evaluates (a subscript, arithmetic, an `-i`
    variable);
  - a subscript with nested brackets or quotes around a substitution;
  - an alias, a hashed command path, a zsh function defined from text, and bash's alias and
    command tables or an exported `BASH_FUNC_` named in any word.
- zsh's `=npx` and bash's `coproc NAME { … }` are read as the program they run, so their
  rules apply.
- Commands whose program word is a variable (`$EDITOR file`) are now blocked as too complex;
  spell the program out. A program under a standard directory stays allowed and is checked
  as written (`"$HOME/.cargo/bin/cargo" build`, also `TMPDIR`, `CLAUDE_PLUGIN_ROOT`,
  `CLAUDE_PROJECT_DIR`, `VIRTUAL_ENV`) when the command is plain: no other expansion, and
  only letters, digits, `_ . / : = , + @ % -`, blanks, quotes and `; & | < >` elsewhere. The
  name may not appear anywhere else in the command. `${HOME}/…`, `$PWD` and a `$HOME` inside
  a double-quoted `sh -c` script are not read through. So do function wrappers such as
  `run() { "$@"; }; run make test` whose call words are plain. Accepted over-blocks: a read
  loop that counts (`while read -r l; do n=$((n+1)); done < f`) is too complex, since any
  arithmetic in a command that reads input is, and so is a `retry` wrapper whose body holds
  a number (`for i in 1 2 3`). The too-complex message suggests single
  quotes when a double-quoted script holds a `$`. SECURITY.md lists what is and is not
  blocked. These ship in the next `anthology` binary (0.1.4).

#### Changed

- pin anthology-bin 0.1.4.

### [0.2.2] - 2026-11-22

#### Fixed

- A read-only search is no longer blocked as "too complex" (G0) because its regex has a
  bracket expression that holds a backtick, for example a `grep -E` pattern in double
  quotes that has a quote and a backtick, both escaped, inside square brackets. The guard
  treated every opening square bracket that had a backtick after it as the start of an
  array subscript, and bash and zsh run a command in a subscript. A subscript only follows
  a name, so a bracket at the start of a word, or after punctuation such as a space or a
  quote, is now a plain bracket expression, but only in a command with no `$` anywhere, no
  backtick substitution, and no shell, `eval`, `xargs` or `parallel` in it: otherwise an
  expansion elsewhere can put a name in front of the word (`set -- '[...]'; unset "a$1"`),
  and every bracket is read as before. In such a command everything else is read as before: a bracket
  after a letter, digit, underscore, non-ASCII character, `]`, `}`, `)`, `$`, `*`, `?` or
  `=`; every bracket in a command that has an array literal (`a=( [k]=v )`); and every
  bracket in a command with a `$(...)` or backtick substitution, because the guard takes a
  substitution out of the word and a name may have stood there. A backtick escaped inside
  double quotes is a plain backtick and was already read that way. It still counts in a
  subscript after a name, because bash removes the quotes before it evaluates the
  subscript. The fix ships in the next `anthology` binary (0.1.3).

#### Changed

- pin anthology-bin 0.1.3.

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

### [0.2.1] - 2026-11-22

#### Fixed

- The SessionStart hook prints nothing inside a subagent. When a subagent compacts, Claude
  Code runs the hook there too, and it injected the `using-superpowers` introduction into the
  subagent. The hook now reads the first line of its input and exits quietly when Claude Code
  has set a non-empty `agent_id`. The read waits at most two seconds, so an input that never
  closes cannot hang the hook, and other harnesses are not asked.

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
