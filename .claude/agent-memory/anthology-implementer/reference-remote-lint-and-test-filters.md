---
name: 'remote-build.sh test filters and lint order'
description: 'cargo test takes one filter: pass several after --; lint runs fmt before clippy and only reports diffs'
metadata:
  type: reference
  modified: 2026-11-28
---

rust-test.sh puts the arguments in front of cargo test, so several filters need a double dash: scripts/remote-build.sh test -p anthology-scan -- tree:: files:: scan::. A bare second filter fails with "unexpected argument". The lint mode runs cargo fmt --check first and stops at the first diff, so clippy output only appears once fmt is clean. The run folder on the host is deleted, so fmt diffs are applied by hand with Edit from the diff text. Why: Task 2 of check-repo hit all three and each retry waits for a free build slot.
