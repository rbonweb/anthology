// delegation.ts (post-release mods design §3): in an interactive session the lead's own Edit,
// Write and NotebookEdit are refused, with a few exceptions, and the system prompt gets a short
// coordinator section when the session has the Agent tool. The test's own hooks sit beneath the
// plugin and stand for Claude Code: `tool.call` is the tool itself (it records what ran),
// `session.*` and `env.get` are the session, `prompt.compose` is the engine's own composition.
import type { On, PromptComposeSection, PromptComposeTrait, RenderSurface } from 'claude-code'
import { type Engine, expect, test } from 'claude-code/testing'

const HOME = '/home/dev'
const ROOT = '/work/repo'
const LOCK = '{"version": "0.1.1", "tag": "anthology-bin--v0.1.1", "repo": "rbonweb/anthology", "sha256": {}}'

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
  fails?: readonly ('env.get' | 'state.get' | 'state.set' | 'settings.read' | 'tool.list' | 'session.root' | 'fs.exists' | 'fs.read')[]
  /** an Edit of a path holding this text comes back from the tool as an error */
  broken?: string
  /** a tool.call of a path holding this text is refused beneath the plugin (another plugin's deny) */
  denied?: string
  /** the folders that hold a `.git`; unset, every folder does, so every path is inside a repo */
  repos?: readonly string[]
  /** the text of `<root>/.claude/anthology.json`; unset, there is no such file */
  repoConfig?: string
  /**
   * What the installed `anthology switch delegation <cwd>` prints (`off`, `ignored` or `on`);
   * unset, no binary is installed
   */
  switchAnswer?: string
  /**
   * Puts a `git` at /usr/bin/git on the PATH and lets it answer `check-ignore` and `ls-files`;
   * unset, there is no git on the PATH
   */
  git?: {
    /** absolute paths that `git check-ignore` reports as ignored, tracked or not (stricter than git) */
    ignored?: readonly string[]
    /** absolute paths that are in the index */
    tracked?: readonly string[]
    /** exit code of every git call instead of the answers above (git's own failure is 128) */
    exit?: number
    /** the git call cannot start */
    cannotStart?: boolean
  }
  /** absolute paths that are symbolic links */
  links?: readonly string[]
}

const boom = (): never => {
  throw new Error('boom')
}

