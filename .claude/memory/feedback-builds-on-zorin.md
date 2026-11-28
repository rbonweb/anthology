---
name: 'Rust builds run on Zorin, not the Mac'
description: 'Agent Rust builds and tests go to ANTHOLOGY_BUILD_HOST=zorin scripts/remote-build.sh; locally only the one proving test, then CI'
metadata:
  type: feedback
  modified: 2026-11-28
---

Every agent brief that builds or tests Rust says: run it on the Zorin build host with `ANTHOLOGY_BUILD_HOST=zorin scripts/remote-build.sh <rust-test.sh args>`, never on the Mac. Locally an agent runs only the one test that proves its fix, then pushes and lets CI run the rest. It finishes with `scripts/remote-build.sh --clean-ours`.

Why: on 2026-11-28, six or seven agents built in Docker on the Mac at once. The load average reached about 32 and each slop fix took 45 to 60 minutes, mostly local builds. The owner asked "use this Zorin. Why are you building the stuff on Mac?" The host was approved on 2026-11-27, and the first build server is still off-limits: see [[feedback-no-remote-build-server]].

How to apply: put the Zorin line in every Rust brief. Zorin is shared with other projects' CI (load around 15 to 20 on 12 cores), so if it is saturated, say so rather than falling back to the Mac silently.
