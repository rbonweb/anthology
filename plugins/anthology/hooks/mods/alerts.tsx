// Alerts and a live status pane (post-release mods design §5, item 4).
//
// Alerts: when Claude Code needs the person (a permission prompt, or an idle wait for input) the
// mod shows a one-line toast, at most once per 20 s. It makes no sound: the shell hook
// hooks/notify.sh keeps the only one, and it is also the alert on a Claude Code without function
// hooks.
//
// Status pane: `/anthology-status` opens a pane that lists the agents that run (agent.spawn and
// agent.report, the latter hooked by index.ts), the last few that finished, and the permission prompts that wait for an answer.
// A row above the prompt ("● 2 running   ✓ 3 done   ! 1 needs you", with the command on the right)
// keeps the counts in view; it shows only while something runs or waits on the person. The pane and
// the row draw with Box and Text only, which every surface has, one row per Text so the terminal
// cuts a long row at its edge.
//
// Privacy: the pane and the toasts name a tool and an agent's type and description, never what a
// tool call does (no command, file path or URL), because a pane may be shown on a shared screen.
//
// What the pane draws lives in `$.state` (`anthology.status`, declared in types/anthology.d.ts),
// never in a module variable, so a hot reload keeps it and a write redraws the pane.
// ANTHOLOGY_MODS=0 turns everything here off; ANTHOLOGY_NOTIFY=0 turns off the alert alone, as it
// does for notify.sh.
//
// Nothing here may throw once a hook has called `next`: the hook's `.catch` would call `next`
// again and replay a result that drops what the hooks beneath it returned. So every tracking call
// goes through `quietly`, which swallows its failure: the pane may be stale, the event is not hurt.
// The one exception is `alertsReport`, the part of the shared `agent.report` hook: index.ts calls
// it inside its own try/catch (`isolated`), which is the wrapper there.
import { atom, read, update } from 'claude-code'
import type { AgentReportInput, AgentSpawnInput, EngineInterface, On, PluginState, ShapedValue } from 'claude-code'
import { modsOff } from './util.ts'

type Status = ShapedValue<PluginState['anthology']['status']>
type AnthologyRun = Status['running'][number]
type AnthologyFinished = Status['finished'][number]
type AnthologyPrompt = Status['prompts'][number]

const PANE = 'anthology-status'
const COMMAND = 'anthology-status'
const ALERT_GAP_MS = 20_000
const TOAST_MS = 8_000
const REASONS: Readonly<Record<string, string>> = {
  permission_prompt: 'Claude needs your approval',
  idle_prompt: 'Claude is waiting for you',
}

// Bounds, so a long session never grows the state without end.
const KEEP_RUNNING = 60
const KEEP_FINISHED = 5
const KEEP_PROMPTS = 20
// What the pane shows of a list before it says "and N more".
const SHOW_RUNNING = 20
const SHOW_PROMPTS = 5

// Bump the shape tag when a row's fields change: a value an earlier version stored reads as empty.
const status = atom({ plugin: 'anthology', key: 'status' } as const, { running: [], finished: [], prompts: [] }, { shape: 'status-1' })

// ---- Text helpers (pure) ------------------------------------------------------------------------

/** One short line of free text: whitespace folded, cut to `max` characters. */
export function oneLine(text: string, max = 120): string {
  const flat = text.replace(/\s+/g, ' ').trim()
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat
}

/** `anthology:implementer` shows as `implementer`: the pane is short of columns, not of context. */
export function shortType(type: string): string {
  const i = type.lastIndexOf(':')
  return i >= 0 && i < type.length - 1 ? type.slice(i + 1) : type
}

