// The anthology plugin's Claude Code mods: the one hooks module hooks.json names under
// `modules` (one entry per plugin; a second fails the whole hooks.json). Each feature is a file
// beside this one exporting a `register<Feature>(on)`, added here as one import and one line.
// Every hook carries `.catch(($, e, next) => next(e))`, so a failing mod leaves Claude Code's own
// verdict and the shell hooks in charge. `ANTHOLOGY_MODS=0` turns them all off (util.ts).
//
// `tool.check` loads from Claude Code 2.1.311, and `engines` (>=2.1.330) is not enforced there,
// so a build older than the one these mods are verified on still runs them: permissions.ts
// checks `$.session.version()` itself and keeps Claude Code's verdict below 2.1.330.
// The shell hooks in hooks.json run on every build, so nothing gets looser without the mods.
import type { Register } from 'claude-code'
import { registerPermissions } from './permissions.ts'

export const register: Register = on => {
  registerPermissions(on)
}
