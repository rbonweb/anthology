---
name: export
description: Export your Anthology memory notes (personal notes, plus this repo's notes if it is approved) to a portable anthology-memory-<date>.tar.gz. Run only when you ask for it.
disable-model-invocation: true
---

# /anthology:export

Pack the memory notes into one archive you can move to another machine. Nothing is uploaded.

1. Run the exporter from the folder you are working in (it writes the archive there; add
   `--out <folder>` to put it elsewhere):

   ```
   python3 "${CLAUDE_PLUGIN_ROOT}/scripts/memory_portable.py" export --json
   ```

2. Report to the user, from the JSON result:
   - the archive path (`path`);
   - the counts: personal notes, repo notes, agent notes (`counts`);
   - if `repo_included` is false, say that this folder is not an approved repo, so only the
     personal notes were exported (`/anthology:setup` approves a repo).
3. If `secrets` is not empty, list each file and pattern label it names (labels only, never
   the secret itself) and recommend the user review those notes before sharing the archive.
4. Remind the user that the archive contains their **personal notes**: keep it private and
   do not commit it or share it publicly.
