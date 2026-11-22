# Anthology

A plugin marketplace for Claude Code that gives every project a roster of agents with fixed models and their own knowledge, guardrails, condensed command output and memory that lives in the repo.

## What it is

Anthology is opinionated and preconfigured. Install it and follow its rules: there are no modes, levels or toggles to set. macOS and Linux only.

It ships three plugins. Installing `anthology` brings the other two with it:

| Plugin | What it is |
|---|---|
| `anthology` | The agent roster, their knowledge, the three user commands, the memory tools and rtk |
| `anthology-guardrails` | The hook that blocks the things agents must not do (see [Guardrails](#guardrails)) |
| `anthology-superpowers` | The superpowers skills for the main session, renamed so they do not clash with the official copy |

More: [SECURITY.md](SECURITY.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [DECISIONS.md](DECISIONS.md) · [LICENSE](LICENSE) (Apache-2.0). Each plugin ships its license text, and each vendored folder ships its upstream license (and NOTICE, where upstream has one).

## Install

1. Add the marketplace:

   ```text
   /plugin marketplace add rbonweb/anthology
   ```

   From a terminal, the same step is `claude plugin marketplace add rbonweb/anthology`.

2. Install the plugin. This also installs `anthology-guardrails` and `anthology-superpowers`:

   ```text
   /plugin install anthology@anthology
   ```

   From a terminal: `claude plugin install anthology@anthology`.

3. Turn on auto-update: run `/plugin`, open the **Marketplaces** tab, select `anthology` and enable auto-update. Auto-update is off by default for every marketplace that is not Anthropic's own, so without this step you only get new versions when you update by hand (see [Updating](#updating-and-removing)).
4. Run `/anthology:setup` in each repo you want memory in. The first run also does the one-time machine setup.

Check [Requirements](#requirements) first. The first session after you install downloads the plugins' `anthology` program; until it is installed, the guardrails block shell commands, file edits and MCP tools (see [The `anthology` binary](#the-anthology-binary)).

### The `anthology` binary

Both `anthology` and `anthology-guardrails` run one static program, `anthology`, written in Rust. It needs no language runtime on your machine.

- **Where it comes from.** This repo's `anthology-release.yml` workflow builds and tests it on native runners and publishes it on the GitHub release tagged `anthology-bin--v<version>`: one `anthology-<target>.tar.gz` for each of macOS and Linux on arm64 and x86_64, plus `checksums.txt`. Each archive has a build-provenance attestation: `gh attestation verify anthology-<target>.tar.gz --repo rbonweb/anthology`.
- **Pinned.** Each of the two plugins ships the same `anthology.lock`: the version, the release tag and a sha256 for each target's archive. It never means "latest": a new binary arrives only with a plugin update, which installs the new version beside the old one.
- **Installed on first use.** At session start (or at the first tool call that needs it), the plugin's `sh` hook downloads the archive for your platform over HTTPS with `curl`, `wget` or `node`, checks its sha256 against `anthology.lock` before unpacking it, checks that the binary runs and reports the pinned version, and only then installs it in `~/.claude/anthology/bin/<version>/` (under `$CLAUDE_CONFIG_DIR` when that is set). Each session start re-checks the installed binary against the lock. A version that no lock pins and that has not been used for 30 days is deleted.
- **Offline or air-gapped.** On a machine with access, download the `anthology-<target>.tar.gz` that `anthology.lock` names (the URL is `https://github.com/rbonweb/anthology/releases/download/<tag>/anthology-<target>.tar.gz`), copy it into `~/.claude/anthology/bin/` and start a new session. It is checked against the same pin; a file that does not match is not used, and the message says so.
- **If it cannot be installed, the guardrails fail closed.** Read-only tools still work (`Read`, `Grep`, `Glob`, `WebFetch`, `WebSearch`, `Skill`, `Task`, `Agent`, the to-do and plan tools and the like), so `/anthology:setup` can still load. Shell commands, file writes and edits, MCP tools and any tool not on that list are blocked until the binary is installed. A blocked call tries the install again at most once a minute, so the session heals once the network is back. Until the binary is installed, the `anthology` plugin's routing guide, memory and rtk are off.

What you may see, and what to do (the text in parentheses names the cause):

| The message says | What to do |
|---|---|
| At session start: `anthology-guardrails: could not install the guard binary (…)` or `anthology: could not install the anthology binary (…)` | Start a new session while online, or run `/anthology:setup` |
| `Blocked by anthology-guardrails (setup): the guard binary is not installed (…)` | The same. Inside a session, waiting a minute and retrying also works |
| `… the last install attempt was less than a minute ago` | Retry in a minute, or run `/anthology:setup` |
| `… did not work (…) and was set aside` | Nothing: it is reinstalled on a later call (at most once a minute) |
| `… failed to run 3 times in 24 hours` | A new session tries once more, at most once an hour. Or disable `anthology-guardrails` until a fixed version ships |
| `… does not run on this machine` | A wrong architecture, or a home folder mounted `noexec`. Disable `anthology-guardrails`, fix the cause, start a new session |
| `… no downloader (curl, wget or node) was found` | Install one, or download the archive by hand: the message gives the URL and the folder |
| `… cannot be installed because <folder> is not writable` | Fix that folder's permissions, then start a new session |
| `… there is no guard binary for this platform` | Anthology supports macOS and Linux on x86_64 and arm64 only. Disable `anthology-guardrails` to use Claude Code here |
| `Blocked by anthology-guardrails (internal): …` | The guard itself failed or ran out of time on this input, and blocked it to be safe. Rephrase the command, or report it |

**The way out that always works:** run `/plugin` and disable `anthology-guardrails`. `/plugin` never goes through the guard, so it works even when every tool call is blocked, and every `(setup)` and `(internal)` message names it. Disabling it turns all the guardrails off, so turn it back on once the cause is fixed.

### What `/anthology:setup` does

It checks your machine, then shows every change and applies them only after you confirm once.

1. **Doctor.** Checks that the `anthology` binary the guardrails use is installed and matches their `anthology.lock`, that `node` and `docker` are on your path (and Docker is running), that BrowserOS Neo is installed or configured as an MCP server, the rtk binary and the platform. BrowserOS Neo is found as any `BrowserOS*.app` in `/Applications` or `~/Applications`, a `browseros` binary on your path, or an MCP server in any scope; a separate check says whether that server is named `browseros-neo`, because agents call `mcp__browseros-neo__*`. Doctor also checks the compatibility assumptions Anthology relies on: the plugin's `bin/` wrappers exist and (inside Claude Code) are on your path, the plugin data folder exists, and agent memory lands in `.claude/agent-memory/anthology-<agent>/`. If `CLAUDE_CODE_SUBAGENT_MODEL` is set, it says where (see "Agent models and `CLAUDE_CODE_SUBAGENT_MODEL`" below). After a first setup it also checks that the plugin-read permission (below) still matches where the plugin is installed. On Windows it refuses and stops.
2. **Duplicates.** If you have the official `superpowers` or `design-skills` installed, it recommends uninstalling them and shows the commands for you to run (see [Replacing the official superpowers and design-skills](#replacing-the-official-superpowers-and-design-skills)).
3. **Binaries.** Makes sure the `anthology` binary that `anthology-guardrails` pins is installed, with the same download and checks as at session start (see [The `anthology` binary](#the-anthology-binary)). Then it downloads the pinned rtk binary for your platform from this repo's releases, checks its sha256 against `rtk.lock` and installs it in the plugin's data folder. If the download or the check fails, nothing is installed and setup shows a warning.
4. **Settings.** Edits `~/.claude/settings.json`:
   - allows `Bash(rtk *)`. This also auto-approves `rtk bypass <cmd>` (see [rtk](#rtk));
   - allows `Bash(anthology-mem save *)`, so agents that cannot edit files can still save their own memory notes (see [Saving notes](#saving-notes-anthology-mem-save));
   - denies `Read(...)` on the Chrome and Chromium data folders and everything under them: `~/Library/Application Support/Google` and `.../Chromium` on macOS, `~/.config/google-chrome` and `~/.config/chromium` (with the `-beta`, `-dev` and similar channel variants) on Linux;
   - allows `Read(...)` on the parent folder of the plugin's install location, so agents can read their own knowledge files. Outside auto mode, Claude Code does not auto-approve those reads;
   - sets `worktree.baseRef` to `"head"`, so agents that work in a worktree start from your current commit instead of the remote's default branch;
   - only if you say yes to its own question: removes `CLAUDE_CODE_SUBAGENT_MODEL` from the `env` block (see "Agent models and `CLAUDE_CODE_SUBAGENT_MODEL`" below).
5. **Memory.** Approves the current repo for memory, which also adds two `merge=union` lines to the repo's `.gitattributes` (you can decline this step), and creates the personal-memory folder. If Claude Code kept notes for this folder before Anthology (the old path-keyed `~/.claude/projects/<encoded path>/memory/`), it lists each one with its diff and copies them all into personal memory, unless you pick some to move into the repo's memory instead. The default, and the recommendation, is none. Notes the privacy scan flags (home paths, emails, IP addresses, private host names, ssh targets and key paths, secrets) and `user` notes cannot be moved.

When it finishes, restart the session so the new permissions and the rtk hook take effect. Nothing is written to `~/.claude/CLAUDE.md`.

After setup, some things happen on their own at session start: Anthology delivers the routing guide, runs the guardrails self-test (see [Guardrails](#guardrails)), reconciles the rtk binary with `rtk.lock`, and in approved repos keeps memory working (see [Memory](#memory)). Until you have run `/anthology:setup` it prints a one-line reminder to do so, and it warns you if the plugin has moved so that agents can no longer read their knowledge files; re-run `/anthology:setup` to fix that. It also prints one loud line when a compatibility assumption breaks: Claude Code did not pass `CLAUDE_PLUGIN_DATA`, the plugin's `bin/` wrappers are missing or not executable, agent memory was written somewhere other than `anthology-<agent>/`, or `CLAUDE_CODE_SUBAGENT_MODEL` is set (run `/anthology:setup` to remove it). When a release changes the `settings.json` entries that setup makes and one of them is missing, it prints "anthology settings changed in this release — run /anthology:setup to review" and changes nothing itself. If you deleted one of those entries on purpose (for example `Bash(rtk *)`), the line returns at each session start after every such release, until you run setup again. If the personal-memory folder is a symlink or not a folder, it prints a line saying personal memory was not loaded and nothing was moved or copied.

## The product

### Model roles

The main session talks to you in normal prose and orchestrates. It does small tasks (a handful of steps) itself and delegates the rest.

| Model | Role | Good for | Not for |
|---|---|---|---|
| Opus | Lead, researcher, reviewer | Ambiguous problems, research, architecture, decomposition, root-causing unknown bugs, high-stakes review | Mechanical edits, bulk searching |
| Sonnet | Builder | Implementing a clear plan, multi-file edits, tests, scoped refactors, reproducible bugs, docs, UI builds | Deciding what to build |
| Haiku | Scout, runner | Finding code, summarizing output, running tests and reporting, renames, boilerplate | Anything needing judgment |

Models are the aliases `opus`, `sonnet` and `haiku`, so agents follow new models automatically.

### Agent models and `CLAUDE_CODE_SUBAGENT_MODEL`

`CLAUDE_CODE_SUBAGENT_MODEL` makes every agent run on one model. It overrides the `model` of each roster agent, so the Opus, Sonnet and Haiku roles above do not apply while it is set.

- **What setup offers.** `/anthology:setup` says where it is set, never its value. If it is in the `env` block of your user `settings.json` (the file setup edits), setup asks you separately whether to remove it, and recommends yes. The preview shows the removal as a diff with the value shown as `"<redacted>"`. Setup removes it only if you say yes.
- **Anywhere else** (your shell profile, the project's `.claude/settings.json` or `.claude/settings.local.json`, managed settings), setup leaves it alone and says how to remove it there. While it is still set in one of those places, the tiers do not apply. If it is set twice in your user `settings.json`, setup asks you to delete both lines by hand.
- **By hand.** Delete the `CLAUDE_CODE_SUBAGENT_MODEL` line from the `env` block of the settings file, or the line that sets it in your shell profile (such as `~/.zshrc`), then restart Claude Code.
- **Undo.** `/anthology:setup remove` puts back the value setup removed. If you have set it again since, your new value stays.

### The roster

Agents are called `anthology:<name>`. A routing guide, delivered to the main session at session start, tells it when to delegate and to whom. It never loads into the agents themselves.

| Agent | Model | Edits | Use it for | Knowledge it reads |
|---|---|---|---|---|
| `researcher` | opus | no | Research, competitor and user-research synthesis | competitive-brief, synthesize-research, research-synthesis, ux-research, journey-mapping |
| `architect` | opus | no | Architecture, system design, test strategy, plans | system-design, architecture, testing-strategy; preloaded: writing-plans |
| `implementer` | sonnet | yes, in a worktree | Building from a clear plan | karpathy-guidelines; preloaded: test-driven-development, verification-before-completion |
| `debugger` | sonnet | yes, in a worktree | Reproducible bugs | karpathy-guidelines; preloaded: systematic-debugging |
| `reviewer` | opus | no | Code review, tech debt, deploy readiness | code-review-checklist, karpathy-guidelines; tech-debt and deploy-checklist only when they apply |
| `scout` | haiku | no | Finding code, running tests, summarizing output | none |
| `docs-writer` | sonnet | yes, in a worktree | Documentation | documentation, ux-writing |
| `ui-designer` | sonnet | yes, in a worktree | Building UI | frontend-design, design-taste-frontend, ui-taste, landing-page, web-design-system, design-handoff, design-systems, interaction-design, design-elevation |
| `ui-reviewer` | opus | no | Reviewing the rendered UI in BrowserOS Neo | web-design-guidelines, web-review, design-critique, accessibility-audit |
| `product-planner` | opus | docs only | Specs, sprint plans, roadmaps, product brainstorming | write-spec, sprint-planning, roadmap-update, product-brainstorming, ux-strategy, design-ops |
| `marketer` | sonnet | docs only | SEO, campaigns, content, email sequences | seo-audit, campaign-plan, content-creation, email-sequence |

- **Knowledge belongs to agents.** Every skill we ship is read by at least one agent, and the main session never sees it. The only skills the main session has are `anthology-superpowers`, the three user commands and `anthology:large-builds`, the rulebook for long multi-agent builds. Each agent reads its own knowledge files before it starts anything else.
- **Builders and critics are different models.** For UI work, `ui-designer` (Sonnet) builds and `ui-reviewer` (Opus) checks.
- **For websites and landing pages** `design-taste-frontend` leads; **for app screens** `ui-taste` leads; `frontend-design` is the baseline.
- **No agent can call other agents,** so the main session stays the single orchestrator. In the Edits column, "no" means the Edit, Write and NotebookEdit tools are turned off for that agent. It can still run Bash, which is how it saves its own memory notes. "Docs only" for `product-planner` and `marketer` is an instruction in the agent, not a tool restriction.
- **Every agent has project memory** (see [Memory](#memory)).
- **Security work** goes to the `claude-security` plugin. There is no Anthology agent for it.

### House style

Agents write in one fixed terse style in chat and reports. Files, code, commits and docs stay in normal prose. There is no switch.

### Commands

| Command | What it does |
|---|---|
| `/anthology:setup` | Checks your machine, shows the changes above as diffs and makes them after you confirm |
| `/anthology:setup remove` | Takes out the settings entries and the rtk download that setup added; never deletes your notes (see [Updating and removing](#updating-and-removing)) |
| `/anthology:export` | Exports your memory notes to a portable archive |
| `/anthology:import <file>` | Imports an archive after a dry run |

These run only when you ask for them.

### Working rules the routing guide sets

- **Worktrees.** Writers use Claude Code's built-in isolation, never `git worktree add`. Writers commit before they finish, and the main session reviews and merges. Parallel writers get separate files.
- **Docker.** Tests, builds and end-to-end suites run in containers that mount only the current worktree. Agents do no host-level installs (`brew`, `npm -g`, global `pip`, browser downloads). Project-level installs such as `npm install` are fine. If Docker is unavailable, the agent stops and tells you.
- **Blocked agents.** A subagent cannot ask you a question. It stops and reports it, and the main session answers or asks you, then resumes it.
- **Coding and long builds.** Code follows the Karpathy guidelines (think first, simplest fit, surgical diffs, verify); the implementer, debugger and reviewer read them at startup. For long or multi-agent builds, status reports and decisions made while you are away, the main session loads `anthology:large-builds`: plain feature-based status reports (done, running, blocked, an honest estimate), and each decision recorded as `Ruling: what — why — cost if wrong`, while still stopping for irreversible, security-sensitive or outward-facing actions.

### Guardrails

Two layers keep agents from installing browser tooling on your machine, touching your Chrome, using any browser other than BrowserOS Neo, or writing secrets into memory.

1. **Deny rules.** Setup adds `Read(...)` deny rules for the Chrome and Chromium data folders to `~/.claude/settings.json`. They cover Claude Code's built-in file tools and the Bash file commands it recognises, and a hook cannot override them.
2. **A hook.** `anthology-guardrails` runs a `PreToolUse` hook on every tool call. It blocks with a message that names the alternative.

| ID | Blocks | Points the agent to |
|---|---|---|
| G1 | Host browser-automation installs and runs: `playwright install\|test\|codegen\|open`, puppeteer installs, `pip install playwright`, `brew install chromium\|google-chrome\|playwright` | Docker; BrowserOS Neo for browser work |
| G2 | Launching Chrome or Chromium on the host, and any `--remote-debugging-port` | BrowserOS Neo |
| G3 | Any tool touching a Chrome or Chromium profile folder or its parent | Nothing: profiles are off-limits |
| G4 | Any `agent-browser` command or install | BrowserOS Neo MCP tools |
| G5 | MCP tools whose names contain `playwright`, `puppeteer`, `chrome-devtools` or `claude-in-chrome` | BrowserOS Neo MCP tools |
| G6 | Secret-looking content written into `.claude/memory/` or `.claude/agent-memory/` | Keep secrets out of memory |

Commands inside `docker` or `docker-compose` are exempt from G1 and G2 only. Rules match commands, not arbitrary text: `grep playwright package.json` and `git commit -m "drop playwright"` are allowed.

If the guard cannot run (its binary is missing, broken or cannot be installed), it fails closed: read-only tools still work, everything else is blocked with a message that says why and names the fix (see [The `anthology` binary](#the-anthology-binary)), and session start says so loudly.

#### Known limits

The guard is built to stop accidental browser installs and launches by cooperative agents. It is a hook plus deny rules, not a sandbox or a jail, and it does not defend against deliberate obfuscation. Known limits:

- Commands built inside variables or arrays and then run are not seen for what they are.
- A command whose text comes from another command's output and is fed to a shell is not looked into: `bash -c "$(…)"`, `eval`, `<(…)`, `source <(…)`.
- Unusual nested shell syntax can hide a command, such as a comment or a `case` inside `$(…)`, or a quoted `}` inside `${…}`.
- Free-text tool fields (an agent prompt, todo text) and most MCP tool arguments are not checked for profile paths. Inside interpreter code (`python3 -c`, `node -e`, a heredoc fed to `python3`, a BrowserOS Neo `run` script) only profile paths spelled out in one string are seen: a path assembled piecewise (`Path.home() / "Library" / …`, variables) is not, and neither is a command run from inside the code (`os.system(…)`).
- Browser profile protection covers the known profile folders and their parents. Grandparent folders (for example `find ~ -name Cookies`) and the profile folders of snap and flatpak browsers are not covered. The native `Read` deny rules that setup adds are the second layer for browser profiles.
- A hook sees the command Claude runs, not what a script does inside. A script that launches Chrome internally is invisible to it. That is why the knowledge we ship never launches Chrome.
- Secret detection covers what is written in the command text, `Write` and `Edit`. Copying a secret **file** into a memory folder is not caught.
- The guard ignores malformed input rather than blocking. If it cannot run at all, it blocks every tool call that is not read-only until its binary is installed, and `/plugin` can always disable it.
- While its binary is missing, the read-only tools it lets through (`Read`, `Grep`, `Glob`, `WebFetch` and the like) are not checked for browser profile paths. The native `Read` deny rules that setup adds still apply to them.

See [SECURITY.md](SECURITY.md) for the full threat model.

### BrowserOS Neo is the only browser

All real-browser work goes through BrowserOS Neo, over its MCP tools (`mcp__browseros-neo__*`): signed-in work, forms, dashboards, local dev-server checks, UI review. Agents share the browser and each uses its own tab. You install BrowserOS Neo; agents never do. If it is unavailable, the agent stops and asks.

Nothing we ship launches Chrome. Browser test suites that a project already has (for example Playwright tests) still run, but only inside Docker.

## rtk

rtk condenses the output of common dev commands, so the main session and every agent spend fewer tokens on `git status`, test runs and the like. A `PreToolUse` hook on Bash runs `rtk hook claude`, which rewrites commands (`git status` becomes `rtk git status`).

- **Always on.** There are no options. Output is condensed, and the routing guide tells Claude to treat it as complete. Exact text stays exact for `cat`, `grep -o` and `grep -c`, `jq -r` and `curl -o` and `curl -w`. `rtk bypass <cmd>` runs a command unfiltered, and the guide says to use it only when a result is unusable.
- **Claude Code only.** The rtk source lives in this repo under `rtk/`. It carries no integration for any other AI tool, and it has no telemetry. A CI test runs the built binary with the network switched off.
- **Apache-2.0 edition.** Upstream rtk also contains a BUSL-1.1 compression engine; Anthology removes it from the source it ships, so what you install is the Apache-2.0 edition and the engine-only features (compressing output after a tool has run, user-defined pipelines and `rtk tasks`) are not part of it.
- **Verified binaries.** Setup downloads the binary for your platform from this repo's public GitHub releases (tag `rtk--v<version>`) over HTTPS. `plugins/anthology/rtk.lock` pins the version and a sha256 for each of the four targets (macOS and Linux, arm64 and x86_64), and the binary is installed only when the hash of the downloaded archive matches. Each release asset has a build-provenance attestation you can check with `gh attestation verify`. At every session start Anthology reconciles the installed binary with the lock.
- **Local history.** rtk keeps a SQLite history on your machine, never uploaded: every Bash command line the hook sees and the rtk command it became, with token counts (for `rtk gain`), for 90 days; and, for `rtk recall`, the output it left out of a result, for 30 days. Command lines can contain secrets (`TOKEN=…`, `Authorization:` headers). Anthology keeps it in `~/.claude/anthology/rtk/` (`history.db`, `recall.db`, `tee/`; files are private to you), and `/anthology:setup remove` deletes that folder. To clear it sooner, delete the folder: that is the only way to remove the command lines from disk. `rtk gain --reset --yes` only zeroes the `rtk gain` numbers; the text of deleted rows can stay in the database file until it is overwritten. rtk has no switch that stops the history but keeps `rtk gain`. Version `1.2.1-anthology.1` wrote to rtk's default folder instead (`~/Library/Application Support/rtk/` on macOS, `~/.local/share/rtk/` on Linux); remove does not touch that folder, because another rtk install may use it, so delete it yourself if you ran that version.
- **File reads stay as typed.** The hook does not rewrite `cat`, `head` or `tail` into `rtk read`: on real use that saved about 3% of what was read, and it changes the text Claude reads. `/anthology:setup` shows, from this history, how many commands went through rtk and how many tokens it saved (`rtk_savings`).
- **Fails safe.** If the binary is missing, the hook exits 0 and commands run unchanged. The `rtk` command on your Bash path says "run /anthology:setup" if setup has not run.

### The `Bash(rtk *)` risk

Setup adds a blanket `Bash(rtk *)` allow so rewritten commands do not prompt (unless one of your deny or ask rules is involved, below). Because `rtk bypass <cmd>` runs any command, **this also auto-approves `rtk bypass <anything>`**. That is an accepted risk, chosen for a prompt-free workflow. What still holds:

- **Your deny and ask rules.** rtk's hook reads the `permissions` of your settings files (user, project, local and managed `managed-settings.json`) and applies them to the command and to the command an `rtk …` form wraps (`rtk bypass`, `run`, `err`, `test`, `summary`, `rtk git …`, and so on). A matching deny rule blocks the call (a launcher the hook does not model, such as `caffeinate git push`, is left to Claude Code's own matching). On a line that mentions `rtk`, a matching ask rule makes Claude Code prompt, even under `Bash(rtk *)`. On other lines rtk's ask is dropped, because a hook ask would prompt in bypass and auto mode too, so only Claude Code's own, shallower matching applies: it does not see `git -C . push`, `bash -c '…'` or `find -exec`. Use a deny rule for anything that must never run. A `Read(…)` deny rule blocks `cat`, `head`, `tail`, `grep`, `rg` and `rtk read` of a denied path.
- **Commands, not what programs do.** The hook aims to match Claude Code's own rule matching and add to it: a deny rule, and an ask rule on a line that mentions `rtk`, is enforced against the commands the agent types (every `rtk` form, wrapper, compound command, substitution and redirect), and a `Read(…)` rule against every file the line names or redirects. What an allowed program then does with files, plugins, configs or build steps it loads (`build.rs`, test files, MSBuild, the linker, plugins, a kubeconfig `exec`, compose files) is out of scope: deny rules restrict commands, not what programs do.
- **The plain shape once you have a rule.** With any deny or ask rule, a line that runs `rtk` goes through without a prompt only when it is one plain command from a short list: `rtk ls`, `tree`, `read`, `wc`, `diff`, `grep`, `rg`, `find`, the everyday `rtk git` reads plus `add` (without `-f`), `commit`, `checkout` and `switch`, and `rtk cargo build`, `test`, `check`, `clippy`, `fmt` (nothing after `--`), each with listed options only and literal paths checked against your `Read(…)` rules. A pipe, `&&`, `;`, a redirect, a substitution, a variable, a glob, `cd` in front, a word starting with `@`, a `:` in a git word other than a commit message, `rtk bypass`, and every program that loads configuration or runs code a flag names (kubectl, docker, dotnet, go, npm, npx, pip, uv, pytest, prettier, rspec, gh…) make it ask; so does a rule's command text anywhere in the line, however it is quoted. rtk rewrites only into that shape and otherwise leaves your command as written. With only a `git push` deny, 54 of 124 common agent commands probed prompt (mostly `rtk` forms typed directly) and 26 keep their original text instead of an rtk rewrite; `rtk git status`, `rtk ls`, `rtk cargo test`, `rtk grep` and `rtk read` do not prompt. Without any rule nothing changes.
- **Not covered.** Rules given only with `--settings` or `--disallowedTools`, server-managed settings and a macOS MDM profile never reach a hook, so they are not applied to the wrapped command. Neither are files a recursive search walks into (`grep -r`, also when `rg -g` or `--include` picks them); files, links or settings the same command creates (`ln -s . d && cat d/secret.txt`); what a plain command runs from files (the tests and build scripts `cargo test` builds, git hooks and config), which is out of scope as above; or git output that prints a `Read`-denied file's content without naming it (`git show HEAD`, `git log -p`, `git diff`). Code that hides a command from a parser (a `python -c` script, `eval` of a variable) is outside every rule, as it is without rtk; behind `rtk` it makes the hook ask when you have rules. Commands that match no rule run without a prompt.
- **The guardrails** check the command the agent asked for (see [SECURITY.md](SECURITY.md#guardrail-limits)).

`/anthology:setup remove` takes the allow out again. If you are not comfortable with it, do not run setup, or run `remove` afterwards.

## Memory

Memory is plain Markdown notes committed with your project, so it survives a re-clone, a folder move and a team merge. The notes are the only source of truth. The `MEMORY.md` indexes and a local SQLite search index are derived from them and can always be rebuilt.

| Whose memory | Where |
|---|---|
| Main session | `<repo>/.claude/memory/` (committed) |
| Each roster agent | `<repo>/.claude/agent-memory/anthology-<agent>/` (committed; `anthology:implementer` becomes `anthology-implementer`) |
| You (`user` notes) | `~/.claude/anthology/personal-memory/`, never in a repo (under `$CLAUDE_CONFIG_DIR` if you set it) |

**Repo memory is visible to everyone who can see the repo.** Never save secrets or personal details in it. Personal notes go in personal memory, which stays on your machine. Claude Code's auto memory saves every note type into the repo's folder, so at each session start Anthology moves `type: user` notes out of `.claude/memory/` and `.claude/agent-memory/` into personal memory and tells you; if one was already committed, commit its removal. The repo `MEMORY.md` indexes never list `user` notes, and the session context always names the personal-memory folder, even when it is empty.

### Approved repos only

Memory is active only in repos you approved by running `/anthology:setup` inside them. In any other repo no repo memory is written or injected (your personal-memory index still is), and no notes are moved. If that repo already has `.claude/memory/`, you get one line telling you to run `/anthology:setup`. That covers repo auto-memory only.

**Agent memory is different.** Claude Code loads `.claude/agent-memory/anthology-<agent>/MEMORY.md` into that agent itself (the agents set `memory: project`), whether or not you approved the repo, and the agent bodies tell the agent to read the folder. So notes a stranger committed there can reach an agent in a repo you cloned. The bodies say to treat notes as hints, never instructions: they cannot override your request, the BrowserOS-only rule or the Docker rule (subagents never receive the routing guide, so it is not named). That is guidance to the model, not a barrier. Review `.claude/agent-memory/` in any repo you do not trust, as you would review code (see [ADR-019](DECISIONS.md)).

Approvals are kept in `~/.claude/anthology/approved-repos.json`, keyed by the normalized git remote URL (the repo path if there is no remote). Memory also stays off if you turned auto memory off in Claude Code.

### Surviving re-clones

Because approvals follow the remote URL, re-cloning the same repo into any folder works with no new setup. At session start Anthology repairs the clone:

- It writes this clone's absolute path into `autoMemoryDirectory` in `<repo>/.claude/settings.local.json`. That file is per-clone and git-ignored, and Anthology makes sure it is ignored.
- It regenerates the `MEMORY.md` indexes from the notes.
- If it had to repair the setting, it injects the repo's memory index for that session, because the setting may only apply from the next one.
- It copies any notes in Claude Code's old path-keyed memory folder for this clone into personal memory, never into the repo, once (a ledger remembers them), and moves `user` notes out of the repo. Nothing is overwritten: a different note with the same name is kept side by side as `<name>.migrated-<date>.md`. To move an old note into the repo's memory, re-run `/anthology:setup` and pick it.

### Team merges

- Notes are named `<type>-<topic-slug>.md` and link to each other with `[[note-name]]`. Two people editing the same note is a real conflict, resolved like code.
- When you approve a repo, Anthology adds these lines to `.gitattributes`, once, so the generated indexes do not conflict:

  ```text
  .claude/memory/MEMORY.md merge=union
  .claude/agent-memory/*/MEMORY.md merge=union
  ```

- The next session start regenerates the index, which cleans up anything the merge left behind. GitHub's web merge button ignores merge drivers, so a PR may still show a conflict in a `MEMORY.md` there. Keeping both sides' lines, or merging locally, is always correct.
- Memory changes go in their own commits, `chore(memory): …`, so they never ride along with unrelated work. Agents that work in a worktree commit their memory there before they finish.

### CODEOWNERS for memory

Notes load into everyone's sessions, so review them like code. In your own repo, add owners for the memory folders:

```text
# .github/CODEOWNERS
/.claude/memory/        @your-org/reviewers
/.claude/agent-memory/  @your-org/reviewers
```

### Searching: `anthology-mem`

`anthology-mem` is on the Bash path and works over a local index in the plugin's data folder (never committed).

```text
anthology-mem search "<terms>"          full-text search over the note bodies
anthology-mem related <note> --depth N  notes linked to a note, to a depth
anthology-mem orphans                   notes nothing links to
anthology-mem stale                     superseded or old notes
anthology-mem rebuild                   delete and rebuild the index
```

Links are the `[[note-name]]` links in the notes, plus relations such as `supersedes` and `depends_on` declared in a note's frontmatter. There are no embeddings, so nothing needs an API key or a model download.

### Saving notes: `anthology-mem save`

Roster agents save their notes through one command, so a read-only agent needs no Write tool, heredoc or redirect:

```text
anthology-mem save --agent <name> --type <project|feedback|reference> --slug <topic-slug> \
  --name '<title>' --description '<one line>' --body '<text>' [--replace]
```

It writes `<repo>/.claude/agent-memory/anthology-<name>/<type>-<topic-slug>.md` with the frontmatter the indexes read, then regenerates that folder's `MEMORY.md`. It writes nowhere else: the agent must be on the roster, the type and slug are checked, symlinked folders or files are refused, an existing note is only overwritten with `--replace`, and a note that looks like it holds a secret (or the value of a `*TOKEN*`/`*KEY*`/`*SECRET*`/`*PASSWORD*` environment variable) or that the privacy scan flags (home paths, emails, IP addresses, private host names, ssh targets and key paths) is refused, with the kind and line named. Setup allows exactly `Bash(anthology-mem save *)`; Claude Code still asks before a command that adds anything to it (`&&`, a redirect, `$(...)`). A body line that starts with `#` also makes Claude Code ask, so notes use **bold** lines instead of `#` headings.

### Export and import

- `/anthology:export` writes `anthology-memory-<date>.tar.gz` under `~/.claude/anthology/exports/` by default. It holds your personal notes plus the current repo's notes, and a manifest. Nothing is uploaded. The archive holds personal notes, so keep it private: if you pass `--out` with a folder inside a git work tree, export refuses.
- `/anthology:import <file>` shows a dry run first: new notes, identical ones (skipped) and conflicts. Nothing is overwritten. A conflicting note is kept as `<name>.imported-<date>.md`, and Claude merges it with you. Archives over 50 MB are refused, files are extracted safely (no absolute paths, `..`, links or devices), only the memory folders can be written, and secret-looking content is reported. A repo or agent note the privacy scan flags is written to personal memory instead of the repo, and the dry run lists it as blocked.

## Updating and removing

**Updating.** Auto-update is off by default for third-party marketplaces such as `anthology`. Turn it on in `/plugin` > **Marketplaces** > `anthology` (install step 3). Claude Code then checks the marketplace after a session starts and updates the installed plugins on disk; it shows "Plugin updated", and the new version loads after `/reload-plugins` or at your next launch. Setting `DISABLE_AUTOUPDATER=1`, `DISABLE_UPDATES=1` or `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` also turns plugin auto-update off unless you set `FORCE_AUTOUPDATE_PLUGINS=1`. Without auto-update, update by hand: run `claude plugin marketplace update anthology`, then `claude plugin update anthology@anthology` (and the same for `anthology-guardrails` and `anthology-superpowers`), and restart Claude Code. The routing guide updates with the plugin, the `anthology` binary that the new `anthology.lock` pins is installed beside the old one at the next session start, rtk is reconciled to the pinned version at each session start, and the memory index format is versioned and rebuilt when it changes.

**Removing.** Run `/anthology:setup remove`. It takes out the settings entries Anthology added (the `Bash(rtk *)` and `Bash(anthology-mem save *)` allows, the Chrome deny rules and the plugin-read allow; rules you added yourself are never touched; `worktree.baseRef` goes back to its previous value, and so does `CLAUDE_CODE_SUBAGENT_MODEL` if setup removed it), deletes the downloaded rtk, rtk's local history (`~/.claude/anthology/rtk/`) and `~/.claude/anthology/state.json`, and optionally removes the repo approvals. It never deletes memory notes, the `.gitattributes` lines, the personal-memory folder or your exports. Then uninstall the plugin:

```text
claude plugin uninstall anthology@anthology --prune -y
```

`--prune -y` also removes the dependencies that were installed automatically. Uninstalling without `--keep-data` also removes the plugin's data folder (the rtk binaries and the memory search index).

Remove keeps the `anthology` binaries in `~/.claude/anthology/bin/` while `anthology-guardrails` is still installed, because its guard needs them. After you uninstall, delete that folder yourself (about 6 MB per version).

## Requirements

- macOS or Linux, on arm64 or x86_64. Setup refuses on Windows.
- Claude Code `2.1.311` or newer.
- For the one-time download of the `anthology` binary: network access to GitHub, one of `curl`, `wget` or `node`, one of `sha256sum`, `shasum` or `openssl`, and `tar`. macOS and most Linux systems have these. `node` is the last resort, and behind a proxy it works only on node 24 or newer. Offline, copy the archive in by hand (see [The `anthology` binary](#the-anthology-binary)).
- Node 22 or newer.
- Docker, for tests, builds and end-to-end suites.
- BrowserOS Neo, for all browser work.
- git.

`/anthology:setup` checks that the `anthology` binary is installed, that `node`, Docker (installed and running) and BrowserOS Neo are present, and tells you what is missing. It does not check versions, git or the Claude Code version.

## Replacing the official superpowers and design-skills

Anthology carries its own copies, so setup recommends uninstalling the official ones:

- **superpowers.** `anthology-superpowers` is the same skills library under a different name, so it does not clash with the official `superpowers` (two enabled plugins with one name lose hooks and collide on commands). Both would work side by side, but their instructions would load twice. Anthology's copy also adds `references/large-builds.md` to subagent-driven-development: the rules the main session follows on long multi-agent builds (start work against published interfaces, test once at the cheapest level, one review per task, batch merges, plain-language status reports). The main session reads it only when it runs such a build.
- **design-skills.** Anthology uses it only inside its agents' knowledge. An installed copy would expose its skills loose in the main session.

Setup finds the duplicates and shows the commands. You run them yourself; setup does not:

```text
claude plugin uninstall <plugin> -s <scope>
```

`claude plugin list` shows where each copy came from and at which scope it is installed.