/** Stands for Claude Code beneath the plugin. Returns what the tools were asked to do. */
function engine(on: On, world: World = {}): string[] {
  const ran: string[] = []
  const env = { HOME, ...(world.git ? { PATH: '/usr/bin' } : {}), ...world.env } as { readonly [name: string]: string }
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
  // a folder holds a `.git` when it is listed in `repos` (a file in a worktree answers the same)
  on(
    'fs.exists',
    fails.has('fs.exists')
      ? boom
      : (_$, e) => ({
          value: e.path.endsWith('/.git') && (world.repos === undefined || world.repos.includes(e.path.slice(0, -'/.git'.length))),
        }),
  )
  on(
    'fs.read',
    fails.has('fs.read')
      ? boom
      : (_$, e) => {
          if (e.path === `${ROOT}/.claude/anthology.json` && world.repoConfig !== undefined) return { value: world.repoConfig }
          if (e.path.endsWith('/anthology.lock') && world.switchAnswer !== undefined) return { value: LOCK }
          throw new Error(`ENOENT: ${e.path}`)
        },
  )
  // the installed binary, as the shim finds it, when the world answers `anthology switch`
  on('fs.stat', (_$, e) => {
    if (world.git && e.path === '/usr/bin/git') return { value: { kind: 'file' as const, size: 100, mtimeMs: 0, isLink: false } }
    if (world.links?.includes(e.path)) return { value: { kind: 'file' as const, size: 0, mtimeMs: 0, isLink: true } }
    const ah = `${HOME}/.claude/anthology`
    const known: { [path: string]: { kind: 'dir' | 'file'; size: number } } = {
      [`${ah}/bin`]: { kind: 'dir', size: 0 },
      [`${ah}/bin/0.1.1`]: { kind: 'dir', size: 0 },
      [`${ah}/bin/0.1.1/anthology`]: { kind: 'file', size: 100 },
      [`${ah}/bin/0.1.1/.sha256`]: { kind: 'file', size: 65 },
    }
    const s = world.switchAnswer === undefined ? undefined : known[e.path]
    if (s === undefined) throw new Error(`ENOENT: ${e.path}`)
    return { value: { ...s, mtimeMs: 0, isLink: false } }
  })
  on('process.run', (_$, e) => {
    if (world.git && e.argv[0] === '/usr/bin/git') {
      const sub = e.argv.findIndex(a => a === 'check-ignore' || a === 'ls-files')
      const path = e.argv[e.argv.length - 1] ?? ''
      ran.push(`git ${e.init?.cwd ?? ''}: ${e.argv.slice(sub).join(' ')}`)
      if (world.git.cannotStart) throw new Error('spawn git ENOENT')
      const answer = (exitCode: number) => ({ value: { exitCode: world.git?.exit ?? exitCode, stdout: '', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } })
      // check-ignore: 0 ignored, 1 not. ls-files --error-unmatch: 0 in the index, 1 not.
      if (e.argv[sub] === 'check-ignore') return answer(world.git.ignored?.includes(path) ? 0 : 1)
      return answer(world.git.tracked?.includes(path) ? 0 : 1)
    }
    ran.push(`run ${e.argv.slice(1).join(' ')}`)
    return { value: { exitCode: 0, stdout: `${world.switchAnswer}\n`, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
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

// The host rule as a table: [entrypoint, attended, mode, embedded]; `undefined` is an unset variable.
// The attended flag decides when it is `0` or `1`; any other value leaves it to an `sdk-` entrypoint;
// mode `full` wins. The guard (`host::embedded`) and the superpowers session-start hook hold the same
// rule, and crates/repo-tests/tests/host_predicate.rs reads these rows to run both against them.
// host-truth-table:begin
const HOST_TABLE: [string | undefined, string | undefined, string | undefined, boolean][] = [
  ['claude-vscode', '1', undefined, false],
  ['cli', '1', undefined, false],
  ['sdk-ts', '1', undefined, false],
  ['sdk-cli', '1', undefined, false],
  ['sdk-', '1', undefined, false],
  ['sdk-ts', '0', undefined, true],
  ['sdk-cli', '0', undefined, true],
  ['hostapp', '0', undefined, true],
  ['cli', '0', undefined, true],
  [undefined, '0', undefined, true],
  ['hostapp', undefined, undefined, false],
  ['hostapp', '1', undefined, false],
  ['sdk-cli', undefined, undefined, true],
  ['sdk-', undefined, undefined, true],
  ['sdk-cli', '', undefined, true],
  ['sdk-cli', '00', undefined, true],
  ['sdk-cli', 'true', undefined, true],
  ['hostapp', '', undefined, false],
  ['hostapp', '00', undefined, false],
  ['hostapp', 'false', undefined, false],
  ['hostapp', 'true', undefined, false],
  ['cli', undefined, undefined, false],
  ['cli', '', undefined, false],
  ['cli', 'true', undefined, false],
  ['claude-vscode', undefined, undefined, false],
  ['claude-vscode', '', undefined, false],
  ['claude-vscode', 'true', undefined, false],
  ['SDK-cli', undefined, undefined, false],
  ['xsdk-cli', undefined, undefined, false],
  ['sdk', undefined, undefined, false],
  ['', undefined, undefined, false],
  ['sdk-ts', '0', 'full', false],
  ['sdk-cli', undefined, 'full', false],
  ['sdk-cli', '1', 'full', false],
  ['hostapp', '0', 'full', false],
  ['sdk-ts', '0', 'lean', true],
  ['sdk-ts', '0', '', true],
  ['sdk-ts', '0', 'FULL', true],
  ['sdk-ts', '0', 'full ', true],
  [undefined, undefined, 'lean', false],
  [undefined, undefined, undefined, false],
]
// host-truth-table:end

for (const [entrypoint, attended, mode, embedded] of HOST_TABLE) {
  const label = JSON.stringify({ entrypoint, attended, mode })
  test(`host rule ${label}: ${embedded ? 'embedded, not refused' : 'a person, refused'}`, async ($, on) => {
    const env: { [name: string]: string } = {}
    if (entrypoint !== undefined) env.CLAUDE_CODE_ENTRYPOINT = entrypoint
    if (attended !== undefined) env.CLAUDE_CODE_SESSION_ATTENDED = attended
    if (mode !== undefined) env.ANTHOLOGY_HOST_MODE = mode
    engine(on, { env })
    const w = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
    if (embedded) expect(w.deny).toBeUndefined()
    else expect(w.deny).toContain('anthology:implementer')
  })
}

test('an interactive claude that inherited an sdk entrypoint is still refused', async ($, on) => {
  const ran = engine(on, { env: { CLAUDE_CODE_ENTRYPOINT: 'sdk-ts', CLAUDE_CODE_SESSION_ATTENDED: '1' } })
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
    `${ROOT}/.claude/memory/../settings.json`,
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
    'relative/.superpowers/a.md',
    // look-alikes of the memory folders
    `${ROOT}/.claude/memory-old/a.md`,
    `${ROOT}/.claude/memoryx.md`,
    `${ROOT}/.claude/agent-memory/a.md`,
    `${ROOT}/.claude/settings.json`,
    `${ROOT}/src/.claude/memory/a.md`,
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
  for (const p of [`${HOME}/.claude/plans/a.md`, `${HOME}/.claude/anthology/personal-memory/a.md`]) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toContain('anthology:')
  }
  // the whole config folder is the user's own, wherever it is (see the config-folder tests below)
  expect((await $.tool.call({ tool: 'Write', file_path: '/cfg/settings.json', content: 'x' })).deny).toBeUndefined()
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

test('adds no coordinator section in an SDK host', async ($, on) => {
  engine(on, { env: SDK_HOST })
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro'])
})

test('still adds it in claude-vscode attended', async ($, on) => {
  engine(on, { env: { CLAUDE_CODE_ENTRYPOINT: 'claude-vscode', CLAUDE_CODE_SESSION_ATTENDED: '1' } })
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro', 'anthology:coordinator'])
})

test('compose does not add the section twice', async ($, on) => {
  engine(on, { sections: [{ id: 'anthology:coordinator', text: 'already', scope: 'session' }] })
  const r = await compose($)
  expect(r.sections.filter(s => s.id === 'anthology:coordinator')).toHaveLength(1)
})

// --- the project's own agents come first ----------------------------------------------------------

test('the coordinator section sends the change to the project own agent when one fits', async ($, on) => {
  engine(on)
  const s = (await compose($)).sections.find(x => x.id === 'anthology:coordinator')
  if (!s) throw new Error('no coordinator section')
  expect(s.text).toContain("the project's own when one fits, else anthology:implementer")
  expect(s.text.length).toBeLessThanOrEqual(400)
})

test('the refusal names the project own agent first and the anthology one as the fallback', async ($, on) => {
  engine(on)
  const code = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(code.deny).toContain(
    "Dispatch the project's own matching agent when one fits, else anthology:implementer (anthology:debugger for a bug fix), with this change.",
  )
  const md = await $.tool.call({ tool: 'Write', file_path: `${ROOT}/docs/a.md`, content: 'x' })
  expect(md.deny).toContain("Dispatch the project's own matching agent when one fits, else anthology:docs-writer, with this change.")
  const ui = await $.tool.call({ tool: 'Write', file_path: `${ROOT}/web/a.css`, content: 'x' })
  expect(ui.deny).toContain("Dispatch the project's own matching agent when one fits, else anthology:ui-designer, with this change.")
})

test('a dispatch to any agent is never refused or redirected', async ($, on) => {
  const ran = engine(on)
  const types = ['code-reviewer', 'general-purpose', 'anthology:implementer', 'shop:theme-builder', 'Explore']
  for (const subagent_type of types) {
    const r = await $.tool.call({ tool: 'Agent', subagent_type, description: 'a job', prompt: 'do it' })
    expect(r.deny).toBeUndefined()
  }
  // not even with the tiny-fix budget used up
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect((await edit($, SRC)).deny).toBeDefined()
  const after = await $.tool.call({ tool: 'Agent', subagent_type: 'code-reviewer', description: 'a job', prompt: 'do it' })
  expect(after.deny).toBeUndefined()
  expect(ran.filter(r => r.startsWith('Agent'))).toHaveLength(types.length + 1)
})

// --- files that are not project code ---------------------------------------------------------------

test('a file outside every git repo is the lead own: any size, no budget', async ($, on) => {
  const ran = engine(on, { repos: [ROOT] })
  expect((await $.tool.call({ tool: 'Write', file_path: '/srv/other/a.ts', content: lines(40) })).deny).toBeUndefined()
  expect((await edit($, '/srv/other/a.ts', lines(40), lines(41))).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'Write', file_path: '/tmp/scratch.json', content: 'x' })).deny).toBeUndefined()
  // none of that cost the tiny-fix budget
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect(ran).toHaveLength(6)
})

test('a file inside a git repo is still handed off, at any depth', async ($, on) => {
  engine(on, { repos: [ROOT] })
  for (const p of [SRC, `${ROOT}/a/b/c/d/e/f.rs`, `${ROOT}/x.md`]) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toContain('anthology:')
  }
  expect((await edit($, SRC, lines(6), lines(6))).deny).toContain('anthology:implementer')
})

test('a repo nested inside another is a repo of its own', async ($, on) => {
  engine(on, { repos: ['/work/outer', '/work/outer/inner'], cwd: '/work/outer' })
  expect((await $.tool.call({ tool: 'Write', file_path: '/work/outer/inner/a.ts', content: 'x' })).deny).toContain('anthology:')
  expect((await $.tool.call({ tool: 'Write', file_path: '/work/elsewhere/a.ts', content: 'x' })).deny).toBeUndefined()
})

test('a relative path is judged from the session directory', async ($, on) => {
  engine(on, { repos: [ROOT], cwd: '/srv/scratch' })
  expect((await $.tool.call({ tool: 'Write', file_path: 'notes.ts', content: 'x' })).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'Write', file_path: `${ROOT}/src/a.ts`, content: 'x' })).deny).toContain('anthology:')
})

