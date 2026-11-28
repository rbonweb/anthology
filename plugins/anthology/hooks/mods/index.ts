// The anthology plugin's Claude Code mods: the one hooks module hooks.json names under
// `modules` (one entry per plugin; a second fails the whole hooks.json). Each feature is a file
// beside this one exporting a `register<Feature>(on)` for the events only it hooks.
//
// An event that two features hook is registered here, once. Claude Code throws on a second
// registration of one event, which unloads the whole module. So a shared event's hook calls `next`
// once, then hands `e` (and what `next` resolved to, where the part changes it) to each feature's
// exported part, each in its own `isolated` try/catch: one feature's failure never drops another's
// result, and the hook's `.catch` (which calls `next` again) is left for a failure of this file only.
//
// Every hook carries `.catch(($, e, next) => next(e))`, so a failing mod leaves Claude Code's own
// verdict and the shell hooks in charge. `ANTHOLOGY_MODS=0` turns them all off (util.ts).
//
// Claude Code before 2.1.330 refuses a module that hooks an event it lacks, silently and whole;
// the shell hooks in hooks.json still run there, so nothing gets looser without the mods.
// `tool.check` is the exception: it loads from 2.1.311, and `engines` (>=2.1.330) is not enforced,
// so permissions.ts checks `$.session.version()` itself and keeps Claude Code's verdict below
// 2.1.330.
//
// `tool.check` is hooked by the permissions feature alone, so it is registered in
// `registerPermissions` like any single-owner event; only `agent.report` is shared (below).
// Likewise `session.append` belongs to the masking feature alone (`registerMasking`).
import type { Register } from 'claude-code'
import { afterworkReport } from './afterwork.ts'
import { alertsReport, registerAlerts } from './alerts.tsx'
import { registerDelegation } from './delegation.ts'
import { registerMasking } from './masking.ts'
import { nudgeReport } from './nudge.ts'
import { registerPermissions } from './permissions.ts'
import { isolated } from './util.ts'

export const register: Register = on => {
  registerPermissions(on)
  registerAlerts(on)
  registerDelegation(on)
  registerMasking(on)

  // Shared: the after-work check and the parallel work reminder (each changes the text, in that
  // order) and the status pane (tracks the run).
  on('agent.report', async ($, e, next) => {
    const core = await next(e)
    let report = await isolated(() => afterworkReport($, e, core), core)
    report = await isolated(() => nudgeReport($, e, report), report)
    await isolated(() => alertsReport($, e), undefined)
    return report
  }).catch(($, e, next) => next(e))
}
