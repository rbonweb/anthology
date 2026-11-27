// afterwork.ts (post-release mods design §4): on `agent.report` of a writing anthology agent, the
// mod appends an "After-work check" block naming what the report lacks, and nothing else. A block
// is raised only by a real gap (no commit, no test evidence, attribution lines, uncommitted files
// in the agent's worktree); the project-map and leftover items are reminders that ride in a block
// a gap raised. The test's own hooks sit beneath the plugin and stand for Claude Code:
// `agent.report` is the engine's delivery of the text, `env.get` the environment, `agent.list`,
// `session.*` and `fs.stat` where the agent worked, and `process.run` the one git call.
import type { AgentInfo, AgentReportInput, On, ProcessRunResult } from 'claude-code'
import { expect, test } from 'claude-code/testing'

type Run = {
  readonly argv: readonly string[]
  readonly init?: { readonly cwd?: string; readonly timeoutMs?: number; readonly env?: Record<string, string> }
}

type World = {
  env?: Record<string, string>
  agents?: () => AgentInfo[]
  cwd?: string
  root?: string
  /** Where a path really lands, for a link; any other path lands on itself. */
  real?: Record<string, string>
  git?: (run: Run) => ProcessRunResult | Promise<ProcessRunResult>
  /** What core says of the report, beside its text. */
  review?: { review: 'flagged'; warning: string }
  /** The paths that exist for `fs.exists`; any other path does not. */
  files?: { includes(path: string): boolean }
  /** The paths of a regular `git` file; by default one on the system PATH. Any other `git` path is missing. */
  gitAt?: readonly string[]
  /** The paths of a folder called `git`. */
  gitDirs?: readonly string[]
  /** The paths of a link called `git`, with what it leads to (a dangling link is `other`). */
  gitLinks?: Record<string, 'file' | 'dir' | 'other'>
}

/** The PATH the test session has unless `env.PATH` says otherwise. */
const SYSTEM_PATH = '/usr/local/bin:/usr/bin:/bin'

const CLEAN: ProcessRunResult = {
  exitCode: 0,
  stdout: '',
  stderr: '',
  isStdoutTruncated: false,
  isStderrTruncated: false,
}

/** Stands for Claude Code beneath the plugin: it delivers the text it was given, as core does. */
function engine(on: On, w: World = {}) {
  const runs: Run[] = []
  on('agent.report', (_$, e) => ({ text: e.text, review: 'cleared', ...w.review }))
  on('env.get', (_$, e) => ({ value: w.env?.[e.name] ?? (e.name === 'PATH' ? SYSTEM_PATH : undefined) }))
  on('agent.list', () => ({ value: w.agents?.() ?? [] }))
  on('session.cwd', () => ({ value: w.cwd ?? '/work/repo' }))
  on('session.root', () => ({ value: w.root ?? '/work/repo' }))
  on('fs.stat', (_$, e) => {
    const fileAt = (w.gitAt ?? ['/usr/bin/git']).includes(e.path)
    const link = w.gitLinks?.[e.path]
    // What `$.fs.stat` says of a `git` path: a link is followed, a dangling one is `other`, a missing path rejects.
    if (fileAt || link !== undefined || w.gitDirs?.includes(e.path)) {
      const kind = fileAt ? 'file' : link ?? 'dir'
      return { value: { kind, size: 0, mtimeMs: 0, isLink: link !== undefined } }
    }
    if (e.path === 'git' || e.path.endsWith('/git')) throw new Error(`ENOENT: ${e.path}`)
    return { value: { kind: 'dir', size: 0, mtimeMs: 0, isLink: false, realPath: w.real?.[e.path] ?? e.path } }
  })
  on('fs.exists', (_$, e) => ({ value: w.files?.includes(e.path) ?? false }))
  on('process.run', async (_$, e) => {
    runs.push(e)
    return { value: await (w.git ?? (() => CLEAN))(e) }
  })
  return runs
}

const FULL = [
  'Summary: added the after-work check.',
  'Changes: committed 3f9a2c1 on branch p7-afterwork.',
  'Verification: claude plugin test passed (9 tests); tsc clean.',
  'Cleanup: removed my container and its volume; no temp files left.',
  'Project map: updated in the same commit.',
  'Open questions: none.',
].join('\n')

