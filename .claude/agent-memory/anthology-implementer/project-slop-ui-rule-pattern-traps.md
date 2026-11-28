---
name: 'Slop UI rules: pattern traps the plan text has'
description: 'Where the plan regexes for the UI rules over-match, and the guards the code adds; read before editing rules/ui.rs'
metadata:
  type: project
  modified: 2026-11-28
---

The plan patterns for the nine UI rules (crates/anthology-slop/src/rules/ui.rs) are the spec, but five need a guard or they flag the wrong thing. Each guard has a test that goes red without it.
1. `color\s*:\s*transparent` also matches `background-color: transparent`. A leading `(?:^|[^-\w])` stops it.
2. `filter\s*:\s*blur(` also matches `backdrop-filter`, which is glass, not an orb. Same leading guard, with `-webkit-filter` allowed.
3. Tailwind `\bblur-3xl` also matches `backdrop-blur-3xl` (a word boundary sits after the dash). Use `(?:^|[^\w-])blur-`.
4. `\b-z-10` never matches after a space (no word boundary between a space and a dash). Write `(?:^|[^\w-])-z-10\b`.
5. The rule-body regex `([^{}]+)\{([^{}]*)\}` misses `style={{ ... }}` (nothing precedes the inner brace) and a bare `style="..."` value (no braces). The code takes every innermost `{...}` pair, and a style span with no braces counts as one body.
The gradient call span is found by balanced parentheses, not by the plan greedy `[^;{}]*`, so one call in a list of backgrounds gets its own span and an old violet call next to a new clean one does not count as new text. Why: the engine keeps a block hit when its span touches new text.
