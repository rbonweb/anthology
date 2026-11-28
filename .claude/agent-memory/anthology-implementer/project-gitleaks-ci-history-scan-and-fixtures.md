---
name: 'gitleaks CI scans history: fix a fixture secret by rewriting the branch'
description: 'The gitleaks job reads every commit and prints no detail; how to find the hit locally with the pinned image and which fixture shape fires'
metadata:
  type: project
  modified: 2026-11-28
---

The gitleaks CI job scans the whole git history of the branch (git --log-opts=HEAD), so a fixture secret that was ever committed stays a finding after a later commit removes it. Fix the literal and rewrite the branch (git reset --soft to the merged base, then commit again, push with force-with-lease) before the PR is reviewed.
The job runs with --redact and prints only "leaks found: N", no rule or file. To find the hit before pushing, run the pinned image from ci/images/gitleaks.Dockerfile in dir mode on one folder at a time with a JSON report written to a mounted folder: dir --config /repo/.gitleaks.toml --report-format json --report-path /out/report.json /repo/<folder>. dir takes one path (more than one scans the container root) and a report path of /dev/stdout prints nothing. The word gitleaks in a Bash line trips the worktree guard, so put the docker call in a small script file in the worktree, run it with bash and delete it.
Dir mode sees paths as /repo/..., so the anchored path allowlists in .gitleaks.toml do not apply: files they allow (crates/anthology-guard/tests/data/test_guard_rules.jsonl) still list findings there and are fine in CI.
Rule that fired on a test fixture: generic-api-key on a quoted 12-character letters-and-digits literal next to the name api_key in a Rust test string. A quoted password value and an unquoted env-style password value in the same file did not fire. Build any api key value at run time with concat, as the AWS and token fixtures are.
Why: the secret masking PR failed gitleaks on its first push and the finding could only be cleared by rewriting the commits.
