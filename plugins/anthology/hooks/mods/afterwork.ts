// Automatic after-work check (post-release mods design §4). When a writing anthology agent
// reports to the lead, string checks name what the report does not say, and the findings are
// appended below the agent's own text, which is never rewritten or cut. No model call and no
// spawned agent: the check reads the report, plus at most one `git status` (see dirtyFiles).
//
// A report that carries all five items is delivered as written, with no block: the lead reads
// nothing extra on the usual path. The block lists only what is missing, so a block means a gap.
//
// Off: ANTHOLOGY_AFTERWORK=0 turns this check off, ANTHOLOGY_MODS=0 turns every anthology mod off.
import type { EngineInterface, On } from 'claude-code'
import { modsOff } from './util.ts'

/** The agent types that write, as `agent.report` names them. Reviewer and scout only read. */
const WRITERS: ReadonlySet<string> = new Set([
  'anthology:implementer',
  'anthology:debugger',
  'anthology:docs-writer',
  'anthology:ui-designer',
])

/** The one git call must end well inside the hook's 10 s budget. */
const GIT_TIMEOUT_MS = 2000

/** A git sha: 7 to 40 lowercase hex digits with both a digit and a letter, so a number is not one. */
const SHA = /\b(?=[0-9a-f]*[0-9])(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\b/
const NOT_COMMITTED = /\b(?:not|never|nothing|no)\s+committed\b/gi
const COMMITTED = /\bcommitted\b/i

const TESTED = /\b(?:tests?|tested|testing|pytest|jest|vitest|clippy|lint\w*|tsc|validat\w+)\b/i
const NOT_TESTED = [
  /\b(?:not|never|no|without)\s+(?:\w+\s+){0,2}(?:tests?|tested|testing)\b/gi,
  /\btests?\s*[:-]?\s*(?:none|n\/a|skipped)\b/gi,
]

const THING = '(?:containers?|volumes?|images?|temp(?:orary)?|tmp|scratch|leftovers?|worktrees?)'
const DID = '(?:remov|delet|clean|prun|stopp)'
const CLEANED = new RegExp(
  `\\b${DID}\\w*[^.\\n]{0,80}\\b${THING}\\b|\\b${THING}\\b[^.\\n]{0,80}\\b${DID}|\\bno (?:leftovers|temp)|\\bnothing (?:was )?left\\b|\\bleft nothing\\b`,
  'i',
)

const MAP = /project[- ]map|reference-project-map|\bno map\b/i

/** A line that starts with an attribution trailer or footer naming the assistant or its maker. */
const ATTRIBUTION = /^[\s>*\-\u{1F916}]*(?:co-authored-by:|generated with\b)[^\n]*(?:claude|anthropic)/imu

function without(text: string, res: readonly RegExp[]): string {
  return res.reduce((t, re) => t.replace(re, ' '), text)
}

/** What the report text leaves out, one phrase per gap; empty when the report has all five. */
export function findGaps(text: string): string[] {
  const gaps: string[] = []
  const commitText = without(text, [NOT_COMMITTED])
  if (!SHA.test(commitText) && !COMMITTED.test(commitText)) {
    gaps.push('no commit sha or "committed" in the report')
  }
  if (!TESTED.test(without(text, NOT_TESTED))) gaps.push('nothing about what was tested')
  if (!CLEANED.test(text)) gaps.push('no mention of leftovers removed (containers, volumes, temp files)')
  if (!MAP.test(text)) gaps.push('no mention of the project map (updated, or that none exists)')
  if (ATTRIBUTION.test(text)) {
    gaps.push('the report carries attribution lines (Co-Authored-By / Generated with); commits and PRs must not')
  }
  return gaps
}

/**
 * How many files the agent's own worktree holds uncommitted, or 0 when that is not known. The
 * event carries no path, so the folder comes from `$.agent.list()` for the event's `agentId`, and
 * only when it is not the session's own folder (there the files are the lead's, not the agent's).
 * One `git status`, no shell, killed after GIT_TIMEOUT_MS. Any failure (no agent in the list, a
 * timeout, a git error, a non-zero exit) is 0, so the text-only check stands. Takes `$`, so it is
 * a top-level function of this file.
 */
async function dirtyFiles($: EngineInterface, agentId: string): Promise<number> {
  try {
    const agent = (await $.agent.list()).find(a => a.id === agentId)
    const dir = agent?.cwd.replace(/\/+$/, '')
    if (!dir || dir === (await $.session.cwd()).replace(/\/+$/, '')) return 0
    const run = await $.process.run(
      ['git', '-c', 'core.fsmonitor=false', '--no-optional-locks', 'status', '--porcelain'],
      { cwd: dir, timeoutMs: GIT_TIMEOUT_MS },
    )
    if (run.exitCode !== 0) return 0
    return run.stdout.split('\n').filter(l => l.trim() !== '').length
  } catch {
    return 0
  }
}

export function registerAfterwork(on: On): void {
  on('agent.report', async ($, e, next) => {
    const r = await next(e)
    if (r.consumed !== undefined) return r
    if (e.parentAgentId !== undefined || e.status !== 'completed' || !WRITERS.has(e.subagentType)) return r
    if (r.text.trim() === '') return r
    if ((await modsOff($)) || (await $.env.get('ANTHOLOGY_AFTERWORK')) === '0') return r
    const gaps = findGaps(r.text)
    const dirty = await dirtyFiles($, e.agentId)
    if (dirty > 0) gaps.push(`${dirty} uncommitted change${dirty === 1 ? '' : 's'} in the agent's worktree (git status)`)
    if (gaps.length === 0) return r
    const block = [
      'After-work check (anthology; string checks on the report above, which is unchanged):',
      ...gaps.map(g => `- ${g}`),
    ].join('\n')
    return { text: `${r.text}\n\n${block}` }
  }).catch(($, e, next) => next(e))
}
