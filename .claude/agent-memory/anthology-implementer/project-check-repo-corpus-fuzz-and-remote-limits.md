---
name: 'check-repo corpus job, fuzz target and what Zorin cannot run'
description: 'How the network corpus job is built and tested, why cargo fuzz cannot build on the Zorin image, and the false alarms the first real run found'
metadata:
  type: project
  modified: 2026-11-28
---

ci/jobs/check-repo-corpus.sh has two forms: no argument builds the release binary through scripts/rust-test.sh run, and --run BIN [TSV] scans the corpus (the seam repo-tests drives with a fake binary; node parses the JSON). ci/check-repo-corpus.tsv pins 12 repos by full SHA from git ls-remote; the scan runs inside the rust image, which has git and node.
The Zorin rust image has no C++ compiler, so cargo fuzz build fails in libfuzzer-sys (cc: c++ not found); the GitHub runner has one. A fuzz/Cargo.lock edit can still be checked there with cargo metadata --locked (needs the rustup nightly the fuzz folder pins). remote-build.sh run never copies files back, so write lock files by hand and verify with --locked. A throwaway mutation test (tests/ file with a xorshift mutator over seed files, deleted before commit) ran 68,873 inputs through jsonc, yara and a one-file scan with no panic.
First real run (2026-11-28, scanner with Tasks 1-10): 10 of 12 repos clean; vscode gave 24 HIGH and claude-code 4 HIGH, all false alarms (cred-path on .npmrc or .ssh names in comments and tests, env-dump on a compared process.env, vscode-hidden matching .vscode-test, whitespace-push inside a terminal recording string, ai-cli-abuse on a string that checks for the flag, bidi characters in test files). Why: the plan promises zero HIGH on the corpus, so those rules need narrowing before the corpus job can pass.
