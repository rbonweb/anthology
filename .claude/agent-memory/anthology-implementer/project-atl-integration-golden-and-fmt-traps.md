---
name: 'ATL integration: golden bless syntax and comment traps'
description: 'golden bless takes one comma list; rustfmt drops a block comment beside a mod name; the done-check excuses pinned names by list'
metadata:
  type: project
  modified: 2026-11-22
---

anthology-dev golden bless|check takes exactly one --surface with a comma list (--surface a,b,c); repeating --surface exits 2 with "give exactly one of --surface or --all".
rustfmt moves a trailing line comment after mod name { onto the next line and silently drops a block comment placed between mod and the name or before the brace, so a module name the port ledger pins cannot carry the legacy word; crates/repo-tests/tests/atl_rename.rs lists such lines in PINNED_LINES and pinned file names in PINNED_PATHS, and fails when an entry is no longer used. The old-name regex is a plain case-insensitive substring, so a camel-case identifier built from "sort" and "key" matches too (upstream_watch.rs had one; it is now NumberKey); rename such identifiers.
Why: each trap cost a container round trip in the integration run.
How to apply: when a golden line or a pinned name must keep the old name, add it to those lists with the reason instead of widening the regex.
