---
name: import
description: Import an Anthology memory archive (anthology-memory-<date>.tar.gz) after a dry run you confirm. Nothing is overwritten; differing notes are kept side by side and merged with you. Run only when you ask for it.
argument-hint: <archive file>
disable-model-invocation: true
---

# /anthology:import <file>

Bring notes from an `anthology-memory-<date>.tar.gz` into this machine's memory. The importer
only writes to personal memory, this repo's `.claude/memory/` and its `.claude/agent-memory/`
folders (the repo ones only if the repo is approved, and only for notes that pass the privacy
scan), and never overwrites a note.

1. **Dry run.** Show what would happen. This writes nothing:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/memory_portable.py" import "<file>" --dry-run
   ```

   The first line of the output carries the archive's `sha256`; note it. If the command
   exits with code 2 the archive was rejected as unsafe or malformed: show the message from
   stderr, stop, and do not try to work around it.
2. **Show the plan** to the user: the new notes, the identical ones (skipped), the conflicts
   (kept side by side as `<name>.imported-<date>.md`), repo notes skipped because this folder
   is not an approved repo, and any secret-looking content (labels only). Repo and agent notes
   the privacy scan blocks (home paths, emails, private IPs or hosts, ssh targets and key paths,
   secrets), and `type: user` notes, are listed as blocked with a reason: they go to personal
   memory instead of the repo, and the apply step checks them again.
3. **Wait for the user's confirmation.** Do not apply without it.
4. **Apply** exactly the archive that was shown, by passing the sha256 from the dry run:

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/memory_portable.py" import "<file>" --yes --sha256 <sha256 from the dry run>
   ```

   Report how many notes were written, and which repo notes went to personal memory because
   the privacy scan blocked them. The `MEMORY.md` indexes are regenerated for you.
5. **Merge the conflicts.** For each `<name>.imported-<date>.md` (the apply output lists
   them), read it next to the existing `<name>.md`, and go through the differences with the
   user: keep what they want in `<name>.md` (edit it), then delete the `.imported-` file
   once it is merged. Never delete an `.imported-` file before the user agrees the merge is done.
