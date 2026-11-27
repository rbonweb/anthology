// Strict delegation (post-release mods design §3). In an interactive session the lead plans,
// briefs agents, checks and merges; it does not make the changes. Two parts:
//   - `prompt.compose` adds a short coordinator section when the session has the Agent tool;
//   - `tool.call` refuses the main loop's Edit, Write and NotebookEdit, says why, names the agent
//     to hand the change to (the project's own matching agent when one fits, else ours), and lets
//     through the lead's own notes (memory, plans, `.superpowers/`), files that are not project
//     code (the Claude config folder; anything outside every git repo) and a tiny fix: one Edit,
//     at most 3 per turn, all in one file, counted only when the edit succeeded. How tiny depends
//     on the risk: 5 lines and about 2 KB; a small config file 15 lines; a pure deletion 30 lines
//     and 4 KB.
// Only those three file tools are gated: a dispatch (the `Agent` tool) to any agent, the project's
// own included, is never touched, and neither is a call to any other tool.
// A repo can switch the mod off with `"delegation": false` in `<root>/.claude/anthology.json`.
// Subagent calls (they carry an `agentId`) are never touched. A headless run (`claude -p`, the
// SDK with nothing attached: `$.session.surfaces()` is empty) is never touched either, and
// neither is a session whose tool pool has no `Agent` tool, since the lead could not hand the work
// on, nor an embedded host (an SDK program drives the session and owns its system prompt; see
// `isEmbeddedHost`), whatever surface is attached: there the coordinator section is left out too.
// Nothing at `tool.call` tells a teammate in a pane of its own from a lead (an in-process
// teammate carries an `agentId` and is skipped already), so the `Agent` tool in the pool is the
// rule there: a pane teammate that has it is treated as a lead.
// This is a workflow rule, not a safety boundary: every hook fails open, and a Bash write
// (`sed -i`, a redirect) is not seen. ANTHOLOGY_DELEGATION=0 (or ANTHOLOGY_MODS=0) turns it off.
import type { EngineInterface, On, PromptComposeSection } from 'claude-code'
import { modsOff } from './util.ts'

// Two values in `$.state`, declared in types/anthology.d.ts. `delegation`: the file the lead
// edited this turn and how many edits it has used. `coordinator`: whether the prompt carries the
// coordinator section (set by prompt.compose; unset or false makes a refusal carry the rule).
const budget = { plugin: 'anthology', key: 'delegation' } as const
const coordinator = { plugin: 'anthology', key: 'coordinator' } as const
const NO_EDITS = { file: '', edits: 0 }

const MAX_EDIT_LINES = 5
const MAX_EDIT_CHARS = 2048
const MAX_EDITS_PER_TURN = 3
// Risk, not only size. A pure deletion adds nothing to review; a small config file is read at a
// glance. Code that is added or changed stays at MAX_EDIT_LINES.
const MAX_DELETE_LINES = 30
const MAX_DELETE_CHARS = 4096
const MAX_CONFIG_LINES = 15
const CONFIG_FILE = /\.(json|jsonc|json5|ya?ml|toml|ini|cfg|conf|env|properties|plist)$/i
/** Folders looked at on the way up from a file to see whether a `.git` is there. */
const MAX_WALK = 64

// Steady text (the prompt cache holds), 400 characters at most.
const COORDINATOR: PromptComposeSection = {
  id: 'anthology:coordinator',
  scope: 'session',
  text:
    'Coordinator: you lead, you do not make. Plan, brief agents, check their work and merge. ' +
    "Hand every change to the matching agent: the project's own when one fits, else " +
    'anthology:implementer, anthology:debugger, anthology:docs-writer or anthology:ui-designer. ' +
    'Inline only a fix of at most a few lines in one file that the user asked for (5 lines, ' +
    '3 edits a turn). Memory, plans, .superpowers are yours.',
}

/**
 * The boolean `key` of `<session root>/.claude/anthology.json`, the repo's own switches; undefined
 * when there is no such file, it is not a JSON object, or the key is not a boolean.
 */
async function repoSwitch($: EngineInterface, key: string): Promise<boolean | undefined> {
  try {
    const root = await $.session.root()
    const parsed: unknown = JSON.parse(await $.fs.read(`${root}/.claude/anthology.json`))
    const value = typeof parsed === 'object' && parsed !== null ? (parsed as { readonly [k: string]: unknown })[key] : undefined
    return typeof value === 'boolean' ? value : undefined
  } catch {
    return undefined
  }
}

