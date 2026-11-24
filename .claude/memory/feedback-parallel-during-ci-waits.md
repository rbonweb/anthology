---
name: parallel-during-ci-waits
description: "While PRs wait on CI or a release runs, the lead keeps agents busy on deferred follow-ups and release steps that don't depend on what is pending; never report \"nothing to start\" while deferred items exist"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-24T06:21:01.345Z
---

While open PRs wait on CI, or a release workflow runs, start the work that does not depend on them:
- deferred review findings and "next batch" follow-ups, as draft PRs for the next release;
- release steps with no dependency on the pending item (for example a plugin that ships no `anthology.lock` can be released before the binary is out);
- the next step's agent, briefed to wait for its input in the foreground (for example the lock PR agent waiting on the release workflow);
- research on open questions from the session.

**Why:** the owner had to remind the lead more than once. During long CI queues the lead reported "nothing startable" over and over while deferred items sat in the ledger.

**How to apply:** at every 10-minute check, read the ledger for "deferred", "follow-up" and "open question" lines, and start an agent for each one not owned by a live agent. "Nothing startable" is only true when that list is empty.
