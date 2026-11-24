// delegation.ts (post-release mods design §3): in an interactive session the lead's own Edit,
// Write and NotebookEdit are refused, with a few exceptions, and the system prompt gets a short
// coordinator section when the session has the Agent tool. The test's own hooks sit beneath the
// plugin and stand for Claude Code: `tool.call` is the tool itself (it records what ran),
// `session.*` and `env.get` are the session, `prompt.compose` is the engine's own composition.
import type { On, PromptComposeSection, PromptComposeTrait, RenderSurface } from 'claude-code'
import { type Engine, expect, test } from 'claude-code/testing'

const HOME = '/home/dev'
const ROOT = '/work/repo'

type World = {
  surfaces?: readonly RenderSurface[]
  env?: { readonly [name: string]: string }
  sections?: readonly PromptComposeSection[]
  /** the tool pool the session lists; the lead can dispatch only with `Agent` in it */
  tools?: readonly string[]
  cwd?: string
  /** the `autoMemoryDirectory` setting */
  autoMemoryDirectory?: string
  /** events whose hook beneath the plugin throws, to see the mod fail open */
  fails?: readonly ('env.get' | 'state.get' | 'state.set' | 'settings.read' | 'tool.list' | 'session.root')[]
  /** an Edit of a path holding this text comes back from the tool as an error */
  broken?: string
  /** a tool.call of a path holding this text is refused beneath the plugin (another plugin's deny) */
  denied?: string
}

const boom = (): never => {
  throw new Error('boom')
}

/** Stands for Claude Code beneath the plugin. Returns what the tools were asked to do. */
function engine(on: On, world: World = {}): string[] {
  const ran: string[] = []
  const env = { HOME, ...world.env } as { readonly [name: string]: string }
  const fails = new Set<string>(world.fails)
  const tools = world.tools ?? ['Agent', 'Read']
  on('session.surfaces', () => ({ value: world.surfaces ?? ['terminal'] }))
  on('session.root', fails.has('session.root') ? boom : () => ({ value: ROOT }))
  on('session.cwd', () => ({ value: world.cwd ?? ROOT }))
  on('env.get', fails.has('env.get') ? boom : (_$, e) => ({ value: env[e.name] }))
  // the event answers the whole settings object; a read of one key is that object, projected
  on(
    'settings.read',
    fails.has('settings.read')
      ? boom
      : () => ({ value: world.autoMemoryDirectory === undefined ? {} : { autoMemoryDirectory: world.autoMemoryDirectory } }),
  )
  on('tool.list', fails.has('tool.list') ? boom : () => ({ value: tools.map(name => ({ name, description: '', mcp: false })) }))
  // the kit answers `$.state` itself, and a hook that throws is skipped; a `{ deny }` makes the call reject
  if (fails.has('state.get')) on('state.get', () => ({ deny: 'boom' }) as never)
  if (fails.has('state.set')) on('state.set', () => ({ deny: 'boom' }) as never)
  on('turn.start', (_$, e) => ({ turnId: e.turnId }))
  on('tool.call', (_$, e) => {
    const path = String(e.file_path ?? e.notebook_path)
    ran.push(`${e.tool} ${path}`)
    if (world.denied !== undefined && path.includes(world.denied)) return { deny: 'blocked beneath' }
    if (world.broken !== undefined && path.includes(world.broken)) return { result: 'boom', isError: true as const }
    return { result: 'done' }
  })
  const sections = world.sections ?? [{ id: 'intro', text: 'You are Claude Code.', scope: 'shared' as const }]
  on('prompt.compose', () => ({ sections }))
  return ran
}

