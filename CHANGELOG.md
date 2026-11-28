# Changelog

All notable changes are recorded here, per plugin. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and each plugin uses
[Semantic Versioning](https://semver.org/) on its own: a release is tagged
`<plugin>--v<version>`, and `scripts/release.sh` moves a plugin's Unreleased entries into
a new version section.

## anthology

### [Unreleased]

- Fix: auto mode no longer refuses a plain `grep ... | tail` with "a hook changed this call's input after auto mode reviewed it" when the ATL binary in use is older than the plugin. ATL `1.2.1-anthology.7` and later rewrite nothing in auto mode, but an older binary (a running session that began before an update, or an install that did not finish) still rewrote `grep -n "x" file | tail -8` into `atl grep ...`. `bin/atl-hook` now drops any plain rewrite when the hook input's `permission_mode` is `auto`, whatever the binary answered, so the call runs as typed. A deny and an ask are kept as before. Other modes rewrite as before. No ATL release is needed.

### [0.5.11] - 2026-11-28

- Add: the `website-clone` knowledge, vendored from JCodesMore/ai-website-cloner-template (MIT) and adapted for Anthology. It copies a site's structure, layout, palette, fonts and feel into the project's own stack, starting from a tokens file, and always replaces the brand, images, text and form actions. Capture and comparison run in BrowserOS Neo, builds in Docker. `ui-designer` reads it only when the brief is a clone.
- Add: the routing guide's fan-out rule for several similar parts (one agent builds the shared base and commits, then one agent per part in parallel on disjoint files, then one review), and its clone line (ui-designer builds the shared base, then one ui-designer per page, then ui-reviewer).

### [0.5.10] - 2026-11-27

- Pin anthology-bin 0.2.9.

#### Fixed

- The `anthology` binary 0.2.9: the guard closes three bypasses that were already on `build` (they are not new in
  0.2.9; each was shown to run a program in bash 5 or zsh 5.9 while the guard allowed the command).
  1. Programs that run another program from an option the guard did not read. The value is now read as a command
     (so an install in it is G1), and a value that is not a known harmless one (`cat`, `less`, a compressor for
     `sort`) makes the command too complex (G0), because these programs also pass the command a file name that
     the text may not show (`rg --pre sh x p.sh` runs `sh p.sh`). Covered: `rg --pre` and `--hostname-bin` (and an
     abbreviation of either of at least four characters), ugrep's `--filter` and `--pager` (ugrep is the `grep`
     that Claude Code's shell runs; also `ug`, `egrep` and `fgrep`), `sort --compress-program` (and its `--co...`
     abbreviations; `gsort` too), and `LESSOPEN` and `LESSCLOSE` as a prefix assignment or an `export`.
     The options that only look alike (`--pre-glob`, `--filter-magic-label`) stay allowed, and so do
     `--pre=cat`, `--pager=less` and `sort --compress-program=gzip`.
     The harmless values for these options no longer include editors (`vi`, `vim`, `nano`, ...): vim and ex run
     the `:!` lines of the file they are handed. The same class on `build` is also closed: an awk (`awk`, `gawk`,
     `mawk`, `nawk`) program with `system(`, a pipe to or from a string, `|&` or an `@` directive, `awk -f`, `-E`,
     `-l` and `-i` other than `inplace` are too complex; so are `RIPGREP_CONFIG_PATH` set to a file,
     `ugrep --config`, `ugrep ---FILE` and `ug` (which reads `.ugrep`). A `.ugrep` file read by plain `ugrep`
     cannot be seen; see Known limits in `docs/guardrails.md`.
  2. zsh runs a command written between two backslash-escaped backticks inside an arithmetic subscript, in a
     `((` command, in `$((` and in `for ((`; bash does not. In arithmetic text the escaped pair is now read as a
     command substitution, and an unclosed one is too complex.
  3. An unquoted zsh glob group among the words of a command (`docker exec c (flock|x) psql -c "..."`, with a file
     `flock` in the folder) expands to words the guard never saw, so zsh ran `flock`. The lexer split the command
     at the paren and the analysis skipped the program that ran; such a `(` after a blank that follows words of a
     command is now too complex. A subshell or pattern list that follows `if`, `then`, `else`, `elif`, `while`,
     `until`, `do`, `!`, `{`, `case ... in` or `function name`, `for ((`, a function definition `f ()`, a `[[ ]]`
     pattern and an arithmetic command are unchanged; `time (cmd)` is now too complex.