test('the Claude config folder is the lead own, even when it sits inside a repo', async ($, on) => {
  // a dotfiles repo at the home folder holds ~/.claude, the working repo is elsewhere
  const ran = engine(on, { repos: [HOME, ROOT] })
  for (const p of [`${HOME}/.claude/settings.json`, `${HOME}/.claude/agents/mine.md`, '~/.claude/keybindings.json']) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: lines(30) })).deny).toBeUndefined()
  }
  // the home folder outside .claude is a repo like any other
  expect((await $.tool.call({ tool: 'Write', file_path: `${HOME}/dotfiles/a.ts`, content: 'x' })).deny).toContain('anthology:')
  expect(ran).toHaveLength(3)
})

test('CLAUDE_CONFIG_DIR moves the config folder', async ($, on) => {
  engine(on, { env: { CLAUDE_CONFIG_DIR: '/cfg' } })
  expect((await $.tool.call({ tool: 'Write', file_path: '/cfg/settings.json', content: 'x' })).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'Write', file_path: `${HOME}/.claude/settings.json`, content: 'x' })).deny).toContain('anthology:')
  expect((await $.tool.call({ tool: 'Write', file_path: '/cfg/../src/a.ts', content: 'x' })).deny).toContain('anthology:')
})

test('a path that climbs out of the config folder into a repo is refused', async ($, on) => {
  engine(on, { repos: [ROOT] })
  expect((await $.tool.call({ tool: 'Write', file_path: `${HOME}/.claude/../../../work/repo/src/a.ts`, content: 'x' })).deny).toContain('anthology:')
})

