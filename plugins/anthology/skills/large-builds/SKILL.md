---
name: large-builds
description: Rules for long or large multi-agent builds (many tasks, several agents at once, runs measured in hours) — start work early against published interfaces, keep agents busy with a ~10-minute check for startable work, tiered testing (CI is the PR gate), one review per task, batch merges, rulings instead of waiting, never route around a refusal, plain-language status reports. Use with anthology-superpowers:subagent-driven-development or dispatching-parallel-agents whenever a build spans many tasks or hours; where they differ, this wins for that run. Also the rules for every session: how to code, how to report status, and rulings while the user is away.
---

# Large Builds

Read this before the first task of a large or long build (many tasks, several agents working at once, or a run measured in hours). It sits on top of anthology-superpowers:subagent-driven-development and anthology-superpowers:dispatching-parallel-agents; where it differs from them, this file wins for that run. Each rule is followed by why.

## Every session, not only large builds

- Writing, reviewing or refactoring code: follow karpathy-guidelines (think
  before coding, simplicity first, surgical changes, goal-driven
  verification). The full text is
  `${CLAUDE_PLUGIN_ROOT}/knowledge/karpathy-guidelines/SKILL.md`; the
  implementer, debugger and reviewer read it at startup. Why: most wasted
  agent work is unasked-for scope and unverified claims.
- "Decide; don't wait" and "Status reports for humans" below apply to every
  session. Why: a parked question or an unreadable report costs the same in
  a short session as in a long one.
- Never state behaviour, an API parameter or a cause from memory: grep the
  code, read the current docs or measure, else say "unverified". Why: wrong
  claims from memory cost rework in both large projects.
- "Done" names its evidence: an image is done when viewed, a page when
  rendered on each surface, a check when it failed on a known-bad case, a new
  runner after one canary job. Why: 52 presets were called done after about 12
  were viewed.
- Search for secrets with `grep -c` or `-l` first, and print a match only
  masked. A brief that hands over a secret says once who reads it and where
  it goes. Why: one agent printed full vendor API keys into a log.

## The lead and the user

- A merge conflict goes to an implementer, with the branches and the
  conflicting files named; it merges on its own branch from the start, and the
  lead merges that branch. The lead does not resolve it, in any file (docs
  and memory notes included). Why: resolving a conflict is code work, and the
  lead's context is for decisions; a conflict agent could not write main.
- Answer questions; act only on asks. A setting nobody asked for, or one that
  injects behaviour, is listed for approval, never applied. Why: an unasked
  setting and an over-scoped brief both changed what the owner had not
  agreed to.
- An interim notice that an agent is "waiting on its own background work"
  needs no reply and no action. Why: the agent is still working, and a
  reply only adds noise.
- The lead never blocks on a run of more than a minute or two. Start it in
  the background, or hand it to an agent, and read the result when it exits.
  Why: a blocked lead cannot answer the user, merge or start the next task.
- A plan holds no task for the lead that writes code, tests or docs; such a
  task goes to the matching agent. Why: a lead-coded task is where a plan
  quietly turns the lead into a worker.
- A user's stop or pause beats the plan, the recurring check and any
  scheduled start. Start nothing new until the user says go. Why: a task
  started seconds after "do not pick up new tasks" breaks the user's trust.
  To stop, TaskStop every agent (the waiting ones too) and every background
  job, list them again, and say "stopped" only when none remain. Why: a
  waiting agent is still running, so "all stopped" was false.
- Before a restart, tell every running agent to commit its work in progress,
  and wait for those commits. Why: uncommitted worktree changes are lost.
- Brief an agent with the plan path and the task ID, and let it read the task
  itself; paste no plan text. Why: copying plan sections costs the lead a
  read per task and drifts from the plan.
- A writer reports at push, with the PR number and head SHA; it does not
  wait for CI. The lead's one background watcher reads CI for every open PR,
  including integration branches, and sends each red job back to its writer
  with SendMessage. Why: a writer that waits for green CI sits in
  wait-stop-wake cycles, and PRs nobody watched sat red until the owner
  noticed.
- Never park ready work. Start it, or say what blocks it. Why: a parked task
  idles the agents that could run it.
- Run `scripts/release.sh` in the background, and never pipe it into `tail`.
  Why: a pipe hides the script's exit status and its output until it ends.
