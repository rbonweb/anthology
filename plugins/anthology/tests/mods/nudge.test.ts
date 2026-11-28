// nudge.ts (phase 2 safety plan, task 3): on `agent.report` in the main session of a long build, the
// mod appends one reminder to start the tasks whose inputs are published. It reads the SDD ledger
// under `<root>/.superpowers/sdd` and the plan the ledger names, counts the agents `$.agent.list()`
// shows, and keeps its last reminder in `$.state` so it is not repeated. The test's own hooks sit
// beneath the plugin and stand for Claude Code: `agent.report` is the engine's delivery of the text,
// `fs.*` the repo, `agent.list` the running agents, and `mock.clock` and `mock.env` the clock and the
// environment. The reports are a reviewer's, which the after-work check never looks at.
import type { AgentReportInput, On } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'

const T0 = Date.UTC(2026, 10, 28, 12, 0, 0)
const HOUR = 3_600_000
const MINUTE = 60_000
const ROOT = '/work/repo'
const SDD = `${ROOT}/.superpowers/sdd`
const FOLDER = '2026-11-28-phase2'
const LEDGER_FROM_ROOT = `.superpowers/sdd/${FOLDER}/progress.md`
const PLAN_FROM_ROOT = `.superpowers/sdd/${FOLDER}/plan.md`
const PLAN = `${ROOT}/${PLAN_FROM_ROOT}`

/** Five tasks, ids as the plan's own headings spell them. */
const PLAN_TEXT = ['# Plan', ...['0', '1', '2', '3', '4'].map(n => `### Task ${n}: thing ${n}\nFiles: x`)].join('\n\n')

const header = `# SDD ledger — plan: ${PLAN_FROM_ROOT} (branch \`build\`)`
const ledgerWith = (...lines: string[]) => [header, ...lines].join('\n')
/** One task done. */
const ONE_DONE = ledgerWith('- Task 0: complete (abc1234)')

type World = {
  env?: Record<string, string>
  /** Whether `<root>/.superpowers/sdd` exists; by default it does. */
  hasSdd?: boolean
  /** The folder under `.superpowers/sdd` that holds the ledger. */
  folder?: string
  /** The ledger's text, and when it last changed; the plan exists unless `noPlan`. */
  ledger?: string
  mtimeMs?: number
  noPlan?: boolean
  /** The agents the engine lists, as `[id, status]`; by default the reporter and two more run. */
  agents?: readonly (readonly [string, string])[]
  /** What the engine's own `agent.report` answers, when it is not the text. */
  consumed?: string
  /** Makes `$.fs.list` reject. */
  isListBroken?: boolean
}

const RUNNING_TWO = [['a1', 'running'], ['a2', 'running'], ['a3', 'running']] as const

/** Stands for Claude Code beneath the plugin, and records what the mod asks of it. */
function engine(on: On, w: World = {}) {
  const seen = { exists: [] as string[], lists: 0, agentLists: 0 }
  const clock = mock.clock(on, { now: T0 })
  mock.env(on, { ANTHOLOGY_AFTERWORK: '0', ...w.env })
  on('agent.report', (_$, e) => (w.consumed === undefined ? { text: e.text } : { consumed: w.consumed }))
  on('session.root', () => ({ value: ROOT }))
  on('fs.exists', (_$, e) => {
    seen.exists.push(e.path)
    return { value: (w.hasSdd ?? true) && (e.path === SDD || (e.path === PLAN && !w.noPlan)) }
  })
  on('fs.list', () => {
    seen.lists += 1
    if (w.isListBroken) return { deny: 'fs is down' }
    return { value: [{ name: w.folder ?? FOLDER, kind: 'dir' as const, size: 0, mtimeMs: 0, isLink: false }] }
  })
  const ledgerPath = () => `${SDD}/${w.folder ?? FOLDER}/progress.md`
  on('fs.stat', (_$, e) => {
    if (e.path !== ledgerPath()) throw new Error(`ENOENT: ${e.path}`)
    return { value: { kind: 'file' as const, size: 100, mtimeMs: w.mtimeMs ?? T0 - HOUR, isLink: false } }
  })
  on('fs.read', (_$, e) => {
    if (e.path === ledgerPath()) return { value: w.ledger ?? ONE_DONE }
    if (e.path === PLAN) return { value: PLAN_TEXT }
    throw new Error(`ENOENT: ${e.path}`)
  })
  on('agent.list', () => {
    seen.agentLists += 1
    return { value: (w.agents ?? RUNNING_TWO).map(([id, status]) => ({ id, status })) as never }
  })
  // `w` is the world the test may change between two reports.
  return { seen, clock, world: w }
}

const report = (over: Partial<AgentReportInput> = {}): AgentReportInput => ({
  agentId: 'a1',
  subagentType: 'anthology:reviewer',
  provider: { plugin: 'anthology', tier: 'user' },
  status: 'completed',
  text: 'Reviewed: no findings.',
  ...over,
})

/** Five tasks, one done, so four are not started; two others run, so two could start. */
const REMINDER =
  `Anthology reminder: 2 agent(s) running, 2 task(s) of plan.md not started. Start each task whose inputs ` +
  `are published now, one agent per task (large-builds, Start early). If the rest must wait, add a line ` +
  `"Serial: <reason>" to ${LEDGER_FROM_ROOT}; this reminder then stops.`

test('no_sdd_folder_costs_one_check', async ($, on) => {
  const { seen } = engine(on, { hasSdd: false })
  const r = await $.agent.report(report())
  expect(r.text).toBe('Reviewed: no findings.')
  expect(seen.exists).toEqual([SDD])
  expect(seen.lists).toBe(0)
  expect(seen.agentLists).toBe(0)
})

