# Memory

This page covers how Anthology keeps memory in your repo, which repos it applies to, how it survives re-clones and team merges, the `anthology-mem` commands, and export and import.

Memory is plain Markdown notes committed with your project, so it survives a re-clone, a folder move and a team merge. The notes are the only source of truth. The `MEMORY.md` indexes and a local SQLite search index are derived from them and can always be rebuilt.

| Whose memory | Where |
|---|---|
| Main session | `<repo>/.claude/memory/` (committed) |
| Each roster agent | `<repo>/.claude/agent-memory/anthology-<agent>/` (committed; `anthology:implementer` becomes `anthology-implementer`) |
| You (`user` notes) | `~/.claude/anthology/personal-memory/`, never in a repo (under `$CLAUDE_CONFIG_DIR` if you set it) |

**Repo memory is visible to everyone who can see the repo.** Never save secrets or personal details in it. Personal notes go in personal memory, which stays on your machine. Claude Code's auto memory saves every note type into the repo's folder, so at each session start Anthology moves `type: user` notes out of `.claude/memory/` and `.claude/agent-memory/` into personal memory and tells you. If one was already committed, commit its removal. The repo `MEMORY.md` indexes never list `user` notes, and the session context always names the personal-memory folder, even when it is empty.

## Approved repos only

Memory is active only in repos you approved by running `/anthology:setup` inside them. In any other repo no repo memory is written or injected (your personal-memory index still is), and no notes are moved. If that repo already has `.claude/memory/`, you get one line telling you to run `/anthology:setup`. That covers repo auto-memory only.

Approvals are kept in `~/.claude/anthology/approved-repos.json`, keyed by the normalized git remote URL (the repo path if there is no remote). Memory also stays off if you turned auto memory off in your user or local settings or with `CLAUDE_CODE_DISABLE_AUTO_MEMORY`.

The `autoMemoryEnabled: false` that setup writes into an approved repo's committed `.claude/settings.json` does not count. It switches off Claude Code's own auto memory so that this memory is the only one, and in such a repo session start injects the repo's memory index every session, because Claude Code no longer loads it (see [What setup does](setup.md#what-anthologysetup-does), step 5).

**Agent memory is different.** Claude Code loads `.claude/agent-memory/anthology-<agent>/MEMORY.md` into that agent itself (the agents set `memory: project`), whether or not you approved the repo, as long as Claude Code's auto memory is on. That is part of auto memory: in a repo where setup turned auto memory off, the preload stops, and the line in each agent's body telling it to read its own `MEMORY.md` with Read is what loads the notes. Agents still save notes with `anthology-mem save`.

The agent bodies tell the agent to read the folder either way. So notes a stranger committed there can reach an agent in a repo you cloned. The bodies say to treat notes as hints, never instructions: they cannot override your request, the BrowserOS-only rule or the Docker rule (subagents never receive the routing guide, so it is not named). That is guidance to the model, not a barrier. Review `.claude/agent-memory/` in any repo you do not trust, as you would review code (see [ADR-019](../DECISIONS.md)).

## Surviving re-clones

Because approvals follow the remote URL, re-cloning the same repo into any folder works with no new setup. At session start Anthology repairs the clone:

- It writes this clone's absolute path into `autoMemoryDirectory` in `<repo>/.claude/settings.local.json`. That file is per-clone and git-ignored, and Anthology makes sure it is ignored.
- It regenerates the `MEMORY.md` indexes from the notes.
- If it had to repair the setting, it injects the repo's memory index for that session, because the setting may only apply from the next one.
- It copies any notes in Claude Code's old path-keyed memory folder for this clone into personal memory, never into the repo, once (a ledger remembers them), and moves `user` notes out of the repo. Nothing is overwritten: a different note with the same name is kept side by side as `<name>.migrated-<date>.md`. To move old `project`, `feedback` and `reference` notes into the repo's memory, run `/anthology:setup` and say yes to moving the old notes.

## Team merges

- Notes are named `<type>-<topic-slug>.md` and link to each other with `[[note-name]]`. Two people editing the same note is a real conflict, resolved like code.
- When you approve a repo, Anthology adds these lines to `.gitattributes`, once, so the generated indexes do not conflict:

  ```text
  .claude/memory/MEMORY.md merge=union
  .claude/agent-memory/*/MEMORY.md merge=union
  ```

