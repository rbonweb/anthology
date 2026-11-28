---
name: 'check-repo content rules: where the code departs from the plan text'
description: 'Task 4 rules in anthology-scan: tightenings for the self-scan and corpus, the reached-file helpers and what the orchestrator must call'
metadata:
  type: project
  modified: 2026-11-29
---

crates/anthology-scan/src/rules/{hidden,payload,exfil}.rs. Orchestrator hooks: hidden::raise_homoglyph_twins(paths, findings); exfil::raise_reached(findings, reached) lifts MEDIUM exfil findings whose why carries RAISE_NOTE (theft shape capped by a test path, any MEDIUM chat or ngrok webhook) to HIGH; a comment, a docker bind or mount spec or a dump with no send near is never lifted. payload::raise_reached(f, out) and hidden::raise_reached(f, out) take the FileCtx of a reached file and push the findings the generated-folder cap (blob, decode-exec HIGH) or the test-path cap (bidi HIGH) held back; scan.rs merges one finding per rule and path, higher severity wins. Homoglyph test is per word. payload.decode-exec: within 10 lines HIGH, farther apart in one file MEDIUM; eval, Function and exec count only as calls with no dot before them. exfil.cred-path is HIGH only for a path in code, not a bind spec, read by a real read call within 3 lines (readFile, readSync, open, cat, cp, Get-Content, File.read, os.ReadFile, type in .bat) or by a redirection, with a send within 10 lines; test, spec and fixture paths cap it at MEDIUM. in_comment lexes the line (strings, regex literals, block comments) so a // inside a literal does not hide code. env_dumps covers spread, Object.assign, entries, os.environ copy and printenv piped to a sender. exfil rules run on Code files only; dotted indicators are written with [.] in patterns and split with concat! in tests so the repo scans itself clean.
