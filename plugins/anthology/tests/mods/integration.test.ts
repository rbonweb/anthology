// The mods together (post-release mods design §1): hooks.json names one module, and index.ts is the
// one place that registers an event two features hook. `agent.report` is hooked by the after-work
// check and by the status pane's tracking, so index.ts registers it once, calls `next` once, and
// gives each feature its own part: a failure in one must never drop the other's result.
// The test's own hooks sit beneath the plugin and stand for Claude Code.
import type { AgentReportInput, On } from 'claude-code'
import { expect, test } from 'claude-code/testing'

const CORE = { plugin: 'engine', tier: 'core' } as const
const BLOCK = 'After-work check'

type World = {
  /** Makes `$.clock.now` fail, the way the review found it on a merged tree. */
  isClockBroken?: boolean
  /** Makes `$.state.set` fail. */
  isStateBroken?: boolean
  /** Makes the after-work switch lookup fail, so the after-work part throws. */
  isAfterworkBroken?: boolean
}

/** Stands for Claude Code beneath the plugin, and records what the mods ask of it. */
function engine(on: On, w: World = {}) {
  const seen = { statuses: [] as (string | undefined)[], reportRuns: 0 }
  on('agent.report', (_, e) => {
    seen.reportRuns += 1
    return { text: e.text }
  })
  on('env.get', (_, e) => (w.isAfterworkBroken && e.name === 'ANTHOLOGY_AFTERWORK' ? { deny: 'env is down' } : { value: undefined }))
  on('clock.now', () => (w.isClockBroken ? { deny: 'clock is down' } : { value: 1_000 }))
  if (w.isStateBroken) on('state.set', () => ({ deny: 'state is down' }) as never)
  on('agent.list', () => ({ value: [] }))
  on('ui.status', (_, e) => {
    seen.statuses.push(e.text)
    return { value: undefined }
  })
  return seen
}

const writer = (text: string): AgentReportInput => ({
  agentId: 'a1',
  subagentType: 'anthology:implementer',
  provider: CORE,
  status: 'completed',
  text,
})

// A report with no commit and no test evidence: the after-work check has two gaps to name.
const GAPPY = 'done'

test('a report gets the after-work block and is tracked by the pane, from one registration', async ($, on) => {
  const seen = engine(on)
  const r = await $.agent.report(writer(GAPPY))
  expect(r.text).toContain(BLOCK)
  expect(r.text?.startsWith(`${GAPPY}\n\n`)).toBe(true)
  expect(seen.reportRuns).toBe(1)
  expect(seen.statuses).toEqual([undefined])
})

test('tracking that throws on the clock leaves the after-work block', async ($, on) => {
  const seen = engine(on, { isClockBroken: true })
  const r = await $.agent.report(writer(GAPPY))
  expect(r.text).toContain(BLOCK)
  expect(seen.reportRuns).toBe(1)
})

test('tracking that throws on the state leaves the after-work block', async ($, on) => {
  const seen = engine(on, { isStateBroken: true })
  const r = await $.agent.report(writer(GAPPY))
  expect(r.text).toContain(BLOCK)
  expect(seen.reportRuns).toBe(1)
})

test('an after-work check that throws leaves the report as core made it, and the pane still tracks it', async ($, on) => {
  const seen = engine(on, { isAfterworkBroken: true })
  const r = await $.agent.report(writer(GAPPY))
  expect(r.text).toBe(GAPPY)
  expect(seen.reportRuns).toBe(1)
  expect(seen.statuses).toEqual([undefined])
})
