---
name: 'check-repo scan speed: profile by hand and keep slow patterns off every file'
description: 'A pattern with no literal start costs a full pass; profile_a_folder shows where a scan spends time'
metadata:
  type: project
  modified: 2026-11-29
---

A regex that starts with a class or has no required literal (the environment-dump pattern in exfil.rs) is a slow pass over the whole text, so a rule must gate it with a plain substring search or run it only when its answer decides the level. FW8 added two such passes per script and a scan of vscode (22k files) went from 1.6 s to 7 to 11 s; gating them (may_dump_env, lazy secret reads) put it back to 1.5 s on an idle host. To profile: run the ignored test scan::tests::profile_a_folder in release with SCAN_PERF_ROOT set to a folder (it prints each orchestrator phase and each rule module summed over threads); on the build host use a docker run that mounts one scratch root holding both a copy of the source and the checkout, and remove the scratch root and the target volume afterward. The guard against a repeat is the test that counts full environment scans (a_file_that_names_no_environment_costs_no_environment_scan). Why: a time bound on a synthetic tree is flaky across debug and release builds, so the guard counts the expensive pass instead.
