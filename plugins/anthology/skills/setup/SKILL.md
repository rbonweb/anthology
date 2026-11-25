---
name: setup
description: Set up Anthology (ATL, permissions, repo memory) or remove it again with `/anthology:setup remove`. Shows every change first and applies it only after you confirm. Run only when you ask for it.
disable-model-invocation: true
---

# /anthology:setup

Installs or removes Anthology's local pieces. Nothing changes until the user confirms. Every
command below prints one JSON object. Run them from the user's current folder.

Anthology supports macOS and Linux only. If a command exits with code 2, show its message and stop.

If a Bash call is blocked with "(setup)", the guard binary is missing. The block ends with the one command the guard lets through until the binary is installed, `/bin/sh '<guardrails plugin folder>/hooks/bootstrap.sh' guard --plugin guardrails --quiet --budget 300`: copy it from the message exactly (it names the real folder), run it with a Bash timeout of 360000 ms, and then run the blocked command again. It prints `status=installed` or `status=present` when the binary is in place. `status=failed:still downloading (...)` means a slow link: the download goes on in the background, so run the same command once more. Any other `status=failed:` reason: show it, and tell the user to start a new session while online, or to disable anthology-guardrails with /plugin.

## Setup (`/anthology:setup`)

1. Preview. This writes nothing:

   ```
   "${CLAUDE_PLUGIN_ROOT}/bin/anthology" setup preview --plugin-root "${CLAUDE_PLUGIN_ROOT}" --json
   ```

   If `memory.repo.skipped` is set and `memory.repo.repos_below` is present, the current folder is not a
   repo but holds repos. List them (`repos_below` holds absolute paths; `repos_below_more` counts the
   ones not listed), ask which one to set up, and run the preview again with `--cwd <abs repo path>`
   added. Use that same `--cwd` on the apply in step 5. Skipping the repo is a valid answer: then
   continue without it, and say that the repo's memory, attribution and auto memory settings were not
   changed. Never pick a repo yourself.

   The list can be incomplete in a very large folder: only the first 200 entries are scanned. Folder
   names come from disk, so single-quote the path in the command. If a path contains a quote, a
   backtick, a `$` or a control character, tell the user and do not run it. If the rerun with `--cwd`
   still says skipped, say so and stop; do not try another repo or loop.

