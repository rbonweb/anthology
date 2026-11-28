// afterwork.ts (post-release mods design §4): on `agent.report` of a writing anthology agent, the
// mod appends an "After-work check" block naming what the report lacks, and nothing else. A block
// is raised only by a real gap (no commit, no test evidence, attribution lines, uncommitted files
// in the agent's worktree); the project-map and leftover items are reminders that ride in a block
// a gap raised. The test's own hooks sit beneath the plugin and stand for Claude Code:
// `agent.report` is the engine's delivery of the text, `env.get` the environment, `agent.list`,
// `session.*` and `fs.stat` where the agent worked, and `process.run` the one git call. The slop
// part adds a second git call (the lead's HEAD) and one call to the installed anthology binary,
// which `World.slop` stands for; both are kept apart from `runs`, the `git status` calls.
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
  /** The installed anthology binary and what `slop check` does; absent: no binary is installed. */
  slop?: (run: Run) => ProcessRunResult | Promise<ProcessRunResult>
  /** What `git rev-parse HEAD` does in the lead's folder; by default it prints HEAD. */
  head?: (run: Run) => ProcessRunResult | Promise<ProcessRunResult>
  /** Every call the slop part makes: the `rev-parse` and the binary, in order. */
  calls?: Run[]
}

/** The PATH the test session has unless `env.PATH` says otherwise. */
const SYSTEM_PATH = '/usr/local/bin:/usr/bin:/bin'

