---
name: 'claude plugin test kit traps for the anthology mods'
description: 'How the test kit answers $ calls, what a test cannot raise (approver, ceiling), why .catch is checked through validate, what tsc catches that check does not, and that a session.append hook beneath must call next'
metadata:
  type: project
  modified: 2026-11-28
---

In plugins/anthology/tests/mods, the test hooks sit beneath the plugin and stand for Claude Code.
- A hook on a $ op (process.run, session.cwd, settings.read, fs.stat, env.get) answers { value: X }, not a bare X.
- A $ call no test hook answers throws beneath, and a test hook that throws is skipped for the rest of that dispatch. A mod that catches failures then quietly keeps the verdict, so a guard test can pass for the wrong reason. Mutate each guard to prove it.
- A kit query carries no approver, and a hook above cannot add one (approver is pinned). So with the approver === person gate the mod never allows through the kit. The allow paths are tested by calling the exported checkBash with a stand-in $ and crafted e; the kit tests cover pass-through and the matcher.
- The test body $ and the test hooks $ cannot call session.*, settings.read or tool.check (the scan lists no calls for them). An inline plugin in the test cannot call helpers either: it is serialized into its own module.
- The test body $ has no state: $.state.get and $.state.set are undefined there. Show a state rule by behavior (two reports in a row, or a changed folder), with mock.clock to move time.
- The kit drops a test-written plugin or ceiling on a verdict. To raise plugin, load an inline append-tier plugin that asks.
- A failing hook without .catch is skipped and the chain continues beneath, which a verdict test cannot tell from .catch(next(e)). ci/jobs/mods.sh fails on validate printing gating hook without .catch; that is the kill for the .catch mutant.
- The kill switch (modsOff) is the first $ call, so an env.get recorder shows whether the mod saw a check; that is the kill for the { tool: Bash } matcher mutant.
- A test hook on session.append (or any chain event) that answers without calling next is skipped ("returned an answer without next"), so a recorder there must be (_$, e, next) => { seen.push(e); return next(e) }. The recorder then sees the row as the plugin rewrote it. $.session.append(input) in a test takes the full row (message, door, origin, uuid).
- Calling engine(on, ...) twice in one test stacks two sets of beneath hooks; write a test per variant instead of a loop inside one test.
- ci/jobs/mods.sh (check) runs the tests but does not type-check them. ci/jobs/mods.sh tsc does, and it caught a hook value that lacked isLink (an fs.list entry is a full FsEntry) and an env table typed with optional keys. Run both before the PR. A cold run can take 5 to 15 minutes (npm ci over the network): wait with timeout 170 docker wait on the container whose name starts anthology-mods-<worktree folder>. A long-lived dev container with npm ci done once runs plugin test in seconds (see the container-runs note).
Why: these cost two mutation rounds on P5, a probe round in the #164 security fix, a tsc round on the parallel work reminder and a red run on the secret masking mod (the recorder trap).
