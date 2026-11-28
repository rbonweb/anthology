---
name: 'claude plugin test kit traps for the anthology mods'
description: 'How the test kit answers $ calls, what a test cannot raise (approver, ceiling), why .catch is checked through validate, and what tsc catches that check does not'
metadata:
  type: project
  modified: 2026-11-28
---

In plugins/anthology/tests/mods, the test hooks sit beneath the plugin and stand for Claude Code.
- A hook on a $ op (process.run, session.cwd, settings.read, fs.stat, env.get) answers { value: X }, not a bare X.
- A $ call no test hook answers throws beneath, and a test hook that throws is skipped for the rest of that dispatch. A mod that catches failures then quietly keeps the verdict, so a guard test can pass for the wrong reason. Mutate each guard to prove it.
- A kit query carries no approver, and a hook above cannot add one (approver is pinned). So with the approver === person gate the mod never allows through the kit. The allow paths are tested by calling the exported checkBash with a stand-in $ and crafted e; the kit tests cover pass-through and the matcher.
- The test body $ and the test hooks $ cannot call session.*, settings.read or tool.check (the scan lists no calls for them). An inline plugin in the test cannot call helpers either: it is serialized into its own module.
- The test body $ has no state: $.state.get and $.state.set are undefined there. Show a state rule by behaviour (two reports in a row, or a changed folder), with mock.clock to move time.
- The kit drops a test-written plugin or ceiling on a verdict. To raise plugin, load an inline append-tier plugin that asks.
- A failing hook without .catch is skipped and the chain continues beneath, which a verdict test cannot tell from .catch(next(e)). ci/jobs/mods.sh fails on validate printing gating hook without .catch; that is the kill for the .catch mutant.
- The kill switch (modsOff) is the first $ call, so an env.get recorder shows whether the mod saw a check; that is the kill for the { tool: Bash } matcher mutant.
- ci/jobs/mods.sh (check) runs the tests but does not type-check them. ci/jobs/mods.sh tsc does, and it caught a hook value that lacked isLink (an fs.list entry is a full FsEntry). Run both before the PR. Each run takes 5 to 10 minutes: wait with timeout 170 docker wait on the container whose name starts anthology-mods-<worktree folder>.
Why: these cost two mutation rounds on P5, a probe round in the #164 security fix and a tsc round on the parallel work reminder.