- Fix: the guard in the `anthology` binary 0.2.9 closes three bypasses of its Bash check (options that run another program such as `rg --pre`, an escaped backtick in a zsh arithmetic subscript, and an unquoted zsh glob group such as `(a|b)` among a command's words), and treats editors, `RIPGREP_CONFIG_PATH`, `ugrep --config` and awk programs that run commands as too complex to check. See the `anthology-guardrails` entry and the 0.2.9 line above.

### [0.5.9] - 2026-11-27

- Pin anthology-bin 0.2.8 and ATL 1.2.1-anthology.9.
- Fix: Anthology no longer overrides your own per-repository agents and skills. Session start now adds a short "This project's own agents and skills" section after the routing guide: your `.claude/agents/*.md` and `.claude/skills/*/SKILL.md` (each by its frontmatter `name` and a description cut to 100 characters), an agent table or list under a heading with "agent" in it in the repo's `CLAUDE.md`, and then your own agents and skills in the Claude config folder (at most 12 entries in all, project ones first, files that fail to parse and Anthology's own skipped). Under it, the rule: for the jobs they cover, use the project's own agents and skills before Anthology's, and Anthology's agents fill the gaps. The routing guide says the same in one line, and the delegation mod's refusal now says to dispatch the project's own matching agent when one fits, else the `anthology:*` one. A dispatch to any other agent is never refused or redirected.
- Fix: the delegation mod no longer refuses edits that are not project code or are low risk. Files outside every git repo and everything in the Claude config folder (`~/.claude`, or `CLAUDE_CONFIG_DIR`) are yours to edit. In a repo, a pure deletion of up to 30 lines (4 KB) and an edit of up to 15 lines in a small config file (`json`, `yaml`, `toml`, `ini`, `conf`, `env` and the like) are tiny fixes, still at most 3 a turn in one file; code that is added or changed stays at 5 lines, and every code edit inside a repo is still handed to an agent.
- Added: a repo approved in `/anthology:setup` can switch parts of Anthology off for itself in `.claude/anthology.json` (read at the git top): `{"routing_guide": false}` leaves the routing guide, the project-agents section and the project-map line out of session start (memory still loads, and a floor of safety lines stays: no attribution, containers, no host installs, literal-path deletes, the browser rule, no secrets in repo memory), and `{"delegation": false}` turns the delegation mod off for that repo (it asks the new `anthology switch` command). In any other repo the file is ignored, and session start says so; when it counts, session start says that too. `ANTHOLOGY_DELEGATION=0` still turns the mod off everywhere. The project-agents list drops `<`, `>`, backticks and hidden characters, says its entries are the repo's own text, and reads at most 200 names per folder. A pure deletion needs an exactly empty `new_string`, and CI pipeline files keep the code limit.
- Fix: the routing guide no longer assumes Docker and worktrees: the worktree rules start "With worktrees" and the container rules "If the repo has tests, builds, E2E". It also no longer sends every repo to `anthology:architect` for a missing project map: session start says so only for a repo approved in `/anthology:setup`. To fit the 3800-character cap the headings "Model roles" and "Command output: ATL" are now "Roles" and "ATL output".
- Fix: ATL never widens permissions. `Bash(atl *)` approves every line that starts with `atl`, so ATL's hook now gives a line no decision of its own (leaving `Bash(atl *)` to approve it) only when Claude Code would have allowed the command anyway, with or without any deny, ask or `Read()` rule. A rewrite (`git push --force` to `atl git push --force`, `find . -delete`, `npx pkg`, `ssh h rm -rf x`, ...) is made only when the original is read-only by `anthology perm classify` or covered by your own allow rules, matched the way Claude Code matches them (exact, `prefix:*`, `prefix *`, wildcard; the blanket `Bash(atl *)` never counts); otherwise it is left as typed. A typed `atl` line (`atl bypass`, `atl proxy`, `atl run`, `err`, `test`, `summary`, `atl <tool>` such as `atl rm -rf x` or `atl git push --force`) is judged as the command it runs, in the shapes a path to atl, `env`, an assignment, `&&`, `||`, `;`, pipes, groups, `sh -c`, `xargs` and `find -exec`: your deny, ask and `Read()` rules apply to it, and it asks unless that command is read-only or allowed by your rules, so `atl bypass git log -5` still runs without a prompt and `atl bypass rm -rf x` asks. atl's own writing subcommands (`atl init -g`, `config --create`, `trust -y`, `learn --write-rules`, `gain --reset --yes`) ask unless you allowed them, and a subcommand word that is not plain text (`atl {bypass,rm}`, `atl ${X:-bypass}`, `atl $(echo bypass)`, `atl $X`) always asks. In auto mode nothing is rewritten, so the ask is about the line as typed. An ATL installed without the plugin has no classifier, so only your own allow rules apply there. The setup note about `Bash(atl *)` (`BYPASS_NOTE`, and its copy in the setup skill) now says this. This is ATL `1.2.1-anthology.9`; SECURITY.md and docs/atl.md no longer call the `atl bypass` approval an accepted risk.
- Fix: agents no longer type `atl` themselves, which caused repeated permission prompts. A typed `atl grep -rn ...` that searches whole folders must ask, by design, because the search may reach a secret file; a plain `grep` is rewritten by ATL, or left alone in auto mode. The shared `## Shell` section of all 11 agent prompts and section 7 of the routing guide now say: type commands plainly, never type `atl` (it is added for you), except `atl bypass <cmd>` when a result is unusable. To stay within the 3800-character cap the guide's section 2 heading "Which agent" is now "Agents" and section 7 "ATL output" is now "ATL".

