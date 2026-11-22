// Permission levels (post-release mods design §2): a Bash command that `anthology perm classify`
// proves read-only runs without a prompt; everything else is decided as without the mod.
//
// Ordering: `tool.check` fires after `tool.call` and the PreToolUse hooks (guard.sh, rtk-hook,
// no-attribution.sh) and before the mode settles an ask. `next(e)` resolves the verdict beneath
// this hook: core's rules, mode and the tool's own check, the PreToolUse decision, and any plugin
// hooks below. Plugins above (an organization's `prepend` tier) still have the last word.
//
// The mod reads that verdict first and changes only an unmarked `ask`: one with no `rule` (a
// user's allow, ask or deny rule), `setting` (the outside-directory read block), `hook` (a
// PreToolUse decision, such as the guard's), `plugin` (another mod's ask) or `ceiling` (the
// organization's). A deny is never touched. The command classified is `e.input.command`, the
// text that will run (spike S2: tool.check sees rtk's rewrite). Any failure keeps the verdict.
import type { EngineInterface, On, ToolCheckInput, ToolCheckResult } from 'claude-code'
import { modsOff, permissionSettings, runAnthology, sessionRoots } from './util.ts'

const TOKEN = 'anthology-perm:read-only:v1\n'
const MAX_COMMAND = 4096
const TIMEOUT_MS = 3000

/** The command to classify, or undefined when the mod must leave the verdict as it is. */
function judgeable(e: ToolCheckInput, v: ToolCheckResult): string | undefined {
  if (v.decision !== 'ask') return undefined
  if (v.rule !== undefined || v.setting !== undefined || v.hook !== undefined) return undefined
  if (v.plugin !== undefined || v.ceiling !== undefined) return undefined
  if (e.ceiling !== undefined || e.isCeilingUnread !== undefined || e.op !== undefined) return undefined
  if (typeof e.input !== 'object' || e.input === null) return undefined
  const input = e.input as Record<string, unknown>
  if (input.dangerouslyDisableSandbox !== undefined && input.dangerouslyDisableSandbox !== false) return undefined
  const command = input.command
  if (typeof command !== 'string' || command.trim() === '') return undefined
  if (command.length > MAX_COMMAND || /[\n\r]/.test(command)) return undefined
  return command
}

/** Read and Bash deny and ask rules, as written, for the classifier's path check. */
function readRules(rules: readonly string[]): string[] {
  return rules.filter(r => /^(Read|Bash)(\(|$)/.test(r))
}

/** True only when the classifier answered exactly its token; every failure is false. */
async function proven($: EngineInterface, command: string): Promise<boolean> {
  try {
    const p = await permissionSettings($)
    const stdin = JSON.stringify({
      command,
      cwd: await $.session.cwd(),
      roots: await sessionRoots($, p),
      read_rules: readRules([...p.deny, ...p.ask]),
    })
    const r = await runAnthology($, ['perm', 'classify'], stdin, TIMEOUT_MS)
    return r.exitCode === 0 && !r.isStdoutTruncated && r.stdout === TOKEN
  } catch {
    return false
  }
}

export function registerPermissions(on: On): void {
  on('tool.check', { tool: 'Bash' }, async ($, e, next) => {
    const v = await next(e)
    const command = judgeable(e, v)
    if (command === undefined || (await modsOff($))) return v
    return (await proven($, command)) ? { decision: 'allow', reason: 'anthology: read-only command' } : v
  }).catch(($, e, next) => next(e))
}
