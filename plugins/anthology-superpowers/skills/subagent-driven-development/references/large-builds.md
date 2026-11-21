# Large Builds

Part of anthology-superpowers:subagent-driven-development. Read this before
Task 1 when a plan has many tasks, several agents working at once, or a run
measured in hours. Where it differs from SKILL.md or
[fix-loop.md](fix-loop.md), this file wins for that run. Each rule is
followed by why.

## Start early

- Start every task whose inputs are published interfaces, not merged code.
  Why: waiting for merges serialises the whole plan behind its slowest task.
- Code against the signatures the spec or plan publishes, exactly. Why: the
  real code lands with those signatures, so the later swap is mechanical.
- When a body you need does not exist yet, write a minimal private stand-in
  in your own files, mark it `// SWAP(<task>)` in the file's comment syntax,
  and list it in your report. Why: a greppable marker turns the swap into a
  search instead of an investigation.
- When the dependency merges, merge it into your branch, replace every
  `SWAP(<task>)` stand-in and delete the markers. Why: a leftover stand-in is
  untested code that looks real.
- Never edit another task's files to unblock yourself; stand in for them.
  Why: two writers on one file is a conflict and a review nobody owns.

## Run in parallel without collisions

- Give each parallel implementer a disjoint file slice and its own worktree
  or sub-branch, and merge the results yourself. Why: separate trees cannot
  clobber each other. This is the exception to SKILL.md's "never in
  parallel": one working tree still gets one writer.
- Nested subagents are fine on the same terms: disjoint slice, own worktree,
  the parent merges. Why: a big task often splits cleanly by module.
- In a shared registry (a module list, an owners table), append your own
  line only; never reorder or edit other lines. Why: append-only edits merge
  without conflicts.
- Put in each dispatch: the slice, the interfaces to code against, the SWAP
  rule, the test tier below, and "no reviewers". Why: a worker cannot follow
  rules it was never given.

## Keep capacity busy

- On a long run, every ~10 minutes, look for work that could start now:
  swaps for dependencies that just merged, plumbing, docs, early reviews of
  finished tasks, release prep. Start it. Why: agents idling on a test queue
  are the costliest waste in a long run.
- Use each bounded wait (SKILL.md, "Waiting on dispatched subagents") for
  this check. Why: the wait is already a check-in, so the scan costs nothing.
- Never park an agent on a shared queue (a build server, a slow job) while
  other work exists. Why: queue time is wall-clock time the whole build pays.

## Test at the cheapest level that catches the bug, once

- While coding: the tests of the module or package you changed, plus its
  formatter and linter. Why: fast feedback, right where the bug is.
- Before opening or updating a PR: the same, nothing more. CI is the PR
  gate; never repeat its full matrix locally. Why: a duplicate run holds
  capacity and finds nothing CI would miss.
- After a batch merge: one integration run on the merged result. Why:
  integration bugs exist only once branches meet.
- At release: the full run of everything, once. Why: that is the one point
  where every platform and suite must agree.
- A known flaky test gets one re-run, not an investigation; a second failure
  is real. Why: chasing a known flake burns hours and changes nothing.
- Slow or secondary platforms do not block a merge; fix their failures
  forward. Why: one slow runner should not hold every branch hostage.

## Write timing tests that survive shared runners

- To assert linear time, bound the growth ratio, not the absolute time: run
  sizes n and 4n and require t(4n) < 10 × t(n). Why: linear growth gives
  about 4x and quadratic about 16x, so 10x separates them on any machine.
- Give the baseline a floor: compare against max(t(n), a few tens of
  milliseconds), or repeat the small run until it reaches that. Why: a tiny
  baseline jitters on a shared CI runner and makes any ratio meaningless.

## Review budget

- One independent review per task. Why: a second review mostly re-finds the
  first one's points.
- Fix every Critical and Important finding. List the Minors in the ledger
  and the report, with no fix loop. Why: Minors rarely pay for another round.
- No re-review after the fixes, except after a Critical finding in security
  code (authentication, permission or command guards, sandboxing, secrets,
  privacy): there, one scoped re-review. Why: that is where an unverified fix
  costs most. This replaces fix-loop.md's rounds for the run.
- Docs-only, test-only and mechanical changes (renames, stand-in swaps,
  regenerated files) get no review; CI covers them. Why: a reviewer adds
  nothing a passing build has not already shown.

## Merge in batches

- Merge green branches together, then run one integration test on the batch.
  Why: one run per batch instead of one per branch.
- If the batch run fails, find the branch that broke it, take it out of the
  batch and merge the rest. Why: one bad branch should not hold up the
  green ones.
- Merge, never rebase, a branch that already has a PR. Why: a rebase
  rewrites the commits the reviewer saw.

## Decide; don't wait

- When the human is away, decide and record
  `Ruling: <what> — <why> — <cost if wrong>` in the ledger and the report.
  Why: a wrong ruling costs visible, undoable rework; a parked question
  costs the whole day.
- Still stop for what SKILL.md lists: irreversible, security-sensitive, and
  outside-the-worktree actions. Why: those cannot be undone by rework.

## Permissions

- When a safety check, hook or permission prompt refuses one agent's action,
  no other agent may do it on that agent's behalf. Why: routing around a
  refusal defeats the check and hides it from the human.
- Never edit permission settings, hooks or agent configuration to get past
  a refusal. Why: the same reason.
- Take it to the human with the one exact command that fixes it and what
  that command changes. Example: git keeps using a stale saved credential;
  the fix is `gh auth setup-git`, which makes git use the GitHub CLI's
  login. Why: one command is a quick yes or no; a vague report stalls.

## Status reports for humans

- Write for a non-technical owner, in feature terms ("search now works
  offline"), not task ids, branches or file names. Why: the owner decides
  on features, not on plumbing.
- Lead with what is done, then what is running, then what is blocked and
  who must act. Why: done is what the owner can use or check today.
- Give an honest time estimate for the rest, and say what it depends on.
  Why: a hopeful estimate that slips costs more trust than a long one.

```text
Done: <features that now work>.
Running: <feature>: <what is happening>, about <time>.
Blocked: <feature>: needs <who, what>.
Remaining: about <time>, if <assumption>.
```
