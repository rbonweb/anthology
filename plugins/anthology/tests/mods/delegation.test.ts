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
}

/** Stands for Claude Code beneath the plugin. Returns what the tools were asked to do. */
function engine(on: On, world: World = {}): string[] {
  const ran: string[] = []
  const env = { HOME, ...world.env } as { readonly [name: string]: string }
  on('session.surfaces', () => ({ value: world.surfaces ?? ['terminal'] }))
  on('session.root', () => ({ value: ROOT }))
  on('env.get', (_$, e) => ({ value: env[e.name] }))
  on('turn.start', (_$, e) => ({ turnId: e.turnId }))
  on('tool.call', (_$, e) => {
    ran.push(`${e.tool} ${String(e.file_path ?? e.notebook_path)}`)
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

test('the desktop surface counts as interactive', async ($, on) => {
  engine(on, { surfaces: ['desktop'] })
  const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(r.deny).toContain('anthology:implementer')
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
]

for (const path of EXEMPT) {
  test(`the lead may Write ${path}`, async ($, on) => {
    const ran = engine(on)
    const w = await $.tool.call({ tool: 'Write', file_path: path, content: lines(40) })
    expect(w.deny).toBeUndefined()
    expect(ran).toEqual([`Write ${path}`])
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
  ]) {
    const w = await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })
    expect(w.deny).toContain('anthology:')
  }
})

test('CLAUDE_CONFIG_DIR moves the memory and plans folders', async ($, on) => {
  engine(on, { env: { CLAUDE_CONFIG_DIR: '/cfg' } })
  const mem = await $.tool.call({ tool: 'Write', file_path: '/cfg/projects/p/memory/a.md', content: 'x' })
  expect(mem.deny).toBeUndefined()
  const plan = await $.tool.call({ tool: 'Write', file_path: '/cfg/plans/a.md', content: 'x' })
  expect(plan.deny).toBeUndefined()
  const old = await $.tool.call({ tool: 'Write', file_path: `${HOME}/.claude/plans/a.md`, content: 'x' })
  expect(old.deny).toContain('anthology:')
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
  for (const word of ['plan', 'merge', 'anthology:implementer', 'anthology:debugger', 'anthology:docs-writer', 'anthology:ui-designer', '5 lines', '3 edits']) {
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