- After a guardrails release, restart the lead session. Why: a session keeps
  the plugin version it loaded, so a stale guard keeps running.
- Brief with repo-relative paths. Send review packages to the agent's
  worktree or to `/tmp`, not into the lead's checkout. Why: an absolute path
  into another checkout points a worktree agent at the wrong tree, and
  access to it can be refused.
- When the session starts outside a repo, keep the shell in the target repo
  for every command. Why: setup and the repo's own rules apply only to a
  shell inside the repo.
- Append to ledgers with Edit, never a shell redirect. Why: a redirect
  append can be refused in a worktree, and Edit shows exactly what changed.

## Waiting and hangs

- Agents: never a long `sleep` (it is blocked). Run the slow command in the
  background and read its output when it exits, or watch a condition with
  Monitor. Why: sleeping wastes the turn and trips the block.
- Load a deferred tool such as Monitor with ToolSearch
  (`select:Monitor`) before the first call. Why: calling a tool whose schema
  is not loaded fails.
- Put a timeout on every test run, and start the run in the background
  (a foreground Bash call is capped at 600 s): `timeout 900 <command>`
  inside the container command (Linux images have `timeout`; the macOS host
  has none), or the Bash tool's own `timeout` parameter. Never rely on a
  host `timeout`. When it fires, or the run shows no progress, stop and
  report a hang instead of waiting. Why: a hung run once sat for 36
  minutes unnoticed.

## Docker names per worktree

- Tag every image and name every container per worktree, for example
  `myapp-test-<worktree folder name>`; read the folder name from `pwd` and
  type it literally. Remove only your own names afterwards. Why: when every
  worktree tags the same name, parallel agents test each other's code.
- No host installs (`brew`, `npm -g`, global `pip`, browser downloads); a
  project `npm install` is fine. Why: host installs change the owner's
  machine, while a project install stays in the project folder.
- Give every cleanup rule a performance check (cache hits, build time before
  and after). Why: one cleanup rule threw away the build cache and builds took
  18-47 minutes.

## Memory stays small

- Every agent that saves notes (the scout excepted) saves only lasting facts (a rule, a surprise, where
  something lives, why a decision was made), never per-task logs or what a
  diff already shows. Why: the index loads in every session, and hundreds
  of log-style notes made it 17-20 KB.
- Run `anthology-mem search <topic>` before saving; update a matching note
  with `--replace` instead of adding a duplicate. Why: duplicates bury the
  one true note.

## Long runs

- Set a recurring ~10-minute check (CronCreate) at the start of a long run, and delete it (CronDelete) when the run ends.

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
  clobber each other. This is the exception to
  anthology-superpowers:subagent-driven-development's "never in parallel":
  one working tree still gets one writer.
- Nested subagents are fine on the same terms: disjoint slice, own worktree,
  the parent merges. Why: a big task often splits cleanly by module.
- In a shared registry (a module list, an owners table), append your own
  line only; never reorder or edit other lines. Why: append-only edits merge
  without conflicts.
- Put in each dispatch: the plan path and task ID (or the task-brief file),
  which is the brief itself; the slice as one line, not plan text; the
  interfaces to code against; the SWAP rule; the test tier below; and "no
  reviewers". Why: a worker cannot follow rules it was never given, and
  pasted plan text costs the lead a read per task.
- At dispatch, split per-unit work (pages, themes, capture sets, CI jobs)
  into one agent per unit or pair. Cap the count only on a measured
  shared-resource limit, stated once. Why: the owner asked for more parallel
  agents 10 times across three projects.
- Split by file ownership at dispatch, and reassign a slice only after its
  owner commits. Why: a slice reassigned mid-flight strands the first owner's
  uncommitted changes.

## Keep capacity busy

- On a long run, every ~10 minutes, look for work that could start now:
  swaps for dependencies that just merged, plumbing, docs, early reviews of
  finished tasks, release prep. Start it. Why: agents idling on a test queue
  are the costliest waste in a long run.
- Use each bounded wait (anthology-superpowers:subagent-driven-development,
  "Waiting on dispatched subagents") for this check. Why: the wait is already a check-in, so the scan costs nothing.
- Never park an agent on a shared queue (a build server, a slow job) while
  other work exists. Why: queue time is wall-clock time the whole build pays.
