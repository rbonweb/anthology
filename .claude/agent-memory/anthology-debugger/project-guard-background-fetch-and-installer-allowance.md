---
name: 'Guard binary download runs detached; the installer is the third allow site'
description: 'How the guardrails shim gets its binary on slow links, and how to test it without hanging CI'
metadata:
  type: project
  modified: 2026-11-24
---

The guardrails guard and session modes (ANTH_BG in plugins/*/hooks/lib.sh) take the archive from a detached bootstrap.sh fetch in <ah>/bin/.fetch-<version>/ (run/ lock with pid, part, done, fail). Why: one hook call has 20 s; a slower download used to restart from zero every minute and never finish, and the block pointed at /anthology:setup, which needs Bash. guard.sh now has three exit 0 sites; the third is anth_fix_call, the exact installer while the state is missing (statics.rs counts the sites). How to apply: in shim tests use a curl stub on PATH (crates/repo-tests/tests/shims/slow.rs), call stop_fetcher at the end of any test that starts a download, since setsid fetchers escape the harness group kill. CI containers have no init that reaps, so a killed fetcher is a zombie that still answers kill -0; anth_fetch_alive reads /proc/<pid>/stat for that. The local Docker run can skip apt: rust:slim already has dash, bash and setsid.
