// Strict delegation (post-release mods design §3). In an interactive session the lead plans,
// briefs agents, checks and merges; it does not make the changes. Two parts:
//   - `prompt.compose` adds a short coordinator section when the session has the Agent tool;
//   - `tool.call` refuses the main loop's Edit, Write and NotebookEdit, names the agent to hand
//     the change to, and lets through the lead's own notes (memory, plans, `.superpowers/`) and
//     a tiny fix (one Edit of at most 5 lines, at most 3 per turn, all in one file).
// Subagent calls (they carry an `agentId`) are never touched. A headless run (`claude -p`, the
// SDK with nothing attached: `$.session.surfaces()` is empty) is never touched either.
// This is a workflow rule, not a safety boundary: every hook fails open, and a Bash write
// (`sed -i`, a redirect) is not seen. ANTHOLOGY_DELEGATION=0 (or ANTHOLOGY_MODS=0) turns it off.
import type { EngineInterface, On, PromptComposeSection } from 'claude-code'
import { modsOff } from './util.ts'

// The tiny-fix budget lives in `$.state`, declared in types/anthology.d.ts: the file the lead
// edited this turn and how many edits it has used.
const budget = { plugin: 'anthology', key: 'delegation' } as const
const NO_EDITS = { file: '', edits: 0 }

const MAX_EDIT_LINES = 5
const MAX_EDITS_PER_TURN = 3

// Steady text (the prompt cache holds), 400 characters at most.
const COORDINATOR: PromptComposeSection = {
  id: 'anthology:coordinator',
  scope: 'session',
  text:
    'Coordinator: you lead, you do not make. Plan, brief agents, check their work and merge. ' +
    'Hand every change to the matching agent: anthology:implementer, anthology:debugger, ' +
    'anthology:docs-writer or anthology:ui-designer. Inline Edit only for a tiny fix: at most ' +
    '5 lines, 3 edits, one file per turn. Other lead Edit and Write calls are refused, except ' +
    'memory, plans and .superpowers notes.',
}

/** ANTHOLOGY_DELEGATION=0 turns this mod off; ANTHOLOGY_MODS=0 turns every anthology mod off. */
async function delegationOff($: EngineInterface): Promise<boolean> {
  return (await modsOff($)) || (await $.env.get('ANTHOLOGY_DELEGATION')) === '0'
}

/** A person can be at the session: some surface draws. Empty in a plain `-p` run. */
async function isInteractive($: EngineInterface): Promise<boolean> {
  return (await $.session.surfaces()).length > 0
}

/** The absolute path with `.` and `..` folded away (no symlinks resolved); undefined if relative. */
function normalize(path: string): string | undefined {
  if (!path.startsWith('/')) return undefined
  const kept: string[] = []
  for (const part of path.split('/')) {
    if (part === '' || part === '.') continue
    if (part === '..') kept.pop()
    else kept.push(part)
  }
  return `/${kept.join('/')}`
}

function inside(path: string, dir: string): boolean {
  return path.startsWith(dir === '/' ? '/' : `${dir}/`)
}

/**
 * The lead's own notes: the auto-memory folder, `plans/` (both under Claude's config folder),
 * and the project's `.superpowers/` and `docs/superpowers/plans/`.
 */
async function isLeadNote($: EngineInterface, file: string): Promise<boolean> {
  const path = normalize(file)
  if (path === undefined) return false
  const home = await $.env.get('HOME')
  const config = (await $.env.get('CLAUDE_CONFIG_DIR')) ?? (home ? `${home}/.claude` : undefined)
  const claude = config === undefined ? undefined : normalize(config)
  if (claude !== undefined) {
    if (inside(path, `${claude}/plans`)) return true
    if (inside(path, `${claude}/projects`)) {
      // projects/<project>/memory/<file>
      const [project, memory, ...rest] = path.slice(`${claude}/projects/`.length).split('/')
      if (project && memory === 'memory' && rest.length > 0) return true
    }
  }
  const root = normalize(await $.session.root())
  if (root === undefined) return false
  return inside(path, `${root}/.superpowers`) || inside(path, `${root}/docs/superpowers/plans`)
}

