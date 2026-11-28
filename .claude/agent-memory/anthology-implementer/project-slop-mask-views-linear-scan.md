---
name: 'Slop mask views: one linear scan, shared helpers in css.rs'
description: 'HTML, JSX and CSS views must stay linear in input size; their shared helpers live in mask/css.rs'
metadata:
  type: project
  modified: 2026-11-28
---

The slop hook blocks when it hits its 60 s timeout (onFailure block), so a mask view that is quadratic on one long line or on many unclosed openers turns an odd file into a blocked edit. Why: regex finds that match to the end of the line, or a close tag searched once per opener, rescan the haystack per match. Rules kept in crates/anthology-slop/src/mask: comments are found by one byte scan (comment_spans in css.rs, not a regex), and an HTML raw element (script, style, pre, code) with no closing tag is searched once and remembered. Tests comment_scanning_stays_linear_on_long_lines and unclosed_raw_elements_are_searched_once pin this with a 5 s bound; the second fails slowly (about 90 s at 30000 openers) if the memo is removed. Shared helpers for the html, jsx and css views (masked_code, comment_spans, keep, span_of and the test support module) are in crates/anthology-slop/src/mask/css.rs because the task could touch only those three files.
