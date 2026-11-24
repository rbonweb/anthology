---
name: 'Docs tests: plain rust image, and no "Python" in docs'
description: 'How to run docs and cutover_docs tests, the word rule, and how the README and docs pages are pinned'
metadata:
  type: project
  modified: 2026-11-24
---

Run the docs tests with scripts/rust-test.sh run cargo test --locked -p repo-tests (about 5 minutes with the volumes warm), or in a plain rust image with only the worktree mounted: docker run --rm -v "$PWD":/w -w /w <FROM line of ci/images/rust.Dockerfile> sh <script>, where the script runs rustup toolchain install, sets CARGO_TARGET_DIR=/target and runs cargo test --locked -p repo-tests --test docs --test cutover_docs.
**Why:** the docs tests fall back to a file walk without git, so mounting only the worktree works.
cutover_docs fails if a doc other than the allowed ones mentions Python. Write "0.1.x layout" style wording instead. The two allowed lines (python3 -c, python -c) are listed by file in ALLOWED_LINES, so moving that text to another doc means moving its entry.
cutover_docs reads git ls-files: a new doc must be git-added before the test sees it, or ALLOWED_LINES entries look unused.
README.md is a front page of at most 120 lines (test_readme_stays_a_short_front_page). Detail lives in docs/*.md, listed in DOC_PAGES in crates/repo-tests/tests/docs.rs; links resolve relative to the doc that holds them. The anchor check strips underscores from headings, which GitHub does not, so never link to a heading with an underscore in it.
cargo stops at the first failing test binary unless --no-fail-fast is given.
**How to apply:** any docs edit to CONTRIBUTING.md, README.md, SECURITY.md, PLAN.md or docs/*.md.
