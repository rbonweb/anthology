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
   - Node missing: some project tooling needs it; explain, and continue.
   - rtk `null`: the rtk wiring is not part of this install yet; the rtk step is skipped.
3. If `duplicates` is not empty, Anthology already bundles superpowers and design-skills, so the
   separate installs conflict. Show each `command` (`claude plugin uninstall <plugin> -s <scope>`)
   and ask the user to run them. Do not run them for the user.
4. Show every change, then ask one yes/no question ("Apply these changes?"):
   - **rtk:** `rtk` (`version`, `target`, `dest`) is downloaded, hash-checked and installed. If it
     says skipped, say so.
   - **settings.json:** show `settings.diff` as a diff. Then print this note verbatim:
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

1. Preview first, so the user sees what is there:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py" preview --plugin-root "${CLAUDE_PLUGIN_ROOT}" --json
   ```

2. Say what remove does: it takes out the settings.json entries Anthology added (and restores
   `worktree.baseRef` to its previous value), deletes the downloaded rtk and Anthology's
   `state.json`. Rules the user added themselves are never touched. Memory notes, `.gitattributes`
   and the personal-memory folder are never deleted.
3. Ask one yes/no question: also drop the repo memory approval(s)? Recommend **no**: keeping them
   means memory keeps working if Anthology is reinstalled.
4. Run it, adding `--unapprove` (this repo) or `--unapprove-all` only if the user said yes:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py" remove --plugin-root "${CLAUDE_PLUGIN_ROOT}"
   ```

5. Report the result and any `warnings`. If it says settings.json is not valid JSON, show the message:
   nothing was changed, and the user can fix the file and run remove again.
