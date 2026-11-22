---
name: 'Reading a difftest golden report'
description: 'Where the golden check report lands and how to read its hook cases inside the rust container'
metadata:
  type: reference
  modified: 2026-11-22
---

difftest golden check --report PATH writes JSON, not text: each failure has "id", "case", "python" and "rust". With the cargo target in a Docker volume the report is only visible inside the container, so grep it there. guard.hook cases carry the hook input as "stdin_hex"; the rust:slim image has no xxd, so decode with perl: perl -ne 'chomp; s/(..)/chr(hex($1))/ge; print'. Why: reviewing every changed verdict before a bless is required (see the G0 bless note), and the hex hides the command.
