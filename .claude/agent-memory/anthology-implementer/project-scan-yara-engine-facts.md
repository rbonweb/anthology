---
name: 'check-repo YARA engine: design facts and corpus numbers'
description: 'How anthology-scan yara works, why the vendored GuardDog rules compile unchanged, and the benign-corpus hit counts behind an empty DISABLED list'
metadata:
  type: project
  modified: 2026-11-28
---

Why: Task 5 of the check-repo plan; later tasks (T2 orchestrator, T11, T15) read these facts.
- crates/anthology-scan/src/yara/: parse.rs (parser, per-rule errors, resync at the next rule), atoms.rs (string to regex::bytes plus literal atoms), eval.rs (lazy per-file run, path_include/path_exclude, ecosystem), mod.rs (compile, FILES, guarddog(), UNSUPPORTED, DISABLED, tests).
- All 44 threat rules and the 4 helper .meta files compile, so UNSUPPORTED is empty. The 18 rules with severity high and specificity high (not threat_npm_*) are enabled; base64exec is high/medium and so is not in guarddog(); tests reach it through compile(FILES).
- compile() builds every rule; guarddog() builds only the enabled rules plus the rules they name, so startup stays small.
- Atom prefilter: Extractor prefix literals, else the longest literal on every path of the pattern; one AhoCorasick (ascii_case_insensitive, Standard match kind, find_overlapping_iter). wide strings and non-ASCII regexes (Unicode fallback) get no atom and always run.
- A hit is one per rule per file; line and seen come from the earliest match among the strings the condition looked at, cut at 200 bytes. Counts (#id) do not overlap.
- Path globs supported: *.ext, */name, name, and dir/* (exclude, any depth). setup.py rules use */setup.py,setup.py.
- Corpus (2026-11-28, 51,000 code files of 16 benign repos): 17 files hit, all MEDIUM-level, none in a file an install script runs: download_exec 9, filesystem_autostart 4, one each dynamic_loader, destruction, cryptomining. Release speed: 10 MB under 0.2 s, django 20 MB in 0.3 s.
- Tools: host rustfmt (rustfmt --edition 2024 FILE) formats a module before the Zorin lint, since remote-build fmt only reports.