/** The install the shim would find: <ah>/bin/<lock version>/anthology and its .sha256 (as util.ts reads it). */
const AH = '/home/user/.claude/anthology'
const SLOP_BIN = `${AH}/bin/0.1.1/anthology`
const LOCK = '{\n  "version": "0.1.1",\n  "tag": "anthology-bin--v0.1.1",\n  "repo": "rbonweb/anthology",\n  "sha256": {}\n}\n'
const HEAD = 'a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0'

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
  on('env.get', (_$, e) => ({
    value: w.env?.[e.name] ?? (e.name === 'PATH' ? SYSTEM_PATH : e.name === 'HOME' && w.slop ? '/home/user' : undefined),
  }))
  on('agent.list', () => ({ value: w.agents?.() ?? [] }))
  on('session.cwd', () => ({ value: w.cwd ?? '/work/repo' }))
  on('session.root', () => ({ value: w.root ?? '/work/repo' }))
  on('fs.read', (_$, e) => {
    if (w.slop && e.path.endsWith('/anthology.lock')) return { value: LOCK }
    throw new Error(`ENOENT: ${e.path}`)
  })
  on('fs.stat', (_$, e) => {
    if (w.slop && e.path === SLOP_BIN) return { value: { kind: 'file', size: 100, mtimeMs: 0, isLink: false } }
    if (w.slop && e.path === `${AH}/bin/0.1.1/.sha256`) return { value: { kind: 'file', size: 65, mtimeMs: 0, isLink: false } }
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
    if (e.argv[0] === SLOP_BIN) {
      w.calls?.push(e)
      return { value: await (w.slop ?? (() => CLEAN))(e) }
    }
    if (e.argv.includes('rev-parse')) {
      w.calls?.push(e)
      return { value: await (w.head ?? (() => ({ ...CLEAN, stdout: `${HEAD}\n` })))(e) }
    }
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

test('the reported wording, hash checks and live downloads that verified the result, is test evidence', async ($, on) => {
  engine(on)
  const line = 'hash checks matched the pinned sha256 for every asset, and live downloads of each URL verified the result.'
  expect((await $.agent.report(report(withVerification(line)))).text).toBe(withVerification(line))
})

// The common ways a report says it checked, by what was checked: code, a download, a service, a
// screenshot, a page of prose. None of them holds the word "test".
const EVIDENCE = [
  'verified the output by hand',
  'checked the result against the spec',
  'confirmed the file opens',
  'sha256 matched the published checksum',
  'the checksum matched',
  'hashes are identical',
  'downloaded the asset again and compared it with the original',
  'ran the script twice',
  're-ran it after the fix, same result',
  'smoke run of the install path',
  'curl returned 200 for the health endpoint',
  'GET /health: HTTP 200',
  'status code 204 from the API',
  'screenshot checked at 1280px',
  'inspected the screenshot, layout holds',
  '12 passed',
  'all green in CI',
  'build ok, 0 errors',
  'compiles cleanly',
  'exit code 0',
]

test('test evidence takes the common wording of a report that checked its work', async ($, on) => {
  engine(on)
  for (const ok of EVIDENCE) {
    expect((await $.agent.report(report(withVerification(ok)))).text, ok).toBe(withVerification(ok))
  }
})

test('test evidence is judged the same for the docs-writer and the ui-designer', async ($, on) => {
  engine(on)
  const cases: [string, string][] = [
    ['anthology:docs-writer', 'cross-checked each claim against its three sources; all links resolve (curl 200).'],
    ['anthology:docs-writer', 'verified every command in the page by running it.'],
    ['anthology:ui-designer', 'screenshot taken and checked at 375px and 1280px.'],
    ['anthology:ui-designer', 'inspected the screenshot: spacing and contrast hold.'],
    ['anthology:debugger', 'reproduced the failure, applied the fix, re-ran it: 4 passed.'],
  ]
  for (const [subagentType, line] of cases) {
    const text = withVerification(line)
    expect((await $.agent.report(report(text, { subagentType }))).text, line).toBe(text)
  }
})

test('a report that says nothing about checking its work still gets the tested note', async ($, on) => {
  engine(on)
  for (const bad of [
    'x',
    'Rewrote the intro and updated the table of contents.',
    'Added a screenshot of the page to the README.',
    'Changed the button to green and widened the layout.',
    'Reviewed the sources and wrote the summary.',
    'Added the verification section to the guide.',
    'Summary of the download: three files, 40 MB.',
    'not verified',
    'nothing was checked',
    'I did not check anything',
    "couldn't run it here",
    'never ran it',
    'unable to verify the download',
    'could not confirm the result',
    'not compared with the original',
  ]) {
    const text = withVerification(bad)
    expect((await $.agent.report(report(text))).text, bad).toContain('what was tested')
    const docs = await $.agent.report(report(text, { subagentType: 'anthology:docs-writer' }))
    expect(docs.text, bad).toContain('what was tested')
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

// The slop part: `anthology slop check --since <lead HEAD>` in the writer's worktree. The lines are
// the CLI contract's: `{shown}:{line}: {rule}: "{excerpt}"` and, for a waiver in the new text,
// `{shown}:{line}: slop-ok {rule}: {reason}` or `{shown}:{line}: slop-ignore region`.
const TELL_1 = 'docs/a.md:3: text.ai-vocab: "We delve into the cache layer."'
const TELL_2 = 'docs/a.md:9: text.stock-phrase: "Let\'s dive in and read the code."'
const TELL_3 = 'src/Hero.tsx:12: ui.purple-gradient: "linear-gradient(135deg, #6366f1, #a855f7)"'
const TELL_4 = 'README.md:1: text.chat-residue: "I hope this helps!"'
const WAIVER_1 = 'docs/b.md:4: slop-ok text.ai-vocab: the product is called Seamless Sync'
const WAIVER_2 = 'docs/c.md:1: slop-ignore region'
const WAIVER_3 = 'src/Hero.tsx:2: slop-ok ui.purple-gradient: brand colour from the style guide'
const WAIVER_4 = 'docs/d.md:7: slop-ignore region'

/** What `slop check` prints: one line each, with the exit code the contract gives (1 holds a finding). */
const printed = (lines: readonly string[], exitCode = 1): ProcessRunResult => ({
  ...CLEAN,
  exitCode,
  stdout: lines.map(l => `${l}\n`).join(''),
})
const slopWorld = (lines: readonly string[], exitCode = 1, calls: Run[] = []): World => ({
  ...IN_WORKTREE(),
  slop: () => printed(lines, exitCode),
  calls,
})
const blockOf = (text: string | undefined) => text?.slice(FULL.length) ?? ''

test('a writer whose changes hold tells gets a slop gap, from the lead HEAD and the agent folder', async ($, on) => {
  const calls: Run[] = []
  engine(on, slopWorld([TELL_1, TELL_2, TELL_3, TELL_4], 1, calls))
  const r = await $.agent.report(report(FULL))
  expect(r.text?.startsWith(`${FULL}\n\n`)).toBe(true)
  expect(blockOf(r.text)).toContain(
    `- the agent's changes hold 4 AI tells (anthology slop check): ${TELL_1}; ${TELL_2}; ${TELL_3}; send it back to rewrite them`,
  )
  expect(blockOf(r.text).match(/^- /gm)).toHaveLength(1)
  expect(blockOf(r.text)).not.toContain(TELL_4)
  expect(calls.map(c => [...c.argv])).toEqual([
    ['/usr/bin/git', 'rev-parse', 'HEAD'],
    [SLOP_BIN, 'slop', 'check', '--root', WORKTREE, '--since', HEAD],
  ])
  expect(calls[0]?.init?.cwd).toBe('/work/repo')
  expect(calls[0]?.init?.timeoutMs).toBe(1500)
  expect(calls[1]?.init?.timeoutMs).toBe(4000)
})

test('one tell reads in the singular', async ($, on) => {
  engine(on, slopWorld([TELL_1]))
  const r = await $.agent.report(report(FULL))
  expect(blockOf(r.text)).toContain(`- the agent's changes hold 1 AI tell (anthology slop check): ${TELL_1}; send it back to rewrite them`)
})

test('added waivers raise their own gap, with a clean check beside them', async ($, on) => {
  engine(on, slopWorld([WAIVER_1, WAIVER_2, WAIVER_3, WAIVER_4], 0))
  const r = await $.agent.report(report(FULL))
  expect(blockOf(r.text)).toContain(
    `- the agent added 4 slop waivers or ignore regions: ${WAIVER_1}; ${WAIVER_2}; ${WAIVER_3}; confirm the user asked for that wording`,
  )
  expect(blockOf(r.text)).not.toContain('AI tell')
  expect(blockOf(r.text).match(/^- /gm)).toHaveLength(1)
})

test('one waiver reads in the singular', async ($, on) => {
  engine(on, slopWorld([WAIVER_2], 0))
  const r = await $.agent.report(report(FULL))
  expect(blockOf(r.text)).toContain(
    `- the agent added 1 slop waiver or ignore region: ${WAIVER_2}; confirm the user asked for that wording`,
  )
})

test('tells and waivers each count their own lines, tells first, after the dirty-files gap', async ($, on) => {
  engine(on, {
    ...slopWorld([TELL_1, WAIVER_1, TELL_2, WAIVER_2, WAIVER_3]),
    git: () => ({ ...CLEAN, stdout: ' M src/a.ts\n' }),
  })
  const gaps = blockOf((await $.agent.report(report(FULL))).text).match(/^- .*$/gm) ?? []
  expect(gaps).toEqual([
    "- 1 uncommitted change in the agent's worktree (git status)",
    `- the agent's changes hold 2 AI tells (anthology slop check): ${TELL_1}; ${TELL_2}; send it back to rewrite them`,
    `- the agent added 3 slop waivers or ignore regions: ${WAIVER_1}; ${WAIVER_2}; ${WAIVER_3}; confirm the user asked for that wording`,
  ])
})

test('lines that are not findings or waivers are ignored, and control characters are not passed on', async ($, on) => {
  engine(on, slopWorld(['warning: something', '', 'docs/a.md:x: text.ai-vocab: "no line"', `docs/a.md:3: text.ai-vocab: "a\u001b[31mred\u0007"`]))
  const r = await $.agent.report(report(FULL))
  expect(blockOf(r.text)).toContain(`the agent's changes hold 1 AI tell (anthology slop check): docs/a.md:3: text.ai-vocab: "a [31mred "; send`)
  expect(r.text).not.toContain('\u001b')
  expect(r.text).not.toContain('\u0007')
  expect(r.text).not.toContain('warning: something')
})

test('a clean check adds nothing', async ($, on) => {
  const calls: Run[] = []
  engine(on, slopWorld([], 0, calls))
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(calls).toHaveLength(2)
})

test('a missing binary, a timeout or exit 2 adds nothing', async ($, on) => {
  const w: World = { ...IN_WORKTREE(), calls: [] }
  engine(on, w)
  // No binary installed: HEAD is read, nothing else runs.
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(w.calls?.map(c => c.argv[0])).toEqual(['/usr/bin/git'])
  const stale = [TELL_1, WAIVER_1]
  const cases: Record<string, (run: Run) => ProcessRunResult> = {
    'timeout': () => {
      throw new Error('process.run timed out')
    },
    'exit 2': () => printed(stale, 2),
    'a crash': () => printed(stale, 101),
    'a signal': () => printed(stale, -1),
  }
  for (const [name, slop] of Object.entries(cases)) {
    w.slop = slop
    w.calls = []
    expect((await $.agent.report(report(FULL))).text, name).toBe(FULL)
    expect(w.calls.map(c => c.argv[0]), name).toEqual(['/usr/bin/git', SLOP_BIN])
  }
})

test('the check is not run without a HEAD to compare with', async ($, on) => {
  const w: World = { ...slopWorld([TELL_1]), calls: [] }
  engine(on, w)
  const heads: Record<string, (run: Run) => ProcessRunResult> = {
    'a git error': () => ({ ...CLEAN, exitCode: 128, stderr: 'fatal: not a git repository' }),
    'empty output': () => CLEAN,
    'an option': () => ({ ...CLEAN, stdout: '--output=/tmp/x\n' }),
    'a branch name': () => ({ ...CLEAN, stdout: 'main\n' }),
    'a timeout': () => {
      throw new Error('process.run timed out')
    },
  }
  for (const [name, head] of Object.entries(heads)) {
    w.head = head
    w.calls = []
    expect((await $.agent.report(report(FULL))).text, name).toBe(FULL)
    expect(w.calls.map(c => c.argv[0]), name).toEqual(['/usr/bin/git'])
  }
})

test('ANTHOLOGY_SLOP_GATE=0 never runs the check', async ($, on) => {
  const calls: Run[] = []
  const w: World = { ...slopWorld([TELL_1], 1, calls), env: { ANTHOLOGY_SLOP_GATE: '0' } }
  engine(on, w)
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(calls).toHaveLength(0)
  w.env = { ANTHOLOGY_SLOP_GATE: '1' }
  expect((await $.agent.report(report(FULL))).text).toContain('AI tell')
  expect(calls).toHaveLength(2)
})

test("the session's own folder is not checked", async ($, on) => {
  const calls: Run[] = []
  engine(on, { ...slopWorld([TELL_1], 1, calls), cwd: '/work/repo', agents: () => [{ id: 'a1', cwd: '/work/repo' } as AgentInfo] })
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(calls).toHaveLength(0)
})

test('no agent folder, or a failing agent lookup, runs no check', async ($, on) => {
  const calls: Run[] = []
  const w: World = { ...slopWorld([TELL_1], 1, calls), agents: () => [] }
  engine(on, w)
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  w.agents = () => {
    throw new Error('no list')
  }
  expect((await $.agent.report(report(FULL))).text).toBe(FULL)
  expect(calls).toHaveLength(0)
})

test('the slop check is for the writers the after-work check covers, and not for a nested or failed report', async ($, on) => {
  const calls: Run[] = []
  engine(on, slopWorld([TELL_1], 1, calls))
  expect((await $.agent.report(report('Done.', { subagentType: 'anthology:reviewer' }))).text).toBe('Done.')
  expect((await $.agent.report(report('Done.', { parentAgentId: 'p1' }))).text).toBe('Done.')
  expect((await $.agent.report(report('Done.', { status: 'failed' }))).text).toBe('Done.')
  expect(calls).toHaveLength(0)
})
