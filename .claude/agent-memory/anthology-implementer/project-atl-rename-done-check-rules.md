---
name: 'The ATL rename check: what an old-name line needs'
description: 'Every remaining old-name line in code needs the word legacy on the same line; rustfmt moves trailing comments after a function brace'
metadata:
  type: project
  modified: 2026-11-22
---

crates/repo-tests/tests/atl_rename.rs (ignored until the release integration) fails on any case-insensitive old-name match outside the allowed paths unless the same line holds one of: the upstream names, the five old release tags, "fork of", test_ ids of the old name, or the word legacy (case-insensitive, so LEGACY_ constants count).
rustfmt moves a trailing // comment after a function opening brace onto its own line, so a function whose name carries the old name needs the word in the signature: fn name(/* legacy */) {. A trailing comment after a string argument inside a macro call stays on its line.
state::write only merges keys; dropping a state.json key needs state::remove_keys (added in T4, anthology-core).
Doc, skill and README lines that describe the old install must say legacy (or use the old release tag form) on the same line; reflow a paragraph so each line that names the old name also says it.
Why: the check is line based, and the first run of a mechanical rename leaves many lines that read fine to a person but fail it.
How to apply: grep -n -i the old name in each file you touch and read every hit for the word legacy before you commit.
