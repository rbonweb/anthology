---
name: 'Release prep: lock commands and the map budget'
description: 'How to bump the workspace locks with only our crates changing, and that the project map budget is not test-enforced'
metadata:
  type: project
  modified: 2026-11-27
---

Root lock: scripts/rust-test.sh run cargo update --workspace --offline. Fuzz lock: the same with --manifest-path fuzz/Cargo.toml (a separate workspace). Each rewrites only our own crates versions; third-party 0.2.1 crates such as wait-timeout and windows-link stay. Why: the bump must not touch third-party entries. After an Edit that removes the end of DECISIONS.md, check the file still ends with a newline.
The project map budget is 7000 (measure with wc -c, bytes, which is the stricter count). No test or script measures it: roster.rs only checks the template text says Size budget: 7000 characters. So check by hand with wc -c after every map edit. The map was 7618 bytes on build once a new fact landed; getting it under needed merging duplicated duty lines, dropping test names and extra function names, and removing the originSessionId frontmatter line. Every added line needs a cut: drop test-enforced duties and long job lists first.