function report(text: string, over: Partial<AgentReportInput> = {}): AgentReportInput {
  return {
    agentId: 'a1',
    subagentType: 'anthology:implementer',
    provider: { plugin: 'anthology', tier: 'user' },
    status: 'completed',
    text,
    ...over,
  }
}

const CHECK = 'After-work check'
const withVerification = (line: string) => FULL.replace('claude plugin test passed (9 tests); tsc clean.', line)
const withChanges = (line: string) => FULL.replace('committed 3f9a2c1 on branch p7-afterwork.', line)

test('a full report from a writer is delivered as written, with no block', async ($, on) => {
  engine(on)
  const r = await $.agent.report(report(FULL))
  expect(r.text).toBe(FULL)
})

test('each real gap is flagged by name, and only that gap', async ($, on) => {
  engine(on)
  const cases: { text: string; flag: string }[] = [
    { text: withChanges('x'), flag: 'no commit' },
    { text: withVerification('x'), flag: 'what was tested' },
  ]
  for (const { text, flag } of cases) {
    const r = await $.agent.report(report(text))
    expect(r.text).toContain(`${text}\n\n`)
    expect(r.text?.slice(text.length)).toContain(CHECK)
    expect(r.text?.slice(text.length)).toContain(flag)
    expect(r.text?.slice(text.length).match(/^- /gm)).toHaveLength(1)
  }
})

test('a report short of everything gets one block with the two real gaps and no reminder', async ($, on) => {
  engine(on)
  const r = await $.agent.report(report('Did the thing.'))
  const block = r.text?.slice('Did the thing.'.length) ?? ''
  expect(r.text?.startsWith('Did the thing.\n\n')).toBe(true)
  expect(block.match(/^- /gm)).toHaveLength(2)
  expect(block).not.toContain('Reminder')
})

test('a report with a commit and test evidence but no map or cleanup line gets no block', async ($, on) => {
  engine(on)
  const text = FULL.replace('Cleanup: removed my container and its volume; no temp files left.\n', '').replace(
    'Project map: updated in the same commit.\n',
    '',
  )
  expect((await $.agent.report(report(text))).text).toBe(text)
  const rewired = `${text}\nAdded hooks/mods/x.ts and ran the tests in a container.`
  expect((await $.agent.report(report(rewired))).text).toBe(rewired)
})

test('the commit check takes a sha or the word, and not "not committed"', async ($, on) => {
  engine(on)
  for (const ok of ['committed', 'HEAD is 3f9a2c1', 'commit 1234567', 'HEAD at 12345678901234567890']) {
    expect((await $.agent.report(report(withChanges(ok)))).text).toBe(withChanges(ok))
  }
  for (const bad of ['not committed', 'uncommitted changes remain', 'nothing committed', 'ticket 1234567', 'x']) {
    expect((await $.agent.report(report(withChanges(bad)))).text).toContain('no commit')
  }
})

test('test evidence is a test word, and "not run" or "skipped" in any order is none', async ($, on) => {
  engine(on)
  for (const ok of ['cargo test passed', 'tsc clean', 'ran clippy', 'tests passed (9)']) {
    expect((await $.agent.report(report(withVerification(ok)))).text).toBe(withVerification(ok))
  }
  for (const bad of [
    'tests were not run',
    'Tests: not run',
    'tests skipped',
    'not tested',
    'the tests were skipped',
    'no tests',
    'Tests: none',
    'I did not run the tests',
  ]) {
    const text = withVerification(bad)
    expect((await $.agent.report(report(text))).text).toContain('what was tested')
  }
})

test('attribution lines in the report are flagged, in either spelling', async ($, on) => {
  engine(on)
  for (const line of [
    'Co-Authored-By: Claude Sonnet <noreply@anthropic.com>',
    '  - Generated with [Claude Code](https://claude.com/claude-code)',
  ]) {
    const text = `${FULL}\n${line}`
    const r = await $.agent.report(report(text))
    expect(r.text?.slice(text.length)).toContain('attribution')
  }
})

test('a report that only says it added none is not flagged for attribution', async ($, on) => {
  engine(on)
  const text = `${FULL}\nNo Co-Authored-By or "Generated with" lines were added.`
  expect((await $.agent.report(report(text))).text).toBe(text)
})