### [0.5.7] - 2026-11-27

#### Changed

- The `anthology` binary 0.2.8: session start lists the repo's own agents and skills after the routing guide,
  reads `.claude/anthology.json` (`routing_guide: false` leaves the guide out but keeps a safety floor, and only
  in a repo approved in `/anthology:setup`; elsewhere the file is ignored and session start says so), adds the
  `anthology switch` command and says "read the project map, else dispatch the architect" only for an approved repo.
- The `anthology` binary 0.2.7: session start warns when the official `superpowers` plugin is enabled next
  to `anthology-superpowers`, and the message for a missing binary on the first offline session now says that
  the guard, which runs the same binary, blocks shell commands, file edits and MCP tools too (#261).
- The `anthology` binary 0.2.6: the guard no longer blocks `docker exec db psql -c "select count(*) from t"`
  as too complex to check. Inside a docker command every `-c` word was read as a shell option and the next
  word was parsed as a shell script, so SQL such as `count(*)` failed to parse. `psql -c` of plain SQL runs
  no shell and `docker exec` runs its arguments without one, so that SQL is now data. This holds only when
  the whole command is plain text (no parameter, substitution, glob, brace, redirect or parenthesis, so the shell
  cannot split or expand a word into others), the command is `docker exec`, `docker compose exec` or
  `docker-compose exec` with only the options `-u`, `--user`, `-w`, `--workdir`, `--detach-keys`, `-i`,
  `-t`, `-d`, `-it`, `--privileged`, `--interactive`, `--tty`, `--detach` and `-e`/`--env` for the libpq
  connection variables (`PGPASSWORD`, `PGUSER`, `PGDATABASE`, `PGHOST`, `PGPORT` and a few more; never
  `PATH`, and `--env-file` is not accepted), the program word is exactly `psql` (not `PSQL`, not a path),
  and each `-c` and `--command` value (there may be several; one that fails ends the reading for all of them) holds no backslash (`\!` and `\o |` make psql start a
  shell), `$`, backtick, `~`, `^` or `#`. The options of psql are checked too: `-c`, `--command`, `-U`, `-h`,
  `-p`, `-d` (and their long forms) with a plain name, `-v` for `ON_ERROR_STOP`, `ON_ERROR_ROLLBACK`,
  `AUTOCOMMIT`, `ECHO`, `QUIET` and `VERBOSITY`, a cluster of `A`, `t`, `q`, `X`, `x`, `H`, and at most two
  plain database or user names. `-o`, `-f`, `-L`, `-P`, any attached value and any connection string keep the
  reading they had, because psql can run a program for them. Anything else keeps the reading it had,
  including `sh -c`, `bash -c`, `docker run` and every other program that takes `-c`.
- Fix: ATL no longer prompts for a harmless read, and no longer makes auto mode refuse a call. With any deny, ask or `Read()` rule set, a line that merely contains `atl` or the legacy `rtk` (`grep -n x atl/src/main.rs`, `ls -la atl | head -60`) was not in the plain shape, so ATL's hook asked about it, and a hook ask beats `Bash(atl *)` in every mode. When `anthology perm classify` shows the line as typed read-only, every path it reads is under the project (`CLAUDE_PROJECT_DIR`, else the git top level, else the working directory), and no ask or `Read()` rule applies, ATL now hands back the plain rewrite with no decision of its own, so your `Bash(atl *)` rule decides, or leaves the line to Claude Code when it cannot rewrite it plainly. Deny rules are checked first and still win, and an ask rule still asks. In auto mode ATL no longer rewrites a command at all, so auto mode judges what the model wrote instead of refusing with "a hook changed this call's input after auto mode reviewed it". Other modes rewrite as before. This is ATL `1.2.1-anthology.7`; `bin/atl-hook` now tells ATL where the plugin's `bin/anthology` is (`ATL_PERM_CLASSIFY`) and which `PATH` you exported (`ATL_HOOK_USER_PATH`). An ATL installed without the plugin has no classifier and asks as before.
- Fix: the after-work check no longer flags "no commit" and "nothing about what was tested" on a report whose edited files are all outside a git repo, such as `~/.claude/settings.json`: there is nothing to commit or test there. The edited paths are read from the report text (absolute, `~/` and relative ones, relative to the agent's folder), and a path is in a repo when it or a folder above it holds `.git`. A report that names no path, a path in a repo, or a path the check cannot place keeps both checks, so a mixed report is still checked. Attribution lines are still flagged.
- Docs: the README install steps lead with `claude plugin marketplace add` and `claude plugin install`, which also work in the VS Code extension, which has no `/plugin` slash commands; the `/plugin` form stays as the terminal-app alternative. `docs/setup.md` explains the two cached copies of a dependency (`X.Y.Z` and `X.Y.Z-<sha>`) and how to clean up by hand.
- Fix: the guard no longer blocks a read-only search such as `grep -rnoE "[\"'\`][^\"'\`]*(\$\{|\" *\+)" dir | cut -c1-200 | head` as too complex to check. A dollar sign that a backslash escapes, or that a backslash follows, is a plain character in bash and zsh and expands nothing, but the guard counted it as an expansion, which made it read each `[` after a quote or a backtick as a possible subscript start. This holds only in a command line made up entirely of searches and filters (`grep`, `egrep`, `fgrep`, `rg`, `cut`, `head`, `tail`, `sort`, `uniq`, `wc`); in any other line an escaped dollar can be how a live one reaches a nested text such as `trap "..."` or `. <(...)`, so it is read as before. A real expansion (`$(`, `$name`, `${`, a backtick, or a `$` in single quotes) is read as before. Guard binary 0.2.6.
- Fix: `/anthology:setup` now shows the `.gitattributes` change that makes the generated `MEMORY.md` indexes merge by keeping both sides' lines (`merge=union`), so parallel agents stop conflicting there. The preview lists the lines it adds, never adds one twice and keeps the rest of the file, and tells you to commit the file so other worktrees and clones have the rule. Session start prints one line when an approved repo's `HEAD` lacks the rule; it writes nothing.
- Fix: ATL's read-only proof now compares the project and the paths the classifier lists as the file system resolves them, so a project under a symlinked folder (on macOS `/var` and `/tmp` are links into `/private`) is still the project, and a path that resolves outside the project never counts. The `1.2.1-anthology.7` release build failed its macOS tests for this reason and was not published; this is ATL `1.2.1-anthology.8`.
- Add: session start prints one line when the official `superpowers` plugin is enabled next to `anthology-superpowers`, because both sets of instructions would load. The line names both plugins and the command that disables the official one (`claude plugin disable <plugin> -s <scope>`). It does not name `anthology-superpowers`: `anthology` requires it, so Claude Code refuses to disable it alone. It shows at startup only, not on resume, clear or compact, and not for an embedded host. It uses the same detection as `/anthology:setup`. Needs the next `anthology` binary.
- Fix: when the `anthology` binary cannot be installed (offline on the first session), the session-start line now says that the guard, which runs the same binary, blocks shell commands, file edits and MCP tools too, and points to the `anthology-guardrails` line for the command that installs it. Before, it named only the routing guide, memory and atl, and sent the reader to `/anthology:setup`, which needs a shell command the closed guard blocks.

### [0.5.6] - 2026-11-26

- Remove: Anthology no longer raises alerts of its own. The `Notification` hook and `hooks/notify.sh` (the Glass sound on macOS, the `notify-send` desktop alert on Linux, the terminal bell) are gone, and so is the in-session toast of the alerts mod. The `ANTHOLOGY_NOTIFY` switch goes with them, since there is nothing left to turn off. The `/anthology-status` pane and its command stay. Any notification app you choose works alongside Anthology, or you can add a `Notification` hook of your own.

### [0.5.5] - 2026-11-26

- Add: licensing is the user's job. The routing guide and the researcher, architect, implementer, debugger, reviewer and ui-designer prompts now tell the lead and these agents never to refuse, hold back or audit licences when they use, adapt or recommend outside code. When a licence clearly applies, they say so in one line and carry on.
- Remove: the status row above the prompt (`● 2 running   ! 1 needs you   /anthology-status`) is gone. Nothing is drawn above the prompt any more. `/anthology-status` still opens the pane with the running agents, the last five that finished and the permission prompts that wait for you.
- Fix: on macOS the `Notification` hook no longer posts its alert through `osascript`. macOS attributes such a notification to Script Editor (running it inside `tell application id "<host>"` changes nothing), so clicking the alert opened Script Editor instead of your editor or terminal. The hook now plays the Glass sound with `afplay` and rings the terminal bell on macOS; the in-session toast is the visible alert. Linux alerts are unchanged.

### [0.5.4] - 2026-11-25

#### Changed

- The `anthology` binary 0.2.5: git is run only from an absolute `PATH` entry, never a relative
  one, when Anthology runs it inside a repo.
- Pin anthology-bin 0.2.5.
- Fix: the after-work check and the binary's own git calls (memory sync, session start) now look for `git` only on the absolute entries of `PATH` and run it by its absolute path. A `.`, empty or relative entry can no longer run a `git` planted in a repository.

### [0.5.3] - 2026-11-25

- Fix: when your PATH held a relative or empty entry (`.`, `::`, a leading or trailing `:`, or a folder such as `bin`), the repository you opened could supply programs that the plugin's hooks and the `anthology` and `anthology-mem` commands ran: the hooks' `sh`; the `curl`, `tar`, `shasum`, `date`, `readlink` or `dirname` used while finding and installing the binary; and the `cat`, `sed`, `grep`, `tr`, `uname` or notifier used by the atl, attribution and notification hooks. The hooks now start with `/bin/sh`, and these helpers run on PATH without those entries, keeping the absolute ones in order and falling back to the standard system folders when none is left. The anthology binary still gets the PATH you exported, unchanged; when you exported none, it gets the safe one. The sha256 check is unchanged.

### [0.5.2] - 2026-11-25

#### Changed

- The `anthology` binary 0.2.4 changes the session-start message for failing-closed guardrails:
  it now names the exact recovery command.
- Pin anthology-bin 0.2.4.
- Fix: `/anthology:setup` no longer tells the model to retry a command the guard blocks because its binary is missing. It runs the installer command the block names, which the guard lets through until the binary is installed.
- Fix: when the guard binary is missing and session start has no time left to run the self-test, the "guardrails are failing closed" message no longer says to run `/anthology:setup`, which needs the shell commands the guard is blocking. It now names the one command the guard lets through, `/bin/sh '<guardrails plugin folder>/hooks/bootstrap.sh' guard --plugin guardrails --quiet --budget 300`, with the real folder, and keeps the advice to start a new session while online or disable anthology-guardrails with `/plugin`.

### [0.5.1] - 2026-11-25

#### Changed

- The `anthology` binary 0.2.3 carries the Rust changes made since 0.2.2. The embedded-host
  rule now lets the attended flag (`CLAUDE_CODE_SESSION_ATTENDED`) decide first, in the guard
  and the session start alike. The stale-plugin restart notice compares a plugin only with the
  install entry of its own scope and orders release candidates by number. The guard's "too
  complex" (G0) message names its cause for three more shapes. No guard verdict changes. The
  entries that follow give the details.
- Fix: the agents' waiting rule now fits Claude Code's 180 s limit on a subagent's foreground Bash call. They block in `timeout 170` chunks (`gh pr checks --watch`, `gh run watch`, `docker wait`) instead of relying on the `timeout` parameter or `run_in_background`.
- Fix: the four writing agents read `.claude/memory/reference-project-map.md` in their working folder before calling the map missing, and quote the error when the read fails. Every agent's Memory section says its notes are the repo's `.claude/agent-memory/<agent>/`, never `~/.claude`.
- Fix: the after-work check counts a report that says the project map is "missing" as dealing with the map only when the file is really absent in the agent's working folder.
- Pin anthology-bin 0.2.3.
- Fix: an interactive `claude` started from inside an SDK process no longer loses its guide, memory and coordinator section. The attended flag (`CLAUDE_CODE_SESSION_ATTENDED`, `0` or `1`) now decides when it is set; an `sdk-*` entrypoint counts only when the flag is unset or holds another value. `ANTHOLOGY_HOST_MODE=full` still wins. The delegation mod, the session start and the G0 message follow the same rule.
- Fix: the stale-plugin restart notice compares the loaded plugin only with the install entry of its own scope, so a project install pinned older than the user one no longer asks for a restart.
- Fix: the stale-plugin notice orders release candidates of one version by number (`0.2.9-rc.1` is older than `0.2.9-rc.2`), and still treats two build tags (shas) as equal.

### [0.5.0] - 2026-11-24

#### Added

- `anthology-mem save --repo` writes a repo note with the same privacy and secret checks as an
  agent note, and never a user note, so the lead can save repo notes through the tool instead of
  by hand. `anthology-mem reindex --agent|--repo|--all` rebuilds a `MEMORY.md` after a merge
  conflict without hand edits. Both refuse to use a home folder that is itself a git repo. The
  change ships in the `anthology` binary 0.2.2.

#### Changed

- Every agent prompt now carries one shared Shell section: the zsh traps, one plain command per
  call, literal deletes, the rules for waiting and background jobs, durations taken from
  timestamps, and settings keys named in reports. Writers append to notes with Edit and use
  `anthology-mem reindex --agent <name>` for a conflicted `MEMORY.md`. Agents read `MEMORY.md`
  only if it exists.
- Lead rules updated. Stop means TaskStop every agent and background job. Waiting notices are
  ignored, and idle agents with startable work are a bug. Conflicts in any file go to the
  implementer. The lead saves repo notes with `anthology-mem save --repo`. The `large-builds`
  skill now has writers report at push, with one background watcher that sends red CI back, and
  it adds the status cadence, estimates that are measured or "unknown", the `release.sh` and
  restart rules, and the path and ledger rules.
- The version-bump gate compares a released plugin with its latest release tag (tags created in
  the pushed range are ignored) and with the merge base. A bump already on the base counts, so
  branches no longer re-bump the plugin and conflict. See ADR-036.
- `scripts/release.sh` tags the release commit first, then pushes the branch and the tag
  together with `git push --atomic`. A failed or interrupted run deletes the local tag and
  prints the recovery command. Untracked agent notes no longer block a release. The last line
  is `RELEASE OK <tag>` or `RELEASE FAILED: <step>`. Changelog fragments in
  `changelog.d/<plugin>/*.md` are compiled into the release and deleted in the release commit.
  The script is now shellchecked.
- CI has an informational `lock-fresh` job that warns when a pull request makes the pinned
  `anthology-bin` binary stale. It never fails the PR. The version-bump comment now names the
  released tag, and a test pins the fuzz lock check (`cargo metadata --locked` in `fuzz/`).
- The README is now a short front page. The details live in `docs/` (setup, agents, mods,
  guardrails, ATL, memory, the binary), and the original build plan moved to
  `docs/history/PLAN.md`.
- shellcheck now also checks `ci/jobs/lock-fresh.sh` and `scripts/rust-test.sh` (the latter as
  bash).
- `scripts/rust-test.sh test -p <crate>` no longer adds `--workspace`, so a one-crate run tests
  only that crate, plain and with all features.
- The agent status is now a row Anthology draws above the prompt
  (`● 2 running   ✓ 3 done   ! 1 needs you`, with `/anthology-status` on the right) instead of
  a labelled status entry. It shows only while an agent runs or a prompt waits for you.
- Session start: an embedded host (SDK entrypoint, or unattended session, unless
  `ANTHOLOGY_HOST_MODE=full`) gets no routing guide or memory, and only the guard self-test and
  bootstrap failure lines. The setup nag and the settings-changed note show only at startup. A
  resumed or compacted session names an anthology plugin older than the installed one. The
  release smoke runs in full host mode. See ADR-037.
- Strict delegation and the coordinator section no longer apply in an embedded SDK host
  (unattended, or an `sdk-*` entrypoint). There the lead's `Edit`, `Write` and `NotebookEdit`
  calls are not refused, even with a surface attached. `ANTHOLOGY_HOST_MODE=full` keeps both.
  See ADR-037.

#### Fixed

- `/anthology:setup` run from a folder that holds repos but is not one itself now lists the
  repos below it. The skill asks which one and reruns the preview and the apply with `--cwd`
  (the path in single quotes; odd names are refused), so the repo's memory, attribution and
  auto memory settings are no longer skipped silently. The preview's `memory.repo` carries the
  list as `repos_below` and `repos_below_more`, and the change ships in the `anthology` binary
  0.2.2.
- The debug-build timing bound in the ATL megabyte-line gate test is now a ratio to the same
  line at 250 KB, so slow CI runners no longer fail it. This changes a test only.
- Pin anthology-bin 0.2.2.

### [0.4.1] - 2026-11-23

#### Added

- Automatic after-work check (a Claude Code mod, `hooks/mods/afterwork.ts`): when an implementer, debugger, docs-writer or ui-designer reports to the main session, the report is checked for a commit, test evidence, stray attribution lines and uncommitted files in the agent's worktree, and a short "After-work check" block naming each gap is appended. The project map and leftovers are reminders only: they ride in a block a gap raised and never raise one. The agent's text is never changed, and a report with no gap gets no block. Set `ANTHOLOGY_AFTERWORK=0` (or `ANTHOLOGY_MODS=0`) to turn it off.
- Strict delegation, a Claude Code mod (`hooks/mods/delegation.ts`, Claude Code 2.1.330 or later), on in interactive
  sessions only. The system prompt gets a short coordinator section when the session has the
  `Agent` tool, and the main session's `Edit`, `Write` and `NotebookEdit` calls are refused with a
  message that says why and names the agent to use (`implementer`, `debugger`, `docs-writer` or
  `ui-designer`). Allowed: the lead's memory (auto memory, `autoMemoryDirectory`, the repo's
  `.claude/memory/`, personal memory), `~/.claude/plans/`, `.superpowers/` and
  `docs/superpowers/plans/`, and a tiny fix (an `Edit` of at most 5 lines and about 2 KB, at most 3
  per turn, in one file; a failed edit does not count). Subagent calls, headless runs and sessions
  with no `Agent` tool are never touched. Set `ANTHOLOGY_DELEGATION=0` (or `ANTHOLOGY_MODS=0`) to
  turn it off. Not yet built: the gate on MCP tools that are not read-only and the
  `scan_secrets` allowlist. The plugin manifest names a `types` contract (`types/anthology.d.ts`)
  for the mod's `$.state` values.
- Alerts and a live status pane, as a mod (`hooks/mods/alerts.tsx`). When Claude Code needs
  you (a permission prompt or an idle wait) the mod shows a one-line toast, at most once every
  20 seconds; it makes no sound, and the shell alert keeps the only sound and stays as the
  fallback. `ANTHOLOGY_NOTIFY=0` turns the toast off. `/anthology-status` opens a pane with the
  running agents, the last five that finished and the pending permission prompts, and a status
  line under the prompt keeps the count. The pane and toast never show a command, file path or
  URL. `ANTHOLOGY_MODS=0` turns it all off. Not yet: the design's open pull requests and CI
  block (it needs `gh`). The plugin now requires Claude Code 2.1.330 or later.
- Permission levels: a Claude Code mod (`hooks/mods/`, Claude Code 2.1.330 or later) lets a
  shell command run without a prompt when the installed `anthology perm classify` proves it
  only reads and Claude Code allows a `Read` of every path it reads. It changes only a plain
  ask that a person answers in the dialog. Denies, rule, hook, mod, setting and organization
  asks, headless, background and `dontAsk` sessions, SDK hosts, auto mode's classifier and
  sessions with `allowManagedPermissionRulesOnly` stay exactly as Claude Code decides them. It
  never downloads the binary. Set `ANTHOLOGY_MODS=0` to turn it off.

#### Changed

- The plugin requires Claude Code 2.1.330 or later (`engines`), because of its Claude Code mods.
  The mods load as one module (`hooks/mods/index.ts`). An event that two mods hook (a writer's
  `agent.report`: the after-work check and the status pane) is registered once there, and each mod's
  part runs on its own, so a failure in one never drops the other's result.
- pin anthology-bin 0.2.1.

#### Fixed

- The 10 s limit on `git` (memory sync and session start) now also covers starting git. On
  macOS, when two threads of one process started a program at the same moment, one could
  inherit the other's pipes and the first `git` start then waited for that program to end,
  with no limit (the `a_hung_git_is_killed_at_the_timeout` test saw a 30 s wait on a CI
  runner). Starts now take turns, and a start that is still stuck at the deadline is
  abandoned: the call returns no result at the limit. Normal git calls are unchanged. The
  `anthology` binary is 0.2.1.

### [0.3.1] - 2026-11-23

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

### [0.2.18] - 2026-11-27

- Pin anthology-bin 0.2.9.
- Fix: the guard closes three known bypasses of its Bash check. A command that runs another program through an option the guard did not model (`rg --pre`, `rg --hostname-bin`, `grep --filter` or `--pager` in ugrep, `sort --compress-program`, `LESSOPEN`, `LESSCLOSE`), an escaped backtick inside a zsh arithmetic subscript, and an unquoted zsh glob group such as `(a|b)` among a command's words are now blocked as too complex to check. Editors, `RIPGREP_CONFIG_PATH`, `ugrep --config` and an awk program that runs commands are treated the same way. Everyday `rg`, `grep`, `ugrep`, `sort` and `awk` commands still run without a prompt. Needs the `anthology` binary 0.2.9.

### [0.2.17] - 2026-11-27

- Pin anthology-bin 0.2.8.

### [0.2.16] - 2026-11-27

- Fix: `hooks/lib.sh` (shared with `anthology`) words the `anthology` plugin's "could not install the anthology binary" session-start line so it says the guard, which runs the same binary, blocks shell commands, file edits and MCP tools too. The guardrails' own session-start line, which names the installer command, is unchanged.

### [0.2.15] - 2026-11-25

- Pin anthology-bin 0.2.5.

### [0.2.14] - 2026-11-25

- Fix: when your PATH held a relative or empty entry (`.`, `::`, a leading or trailing `:`, or a folder such as `bin`), the repository you opened could take over the guard. Claude Code looked up the hooks' `sh` on that PATH, so a planted `./sh` could replace the guard and allow every tool call. The guard's installer could also run a planted `curl`, `tar`, `shasum` or `sh`. The hooks now start with `/bin/sh`, and they drop those PATH entries for their own helpers, keeping the absolute ones in order and falling back to the standard system folders when none is left. The guard binary still gets the PATH you exported, unchanged; when you exported none, it gets the safe one. The sha256 check and the fail-closed behaviour are unchanged.

### [0.2.13] - 2026-11-25

- Pin anthology-bin 0.2.4.

### [0.2.12] - 2026-11-25

- Fix: on a slow network the guard binary never installed and the session stayed blocked. Each tool call gave the download 20 seconds and then started again from zero, so an archive that needed longer never arrived. The download now runs in the background with a 10-minute budget, starting at session start, and a stopped download continues from what it already got (with `curl`). A blocked call waits up to 20 seconds for it and then says how many bytes have arrived.
- Fix: a missing guard binary no longer locks the session. The block named `/anthology:setup`, which needs the shell commands the guard was blocking. While the binary is missing, the guard now lets exactly one shell command through: the plugin's own installer, `/bin/sh '<plugin folder>/hooks/bootstrap.sh' guard --plugin guardrails` (optionally with `--quiet` and `--budget N`), with the path matched exactly. Every `(setup)` block and the session-start message name that command with the real folder. Anything else is still blocked.
- The block now says why the install failed: still downloading, the download timed out (slow network), no network, or a sha256 mismatch with `anthology.lock`.

### [0.2.11] - 2026-11-25

- Pin anthology-bin 0.2.3.
- The guard's "too complex" (G0) message now names its cause for three more common shapes: a shell fed by another command's output (`curl ... | sh`), a shell whose heredoc or here-string is only a command substitution, and a command substitution used as the program word (`$(echo ls) a`). The reason the analysis stopped now comes before a mark on an earlier command. No verdict changes.
- In an SDK-embedded host, the guard's limit and panic lines no longer tell the reader to use `/plugin`, as the G0 line already did.
- Fix: the guard no longer takes an interactive `claude` for an embedded host when it inherited an `sdk-*` entrypoint from an SDK process. `CLAUDE_CODE_SESSION_ATTENDED` decides when it is `0` or `1`; the `sdk-*` entrypoint counts only when the flag is unset or holds another value. `ANTHOLOGY_HOST_MODE=full` still wins. Only the text of the G0 message changes (the `/plugin disable` line); verdicts are the same.

### [0.2.10] - 2026-11-24

#### Added

- The guard reads a `"$NAME"` program or script word as the file it names when you list NAME in
  `ANTHOLOGY_GUARD_PATH_VARS` (opt-in; see ADR-038 and `SECURITY.md`).

#### Changed

- The guard's too-complex (G0) message now names its cause when it can (a program from a
  variable or glob, a heredoc or piped shell script, a sourced file or script), and drops the
  `/plugin disable` line when Claude Code runs inside an embedded host. See ADR-037.
- Pin anthology-bin 0.2.2.

### [0.2.9] - 2026-11-23

#### Changed

- pin anthology-bin 0.2.1.

### [0.2.8] - 2026-11-23

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

### [0.2.3] - 2026-11-24

- Fix: the session-start hook gives an interactive `claude` its introduction even when it inherited an `sdk-*` entrypoint from an SDK process. `CLAUDE_CODE_SESSION_ATTENDED` decides when it is `0` or `1`; the `sdk-*` entrypoint counts only when the flag is unset or holds another value. `ANTHOLOGY_HOST_MODE=full` still wins.

### [0.2.2] - 2026-11-24

#### Fixed

- The session-start hook no longer injects the `using-superpowers` bootstrap in an embedded
  host (an unattended session, or an `sdk-*` entrypoint), unless `ANTHOLOGY_HOST_MODE=full`.
  An app that drives Claude Code owns its prompt, so the introduction had no reader there. See
  ADR-037.

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
