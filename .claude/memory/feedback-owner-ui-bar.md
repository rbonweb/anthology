---
name: 'Owner''s UI bar: review before merge, measured polish, every surface'
description: 'UI review before merge on the newest build (less review = measured pass, never none); headings ≤2 lines, no orphans, no clipped descenders, no dead chevrons, official marks, every surface'
metadata:
  type: feedback
  modified: 2026-11-28
---

The owner's UI quality bar, gathered from ThemeOasis, Sinrix and Rakazo (audit 2026-11-28, `.superpowers/sdd/2026-11-28-design-feedback/findings.md`).

- UI review runs before merge, on the newest build, and the brief names that build or URL. "Less review" from the owner means a fast measured ui-reviewer pass, never none.
  Why: in all three projects UI merged unreviewed or was reviewed on an older build, and the owner found about ten visible faults in one sitting.
- Polish bar the owner checks by eye:
  - headings at most 2 lines;
  - no orphan item alone on a wrapped row;
  - no clipped descenders (clip-text with tight line-height);
  - even section spacing;
  - no dead chevrons or arrows;
  - no focus ring on mouse click (`:focus-visible` only, border or underline change);
  - official third-party marks (payment, store, social), never hand-drawn.
- Every surface and entry route a change touches gets rendered (web widths, mobile app, deep links, signed-out, empty and disabled states). A screen shows the true server state: a closed sign-up says it is closed.
- Placeholder people are neutral invented names, never from the owner's context. Never default public identity or legal facts (entity, jurisdiction, phone, chat channel): ask once, and keep a banned-names list in project memory.
- Positive: specific, price-anchored headlines ("Stop paying $400 for a Shopify theme") work for this owner.

How to apply: put these into ui-designer and ui-reviewer briefs, and see [[feedback-design-direction-first]].
