---
name: 'Mods that use $.state need a plugin.json types contract'
description: 'claude plugin validate refuses $.state keys not declared in the manifest types file; claude plugin test needs full compose input'
metadata:
  type: project
  modified: 2026-11-22
---

The anthology plugin declares its $.state keys in plugins/anthology/types/anthology.d.ts, named by "types": "./types/anthology.d.ts" in plugin.json (the path must start with ./). A declare module block inside a hooks file is not enough: validate says the key is not declared. Several mods will add keys to the same PluginState.anthology object, so merge them there, never declare the anthology property twice.
In plugin tests, $.prompt.compose needs model, promptModel and outputStyle in its input or the hook is skipped silently (the section just does not appear), so a negative test can pass for the wrong reason. Always pair it with a positive test using the same input.
Install of the pinned Claude Code in a container takes 2 to 5 minutes on a busy machine: keep one long-lived container (docker run -d with a sleep) and docker exec the validate, test and tsc steps; remove it at the end.
Why: avoids three repeated npm ci waits and a validate refusal.