/**
 * ANTHOLOGY_DELEGATION=0 turns this mod off; ANTHOLOGY_MODS=0 turns every anthology mod off; a
 * repo turns it off with `"delegation": false` in its `.claude/anthology.json`.
 */
async function delegationOff($: EngineInterface): Promise<boolean> {
  return (
    (await modsOff($)) ||
    (await $.env.get('ANTHOLOGY_DELEGATION')) === '0' ||
    (await repoSwitch($, 'delegation')) === false
  )
}

/** A person can be at the session: some surface draws. Empty in a plain `-p` run. */
async function isInteractive($: EngineInterface): Promise<boolean> {
  return (await $.session.surfaces()).length > 0
}

/**
 * An embedded host: a program drives the session through the SDK and nobody works at it, even when
 * a surface is attached. Same rule as the guard's host check: the mode is not `full`, and
 * `CLAUDE_CODE_SESSION_ATTENDED` decides when it is `0` (embedded) or `1` (not). Otherwise the
 * entrypoint decides: it starts with `sdk-`. An interactive `claude` that an SDK process started
 * inherits an `sdk-*` entrypoint, but Claude Code recomputes the attended flag for each session.
 * Read through `$.env.get`: `process` does not exist in a mod.
 */
async function isEmbeddedHost($: EngineInterface): Promise<boolean> {
  if ((await $.env.get('ANTHOLOGY_HOST_MODE')) === 'full') return false
  const attended = await $.env.get('CLAUDE_CODE_SESSION_ATTENDED')
  if (attended === '0') return true
  if (attended === '1') return false
  return ((await $.env.get('CLAUDE_CODE_ENTRYPOINT')) ?? '').startsWith('sdk-')
}

/** The lead can hand work on only when the session's tool pool has the Agent tool. */
async function canDispatch($: EngineInterface): Promise<boolean> {
  return (await $.tool.list()).some(t => t.name === 'Agent')
}

/** The absolute path with `.` and `..` folded away (no symlinks resolved). */
function normalize(path: string): string {
  const kept: string[] = []
  for (const part of path.split('/')) {
    if (part === '' || part === '.') continue
    if (part === '..') kept.pop()
    else kept.push(part)
  }
  return `/${kept.join('/')}`
}

/**
 * `path` made absolute and folded: `~` is the home folder, a relative path is taken from the
 * session's directory. Undefined when it cannot be made absolute (no home, nothing given).
 */
async function resolvePath($: EngineInterface, path: string): Promise<string | undefined> {
  if (path === '') return undefined
  if (path === '~' || path.startsWith('~/')) {
    const home = await $.env.get('HOME')
    return home?.startsWith('/') ? normalize(`${home}${path.slice(1)}`) : undefined
  }
  if (path.startsWith('/')) return normalize(path)
  return normalize(`${await $.session.cwd()}/${path}`)
}

function inside(path: string, dir: string): boolean {
  return path.startsWith(dir === '/' ? '/' : `${dir}/`)
}

/**
 * The lead's own notes: under Claude's config folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`) the
 * auto-memory folder `projects/<project>/memory/`, `plans/` and Anthology's personal memory
 * `anthology/personal-memory/`; the `autoMemoryDirectory` setting when set; and in the project the
 * repo memory `.claude/memory/`, `.superpowers/` and `docs/superpowers/plans/`.
 */
async function isLeadNote($: EngineInterface, file: string): Promise<boolean> {
  const path = await resolvePath($, file)
  if (path === undefined) return false
  const home = await $.env.get('HOME')
  const config = (await $.env.get('CLAUDE_CONFIG_DIR')) ?? (home ? `${home}/.claude` : '')
  const claude = await resolvePath($, config)
  const root = await resolvePath($, await $.session.root())
  const custom = await $.settings.read({ key: 'autoMemoryDirectory' })
  const dirs: (string | undefined)[] = [
    claude && `${claude}/plans`,
    claude && `${claude}/anthology/personal-memory`,
    root && `${root}/.superpowers`,
    root && `${root}/docs/superpowers/plans`,
    root && `${root}/.claude/memory`,
    typeof custom === 'string' ? await resolvePath($, custom) : undefined,
  ]
  if (dirs.some(dir => dir !== undefined && inside(path, dir))) return true
  if (claude !== undefined && inside(path, `${claude}/projects`)) {
    // projects/<project>/memory/<file>
    const [project, memory, ...rest] = path.slice(`${claude}/projects/`.length).split('/')
    return !!project && memory === 'memory' && rest.length > 0
  }
  return false
}