test('the map reminder shows only for a named new, removed or renamed piece, and not when the map is dealt with', async ($, on) => {
  engine(on)
  const base = 'Changes: Added hooks/mods/x.ts, renamed the build command.'
  const r = await $.agent.report(report(base))
  expect(r.text).toContain('Reminder: ')
  expect(r.text).toContain('project map')
  expect((await $.agent.report(report('Did the thing.'))).text).not.toContain('Reminder')
  for (const said of ['map not needed', 'Updated the project map.', 'Updated reference-project-map.md.']) {
    const t = await $.agent.report(report(`${base}\n${said}`))
    expect(t.text).toContain(CHECK)
    expect(t.text).not.toContain('Reminder')
  }
})

// The agent folder of IN_WORKTREE (below), and the map in it.
const MAP = '/work/repo/.claude/worktrees/agent-1/.claude/memory/reference-project-map.md'
const ABSENT_CLAIMS = [
  'No project map exists.',
  'There is no map.',
  'The map is missing.',
  'Project map missing in this worktree.',
  'No map yet.',
]
// Names a new piece without a path: a path in a world with no `.git` would read as an edit outside a repo.
const MAP_BASE = 'Changes: Added a new mod, renamed the build command.'

test('a "map missing" claim counts as dealing with the map only when the file is absent in the agent folder', async ($, on) => {
  engine(on, { ...IN_WORKTREE(), files: [] })
  for (const said of ABSENT_CLAIMS) {
    const r = await $.agent.report(report(`${MAP_BASE}\n${said}`))
    expect(r.text).toContain(CHECK)
    expect(r.text).not.toContain('Reminder')
  }
})

test('a "map missing" claim does not count when the map is there, and the reminder stays', async ($, on) => {
  engine(on, { ...IN_WORKTREE(), files: [MAP] })
  for (const said of ABSENT_CLAIMS) {
    const r = await $.agent.report(report(`${MAP_BASE}\n${said}`))
    expect(r.text).toContain('Reminder: ')
    expect(r.text).toContain('project map')
  }
})

test('a "map missing" claim does not count when the agent folder is not known', async ($, on) => {
  engine(on, { agents: () => [], files: [] })
  for (const said of ABSENT_CLAIMS) {
    const r = await $.agent.report(report(`${MAP_BASE}\n${said}`))
    expect(r.text).toContain('Reminder: ')
  }
})

test('the map is looked up at the agent folder, never the session folder', async ($, on) => {
  // The agent folder holds its own .git, so the walk up never reaches the session folder's map.
  engine(on, { ...IN_WORKTREE(), files: [`${WORKTREE}/.git`, '/work/repo/.claude/memory/reference-project-map.md'] })
  const r = await $.agent.report(report(`${MAP_BASE}\nThe map is missing.`))
  expect(r.text).not.toContain('Reminder')
})

// What an agent writes when it follows "call it missing only after that Read fails, and quote the
// error": the claim names the file and carries the error text, so no leftover mention of the
// project map may stand for dealing with it.
const QUOTED_CLAIMS = [
  'Map file not found at .claude/memory/reference-project-map.md',
  'Read .claude/memory/reference-project-map.md: not found.',
  'Read of /work/repo/.claude/memory/reference-project-map.md failed: File does not exist.',
  'The project map does not exist.',
  'project map: ENOENT',
  "ENOENT: no such file or directory, open '/work/repo/.claude/memory/reference-project-map.md'",
]

test('a quoted read error is judged by whether the map exists, not by the words around it', async ($, on) => {
  // The claims name paths, so the worktree and its parent repo hold `.git`: they are not edits outside one.
  const gits = (p: string) => p === `${WORKTREE}/.git` || p === '/work/repo/.git'
  const w: World = { ...IN_WORKTREE(), files: { includes: p => p === MAP || gits(p) } }
  engine(on, w)
  for (const said of QUOTED_CLAIMS) {
    const r = await $.agent.report(report(`${MAP_BASE}\n${said}`))
    expect(r.text, said).toContain('Reminder: ')
  }
  w.files = { includes: gits }
  for (const said of QUOTED_CLAIMS) {
    const r = await $.agent.report(report(`${MAP_BASE}\n${said}`))
    expect(r.text, said).toContain(CHECK)
    expect(r.text, said).not.toContain('Reminder')
  }
})

test('a report that updated the map and says nothing of it missing is not asked about the lookup', async ($, on) => {
  engine(on, { ...IN_WORKTREE(), files: { includes: () => false } })
  const r = await $.agent.report(report(`${MAP_BASE}\nUpdated .claude/memory/reference-project-map.md; tests failed before the fix.`))
  expect(r.text).not.toContain('Reminder')
})

