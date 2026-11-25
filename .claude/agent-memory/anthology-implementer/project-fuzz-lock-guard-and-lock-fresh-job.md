---
name: 'cargo-fuzz has no --locked; the fuzz lock guard and the lock-fresh job'
description: 'fuzz.sh guards fuzz/Cargo.lock with cargo metadata --locked; lock-fresh.sh is informational and has a dev.sh test seam'
metadata:
  type: project
  modified: 2026-11-24
---

cargo-fuzz 0.13 has no --locked option, so ci/jobs/fuzz.sh runs cargo metadata --locked in fuzz/ before any cargo fuzz command; a stale fuzz/Cargo.lock fails there. repo-tests ci.rs fuzz_build_is_locked pins that order. Why: a plan item asked for --locked on cargo fuzz build, which cargo-fuzz rejects.
ci/jobs/lock-fresh.sh runs scripts/dev.sh lock check-fresh, always exits 0, and reports through a ::warning annotation and GITHUB_STEP_SUMMARY. ANTHOLOGY_DEV_SH replaces dev.sh so repo-tests can run it with a fake. The job in ci.yml has continue-on-error and no permissions block. The tool output is printed indented so no line can start a workflow command. ci/jobs/shellcheck.sh lists its scripts by hand, so a new ci/jobs script is not shellchecked until added there.
