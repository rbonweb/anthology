# Claude Code mods

This page covers the mods the `anthology` plugin adds on Claude Code 2.1.330 or later: strict delegation, the after-work check, and the status pane. A mod is code the plugin loads into Claude Code itself; on an older Claude Code none of them run.

A fourth mod, which lets provably read-only shell commands run without a prompt, is described under [Guardrails](guardrails.md#read-only-commands-without-a-prompt). The switches for all of them are at the [end of this page](#switches).

## Strict delegation

The routing guide says the main session plans, briefs agents, checks and merges, and never produces the work itself. The mod `hooks/mods/delegation.ts` makes that rule hold in **interactive** sessions. It does nothing in a headless run (`claude -p`, CI), in a subagent, in an embedded host (see [Host mode](agents.md#host-mode)), or in a session whose tool pool has no `Agent` tool (the lead could not hand the work on).

- **Coordinator text.** When the session has the `Agent` tool, the system prompt gets one short section: the lead plans, briefs, checks and merges, hands the making to agents, and fixes inline only a few lines in one file that you asked for. A prompt without the section (`--bare`, a teammate's) gets the same rule inside each refusal.
- **Refusals.** The main session's `Edit`, `Write` and `NotebookEdit` calls on project code are refused. The message says why (the fix is too long or too large, a second file, the budget is used up, or a `Write`, `NotebookEdit` or `replace_all` is never inline) and sends the change to the project's own matching agent when one fits (see [Your own agents and skills](agents.md#your-own-agents-and-skills)), else to the Anthology agent for the file:
  - `anthology:docs-writer` for `.md`, `.mdx`, `.markdown`, `.rst` and `.txt` files;
  - `anthology:ui-designer` for `.tsx`, `.jsx`, `.css`, `.html` and `.svg`;
  - `anthology:implementer` (or `anthology:debugger` for a bug fix) for anything else.

  Only those three file tools are gated. A dispatch (the `Agent` tool) to any agent, a project agent included, is never refused or redirected.
- **Allowed without a refusal.**
  - Notes the lead keeps. Under `CLAUDE_CONFIG_DIR` (else `~/.claude`): `projects/<project>/memory/` (Claude Code's auto memory), `plans/` and `anthology/personal-memory/`. The folder in your `autoMemoryDirectory` setting, when you set one. In the project: `.claude/memory/` (the repo memory, where setup turned auto memory off), `.superpowers/` and `docs/superpowers/plans/`. `~` and `..` are folded and a relative path is taken from the session's directory before the match.
  - Files that are not project code: everything in the Claude config folder (`~/.claude`, or `CLAUDE_CONFIG_DIR`), even when a dotfiles repo holds it, and any file outside every git repo (a folder or file named `.git` in its folder or above it makes it part of a repo; when that check cannot run, the file counts as project code). Any size, no budget.
  - Local setup: a file in the repo that git ignores and does not track, such as `.env.<name>.local`. Only the main checkout holds it, and Claude Code refuses an agent in a worktree a write there, so no agent could make it. Git decides: `git check-ignore` must report it ignored and `git ls-files --error-unmatch` must find it outside the index, with the `git` found on the absolute `PATH` entries only. Nothing under `.git` counts, nor a symbolic link, and when any step fails or git is missing the file is handed off as before. Any size, no budget.
  - A tiny fix: one `Edit`, no `replace_all`, at most 3 of them per turn, all in one file. The count starts again at each turn, and an edit that failed or was refused by another hook does not count. A `Write` or `NotebookEdit` is never a tiny fix. How much it may change depends on the risk: code that is added or changed, at most 5 lines each in `old_string` and `new_string` and about 2 KB in all (2048 characters); a small config file (`.json`, `.jsonc`, `.json5`, `.yaml`, `.yml`, `.toml`, `.ini`, `.cfg`, `.conf`, `.env`, `.properties`, `.plist`), 15 lines and the same 2 KB; a pure deletion (`new_string` exactly empty; blank text is a replacement), up to 30 lines and 4 KB, in any file. CI pipeline files (`.github/`, `.gitlab-ci.yml`, `.circleci/` and the like) run code, so they keep the 5-line code limit.
- **Limits.** Bash writes (`sed -i`, a redirect) are not seen; the routing guide covers them. Paths are matched as written, with no symlinks resolved, so a link that points into a repo from outside one is judged by where the link sits. "Outside every git repo" means no `.git` folder or file in the file's folder or above it: a bare repository used through `GIT_DIR` (a dotfiles repo with its work tree at `~`), a Jujutsu (`.jj`) or Mercurial (`.hg`) checkout has none, so its files count as outside and are yours to edit. A repo can switch the mod off with `"delegation": false` in its `.claude/anthology.json`, but only when it is approved in `/anthology:setup` (see [Your own agents and skills](agents.md#your-own-agents-and-skills)). A teammate in a pane of its own looks like a lead to a hook (nothing tells them apart), so the `Agent` tool in its pool is the rule there. This is a workflow rule, not a safety boundary: if the mod fails, Claude Code's own verdict stands and the edit runs.
- **Not built yet.** A gate on MCP tools that do not say they are read-only, with an allowlist for `mcp__plugin-authoring__scan_secrets`. For now only the file tools above are gated.

## After-work check

When a writing agent (`implementer`, `debugger`, `docs-writer`, `ui-designer`) reports to the main session, the mod `hooks/mods/afterwork.ts` checks the report before the lead reads it. A short "After-work check" block is appended only when there is a real gap:

- no commit (a sha, or the word "committed");
- no test evidence (a test word; "tests not run" or "skipped" counts as none);
- attribution lines (`Co-Authored-By: Claude`, "Generated with") in the report, which must not be there;
- uncommitted files in the agent's own worktree;
- AI tells in the agent's changes, as `anthology slop check` finds them (see [The slop gate](slop-gate.md)): the gap names the first three and asks the lead to send the agent back to rewrite them;
- slop waivers or ignore regions the agent added: the gap names the first three and asks the lead to confirm the user asked for that wording.

Two more items are reminders, not gaps. They ride in a block that a gap raised and never raise one:

- the project map, when the report names a new, removed or renamed piece and does not say the map was updated, is missing or is not needed;
- leftovers, when the report starts containers, volumes or temp files and does not say they were removed (never asked of the docs-writer).

The agent's own text is never rewritten or cut. A report with no gap is delivered with no block, so no block means no gap found. The check is string matching on the report, with no model call, and it is a hint, not proof: a report can mention a test it did not run.

When the agent has its own worktree, the mod also runs one `git status --porcelain` there (2 second limit, the agent's memory notes left out). Any git failure leaves the text-only check.

The two slop gaps come from one more run in that worktree: `anthology slop check --since <the main session's HEAD>`, which looks at the lines the agent added and at the files it left untracked (4 second limit, plus a 1.5 second `git rev-parse HEAD`). The slop gate refuses a Write, Edit or MultiEdit that adds a tell, so this part is there for text that reached a file another way, such as a scaffolder or `sed` run through the shell, and for waivers. It adds nothing when the binary is not installed (the mod never downloads it), when the run times out or fails, or when the agent works in the main session's own folder.

Reports of the reviewer and scout, nested reports, and reports of failed or stopped runs are left as they are.

## The status pane

`/anthology-status` opens a pane called Anthology. It lists the agents that run, the last five that finished and the permission prompts that wait for you. Nothing is drawn above the prompt: the pane is the only status display. Times are on your machine's clock. The pane draws plain text, so it reads the same on the terminal, the desktop app, VS Code and mobile.

Anthology sends no desktop alerts, sounds or toasts; any notification app you choose works alongside it.

The pane names a tool and an agent's type and description, and never what a tool call does: no command, file path or URL is shown, on any surface.

```text
Needs your approval (2)
Bash
Edit  from implementer

Running (2)
14:02  implementer  Build the alerts mod
14:05  scout  Find the notify hooks

Finished
ok       3m 12s  reviewer  Review the scaffold
failed   1m 04s  implementer  Fix the CI job
```

Approvals come first because nothing moves until you answer them. A subagent's approval stays listed when the lead's turn ends, until that agent reports. A run that ended without reporting drops off the list when Claude Code no longer lists it as alive. With nothing to show the pane says so. A long row is cut at the pane's edge, and a list past 20 running agents ends with `and N more`.

Not built yet: a block of open pull requests and their CI status (it needs `gh`). The pane does not show it.

## Secret masking

The mod `hooks/mods/masking.ts` masks secrets in the output of a Bash call before Claude reads it, for the main session and for subagents. A secret is replaced with a marker such as `[masked by anthology: github-token]`; the rest of the output is left as it was. The marker names the kind of secret, never the value. The result Claude Code writes to the transcript is the masked one too.

What it masks: AWS access keys, GitHub tokens, API keys, Slack tokens, private keys (the lines between `BEGIN` and `END` become one marker line, and both header lines stay) and the value of a `password` or `api_key` setting. It leaves lockfile hashes, git SHAs, integrity strings (`sha512-...`), CSS class names and code that only reads a secret (`password = os.environ["DB_PASSWORD"]`) as they were.

How it stays fast: a cheap text check runs first, and the masker, `anthology guard mask`, starts only when the output holds a likely match. Output with no match starts no process. Output over 1,000,000 characters is left as it is.

Only Bash results are masked. Output of Read, background commands and other tools is not, and neither are other kinds of secret (JWTs, bearer tokens, Stripe keys).

The mod fails open: with the binary missing or too old, an error or a timeout, Claude sees the output unmasked.

Claude may not write the marker into a file. A Write, Edit or MultiEdit whose new text contains `[masked by anthology:` is refused, with the message to write the real value from its source (an env file or `~/.config/<project>/<service>.env`) and never the marker.

## Parallel work reminder

During a long build with a plan and a ledger in `.superpowers/sdd`, the lead tends to run one task at a time when more could start. The mod `hooks/mods/nudge.ts` adds one reminder to an agent's report, in the main session only, when:

- the repo has a `.superpowers/sdd` folder (a repo without one costs one file check);
- the newest `progress.md` in it changed in the last 6 hours, its first line names a plan that exists, and no ledger line starts with `Serial:`;
- tasks of the plan are not started, at most two other agents are running, and at least one task could start now.

The reminder says how many agents run and how many tasks are not started, and asks the lead to start each task whose inputs are published, one agent per task. If the rest must wait, the lead adds a line `Serial: <reason>` to the ledger, and the reminder stops.

It shows at most once every 20 minutes, and not again for the same number of tasks left. It adds nothing to the report of a subagent, or in an embedded host (see [Host mode](agents.md#host-mode)). It fails open: an error leaves the report as it was.

## Switches

Set these in the environment, for example in the `env` block of `~/.claude/settings.json`, and restart the session.

| Switch | Effect |
|---|---|
| `ANTHOLOGY_MODS=0` | Turns off every Anthology mod: strict delegation, the after-work check, the status pane, the read-only commands mod, secret masking and the parallel work reminder |
| `ANTHOLOGY_DELEGATION=0` | Turns off strict delegation (for one approved repo only: `{"delegation": false}` in its `.claude/anthology.json`, see [Your own agents and skills](agents.md#your-own-agents-and-skills)) |
| `ANTHOLOGY_AFTERWORK=0` | Turns off the after-work check |
| `ANTHOLOGY_SECRET_MASK=0` | Turns off secret masking of Bash output |
| `ANTHOLOGY_NUDGE=0` | Turns off the parallel work reminder |
| `ANTHOLOGY_SLOP_GATE=0` | Turns off the slop gate (see [The slop gate](slop-gate.md)) and its part of the after-work check |
| `ANTHOLOGY_HOST_MODE=full` | Opts back in when another program drives the session (see [Host mode](agents.md#host-mode)) |
