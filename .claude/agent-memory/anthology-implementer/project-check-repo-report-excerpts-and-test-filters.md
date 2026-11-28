---
name: 'check-repo report: how excerpts are read, and running several test filters remotely'
description: 'report.rs reads triage excerpts from Meta.target or the quarantine dest; remote-build test takes extra filters only after --'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-scan/src/report.rs render_json has no DiskTree, so it reads each triage excerpt itself: root is Meta.target for a folder scan (the CLI must pass the canonical folder) and the quarantine dest for a URL scan. read_inside walks each path part with symlink_metadata and refuses a link, a dot-dot, an absolute path or a non-regular file; the test excerpts_never_follow_a_symlink goes red if that call becomes fs::metadata. Why: the scan frozen interface gives the report only Scan and Meta, and the lead may not open the target files.
Every repo-derived string in the report goes through field (escape, mask, newline and tab written out, cut) so a line break cannot forge a finding block. escape::mask extends a keyword hit (password, Bearer, Basic) to the value after it and a private-key hit to the END marker; the spec said only "to the next whitespace or quote", which left values visible.
Test filters: cargo test takes one filter, so run several as ANTHOLOGY_BUILD_HOST=zorin scripts/remote-build.sh test -p anthology-scan -- escape:: report:: (extra filters after the double dash). A local rustfmt exists; rustfmt --edition 2024 <files> formats a file without a build, and the remote lint fails on any fmt diff.
