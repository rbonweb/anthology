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
  - A tiny fix: one `Edit`, no `replace_all`, at most 3 of them per turn, all in one file. The count starts again at each turn, and an edit that failed or was refused by another hook does not count. A `Write` or `NotebookEdit` is never a tiny fix. How much it may change depends on the risk: code that is added or changed, at most 5 lines each in `old_string` and `new_string` and about 2 KB in all (2048 characters); a small config file (`.json`, `.jsonc`, `.json5`, `.yaml`, `.yml`, `.toml`, `.ini`, `.cfg`, `.conf`, `.env`, `.properties`, `.plist`), 15 lines and the same 2 KB; a pure deletion (`new_string` empty), up to 30 lines and 4 KB, in any file.
- **Limits.** Bash writes (`sed -i`, a redirect) are not seen; the routing guide covers them. Paths are matched as written, with no symlinks resolved. A teammate in a pane of its own looks like a lead to a hook (nothing tells them apart), so the `Agent` tool in its pool is the rule there. This is a workflow rule, not a safety boundary: if the mod fails, Claude Code's own verdict stands and the edit runs.
- **Not built yet.** A gate on MCP tools that do not say they are read-only, with an allowlist for `mcp__plugin-authoring__scan_secrets`. For now only the file tools above are gated.

## After-work check

When a writing agent (`implementer`, `debugger`, `docs-writer`, `ui-designer`) reports to the main session, the mod `hooks/mods/afterwork.ts` checks the report before the lead reads it. A short "After-work check" block is appended only when there is a real gap:

- no commit (a sha, or the word "committed");
- no test evidence (a test word; "tests not run" or "skipped" counts as none);
- attribution lines (`Co-Authored-By: Claude`, "Generated with") in the report, which must not be there;
- uncommitted files in the agent's own worktree.

Two more items are reminders, not gaps. They ride in a block that a gap raised and never raise one:

- the project map, when the report names a new, removed or renamed piece and does not say the map was updated, is missing or is not needed;
- leftovers, when the report starts containers, volumes or temp files and does not say they were removed (never asked of the docs-writer).

The agent's own text is never rewritten or cut. A report with no gap is delivered with no block, so no block means no gap found. The check is string matching on the report, with no model call, and it is a hint, not proof: a report can mention a test it did not run.

When the agent has its own worktree, the mod also runs one `git status --porcelain` there (2 second limit, the agent's memory notes left out). Any git failure leaves the text-only check. Reports of the reviewer and scout, nested reports, and reports of failed or stopped runs are left as they are.

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

## Switches

Set these in the environment, for example in the `env` block of `~/.claude/settings.json`, and restart the session.

| Switch | Effect |
|---|---|
| `ANTHOLOGY_MODS=0` | Turns off every Anthology mod: strict delegation, the after-work check, the status pane, and the read-only commands mod |
| `ANTHOLOGY_DELEGATION=0` | Turns off strict delegation (for one repo only: `{"delegation": false}` in its `.claude/anthology.json`, see [Your own agents and skills](agents.md#your-own-agents-and-skills)) |
| `ANTHOLOGY_AFTERWORK=0` | Turns off the after-work check |
| `ANTHOLOGY_HOST_MODE=full` | Opts back in when another program drives the session (see [Host mode](agents.md#host-mode)) |
