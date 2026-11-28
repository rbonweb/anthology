---
name: 'check-repo report: which JSON fields are wrapped, and how run on open is counted'
description: 'Wrapped fields, marker defanging, and the why-phrase the run on open count depends on'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-scan/src/report.rs wrap() puts a nonce marker pair around finding path, why, seen and do, skipped entries, failed or skipped tool detail, error notes, about and instruction paths. target, url, rev and quarantine stay bare: crates/anthology-cli/tests/check_repo.rs compares them to the raw values. Marker-shaped text (any case) inside a wrapped body gets a backslash between the two angle brackets.
The run on open header line counts findings whose why holds the Open trigger text from scan.rs trigger_text. Scan has no surface list, so that phrase is the link; the test run_on_open_counts_what_runs_when_the_folder_opens scans a real tree and breaks if the phrase changes.
rules::autorun many_msbuild_commands_get_their_lines_in_one_pass is a timing test (3 s in a debug build); it failed when two remote runs shared the host, and passed alone.
