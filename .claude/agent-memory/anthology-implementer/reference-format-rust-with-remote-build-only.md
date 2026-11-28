---
name: 'Formatting Rust when only the remote host builds'
description: 'How to apply rustfmt output when cargo runs only on the build host and lint fails on fmt'
metadata:
  type: reference
  modified: 2026-11-28
---

lint (fmt --check, clippy -D warnings) runs fmt first, so a width-100 or import-order diff hides clippy errors until fmt is clean. To format without a local toolchain: ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh run rustfmt --edition 2024 --emit stdout <files> > a temp file outside the repo; each file is a header line, a blank line, then its content. Split it back with a throwaway script file (sed -n ranges), then remove the script before committing. Why: the host has no cargo, and sed -i or heredocs are refused in a worktree. Clippy in this toolchain wants let chains (collapsible_if) and merged range patterns (manual_range_patterns).
