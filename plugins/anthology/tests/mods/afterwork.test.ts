// afterwork.ts (post-release mods design §4): on `agent.report` of a writing anthology agent, the
// mod appends an "After-work check" block naming what the report lacks, and nothing else. The
// test's own hooks sit beneath the plugin and stand for Claude Code: `agent.report` is the engine's
// delivery of the text, `env.get` the environment, `agent.list` and `session.cwd` where the agent
// worked, and `process.run` the one git call.
import type { AgentInfo, AgentReportInput, On, ProcessRunResult } from 'claude-code'
import { expect, test } from 'claude-code/testing'

type Run = {
  readonly argv: readonly string[]
  readonly init?: { readonly cwd?: string; readonly timeoutMs?: number }
}

type World = {
  env?: Record<string, string>
  agents?: () => AgentInfo[]
  cwd?: string
  git?: (run: Run) => ProcessRunResult | Promise<ProcessRunResult>
}

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
  on('agent.report', (_$, e) => ({ text: e.text, review: 'cleared' }))
  on('env.get', (_$, e) => ({ value: w.env?.[e.name] }))
  on('agent.list', () => ({ value: w.agents?.() ?? [] }))
  on('session.cwd', () => ({ value: w.cwd ?? '/work/repo' }))
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

test('a full report from a writer is delivered as written, with no block', async ($, on) => {
  engine(on)
  const r = await $.agent.report(report(FULL))
  expect(r.text).toBe(FULL)
})

test('each missing item is flagged by name, and only that item', async ($, on) => {
  engine(on)
  const cases: { drop: string; flag: string }[] = [
    { drop: 'committed 3f9a2c1', flag: 'no commit' },
    { drop: 'claude plugin test passed (9 tests); tsc clean.', flag: 'nothing about what was tested' },
    { drop: 'removed my container and its volume; no temp files left.', flag: 'leftovers removed' },
    { drop: 'Project map: updated in the same commit.', flag: 'the project map' },
  ]
  for (const { drop, flag } of cases) {
    const text = FULL.replace(drop, 'x')
    const r = await $.agent.report(report(text))
    expect(r.text).toContain(`${text}\n\n`)
    expect(r.text?.slice(text.length)).toContain(CHECK)
    expect(r.text?.slice(text.length)).toContain(flag)
    expect(r.text?.slice(text.length).match(/^- /gm)).toHaveLength(1)
  }
})

test('the commit check takes a sha or the word, and not "not committed"', async ($, on) => {
  engine(on)
  const word = await $.agent.report(report(FULL.replace('committed 3f9a2c1', 'committed')))
  expect(word.text).toBe(FULL.replace('committed 3f9a2c1', 'committed'))
  const sha = await $.agent.report(report(FULL.replace('committed 3f9a2c1', 'HEAD is 3f9a2c1')))
  expect(sha.text).toBe(FULL.replace('committed 3f9a2c1', 'HEAD is 3f9a2c1'))
  const not = await $.agent.report(report(FULL.replace('committed 3f9a2c1', 'not committed')))
  expect(not.text).toContain('no commit')
  const un = await $.agent.report(report(FULL.replace('committed 3f9a2c1', 'uncommitted changes remain')))
  expect(un.text).toContain('no commit')
})

test('a map that does not exist, said so, counts as mentioned', async ($, on) => {
  engine(on)
  const text = FULL.replace('Project map: updated in the same commit.', 'No project map exists in this repo.')
  expect((await $.agent.report(report(text))).text).toBe(text)
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

test('a report missing everything gets one block with every item', async ($, on) => {
  engine(on)
  const r = await $.agent.report(report('Did the thing.'))
  const block = r.text?.slice('Did the thing.'.length) ?? ''
  expect(r.text?.startsWith('Did the thing.\n\n')).toBe(true)
  expect(block.match(/^- /gm)).toHaveLength(4)
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

test('a nested, failed, stopped, empty or consumed report is left as it is', async ($, on) => {
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

test('dirty files in the agent worktree add one line, from one git call with a timeout', async ($, on) => {
  const runs = engine(on, {
    ...IN_WORKTREE(),
    git: () => ({ ...CLEAN, stdout: ' M src/a.ts\n?? notes.md\n' }),
  })
  const r = await $.agent.report(report(FULL))
  expect(r.text?.startsWith(`${FULL}\n\n`)).toBe(true)
  expect(r.text?.slice(FULL.length)).toContain('2 uncommitted')
  expect(runs).toHaveLength(1)
  expect(runs[0]?.argv.slice(0, 1)).toEqual(['git'])
  expect(runs[0]?.argv).toContain('status')
  expect(runs[0]?.init?.cwd).toBe(WORKTREE)
  expect(runs[0]?.init?.timeoutMs).toBeGreaterThan(0)
  expect(runs[0]?.init?.timeoutMs).toBeLessThanOrEqual(3000)
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
