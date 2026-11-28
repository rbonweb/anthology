---
name: 'What a BrowserOS run script can measure, for judging UI review rules'
description: 'run sandbox has no document; layout metrics need page.evaluateInPage; width via page.cdp; 60 s cap, about 5 fresh pages per call'
metadata:
  type: reference
  modified: 2026-11-28
---

The run tool sandbox has no window or document. getClientRects, scrollHeight and getComputedStyle work only inside page.evaluateInPage, which the tool text allows only when snapshot, read and grep cannot answer, so a prompt that wants measurements should say to use it. There is no viewport API: width changes need raw CDP through page.cdp (Emulation.setDeviceMetricsOverride). page.click is native input, so :focus-visible heuristics apply (text inputs match it on mouse click, buttons do not). One run is capped at 60 s and the tool advises about 5 fresh navigations per call, so "one run per width on every route" does not fit a large app. Page actions take snapshot refs, never CSS selectors.
Why: PR 322 (design bar in ui-reviewer) needed this to judge whether its measured pass was runnable; the tool descriptions are the source.