const REPO = '/work/repo/.claude/worktrees/agent-1'
const REPO_MAP = `${REPO}/.claude/memory/reference-project-map.md`
const at = (cwd: string): World => ({ agents: () => [{ id: 'a1', cwd } as AgentInfo] })

test('a subfolder cwd also looks in the parent folders, up to the first one that holds .git', async ($, on) => {
  const w: World = { ...at(`${REPO}/crates/x`), files: { includes: p => p === REPO_MAP || p === `${REPO}/.git` } }
  engine(on, w)
  const there = await $.agent.report(report(`${MAP_BASE}\nThe map is missing.`))
  expect(there.text).toContain('Reminder: ')
  w.files = { includes: p => p === `${REPO}/.git` }
  const gone = await $.agent.report(report(`${MAP_BASE}\nThe map is missing.`))
  expect(gone.text).not.toContain('Reminder')
})

test('the walk up stops at the first folder with .git, so a map above the repo does not count', async ($, on) => {
  const above = '/work/repo/.claude/worktrees/.claude/memory/reference-project-map.md'
  engine(on, { ...at(`${REPO}/crates/x`), files: { includes: p => p === above || p === `${REPO}/.git` } })
  const r = await $.agent.report(report(`${MAP_BASE}\nThe map is missing.`))
  expect(r.text).not.toContain('Reminder')
})

test('a trailing slash on the agent cwd does not matter', async ($, on) => {
  engine(on, { ...at(`${REPO}/`), files: { includes: p => p === REPO_MAP } })
  const there = await $.agent.report(report(`${MAP_BASE}\nThe map is missing.`))
  expect(there.text).toContain('Reminder: ')
})

test('a map that is there does not undo a report that says it updated the map', async ($, on) => {
  engine(on, { ...IN_WORKTREE(), files: [MAP] })
  const r = await $.agent.report(report(`${MAP_BASE}\nUpdated the project map.`))
  expect(r.text).not.toContain('Reminder')
  const said = await $.agent.report(report(`${MAP_BASE}\nThe map is missing, so I did not update it.`))
  expect(said.text).toContain('Reminder: ')
})

test('a failing map lookup leaves the claim unverified', async ($, on) => {
  const broken = {
    includes: (): boolean => {
      throw new Error('no fs')
    },
  }
  engine(on, { ...IN_WORKTREE(), files: broken })
  const r = await $.agent.report(report(`${MAP_BASE}\nThe map is missing.`))
  expect(r.text).toContain('Reminder: ')
})

test('the leftover reminder needs a started container, volume or temp file with no removal said', async ($, on) => {
  engine(on)
  const ran = 'Ran the tests in a container.'
  expect((await $.agent.report(report(ran))).text).toContain('Reminder: ')
  for (const said of ['Removed the container.', 'Nothing to clean up.', 'No leftovers.', 'Volumes were deleted.']) {
    const r = await $.agent.report(report(`${ran}\n${said}`))
    expect(r.text).toContain(CHECK)
    expect(r.text).not.toContain('Reminder')
  }
  const docs = await $.agent.report(report(ran, { subagentType: 'anthology:docs-writer' }))
  expect(docs.text).toContain(CHECK)
  expect(docs.text).not.toContain('Reminder')
})

test('every writing agent is checked, and the reviewer, scout and others are skipped', async ($, on) => {
  engine(on)
  for (const t of ['implementer', 'debugger', 'docs-writer', 'ui-designer']) {
    const r = await $.agent.report(report('Done.', { subagentType: `anthology:${t}` }))
    expect(r.text).toContain(CHECK)
  }
  for (const t of ['anthology:reviewer', 'anthology:scout', 'anthology:architect', 'Explore', 'implementer', 'other:implementer']) {
    const r = await $.agent.report(report('Done.', { subagentType: t }))
    expect(r.text).toBe('Done.')
  }
})

test('a nested, failed, stopped or empty report is left as it is', async ($, on) => {
  engine(on)
  expect((await $.agent.report(report('Done.', { parentAgentId: 'p1' }))).text).toBe('Done.')
  expect((await $.agent.report(report('Done.', { status: 'failed' }))).text).toBe('Done.')
  expect((await $.agent.report(report('Done.', { status: 'stopped' }))).text).toBe('Done.')
  expect((await $.agent.report(report(' \n'))).text).toBe(' \n')
})

