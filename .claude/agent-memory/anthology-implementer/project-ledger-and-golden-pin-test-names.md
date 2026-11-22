---
name: 'Ledger and difftest names pin the old rtk spellings'
description: 'Renaming rtk to atl cannot rename ported test files, module and fn names, or difftest surface names; keep them with a legacy marker'
metadata:
  type: project
  modified: 2026-11-22
---

port/ledger/*.tsv pin Rust test file paths, module names and fn names, and port/difftest/divergences/<surface>.toml plus the golden file name pin the difftest surface name (dev.rtk-trim). A global rename breaks ci ledger check and golden check.
**Why:** port/** is final and must not change, so the code has to keep those names.
**How to apply:** after a rename, restore the pinned names (check with the ledger listing: cargo test --workspace -- --list, then anthology-dev ledger check --report --listings DIR --platform linux) and put a legacy comment on each kept line.
