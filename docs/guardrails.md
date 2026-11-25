# Guardrails

This page covers what the guard blocks, its known limits, the shell commands that run without a prompt, the commit attribution rule and the browser rule. [SECURITY.md](../SECURITY.md) has the full threat model.

## What the guard blocks

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

If the guard cannot run (its binary is missing, broken or cannot be installed), it fails closed. Read-only tools still work, everything else is blocked with a message that says why and names the fix, and session start says so loudly. See [The `anthology` binary](binary.md#if-it-cannot-be-installed).

## Known limits

The guard is built to stop accidental browser installs and launches by cooperative agents. It is a hook plus deny rules, not a sandbox or a jail, and it does not defend against deliberate obfuscation. Known limits:

- A command whose program or script comes from a variable, an array, a positional parameter, an alias or a value read from input is blocked as too complex to check, except a program under a standard directory such as `"$HOME/.cargo/bin/cargo"`. [SECURITY.md](../SECURITY.md#values-the-guard-cannot-see) lists exactly what is blocked and what still gets through.
- A command whose text comes from another command's output and is fed to a shell is not looked into: `bash -c "$(…)"`, `eval`, `<(…)`, `source <(…)`.
- Unusual nested shell syntax can hide a command, such as a comment or a `case` inside `$(…)`, or a quoted `}` inside `${…}`.
- Free-text tool fields (an agent prompt, todo text) and most MCP tool arguments are not checked for profile paths. Inside interpreter code (`python3 -c`, `node -e`, a heredoc fed to `python3`, a BrowserOS Neo `run` script) only profile paths spelled out in one string are seen: a path assembled piecewise (`Path.home() / "Library" / …`, variables) is not, and neither is a command run from inside the code (`os.system(…)`).
- Browser profile protection covers the known profile folders and their parents. Grandparent folders (for example `find ~ -name Cookies`) and the profile folders of snap and flatpak browsers are not covered. The native `Read` deny rules that setup adds are the second layer for browser profiles.
- A hook sees the command Claude runs, not what a script does inside. A script that launches Chrome internally is invisible to it. That is why the knowledge we ship never launches Chrome.
- Secret detection covers what is written in the command text, `Write` and `Edit`. Copying a secret **file** into a memory folder is not caught.
- The guard ignores malformed input rather than blocking. If it cannot run at all, it blocks every tool call that is not read-only until its binary is installed, and `/plugin` can always disable it.
- While its binary is missing, the read-only tools it lets through (`Read`, `Grep`, `Glob`, `WebFetch` and the like) are not checked for browser profile paths. The native `Read` deny rules that setup adds still apply to them.

The `Bash(atl *)` allow that setup adds has its own accepted risk: see [The `Bash(atl *)` risk](atl.md#the-bashatl--risk).

## Read-only commands without a prompt

On Claude Code 2.1.330 or later, a Claude Code mod in the `anthology` plugin lets a shell command run without a permission prompt when `anthology perm classify` proves it only reads, and Claude Code itself would allow a `Read` of every file it reads. It only changes a plain ask that you would answer in the permission dialog. Everything else stays exactly as Claude Code decides it:

- a deny, and any ask from your own allow, ask or deny rules, a `PreToolUse` hook such as the guardrails, another mod, the outside-directory read block or your organization;
- headless (`claude -p`), background and `dontAsk` sessions, SDK hosts and auto mode's classifier: their denies and answers are not lifted;
- sessions whose managed settings set `allowManagedPermissionRulesOnly`, and Claude Code builds before 2.1.330.

If the `anthology` binary is not installed, the mod changes nothing and never downloads it. Set `ANTHOLOGY_MODS=0` to turn it off (see [Switches](mods.md#switches)).

## Commits carry only your name

Commits, PRs and code comments carry only the git config author. The writing agents never add a `Co-Authored-By` trailer or a "Generated with" line and never mention the assistant or AI.

A `PreToolUse` hook (`hooks/no-attribution.sh`) denies a `git commit` or `gh pr create|edit` command whose command text carries one. It only searches that text with `grep`; it parses and runs nothing. It catches the usual habit and is not a guarantee: `--trailer`, `-F` and `--body-file` are not checked.

Claude Code's own instructions tell the model to add that trailer, so the hook alone costs a retry on every commit. The fix is at the source: Claude Code's `attribution` setting.

- Setup writes `"attribution": {"commit": "", "pr": ""}` into an approved repo's committed `.claude/settings.json` (an empty string hides that part). A value you already set for either key is left as it is, and the setup preview says so.
- Setup does not touch your user-wide settings for this. To do the same in every repo, put that object in `~/.claude/settings.json` by hand.
- `/anthology:setup remove` takes out only what setup added.
- The hook stays as the backstop, and its block message names this setting.

## BrowserOS Neo is the only browser

All real-browser work goes through BrowserOS Neo, over its MCP tools (`mcp__browseros-neo__*`): signed-in work, forms, dashboards, local dev-server checks, UI review. Agents share the browser and each uses its own tab. You install BrowserOS Neo; agents never do. If it is unavailable, the agent stops and asks.

Nothing we ship launches Chrome. Browser test suites that a project already has (for example Playwright tests) still run, but only inside Docker.
