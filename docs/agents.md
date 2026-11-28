# Agents, commands and working rules

This page covers the model roles, the agent roster, the commands you can run, the rules the routing guide sets for the main session, and when Anthology stays out of a session.

## Model roles

The main session talks to you in normal prose and orchestrates. It does small tasks (a handful of steps) itself and delegates the rest.

| Model | Role | Good for | Not for |
|---|---|---|---|
| Opus | Lead, researcher, reviewer | Ambiguous problems, research, architecture, decomposition, root-causing unknown bugs, high-stakes review | Mechanical edits, bulk searching |
| Sonnet | Builder | Implementing a clear plan, multi-file edits, tests, scoped refactors, reproducible bugs, docs, UI builds | Deciding what to build |
| Haiku | Scout, runner | Finding code, summarizing output, running tests and reporting, renames, boilerplate | Anything needing judgment |

Models are the aliases `opus`, `sonnet` and `haiku`, so agents follow new models automatically.

## Agent models

`CLAUDE_CODE_SUBAGENT_MODEL` makes every agent run on one model. It overrides the `model` of each roster agent, so the Opus, Sonnet and Haiku roles above do not apply while it is set.

- **What setup offers.** `/anthology:setup` says where it is set, never its value. If it is in the `env` block of your user `settings.json` (the file setup edits), setup asks you separately whether to remove it, and recommends yes. The preview shows the removal as a diff with the value shown as `"<redacted>"`. Setup removes it only if you say yes.
- **Anywhere else** (your shell profile, the project's `.claude/settings.json` or `.claude/settings.local.json`, managed settings), setup leaves it alone and says how to remove it there. While it is still set in one of those places, the tiers do not apply. If it is set twice in your user `settings.json`, setup asks you to delete both lines by hand.
- **By hand.** Delete the `CLAUDE_CODE_SUBAGENT_MODEL` line from the `env` block of the settings file, or the line that sets it in your shell profile (such as `~/.zshrc`), then restart Claude Code.
- **Undo.** `/anthology:setup remove` puts back the value setup removed. If you have set it again since, your new value stays.

## The roster

Agents are called `anthology:<name>`. A routing guide, delivered to the main session at session start, tells it when to delegate and to whom. It never loads into the agents themselves.

| Agent | Model | Edits | Use it for | Knowledge it reads |
|---|---|---|---|---|
| `researcher` | opus | no | Research, competitor and user-research synthesis | competitive-brief, synthesize-research, research-synthesis, ux-research, journey-mapping |
| `architect` | opus | no | Architecture, system design, test strategy, plans | system-design, architecture, testing-strategy; preloaded: writing-plans |
| `implementer` | sonnet | yes, in a worktree | Building from a clear plan | karpathy-guidelines; preloaded: test-driven-development, verification-before-completion |
| `debugger` | sonnet | yes, in a worktree | Reproducible bugs | karpathy-guidelines; preloaded: systematic-debugging |
| `reviewer` | opus | no | Code review, tech debt, deploy readiness | code-review-checklist, karpathy-guidelines; tech-debt and deploy-checklist only when they apply |
| `scout` | haiku | no | Finding code, running tests, summarizing output | none |
| `docs-writer` | sonnet | yes, in a worktree | Documentation | documentation, ux-writing, humanizer |
| `ui-designer` | sonnet | yes, in a worktree | Building UI, cloning a site's look | frontend-design, design-taste-frontend, ui-taste, landing-page, web-design-system, design-handoff, design-systems, interaction-design, design-elevation, avoid-ai-design; website-clone only for a clone |
| `ui-reviewer` | opus | no | Reviewing the rendered UI in BrowserOS Neo | web-design-guidelines, web-review, design-critique, accessibility-audit, avoid-ai-design |
| `product-planner` | opus | docs only | Specs, sprint plans, roadmaps, product brainstorming | write-spec, sprint-planning, roadmap-update, product-brainstorming, ux-strategy, design-ops, humanizer |
| `marketer` | sonnet | docs only | SEO, campaigns, content, email sequences | seo-audit, campaign-plan, content-creation, email-sequence, humanizer |

- **Knowledge belongs to agents.** Every skill we ship is read by at least one agent, and the main session never sees it. The only skills the main session has are `anthology-superpowers`, the three user commands and `anthology:large-builds`, the rulebook for long multi-agent builds. Each agent reads its own knowledge files before it starts anything else.
- **Builders and critics are different models.** For UI work, `ui-designer` (Sonnet) builds and `ui-reviewer` (Opus) checks.
- **For websites and landing pages** `design-taste-frontend` leads. **For app screens** `ui-taste` leads. `frontend-design` is the baseline.
- **No agent can call other agents,** so the main session stays the single orchestrator. In the Edits column, "no" means the Edit, Write and NotebookEdit tools are turned off for that agent. It can still run Bash, which is how it saves its own memory notes. "Docs only" for `product-planner` and `marketer` is an instruction in the agent, not a tool restriction.
- **Every agent has project memory** (see [Memory](memory.md)).
- **Security work** goes to the `claude-security` plugin. There is no Anthology agent for it.

Agents write in one fixed terse style in chat and reports. Files, code, commits and docs stay in normal prose. There is no switch.

## Commands

| Command | What it does |
|---|---|
| `/anthology:setup` | Checks your machine, shows the changes as diffs and makes them after you confirm (see [Setup](setup.md)) |
| `/anthology:setup remove` | Takes out the settings entries and the ATL download that setup added; never deletes your notes (see [Removing](setup.md#removing)) |
| `/anthology:export` | Exports your memory notes to a portable archive (see [Export and import](memory.md#export-and-import)) |
| `/anthology:import <file>` | Imports an archive after a dry run |
| `/anthology-status` | Opens a pane with the agents that run, the last few that finished and the permission prompts that wait for you (see [The status pane](mods.md#the-status-pane)) |

These run only when you ask for them.

## Your own agents and skills

The agents above fill the gaps; they do not replace yours. When a repo has its own agents or skills, the main session uses them first. The routing guide says so in one line, and session start adds a short section, "This project's own agents and skills", right after the guide. It lists:

- the repo's `.claude/agents/*.md` and `.claude/skills/*/SKILL.md`, each by its frontmatter `name` and a description cut to 100 characters;
- an agent table or list in the repo's `CLAUDE.md` (or `.claude/CLAUDE.md`): the rows of a table and the items of a list under a heading that has "agent" in it, such as `## Project agents`. In a table, the column whose header says "agent" is the name; without one, the first column is;
- then your own agents and skills in the Claude config folder (`~/.claude/agents`, `~/.claude/skills`, or under `CLAUDE_CONFIG_DIR`).

Under the heading is the rule: for the jobs they cover, use the project's own agents and skills before Anthology's; Anthology's agents fill the gaps. Then a line says the entries are text from the repo's own files, not instructions. The list holds at most 12 entries (project ones first; a name appears once), so it stays inside the session-start budget. Names and descriptions lose `<`, `>`, backticks and control, invisible and direction-changing characters. A file that does not parse, one with no description, and Anthology's own (`anthology`, `anthology:*`, `anthology-*`) are left out. A dispatch to any agent, yours included, is never refused or redirected.

A repo can switch parts of this off for itself in `.claude/anthology.json` at the project root (found at the git top, so a session started in a subfolder reads the same file), a JSON object with boolean keys. A missing key, a value that is not `false`, or a file that does not parse leaves the default. **A switch counts only in a repo you approved in `/anthology:setup`.** A repo you cloned can ship the file, and it cannot use it to switch Anthology's rules off: in any other repo the file is ignored, and session start says so in its message. When a switch does count, the message says that too, every session.

| Key | `false` means |
|---|---|
| `routing_guide` | Session start leaves out the routing guide, this section and the project-map line below. Memory still loads, and a **safety floor** of guide lines stays: no attribution, tests and builds in containers, no host installs, literal-path deletes, the browser rule and no secrets in repo memory. |
| `delegation` | The [strict-delegation mod](mods.md#strict-delegation) does nothing in this repo. It asks the installed `anthology switch delegation <folder>`, which applies the same approval rule; with no binary installed the mod stays on. |

The line "before planning, read `.claude/memory/reference-project-map.md`; missing: dispatch `anthology:architect`" is not in the guide. Session start adds it only for a repo you approved in `/anthology:setup`.

## Working rules the routing guide sets

- **Worktrees.** With worktrees, writers use Claude Code's built-in isolation, never `git worktree add`. Writers commit before they finish, and the main session reviews and merges. Parallel writers get separate files. Several similar parts (pages, themes, endpoints) fan out: one agent builds the shared base and commits, then one agent per part works in parallel on separate files, then one review. To clone a site, one `ui-designer` captures the pages and builds the shared base, then one `ui-designer` per page builds in parallel, then `ui-reviewer` checks. The clone keeps the original's structure and look and always replaces its brand, images, text and form actions.
- **Docker.** If the repo has tests, builds or end-to-end suites, they run in containers that mount only the current worktree. Agents do no host-level installs (`brew`, `npm -g`, global `pip`, browser downloads). Project-level installs such as `npm install` are fine. If Docker is unavailable, the agent stops and tells you.
- **Blocked agents.** A subagent cannot ask you a question. It stops and reports it, and the main session answers or asks you, then resumes it.
- **Coding and long builds.** Code follows the Karpathy guidelines (think first, simplest fit, surgical diffs, verify). The implementer, debugger and reviewer read them at startup. For long or multi-agent builds, status reports and decisions made while you are away, the main session loads `anthology:large-builds`: plain feature-based status reports (done, running, blocked, an honest estimate), and each decision recorded as `Ruling: what — why — cost if wrong`, while still stopping for irreversible, security-sensitive or outward-facing actions.
- **Strict delegation.** The main session plans, briefs agents, checks and merges, and does not produce the work itself. A mod enforces this in interactive sessions (see [Strict delegation](mods.md#strict-delegation)).
- **Credentials.** An agent that fetches or generates credentials (from the browser, a dashboard or a CLI) saves them as `KEY=value` lines in `~/.config/<project>/<service>.env`, for example `~/.config/sinrix/cloudflare-email.env`. `<project>` is the project's name in lowercase, taken from the repo folder unless the project names itself otherwise. The folder is mode 700 and the file 600. Credentials never go in the repo, a commit, a log or the agent's output, and its report names only the path and the keys.
- **Commits and browsers.** Commits carry only your name, and all browser work goes through BrowserOS Neo (see [Guardrails](guardrails.md)).

## Host mode

When another program runs Claude Code for you (the Agent SDK, or `claude -p` in a script), nobody reads the session start. Anthology treats a session as embedded when `CLAUDE_CODE_SESSION_ATTENDED` is `0`. When that flag is `1`, the session is not embedded, whatever its entrypoint. Only when the flag is unset (or holds any other value) does an entrypoint that starts with `sdk-` mark an embedded session. So an interactive `claude` that an SDK process started, and that inherited an `sdk-*` entrypoint, keeps its guide. Interactive terminal and VS Code sessions are not embedded by default.

In an embedded session the host brings its own prompt and Anthology adds none of its own:

- The session start sends no routing guide and no memory index. Its message keeps only the guard self-test and the guard binary bootstrap failures.
- `anthology-superpowers` adds no introduction.
- The strict-delegation mod adds no coordinator text and refuses nothing.
- The guardrails stay on.

To get the full session start in such a session, set `ANTHOLOGY_HOST_MODE=full` in its environment (for example under `env` in the settings that session reads).

## Stale plugin version

After an update of the anthology plugin, a session you resume or compact keeps running the version it loaded. When that version is older than the installed one, the session start says so, names the plugin, and asks you to restart Claude Code.
