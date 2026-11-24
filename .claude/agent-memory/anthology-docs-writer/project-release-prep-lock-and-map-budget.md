---
name: 'Release prep: lock commands and the map budget'
description: 'How to bump the workspace locks with only our crates changing, and that the project map has little headroom'
metadata:
  type: project
  modified: 2026-11-24
---

Root lock: scripts/rust-test.sh run cargo update --workspace --offline. Fuzz lock: the same with --manifest-path fuzz/Cargo.toml (a separate workspace). Each rewrites only our own crates versions; third-party 0.2.1 crates such as wait-timeout and windows-link stay. The project map is capped at 7000 characters (wc -m) and sat at 6983 before the 0.2.2 release prep, so every added line needs a cut: drop test-enforced duties and long job lists first. Why: the bump must not touch third-party entries, and the map budget forces trims. After an Edit that removes the end of DECISIONS.md, check the file still ends with a newline.
