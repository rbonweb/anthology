---
name: 'US English guard test: scope, allow list and traps'
description: 'crates/repo-tests/tests/us_english.rs fails on UK spelling in first-party text; what it skips, how to allow a contract string, and the traps met in the sweep'
metadata:
  type: project
  modified: 2026-11-28
---

crates/repo-tests/tests/us_english.rs scans tracked text files (a walk fallback runs where git is absent, as on a remote Zorin run) and flags UK word forms per identifier part (snake and camel case split). Excluded: vendor, generated knowledge, superpowers plugin, atl except the fork files listed in ATL_OWN, the scanner crate, .gz goldens, port/ ids and ledger, guard test data. Inherited test_* names that port/python-test-ids.txt holds keep their spelling. A UK word that is a name another tool reads goes in the ALLOWED table (file, lower-case word, reason); a row nothing uses fails the test.
Traps: (1) the difftest op name for the sanitize helpers sits inside recorded golden cases with its UK spelling, so only the Rust calls were renamed. (2) golden bless adds a header today equal to the legacy day 2026-11-22 and drops D-1 marks of the cases it re-records; that is expected. (3) GitHub status function for a canceled run and the Claude Code hook outcome value both keep the UK double-l spelling: they are names another tool reads. (4) A shared line in all agent prompts needs a budget rise for the agents at 0-4 spare bytes (debugger, implementer, scout), and ui-designer at 60 body lines takes the sentence at the end of an existing rule. (5) The atl_rename test needs git and the session_start skills test lagged the check-repo skill on a remote run: judge them in CI.
Why: the owner wants US English everywhere and every agent writing it; the test keeps it from coming back.
