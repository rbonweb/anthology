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

Check [Requirements](#requirements) first.

### What `/anthology:setup` does

It checks your machine, then shows every change and applies them only after you confirm once.

1. **Doctor.** Checks that `python3`, `node` and `docker` are on your path (and Docker is running), that BrowserOS Neo is installed or configured as an MCP server, the rtk binary and the platform. BrowserOS Neo is found as any `BrowserOS*.app` in `/Applications` or `~/Applications`, a `browseros` binary on your path, or an MCP server in any scope; a separate check says whether that server is named `browseros-neo`, because agents call `mcp__browseros-neo__*`. Doctor also checks the compatibility assumptions Anthology relies on: the plugin's `bin/` wrappers exist and (inside Claude Code) are on your path, the plugin data folder exists, and agent memory lands in `.claude/agent-memory/anthology-<agent>/`. If `CLAUDE_CODE_SUBAGENT_MODEL` is set (in your environment or in `settings.json`), it warns that this overrides every agent's model so the roster's tiers do not apply; Anthology never changes that setting. After a first setup it also checks that the plugin-read permission (below) still matches where the plugin is installed. On Windows it refuses and stops.
2. **Duplicates.** If you have the official `superpowers` or `design-skills` installed, it recommends uninstalling them and shows the commands for you to run (see [Replacing the official superpowers and design-skills](#replacing-the-official-superpowers-and-design-skills)).
3. **rtk.** Downloads the pinned rtk binary for your platform from this repo's releases, checks its sha256 against `rtk.lock` and installs it in the plugin's data folder. If the download or the check fails, nothing is installed and setup shows a warning.
4. **Settings.** Edits `~/.claude/settings.json`:
   - allows `Bash(rtk *)`. This also auto-approves `rtk bypass <cmd>` (see [rtk](#rtk));
   - allows `Bash(anthology-mem save *)`, so agents that cannot edit files can still save their own memory notes (see [Saving notes](#saving-notes-anthology-mem-save));
   - denies `Read(...)` on the Chrome and Chromium data folders and everything under them: `~/Library/Application Support/Google` and `.../Chromium` on macOS, `~/.config/google-chrome` and `~/.config/chromium` (with the `-beta`, `-dev` and similar channel variants) on Linux;
   - allows `Read(...)` on the parent folder of the plugin's install location, so agents can read their own knowledge files. Outside auto mode, Claude Code does not auto-approve those reads;
   - sets `worktree.baseRef` to `"head"`, so agents that work in a worktree start from your current commit instead of the remote's default branch.
5. **Memory.** Approves the current repo for memory, which also adds two `merge=union` lines to the repo's `.gitattributes` (you can decline this step), and creates the personal-memory folder. If Claude Code kept notes for this folder before Anthology (the old path-keyed `~/.claude/projects/<encoded path>/memory/`), it lists each one with its diff and copies them all into personal memory, unless you pick some to move into the repo's memory instead. The default, and the recommendation, is none. Notes the privacy scan flags (home paths, emails, IP addresses, private host names, ssh targets and key paths, secrets) and `user` notes cannot be moved.

When it finishes, restart the session so the new permissions and the rtk hook take effect. Nothing is written to `~/.claude/CLAUDE.md`.

After setup, some things happen on their own at session start: Anthology delivers the routing guide, runs the guardrails self-test (see [Guardrails](#guardrails)), reconciles the rtk binary with `rtk.lock`, and in approved repos keeps memory working (see [Memory](#memory)). Until you have run `/anthology:setup` it prints a one-line reminder to do so, and it warns you if the plugin has moved so that agents can no longer read their knowledge files; re-run `/anthology:setup` to fix that. It also prints one loud line when a compatibility assumption breaks: Claude Code did not pass `CLAUDE_PLUGIN_DATA`, the plugin's `bin/` wrappers are missing or not executable, agent memory was written somewhere other than `anthology-<agent>/`, or `CLAUDE_CODE_SUBAGENT_MODEL` is set. When a release changes the `settings.json` entries that setup makes and one of them is missing, it prints "anthology settings changed in this release — run /anthology:setup to review" and changes nothing itself. If you deleted one of those entries on purpose (for example `Bash(rtk *)`), the line returns at each session start after every such release, until you run setup again. If the personal-memory folder is a symlink or not a folder, it prints a line saying personal memory was not loaded and nothing was moved or copied.

## The product

### Model roles

The main session talks to you in normal prose and orchestrates. It does small tasks (a handful of steps) itself and delegates the rest.

| Model | Role | Good for | Not for |
|---|---|---|---|
| Opus | Lead, researcher, reviewer | Ambiguous problems, research, architecture, decomposition, root-causing unknown bugs, high-stakes review | Mechanical edits, bulk searching |
| Sonnet | Builder | Implementing a clear plan, multi-file edits, tests, scoped refactors, reproducible bugs, docs, UI builds | Deciding what to build |
| Haiku | Scout, runner | Finding code, summarizing output, running tests and reporting, renames, boilerplate | Anything needing judgment |

Models are the aliases `opus`, `sonnet` and `haiku`, so agents follow new models automatically.

### The roster

Agents are called `anthology:<name>`. A routing guide, delivered to the main session at session start, tells it when to delegate and to whom. It never loads into the agents themselves.

| Agent | Model | Edits | Use it for | Knowledge it reads |
|---|---|---|---|---|
| `researcher` | opus | no | Research, competitor and user-research synthesis | competitive-brief, synthesize-research, research-synthesis, ux-research, journey-mapping |
| `architect` | opus | no | Architecture, system design, test strategy, plans | system-design, architecture, testing-strategy; preloaded: writing-plans |
| `implementer` | sonnet | yes, in a worktree | Building from a clear plan | preloaded: test-driven-development, verification-before-completion |
| `debugger` | sonnet | yes, in a worktree | Reproducible bugs | preloaded: systematic-debugging |
| `reviewer` | opus | no | Code review, tech debt, deploy readiness | code-review-checklist, tech-debt, deploy-checklist |
| `scout` | haiku | no | Finding code, running tests, summarizing output | none |
| `docs-writer` | sonnet | yes, in a worktree | Documentation | documentation, ux-writing |
| `ui-designer` | sonnet | yes, in a worktree | Building UI | frontend-design, design-taste-frontend, ui-taste, landing-page, web-design-system, design-handoff, design-systems, interaction-design, design-elevation |
| `ui-reviewer` | opus | no | Reviewing the rendered UI in BrowserOS Neo | web-design-guidelines, web-review, design-critique, accessibility-audit |
| `product-planner` | opus | docs only | Specs, sprint plans, roadmaps, product brainstorming | write-spec, sprint-planning, roadmap-update, product-brainstorming, ux-strategy, design-ops |
| `marketer` | sonnet | docs only | SEO, campaigns, content, email sequences | seo-audit, campaign-plan, content-creation, email-sequence |

- **Knowledge belongs to agents.** Every skill we ship is read by at least one agent, and the main session never sees it. The only skills the main session has are `anthology-superpowers` and the three user commands. Each agent reads its own knowledge files before it starts anything else.
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

If the guard cannot run (for example `python3` is missing), you get a loud message at session start instead of silence.

#### Known limits

The guard is built to stop accidental browser installs and launches by cooperative agents. It is a hook plus deny rules, not a sandbox or a jail, and it does not defend against deliberate obfuscation. Known limits:

- Commands built inside variables or arrays and then run are not seen for what they are.
- A command whose text comes from another command's output and is fed to a shell is not looked into: `bash -c "$(…)"`, `eval`, `<(…)`, `source <(…)`.
- Unusual nested shell syntax can hide a command, such as a comment or a `case` inside `$(…)`, or a quoted `}` inside `${…}`.
- Free-text tool fields (an agent prompt, todo text) and most MCP tool arguments are not checked for profile paths. Inside interpreter code (`python3 -c`, `node -e`, a heredoc fed to `python3`, a BrowserOS Neo `run` script) only profile paths spelled out in one string are seen: a path assembled piecewise (`Path.home() / "Library" / …`, variables) is not, and neither is a command run from inside the code (`os.system(…)`).
- Browser profile protection covers the known profile folders and their parents. Grandparent folders (for example `find ~ -name Cookies`) and the profile folders of snap and flatpak browsers are not covered. The native `Read` deny rules that setup adds are the second layer for browser profiles.
- A hook sees the command Claude runs, not what a script does inside. A script that launches Chrome internally is invisible to it. That is why the knowledge we ship never launches Chrome.
- Secret detection covers what is written in the command text, `Write` and `Edit`. Copying a secret **file** into a memory folder is not caught.
- The guard ignores malformed input rather than blocking, so it can never brick a session. If it cannot run at all, you get a loud message at session start.

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
- **Fails safe.** If the binary is missing, the hook exits 0 and commands run unchanged. The `rtk` command on your Bash path says "run /anthology:setup" if setup has not run.

### The `Bash(rtk *)` risk

Setup adds a blanket `Bash(rtk *)` allow so rewritten commands do not prompt (unless one of your deny or ask rules is involved, below). Because `rtk bypass <cmd>` runs any command, **this also auto-approves `rtk bypass <anything>`**. That is an accepted risk, chosen for a prompt-free workflow. What still holds:

- **Your deny and ask rules.** rtk's hook reads the `permissions` of your settings files (user, project, local and managed `managed-settings.json`) and applies them to the command and to the command an `rtk …` form wraps (`rtk bypass`, `run`, `err`, `test`, `summary`, `rtk git …`, and so on). A matching deny rule blocks the call (or, when the command is reached only through a launcher the hook does not model, such as `caffeinate git push`, asks); a matching ask rule makes Claude Code prompt, even under `Bash(rtk *)`. A `Read(…)` deny rule blocks `cat`, `head`, `tail`, `grep`, `rg` and `rtk read` of a denied path.
- **Commands, not what programs do.** The hook aims to match Claude Code's own rule matching and add to it: a deny or ask rule is enforced against the commands the agent types (every `rtk` form, wrapper, compound command, substitution and redirect), and a `Read(…)` rule against every file the line names or redirects. What an allowed program then does with files, plugins, configs or build steps it loads (`build.rs`, test files, MSBuild, the linker, plugins, a kubeconfig `exec`, compose files) is out of scope: deny rules restrict commands, not what programs do.
- **An allowlist once you have a rule.** With any deny or ask rule, an `rtk …` command runs without a prompt only when the hook understands all of it: a listed program (`git`, `ls`, `cat`, `grep`, `cargo`, `npm`, test runners, linters…) without the options that run a string or name a program or plugin to load (`git grep -O`, `git rebase -x`, `git -c`, `find -exec`, `go build -toolexec`, `dotnet -p:`, `prettier --plugin`…) and without a stdin script (including `/dev/stdin` or `/dev/fd/N`), `env -S`, `hash`, `alias`, a function, a substitution, a comment or a here-document. Anything else asks, including programs that run a string (`awk`, `make`, `ed`, `sqlite3`), shells, interpreters, scripts, packages fetched from a VCS or URL (`pip install git+…`, `npx -p <spec>`) and `rtk bypass` of an unlisted program. A deny or ask rule's command text anywhere in a line that runs `rtk` also asks, however it is quoted. With only a `git push` deny, about 22 of 124 common agent commands probed prompt; `rtk git status`, `rtk ls`, `rtk cargo test`, `rtk grep` and `rtk read` do not. Without any rule nothing changes.
- **Not covered.** Rules given only with `--settings` or `--disallowedTools`, server-managed settings and a macOS MDM profile never reach a hook, so they are not applied to the wrapped command. Neither are files a recursive search walks into (`grep -r`); files, links or settings the same command creates (`ln -s . d && cat d/secret.txt`); what a listed program runs from files (test code, build and `npm run` scripts, git hooks and config, plugins and registry packages), which is out of scope as above; or git object reads, which print a `Read`-denied file's committed content (`git show HEAD:<file>`, `git cat-file -p`, `git log -p`). Code that hides a command from a parser (a `python -c` script, `eval` of a variable) is outside every rule, as it is without rtk; behind `rtk` it makes the hook ask when you have rules. Commands that match no rule run without a prompt.
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

**Updating.** Auto-update is off by default for third-party marketplaces such as `anthology`. Turn it on in `/plugin` > **Marketplaces** > `anthology` (install step 3). Claude Code then checks the marketplace after a session starts and updates the installed plugins on disk; it shows "Plugin updated", and the new version loads after `/reload-plugins` or at your next launch. Setting `DISABLE_AUTOUPDATER=1`, `DISABLE_UPDATES=1` or `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` also turns plugin auto-update off unless you set `FORCE_AUTOUPDATE_PLUGINS=1`. Without auto-update, update by hand: run `claude plugin marketplace update anthology`, then `claude plugin update anthology@anthology` (and the same for `anthology-guardrails` and `anthology-superpowers`), and restart Claude Code. The routing guide updates with the plugin, rtk is reconciled to the pinned version at each session start, and the memory index format is versioned and rebuilt when it changes.

**Removing.** Run `/anthology:setup remove`. It takes out the settings entries Anthology added (the `Bash(rtk *)` and `Bash(anthology-mem save *)` allows, the Chrome deny rules and the plugin-read allow; rules you added yourself are never touched; `worktree.baseRef` goes back to its previous value), deletes the downloaded rtk, rtk's local history (`~/.claude/anthology/rtk/`) and `~/.claude/anthology/state.json`, and optionally removes the repo approvals. It never deletes memory notes, the `.gitattributes` lines, the personal-memory folder or your exports. Then uninstall the plugin:

```text
claude plugin uninstall anthology@anthology --prune -y
```

`--prune -y` also removes the dependencies that were installed automatically. Uninstalling without `--keep-data` also removes the plugin's data folder (the rtk binaries and the memory search index).

## Requirements

- macOS or Linux. Setup refuses on Windows.
- Claude Code `2.1.311` or newer.
- Python 3 (3.9 or newer; macOS's own `python3` is enough). Anthology's scripts use only the standard library.
- Node 22 or newer.
- Docker, for tests, builds and end-to-end suites.
- BrowserOS Neo, for all browser work.
- git.

`/anthology:setup` checks that `python3`, `node`, Docker (installed and running) and BrowserOS Neo are present, and tells you what is missing. It does not check versions, git or the Claude Code version.

## Replacing the official superpowers and design-skills

Anthology carries its own copies, so setup recommends uninstalling the official ones:

- **superpowers.** `anthology-superpowers` is the same skills library under a different name, so it does not clash with the official `superpowers` (two enabled plugins with one name lose hooks and collide on commands). Both would work side by side, but their instructions would load twice.
- **design-skills.** Anthology uses it only inside its agents' knowledge. An installed copy would expose its skills loose in the main session.

Setup finds the duplicates and shows the commands. You run them yourself; setup does not:

```text
claude plugin uninstall <plugin> -s <scope>
```

`claude plugin list` shows where each copy came from and at which scope it is installed.
