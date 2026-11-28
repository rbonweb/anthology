---
name: 'Vendoring a multi-skill upstream: NOTICE, link placeholders, patch-from-tree'
description: 'Upstream NOTICE needs license_extra on every entry; link checker flags bare url placeholders; vendor diff makes the patch from the edited tree'
metadata:
  type: project
  modified: 2026-11-28
---

Vendoring several skills from one upstream repo (claude-blog, 2026-11-28) hit three traps:
- The vendor tool refuses an entry when the upstream repo root holds a NOTICE: add "license_extra": ["NOTICE"] to every entry of that repo (key order: after license_file). The NOTICE ships unchanged into each folder, so a "no banned word" scan must skip LICENSE and NOTICE files.
- repo_tests::links::dangling_links does not understand inline code spans, so a doc example such as [name](url), (/blog/x) or (filename.md) counts as dangling. Patch those to the angle form [name](<url>): it stays valid Markdown and the checker skips any target holding < or {. Make the test collect all folders before asserting, or you fix one folder per run.
- Recipe that works: add the entry without the patch transform, scripts/dev.sh vendor sync <name> --ref <tag>, edit the synced folder with Edit, scripts/dev.sh vendor diff <name> > vendor/patches/<name>.patch, add the {"patch": ...} transform, vendor build <name>, vendor check. After any later edit of a target folder, regenerate the patch from the edited tree the same way; diff always compares against the pristine upstream plus the earlier transforms.
Why: each of these cost a full re-run of sync or the test; the plan tables name line numbers at the pin but not these.
