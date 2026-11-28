---
name: 'Slop UI rules: pattern traps the plan text has'
description: 'Where the plan regexes for the UI rules over-match, the guards the code adds, the purple-gradient stop rule, and the round-2 var(), star-row, import and generator rules; read before editing rules/ui.rs'
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

Purple gradient, after the pre-release review (item 6): the plan said "any violet colour in the call", which fired on viridis legends, hue sliders and Discord blurple. Now `is_purple_stock` fires only for 2 or 3 stops with a violet among them: three stops must all be violet family (h 228..=330), two stops need the other end chromatic blue through pink (h 190..=345). Stops are the top-level comma items minus the leading angle/shape/position and bare-percentage hints. A violet next to white, transparent or green no longer fires; this overrides the plan Task 5 test that `var(--brand), white` fires (review ruling). Class-based Tailwind stops are unchanged. Tests that checked var resolution with a `white` partner must use a blue partner or they pass for the wrong reason.
Linear scan: `gradient_calls` pairs `(` with `)` on one stack pass and clears it at `;`, `{`, `}`; a call over 4096 bytes is dropped so nested calls stay cheap. The plan per-call search to the end of the text took minutes on 1 MiB of unclosed `linear-gradient(`. Both perf tests (unclosed, nested) never finish if the code regresses, so run them under `timeout` and kill the leftover test process in the session container.

Round 2 (review-2 I1, I5, M6):
- var() resolution is the `Vars` struct: each (name, depth) resolves once and is kept, so work does not grow with uses. A name keeps 4 distinct values (`MAX_VALUES`; without it the dedupe check alone is quadratic: 80 000 declarations take 39 s in a debug build), one resolved value is capped at 64 KiB, and a file may build 4 MiB in all (`MAX_EXPANDED`); past a cap the call is let through. A test value padded with spaces must put them between two colours: `--name:` trims the value, so leading spaces vanish.
- five-star-row: a `.map` over an array of five is skipped when the star, or what sits between the array and the star, has an attribute set from an expression other than `key` and a literal (`filled={i < v}`, `className={cn(..)}`). `size={16}` and `filled` stay a hard-coded row. Only the array forms are read this way; five written-out `<Star />` still fire.
- effect-library: the tag name is regex group 1; a name the file imports from a source that does not contain magicui, magic-ui or aceternity is skipped. A name that is not imported at all still fires.
- generator-signature: a name match is dropped when `view.text` is not masked at its start. The meta-tag alternative (group 1) always fires. Quoted names and bare URLs in prose are masked in `text`, so they still fire: a limit of reading `text`. Tests need `crate::mask::view(kind, doc)`; `View::plain` makes everything text.
- Run the repo scan with `git ls-files -z -- '*.md' '*.html' '*.tsx' '*.jsx' '*.css' ... | xargs -0 anthology slop check` (about 850 files): the UI rules had no hit on tracked files before or after.
