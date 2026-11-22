---
name: export
description: Export your Anthology memory notes (personal notes, plus this repo's notes if it is approved) to a portable anthology-memory-<date>.tar.gz. Run only when you ask for it.
disable-model-invocation: true
---

# /anthology:export

Pack the memory notes into one archive you can move to another machine. Nothing is uploaded.

1. Run the exporter. By default it writes the archive to Anthology's own folder
   (`<anthology home>/exports/`, private to the user), never into the current folder or a repo:

   ```
   "${CLAUDE_PLUGIN_ROOT}/bin/anthology" memory export --json
   ```

   The user may pass `--out <folder>` to choose another place, but the exporter refuses any
   folder inside a git repo (exit code 2: "export refuses to write personal notes inside a
   git repo"). Never work around that refusal, for example by exporting elsewhere and copying
   the archive into a repo.
2. Report to the user, from the JSON result:
   - the archive path (`path`): this is where the file is, and they move it themselves;
   - the counts: personal notes, repo notes, agent notes (`counts`);
   - if `repo_included` is false, say that this folder is not an approved repo, so only the
     personal notes were exported (`/anthology:setup` approves a repo).
3. If `secrets` is not empty, list each file and pattern label it names (labels only, never
   the secret itself) and recommend the user review those notes before sharing the archive.
4. Remind the user that the archive contains their **personal notes**: keep it private and
   never commit it or share it publicly.
