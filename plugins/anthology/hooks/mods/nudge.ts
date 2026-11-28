// Parallel work reminder (phase 2 safety plan, task 3). During a long build the lead tends to run one
// task at a time when more could start. When an agent reports, this adds one reminder below the
// report, in the main session only, if the repo keeps an SDD ledger (`.superpowers/sdd/<folder>/
// progress.md`), the ledger is live, its plan has tasks not started, at most two other agents run
// and at least one task could start now. It reads the files and `$.agent.list()` only: no model
// call, no spawned process. It is one part of the shared `agent.report` hook that index.ts
// registers, so it exports a function and registers nothing.
//
// A repo with no `.superpowers/sdd` folder costs one `$.fs.exists`. The reminder shows at most once
// every 20 minutes and not again for the same number of tasks left on the same ledger (the last
// one is kept in `$.state`, key `nudge`). A ledger line that starts `Serial:` ends it.
//
// Off: ANTHOLOGY_NUDGE=0 turns this off, ANTHOLOGY_MODS=0 turns every anthology mod off. It fails
// open: index.ts runs it in `isolated`, so an error leaves the report as it was.
import type { AgentReportInput, AgentReportResult, EngineInterface } from 'claude-code'
import { isEmbeddedHost } from './delegation.ts'
import { modsOff } from './util.ts'

const last = { plugin: 'anthology', key: 'nudge' } as const

/** A ledger that did not change in this long is an old build's. */
const LIVE_MS = 6 * 3_600_000
/** The same ledger is not reminded again within this long. */
const REPEAT_MS = 20 * 60_000
/** The most other agents running for the reminder to show: with more, the lead is already parallel. */
const MAX_ACTIVE = 2
/** The most folders of `.superpowers/sdd` looked at, newest name first. */
const MAX_FOLDERS = 100

const LEDGER_HEAD = /^# SDD ledger \S plan: (\S+)/
const SERIAL = /^Serial:/m
const TASK_HEADING = /^### Task ([A-Za-z0-9.-]+):/gm
const TASK_DONE = /Task ([A-Za-z0-9.-]+): complete/g
/** The agent statuses that hold a slot: started, or about to be, or waiting on something. */
const BUSY: ReadonlySet<string> = new Set(['pending', 'running', 'waiting'])

/** The newest `<sdd>/<folder>/progress.md` by change time, or undefined when there is none. */
async function newestLedger($: EngineInterface, sdd: string): Promise<{ path: string; mtimeMs: number } | undefined> {
  const names = (await $.fs.list(sdd))
    .filter(f => f.kind === 'dir' && !/\p{Cc}/u.test(f.name))
    .map(f => f.name)
    .sort()
    .reverse()
    .slice(0, MAX_FOLDERS)
  let newest: { path: string; mtimeMs: number } | undefined
  for (const name of names) {
    const path = `${sdd}/${name}/progress.md`
    const stat = await $.fs.stat(path).catch(() => undefined)
    if (stat?.kind === 'file' && (newest === undefined || stat.mtimeMs > newest.mtimeMs)) {
      newest = { path, mtimeMs: stat.mtimeMs }
    }
  }
  return newest
}

/**
 * This feature's part of the one `agent.report` hook (index.ts registers the event once, calls
 * `next` once and hands `r`, what it resolved to, to each feature). Returns `r` as it is, or `r`
 * with the reminder appended. It may throw: index.ts then keeps `r`.
 */
export async function nudgeReport($: EngineInterface, e: AgentReportInput, r: AgentReportResult): Promise<AgentReportResult> {
  if (r.consumed !== undefined || e.parentAgentId !== undefined) return r
  if ((await modsOff($)) || (await $.env.get('ANTHOLOGY_NUDGE')) === '0' || (await isEmbeddedHost($))) return r

  const root = (await $.session.root()).replace(/\/+$/, '')
  const sdd = `${root}/.superpowers/sdd`
  if (!(await $.fs.exists(sdd))) return r

  const now = await $.clock.now()
  const ledger = await newestLedger($, sdd)
  if (ledger === undefined || now - ledger.mtimeMs > LIVE_MS) return r

  const text = await $.fs.read(ledger.path)
  const plan = LEDGER_HEAD.exec(text)?.[1]
  if (plan === undefined || SERIAL.test(text)) return r
  const planPath = plan.startsWith('/') ? plan : `${root}/${plan}`
  if (!(await $.fs.exists(planPath))) return r

  const total = [...(await $.fs.read(planPath)).matchAll(TASK_HEADING)].length
  const done = new Set([...text.matchAll(TASK_DONE)].map(m => m[1])).size
  const pending = total - done
  const active = (await $.agent.list()).filter(a => a.id !== e.agentId && BUSY.has(a.status)).length
  const startable = pending - active
  if (active > MAX_ACTIVE || startable < 1) return r

  const { value: before, version } = await $.state.get(last)
  if (before?.ledger === ledger.path && (before.pending === pending || now - before.at < REPEAT_MS)) return r
  // Compare and set: of two reports that arrive together, one shows the reminder.
  const set = await $.state.set(last, { ledger: ledger.path, pending, at: now }, { ifVersion: version })
  if (!set.isSet) return r

  const name = (plan.split('/').pop() ?? plan).replace(/\p{Cc}/gu, '?').slice(0, 80)
  const line =
    `Anthology reminder: ${active} agent(s) running, ${startable} task(s) of ${name} not started. ` +
    `Start each task whose inputs are published now, one agent per task (large-builds, Start early). ` +
    `If the rest must wait, add a line "Serial: <reason>" to ${ledger.path.slice(root.length + 1)}; this reminder then stops.`
  return { ...r, text: `${r.text}\n\n${line}` }
}
