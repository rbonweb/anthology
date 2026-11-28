# Setup, updating and removing

This page covers what `/anthology:setup` checks and changes, what Anthology does at session start, how to replace the official superpowers and design-skills plugins, and the full steps for updating and removing.

## What `/anthology:setup` does

It checks your machine, then shows every change and applies them only after you confirm once. Run it in each repo you want memory in. The first run also does the one-time machine setup.

1. **Doctor.** Checks:
   - the `anthology` binary that the guardrails use is installed and matches its `anthology.lock`;
   - `node` and `docker` are on your path, and Docker is running;
   - BrowserOS Neo is installed or configured as an MCP server. It is found as any `BrowserOS*.app` in `/Applications` or `~/Applications`, a `browseros` binary on your path, or an MCP server in any scope. A separate check says whether that server is named `browseros-neo`, because agents call `mcp__browseros-neo__*`;
   - the ATL binary and the platform. On Windows, setup refuses and stops.

   Doctor also checks the compatibility assumptions Anthology relies on: the plugin's `bin/` wrappers exist and (inside Claude Code) are on your path, the plugin data folder exists, and agent notes land in `.claude/agent-memory/anthology-<agent>/` (agents save them with `anthology-mem save`; Claude Code preloads them only while its auto memory is on). If `CLAUDE_CODE_SUBAGENT_MODEL` is set, it says where (see [Agent models](agents.md#agent-models)). After a first setup it also checks that the plugin-read permission (step 4) still matches where the plugin is installed.