test('when the repo check cannot run the file is treated as project code', async ($, on) => {
  const ran = engine(on, { fails: ['fs.exists'], repos: [] })
  expect((await $.tool.call({ tool: 'Write', file_path: '/srv/other/a.ts', content: 'x' })).deny).toContain('anthology:')
  expect(ran).toHaveLength(0)
})

// --- local setup: a file that is gitignored and untracked --------------------------------------------
// No agent can reach such a file (it exists only in the main checkout), so the lead makes it. Git
// decides, by `check-ignore` and the index, never a name pattern.

const LOCAL = `${ROOT}/.env.foo.local`
const gitCalls = (ran: string[]) => ran.filter(r => r.startsWith('git '))

test('the lead may Write, Edit and NotebookEdit a file that git ignores and does not track', async ($, on) => {
  const ran = engine(on, { repos: [ROOT], git: { ignored: [LOCAL, `${ROOT}/n.ipynb`] } })
  expect((await $.tool.call({ tool: 'Write', file_path: LOCAL, content: lines(40) })).deny).toBeUndefined()
  expect((await edit($, LOCAL, lines(30), lines(31))).deny).toBeUndefined()
  expect((await $.tool.call({ tool: 'NotebookEdit', notebook_path: `${ROOT}/n.ipynb`, new_source: 'x' })).deny).toBeUndefined()
  expect(ran.filter(r => !r.startsWith('git '))).toEqual([`Write ${LOCAL}`, `Edit ${LOCAL}`, `NotebookEdit ${ROOT}/n.ipynb`])
  // none of that cost the tiny-fix budget
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
})

