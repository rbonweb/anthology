---
name: 'Zorin lint: rustfmt diffs are not applied back, so write to the 60-column chain width'
description: 'lint on Zorin prints fmt diffs and clippy errors one stage at a time; nothing syncs back, so apply diffs by hand'
metadata:
  type: reference
  modified: 2026-11-28
---

scripts/remote-build.sh lint runs cargo fmt --check first, then clippy only when fmt is clean, so one broken stage hides the next (about 4 to 8 minutes a round). The remote copy is never synced back, so rustfmt diffs must be applied by hand. Why it matters: rustfmt here breaks any method chain longer than 60 columns and spreads arrays whose items exceed 10 characters one per line, so code written in the 100-column style fails fmt. Tips: redirect the run to a file under /tmp (a pipe into tail keeps only the last 80 lines and can drop the first diffs); keep long word lists as one string split with split_whitespace; clippy collapsible_if (edition 2024 let chains) and redundant_closure fire in test code too. To pass several test filters use: test -p CRATE -- filter_a filter_b (cargo itself accepts one).