test('a report consumed by a hook beneath stays consumed', async ($, on) => {
  on('agent.report', () => ({ consumed: 'kept' }))
  const r = await $.agent.report(report('Done.'))
  expect(r).toEqual({ consumed: 'kept' })
})

test('the agent text stays verbatim ahead of the block, odd spacing and all', async ($, on) => {
  engine(on)
  const text = '  Summary:\tweird   spacing\r\n\n\nCo-Authored-By: Claude <x>\n'
  const r = await $.agent.report(report(text))
  expect(r.text?.startsWith(text)).toBe(true)
})

test('the review and warning core gave the report stay with the rewritten text', async ($, on) => {
  engine(on, { review: { review: 'flagged', warning: 'W' } })
  const r = await $.agent.report(report('Done.'))
  expect(r.text).toContain(CHECK)
  expect(r).toMatchObject({ review: 'flagged', warning: 'W' })
})

test('ANTHOLOGY_AFTERWORK=0 and ANTHOLOGY_MODS=0 each turn the check off', async ($, on) => {
  const w: World = { env: { ANTHOLOGY_AFTERWORK: '0' } }
  engine(on, w)
  expect((await $.agent.report(report('Done.'))).text).toBe('Done.')
  w.env = { ANTHOLOGY_MODS: '0' }
  expect((await $.agent.report(report('Done.'))).text).toBe('Done.')
  w.env = { ANTHOLOGY_AFTERWORK: '1' }
  expect((await $.agent.report(report('Done.'))).text).toContain(CHECK)
  w.env = {}
  expect((await $.agent.report(report('Done.'))).text).toContain(CHECK)
})

const WORKTREE = '/work/repo/.claude/worktrees/agent-1'
const IN_WORKTREE = (): World => ({ agents: () => [{ id: 'a1', cwd: WORKTREE } as AgentInfo] })

test('dirty files in the agent worktree add one gap, from one git call with a timeout', async ($, on) => {
  const runs = engine(on, {
    ...IN_WORKTREE(),
    git: () => ({ ...CLEAN, stdout: ' M src/a.ts\n?? notes.md\n' }),
  })
  const r = await $.agent.report(report(FULL))
  expect(r.text?.startsWith(`${FULL}\n\n`)).toBe(true)
  expect(r.text?.slice(FULL.length)).toContain('2 uncommitted')
  expect(runs).toHaveLength(1)
  expect(runs[0]?.argv.slice(0, 1)).toEqual(['/usr/bin/git'])
  expect(runs[0]?.argv).toContain('status')
  expect(runs[0]?.init?.cwd).toBe(WORKTREE)
  expect(runs[0]?.init?.timeoutMs).toBeGreaterThan(0)
  expect(runs[0]?.init?.timeoutMs).toBeLessThanOrEqual(3000)
})

test('git is the absolute file on the absolute PATH entries; relative and empty entries are ignored', async ($, on) => {
  // The worktree holds a planted `git`: `.`, `::`, `bin` and a bare leading colon must not find it.
  const planted = [`${WORKTREE}/git`, `${WORKTREE}/bin/git`, 'git', 'bin/git', './git']
  const runs = engine(on, {
    ...IN_WORKTREE(),
    env: { PATH: '.:bin::/opt/tools/bin/:relative/dir:/usr/bin:/bin' },
    gitAt: [...planted, '/opt/tools/bin/git', '/usr/bin/git'],
    git: () => ({ ...CLEAN, stdout: ' M a\n' }),
  })
  const r = await $.agent.report(report(FULL))
  expect(r.text).toContain('1 uncommitted')
  expect(runs).toHaveLength(1)
  expect(runs[0]?.argv[0]).toBe('/opt/tools/bin/git')
  expect(runs[0]?.init?.env?.PATH).toBe('/opt/tools/bin/:/usr/bin:/bin')
})

test('git is not run when the PATH has no absolute entry that holds one', async ($, on) => {
  const w: World = { ...IN_WORKTREE(), gitAt: [`${WORKTREE}/git`, 'git', './git', 'bin/git'] }
  const runs = engine(on, w)
  for (const PATH of ['.', ':', '.::bin', 'bin:./tools', '']) {
    w.env = { PATH }
    expect((await $.agent.report(report(FULL))).text, PATH).toBe(FULL)
  }
  w.env = { PATH: '/usr/bin:/bin' } // absolute entries, but none holds a git
  w.gitAt = ['/usr/local/bin/git']
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(runs).toHaveLength(0)
})

