# Claude Code mods

This page covers the mods the `anthology` plugin adds on Claude Code 2.1.330 or later: strict delegation, the after-work check, and the alerts and status pane. A mod is code the plugin loads into Claude Code itself; on an older Claude Code none of them run.

A fourth mod, which lets provably read-only shell commands run without a prompt, is described under [Guardrails](guardrails.md#read-only-commands-without-a-prompt). The switches for all of them are at the [end of this page](#switches).

## Strict delegation

The routing guide says the main session plans, briefs agents, checks and merges, and never produces the work itself. The mod `hooks/mods/delegation.ts` makes that rule hold in **interactive** sessions. It does nothing in a headless run (`claude -p`, CI), in a subagent, in an embedded host (see [Host mode](agents.md#host-mode)), or in a session whose tool pool has no `Agent` tool (the lead could not hand the work on).

- **Coordinator text.** When the session has the `Agent` tool, the system prompt gets one short section: the lead plans, briefs, checks and merges, hands the making to agents, and fixes inline only a few lines in one file that you asked for. A prompt without the section (`--bare`, a teammate's) gets the same rule inside each refusal.
- **Refusals.** The main session's `Edit`, `Write` and `NotebookEdit` calls are refused. The message says why (the fix is too long or too large, a second file, the budget is used up, or a `Write`, `NotebookEdit` or `replace_all` is never inline) and names the agent to hand the change to:
  - `anthology:docs-writer` for `.md`, `.mdx`, `.markdown`, `.rst` and `.txt` files;
  - `anthology:ui-designer` for `.tsx`, `.jsx`, `.css`, `.html` and `.svg`;
  - `anthology:implementer` (or `anthology:debugger` for a bug fix) for anything else.
- **Allowed without a refusal.**
  - Notes the lead keeps. Under `CLAUDE_CONFIG_DIR` (else `~/.claude`): `projects/<project>/memory/` (Claude Code's auto memory), `plans/` and `anthology/personal-memory/`. The folder in your `autoMemoryDirectory` setting, when you set one. In the project: `.claude/memory/` (the repo memory, where setup turned auto memory off), `.superpowers/` and `docs/superpowers/plans/`. `~` and `..` are folded and a relative path is taken from the session's directory before the match.
  - A tiny fix: an `Edit` with `old_string` and `new_string` of at most 5 lines each and about 2 KB changed in all (2048 characters), no `replace_all`, at most 3 of them per turn, all in one file. The count starts again at each turn, and an edit that failed or was refused by another hook does not count. A `Write` or `NotebookEdit` is never a tiny fix.
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

## Alerts and the status pane

Three things tell you that Claude Code needs you or what the agents are doing.

- **Desktop alert.** A `Notification` hook pops a desktop alert with a sound when Claude Code shows a permission prompt or waits for your input. It uses `notify-send` plus `paplay` or `canberra-gtk-play` on Linux, else a terminal bell. On macOS it rings the terminal bell only: a notification posted through `osascript` belongs to Script Editor, so clicking it opens Script Editor. This is the whole alert on a Claude Code without plugin mods.
- **In-session alert.** Where Claude Code loads plugin mods, the mod `hooks/mods/alerts.tsx` also shows a one-line toast in the session, at most once every 20 seconds. It makes no sound: the shell hook keeps the only sound and the desktop alert, so where both run you get the toast and the desktop alert together.
- **Status pane.** `/anthology-status` opens a pane called Anthology (from the same mod). It lists the agents that run, the last five that finished and the permission prompts that wait for you, and a row above the prompt keeps the count (`● 2 running   ✓ 3 done   ✗ 1 failed   ! 1 needs you`, with the command on the right; `failed` counts failed and stopped agents). The row shows only while an agent runs or a prompt waits for you; finished agents alone draw no row. In a narrow window it drops the command, then done, then failed, then the word "running", and always keeps `needs you`. Times are on your machine's clock. The pane and the row draw plain text, so they read the same on the terminal, the desktop app, VS Code and mobile.

The pane and the toast name a tool and an agent's type and description, and never what a tool call does: no command, file path or URL is shown, on any surface.

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
| `ANTHOLOGY_MODS=0` | Turns off every Anthology mod: strict delegation, the after-work check, the toast, the status pane and its row above the prompt, and the read-only commands mod |
| `ANTHOLOGY_DELEGATION=0` | Turns off strict delegation |
| `ANTHOLOGY_AFTERWORK=0` | Turns off the after-work check |
| `ANTHOLOGY_NOTIFY=0` | Turns off the alert: the desktop alert and the toast |
| `ANTHOLOGY_HOST_MODE=full` | Opts back in when another program drives the session (see [Host mode](agents.md#host-mode)) |
