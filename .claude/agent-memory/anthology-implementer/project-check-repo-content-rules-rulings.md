---
name: 'check-repo content rules: where the code departs from the plan text'
description: 'Task 4 rules in anthology-scan: tightenings for the self-scan and corpus, and what the orchestrator must call'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-scan/src/rules/{hidden,payload,exfil}.rs. The orchestrator (scan.rs) must call hidden::raise_homoglyph_twins(all paths, findings) once (check() alone gives MEDIUM) and exfil::raise_reached_webhooks(findings, reached-path test) once (a chat webhook is MEDIUM unless a send is within 10 lines AND the file reads env or a token; reached files are raised to HIGH). Homoglyph test is per word. payload.decode-exec: decoder and executor within 10 lines is HIGH, farther apart in one file is MEDIUM; eval, Function and exec count only as calls with no dot before them. payload.blob base64 needs a letter beyond a-f, else hex needs 2048. content-mismatch reads bytes lossily and needs at most 5 percent invalid or control bytes, so a stray byte or NUL cannot hide a program but real font data is skipped. Corpus narrowings (vscode, claude-code): cred-path HIGH only for a path in code (not a comment) with a read call within 3 lines and a send within 10; env-dump HIGH only with a send or file write within 10 lines; test, spec and fixture paths cap cred-path, env-dump and chat webhooks at MEDIUM, bidi in Code under a test folder is MEDIUM (Agent stays HIGH); whitespace-push ignores a blank run inside a closed string; ai-cli-abuse needs the CLI name within 300 bytes before the flag and the flag not compared with. exfil rules run on Code files only; dotted indicators are written with [.] in patterns and split with concat! in tests so the repo scans itself clean.