- Treat machine or server load as temporary. If you throttle (fewer jobs,
  avoid a server, serialize work) because something is busy, re-check after
  5–10 minutes and lift the limit once the load is gone; never let a
  momentary limit become a standing rule. Why: a 5-minute spike turned into
  a permanent throttle costs hours of build time.
- After any container work (tests, builds), remove the containers, the
  images you built, and the volumes they used once you no longer need them.
  Touch only your own: other projects share the same Docker. Why: leftover
  build volumes fill the disk fast (hundreds of GB in one day).

## Test at the cheapest level that catches the bug, once

- While coding: only the tests for the part you changed (the test file,
  module or package), plus its formatter and linter. Never run the full
  suite during development, not even on a fast server. Run the full suite
  once, at the final go for the feature, which is usually CI on the PR. Why:
  repeated full runs take far longer than they're worth and hold shared
  capacity.
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
- On red CI, read every failed job of the run, fix them in one push and
  cancel superseded queued runs. Before switching runner or image, diff it
  against the hosted runner and dry-run once. Why: one failure per push made
  serial fix cycles, and runner gaps surfaced one per run.

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
  costs most. This replaces the fix-loop's rounds for the run.
- Docs-only, test-only and mechanical changes (renames, stand-in swaps,
  regenerated files) get no review; CI covers them. Why: a reviewer adds
  nothing a passing build has not already shown.

## Design and UI work

- Before the first UI task, collect the logo and colours, light or dark,
  fonts, one to three sites the owner likes, the focus style and the public
  identity (name, contact channels, which legal names may appear). Ask once
  for any identity or legal fact shown publicly; never default one. Owner
  away: record a Ruling per gap and ask about them first on return. Why:
  direction that came after the build reworked the same files about four
  times.
- A clone or like-X brief starts with the reference capture (BrowserOS, 1440
  and 390). Why: a checkout built from memory was rebuilt.
- Keep a banned-names list (owner, legal entity, upstream brands) in project
  memory, and put it in every UI brief and review. Why: forks and stores
  leaked upstream and owner names.
- A UI task's one review is ui-reviewer on the rendered newest build, before
  merge. When the owner wants less review, shrink it to the measured pass,
  never skip it; only an explicit owner "skip review" skips it. Why: in three
  projects UI merged unreviewed, and the owner found the faults.
- A review or QA brief names the build or URL. Why: one audit round ran on
  the older live site.
- Check the first capture by eye before running the batch, and take product
  screenshots only from merged main or a deployed build. Why: about 20 shots
  were unusable and had to be retaken.

## Merge in batches

- Merge green branches together, then run one integration test on the batch.
  Why: one run per batch instead of one per branch.
- If the batch run fails, find the branch that broke it, take it out of the
  batch and merge the rest. Why: one bad branch should not hold up the
  green ones.
- Merge, never rebase, a branch that already has a PR. Why: a rebase
  rewrites the commits the reviewer saw.
- When an integration branch merges part PRs, close those part PRs with a
  comment that points to the integration PR. Why: only PRs that will really
  merge should stay open, or the PR list shows red PRs nobody will fix.

## Decide; don't wait

- When the human is away, decide and record
  `Ruling: <what> — <why> — <cost if wrong>` in the ledger and the report.
  Why: a wrong ruling costs visible, undoable rework; a parked question
  costs the whole day.
- Still stop for what anthology-superpowers:subagent-driven-development lists: irreversible, security-sensitive and
  outward-facing actions (outside the worktree: a merge, a push to a shared
  branch, a publish). Why: those cannot be undone by rework.

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
- Report only on a merge, a finished or failed task, a decision, or the
  10-minute check. Send one block, and never repeat a line that has not
  changed. When only CI is pending, say so once. Why: repeated status lines
  bury the news.
- Give a time estimate only from durations you measured (timestamps you
  read); otherwise write "unknown". Say what it depends on. Why: a guessed
  estimate that slips costs more trust than an honest "unknown".
- Report a start only after the dispatch call returns. Why: a start announced
  before the dispatch is false when the call fails.

```text
Done: <features that now work>.
Running: <feature>: <what is happening>, about <time or unknown>.
Blocked: <feature>: needs <who, what>.
Remaining: about <time or unknown>, if <assumption>.
```