test('git is asked from the repo top, about the absolute path, with the path after --', async ($, on) => {
  const ran = engine(on, { repos: [ROOT], cwd: `${ROOT}/pkg`, git: { ignored: [LOCAL] } })
  expect((await $.tool.call({ tool: 'Write', file_path: '../.env.foo.local', content: 'x' })).deny).toBeUndefined()
  expect(gitCalls(ran)).toEqual([
    `git ${ROOT}: check-ignore -q -- ${LOCAL}`,
    `git ${ROOT}: ls-files --error-unmatch -- ${LOCAL}`,
  ])
})

test('a file in a nested repo is asked about in that repo', async ($, on) => {
  const inner = `${ROOT}/inner/.env.local`
  const ran = engine(on, { repos: [ROOT, `${ROOT}/inner`], git: { ignored: [inner] } })
  expect((await $.tool.call({ tool: 'Write', file_path: inner, content: 'x' })).deny).toBeUndefined()
  expect(gitCalls(ran)[0]).toBe(`git ${ROOT}/inner: check-ignore -q -- ${inner}`)
})

test('a file that is not ignored is still handed off, whatever its name', async ($, on) => {
  const ran = engine(on, { repos: [ROOT], git: { ignored: [] } })
  for (const p of [LOCAL, `${ROOT}/.env.local`, `${ROOT}/src/app.ts`]) {
    expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toContain('anthology:')
  }
  expect((await edit($, LOCAL, lines(6), lines(6))).deny).toContain('anthology:implementer')
  expect(ran.filter(r => !r.startsWith('git '))).toHaveLength(0)
})

test('an ignored file that is tracked is still handed off', async ($, on) => {
  const ran = engine(on, { repos: [ROOT], git: { ignored: [LOCAL], tracked: [LOCAL] } })
  expect((await $.tool.call({ tool: 'Write', file_path: LOCAL, content: 'x' })).deny).toContain('anthology:')
  expect((await edit($, LOCAL, lines(6), lines(6))).deny).toContain('anthology:implementer')
  expect(ran.filter(r => !r.startsWith('git '))).toHaveLength(0)
})

test('nothing under .git is ever the lead own, ignored or not', async ($, on) => {
  const inside = [
    `${ROOT}/.git/config`,
    `${ROOT}/.git/hooks/pre-commit`,
    `${ROOT}/.git`,
    `${ROOT}/inner/.git/config`,
    `${ROOT}/src/../.git/config`,
    `${ROOT}/.GIT/config`,
    '.git/info/exclude',
  ]
  const ran = engine(on, { repos: [ROOT], git: { ignored: inside } })
  for (const p of inside) expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny, p).toContain('anthology:')
  // git was not even asked
  expect(ran).toHaveLength(0)
})

test('a name that only looks like .git is judged by git', async ($, on) => {
  const p = `${ROOT}/.gitlocal/.env`
  engine(on, { repos: [ROOT], git: { ignored: [p] } })
  expect((await $.tool.call({ tool: 'Write', file_path: p, content: 'x' })).deny).toBeUndefined()
})

// git's own failure is 128; a run that cannot start or is killed at the timeout rejects
for (const git of [{ ignored: [LOCAL], exit: 128 }, { ignored: [LOCAL], exit: 2 }, { ignored: [LOCAL], cannotStart: true }]) {
  test(`a git that answers ${JSON.stringify(git)} leaves the refusal`, async ($, on) => {
    const ran = engine(on, { repos: [ROOT], git })
    expect((await $.tool.call({ tool: 'Write', file_path: LOCAL, content: 'x' })).deny).toContain('anthology:')
    expect(ran.filter(r => !r.startsWith('git '))).toHaveLength(0)
  })
}

test('with no git on the PATH the refusal stands, and nothing is spawned', async ($, on) => {
  const ran = engine(on, { repos: [ROOT] })
  expect((await $.tool.call({ tool: 'Write', file_path: LOCAL, content: 'x' })).deny).toContain('anthology:')
  expect(ran).toHaveLength(0)
})

test('a symbolic link is handed off even when git ignores it: a write goes through it', async ($, on) => {
  const ran = engine(on, { repos: [ROOT], git: { ignored: [LOCAL] }, links: [LOCAL] })
  expect((await $.tool.call({ tool: 'Write', file_path: LOCAL, content: 'x' })).deny).toContain('anthology:')
  expect(ran.filter(r => !r.startsWith('git '))).toHaveLength(0)
})

