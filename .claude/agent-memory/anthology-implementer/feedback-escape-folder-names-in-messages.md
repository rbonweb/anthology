---
name: 'Escape folder and file names before printing them'
description: 'Names read from disk are untrusted text; quote, escape, cut and keep non-UTF-8 out of copyable commands'
metadata:
  type: feedback
  modified: 2026-11-22
---

Any directory or file name printed in an error or hint is attacker-controlled (extracted archive, cloned tree) and stderr is read by the model. Quote it, write control characters, quotes and backslashes with escape_default, cut it at about 60 characters, and show a copy-and-run command only when the name is valid UTF-8, unchanged and does not start with a dash.
Why: review of the repos-below hint in anthology-mem found raw names could break the message or inject terminal escapes. The scan helper in anthology-core returns OsString so callers can tell a lossy name from a real one.
How to apply: whenever a message lists names from read_dir. Test with a newline plus ESC name, a non-UTF-8 name (skip when the file system refuses it, macOS does), a 200-character name and a name holding a quote and comma.
