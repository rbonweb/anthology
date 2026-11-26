---
name: 'osascript notifications belong to Script Editor'
description: 'Why notify.sh no longer uses osascript on macOS, and how to check a notification sender'
metadata:
  type: reference
  modified: 2026-11-26
---

On macOS a notification posted through osascript is attributed to com.apple.ScriptEditor2 (usernoted logs: Connection com.apple.ScriptEditor2 with path /usr/bin/osascript), so clicking it opens Script Editor. Wrapping the call in tell application id "<host bundle id>" does not change this: usernoted still logs ScriptEditor2. Check with: /usr/bin/log show --last 40s --info --debug --predicate 'process == "usernoted"' (call /usr/bin/log, zsh has a log builtin). Why it matters: do not reintroduce osascript or the tell-host idea in plugins/anthology/hooks/notify.sh; a real fix for a banner needs a bundle-owning poster (terminal OSC 9/777 escape sequences is an untested idea). Tests: crates/repo-tests/tests/notify.rs.