function field(e: object, name: string): string {
  const v = (e as { readonly [k: string]: unknown })[name]
  return typeof v === 'string' ? v : ''
}

function lineCount(text: string): number {
  return text === '' ? 0 : text.replace(/\n$/, '').split('\n').length
}

/** An Edit of one place (`replace_all` off) with at most 5 lines on each side. */
function isSmallEdit(e: object): boolean {
  if ((e as { readonly replace_all?: unknown }).replace_all === true) return false
  return (
    lineCount(field(e, 'old_string')) <= MAX_EDIT_LINES &&
    lineCount(field(e, 'new_string')) <= MAX_EDIT_LINES
  )
}

/**
 * Spends one edit of this turn's tiny-fix budget on `file`; false when the budget is gone or the
 * turn already edited another file. Compare-and-set, so two calls in parallel cannot both take
 * the last edit.
 */
async function takeTinyFix($: EngineInterface, file: string): Promise<boolean> {
  const where = normalize(file) ?? file
  for (let attempt = 0; attempt < 5; attempt++) {
    const { value, version } = await $.state.get(budget)
    const used = value ?? NO_EDITS
    if (used.edits >= MAX_EDITS_PER_TURN) return false
    if (used.edits > 0 && used.file !== where) return false
    const set = await $.state.set(budget, { file: where, edits: used.edits + 1 }, { ifVersion: version })
    if (set.isSet) return true
  }
  return false
}

/** The agent the change belongs to, by the file it is for. */
function agentFor(file: string): string {
  if (/\.(md|mdx|markdown|rst|txt)$/i.test(file)) return 'anthology:docs-writer'
  if (/\.(tsx|jsx|css|html|svg)$/i.test(file)) return 'anthology:ui-designer'
  return 'anthology:implementer'
}

function refusal(file: string): string {
  const agent = agentFor(file)
  const bug = agent === 'anthology:implementer' ? ' (anthology:debugger for a bug fix)' : ''
  return (
    `anthology: the lead hands off making and changing. Dispatch ${agent}${bug} with this change. ` +
    `Inline only: an Edit of at most ${MAX_EDIT_LINES} lines in one file, at most ${MAX_EDITS_PER_TURN} per turn. ` +
    'Set ANTHOLOGY_DELEGATION=0 to turn this off.'
  )
}

export function registerDelegation(on: On): void {
  on('prompt.compose', async ($, e, next) => {
    const r = await next(e)
    if (!e.tools.includes('Agent') || e.surfaces.length === 0) return r
    if (e.traits.includes('bare') || e.traits.includes('teammate')) return r
    if (r.sections.some(s => s.id === COORDINATOR.id) || (await delegationOff($))) return r
    return { sections: [...r.sections, COORDINATOR] }
  }).catch(($, e, next) => next(e))

  // The budget is per turn of the main loop: a subagent's own turn leaves it alone.
  on('turn.start', async ($, e, next) => {
    if (e.agentId === undefined) await $.state.set(budget, NO_EDITS)
    return next(e)
  }).catch(($, e, next) => next(e))

  on('tool.call', { tool: ['Edit', 'Write', 'NotebookEdit'] }, async ($, e, next) => {
    if (e.agentId !== undefined || (await delegationOff($)) || !(await isInteractive($))) return next(e)
    const file = field(e, e.tool === 'NotebookEdit' ? 'notebook_path' : 'file_path')
    if (await isLeadNote($, file)) return next(e)
    if (e.tool === 'Edit' && isSmallEdit(e) && (await takeTinyFix($, file))) return next(e)
    return { deny: refusal(file) }
  }).catch(($, e, next) => next(e))
}
