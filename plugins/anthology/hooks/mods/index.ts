// The anthology plugin's Claude Code mods: the one hooks module hooks.json names under
// `modules` (one entry per plugin; a second fails the whole hooks.json). Each feature is a file
// beside this one exporting a `register<Feature>(on)`, added here as one import and one line.
// Every hook carries `.catch(($, e, next) => next(e))`, so a failing mod leaves Claude Code's own
// verdict and the shell hooks in charge. `ANTHOLOGY_MODS=0` turns them all off (util.ts).
//
// Claude Code before 2.1.329 refuses a module that hooks an event it lacks, silently and whole;
// the shell hooks in hooks.json still run there, so nothing gets looser without the mods.
import type { Register } from 'claude-code'
import { registerPermissions } from './permissions.ts'

export const register: Register = on => {
  registerPermissions(on)
}