test('when the repo check cannot run, git is not asked and the refusal stands', async ($, on) => {
  const ran = engine(on, { fails: ['fs.exists'], repos: [], git: { ignored: [LOCAL] } })
  expect((await $.tool.call({ tool: 'Write', file_path: LOCAL, content: 'x' })).deny).toContain('anthology:')
  expect(ran).toHaveLength(0)
})

test('the refusal says that ignored, untracked files are the lead own', async ($, on) => {
  engine(on, { repos: [ROOT] })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('gitignored and untracked')
})

// --- risk, not only size ---------------------------------------------------------------------------

test('a pure deletion of up to 30 lines is a tiny fix, one of 31 is not', async ($, on) => {
  const ran = engine(on)
  expect((await edit($, SRC, lines(30), '')).deny).toBeUndefined()
  const over = await edit($, SRC, lines(31), '')
  expect(over.deny).toContain('a deletion is at most 30 lines and this one is 31')
  expect(ran).toEqual([`Edit ${SRC}`])
})

test('the 14-line deletion from the user settings goes through', async ($, on) => {
  const ran = engine(on, { repos: [HOME] })
  expect((await edit($, `${HOME}/.claude/settings.json`, lines(14), '')).deny).toBeUndefined()
  expect(ran).toHaveLength(1)
})

test('a deletion is capped by size as well: 4 KB', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, 'a'.repeat(4096), '')).deny).toBeUndefined()
  expect((await edit($, SRC, 'a'.repeat(4097), '')).deny).toContain('a deletion is at most about 4 KB')
})

test('a deletion counts toward the three edits of the turn and the one-file rule', async ($, on) => {
  engine(on)
  for (let i = 0; i < 3; i++) expect((await edit($, SRC, lines(20), '')).deny).toBeUndefined()
  expect((await edit($, SRC, lines(20), '')).deny).toContain('already used its 3 inline edits')
  await newTurn($)
  expect((await edit($, SRC, lines(20), '')).deny).toBeUndefined()
  expect((await edit($, `${ROOT}/src/other.ts`, lines(20), '')).deny).toContain(`already in ${SRC}`)
})

test('only an empty new_string is a deletion: blank text is a replacement', async ($, on) => {
  engine(on)
  for (const blank of [' ', '\n', '  \n  ', '\t']) {
    expect((await edit($, SRC, lines(6), blank)).deny, JSON.stringify(blank)).toContain('an inline fix is at most 5 lines and this one is 6')
  }
  expect((await edit($, SRC, lines(6), '')).deny).toBeUndefined()
})

test('replacing text is not a deletion, even when the new text is short', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, lines(6), 'x')).deny).toContain('an inline fix is at most 5 lines and this one is 6')
  expect((await edit($, SRC, '', lines(6))).deny).toContain('an inline fix is at most 5 lines')
  // an empty replacement of nothing is no deletion either
  expect((await edit($, SRC, '', '')).deny).toBeUndefined()
})

test('a deletion by replace_all is still refused', async ($, on) => {
  engine(on)
  expect((await edit($, SRC, 'a', '', { replace_all: true })).deny).toContain('replace_all is not an inline fix')
})

test('a small config edit of up to 15 lines is a tiny fix, a code edit is still 5', async ($, on) => {
  engine(on)
  for (const f of ['package.json', 'cfg/app.yaml', 'Cargo.toml', 'tsconfig.jsonc', 'a.ini', 'x.conf', 'my.env']) {
    expect((await edit($, `${ROOT}/${f}`, lines(15), lines(15))).deny).toBeUndefined()
    await newTurn($)
  }
  expect((await edit($, `${ROOT}/package.json`, lines(16), lines(16))).deny).toContain('a config edit is at most 15 lines and this one is 16')
  // CI pipelines run code: they keep the code limit
  for (const f of [
    '.github/workflows/ci.yml',
    '.github/ci.yml',
    '.gitlab-ci.yml',
    'sub/.gitlab-ci.yaml',
    '.circleci/config.yml',
    'azure-pipelines.yml',
    '.travis.yml',
    'bitbucket-pipelines.yml',
  ]) {
    expect((await edit($, `${ROOT}/${f}`, lines(6), lines(6))).deny, f).toContain('an inline fix is at most 5 lines and this one is 6')
  }
  for (const f of ['src/a.ts', 'a.rs', 'a.py', 'README.md', 'a.json.bak', 'Makefile']) {
    const r = await edit($, `${ROOT}/${f}`, lines(6), lines(6))
    expect(r.deny).toContain('an inline fix is at most 5 lines and this one is 6')
  }
})