/** Whether a `.git` (a folder, or a file in a worktree) is in the folder of `path` or above it. */
async function inGitRepo($: EngineInterface, path: string): Promise<boolean> {
  try {
    let dir = path.slice(0, path.lastIndexOf('/'))
    for (let depth = 0; depth < MAX_WALK; depth++) {
      if (await $.fs.exists(`${dir}/.git`)) return true
      if (dir === '') return false
      dir = dir.slice(0, dir.lastIndexOf('/'))
    }
    return true
  } catch {
    // Unknown is project code: the file is handed off as before.
    return true
  }
}

/**
 * Files that are not project code, so there is nothing to hand off: the Claude config folder
 * (`CLAUDE_CONFIG_DIR`, else `~/.claude`: settings, keybindings, agents, plugins), even when a
 * dotfiles repo holds it, and any file outside every git repo.
 */
async function isOutsideProject($: EngineInterface, file: string): Promise<boolean> {
  const path = await resolvePath($, file)
  if (path === undefined) return false
  const home = await $.env.get('HOME')
  const claude = await resolvePath($, (await $.env.get('CLAUDE_CONFIG_DIR')) ?? (home ? `${home}/.claude` : ''))
  if (claude !== undefined && inside(path, claude)) return true
  return !(await inGitRepo($, path))
}

function field(e: object, name: string): string {
  const v = (e as { readonly [k: string]: unknown })[name]
  return typeof v === 'string' ? v : ''
}

function lineCount(text: string): number {
  return text === '' ? 0 : text.replace(/\n$/, '').split('\n').length
}

/** How much an Edit may change and still be a tiny fix, by what it does and to what. */
function limitsFor(file: string, deleting: boolean): { what: string; lines: number; chars: number } {
  if (deleting) return { what: 'a deletion', lines: MAX_DELETE_LINES, chars: MAX_DELETE_CHARS }
  if (CONFIG_FILE.test(file)) return { what: 'a config edit', lines: MAX_CONFIG_LINES, chars: MAX_EDIT_CHARS }
  return { what: 'an inline fix', lines: MAX_EDIT_LINES, chars: MAX_EDIT_CHARS }
}

/** Why an Edit is not a tiny fix, or undefined when it is one (before the budget is looked at). */
function whyNotTiny(e: object, file: string): string | undefined {
  if ((e as { readonly replace_all?: unknown }).replace_all === true) {
    return 'replace_all is not an inline fix'
  }
  const before = field(e, 'old_string')
  const after = field(e, 'new_string')
  const limit = limitsFor(file, before.trim() !== '' && after.trim() === '')
  const lines = Math.max(lineCount(before), lineCount(after))
  if (lines > limit.lines) return `${limit.what} is at most ${limit.lines} lines and this one is ${lines}`
  const chars = before.length + after.length
  if (chars > limit.chars) {
    return `${limit.what} is at most about ${Math.round(limit.chars / 1024)} KB changed and this one is ${chars} characters`
  }
  return undefined
}

/**
 * Spends one edit of this turn's tiny-fix budget on `file`; returns why not when the budget is
 * gone or the turn already edited another file, undefined when it is spent. Compare-and-set, so
 * two calls in parallel cannot both take the last edit. The edit gives it back if it fails.
 */
async function reserveTinyFix($: EngineInterface, file: string): Promise<string | undefined> {
  const where = (await resolvePath($, file)) ?? file
  for (let attempt = 0; attempt < 5; attempt++) {
    const { value, version } = await $.state.get(budget)
    const used = value ?? NO_EDITS
    if (used.edits >= MAX_EDITS_PER_TURN) return `this turn already used its ${MAX_EDITS_PER_TURN} inline edits`
    if (used.edits > 0 && used.file !== where) return `this turn's inline edits are already in ${used.file}`
    const set = await $.state.set(budget, { file: where, edits: used.edits + 1 }, { ifVersion: version })
    if (set.isSet) return undefined
  }
  return 'the inline edit budget is busy'
}

