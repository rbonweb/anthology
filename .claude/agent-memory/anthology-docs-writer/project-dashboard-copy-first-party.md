---
name: 'dashboard-copy is hand-written knowledge'
description: 'plugins/anthology/knowledge/dashboard-copy is first-party: edit by hand, keep under 12000 bytes, own wording, keep the attribution lines'
metadata:
  type: project
  modified: 2026-11-28
---

plugins/anthology/knowledge/dashboard-copy/SKILL.md is the first hand-written knowledge folder (copy and marketing plan, Task A3); every other knowledge folder is generated from vendor/sources.json. Why: a later edit must go to this file directly, not through vendor/, and must keep its rules: at most 12000 bytes, the fixed heading order, own wording only (Carbon and GOV.UK are paraphrased, NN/g is cited, never copied), and the two closing attribution lines (OGL v3 line linked to the National Archives licence page, then the Carbon Apache-2.0 line). Its LICENSE is a byte-identical copy of plugins/anthology/LICENSE. When checking source URLs: WebFetch gets HTTP 403 from nationalarchives.gov.uk, but curl with a browser User-Agent gets 200, so the page is live. Intl.DateTimeFormat formatRange returns a dash range, while the file asks for "to" ranges.