2. Report the environment from `doctor.checks`:
   - `platform_supported` false: stop.
   - Docker missing or not running: tests and builds run in containers, so say so and ask the user
     to install or start it (OrbStack or Docker Desktop).
   - BrowserOS Neo missing: it is the only browser Anthology uses. Ask the user to install it
     themselves and add its MCP server to Claude Code. Never offer to install any browser.
   - `browseros_tools` false: agents call `mcp__browseros-neo__*`, so the BrowserOS MCP server must be
     named exactly `browseros-neo` (user, local, project or managed scope; a plugin-provided server gets
     a different tool prefix). Show the `detail`, which names what was found, and ask the user to
     rename it. `null` means no BrowserOS server is configured (the BrowserOS check above covers it),
     or that the only one was found in the project's `.mcp.json` and is not approved (the `detail`
     says so): ask the user to approve it in Claude Code, or add the server at user scope.
   - `subagent_model` false: `CLAUDE_CODE_SUBAGENT_MODEL` overrides every agent's model, so the
     roster's opus/sonnet/haiku tiers do not apply. Step 4 handles it from the top-level
     `subagent_model`. Continue either way.
   - `path_wrappers` or `plugin_data` false: a Claude Code change broke an assumption Anthology
     relies on (the plugin's `bin/` on PATH or the plugin data folder). Show the `detail`, say so, and
     ask the user to report it. `null` just means there is nothing to check yet.
   - `agent_memory_path` `null` with a `detail` naming a `.claude/agent-memory/<name>/` folder: only a
     possible problem (it may be the user's own agent). Show the `detail` as a note, never a failure.
   - Node missing: some project tooling needs it; explain, and continue.
   - `atl` `null`: the ATL (Anthology Token Layer) wiring is not part of this install yet; the ATL
     step is skipped.
   - `atl_savings`: what ATL did so far, from its own history: how many of the commands the hook
     saw went through ATL, and how many output tokens that saved. Show the `detail` as a note, so the
     user can judge whether ATL is worth keeping. `null` means there is no history yet (or it cannot
     be read); it is never a failure.
3. If `duplicates` is not empty, Anthology already bundles superpowers and design-skills, so the
   separate installs conflict. Show each `command` (`claude plugin uninstall <plugin> -s <scope>`)
   and ask the user to run them. Do not run them for the user. When an item's `command` is `null`
   its scope or name was not one Claude Code accepts: show the `plugin` and `scope` as given and
   offer no command (the user can look the install up with `claude plugin list`). If
   `installed_plugins` is `unreadable`, say that Claude Code's `installed_plugins.json` could not be
   read, so duplicates were not checked; setup can continue.
4. Show every change listed below, then ask for one confirmation: apply these changes and, if
   `memory.legacy_notes` is not empty, whether to move those old notes into Anthology memory
   (recommend **yes**). If `subagent_model.removable` is true, ask a separate yes/no question in the same
   message: remove `CLAUDE_CODE_SUBAGENT_MODEL` from your user settings.json (recommend **yes**;
   `/anthology:setup remove` puts it back). If `subagent_model.elsewhere` is empty, say this lets the
   roster's model tiers apply; otherwise say it stays set in each `where`, so the tiers apply only once
   it is removed there too:
   - **ATL:** `atl` (`version`, `target`, `dest`) is downloaded, hash-checked and installed. If it
     says skipped, say so.
   - **old tool leftovers:** ATL replaces the tool earlier releases installed under its old name.
     `legacy_rtk` lists what apply deletes once settings.json is written, in this order: the
     entry `legacy_rtk.state` names (`present` says whether it is there), the downloaded folder
     `legacy_rtk.dir` and the old tool's local history `legacy_rtk.history` (the command lines it
     saw, its recall store and tee spool). Say plainly that the old history is deleted for good and
     ATL starts with an empty one. Each item's `status` says `delete`, `nothing to delete` or why
     it is left in place. If nothing is there to delete, say nothing about it. The user's own
     allow rules for the old tool are never touched (a legacy `Bash(rtk *)` rule that an earlier
     setup added is replaced by `Bash(atl *)`).
   - **settings.json:** show `settings.diff` as a diff. Values outside
     `permissions.allow`, `permissions.deny` and `worktree.baseRef` show as `"<redacted>"`. Every value under
     those three is shown as it is, including your own rules, so a token written inside one of your allow or
     deny rules would reach the conversation. Then print this note verbatim:
     `settings.bypass_note`, which reads: "Note: `Bash(atl *)` also auto-approves `atl bypass <cmd>`, which runs any command. atl's hook still applies the deny and ask rules in your settings files (user, project, local and managed) to the command it wraps, but not rules given with `--settings` or `--disallowedTools`, server-managed settings or an MDM profile."
     If `settings.error` is set, show it and stop: nothing is written to a file that cannot be parsed.
   - **CLAUDE_CODE_SUBAGENT_MODEL:** `subagent_model.set_in` lists where it is set (never its value).
     If `removable` is true, show `subagent_model.diff` as a diff (the value reads `"<redacted>"`): this is
     the separate change the yes/no question above is about. For each item in `subagent_model.elsewhere`,
     say it is set in `where` and show `how` (one line on removing it there). Setup never changes those
     places. If `set_in` is empty, say nothing about it.
   - **repo memory:** `memory.repo`. If it has a `key`, the repo `<key>` is approved for memory and
     a union-merge rule is added to its `.gitattributes`. If it says "not a git repo", this step
     is skipped (no repo was picked from `repos_below`).
   - **Claude Code's own auto memory:** `memory.repo.auto_memory` is the one memory change to the repo's
     committed `.claude/settings.json`: it adds `"autoMemoryEnabled": false`, so Claude Code's built-in
     auto memory does not run beside the repo's `.claude/memory/` and the personal memory. Show its `path`
     and `diff` as a diff (the value reads `"<redacted>"`; say it is `false`). Say plainly that the file
     is committed: once pushed, auto memory is off for everyone who uses the repo, including teammates who
     do not use Anthology. Auto memory also covers Claude Code's subagent memory, so in this repo roster
     agents no longer get their `MEMORY.md` preloaded (each agent's prompt tells it to read its own).
     Other keys in that file are kept. Your user-wide settings are not touched for this, and other repos
     keep Claude Code's auto memory.
     `/anthology:setup remove` puts the file back. If it has `skipped`, show the reason: the file is left
     alone and Claude Code's auto memory stays on.
   - **Commit and PR attribution:** the same `memory.repo.auto_memory` diff also holds
     `"attribution": {"commit": "", "pr": ""}`, so Claude Code stops telling the model to add a
     co-author trailer or a "Generated with" line (the no-attribution hook stays as the backstop). Say it
     is committed too, so it applies to everyone who uses the repo. A value you already set is never
     overwritten: each item in `notes` names a key that is left as it is, or says your user-wide settings
     also set `attribution` (the repo's value wins in this repo; setup only reads that file); show them. Print `user_scope`
     verbatim: setup does not touch your user-wide settings for this, and you can set the same key there
     by hand.
   - **personal memory:** the folder `memory.personal_dir` is created (private to the user).
   - **old memory notes:** if `memory.legacy_notes` is not empty, these are notes Claude Code's auto memory
     saved for this folder before Anthology. With the user's yes, each note is **copied** (never moved
     or deleted) by its `destination`: `repo` (project, feedback and reference notes) goes into
     `.claude/memory/` under the file name `target`, where everyone who can see the repo can read it;
     `personal` (user notes) goes into personal memory. A note with `destination` `none` is not moved:
     show its `reason` (privacy findings name a kind and a line, never the text; a note without a
     type is also listed; so is a note whose target is not a safe place to write). Show each note's `name`,
     `description` and `destination`. `target_status` says what is already under the name `target` in that
     folder: `new`, `identical` (nothing is written) or `conflict` (the note is saved beside it as
     `target_would_create`). Notes the move itself refuses at write time come back in `memory.blocked`. The originals stay
     where they are, and Claude Code no longer loads them. Without the yes, nothing goes into the repo.
5. Once confirmed, apply, passing the digest from the preview so a changed settings file is never overwritten:

   ```
   "${CLAUDE_PLUGIN_ROOT}/bin/anthology" setup apply --plugin-root "${CLAUDE_PLUGIN_ROOT}" --expect-sha256 <settings.sha256>
   ```

   Add `--cwd <abs repo path>` (single-quoted, as in step 1) if the user picked a repo from
   `repos_below`. Add `--move-old-notes` if the user said yes to moving the old notes (it cannot be
   combined with `--skip-approve`); add nothing if they said no. Add `--skip-approve` if the user does not want this repo
   approved: then the repo's settings are not changed either. Add
   `--remove-subagent-model` only if the user said yes to removing `CLAUDE_CODE_SUBAGENT_MODEL`. If it reports that
   settings.json changed since the preview, run the preview again and ask again.
6. Report the result: what changed (`subagent_model.removed` says whether `CLAUDE_CODE_SUBAGENT_MODEL`
   was removed), any `warnings`, and `memory.approve_error` if present (the repo is
   still approved; say which file was refused). `memory.auto_memory` says whether the repo's
   `.claude/settings.json` was changed (`changed`) or why not (`skipped`); when it was, tell the user to
   commit it, and show its `notes` (attribution keys left as the user had them, and a user-wide `attribution`). `memory.migrated` lists the old notes copied into the repo (`repo`) and into personal memory
   (`personal`), by the names they now have. If `memory.blocked` is not empty, list each `file` with its
   `reason`: those notes were not moved. If `memory.originals` is set, show it: the old notes were left in
   place, never deleted, and are no longer loaded. If `CLAUDE_CODE_SUBAGENT_MODEL` was
   removed, say that if session start still warns about it after the restart, it is also set somewhere
   else (often the shell profile), and `/anthology:setup` shows where. Tell the user to restart the
   session so the new permissions and the ATL hook take effect. `legacy_rtk` says what was deleted
   of the old tool's leftovers (a `warnings` entry names anything that could not be deleted).

## Remove (`/anthology:setup remove`)

1. Preview the removal. This writes nothing:

   ```
   "${CLAUDE_PLUGIN_ROOT}/bin/anthology" setup remove --preview --plugin-root "${CLAUDE_PLUGIN_ROOT}"
   ```

   If it reports an error (for example settings.json is not valid JSON), show the message and stop.
2. Show every change it would make:
   - `settings.diff` as a diff: the settings.json entries Anthology added come out, and
     `worktree.baseRef` goes back to its previous value, as does `env.CLAUDE_CODE_SUBAGENT_MODEL` if setup
     removed it (unless the user has set it again since). Rules the user added themselves are never touched.
     Values outside `permissions.allow`, `permissions.deny` and `worktree.baseRef` show as `"<redacted>"`;
     those three are shown as they are, including your own rules.
   - `atl.dir`: the downloaded ATL folder that would be deleted (or why it is left in place).
   - `atl_history.dir`: ATL's local history (the command lines it saw, its recall store and tee spool),
     which would be deleted (`status` says `delete`, `nothing to delete` or why it is left in place).
   - `legacy_rtk.dir` and `legacy_rtk_history.dir`: the same two for the old tool that earlier
     releases installed, which would be deleted as well (same `status` values).
   - `state.path`: Anthology's `state.json`, which would be deleted.
   - `memory.repo_settings`: each repo whose `.claude/settings.json` setup edited. Show its `path` and `diff`
     (the `autoMemoryEnabled` and `attribution` lines setup added come out, or go back to the value they had;
     the file is deleted only if setup created it and nothing else is in it). Claude Code's auto memory and
     its commit and PR attribution run again there. These
     files are committed, so tell the user to review and commit the change. A `note`
     says why a file is left alone (a `left in place` one is also in `warnings`).
   - Memory notes, `.gitattributes` and the personal-memory folder are never deleted. `memory.this_repo_approved`
     and `memory.unapprove_all` list the repo approvals that the optional flags would drop.
3. Ask the user to confirm (yes/no) before anything is removed. If there are approvals listed, in the same
   question ask whether to also drop them, recommending **no**: keeping them means memory keeps working
   if Anthology is reinstalled.
4. After the confirmation, run it, adding `--unapprove` (this repo) or `--unapprove-all` only if the user said yes:

   ```
   "${CLAUDE_PLUGIN_ROOT}/bin/anthology" setup remove --plugin-root "${CLAUDE_PLUGIN_ROOT}"
   ```

5. Report the result and any `warnings`. If it says settings.json is not valid JSON, show the message:
   nothing was changed, and the user can fix the file and run remove again.
