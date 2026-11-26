// alerts.tsx (post-release mods design §5, item 4): that no alert is raised on a Notification, the
// status pane and its command, and that nothing is drawn above the prompt. The test's own hooks sit
// beneath the plugin and stand for Claude Code: they answer the `$` calls the mod makes (toast,
// sound, status, pane, command) and record them, and `mock.clock` and `mock.env` stand for the
// clock and the environment.
import type { On } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'
import type { Engine } from 'claude-code/testing'

const T0 = Date.UTC(2026, 10, 22, 12, 0, 0)
const CORE = { plugin: 'engine', tier: 'core' } as const
const SURFACES = ['terminal', 'desktop', 'vscode', 'mobile'] as const
type Surface = (typeof SURFACES)[number]

const PANE = {
  plugin: 'anthology',
  component: 'Pane',
  requestId: 'anthology-status',
  props: {
    title: 'Anthology',
    isFocused: false,
    bodyColumns: 48,
    placement: 'dock',
    scroll: { offset: 0, bodyRows: 20 },
    view: {},
  },
} as const

const RUN_COMMAND = { command: 'anthology-status', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 120 } } as const

type Options = {
  env?: Readonly<Record<string, string>>
  /** What `$.ui.open` answers: a placed pane, or the reason it could not be. */
  openReason?: string
  /** Leaves `$.clock.now` unanswered, so every call of it rejects. */
  isClockBroken?: boolean
  /** Makes `$.agent.list` reject. */
  isListBroken?: boolean
}

