// Permission levels (post-release mods design §2): a Bash command that `anthology perm classify`
// proves read-only runs without a prompt; everything else is decided as without the mod.
//
// Ordering: `tool.check` fires after `tool.call` and the PreToolUse hooks (guard.sh, atl-hook,
// no-attribution.sh) and before the mode settles an ask. `next(e)` resolves the verdict beneath
// this hook: core's rules, mode and the tool's own check, the PreToolUse decision, and any plugin
// hooks below. Plugins above (an organization's `prepend` tier) still have the last word.
//
// The mod reads that verdict first and changes only an unmarked `ask`: one with no `rule` (a
// user's allow, ask or deny rule), `setting` (the outside-directory read block), `hook` (a
// PreToolUse decision, such as the guard's), `plugin` (another mod's ask) or `ceiling` (the
// organization's). A deny is never touched. The command classified is `e.input.command`, the
// text that will run (spike S2: tool.check sees atl's rewrite). Any failure keeps the verdict.
//
// It acts only where a person answers the ask (`e.approver === 'person'`), so hooks, SDK hosts,
// auto mode's judge and the denies of dontAsk, headless and background sessions stay exactly as
// Claude Code decides them. It also stands aside on a build before 2.1.330 and where the
// organization allows only managed permission rules. A proof counts only when Claude Code itself
// allows a Read of every path the classifier printed, so the session's own Read rules, working
// directories and modes decide what may be read.
import type { EngineInterface, On, ToolCheckInput, ToolCheckResult } from 'claude-code'
import { modsOff, permissionSettings, runAnthology, sessionRoots } from './util.ts'

const TOKEN = 'anthology-perm:read-only:v1'
const MAX_COMMAND = 4096
const MAX_PATHS = 64
const TIMEOUT_MS = 3000

/** The command to classify, or undefined when the mod must leave the verdict as it is. */
export function judgeable(e: ToolCheckInput, v: ToolCheckResult): string | undefined {
  if (e.tool !== 'Bash' || e.approver !== 'person') return undefined
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

/**
 * True when `$.session.version()`'s `base` is 2.1.330 or later. `tool.check` loads from 2.1.311
 * and `engines` is not enforced there, so the mod checks the build it runs on itself.
 */
export function isSupportedBuild(base: string | undefined): boolean {
  const m = /^([0-9]+)\.([0-9]+)\.([0-9]+)(-|$)/.exec(base ?? '')
  if (m === null) return false
  const [major, minor, patch] = [Number(m[1]), Number(m[2]), Number(m[3])]
  return major !== 2 ? major > 2 : minor !== 1 ? minor > 1 : patch >= 330
}

/**
 * The paths the classifier printed after its token, one absolute path per line, or undefined for
 * anything else: no token, no path, a relative or blank line, an unterminated last line, or more
 * than MAX_PATHS paths.
 */
export function classifiedPaths(stdout: string): string[] | undefined {
  if (!stdout.endsWith('\n')) return undefined
  const [token, ...paths] = stdout.slice(0, -1).split('\n')
  if (token !== TOKEN || paths.length === 0 || paths.length > MAX_PATHS) return undefined
  return paths.every(p => p.startsWith('/') && !/[\r\0]/.test(p)) ? paths : undefined
}

/** Read and Bash deny and ask rules, as written, for the classifier's own early refusal. */
function readRules(rules: readonly string[]): string[] {
  return rules.filter(r => /^(Read|Bash)(\(|$)/.test(r))
}

/** The classifier's paths for a proven command; undefined on every failure. */
async function classify($: EngineInterface, command: string): Promise<string[] | undefined> {
  try {
    const p = await permissionSettings($)
    const stdin = JSON.stringify({
      command,
      cwd: await $.session.cwd(),
      roots: await sessionRoots($, p),
      read_rules: readRules([...p.deny, ...p.ask]),
    })
    const r = await runAnthology($, ['perm', 'classify'], stdin, TIMEOUT_MS)
    if (r === undefined || r.exitCode !== 0 || r.isStdoutTruncated) return undefined
    return classifiedPaths(r.stdout)
  } catch {
    return undefined
  }
}

/** True only when Claude Code's own decision allows a Read of every path. */
async function readsAllowed($: EngineInterface, paths: readonly string[]): Promise<boolean> {
  for (const file_path of paths) {
    const r = await $.tool.check({ tool: 'Read', input: { file_path } })
    if (r.decision !== 'allow') return false
  }
  return true
}

/** True when the organization's managed settings allow only managed permission rules. */
async function managedRulesOnly($: EngineInterface): Promise<boolean> {
  const policy = await $.settings.read({ source: 'policy' })
  const only = policy.allowManagedPermissionRulesOnly
  return only !== undefined && only !== false
}

/** The Bash branch of `tool.check`. A missing `$` op throws, and the hook's `.catch` keeps `v`. */
export async function checkBash(
  $: EngineInterface,
  e: ToolCheckInput,
  next: (e: ToolCheckInput) => Promise<ToolCheckResult>,
): Promise<ToolCheckResult> {
  if (await modsOff($)) return next(e)
  const v = await next(e)
  const command = judgeable(e, v)
  if (command === undefined) return v
  if (!isSupportedBuild((await $.session.version()).base)) return v
  if (await managedRulesOnly($)) return v
  const paths = await classify($, command)
  if (paths === undefined || !(await readsAllowed($, paths))) return v
  return { decision: 'allow', reason: 'anthology: read-only command' }
}

export function registerPermissions(on: On): void {
  on('tool.check', { tool: 'Bash' }, ($, e, next) => checkBash($, e, next)).catch(($, e, next) => next(e))
}
