---
name: 'Ledger and difftest names pin the legacy tool spellings'
description: 'Renaming the token layer cannot rename ported test files, module and fn names, or difftest surface names; the ledger checks fn names equal the Python method names'
metadata:
  type: project
  modified: 2026-11-22
---

port/ledger/*.tsv pin Rust test file paths, module names and fn names, and port/difftest/divergences/<surface>.toml plus the golden file name pin the difftest surface name (the legacy dev.rtk-trim). anthology-dev ledger (CI jobs ledger and rust-test, test the_committed_ledger_is_structurally_sound) also requires each fn name to equal the Python method name, so editing a ledger row to a new fn name is not enough: the fn must keep the Python name.
**Why:** port/** is final, and a mechanical rename of fn names in the ATL rename turned CI red on 43 ledger rows.
**How to apply:** never rename a test fn or module that a ledger row names; crates/repo-tests/tests/atl_rename.rs takes those names out of each line itself (ledger_pins, a port ledger pin), so they need no legacy marker. Check with ci/jobs/ledger.sh steps (cargo test --workspace -- --list, plain, --all-features and --ignored, then anthology-dev ledger check --report --listings DIR --platform linux) before pushing a rename.