/** Stands for Claude Code beneath the plugin, and records what the mod asks of it. */
function engine(on: On, { env = {}, openReason, isClockBroken = false, isListBroken = false }: Options = {}) {
  // A broken clock is one whose `now` is refused; no test of it moves the clock.
  const clock = isClockBroken ? (on('clock.now', () => ({ deny: 'clock is down' })), undefined as never) : mock.clock(on, { now: T0 })
  // These tests are about alerts and the pane. The after-work check, which the merged module also
  // runs on a writer's report, is off here and has its own tests (afterwork.test.ts, integration.test.ts).
  mock.env(on, { ANTHOLOGY_AFTERWORK: '0', ...env })
  const seen = {
    toasts: [] as { text: string; timeoutMs?: number }[],
    sounds: [] as unknown[],
    /** The agents the engine lists as alive: those spawned and not yet reported. */
    alive: new Set<string>(),
    statuses: [] as (string | undefined)[],
    opened: [] as unknown[],
    commands: [] as unknown[],
    notifications: 0,
    /** How often the engine's own agent.report ran: a hook that fails after `next` must not rerun it. */
    reportRuns: 0,
  }
  let next = 0
  on('ui.toast', (_, e) => {
    seen.toasts.push(e)
    return { value: undefined }
  })
  on('audio.play', (_, e) => {
    seen.sounds.push(e.clip)
    return { value: undefined }
  })
  on('ui.status', (_, e) => {
    seen.statuses.push(e.text)
    return { value: undefined }
  })
  on('ui.open', (_, e) => {
    seen.opened.push(e)
    return { value: openReason === undefined ? { isPlaced: true } : { isPlaced: false, reason: openReason } }
  })
  on('command.register', (_, e) => {
    seen.commands.push(e)
    return { value: { command: e.name } }
  })
  on('command.run', () => ({ text: 'engine' }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('classic.Notification', () => {
    seen.notifications += 1
    return {}
  })
  // A user hook beneath the mod answers a Read prompt by itself; every other prompt stays open.
  on('classic.PermissionRequest', (_, e) => (e.tool_name === 'Read' ? { decision: { behavior: 'allow' } } : {}))
  on('classic.PostToolUse', () => ({}))
  on('classic.PostToolUseFailure', () => ({}))
  on('classic.PermissionDenied', () => ({}))
  on('classic.Stop', () => ({}))
  on('agent.spawn', (_, e) => {
    if (e.subagentType === 'denied') return { deny: 'no' }
    const agentId = `a${++next}`
    seen.alive.add(agentId)
    return { model: 'm', agentId }
  })
  on('agent.report', (_, e) => {
    seen.reportRuns += 1
    seen.alive.delete(e.agentId)
    return { text: e.text }
  })
  on('agent.list', () =>
    isListBroken ? { deny: 'no list' } : { value: [...seen.alive].map(id => ({ id, status: 'running' })) as never },
  )
  return { clock, seen }
}

const notify = ($: Engine, type: string) => $.classic.Notification({ message: 'x', notification_type: type })

async function spawn($: Engine, description: string, subagentType = 'anthology:implementer') {
  const r = await $.agent.spawn({
    tool_use_id: 'toolu_1',
    prompt: 'do it',
    description,
    subagentType,
    provider: CORE,
    parentModel: 'm',
    background: true,
    fork: false,
  })
  return r.agentId as string
}

const report = ($: Engine, agentId: string, status: 'completed' | 'failed' | 'stopped', subagentType = 'anthology:implementer') =>
  $.agent.report({ agentId, subagentType, provider: CORE, status, text: 'done' })

const ask = ($: Engine, command: string) => $.classic.PermissionRequest({ tool_name: 'Bash', tool_input: { command } })

type Node = { type?: string; children?: unknown[] }

/** Everything a node shows as one string: its text and its nested runs' text, in order. */
function plain(node: unknown): string {
  if (typeof node === 'string') return node
  const children = (node as Node).children ?? []
  return children.map(plain).join('')
}

/** One string per row the pane draws: a Text is a row, whatever runs it nests. */
function rows(node: unknown): string[] {
  if (typeof node !== 'object' || node === null) return []
  if ((node as Node).type === 'Text') return [plain(node)]
  return ((node as Node).children ?? []).flatMap(rows)
}

/** The rows of the pane as one surface draws it. */
async function pane($: Engine, surface: Surface = 'terminal') {
  const ui = await $.ui.mount({ ...PANE, surface })
  const tree = await ui.drawn()
  await ui.unmount()
  return rows(tree)
}

const ABOVE_PROMPT = {
  plugin: 'anthology',
  component: 'AbovePrompt',
  props: { hasSurvey: false, isCollapsed: false, isWorking: false, maxRows: 10, bodyColumns: 80, scroll: { offset: 0, bodyRows: 10 }, view: {} },
} as const

const hhmm = (ms: number) => {
  const d = new Date(ms)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

// ---- No alerts ------------------------------------------------------------------------------------
// Anthology sends no alert of its own: no toast, no sound and no status, whatever the notification type.

test('a permission prompt, an idle prompt and any other notification raise no toast, sound or status, and the user hooks still run', async ($, on) => {
  const { seen } = engine(on)
  for (const type of ['permission_prompt', 'idle_prompt', 'auth_success', 'elicitation_dialog']) await notify($, type)
  expect(seen.toasts).toEqual([])
  expect(seen.sounds).toEqual([])
  expect(seen.statuses).toEqual([])
  // Each event went down to the engine and the user's own hooks.
  expect(seen.notifications).toBe(4)
})

test('a notification raises no toast however long apart, and ANTHOLOGY_NOTIFY changes nothing', async ($, on) => {
  const { clock, seen } = engine(on, { env: { ANTHOLOGY_NOTIFY: '1' } })
  await notify($, 'permission_prompt')
  await clock.advance(60_000)
  await notify($, 'idle_prompt')
  expect(seen.toasts).toEqual([])
})

test('the pane still lists agents and prompts while no alert is raised', async ($, on) => {
  const { seen } = engine(on)
  await spawn($, 'Build it')
  await ask($, 'git push')
  await notify($, 'permission_prompt')
  expect(seen.toasts).toEqual([])
  expect(await pane($)).toEqual(['Needs your approval (1)', 'Bash', 'Running (1)', `${hhmm(T0)}  implementer  Build it`])
})

// ---- The command ------------------------------------------------------------------------------------

test('the command is registered at session start and opens the pane', async ($, on) => {
  const { seen } = engine(on)
  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  expect(seen.commands).toEqual([
    { name: 'anthology-status', description: 'Show running agents and pending approvals in a pane' },
  ])
  const r = await $.command.run(RUN_COMMAND)
  expect(seen.opened).toEqual([{ id: 'anthology-status', title: 'Anthology' }])
  expect(r.text).toBe('Anthology status pane opened.')
  expect(r.isTextDisplayOnly).toBe(true)
})

test('a pane that cannot be placed says why', async ($, on) => {
  engine(on, { openReason: 'no surface attached' })
  const r = await $.command.run(RUN_COMMAND)
  expect(r.text).toBe('The status pane did not open: no surface attached')
})

test('another command is left to the engine', async ($, on) => {
  const { seen } = engine(on)
  const r = await $.command.run({ ...RUN_COMMAND, command: 'compact' })
  expect(r.text).toBe('engine')
  expect(seen.opened).toEqual([])
})

// ---- The pane ---------------------------------------------------------------------------------------

test('the pane with no agents shows an empty state, on every surface', async ($, on) => {
  engine(on)
  for (const surface of SURFACES) {
    expect(await pane($, surface)).toEqual(['Running (0)', 'No agents yet. They show here while they work.'])
  }
})

test('the pane lists one running agent with its start time, type and description, on every surface', async ($, on) => {
  engine(on)
  await spawn($, 'Build the alerts mod')
  for (const surface of SURFACES) {
    expect(await pane($, surface)).toEqual(['Running (1)', `${hhmm(T0)}  implementer  Build the alerts mod`])
  }
})

test('the pane lists many agents and says how many it leaves out', async ($, on) => {
  engine(on)
  for (let i = 1; i <= 25; i++) await spawn($, `Task ${i}`, i % 2 ? 'anthology:scout' : 'general-purpose')
  const shown = await pane($)
  expect(shown[0]).toBe('Running (25)')
  expect(shown[1]).toBe(`${hhmm(T0)}  scout  Task 1`)
  expect(shown[2]).toBe(`${hhmm(T0)}  general-purpose  Task 2`)
  expect(shown).toHaveLength(22)
  expect(shown.at(-1)).toBe('and 5 more')
})

test('a report moves the agent from running to finished, with how it ended', async ($, on) => {
  const { clock } = engine(on)
  const a = await spawn($, 'Build it')
  const b = await spawn($, 'Review it', 'anthology:reviewer')
  await clock.advance(192_000)
  await report($, a, 'completed')
  await report($, b, 'failed', 'anthology:reviewer')
  expect(await pane($)).toEqual([
    'Running (0)',
    'None.',
    'Finished',
    'failed   3m 12s  reviewer  Review it',
    'ok       3m 12s  implementer  Build it',
  ])
})

test('a stopped agent reads stopped, and a report whose start was not seen still lists', async ($, on) => {
  engine(on)
  const a = await spawn($, 'Build it')
  await report($, a, 'stopped')
  await report($, 'unseen', 'completed', 'general-purpose')
  expect(await pane($)).toEqual([
    'Running (0)',
    'None.',
    'Finished',
    'ok       general-purpose',
    'stopped  0s  implementer  Build it',
  ])
})

test('the pane keeps the last five finished agents, newest first', async ($, on) => {
  engine(on)
  for (let i = 1; i <= 7; i++) await report($, `x${i}`, 'completed', `agent${i}`)
  const finished = (await pane($)).filter(r => r.startsWith('ok'))
  expect(finished).toHaveLength(5)
  expect(finished[0]).toMatch(/agent7$/)
  expect(finished[4]).toMatch(/agent3$/)
})

test('a workflow agent, a teammate and a refused spawn are not tracked', async ($, on) => {
  engine(on)
  const base = { tool_use_id: 't', prompt: 'p', description: 'x', subagentType: 'general-purpose', provider: CORE, parentModel: 'm', background: false, fork: false } as const
  await $.agent.spawn({ ...base, workflow: { runId: 'wf_1', agentIndex: 1 } })
  await $.agent.spawn({ ...base, isTeammate: true })
  expect(await pane($)).toEqual(['Running (0)', 'No agents yet. They show here while they work.'])
})

test('a refused spawn is not tracked', async ($, on) => {
  engine(on)
  await $.agent.spawn({ tool_use_id: 't', prompt: 'p', description: 'x', subagentType: 'denied', provider: CORE, parentModel: 'm', background: false, fork: false })
  expect(await pane($)).toEqual(['Running (0)', 'No agents yet. They show here while they work.'])
})

test('a report reaches the parent as the engine made it', async ($, on) => {
  engine(on)
  const a = await spawn($, 'Build it')
  expect(await report($, a, 'completed')).toEqual({ text: 'done' })
})

// ---- Permission prompts -----------------------------------------------------------------------------

test('a permission prompt shows first in the pane until its call ends', async ($, on) => {
  engine(on)
  await spawn($, 'Build it')
  await ask($, 'git push origin p8-alerts-pane')
  expect(await pane($)).toEqual([
    'Needs your approval (1)',
    'Bash',
    'Running (1)',
    `${hhmm(T0)}  implementer  Build it`,
  ])
  await $.classic.PostToolUse({ tool_name: 'Bash', tool_input: { command: 'git push origin p8-alerts-pane' }, tool_response: '', tool_use_id: 'toolu_9' })
  expect((await pane($))[0]).toBe('Running (1)')
})

test('a prompt another hook answered is not pending', async ($, on) => {
  engine(on)
  await $.classic.PermissionRequest({ tool_name: 'Read', tool_input: { file_path: '/w/a' } })
  expect((await pane($))[0]).toBe('Running (0)')
})

test('a prompt that waits for a subagent names the agent', async ($, on) => {
  engine(on)
  await $.classic.PermissionRequest({ tool_name: 'Edit', tool_input: { file_path: '/w/a.txt' }, agent_id: 'a7', agent_type: 'anthology:implementer' })
  expect((await pane($))[1]).toBe('Edit  from implementer')
})

test('a prompt never shows what the call does: no command, path or URL, on any surface', async ($, on) => {
  const { seen } = engine(on)
  const secrets = ['hunter2-token', '/home/user/secret.txt', 'https://example.test/private']
  await ask($, `curl -H "Authorization: ${secrets[0]}" x`)
  await $.classic.PermissionRequest({ tool_name: 'Edit', tool_input: { file_path: secrets[1] } })
  await $.classic.PermissionRequest({ tool_name: 'Read', tool_input: { path: secrets[1] } })
  await $.classic.PermissionRequest({ tool_name: 'WebFetch', tool_input: { url: secrets[2] } })
  await notify($, 'permission_prompt')
  for (const surface of SURFACES) {
    const shown = (await pane($, surface)).join('\n')
    for (const s of secrets) expect(shown.includes(s)).toBe(false)
    expect(shown).toContain('Needs your approval (3)')
  }
  const everything = JSON.stringify([seen.toasts, seen.statuses])
  for (const s of secrets) expect(everything.includes(s)).toBe(false)
})

test('the lead turn ending leaves a subagent prompt open until that agent reports', async ($, on) => {
  engine(on)
  const a = await spawn($, 'Build it')
  await $.classic.PermissionRequest({ tool_name: 'Edit', tool_input: { file_path: '/w/a' }, agent_id: a, agent_type: 'anthology:implementer' })
  await ask($, 'rm x')
  expect((await pane($))[0]).toBe('Needs your approval (2)')
  await $.classic.Stop({ stop_hook_active: false })
  expect((await pane($)).slice(0, 2)).toEqual(['Needs your approval (1)', 'Edit  from implementer'])
  await report($, a, 'completed')
  expect((await pane($))[0]).toBe('Running (0)')
})

test('a denied call and a finished turn close the prompts', async ($, on) => {
  engine(on)
  await ask($, 'rm x')
  await $.classic.PermissionDenied({ tool_name: 'Bash', tool_input: { command: 'rm x' }, tool_use_id: 't1', reason: 'no' })
  expect((await pane($))[0]).toBe('Running (0)')
  await ask($, 'rm y')
  await ask($, 'rm z')
  expect((await pane($))[0]).toBe('Needs your approval (2)')
  await $.classic.Stop({ stop_hook_active: false })
  expect((await pane($))[0]).toBe('Running (0)')
})

test('a call that fails closes its prompt', async ($, on) => {
  engine(on)
  await ask($, 'make')
  await $.classic.PostToolUseFailure({ tool_name: 'Bash', tool_input: { command: 'make' }, tool_use_id: 't2', error: 'exit 2' })
  expect((await pane($))[0]).toBe('Running (0)')
})

test('a call with another input leaves the prompt open', async ($, on) => {
  engine(on)
  await ask($, 'make')
  await $.classic.PostToolUse({ tool_name: 'Bash', tool_input: { command: 'ls' }, tool_response: '', tool_use_id: 't3' })
  expect((await pane($))[0]).toBe('Needs your approval (1)')
})

// ---- Agents the engine no longer lists, and a tracking call that fails -------------------------------

test('a run that ended without a report leaves the pane', async ($, on) => {
  const { seen } = engine(on)
  const a = await spawn($, 'Build it')
  await spawn($, 'Review it')
  seen.alive.delete(a)
  expect(await pane($)).toEqual(['Running (1)', `${hhmm(T0)}  implementer  Review it`])
  await ask($, 'ls')
  expect(await pane($)).toEqual(['Needs your approval (1)', 'Bash', 'Running (1)', `${hhmm(T0)}  implementer  Review it`])
})

test('where the agent list cannot be read the pane keeps its rows', async ($, on) => {
  engine(on, { isListBroken: true })
  await spawn($, 'Build it')
  expect(await pane($)).toEqual(['Running (1)', `${hhmm(T0)}  implementer  Build it`])
})

test(
  'a tracking call that fails leaves what the hooks beneath returned',
  {
    plugins: [
      {
        name: 'afterwork',
        tier: 'append',
        register(on) {
          on('agent.report', async ($, e, next) => {
            const r = await next(e)
            return { text: `${r.text ?? ''}\n+ checklist` }
          })
        },
      },
    ],
  },
  async ($, on) => {
    const { seen } = engine(on, { isClockBroken: true })
    // The clock is down, so the mod can track nothing; the events must still go through whole.
    const a = await spawn($, 'Build it')
    expect(a).toBe('a1')
    expect(await report($, a, 'completed')).toEqual({ text: 'done\n+ checklist' })
    expect(seen.reportRuns).toBe(1)
    await notify($, 'permission_prompt')
    await ask($, 'ls')
  },
)

// ---- Nothing above the prompt -----------------------------------------------------------------------

test('the old status entry is never set', async ($, on) => {
  const { seen } = engine(on)
  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  const a = await spawn($, 'Build it')
  await ask($, 'git push')
  await report($, a, 'completed')
  await $.classic.Stop({ stop_hook_active: false })
  expect(seen.statuses).toEqual([])
})

test('the mod draws nothing above the prompt, however many agents run or prompts wait', async ($, on) => {
  engine(on)
  await spawn($, 'Build it')
  await ask($, 'git push')
  const ui = await $.ui.mount({ ...ABOVE_PROMPT, surface: 'terminal' })
  const tree = await ui.drawn()
  // The hook calls next(e) and adds nothing, so what is beneath the plugin is drawn as it is.
  expect(await ui.keepsBeneath()).toBe(true)
  await ui.unmount()
  expect(rows(tree)).toEqual([])
})

// ---- Switched off -----------------------------------------------------------------------------------

test('ANTHOLOGY_MODS=0 switches everything off', async ($, on) => {
  const { seen } = engine(on, { env: { ANTHOLOGY_MODS: '0' } })
  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  await notify($, 'permission_prompt')
  const a = await spawn($, 'Build it')
  await ask($, 'git push')
  await report($, a, 'completed')
  const r = await $.command.run(RUN_COMMAND)
  expect(seen.commands).toEqual([])
  expect(seen.toasts).toEqual([])
  expect(seen.sounds).toEqual([])
  expect(seen.statuses).toEqual([])
  expect(seen.opened).toEqual([])
  // The command went to the engine, and the events went down to the engine and the user hooks.
  expect(r.text).toBe('engine')
  expect(seen.notifications).toBe(1)
})