test('a folder, a dangling link or a link to a folder called git is passed over for the next entry', async ($, on) => {
  const runs = engine(on, {
    ...IN_WORKTREE(),
    env: { PATH: '/a:/b:/c:/d:/e' },
    gitDirs: ['/a/git'],
    gitLinks: { '/b/git': 'other', '/c/git': 'dir', '/d/git': 'file' }, // a link to a file is a git
    gitAt: ['/e/git'],
    git: () => ({ ...CLEAN, stdout: ' M a\n' }),
  })
  const r = await $.agent.report(report(FULL))
  expect(r.text).toContain('1 uncommitted')
  expect(runs).toHaveLength(1)
  expect(runs[0]?.argv[0]).toBe('/d/git')
})

test('a git that cannot start, as a file that is not executable, is passed over for the next one', async ($, on) => {
  const runs = engine(on, {
    ...IN_WORKTREE(),
    env: { PATH: '/noexec:/usr/bin' },
    gitAt: ['/noexec/git', '/usr/bin/git'],
    git: run => {
      if (run.argv[0] === '/noexec/git') throw new Error('spawn EACCES')
      return { ...CLEAN, stdout: ' M a\n' }
    },
  })
  const r = await $.agent.report(report(FULL))
  expect(r.text).toContain('1 uncommitted')
  expect(runs.map(x => x.argv[0])).toEqual(['/noexec/git', '/usr/bin/git'])
})

test('at most three gits are tried, and all failing leaves the text-only check', async ($, on) => {
  const runs = engine(on, {
    ...IN_WORKTREE(),
    env: { PATH: '/a:/b:/c:/d' },
    gitAt: ['/a/git', '/b/git', '/c/git', '/d/git'],
    git: () => {
      throw new Error('spawn EACCES')
    },
  })
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(runs).toHaveLength(3)
})

test('the git call leaves the agents own memory notes out, from the repo top', async ($, on) => {
  const runs = engine(on, IN_WORKTREE())
  await $.agent.report(report(FULL))
  const argv = runs[0]?.argv ?? []
  const at = argv.indexOf('--')
  expect(at).toBeGreaterThan(0)
  expect(argv.slice(at + 1)).toEqual([':(top)', ':(top,exclude).claude/agent-memory'])
})

test('output cut at the cap reads as many uncommitted changes', async ($, on) => {
  engine(on, { ...IN_WORKTREE(), git: () => ({ ...CLEAN, stdout: ' M a\n', isStdoutTruncated: true }) })
  const r = await $.agent.report(report(FULL))
  expect(r.text?.slice(FULL.length)).toContain('many uncommitted')
})

test('a clean agent worktree adds nothing', async ($, on) => {
  engine(on, IN_WORKTREE())
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
})

test('git is not run without a worktree of its own for the agent', async ($, on) => {
  const gone = engine(on, { agents: () => [] })
  await $.agent.report(report(FULL))
  expect(gone).toHaveLength(0)
})

test('git is not run when the agent works in the session folder itself', async ($, on) => {
  const runs = engine(on, { cwd: '/work/repo', agents: () => [{ id: 'a1', cwd: '/work/repo' } as AgentInfo] })
  await $.agent.report(report(FULL))
  expect(runs).toHaveLength(0)
})

test('the session folder is told by real path: a trailing slash, a link, the session root', async ($, on) => {
  const w: World = { cwd: '/work/repo/sub', root: '/work/repo', agents: () => [{ id: 'a1', cwd: '/work/repo/' } as AgentInfo] }
  const runs = engine(on, w)
  await $.agent.report(report(FULL)) // the folder is the session root with a trailing slash
  w.agents = () => [{ id: 'a1', cwd: '/link/sub' } as AgentInfo]
  w.real = { '/link/sub': '/work/repo/sub' } // a link onto the session cwd
  await $.agent.report(report(FULL))
  expect(runs).toHaveLength(0)
  w.real = {}
  await $.agent.report(report(FULL)) // an unrelated folder is checked
  expect(runs).toHaveLength(1)
})

