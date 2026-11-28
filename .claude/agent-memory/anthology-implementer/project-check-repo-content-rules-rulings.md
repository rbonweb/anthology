---
name: 'check-repo content rules: where the code departs from the plan text'
description: 'Task 4 rules in anthology-scan: tightenings made to keep the self-scan clean, and what the orchestrator must call'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-scan/src/rules/{hidden,payload,exfil}.rs. The orchestrator (scan.rs) must call hidden::raise_homoglyph_twins(all paths, findings) once: check() alone gives only MEDIUM for hidden.homoglyph-name because it sees one file. Homoglyph test is per word (a letter run), so a Cyrillic name with a .md extension is not a mix. payload.decode-exec needs the decoder and the executor within 10 lines (plan said one file): plugins/anthology/knowledge/design-taste-frontend/scripts/consistency.mjs holds Buffer.from(x, base64) and child_process far apart and would be a false HIGH in the self-scan. eval, Function and exec count only as calls with no dot before them (regex.exec and tab.eval are not executors). payload.blob base64 needs a letter beyond a-f, else hex needs 2048. exfil rules run on Code files only; the dotted indicators are written with [.] in patterns and split with concat! in tests so the repo scans itself clean. Own byte checks (ELF, Mach-O, PE, archives) live in payload.rs because files::magic was a stub when written.
