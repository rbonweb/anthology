---
name: setup
description: Set up Anthology (rtk, permissions, repo memory) or remove it again with `/anthology:setup remove`. Shows every change first and applies it only after you confirm. Run only when you ask for it.
disable-model-invocation: true
---

# /anthology:setup

Installs or removes Anthology's local pieces. Nothing changes until the user confirms. Every
command below prints one JSON object. Run them from the user's current folder.

Anthology supports macOS and Linux only. If a command exits with code 2, show its message and stop.

## Setup (`/anthology:setup`)

1. Preview. This writes nothing:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py" preview --plugin-root "${CLAUDE_PLUGIN_ROOT}" --json
   ```

2. Report the environment from `doctor.checks`:
   - `platform_supported` false: stop.
   - Docker missing or not running: tests and builds run in containers, so say so and ask the user
     to install or start it (OrbStack or Docker Desktop).
   - BrowserOS Neo missing: it is the only browser Anthology uses. Ask the user to install it
     themselves and add its MCP server to Claude Code. Never offer to install any browser.
   - `browseros_tools` false: agents call `mcp__browseros-neo__*`, so the BrowserOS MCP server must be
     named exactly `browseros-neo` (user, local, project or managed scope; a plugin-provided server gets
     a different tool prefix). Show the `detail`, which names what was found, and ask the user to
     rename it. `null` means no BrowserOS server is configured (the BrowserOS check above covers it).
   - `subagent_model` false: show the `detail` warning clearly. `CLAUDE_CODE_SUBAGENT_MODEL` overrides
     every agent's model, so the roster's opus/sonnet/haiku tiers do not apply. Anthology never
     changes it, and whether to keep it is the user's choice. Continue either way.
   - `path_wrappers` or `plugin_data` false: a Claude Code change broke an assumption Anthology
     relies on (the plugin's `bin/` on PATH or the plugin data folder). Show the `detail`, say so, and
     ask the user to report it. `null` just means there is nothing to check yet.
   - `agent_memory_path` `null` with a `detail` naming a `.claude/agent-memory/<name>/` folder: only a
     possible problem (it may be the user's own agent). Show the `detail` as a note, never a failure.
   - Node missing: some project tooling needs it; explain, and continue.
   - rtk `null`: the rtk wiring is not part of this install yet; the rtk step is skipped.
3. If `duplicates` is not empty, Anthology already bundles superpowers and design-skills, so the
   separate installs conflict. Show each `command` (`claude plugin uninstall <plugin> -s <scope>`)
   and ask the user to run them. Do not run them for the user.
4. Show every change, then ask one yes/no question ("Apply these changes?"):
   - **rtk:** `rtk` (`version`, `target`, `dest`) is downloaded, hash-checked and installed. If it
     says skipped, say so.
   - **settings.json:** show `settings.diff` as a diff. Values setup does not change show as
     `"<redacted>"`, so no secret in the file reaches the conversation. Then print this note verbatim:
     `settings.bypass_note`, which reads: "Note: `Bash(rtk *)` also auto-approves `rtk bypass <cmd>`, which runs any command; the guardrails and deny rules still apply to the wrapped command."
     If `settings.error` is set, show it and stop: nothing is written to a file that cannot be parsed.
   - **repo memory:** `memory.repo`. If it has a `key`, the repo `<key>` is approved for memory and
     a union-merge rule is added to its `.gitattributes`. If it says "not a git repo", this step
     is skipped.
   - **personal memory:** the folder `memory.personal_dir` is created (private to the user).
5. On yes, apply, passing the digest from the preview so a changed settings file is never overwritten:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py" apply --plugin-root "${CLAUDE_PLUGIN_ROOT}" --expect-sha256 <settings.sha256>
   ```

   Add `--skip-approve` if the user does not want this repo approved. If it reports that settings.json
   changed since the preview, run the preview again and ask again.
6. Report the result: what changed, any `warnings`, and `memory.approve_error` if present (the repo is
   still approved; say which file was refused). Tell the user to restart the session so the new
   permissions and the rtk hook take effect.

## Remove (`/anthology:setup remove`)

1. Preview the removal. This writes nothing:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py" remove --preview --plugin-root "${CLAUDE_PLUGIN_ROOT}"
   ```

   If it reports an error (for example settings.json is not valid JSON), show the message and stop.
2. Show every change it would make:
   - `settings.diff` as a diff: the settings.json entries Anthology added come out, and
     `worktree.baseRef` goes back to its previous value. Rules the user added themselves are never touched.
     Values remove does not change show as `"<redacted>"`.
   - `rtk.dir`: the downloaded rtk folder that would be deleted (or why it is left in place).
   - `state.path`: Anthology's `state.json`, which would be deleted.
   - Memory notes, `.gitattributes` and the personal-memory folder are never deleted. `memory.this_repo_approved`
     and `memory.unapprove_all` list the repo approvals that the optional flags would drop.
3. Ask the user to confirm (yes/no) before anything is removed. If there are approvals listed, in the same
   question ask whether to also drop them, recommending **no**: keeping them means memory keeps working
   if Anthology is reinstalled.
4. After the confirmation, run it, adding `--unapprove` (this repo) or `--unapprove-all` only if the user said yes:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py" remove --plugin-root "${CLAUDE_PLUGIN_ROOT}"
   ```

5. Report the result and any `warnings`. If it says settings.json is not valid JSON, show the message:
   nothing was changed, and the user can fix the file and run remove again.