test('a git timeout, a git failure or a lookup failure leaves the text-only check', async ($, on) => {
  let how: 'timeout' | 'exit' | 'list' = 'timeout'
  engine(on, {
    agents: () => {
      if (how === 'list') throw new Error('no list')
      return [{ id: 'a1', cwd: WORKTREE } as AgentInfo]
    },
    git: () => {
      if (how === 'timeout') throw new Error('process.run timed out')
      return { ...CLEAN, exitCode: 128, stdout: ' M ignored\n', stderr: 'fatal: not a git repository' }
    },
  })
  for (const h of ['timeout', 'exit', 'list'] as const) {
    how = h
    const full = await $.agent.report(report(FULL))
    expect(full.text).toBe(FULL)
    const part = await $.agent.report(report('Done.'))
    expect(part.text?.startsWith('Done.\n\n')).toBe(true)
    expect(part.text).toContain('no commit')
    expect(part.text).not.toContain('uncommitted')
  }
})

// Edits outside any git repo: there is nothing to commit and no test to run, so the report that says
// neither is not short of anything. The edited paths come from the report text: absolute, `~/` and
// relative (against the agent's folder). A path is in a repo when it or a folder above it holds `.git`.
const HOME_DIR = '/home/u'
const OUTSIDE = `Changed ${HOME_DIR}/.claude/settings.json: set the model.`
const outsideWorld = (over: World = {}): World => ({
  agents: () => [{ id: 'a1', cwd: '/scratch' } as AgentInfo],
  env: { HOME: HOME_DIR },
  files: { includes: () => false },
  ...over,
})
const gitAt = (...dirs: string[]) => ({ includes: (p: string) => dirs.some(d => p === `${d}/.git`) })

test('edits only outside a git repo: no commit and no tested gap, whatever the path spelling', async ($, on) => {
  engine(on, outsideWorld())
  for (const text of [
    OUTSIDE,
    'Edited ~/.claude/settings.json: set the model.',
    'Edited `/etc/hosts` and /opt/tools/conf.toml.',
    'Edited ./notes/todo.md (relative to the scratch folder).',
  ]) {
    expect((await $.agent.report(report(text))).text, text).toBe(text)
  }
})

test('an edit inside a git repo keeps the commit and tested gaps', async ($, on) => {
  engine(on, outsideWorld({ files: gitAt(HOME_DIR) })) // a dotfiles repo in the home folder
  const r = await $.agent.report(report(OUTSIDE))
  expect(r.text).toContain('no commit')
  expect(r.text).toContain('what was tested')
})

test('mixed edits keep the checks, because one path is in a repo', async ($, on) => {
  engine(on, outsideWorld({ files: gitAt('/work/repo') }))
  const r = await $.agent.report(report(`${OUTSIDE}\nAlso edited /work/repo/src/a.ts.`))
  expect(r.text).toContain('no commit')
  expect(r.text).toContain('what was tested')
})

test('a relative path is judged by the agent folder, and unknown is treated as in a repo', async ($, on) => {
  const text = 'Edited src/a.ts.'
  const w = outsideWorld()
  engine(on, w)
  expect((await $.agent.report(report(text))).text).toBe(text)
  w.files = gitAt('/scratch')
  expect((await $.agent.report(report(text))).text).toContain('no commit')
  w.files = { includes: () => false }
  w.agents = () => []
  expect((await $.agent.report(report(text))).text).toContain('no commit')
})

test('a ~ path with no HOME is unknown, so the checks stay', async ($, on) => {
  engine(on, outsideWorld({ env: {} }))
  expect((await $.agent.report(report('Edited ~/.claude/settings.json.'))).text).toContain('no commit')
})

test('a report with no path in it keeps the checks, and a URL or a slash command is no path', async ($, on) => {
  engine(on, outsideWorld())
  for (const text of ['Done.', 'Ran /plugin install and read https://github.com/a/b/c.', 'Chose read/write.']) {
    expect((await $.agent.report(report(text))).text, text).toContain('no commit')
  }
})

test('outside a repo, attribution lines are still flagged and a commit is still not asked for', async ($, on) => {
  engine(on, outsideWorld())
  const text = `${OUTSIDE}\nCo-Authored-By: Claude <x>`
  const block = (await $.agent.report(report(text))).text?.slice(text.length) ?? ''
  expect(block).toContain('attribution')
  expect(block).not.toContain('no commit')
  expect(block).not.toContain('what was tested')
})

test('a failing path lookup keeps the checks', async ($, on) => {
  const broken = {
    includes: (): boolean => {
      throw new Error('no fs')
    },
  }
  engine(on, outsideWorld({ files: broken }))
  expect((await $.agent.report(report(OUTSIDE))).text).toContain('no commit')
})
