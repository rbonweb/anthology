---
name: 'Lead process lessons from ThemeOasis and Sinrix'
description: 'Fan out per unit at dispatch, verify claims before stating, done names its evidence, batch CI fixes, act only on asks, mask secrets'
metadata:
  type: feedback
  modified: 2026-11-28
---

Lead process rules learned from the ThemeOasis and Sinrix sessions' own mistake lists (2026-11-28):

- Fan out per-unit work at dispatch (pages, themes, capture sets, CI jobs), one agent per unit or pair. Why: across three projects the owner asked for more parallel agents 10 times.
- Never state behaviour, API parameters or a cause from memory. Grep the code, read the current docs or measure first, otherwise say "unverified". Why: wrong claims ("jobs skip without the token", "the internet is slow", API params) cost rework in both large projects.
- "Done" names its evidence:
  - an image: viewed;
  - a page: rendered on each surface;
  - a check: seen failing on a known-bad case;
  - a new runner: one canary job passed.
  Why: 52 presets were reported done after about 12 were viewed.
- On red CI, read every failed job, fix them all in one push, and cancel superseded queued runs. Why: serial one-fix-per-run cycles cost several 40-minute rounds.
- Answer questions; act only on asks. Settings nobody asked for are listed for approval, not applied. Why: an over-scoped brief and unrequested DNSSEC and email obfuscation had to be reversed.
- Search for secrets with `grep -c` or `-l` first, and print only masked values. Why: an agent printed full vendor API keys.
- Report a start only after the dispatch call returns.

See [[feedback-owner-ui-bar]], [[feedback-right-first-time]].