async function releaseTinyFix($: EngineInterface): Promise<void> {
  for (let attempt = 0; attempt < 5; attempt++) {
    const { value, version } = await $.state.get(budget)
    const used = value ?? NO_EDITS
    if (used.edits <= 0) return
    const left = used.edits - 1
    const set = await $.state.set(budget, { file: left === 0 ? '' : used.file, edits: left }, { ifVersion: version })
    if (set.isSet) return
  }
}

/** The agent the change belongs to, by the file it is for. */
function agentFor(file: string): string {
  if (/\.(md|mdx|markdown|rst|txt)$/i.test(file)) return 'anthology:docs-writer'
  if (/\.(tsx|jsx|css|html|svg)$/i.test(file)) return 'anthology:ui-designer'
  return 'anthology:implementer'
}

/** True only when prompt.compose said the coordinator section is in this session's prompt. */
async function hasSection($: EngineInterface): Promise<boolean> {
  try {
    return (await $.state.get(coordinator)).value === true
  } catch {
    return false
  }
}

async function remember($: EngineInterface, composed: boolean): Promise<void> {
  try {
    if ((await $.state.get(coordinator)).value !== composed) await $.state.set(coordinator, composed)
  } catch {
    // Unknown stays "not composed": a refusal then carries the rule itself.
  }
}

async function refusal($: EngineInterface, file: string, why: string): Promise<string> {
  const agent = agentFor(file)
  const bug = agent === 'anthology:implementer' ? ' (anthology:debugger for a bug fix)' : ''
  const rule = (await hasSection($)) ? '' : ` ${COORDINATOR.text}`
  return (
    `anthology: the lead hands off making and changing (${why}). ` +
    `Dispatch the project's own matching agent when one fits, else ${agent}${bug}, with this change. ` +
    `Inline only: an Edit of at most ${MAX_EDIT_LINES} lines (${MAX_CONFIG_LINES} in a config file, ${MAX_DELETE_LINES} for a pure deletion) ` +
    `in one file, at most ${MAX_EDITS_PER_TURN} per turn. Files outside a git repo and the Claude config folder are yours. ` +
    `Set ANTHOLOGY_DELEGATION=0, or "delegation": false in .claude/anthology.json, to turn this off.${rule}`
  )
}

export function registerDelegation(on: On): void {
  on('prompt.compose', async ($, e, next) => {
    const r = await next(e)
    if (!e.tools.includes('Agent') || e.surfaces.length === 0) return r
    if (e.traits.includes('bare') || e.traits.includes('teammate')) {
      await remember($, false)
      return r
    }
    // An embedded host owns its system prompt: no coordinator section there.
    if ((await delegationOff($)) || (await isEmbeddedHost($))) return r
    await remember($, true)
    if (r.sections.some(s => s.id === COORDINATOR.id)) return r
    return { sections: [...r.sections, COORDINATOR] }
  }).catch(($, e, next) => next(e))

  // The budget is per turn of the main loop: a subagent's own turn leaves it alone.
  on('turn.start', async ($, e, next) => {
    if (e.agentId === undefined) await $.state.set(budget, NO_EDITS)
    return next(e)
  }).catch(($, e, next) => next(e))

  on('tool.call', { tool: ['Edit', 'Write', 'NotebookEdit'] }, async ($, e, next) => {
    if (e.agentId !== undefined || (await delegationOff($)) || !(await isInteractive($))) return next(e)
    if (await isEmbeddedHost($)) return next(e)
    const file = field(e, e.tool === 'NotebookEdit' ? 'notebook_path' : 'file_path')
    if ((await isLeadNote($, file)) || (await isOutsideProject($, file))) return next(e)
    if (!(await canDispatch($))) return next(e)
    const why = e.tool === 'Edit' ? whyNotTiny(e, file) : `a ${e.tool} is never an inline fix`
    if (why !== undefined) return { deny: await refusal($, file, why) }
    const busy = await reserveTinyFix($, file)
    if (busy !== undefined) return { deny: await refusal($, file, busy) }
    // Only an edit that went through keeps its place in the budget.
    const r = await next(e)
    if (r.deny !== undefined || r.isError === true) await releaseTinyFix($)
    return r
  }).catch(($, e, next) => next(e))
}
