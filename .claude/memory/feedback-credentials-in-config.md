---
name: 'credentials-in-config'
description: 'Credentials agents fetch or generate go to ~/.config/<project>/<service>.env (folder 700, file 600), never in the repo or output'
metadata:
  type: feedback
  modified: 2026-11-28
---

The owner, 2026-11-28: when automation needs credentials fetched from a browser or control panel, or generated, store them at a path like ~/.config/sinrix/cloudflare-email.env: a .config folder, then the project folder, then one .env file per service.

**Why:** secrets must survive across sessions for full automation without ever entering a repo, a commit or a transcript.

**How to apply:** <project> is the project's name in lowercase (the repo folder unless the project names itself otherwise); KEY=value lines; folder mode 700, file 600; reports name only the path and the keys. The rule lives in the agents' shared Shell section and the routing guide. Related: [[licensing-is-the-owners-job]].
