---
name: 'Owner design feedback audit (2026-11-28)'
description: 'Kinds of design, UI and copy faults the owner found in Anthology-built UI in other projects, root causes and counts'
metadata:
  type: project
  modified: 2026-11-28
---

Audit of three owner projects built with plugin 0.1.0 to 0.5.5 (before the 2026-11-28 anti-slop work): 29 owner-found problems, 0 fully fixed on build 28f45a5b, 3 partly (IM4 rubric, gradient-text gate, drawn-icon house rule).
Main root causes, for comparing later releases:
- No ui-review finished before the owner reviewed, in any of the three projects. The lead read an owner request for less review as no review; UI merged before ui-reviewer; one audit targeted an older live build.
- The design-taste-frontend gate and consistency.mjs (orphans, cut text, section gaps, hash links) only check a static file or built folder, so they never ran on SSR apps. ui-reviewer does not load that skill.
- Design direction (theme mode, logo, fonts, references, focus style, public identity) came after the build; files were reworked about four times.
- A like-X brief never loads website-clone, so the reference was not captured first.
- Missing rendered checks: headings over 2 lines, clipped descenders, orphan rows, dead chevrons, label-to-destination mismatches, layout shell changes per route, hand-drawn third-party marks, focus rings on mouse click.
- Missing identity guard: upstream brand leftovers after a fork, invented legal entity and jurisdiction lines, placeholder people taken from the user context, dummy phone and chat widgets copied from a clone source.
- Use cases were derived from the codebase, with no navigation map before UI work.
Build conflict found: website-clone suggests bracketed brand placeholders that the ui.placeholder-text slop rule blocks, and it sends out-of-scope links to a hash that consistency.mjs fails.
Why: the next audit can check which of these later releases fixed, instead of re-mining transcripts.