const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i}`).join('\n')

type Eng = Engine

function edit($: Eng, file_path: string, old_string = 'a', new_string = 'b', extra: object = {}) {
  return $.tool.call({ tool: 'Edit', file_path, old_string, new_string, ...extra })
}

let turn = 0
function newTurn($: Eng, agentId?: string) {
  turn += 1
  return $.turn.start({ text: 'go', turnId: `t${turn}`, ...(agentId ? { agentId } : {}) })
}

const SRC = `${ROOT}/src/app.ts`

test('the main loop is refused a Write and the message names the implementer', async ($, on) => {
  const ran = engine(on)
  const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(r.deny).toContain('anthology:implementer')
  expect(ran).toHaveLength(0)
})

test('the main loop is refused a NotebookEdit', async ($, on) => {
  const ran = engine(on)
  const r = await $.tool.call({
    tool: 'NotebookEdit',
    notebook_path: `${ROOT}/n.ipynb`,
    new_source: 'x',
  })
  expect(r.deny).toContain('anthology:implementer')
  expect(ran).toHaveLength(0)
})

test('the refusal names the agent for the file: docs, UI, anything else', async ($, on) => {
  engine(on)
  const md = await $.tool.call({ tool: 'Write', file_path: `${ROOT}/docs/a.md`, content: 'x' })
  expect(md.deny).toContain('anthology:docs-writer')
  for (const f of ['a.tsx', 'a.jsx', 'a.css', 'a.html', 'a.svg']) {
    const ui = await $.tool.call({ tool: 'Write', file_path: `${ROOT}/web/${f}`, content: 'x' })
    expect(ui.deny).toContain('anthology:ui-designer')
  }
  const other = await $.tool.call({ tool: 'Write', file_path: `${ROOT}/b.rs`, content: 'x' })
  expect(other.deny).toContain('anthology:implementer')
  expect(other.deny).toContain('anthology:debugger')
})

test('the refusal says how to switch the mod off', async ($, on) => {
  engine(on)
  const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(r.deny).toContain('ANTHOLOGY_DELEGATION=0')
})

test('a headless run (no surface) is not refused', async ($, on) => {
  const ran = engine(on, { surfaces: [] })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toBeUndefined()
  expect(ran).toEqual([`Write ${SRC}`])
})

// An embedded host (an SDK program that drives the session, nobody at a terminal) is never refused,
// even when a surface is attached and the Agent tool is in the pool. The test names the variables
// Claude Code sets: the entrypoint and whether a person attends the session.
const SDK_HOST = { CLAUDE_CODE_ENTRYPOINT: 'sdk-ts', CLAUDE_CODE_SESSION_ATTENDED: '0' }

test('skips an SDK host that has the Agent tool and a surface', async ($, on) => {
  const ran = engine(on, { surfaces: ['terminal'], tools: ['Agent', 'Read'], env: SDK_HOST })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toBeUndefined()
  expect((await edit($, SRC, lines(30), lines(30))).deny).toBeUndefined()
  expect(ran).toHaveLength(2)
})

test('still refuses in cli with a surface', async ($, on) => {
  const ran = engine(on, { env: { CLAUDE_CODE_ENTRYPOINT: 'cli', CLAUDE_CODE_SESSION_ATTENDED: '1' } })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toContain('anthology:implementer')
  expect(ran).toHaveLength(0)
})

test('still refuses in claude-vscode attended', async ($, on) => {
  const ran = engine(on, { env: { CLAUDE_CODE_ENTRYPOINT: 'claude-vscode', CLAUDE_CODE_SESSION_ATTENDED: '1' } })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toContain('anthology:implementer')
  expect(ran).toHaveLength(0)
})

test('host mode full keeps the refusal', async ($, on) => {
  const ran = engine(on, { env: { ...SDK_HOST, ANTHOLOGY_HOST_MODE: 'full' } })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toContain('anthology:implementer')
  expect(ran).toHaveLength(0)
})

for (const surface of ['terminal', 'desktop', 'vscode', 'mobile'] as const) {
  test(`the ${surface} surface counts as interactive`, async ($, on) => {
    engine(on, { surfaces: [surface] })
    const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
    expect(r.deny).toContain('anthology:implementer')
  })
}

test('a session whose tool pool has no Agent tool is not refused', async ($, on) => {
  const ran = engine(on, { tools: ['Read', 'Edit'] })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toBeUndefined()
  expect((await edit($, SRC, lines(30), lines(30))).deny).toBeUndefined()
  expect(ran).toHaveLength(2)
})

test('a subagent call passes and is never counted', async ($, on) => {
  const ran = engine(on)
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x', agentId: 'a1' })
  expect(w.deny).toBeUndefined()
  for (let i = 0; i < 4; i++) {
    const r = await edit($, SRC, 'a', 'b', { agentId: 'a1' })
    expect(r.deny).toBeUndefined()
  }
  // the lead still has its whole budget
  const lead = await edit($, SRC)
  expect(lead.deny).toBeUndefined()
  expect(ran).toHaveLength(6)
})

test('ANTHOLOGY_DELEGATION=0 turns the mod off', async ($, on) => {
  const ran = engine(on, { env: { ANTHOLOGY_DELEGATION: '0' } })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toBeUndefined()
  expect(ran).toHaveLength(1)
})

test('ANTHOLOGY_MODS=0 turns the mod off', async ($, on) => {
  const ran = engine(on, { env: { ANTHOLOGY_MODS: '0' } })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toBeUndefined()
  expect(ran).toHaveLength(1)
})

test('any other value of the switch leaves the mod on', async ($, on) => {
  engine(on, { env: { ANTHOLOGY_DELEGATION: '1' } })
  const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(w.deny).toContain('anthology:implementer')
})

// Exceptions: where the lead writes its own notes.
const EXEMPT = [
  `${HOME}/.claude/projects/-work-repo/memory/note.md`,
  `${HOME}/.claude/projects/-work-repo/memory/MEMORY.md`,
  `${HOME}/.claude/plans/p.md`,
  `${ROOT}/.superpowers/sdd/ledger.md`,
  `${ROOT}/docs/superpowers/plans/p.md`,
  // repo memory and personal memory, where auto memory is off (an Anthology-approved repo)
  `${ROOT}/.claude/memory/project-x.md`,
  `${ROOT}/.claude/memory/MEMORY.md`,
  `${HOME}/.claude/anthology/personal-memory/me.md`,
  // forms of the same paths: `~`, `..` and a relative path from the session's directory
  '~/.claude/plans/p.md',
  '~/.claude/anthology/personal-memory/me.md',
  `${ROOT}/src/../.superpowers/a.md`,
  '.superpowers/a.md',
  './.claude/memory/a.md',
  'docs/superpowers/plans/p.md',
]

for (const path of EXEMPT) {
  test(`the lead may Write ${path}`, async ($, on) => {
    const ran = engine(on)
    const w = await $.tool.call({ tool: 'Write', file_path: path, content: lines(40) })
    expect(w.deny).toBeUndefined()
    expect(ran).toHaveLength(1)
  })

  test(`a long Edit of ${path} passes and costs no budget`, async ($, on) => {
    engine(on)
    const e = await edit($, path, lines(30), lines(31))
    expect(e.deny).toBeUndefined()
    for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  })
}

test('the lead may NotebookEdit under .superpowers', async ($, on) => {
  engine(on)
  const r = await $.tool.call({
    tool: 'NotebookEdit',
    notebook_path: `${ROOT}/.superpowers/n.ipynb`,
    new_source: 'x',
  })
  expect(r.deny).toBeUndefined()
})

test('a path that climbs out of an exempt folder is refused', async ($, on) => {
  engine(on)
  for (const p of [
    `${ROOT}/.superpowers/../src/app.ts`,
    `${ROOT}/docs/superpowers/plans/../../../src/app.ts`,
    `${HOME}/.claude/plans/../settings.json`,
    `${HOME}/.claude/projects/-work-repo/memory/../../x.json`,
    `${ROOT}/.claude/memory/../settings.json`,
    `${HOME}/.claude/anthology/personal-memory/../state.json`,
    '../outside.md',
    '.superpowers/../src/app.ts',
  ]) {
    const w = await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })
    expect(w.deny).toContain('anthology:')
  }
})

test('look-alike folders are refused', async ($, on) => {
  engine(on)
  for (const p of [
    `${ROOT}/.superpowers-extra/a.md`,
    `${ROOT}/docs/superpowers/other/a.md`,
    `${HOME}/.claude/projects/-work-repo/notes/a.md`,
    `${HOME}/.claude/projects/memory/a.md`,
    `${HOME}/.claude/settings.json`,
    `${HOME}/.claude/plansx/a.md`,
    'relative/.superpowers/a.md',
    // look-alikes of the memory folders
    `${ROOT}/.claude/memory-old/a.md`,
    `${ROOT}/.claude/memoryx.md`,
    `${ROOT}/.claude/agent-memory/a.md`,
    `${ROOT}/.claude/settings.json`,
    `${ROOT}/src/.claude/memory/a.md`,
    `${HOME}/.claude/anthology/personal-memory-x/a.md`,
    `${HOME}/.claude/anthology/personal-memory`,
    `${HOME}/.claude/anthology/state.json`,
    `${HOME}/.claude/anthology/atl/x`,
    '~root/.claude/plans/a.md',
  ]) {
    const w = await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })
    expect(w.deny).toContain('anthology:')
  }
})

test('CLAUDE_CONFIG_DIR moves the memory, plans and personal-memory folders', async ($, on) => {
  engine(on, { env: { CLAUDE_CONFIG_DIR: '/cfg' } })
  for (const p of ['/cfg/projects/p/memory/a.md', '/cfg/plans/a.md', '/cfg/anthology/personal-memory/a.md']) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toBeUndefined()
  }
  for (const p of [`${HOME}/.claude/plans/a.md`, `${HOME}/.claude/anthology/personal-memory/a.md`, '/cfg/settings.json']) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toContain('anthology:')
  }
})

test('the autoMemoryDirectory setting is the lead memory folder when it is set', async ($, on) => {
  engine(on, { autoMemoryDirectory: '/data/memo' })
  for (const p of ['/data/memo/a.md', '/data/memo/sub/b.md']) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toBeUndefined()
  }
  for (const p of ['/data/memo', '/data/memo-old/a.md', '/data/memo/../x.md', '/data/other/a.md']) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toContain('anthology:')
  }
})

test('the autoMemoryDirectory setting may start with ~', async ($, on) => {
  engine(on, { autoMemoryDirectory: '~/notes/memory' })
  expect((await $.tool.call({ tool: 'Write', file_path: `${HOME}/notes/memory/a.md`, content: 'x' })).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'Write', file_path: `${HOME}/notes/a.md`, content: 'x' })).deny).toContain('anthology:')
})

test('nothing is exempt for the setting when it is not set', async ($, on) => {
  engine(on)
  expect((await $.tool.call({ tool: 'Write', file_path: '/data/memo/a.md', content: 'x' })).deny).toContain('anthology:')
})

test('a relative path is taken from the session directory', async ($, on) => {
  engine(on, { cwd: `${ROOT}/pkg` })
  // ROOT/pkg/.superpowers is not the project's .superpowers
  expect((await $.tool.call({ tool: 'Write', file_path: '.superpowers/a.md', content: 'x' })).deny).toContain('anthology:')
  expect((await $.tool.call({ tool: 'Write', file_path: '../.superpowers/a.md', content: 'x' })).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'Write', file_path: '../.claude/memory/a.md', content: 'x' })).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'Write', file_path: 'src/a.ts', content: 'x' })).deny).toContain('anthology:')
})

test('~ with no home folder is not exempt', async ($, on) => {
  engine(on, { env: { HOME: '' } })
  expect((await $.tool.call({ tool: 'Write', file_path: '~/.claude/plans/a.md', content: 'x' })).deny).toContain('anthology:')
})

// The tiny-fix budget: an Edit of at most 5 lines, at most 3 per turn, all in one file.
test('an Edit of 5 lines is allowed and one of 6 lines is refused', async ($, on) => {
  const ran = engine(on)
  const five = await edit($, SRC, lines(5), lines(5))
  expect(five.deny).toBeUndefined()
  const six = await edit($, SRC, lines(6), lines(6))
  expect(six.deny).toContain('anthology:implementer')
  expect(ran).toEqual([`Edit ${SRC}`])
})

test('either side over 5 lines refuses the Edit', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, 'a', lines(6))).deny).toContain('anthology:')
  expect((await edit($, SRC, lines(6), 'a')).deny).toContain('anthology:')
})

test('a trailing newline does not count as a line', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, `${lines(5)}\n`, `${lines(5)}\n`)).deny).toBeUndefined()
})

test('the third Edit is allowed and the fourth is refused', async ($, on) => {
  const ran = engine(on)
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  const fourth = await edit($, SRC)
  expect(fourth.deny).toContain('anthology:implementer')
  expect(ran).toHaveLength(3)
})

test('a refused Edit does not use up the budget', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, lines(6), 'a')).deny).toBeDefined()
  expect((await edit($, SRC, 'a', 'b', { replace_all: true })).deny).toBeDefined()
  expect((await edit($, `${ROOT}/other.ts`)).deny).toBeUndefined()
})

test('an Edit of a second file in the same turn is refused', async ($, on) => {
  const ran = engine(on)
  expect((await edit($, SRC)).deny).toBeUndefined()
  const second = await edit($, `${ROOT}/src/other.ts`)
  expect(second.deny).toContain('anthology:implementer')
  // the first file is still open to it
  expect((await edit($, SRC)).deny).toBeUndefined()
  expect(ran).toHaveLength(2)
})

test('replace_all is never a tiny fix', async ($, on) => {
  engine(on)
  const r = await edit($, SRC, 'a', 'b', { replace_all: true })
  expect(r.deny).toContain('anthology:implementer')
})

test('a Write is never a tiny fix', async ($, on) => {
  engine(on)
  const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(r.deny).toContain('anthology:implementer')
  // and it did not touch the Edit budget
  expect((await edit($, SRC)).deny).toBeUndefined()
})

test('a tiny Edit on the same file with a different spelling of the path is one file', async ($, on) => {
  engine(on)
  expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, `${ROOT}/src/../src/app.ts`)).deny).toBeUndefined()
})

test('the budget resets at each turn', async ($, on) => {
  engine(on)
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toBeDefined()
  await newTurn($)
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toBeDefined()
})

test('a new turn also frees the file', async ($, on) => {
  engine(on)
  expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, `${ROOT}/src/other.ts`)).deny).toBeDefined()
  await newTurn($)
  expect((await edit($, `${ROOT}/src/other.ts`)).deny).toBeUndefined()
})

test('a subagent turn does not reset the lead budget', async ($, on) => {
  engine(on)
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  await newTurn($, 'a1')
  expect((await edit($, SRC)).deny).toBeDefined()
})

test('the turn.start hook passes the event on', async ($, on) => {
  engine(on)
  const r = await $.turn.start({ text: 'go', turnId: 'keep' })
  expect(r).toEqual({ turnId: 'keep' })
})

// An Edit is also capped at about 2 KB changed (old and new text together).
test('an Edit of 2048 characters is allowed and one of 2049 is refused', async ($, on) => {
  const ran = engine(on)
  expect((await edit($, SRC, 'a'.repeat(1024), 'b'.repeat(1024))).deny).toBeUndefined()
  const big = await edit($, SRC, 'a'.repeat(1024), 'b'.repeat(1025))
  expect(big.deny).toContain('2 KB')
  expect(ran).toHaveLength(1)
})

test('five long lines are over the size cap even though they are only five lines', async ($, on) => {
  engine(on)
  const wide = Array.from({ length: 5 }, () => 'x'.repeat(500)).join('\n')
  expect((await edit($, SRC, wide, wide)).deny).toContain('2 KB')
  const narrow = Array.from({ length: 5 }, () => 'x'.repeat(100)).join('\n')
  expect((await edit($, SRC, narrow, narrow)).deny).toBeUndefined()
})

// The refusal says why.
test('the refusal gives the reason: lines, size, second file, budget, replace_all, Write, NotebookEdit', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, lines(6), 'a')).deny).toContain('at most 5 lines and this one is 6')
  expect((await edit($, SRC, 'x'.repeat(3000), 'a')).deny).toContain('2 KB')
  expect((await edit($, SRC, 'a', 'b', { replace_all: true })).deny).toContain('replace_all is not an inline fix')
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('a Write is never an inline fix')
  const nb = await $.tool.call({ tool: 'NotebookEdit', notebook_path: `${ROOT}/n.ipynb`, new_source: 'x' })
  expect(nb.deny).toContain('a NotebookEdit is never an inline fix')
  expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, `${ROOT}/src/other.ts`)).deny).toContain(`already in ${SRC}`)
  expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toContain('already used its 3 inline edits')
})

// The budget is spent only by an edit that went through.
test('an Edit the tool reports as an error does not use the budget', async ($, on) => {
  const ran = engine(on, { broken: 'broken' })
  const bad = `${ROOT}/src/broken.ts`
  for (let i = 0; i < 4; i++) {
    const r = await edit($, bad)
    expect(r.deny).toBeUndefined()
    expect(r.isError).toBe(true)
  }
  // nothing was spent: three edits to another file still fit, a fourth does not
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toContain('anthology:')
  expect(ran).toHaveLength(7)
})

test('an Edit that a hook beneath refuses does not use the budget', async ($, on) => {
  engine(on, { denied: 'blocked' })
  for (let i = 0; i < 4; i++) {
    expect((await edit($, `${ROOT}/src/blocked.ts`)).deny).toBe('blocked beneath')
  }
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toContain('already used')
})

test('a failed Edit leaves no file lock behind', async ($, on) => {
  engine(on, { broken: 'app' })
  // the only edit of the turn failed and went back, so another file may go first
  expect((await edit($, SRC)).isError).toBe(true)
  expect((await edit($, `${ROOT}/src/other.ts`)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toContain('already in')
})

// Every hook fails open: when something it reads throws, the edit runs.
for (const failing of ['env.get', 'settings.read', 'tool.list', 'session.root'] as const) {
  test(`the Write and the Edit run when ${failing} throws`, async ($, on) => {
    const ran = engine(on, { fails: [failing] })
    const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
    expect(w.deny).toBeUndefined()
    const e = await edit($, SRC, lines(30), lines(30))
    expect(e.deny).toBeUndefined()
    expect(ran).toHaveLength(2)
  })
}

// The budget lives in `$.state`; only a tiny Edit reads it, so only that one has a state failure to survive.
for (const failing of ['state.get', 'state.set'] as const) {
  test(`a tiny Edit runs, every time, when ${failing} throws`, async ($, on) => {
    const ran = engine(on, { fails: [failing] })
    for (let i = 0; i < 5; i++) expect((await edit($, SRC)).deny).toBeUndefined()
    expect(ran).toHaveLength(5)
  })
}

test('the turn still starts when the budget cannot be reset', async ($, on) => {
  engine(on, { fails: ['state.set'] })
  expect(await $.turn.start({ text: 'go', turnId: 'kept' })).toEqual({ turnId: 'kept' })
})

test('compose returns the engine sections when env.get throws', async ($, on) => {
  engine(on, { fails: ['env.get'] })
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro'])
})

test('compose still adds the section when the state cannot be written', async ($, on) => {
  engine(on, { fails: ['state.set'] })
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro', 'anthology:coordinator'])
})

// The refusal carries the short rule itself unless prompt.compose put the section in the prompt.
test('a refusal carries the rule when no section was composed', async ($, on) => {
  engine(on)
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('Coordinator: you lead')
})

test('a refusal carries the rule after a bare or teammate prompt', async ($, on) => {
  engine(on)
  await compose($)
  await compose($, { traits: ['bare'] })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('Coordinator: you lead')
})

test('a refusal leaves the rule out when the section is in the prompt', async ($, on) => {
  engine(on)
  await compose($)
  const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(r.deny).toContain('anthology:implementer')
  expect(r.deny).not.toContain('Coordinator: you lead')
})

// The coordinator section.
const compose = ($: Eng, args: { tools?: readonly string[]; surfaces?: readonly RenderSurface[]; traits?: readonly PromptComposeTrait[] } = {}) =>
  $.prompt.compose({
    model: 'm',
    promptModel: 'm',
    outputStyle: null,
    tools: ['Agent', 'Read', 'Edit'],
    surfaces: ['terminal'],
    traits: [],
    ...args,
  })

test('compose adds one session-scoped coordinator section when the lead has the Agent tool', async ($, on) => {
  engine(on)
  const r = await compose($)
  expect(r.sections.map(s => s.id)).toEqual(['intro', 'anthology:coordinator'])
  const s = r.sections.find(x => x.id === 'anthology:coordinator')
  if (!s) throw new Error('no coordinator section')
  expect(s.scope).toBe('session')
  expect(s.text.length).toBeLessThanOrEqual(400)
  for (const word of [
    'Plan, brief agents, check their work and merge',
    'you do not make',
    'anthology:implementer',
    'anthology:debugger',
    'anthology:docs-writer',
    'anthology:ui-designer',
    'a fix of at most a few lines in one file that the user asked for',
    '5 lines',
    '3 edits',
  ]) {
    expect(s.text).toContain(word)
  }
})

test('compose keeps the engine sections as they were', async ($, on) => {
  engine(on)
  const r = await compose($)
  expect(r.sections[0]).toEqual({ id: 'intro', text: 'You are Claude Code.', scope: 'shared' })
})

test('compose adds nothing without the Agent tool', async ($, on) => {
  engine(on)
  const r = await compose($, { tools: ['Read', 'Edit'] })
  expect(r.sections.map(s => s.id)).toEqual(['intro'])
})

test('compose adds nothing in a headless run', async ($, on) => {
  engine(on)
  const r = await compose($, { surfaces: [] })
  expect(r.sections.map(s => s.id)).toEqual(['intro'])
})

test('compose adds nothing for a bare or teammate prompt', async ($, on) => {
  engine(on)
  for (const trait of ['bare', 'teammate'] as const) {
    const r = await compose($, { traits: [trait] })
    expect(r.sections.map(s => s.id)).toEqual(['intro'])
  }
})

test('compose adds nothing when the mod is off', async ($, on) => {
  engine(on, { env: { ANTHOLOGY_DELEGATION: '0' } })
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro'])
})

test('compose adds nothing when every anthology mod is off', async ($, on) => {
  engine(on, { env: { ANTHOLOGY_MODS: '0' } })
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro'])
})

test('compose does not add the section twice', async ($, on) => {
  engine(on, { sections: [{ id: 'anthology:coordinator', text: 'already', scope: 'session' }] })
  const r = await compose($)
  expect(r.sections.filter(s => s.id === 'anthology:coordinator')).toHaveLength(1)
})