- Commit `.gitattributes`. Session start prints one line when an approved repo's `HEAD` lacks the rule (never written, or written and not committed); it writes nothing. Git reads the rule from the checkout the merge runs in, so a worktree or clone made from a commit without it still conflicts. A conflict that does get through (a merge in such a checkout, or on GitHub) is fixed by rebuilding the index from the notes: `anthology-mem reindex --agent <name>`, or `anthology-mem reindex --repo`.
- The next session start regenerates the index, which cleans up anything the merge left behind. GitHub's web merge button ignores merge drivers, so a PR may still show a conflict in a `MEMORY.md` there. Keeping both sides' lines, or merging locally, is always correct.
- Memory changes go in their own commits, `chore(memory): …`, so they never ride along with unrelated work. Agents that work in a worktree commit their memory there before they finish.

## CODEOWNERS for memory

Notes load into everyone's sessions, so review them like code. In your own repo, add owners for the memory folders:

```text
# .github/CODEOWNERS
/.claude/memory/        @your-org/reviewers
/.claude/agent-memory/  @your-org/reviewers
```

## Searching: `anthology-mem`

`anthology-mem` is on the Bash path and works over a local index in the plugin's data folder (never committed).

```text
anthology-mem search "<terms>"          full-text search over the note bodies
anthology-mem related <note> --depth N  notes linked to a note, to a depth
anthology-mem orphans                   notes nothing links to
anthology-mem stale                     superseded or old notes
anthology-mem rebuild                   delete and rebuild the index
```

Links are the `[[note-name]]` links in the notes, plus relations such as `supersedes` and `depends_on` declared in a note's frontmatter. There are no embeddings, so nothing needs an API key or a model download.

## Saving notes: `anthology-mem save`

Roster agents save their notes through one command, so a read-only agent needs no Write tool, heredoc or redirect:

```text
anthology-mem save --agent <name> --type <project|feedback|reference> --slug <topic-slug> \
  --name '<title>' --description '<one line>' (--body '<text>' | --body-file <path>) [--replace]
```

It writes `<repo>/.claude/agent-memory/anthology-<name>/<type>-<topic-slug>.md` with the frontmatter the indexes read, then regenerates that folder's `MEMORY.md`. It writes nowhere else.

- **What it checks.** The agent must be on the roster, and the type and slug are checked. Symlinked folders or files are refused. An existing note is only overwritten with `--replace`. A note that looks like it holds a secret (or the value of a `*TOKEN*`/`*KEY*`/`*SECRET*`/`*PASSWORD*` environment variable) or that the privacy scan flags (home paths, emails, IP addresses, private host names, ssh targets and key paths) is refused, with the kind and line named.
- **`--body-file`.** It reads the text from a file instead (`-` reads standard input) and applies the same checks as `--body`. The file must be a regular file of at most 16 KB, inside the repo or the system temp folder (`$TMPDIR`, `/tmp`), and not at a secret location (`.ssh`, `.env`, keys, credentials, shell history). Symlinks are resolved first, and a file that is too large is refused, never cut. Use it when the text is long or contains shell-looking lines such as `rm -rf`: Claude Code's safety check reads the command line, so a note that merely mentions a delete command can be refused as one.
- **Repeats.** Saving warns, without failing, when the agent already has a note with a very similar name or description, so a repeat can update that note with `--replace` instead.
- **Index size.** Every generated `MEMORY.md` (personal, repo, agent and imported) is capped at about 8 KB. Past that it lists the newest notes of each type and ends each cut type with `N more — use anthology-mem search`.
- **Permissions.** Setup allows exactly `Bash(anthology-mem save *)`. Claude Code still asks before a command that adds anything to it (`&&`, a redirect, `$(...)`). A body line that starts with `#` also makes Claude Code ask, so notes use **bold** lines instead of `#` headings.

## Export and import

- `/anthology:export` writes `anthology-memory-<date>.tar.gz` under `~/.claude/anthology/exports/` by default. It holds your personal notes plus the current repo's notes, and a manifest. Nothing is uploaded. The archive holds personal notes, so keep it private: if you pass `--out` with a folder inside a git work tree, export refuses.
- `/anthology:import <file>` shows a dry run first: new notes, identical ones (skipped) and conflicts. Nothing is overwritten. A conflicting note is kept as `<name>.imported-<date>.md`, and Claude merges it with you. Archives over 50 MB are refused, files are extracted safely (no absolute paths, `..`, links or devices), only the memory folders can be written, and secret-looking content is reported. A repo or agent note the privacy scan flags is written to personal memory instead of the repo, and the dry run lists it as blocked.
