---
name: 'difftest golden check in a session container needs umask 022'
description: 'docker exec sessions run with umask 000, so setup goldens differ on dir modes (755 vs 777) until you set umask 022'
metadata:
  type: reference
  modified: 2026-11-27
---

Running scripts/rust-test.sh run with an anthology-release-* session container, golden check --surface setup reported hundreds of diffs that were only dir 755 vs 777 in the w/.claude tree. Prefix the command with umask 022 and the same check shows only real diffs. Use golden check --all (there is no flag-free form). Why: it looked like a broken golden and cost a run. How to apply: run sh -c "umask 022 && cargo run --locked -q -p difftest -- golden check --all" before and after blessing.
