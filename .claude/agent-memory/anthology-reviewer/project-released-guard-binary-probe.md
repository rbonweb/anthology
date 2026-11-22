---
name: 'Probe a released guard binary without building'
description: 'Download the anthology-bin release asset in alpine, check its sha256 against a plugin anthology.lock, then pipe hook JSON to guard hook'
metadata:
  type: project
  modified: 2026-11-22
---

To learn what a shipped guardrails version really does, run the binary its plugins/anthology-guardrails/anthology.lock pins instead of building: in alpine:3.20 (busybox wget does https) fetch https://github.com/rbonweb/anthology/releases/download/<tag>/anthology-<triple>.tar.gz, compare sha256sum with the lock entry, extract, and pipe {"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"..."}} into anthology guard hook (exit 2 plus a Blocked by line is a block, exit 0 with no output is allow). On an Apple silicon host the triple is aarch64-unknown-linux-musl. Good block probes are G1 forms such as npx playwright install wrapped in a runner.
Why: it took seconds, while a cold cargo build of the guard takes minutes, and it proves the exact artifact users get.
How to apply: use it whenever a change depends on a guard feature that only a new anthology-bin release carries (PR 174: guard 0.1.1 does not peel atl runners, so atl run -c and atl bypass hide G1 forms until the lock moves to 0.2.0).