test('a config edit keeps the 2 KB size cap and the budget', async ($, on) => {
  engine(on)
  expect((await edit($, `${ROOT}/package.json`, 'a'.repeat(1025), 'b'.repeat(1024))).deny).toContain('2 KB')
  for (let i = 0; i < 3; i++) expect((await edit($, `${ROOT}/package.json`, lines(10), lines(10))).deny).toBeUndefined()
  expect((await edit($, `${ROOT}/package.json`, lines(10), lines(10))).deny).toContain('already used')
})

test('a Write of a config file is still never a tiny fix', async ($, on) => {
  engine(on)
  expect((await $.tool.call({ tool: 'Write', file_path: `${ROOT}/package.json`, content: 'x' })).deny).toContain('a Write is never an inline fix')
})

// --- a repo's own switch ----------------------------------------------------------------------------

test('an approved repo can turn the mod off in .claude/anthology.json', async ($, on) => {
  const ran = engine(on, { repoConfig: '{"delegation": false}', switchAnswer: 'off' })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: lines(40) })).deny).toBeUndefined()
  expect((await edit($, SRC, lines(30), lines(30))).deny).toBeUndefined()
  // the installed binary was asked, with the session folder
  expect(ran.filter(r => r.startsWith('run '))).toContain(`run switch delegation ${ROOT}`)
  // and the prompt carries no coordinator section
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro'])
})

test('a repo that is not approved cannot turn the mod off: the file is ignored', async ($, on) => {
  const ran = engine(on, { repoConfig: '{"delegation": false}', switchAnswer: 'ignored' })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('anthology:implementer')
  expect((await compose($)).sections.map(s => s.id)).toEqual(['intro', 'anthology:coordinator'])
  expect(ran.some(r => r.startsWith('run switch'))).toBe(true)
})

for (const answer of ['on', '', 'OFF', 'off please', 'offline']) {
  test(`an answer of ${JSON.stringify(answer)} from the binary leaves the mod on`, async ($, on) => {
    engine(on, { repoConfig: '{"delegation": false}', switchAnswer: answer })
    expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('anthology:implementer')
  })
}

test('with no installed binary the file cannot turn the mod off, and nothing is spawned', async ($, on) => {
  const ran = engine(on, { repoConfig: '{"delegation": false}' })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('anthology:implementer')
  expect(ran.some(r => r.startsWith('run '))).toBe(false)
})

test('the binary is asked only when the file asks', async ($, on) => {
  const ran = engine(on, { repoConfig: '{"routing_guide": false}', switchAnswer: 'off' })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('anthology:implementer')
  expect(ran.some(r => r.startsWith('run '))).toBe(false)
})

for (const repoConfig of ['{"delegation": true}', '{"delegation": "off"}', '{"delegation": 0}', '{"routing_guide": false}', '{}', 'not json', '[]', 'null', '']) {
  test(`the repo file ${JSON.stringify(repoConfig)} leaves the mod on`, async ($, on) => {
    engine(on, { repoConfig, switchAnswer: 'off' })
    const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
    expect(r.deny).toContain('anthology:implementer')
  })
}

test('without the repo file, or when it cannot be read, the mod stays on', async ($, on) => {
  engine(on, { fails: ['fs.read'] })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toContain('anthology:implementer')
})

test('a session started in a subfolder reads the file at the git top, as session start does', async ($, on) => {
  const ran = engine(on, { repoConfig: '{"delegation": false}', switchAnswer: 'off', repos: [ROOT], cwd: `${ROOT}/pkg/deep` })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })).deny).toBeUndefined()
  // the binary gets the session folder and finds the same top by itself
  expect(ran).toContain(`run switch delegation ${ROOT}/pkg/deep`)
})

test('a subfolder session without the file at the git top is not switched off', async ($, on) => {
  engine(on, { repoConfig: '{"delegation": false}', switchAnswer: 'off', repos: ['/elsewhere'], cwd: '/elsewhere/pkg' })
  expect((await $.tool.call({ tool: 'Write', file_path: '/elsewhere/src/a.ts', content: 'x' })).deny).toContain('anthology:implementer')
})

test('a repo that leaves the setting out is still refused, and the refusal says how to switch off', async ($, on) => {
  engine(on)
  const r = await $.tool.call({ tool: 'Write', file_path: SRC, content: 'x' })
  expect(r.deny).toContain('ANTHOLOGY_DELEGATION=0')
  expect(r.deny).toContain('"delegation": false in .claude/anthology.json')
})

