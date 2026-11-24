---
name: 'release.sh: where its behaviour is pinned, and changelog fragments'
description: 'Three test files pin scripts/release.sh output; fragments are read by release-edit, not by the script'
metadata:
  type: project
  modified: 2026-11-24
---

scripts/release.sh behaviour is pinned in two places: crates/anthology-dev/tests/release_sh.rs (fake dev and fake claude, temp repo) and the release_script_tests module in crates/anthology-dev/tests/release_edit.rs (real release-edit, ported Python tests with their own fake claude). Changing the script output or push order means editing both. Why: the second set looks like plain release-edit tests and is easy to miss.
Changelog fragments (changelog.d/<plugin>/*.md) are read and deleted by the release-edit changelog-check and changelog-move commands, with no new CLI argument, so the frozen difftest goldens and the argparse usage text stay unchanged. The script only adds the fragment folder to the release commit pathspec. Why: a new subcommand or option would change the pinned usage and invalid-choice messages.
scripts/release.sh is bash and is checked by ci/jobs/shellcheck.sh as bash in a second run; the rest of that job is sh. Run the dev tests with scripts/rust-test.sh run cargo test -p anthology-dev --locked --test <file>; the test mode runs the whole workspace.