2. **Duplicates.** If you have the official `superpowers` or `design-skills` installed, it recommends uninstalling them and shows the commands for you to run (see [Replacing the official superpowers and design-skills](#replacing-the-official-superpowers-and-design-skills)).
3. **Binaries.** Makes sure the `anthology` binary that `anthology-guardrails` pins is installed, with the same download and checks as at session start (see [The `anthology` binary](binary.md)). Then it downloads the pinned ATL binary for your platform from this repo's releases, checks its sha256 against `atl.lock` and installs it in the plugin's data folder. If the download or the check fails, nothing is installed and setup shows a warning.
4. **Settings.** Edits `~/.claude/settings.json`:
   - allows `Bash(atl *)`. ATL's hook never widens it: a command is rewritten, and a typed `atl bypass <cmd>` or other `atl …` line is let through, only when the command is read-only or your own allow rules cover it, and it asks otherwise (see [The `Bash(atl *)` risk](atl.md#the-bashatl--risk));
   - allows `Bash(anthology-mem save *)`, so agents that cannot edit files can still save their own memory notes (see [Saving notes](memory.md#saving-notes-anthology-mem-save));
   - denies `Read(...)` on the Chrome and Chromium data folders and everything under them: `~/Library/Application Support/Google` and `.../Chromium` on macOS, `~/.config/google-chrome` and `~/.config/chromium` (with the `-beta`, `-dev` and similar channel variants) on Linux;
   - allows `Read(...)` on the parent folder of the plugin's install location, so agents can read their own knowledge files. Outside auto mode, Claude Code does not auto-approve those reads;
   - sets `worktree.baseRef` to `"head"`, so agents that work in a worktree start from your current commit instead of the remote's default branch;
   - only if you say yes to its own question: removes `CLAUDE_CODE_SUBAGENT_MODEL` from the `env` block (see [Agent models](agents.md#agent-models)).
5. **Memory.** Approves the current repo for memory (you can decline this step). That adds two `merge=union` lines to the repo's `.gitattributes` and `"autoMemoryEnabled": false` to the repo's committed `.claude/settings.json`, merged into what is there.
   - The preview shows the `.gitattributes` change as a diff. It adds only the lines that are missing and keeps the rest of the file. Commit the file: a worktree or clone made from a commit without it does not union-merge the index files.
   - The setting turns off Claude Code's own auto memory in this repo, so the repo's `.claude/memory/` and your personal memory are the only memory. Other repos keep auto memory, and your user-wide settings are not touched for this.
   - The file is committed, so once you push it, auto memory is off for everyone who uses the repo, including teammates who do not use Anthology.
   - Auto memory also covers Claude Code's subagent memory, so in this repo roster agents no longer get their `MEMORY.md` preloaded. Each agent's prompt tells it to read its own.
   - The same step adds the `attribution` setting to that file (see [Commits carry only your name](guardrails.md#commits-carry-only-your-name)). A value you already set for either key is left as it is, and the preview says so. `remove` takes out only what setup added.
   - Setup also creates the personal-memory folder.
   - If Claude Code kept notes for this folder before Anthology (the old path-keyed `~/.claude/projects/<encoded path>/memory/`), setup lists each one and where it would go. If you say yes (recommended), `project`, `feedback` and `reference` notes are copied into the repo's `.claude/memory/` as `<type>-<slug>.md`, and `user` notes into personal memory. The originals are left in place, never deleted, and Claude Code no longer loads them. Notes the privacy scan flags (home paths, emails, IP addresses, private host names, ssh targets and key paths, secrets) and notes without a type are listed and not moved. Without your yes, nothing goes into the repo.

When it finishes, restart the session so the new permissions and the ATL hook take effect. Nothing is written to `~/.claude/CLAUDE.md`.

After it finishes, setup asks whether to run a malicious-code check on this repo. It says first that Claude Code is already open in the repo, so the repo's hooks, env settings and MCP servers may have run. The check still reads install scripts, editor tasks and hidden code that have not run yet. For a repo you did not write, the safest check runs before you open Claude Code in it, with `/anthology:check-repo <path>` from a session in another folder. If you say yes, setup runs the check with `--opened`. See [Check a repo before you open it](check-repo.md).

## What happens at session start

After setup, some things happen on their own at session start. Anthology delivers the routing guide, runs the guardrails self-test (see [Guardrails](guardrails.md)), reconciles the ATL binary with `atl.lock`, and in approved repos keeps memory working (see [Memory](memory.md)).

It also tells you when something needs attention:

- Until you have run `/anthology:setup`, it prints a one-line reminder to do so.
- It warns you if the plugin has moved so that agents can no longer read their knowledge files. Re-run `/anthology:setup` to fix that.
- It prints one loud line when a compatibility assumption breaks: Claude Code did not pass `CLAUDE_PLUGIN_DATA`, the plugin's `bin/` wrappers are missing or not executable, agent memory was written somewhere other than `anthology-<agent>/`, or `CLAUDE_CODE_SUBAGENT_MODEL` is set (run `/anthology:setup` to remove it).
- When a release changes the `settings.json` entries that setup makes and one of them is missing, it prints "anthology settings changed in this release — run /anthology:setup to review" and changes nothing itself. If you deleted one of those entries on purpose (for example `Bash(atl *)`), the line returns at each session start after every such release, until you run setup again.
- When the official `superpowers` plugin is enabled next to Anthology's own `anthology-superpowers`, it prints one line at each session start (not on resume, clear or compact) that names both and gives the command that disables the official one, for example `claude plugin disable superpowers@claude-plugins-official -s user`. Both would otherwise load their instructions. It names the official plugin and not `anthology-superpowers` because `anthology` lists `anthology-superpowers` as a dependency, and Claude Code refuses to disable a plugin that an enabled one requires. A plugin manifest has no way to mark a dependency optional.
- If the personal-memory folder is a symlink or not a folder, it prints a line saying personal memory was not loaded and nothing was moved or copied.
- When the repo holds files that run code when Claude Code or an editor opens it (hooks, env or helper commands in `.claude/settings.json` or `.claude/settings.local.json`, `.mcp.json` servers, a VS Code task that runs on folder open, `.envrc`), it prints one line naming them, once for each repo, and points to `/anthology:check-repo`. It is not printed on resume, clear or compact (see [If the repo is already open](check-repo.md#if-the-repo-is-already-open)).

When another program drives the session, Anthology stays quiet instead (see [Host mode](agents.md#host-mode)).

## Replacing the official superpowers and design-skills

Anthology carries its own copies, so setup recommends uninstalling the official ones:

- **superpowers.** `anthology-superpowers` is the same skills library under a different name, so it does not clash with the official `superpowers` (two enabled plugins with one name lose hooks and collide on commands). Both would work side by side, but their instructions would load twice. Anthology's copy also adds `references/large-builds.md` to subagent-driven-development: the rules the main session follows on long multi-agent builds (start work against published interfaces, test once at the cheapest level, one review per task, batch merges, plain-language status reports). The main session reads it only when it runs such a build.
- **design-skills.** Anthology uses it only inside its agents' knowledge. An installed copy would expose its skills loose in the main session.

Setup finds the duplicates and shows the commands. You run them yourself; setup does not:

```text
claude plugin uninstall <plugin> -s <scope>
```

`claude plugin list` shows where each copy came from and at which scope it is installed.

## Updating

Auto-update is off by default for third-party marketplaces such as `anthology`. Turn it on in `/plugin` > **Marketplaces** > `anthology`. Claude Code then checks the marketplace after a session starts and updates the installed plugins on disk. It shows "Plugin updated", and the new version loads after `/reload-plugins` or at your next launch.

Setting `DISABLE_AUTOUPDATER=1`, `DISABLE_UPDATES=1` or `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` also turns plugin auto-update off unless you set `FORCE_AUTOUPDATE_PLUGINS=1`.

Without auto-update, update by hand: run `claude plugin marketplace update anthology`, then `claude plugin update anthology@anthology` (and the same for `anthology-guardrails` and `anthology-superpowers`), and restart Claude Code.

What an update changes:

- The routing guide updates with the plugin.
- The `anthology` binary that the new `anthology.lock` pins is installed beside the old one at the next session start.
- ATL is reconciled to the pinned version at each session start.
- The memory index format is versioned and rebuilt when it changes.

### Two copies of a plugin in the cache

The plugin cache can hold two folders for one dependency, `X.Y.Z` and `X.Y.Z-<sha>`. This is Claude Code's normal cache scheme for dependencies with a version range, and only the `-<sha>` copy is used. Nothing is broken, and you do not need to act.

Claude Code removes old versions on its own, about 14 days after they are orphaned. `/plugin` shows what is installed.

To clean up by hand instead, quit all Claude Code sessions first. Then delete only the version folders whose path is not an `installPath` in `~/.claude/plugins/installed_plugins.json`. Never delete a folder that file lists.

## Removing

Run `/anthology:setup remove`. It shows every change first and asks you to confirm. It:

- takes out the settings entries Anthology added: the `Bash(atl *)` and `Bash(anthology-mem save *)` allows, the Chrome deny rules and the plugin-read allow. Rules you added yourself are never touched. `worktree.baseRef` goes back to its previous value, and so does `CLAUDE_CODE_SUBAGENT_MODEL` if setup removed it;
- takes out the `autoMemoryEnabled` and `attribution` lines setup added to each approved repo's committed `.claude/settings.json` (or restores the value they had), so Claude Code's own auto memory and attribution run again there. The file is deleted only if setup created it and nothing else is in it. Review and commit that change;
- deletes the downloaded ATL, ATL's local history (`~/.claude/anthology/atl/`), the same two for the legacy rtk install if they are still there (`<plugin data>/rtk/` and `~/.claude/anthology/rtk/`) and `~/.claude/anthology/state.json`;
- optionally removes the repo approvals. It asks, and recommends keeping them, so memory keeps working if you reinstall.

It never deletes memory notes, the `.gitattributes` lines, the personal-memory folder or your exports.

Then uninstall the plugin:

```text
claude plugin uninstall anthology@anthology --prune -y
```

`--prune -y` also removes the dependencies that were installed automatically. Uninstalling without `--keep-data` also removes the plugin's data folder (the ATL binaries and the memory search index).

Remove keeps the `anthology` binaries in `~/.claude/anthology/bin/` while `anthology-guardrails` is still installed, because its guard needs them. After you uninstall, delete that folder yourself (about 6 MB per version).
