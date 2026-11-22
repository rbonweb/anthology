// alerts.tsx (post-release mods design §5, item 4): the alert on a Notification, the status pane and
// its command, and the status line. The test's own hooks sit beneath the plugin and stand for
// Claude Code: they answer the `$` calls the mod makes (toast, sound, status, pane, command) and
// record them, and `mock.clock` and `mock.env` stand for the clock and the environment.
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
  /** What `$.audio.play` does beneath the mod: plays, or refuses as a machine with no player does. */
  isPlayable?: boolean
  /** What `$.ui.open` answers: a placed pane, or the reason it could not be. */
  openReason?: string
}

/** Stands for Claude Code beneath the plugin, and records what the mod asks of it. */
function engine(on: On, { env = {}, isPlayable = true, openReason }: Options = {}) {
  const clock = mock.clock(on, { now: T0 })
  mock.env(on, env)
  const seen = {
    toasts: [] as { text: string; timeoutMs?: number }[],
    sounds: [] as unknown[],
    statuses: [] as (string | undefined)[],
    opened: [] as unknown[],
    commands: [] as unknown[],
    notifications: 0,
  }
  let next = 0
  on('ui.toast', (_, e) => {
    seen.toasts.push(e)
    return { value: undefined }
  })
  on('audio.play', (_, e) => {
    seen.sounds.push(e.clip)
    return isPlayable ? { value: undefined } : { deny: 'no audio player' }
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
  on('agent.spawn', (_, e) => (e.subagentType === 'denied' ? { deny: 'no' } : { model: 'm', agentId: `a${++next}` }))
  on('agent.report', (_, e) => ({ text: e.text }))
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

const hhmm = (ms: number) => {
  const d = new Date(ms)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

// ---- Alerts ---------------------------------------------------------------------------------------

test('a permission prompt plays the sound and shows a toast, and the user hooks still run', async ($, on) => {
  const { seen } = engine(on)
  await notify($, 'permission_prompt')
  expect(seen.toasts).toEqual([{ text: 'Claude needs your approval', timeoutMs: 8000 }])
  expect(seen.sounds).toEqual([{ asset: 'sounds/attention.wav' }])
  expect(seen.notifications).toBe(1)
})

test('an idle prompt alerts with its own line', async ($, on) => {
  const { seen } = engine(on)
  await notify($, 'idle_prompt')
  expect(seen.toasts.map(t => t.text)).toEqual(['Claude is waiting for you'])
  expect(seen.sounds).toHaveLength(1)
})

test('a notification that does not need the person raises no alert', async ($, on) => {
  const { seen } = engine(on)
  await notify($, 'auth_success')
  expect(seen.toasts).toHaveLength(0)
  expect(seen.sounds).toHaveLength(0)
  expect(seen.notifications).toBe(1)
})

test('at most one alert per 20 seconds', async ($, on) => {
  const { clock, seen } = engine(on)
  await notify($, 'permission_prompt')
  await clock.advance(19_999)
  await notify($, 'idle_prompt')
  expect(seen.toasts).toHaveLength(1)
  expect(seen.sounds).toHaveLength(1)
  await clock.advance(1)
  await notify($, 'idle_prompt')
  expect(seen.toasts.map(t => t.text)).toEqual(['Claude needs your approval', 'Claude is waiting for you'])
  expect(seen.sounds).toHaveLength(2)
  // An alert held back does not restart the window.
  await clock.advance(10_000)
  await notify($, 'permission_prompt')
  expect(seen.toasts).toHaveLength(2)
})

test('a sound that cannot play does not fail the alert', async ($, on) => {
  const { seen } = engine(on, { isPlayable: false })
  await notify($, 'permission_prompt')
  expect(seen.toasts).toHaveLength(1)
  expect(seen.notifications).toBe(1)
})

test('ANTHOLOGY_NOTIFY=0 turns the alert off and leaves the pane on', async ($, on) => {
  const { seen } = engine(on, { env: { ANTHOLOGY_NOTIFY: '0' } })
  await notify($, 'permission_prompt')
  expect(seen.toasts).toHaveLength(0)
  expect(seen.sounds).toHaveLength(0)
  await spawn($, 'Build it')
  expect(seen.statuses).toEqual(['1 agent running'])
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
  const { seen } = engine(on)
  await spawn($, 'Build the alerts mod')
  for (const surface of SURFACES) {
    expect(await pane($, surface)).toEqual(['Running (1)', `${hhmm(T0)}  implementer  Build the alerts mod`])
  }
  expect(seen.statuses).toEqual(['1 agent running'])
})

test('the pane lists many agents and says how many it leaves out', async ($, on) => {
  const { seen } = engine(on)
  for (let i = 1; i <= 25; i++) await spawn($, `Task ${i}`, i % 2 ? 'anthology:scout' : 'general-purpose')
  const shown = await pane($)
  expect(shown[0]).toBe('Running (25)')
  expect(shown[1]).toBe(`${hhmm(T0)}  scout  Task 1`)
  expect(shown[2]).toBe(`${hhmm(T0)}  general-purpose  Task 2`)
  expect(shown).toHaveLength(22)
  expect(shown.at(-1)).toBe('and 5 more')
  expect(seen.statuses.at(-1)).toBe('25 agents running')
})

test('a report moves the agent from running to finished, with how it ended', async ($, on) => {
  const { clock, seen } = engine(on)
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
  expect(seen.statuses).toEqual(['1 agent running', '2 agents running', '1 agent running', undefined])
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
  const { seen } = engine(on)
  const base = { tool_use_id: 't', prompt: 'p', description: 'x', subagentType: 'general-purpose', provider: CORE, parentModel: 'm', background: false, fork: false } as const
  await $.agent.spawn({ ...base, workflow: { runId: 'wf_1', agentIndex: 1 } })
  await $.agent.spawn({ ...base, isTeammate: true })
  expect(seen.statuses).toEqual([])
  expect(await pane($)).toEqual(['Running (0)', 'No agents yet. They show here while they work.'])
})

test('a refused spawn is not tracked', async ($, on) => {
  const { seen } = engine(on)
  await $.agent.spawn({ tool_use_id: 't', prompt: 'p', description: 'x', subagentType: 'denied', provider: CORE, parentModel: 'm', background: false, fork: false })
  expect(seen.statuses).toEqual([])
})

test('a report reaches the parent as the engine made it', async ($, on) => {
  engine(on)
  const a = await spawn($, 'Build it')
  expect(await report($, a, 'completed')).toEqual({ text: 'done' })
})

// ---- Permission prompts -----------------------------------------------------------------------------

test('a permission prompt shows first, in the pane and the status line, until its call ends', async ($, on) => {
  const { seen } = engine(on)
  await spawn($, 'Build it')
  await ask($, 'git push origin p8-alerts-pane')
  expect(await pane($)).toEqual([
    'Needs your approval (1)',
    'Bash  git push origin p8-alerts-pane',
    'Running (1)',
    `${hhmm(T0)}  implementer  Build it`,
  ])
  expect(seen.statuses.at(-1)).toBe('1 agent running, 1 waiting for approval')
  await $.classic.PostToolUse({ tool_name: 'Bash', tool_input: { command: 'git push origin p8-alerts-pane' }, tool_response: '', tool_use_id: 'toolu_9' })
  expect((await pane($))[0]).toBe('Running (1)')
  expect(seen.statuses.at(-1)).toBe('1 agent running')
})

test('a prompt another hook answered is not pending', async ($, on) => {
  const { seen } = engine(on)
  await $.classic.PermissionRequest({ tool_name: 'Read', tool_input: { file_path: '/w/a' } })
  expect(seen.statuses).toEqual([])
  expect((await pane($))[0]).toBe('Running (0)')
})

test('a prompt that waits for a subagent names the agent', async ($, on) => {
  engine(on)
  await $.classic.PermissionRequest({ tool_name: 'Edit', tool_input: { file_path: '/w/a.txt' }, agent_id: 'a7', agent_type: 'anthology:implementer' })
  expect((await pane($))[1]).toBe('Edit  /w/a.txt (implementer)')
})

test('a denied call and a finished turn close the prompts', async ($, on) => {
  const { seen } = engine(on)
  await ask($, 'rm x')
  await $.classic.PermissionDenied({ tool_name: 'Bash', tool_input: { command: 'rm x' }, tool_use_id: 't1', reason: 'no' })
  expect(seen.statuses.at(-1)).toBeUndefined()
  await ask($, 'rm y')
  await ask($, 'rm z')
  expect(seen.statuses.at(-1)).toBe('2 waiting for approval')
  await $.classic.Stop({ stop_hook_active: false })
  expect(seen.statuses.at(-1)).toBeUndefined()
  expect((await pane($))[0]).toBe('Running (0)')
})

test('a call that fails closes its prompt', async ($, on) => {
  const { seen } = engine(on)
  await ask($, 'make')
  await $.classic.PostToolUseFailure({ tool_name: 'Bash', tool_input: { command: 'make' }, tool_use_id: 't2', error: 'exit 2' })
  expect(seen.statuses.at(-1)).toBeUndefined()
})

test('a call with another input leaves the prompt open', async ($, on) => {
  const { seen } = engine(on)
  await ask($, 'make')
  await $.classic.PostToolUse({ tool_name: 'Bash', tool_input: { command: 'ls' }, tool_response: '', tool_use_id: 't3' })
  expect(seen.statuses.at(-1)).toBe('1 waiting for approval')
})

test('a long command is cut to one line', async ($, on) => {
  engine(on)
  await ask($, `echo ${'x'.repeat(300)}\nsecond line`)
  const row = (await pane($))[1] as string
  expect(row.startsWith('Bash  echo xxx')).toBe(true)
  expect(row.length).toBeLessThan(140)
  expect(row.includes('\n')).toBe(false)
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