// The mask marker: masking.ts replaces a secret in Bash output with `[masked by anthology: <kind>]`,
// and Claude must never write that text into a file. The refusal sits in the edit hook, ahead of
// every delegation rule, so it holds for a subagent, a headless run, the lead's own notes and a
// session with delegation off.
const MARKER_DENY =
  'Blocked by anthology: this edit contains a masked secret placeholder; write the real value from its source (env file or ~/.config/<project>/<service>.env), never the marker.'
const MARKED = 'GITHUB_TOKEN=[masked by anthology: github-token]'

test('a Write, Edit, MultiEdit or NotebookEdit that carries the mask marker is refused', async ($, on) => {
  const ran = engine(on)
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: `x\n${MARKED}\n` })).deny).toBe(MARKER_DENY)
  expect((await edit($, SRC, 'a', MARKED)).deny).toBe(MARKER_DENY)
  const multi = await $.tool.call({
    tool: 'MultiEdit',
    file_path: SRC,
    edits: [
      { old_string: 'a', new_string: 'b' },
      { old_string: 'c', new_string: MARKED },
    ],
  })
  expect(multi.deny).toBe(MARKER_DENY)
  const notebook = await $.tool.call({ tool: 'NotebookEdit', notebook_path: `${ROOT}/n.ipynb`, new_source: MARKED })
  expect(notebook.deny).toBe(MARKER_DENY)
  expect(ran).toHaveLength(0)
})

test('the marker is refused for a subagent, a headless run, the lead\'s notes and a repo with delegation off', async ($, on) => {
  const ran = engine(on, { surfaces: [], env: { ANTHOLOGY_DELEGATION: '0' } })
  const sub = await $.tool.call({ tool: 'Write', file_path: SRC, content: MARKED, agentId: 'a1' })
  expect(sub.deny).toBe(MARKER_DENY)
  const headless = await $.tool.call({ tool: 'Write', file_path: SRC, content: MARKED })
  expect(headless.deny).toBe(MARKER_DENY)
  const note = await $.tool.call({ tool: 'Write', file_path: `${ROOT}/.superpowers/plan.md`, content: MARKED })
  expect(note.deny).toBe(MARKER_DENY)
  expect(ran).toHaveLength(0)
})

test('a refused marker spends none of the turn\'s inline edits', async ($, on) => {
  const ran = engine(on)
  await newTurn($)
  for (let i = 0; i < 4; i++) expect((await edit($, SRC, 'a', MARKED)).deny).toBe(MARKER_DENY)
  for (let i = 0; i < 3; i++) expect((await edit($, SRC)).deny).toBeUndefined()
  expect(ran).toHaveLength(3)
})

test('text that only mentions masking is not refused', async ($, on) => {
  const ran = engine(on, { surfaces: [] })
  for (const text of ['masked by anthology', '[masked by anthology', 'secrets are masked: see docs', 'masked']) {
    expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: text })).deny).toBeUndefined()
    expect((await edit($, SRC, MARKED, text)).deny).toBeUndefined()
  }
  // Only the new text counts: an edit that removes a marker is fine.
  expect((await edit($, SRC, MARKED, 'GITHUB_TOKEN=real')).deny).toBeUndefined()
  expect(ran).toHaveLength(9)
})

test('a MultiEdit without the marker is passed on, as before', async ($, on) => {
  const ran = engine(on)
  const r = await $.tool.call({ tool: 'MultiEdit', file_path: SRC, edits: [{ old_string: 'a', new_string: 'b' }] })
  expect(r.deny).toBeUndefined()
  expect(ran).toEqual([`MultiEdit ${SRC}`])
})

const MARKER_OFF: { readonly [name: string]: string }[] = [{ ANTHOLOGY_SECRET_MASK: '0' }, { ANTHOLOGY_MODS: '0' }]
for (const env of MARKER_OFF) {
  test(`the marker is allowed with ${JSON.stringify(env)}`, async ($, on) => {
    const ran = engine(on, { surfaces: [], env })
    expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: MARKED })).deny).toBeUndefined()
    expect(ran).toEqual([`Write ${SRC}`])
  })
}

test('the marker check fails open when the session cannot be read', async ($, on) => {
  const ran = engine(on, { surfaces: [], fails: ['env.get'] })
  expect((await $.tool.call({ tool: 'Write', file_path: SRC, content: MARKED })).deny).toBeUndefined()
  expect(ran).toEqual([`Write ${SRC}`])
})