test('nudges_when_two_running_and_tasks_left', async ($, on) => {
  const { seen } = engine(on)
  const r = await $.agent.report(report())
  expect(r.text).toBe(`Reviewed: no findings.\n\n${REMINDER}`)
  expect(seen.agentLists).toBe(1)
})

test('a_task_the_ledger_completes_twice_counts_once', async ($, on) => {
  engine(on, { ledger: ledgerWith('- Task 0: complete (abc1234)', '- Task 0: complete again after the fix') })
  expect((await $.agent.report(report())).text).toBe(`Reviewed: no findings.\n\n${REMINDER}`)
})

test('silent_with_three_running', async ($, on) => {
  engine(on, { agents: [...RUNNING_TWO, ['a4', 'waiting']] })
  expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
})

test('silent_when_every_pending_task_has_an_agent', async ($, on) => {
  // Three tasks done, two left, and two others run them: nothing is left to start.
  engine(on, { ledger: ledgerWith('- Task 0: complete', '- Task 1: complete', '- Task 2: complete') })
  expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
})

test('an agent that ended does not count as running', async ($, on) => {
  engine(on, { agents: [['a1', 'running'], ['a2', 'completed'], ['a3', 'failed'], ['a4', 'killed'], ['a5', 'idle']] })
  const r = await $.agent.report(report())
  expect(r.text).toContain('Anthology reminder: 0 agent(s) running, 4 task(s) of plan.md not started.')
})

test('silent_for_subagent_reports_and_consumed', async ($, on) => {
  const { seen } = engine(on)
  expect((await $.agent.report(report({ parentAgentId: 'lead-sub' }))).text).toBe('Reviewed: no findings.')
  expect(seen.exists).toEqual([])
})

test('silent_for_a_consumed_report', async ($, on) => {
  const { seen } = engine(on, { consumed: 'taken by another plugin' })
  const r = await $.agent.report(report())
  expect(r.consumed).toBe('taken by another plugin')
  expect(r.text).toBeUndefined()
  expect(seen.exists).toEqual([])
})

test('silent_on_stale_ledger_or_missing_plan', async ($, on) => {
  engine(on, { mtimeMs: T0 - 7 * HOUR })
  expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
})

test('silent_when_the_ledger_names_no_plan_file', async ($, on) => {
  engine(on, { noPlan: true })
  expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
})

test('silent_when_the_ledger_has_no_plan_line', async ($, on) => {
  engine(on, { ledger: '# Notes\n- Task 0: complete' })
  expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
})

test('a_ledger_just_inside_six_hours_still_nudges', async ($, on) => {
  engine(on, { mtimeMs: T0 - 6 * HOUR + MINUTE })
  expect((await $.agent.report(report())).text).toContain('Anthology reminder:')
})

test('rate_limits_and_serial_opt_out', async ($, on) => {
  // One other agent runs, so down to three tasks left there is one that could start.
  const { clock, world } = engine(on, { agents: [['a1', 'running'], ['a2', 'running']] })
  const nudged = async () => ((await $.agent.report(report())).text ?? '').includes('Anthology reminder:')
  expect(await nudged()).toBe(true)
  // The same number of tasks left: silent, however long it has been.
  expect(await nudged()).toBe(false)
  await clock.advance(25 * MINUTE)
  expect(await nudged()).toBe(false)
  // A task finished, so the count moved. The last reminder is 25 minutes old: it shows, once.
  world.ledger = ledgerWith('- Task 0: complete', '- Task 1: complete')
  expect(await nudged()).toBe(true)
  expect(await nudged()).toBe(false)
  // The count moved again, but the last reminder is under 20 minutes old.
  world.ledger = ledgerWith('- Task 0: complete', '- Task 1: complete', '- Task 2: complete')
  await clock.advance(19 * MINUTE)
  expect(await nudged()).toBe(false)
  await clock.advance(2 * MINUTE)
  expect(await nudged()).toBe(true)
  // A Serial line ends it, whatever the count.
  world.ledger = ledgerWith('- Task 0: complete', 'Serial: every task edits the same file')
  await clock.advance(60 * MINUTE)
  expect(await nudged()).toBe(false)
})

test('a_reminder_about_another_ledger_does_not_rate_limit_this_one', async ($, on) => {
  const { world } = engine(on)
  const nudged = async () => ((await $.agent.report(report())).text ?? '').includes('Anthology reminder:')
  expect(await nudged()).toBe(true)
  expect(await nudged()).toBe(false)
  // A new build: its ledger lives in another folder, with the same number of tasks left.
  world.folder = '2026-11-29-next'
  expect(await nudged()).toBe(true)
})

for (const [name, env] of [
  ['ANTHOLOGY_NUDGE=0', { ANTHOLOGY_NUDGE: '0' }],
  ['ANTHOLOGY_MODS=0', { ANTHOLOGY_MODS: '0' }],
  ['an embedded host (attended 0)', { CLAUDE_CODE_SESSION_ATTENDED: '0' }],
  ['an embedded host (sdk entrypoint)', { CLAUDE_CODE_ENTRYPOINT: 'sdk-ts' }],
] as const) {
  test(`switch_and_embedded_host_off: ${name}`, async ($, on) => {
    const { seen } = engine(on, { env })
    expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
    expect(seen.exists).toEqual([])
  })
}

test('an_error_leaves_the_report_as_it_was', async ($, on) => {
  engine(on, { isListBroken: true })
  expect((await $.agent.report(report())).text).toBe('Reviewed: no findings.')
})