/** A start time as HH:MM on the clock of the machine the hooks run on, which is the person's. */
export function clock(ms: number): string {
  const d = new Date(ms)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** A span as `42s`, `3m 12s` or `1h 05m`. */
export function span(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`
}

type BandPart = {
  id: 'running' | 'done' | 'asks'
  /** One cell wide, so the row reads the same without colour. */
  glyph: string
  text: string
}

/**
 * The counts the row above the prompt shows, in the order running, done, needs you; a zero count is
 * left out. Empty (no row at all) when nothing runs and nothing waits on the person. Done alone
 * shows nothing: the finished list is capped at five and never ages out, so a row for it would
 * stay up for the rest of the session after the work ended. The pane still lists what finished.
 */
export function bandParts(runningCount: number, doneCount: number, waitingCount: number): BandPart[] {
  if (runningCount === 0 && waitingCount === 0) return []
  const parts: BandPart[] = []
  if (runningCount > 0) parts.push({ id: 'running', glyph: '●', text: `${runningCount} running` })
  if (doneCount > 0) parts.push({ id: 'done', glyph: '✓', text: `${doneCount} done` })
  if (waitingCount > 0) parts.push({ id: 'asks', glyph: '!', text: `${waitingCount} needs you` })
  return parts
}

/** The gap between two counts in the row. */
const BAND_GAP = '   '
/** Cells kept free at the row's two edges (paddingX 1 each side). */
const BAND_EDGES = 2

/** Runs `work` and swallows its failure; see the note at the top of this file. */
async function quietly(work: () => Promise<unknown>): Promise<void> {
  try {
    await work()
  } catch {
    // A stale pane is better than a lost event.
  }
}

/**
 * A short key for one tool call's input, to match a prompt to the call that ends it. Only the key
 * is kept: the input itself is never stored or drawn.
 */
function promptKey(tool: string, agent: string | undefined, input: unknown): string {
  const text = `${tool}\n${agent ?? ''}\n${JSON.stringify(input) ?? ''}`
  let h = 0x811c9dc5
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return (h >>> 0).toString(16)
}

// ---- The pane's model (pure) --------------------------------------------------------------------

type Tone = 'ok' | 'failed' | 'dim'
type PaneRow = {
  /** A leading status word, coloured by `tone`; padded so a column lines up in a monospaced face. */
  lead?: { text: string; tone: Tone }
  /** A dim figure: a start time or a duration. */
  meta?: string
  /** Who or what: an agent type or a tool name, drawn bold. */
  who: string
  text: string
}
type PaneSection = {
  id: 'asks' | 'running' | 'finished'
  title: string
  rows: PaneRow[]
  more: number
  /** The dim line under the title when `rows` is empty. */
  empty?: string
}

/** What the pane shows for a state: approvals first, since they stop everything, then the rest. */
export function paneSections(
  run: readonly AnthologyRun[],
  done: readonly AnthologyFinished[],
  asks: readonly AnthologyPrompt[],
): PaneSection[] {
  const sections: PaneSection[] = []
  if (asks.length > 0) {
    sections.push({
      id: 'asks',
      title: `Needs your approval (${asks.length})`,
      rows: asks.slice(0, SHOW_PROMPTS).map(a => ({
        who: a.tool,
        text: a.agent !== undefined ? `from ${shortType(a.agent)}` : '',
      })),
      more: Math.max(0, asks.length - SHOW_PROMPTS),
    })
  }
  const isEmpty = run.length === 0 && done.length === 0 && asks.length === 0
  sections.push({
    id: 'running',
    title: `Running (${run.length})`,
    rows: run.slice(0, SHOW_RUNNING).map(r => ({
      meta: clock(r.startedAt),
      who: shortType(r.type),
      text: oneLine(r.description),
    })),
    more: Math.max(0, run.length - SHOW_RUNNING),
    empty: isEmpty ? 'No agents yet. They show here while they work.' : 'None.',
  })
  if (done.length > 0) {
    sections.push({
      id: 'finished',
      title: 'Finished',
      rows: done.map(d => ({
        lead:
          d.status === 'completed'
            ? { text: 'ok'.padEnd(7), tone: 'ok' }
            : { text: d.status === 'failed' ? 'failed'.padEnd(7) : 'stopped', tone: d.status === 'failed' ? 'failed' : 'dim' },
        meta: d.startedAt !== undefined ? span(d.endedAt - d.startedAt) : '',
        who: shortType(d.type),
        text: oneLine(d.description),
      })),
      more: 0,
    })
  }
  return sections
}

// ---- State changes (each takes `$`, so each is a top-level function) -----------------------------

const ENDED = new Set<string>(['completed', 'failed', 'killed'])

/**
 * The running rows the engine still lists as alive. A run that ended without an agent.report (a
 * workflow's, a killed one) is no longer listed or is listed as ended, and its row goes. Where the
 * list cannot be read the rows stand as they are.
 */
async function liveRunning($: EngineInterface, rows: readonly AnthologyRun[]): Promise<readonly AnthologyRun[]> {
  try {
    const alive = new Set((await $.agent.list()).filter(a => !ENDED.has(a.status)).map(a => a.id))
    return rows.filter(r => alive.has(r.id))
  } catch {
    return rows
  }
}

async function trackSpawn($: EngineInterface, e: AgentSpawnInput, agentId: string): Promise<void> {
  const row: AnthologyRun = {
    id: agentId,
    type: e.subagentType,
    description: oneLine(e.description),
    startedAt: await $.clock.now(),
  }
  await update($, status, s => ({
    ...s,
    running: [...s.running.filter(r => r.id !== agentId), row].slice(-KEEP_RUNNING),
  }))
}

async function trackReport($: EngineInterface, e: AgentReportInput): Promise<void> {
  const endedAt = await $.clock.now()
  // One write moves the row from running to finished, so the pane never shows it in both or neither.
  await update($, status, s => {
    const seen: AnthologyRun | undefined = s.running.find(r => r.id === e.agentId)
    const row: AnthologyFinished = {
      id: e.agentId,
      type: seen?.type ?? e.subagentType,
      description: seen?.description ?? '',
      ...(seen !== undefined ? { startedAt: seen.startedAt } : {}),
      endedAt,
      status: e.status,
    }
    return {
      // The agent is done, so a prompt it still held open is over too.
      prompts: s.prompts.filter(p => p.agentId !== e.agentId),
      running: s.running.filter(r => r.id !== e.agentId),
      finished: [row, ...s.finished.filter(d => d.id !== e.agentId)].slice(0, KEEP_FINISHED),
    }
  })
}

type ToolRef = { tool_name: string; tool_input: unknown; agent_id?: string; agent_type?: string }

async function addPrompt($: EngineInterface, e: ToolRef): Promise<void> {
  const row: AnthologyPrompt = {
    key: promptKey(e.tool_name, e.agent_id, e.tool_input),
    tool: e.tool_name,
    ...(e.agent_id !== undefined ? { agentId: e.agent_id, ...(e.agent_type !== undefined ? { agent: e.agent_type } : {}) } : {}),
    at: await $.clock.now(),
  }
  await update($, status, s => ({ ...s, prompts: [...s.prompts, row].slice(-KEEP_PROMPTS) }))
}

/** A tool call that ended answers its prompt, if it had one: drop the first with the same key. */
async function endPrompt($: EngineInterface, e: ToolRef): Promise<void> {
  const key = promptKey(e.tool_name, e.agent_id, e.tool_input)
  await update($, status, s => {
    const i = s.prompts.findIndex(p => p.key === key)
    return i >= 0 ? { ...s, prompts: [...s.prompts.slice(0, i), ...s.prompts.slice(i + 1)] } : s
  })
}

/**
 * The lead's turn ended: drops the lead's prompts that an answer never ended (a denied call). A
 * subagent's prompt stays, since the agent may still wait on it; its agent.report closes it.
 */
async function clearPrompts($: EngineInterface): Promise<void> {
  await update($, status, s =>
    s.prompts.some(p => p.agentId === undefined) ? { ...s, prompts: s.prompts.filter(p => p.agentId !== undefined) } : s,
  )
}

/**
 * Shows the toast for `type`, unless the last alert was under 20 s ago or the type is not one that
 * needs the person. Returns when the last alert was, for the next call.
 */
async function alertUser($: EngineInterface, type: string, lastAt: number | undefined): Promise<number | undefined> {
  const reason = REASONS[type]
  if (reason === undefined || (await modsOff($)) || (await $.env.get('ANTHOLOGY_NOTIFY')) === '0') return lastAt
  const now = await $.clock.now()
  if (lastAt !== undefined && now - lastAt < ALERT_GAP_MS) return lastAt
  $.ui.toast(reason, { timeoutMs: TOAST_MS })
  return now
}

// ---- Registration -------------------------------------------------------------------------------

/**
 * This feature's part of the one `agent.report` hook (index.ts registers the event once, calls
 * `next` once and hands each feature `e`): moves the agent from running to finished. It may throw:
 * index.ts isolates it, so a failure here never drops what the other features made of the report.
 */
export async function alertsReport($: EngineInterface, e: AgentReportInput): Promise<void> {
  if (!(await modsOff($))) await trackReport($, e)
}

export function registerAlerts(on: On): void {
  // When the last alert fired. A hot reload or a restart of the hooks worker forgets it, which at
  // worst lets one alert through early.
  let lastAlertAt: number | undefined
  // Set before the first await, so two notifications that arrive together raise one alert.
  let isAlerting = false

  // The Notification event that settings hooks hear (notify.sh among them) reaches here first;
  // `next` hands it on, so those hooks still run.
  on('classic.Notification', async ($, e, next) => {
    if (REASONS[e.notification_type] !== undefined && !isAlerting) {
      isAlerting = true
      await quietly(async () => {
        lastAlertAt = await alertUser($, e.notification_type, lastAlertAt)
      })
      isAlerting = false
    }
    return next(e)
  }).catch(($, e, next) => next(e))

  on('session.start', async ($, e, next) => {
    if (!(await modsOff($))) {
      await $.command.register({ name: COMMAND, description: 'Show running agents and pending approvals in a pane' })
    }
    return next(e)
  }).catch(($, e, next) => next(e))

  on('command.run', { command: COMMAND }, async ($, e, next) => {
    if (await modsOff($)) return next(e)
    const opened = await $.ui.open({ id: PANE, title: 'Anthology' })
    return {
      text: opened.isPlaced ? 'Anthology status pane opened.' : `The status pane did not open: ${opened.reason}`,
      isTextDisplayOnly: true,
    }
  }).catch(($, e, next) => next(e))

  on('agent.spawn', async ($, e, next) => {
    const started = await next(e)
    // A workflow agent and a teammate send no agent.report, so a row for one would never end.
    const agentId = started.agentId
    if (agentId !== undefined && e.workflow === undefined && e.isTeammate === undefined) {
      await quietly(async () => {
        if (!(await modsOff($))) await trackSpawn($, e, agentId)
      })
    }
    return started
  }).catch(($, e, next) => next(e))

  // `agent.report` is not registered here: afterwork.ts hooks it too, and a second registration of
  // one event throws and unloads the module. index.ts registers it once and calls `alertsReport`.

  // A prompt is pending when no hook answered the permission request: the dialog shows next.
  on('classic.PermissionRequest', async ($, e, next) => {
    const answer = await next(e)
    if (answer.decision === undefined) {
      await quietly(async () => {
        if (!(await modsOff($))) await addPrompt($, e)
      })
    }
    return answer
  }).catch(($, e, next) => next(e))

  // The call ends one of three ways, each of which closes its prompt.
  on('classic.PostToolUse', async ($, e, next) => {
    await quietly(async () => {
      if (!(await modsOff($))) await endPrompt($, e)
    })
    return next(e)
  }).catch(($, e, next) => next(e))
  on('classic.PostToolUseFailure', async ($, e, next) => {
    await quietly(async () => {
      if (!(await modsOff($))) await endPrompt($, e)
    })
    return next(e)
  }).catch(($, e, next) => next(e))
  on('classic.PermissionDenied', async ($, e, next) => {
    await quietly(async () => {
      if (!(await modsOff($))) await endPrompt($, e)
    })
    return next(e)
  }).catch(($, e, next) => next(e))
  // A prompt the person turned down ends no call; the end of the lead's turn is the last word on
  // it. A subagent's prompt waits for that agent's report instead.
  on('classic.Stop', async ($, e, next) => {
    await quietly(async () => {
      if (!(await modsOff($))) await clearPrompts($)
    })
    return next(e)
  }).catch(($, e, next) => next(e))

  // The row above the prompt: `● 2 running   ✓ 3 done   ! 1 needs you        /anthology-status`.
  // Box and Text only, so every surface draws it. It is the mod's own row, not a status entry, so
  // Claude Code adds no plugin label to it. Where nothing runs or waits there is no row.
  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    if (e.props.hasSurvey || (await modsOff($))) return next(e)
    const { running, finished, prompts } = await read($, status)
    const parts = bandParts((await liveRunning($, running)).length, finished.length, prompts.length)
    if (parts.length === 0) return next(e)
    const { Box, Text } = $.ui.resolve(e)
    const hint = `/${COMMAND}`
    const used = parts.reduce((n, p) => n + `${p.glyph} ${p.text}`.length, 0) + BAND_GAP.length * (parts.length - 1)
    // A narrow band (a phone) keeps the counts and drops the hint.
    const hasHint = used + hint.length + BAND_GAP.length + BAND_EDGES <= e.props.bodyColumns

    return (
      <Box width="100%" justifyContent="space-between" paddingX={1}>
        <Text wrap="truncate-end">
          {parts.map((p, i) => (
            <Text>
              {i > 0 && BAND_GAP}
              <Text color={p.id === 'running' ? 'suggestion' : p.id === 'asks' ? 'warning' : undefined} dimColor={p.id === 'done'}>
                {p.glyph}
              </Text>
              <Text color={p.id === 'asks' ? 'warning' : undefined} dimColor={p.id !== 'asks'}>
                {` ${p.text}`}
              </Text>
            </Text>
          ))}
        </Text>
        {hasHint && <Text dimColor>{hint}</Text>}
      </Box>
    )
  }).catch(($, e, next) => next(e))

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e, next) => {
    if (await modsOff($)) return next(e)
    const { Box, Text } = $.ui.resolve(e)
    const { running, finished, prompts } = await read($, status)
    const alive = await liveRunning($, running)

    return (
      <Box flexDirection="column" paddingX={1}>
        {paneSections(alive, finished, prompts).map((section, i) => (
          <Box flexDirection="column" marginTop={i === 0 ? 0 : 1}>
            <Text bold color={section.id === 'asks' ? 'permission' : undefined}>
              {section.title}
            </Text>
            {section.rows.length === 0 && <Text dimColor>{section.empty}</Text>}
            {section.rows.map(row => (
              <Text wrap="truncate-end">
                {row.lead !== undefined && (
                  <Text color={row.lead.tone === 'ok' ? 'success' : row.lead.tone === 'failed' ? 'error' : undefined} dimColor={row.lead.tone === 'dim'}>
                    {row.lead.text}
                  </Text>
                )}
                {row.lead !== undefined && '  '}
                {row.meta !== undefined && row.meta !== '' && <Text dimColor>{row.meta}</Text>}
                {row.meta !== undefined && row.meta !== '' && '  '}
                <Text bold>{row.who}</Text>
                {row.text !== '' && `  ${row.text}`}
              </Text>
            ))}
            {section.more > 0 && <Text dimColor>and {section.more} more</Text>}
          </Box>
        ))}
      </Box>
    )
  }).catch(($, e, next) => next(e))
}
