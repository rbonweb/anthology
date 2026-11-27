// Automatic after-work check (post-release mods design §4). When a writing anthology agent
// reports to the lead, string checks name what the report does not say, and the findings are
// appended below the agent's own text, which is never rewritten or cut. No model call and no
// spawned agent: the check reads the report, plus at most one `git status` (see dirtyFiles).
//
// A block is raised only by a real gap, so that a block is worth reading:
//   - no commit (a sha next to commit or HEAD wording, a lettered sha, or the word "committed");
//   - no test evidence (a test word, with "not run" and "skipped" counted as none);
//   - attribution lines in the report;
//   - uncommitted files in the agent's own worktree (from git).
// Two items are reminders, never gaps. They ride in a block a gap raised and never raise one:
//   - the project map, when the report names a new, removed or renamed piece and does not deal
//     with the map (updated it, "map not needed", or "no map" / "map is missing" when the map
//     file is really absent in the agent's folder, which `$.fs.exists` tells);
//   - leftovers, when the report starts containers, volumes or temp files and does not say they
//     were removed. Never for the docs-writer.
// A report with no gap is delivered as written, with no block.
// The commit and test gaps do not apply when every path the report edited is outside a git repo
// (see editsOnlyOutsideRepo): a settings file in the home folder has nothing to commit or test.
//
// Off: ANTHOLOGY_AFTERWORK=0 turns this check off, ANTHOLOGY_MODS=0 turns every anthology mod off.
import type { AgentReportInput, AgentReportResult, EngineInterface, ProcessRunResult } from 'claude-code'
import { modsOff, onAbsolutePath } from './util.ts'

/** The agent types that write, as `agent.report` names them. Reviewer and scout only read. */
const WRITERS: ReadonlySet<string> = new Set([
  'anthology:implementer',
  'anthology:debugger',
  'anthology:docs-writer',
  'anthology:ui-designer',
])

/** The one git call must end well inside the hook's 10 s budget. */
const GIT_TIMEOUT_MS = 2000

/** The most gits on the PATH tried when one cannot start: 3 x 2 s stays inside the 10 s budget. */
const MAX_GITS = 3

/** The most folders mapIsThere looks in, from the agent's cwd upward. */
const MAX_WALK = 32

// Commit. A lettered sha stands alone; an all-digit one only beside commit, HEAD or sha wording.
const SHA = /\b(?=[0-9a-f]*[0-9])(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\b/
const SHA_WORDED = /\b(?:commit(?:ted)?|head|sha)\b[^\n]{0,20}?\b[0-9a-f]{7,40}\b/i
const NOT_COMMITTED = /\b(?:not|never|nothing|no)\s+committed\b/gi
const COMMITTED = /\bcommitted\b/i

// Test evidence. The negations are cut out first, so word order does not matter.
const TESTED = /\b(?:tests?|tested|testing|pytest|jest|vitest|clippy|lint\w*|tsc|validat\w+)\b/i
const NOT_TESTED: readonly RegExp[] = [
  /\b(?:not|never|no|without)\s+(?:\w+\s+){0,2}(?:tests?|tested|testing)\b/gi,
  /\btests?\b[^.\n]{0,20}\b(?:not|never)\s+(?:run|ran|executed)\b/gi,
  /\btests?\s*[:-]?\s*(?:none|n\/a|skipped|not run)\b/gi,
  /\b(?:skipped|skipping)\s+(?:the\s+)?tests?\b/gi,
  /\btests?\b[^.\n]{0,20}\b(?:were|was|are|is)\s+skipped\b/gi,
]

const ATTRIBUTION = /^[\s>*\-\u{1F916}]*(?:co-authored-by:|generated with\b)[^\n]*(?:claude|anthropic)/imu

// Map reminder: a named piece changed, and the map not dealt with.
const REWIRED =
  /\b(?:new|added|created|removed|deleted|renamed|moved|rewired)\b[^.\n]{0,40}?(?:\b(?:modules?|commands?|hooks?|agents?|crates?|scripts?|skills?|mods?)\b|[\w/-]+\.(?:ts|tsx|js|jsx|rs|py|sh|md|json|toml|ya?ml)\b)/i
const MAP_DEALT_WITH =
  /project[- ]map|reference-project-map|\bmap\b[^.\n]{0,20}\b(?:not needed|unneeded|updated|exists)\b/i
// A claim that the map is not there: "no map yet", "the project map is missing", or the Read error an
// agent quotes ("reference-project-map.md: not found", "... failed: File does not exist", ENOENT).
// Where it matches anywhere, only the file decides (see mapIsThere): the words around it, which
// often name the project map themselves, never stand for dealing with it. A word spans a sentence
// at most 20 characters and never crosses a `.`; the file name carries its own `.md`.
const MAP_WORD = String.raw`(?:project[- ])?map\b`
const MAP_NAMED = String.raw`reference-project-map(?:\.md)?\b`
const MAP_GONE = String.raw`(?:missing|absent|not found|not there|(?:does|do|did)\s+not\s+exist|(?:doesn't|don't|didn't)\s+exist|no such file|ENOENT)`
const MAP_ABSENT = new RegExp(
  [
    String.raw`\bno (?:project[- ])?map\b`,
    String.raw`\b${MAP_WORD}[^.\n]{0,20}\b${MAP_GONE}`,
    String.raw`\b${MAP_GONE}\b[^.\n]{0,20}\b${MAP_WORD}`,
    String.raw`${MAP_NAMED}[^.\n]{0,25}\b${MAP_GONE}`,
    String.raw`\b(?:ENOENT|no such file)\b[^\n]{0,80}?${MAP_NAMED}`,
    String.raw`${MAP_NAMED}[:\s]{0,3}(?:read\s+)?failed\b`,
  ].join('|'),
  'i',
)
const MAP_FILE = '.claude/memory/reference-project-map.md'

// Leftover reminder: something was started, and no removal said.
const STARTED =
  /\bdocker (?:run|compose|build)\b|\b(?:started|spun up|created|launched|ran|used|built)\b[^.\n]{0,60}\b(?:containers?|volumes?|temp(?:orary)? (?:files?|dirs?|folders?)|tmp)\b/i
const THING = '(?:containers?|volumes?|images?|temp(?:orary)?|tmp|scratch|leftovers?|worktrees?)'
const DID = '(?:remov|delet|clean|prun|stopp)'
const CLEANED = new RegExp(
  `\\b${DID}\\w*[^.\\n]{0,80}\\b${THING}\\b|\\b${THING}\\b[^.\\n]{0,80}\\b${DID}|\\bno (?:leftovers|temp)|\\bnothing (?:was )?(?:left|to clean)|\\bleft nothing\\b`,
  'i',
)

function without(text: string, res: readonly RegExp[]): string {
  return res.reduce((t, re) => t.replace(re, ' '), text)
}

/**
 * What the report text says about itself. `gaps` raise the block; `reminders` ride in it. `kind` is
 * the agent type, which decides whether leftovers are asked about at all. `mapExists` is whether the
 * map file is in the agent's folder: only `false` lets a "map missing" claim count; `undefined`
 * (not known) does not. `outsideRepo` is true when every path the report edited is outside a git
 * repo (see editsOnlyOutsideRepo): there is nothing to commit and no test to run, so the commit
 * and test gaps do not apply.
 */
export function check(
  text: string,
  kind: string,
  mapExists?: boolean,
  outsideRepo = false,
): { gaps: string[]; reminders: string[] } {
  const gaps: string[] = []
  const reminders: string[] = []
  const commitText = without(text, [NOT_COMMITTED])
  if (!outsideRepo && !SHA.test(commitText) && !SHA_WORDED.test(commitText) && !COMMITTED.test(commitText)) {
    gaps.push('no commit sha or "committed" in the report')
  }
  if (!outsideRepo && !TESTED.test(without(text, NOT_TESTED))) gaps.push('nothing about what was tested')
  if (ATTRIBUTION.test(text)) {
    gaps.push('the report carries attribution lines (Co-Authored-By / Generated with); commits and PRs must not')
  }
  // A claim that the map is absent is judged by the file alone, never by leftover mentions of the
  // map: it deals with the map only when the file is known to be absent.
  const mapDealt = MAP_ABSENT.test(text) ? mapExists === false : MAP_DEALT_WITH.test(text)
  if (REWIRED.test(text) && !mapDealt) {
    reminders.push('the report names new, removed or renamed pieces: update the project map, or say it needs none')
  }
  if (kind !== 'anthology:docs-writer' && STARTED.test(text) && !CLEANED.test(text)) {
    reminders.push('the report mentions containers, volumes or temp files but not their removal: say what was removed')
  }
  return { gaps, reminders }
}

/** Where a path really lands (links followed), else the path itself; no trailing slash. */
async function real($: EngineInterface, path: string): Promise<string> {
  const stat = await $.fs.stat(path, { resolve: true }).catch(() => undefined)
  return (stat?.realPath ?? path).replace(/\/+$/, '')
}

/**
 * What the agent's own worktree holds uncommitted, as a phrase, or undefined when there is nothing
 * or it is not known. The event carries no path, so the folder comes from `$.agent.list()` for the
 * event's `agentId`, and only when it is not the session's own folder (by real path, against both
 * the session's cwd and its root): there the files are the lead's, not the agent's. One
 * `git status`, no shell, killed after GIT_TIMEOUT_MS. The pathspecs are taken from the repo top,
 * so a subfolder cwd sees the whole tree, and leave out `.claude/agent-memory`, which agents are
 * told to leave for the lead. Output cut at the cap reads as "many". Any failure (no agent in the
 * list, a timeout, a git error, a non-zero exit) is undefined, so the text-only check stands.
 * Takes `$`, so it is a top-level function of this file.
 */
async function dirtyFiles($: EngineInterface, agentId: string): Promise<string | undefined> {
  try {
    const agent = (await $.agent.list()).find(a => a.id === agentId)
    if (!agent?.cwd) return undefined
    const dir = await real($, agent.cwd)
    const session = await Promise.all([$.session.cwd(), $.session.root()].map(async p => real($, await p)))
    if (session.includes(dir)) return undefined
    // The worktree is the agent's own, so git is looked up on the absolute PATH entries only: a
    // `.` or empty entry would run a `git` planted in it. The child gets that PATH too.
    // A git that cannot start (not executable, as `$.fs.stat` cannot tell) is passed over for the
    // next on the PATH, as the shell does; a few at most, so the hook's budget holds.
    const git = await onAbsolutePath($, 'git')
    let run: ProcessRunResult | undefined
    for (const bin of git.bins.slice(0, MAX_GITS)) {
      try {
        run = await $.process.run(
          [
            bin, '-c', 'core.fsmonitor=false', '--no-optional-locks', 'status', '--porcelain',
            '--', ':(top)', ':(top,exclude).claude/agent-memory',
          ],
          { cwd: dir, timeoutMs: GIT_TIMEOUT_MS, env: { PATH: git.path } },
        )
        break
      } catch {
        // cannot start, or timed out: the next one
      }
    }
    if (run === undefined || run.exitCode !== 0) return undefined
    const n = run.stdout.split('\n').filter(l => l.trim() !== '').length
    if (run.isStdoutTruncated) return 'many uncommitted changes in the agent\'s worktree (git status)'
    if (n === 0) return undefined
    return `${n} uncommitted change${n === 1 ? '' : 's'} in the agent's worktree (git status)`
  } catch {
    return undefined
  }
}

/**
 * Whether the project map file is in the agent's own folder, asked only when the report names a
 * piece and claims the map is not there (nothing else needs the answer). The folder comes from
 * `$.agent.list()` as in dirtyFiles; it may be the session folder, which is then the agent's. An
 * agent working in a subfolder of its repo has the map above it, so the folders are walked upward
 * from the cwd, up to and including the first one that holds `.git` (a folder or, in a worktree, a
 * file); the map is there if any of them has it. Not known (no such agent, no folder, a failing
 * lookup) is undefined, which leaves the claim unproven.
 */
async function mapIsThere($: EngineInterface, agentId: string, text: string): Promise<boolean | undefined> {
  if (!REWIRED.test(text) || !MAP_ABSENT.test(text)) return undefined
  try {
    const agent = (await $.agent.list()).find(a => a.id === agentId)
    if (!agent?.cwd) return undefined
    let dir = agent.cwd.replace(/\/+$/, '')
    for (let depth = 0; depth < MAX_WALK && dir !== ''; depth++) {
      if (await $.fs.exists(`${dir}/${MAP_FILE}`)) return true
      if (await $.fs.exists(`${dir}/.git`)) return false
      dir = dir.slice(0, Math.max(0, dir.lastIndexOf('/')))
    }
    return false
  } catch {
    return undefined
  }
}

// Edited paths, as the report names them: absolute (`/a/b`, two folders at least, so `/plugin` and
// `/anthology:setup` are not paths), `~/x`, and relative ones (`./x`, `../x`, or a name ending in a
// source or config extension). A URL is none: a path never starts right after `:`, `/`, `.` or a word.
const SEG = String.raw`[\w.@+%-]+`
const NOT_AFTER = String.raw`(?<![\w:/.~@+%-])`
const PATH_TOKENS: readonly RegExp[] = [
  new RegExp(String.raw`${NOT_AFTER}~?(?:/${SEG})+`, 'g'),
  new RegExp(String.raw`${NOT_AFTER}(?:\.{1,2}/)?(?:${SEG}/)*${SEG}\.(?:tsx?|jsx?|rs|py|sh|md|json|toml|ya?ml)\b`, 'g'),
  new RegExp(String.raw`${NOT_AFTER}\.{1,2}/${SEG}(?:/${SEG})*`, 'g'),
]

/** The most distinct paths looked at; a report that names more keeps the checks. */
const MAX_PATHS = 20

function pathsIn(text: string): string[] {
  const found = new Set<string>()
  for (const re of PATH_TOKENS) {
    for (const m of text.matchAll(re)) {
      const p = m[0].replace(/[.,;:]+$/, '')
      const folders = p.split('/').filter(s => s !== '').length
      if (p.startsWith('/') ? folders >= 2 : true) found.add(p)
    }
  }
  return [...found]
}

/** `a/./b//c/../d` as `a/b/d`, for an absolute path. */
function normalize(path: string): string {
  const out: string[] = []
  for (const s of path.split('/')) {
    if (s === '' || s === '.') continue
    if (s === '..') out.pop()
    else out.push(s)
  }
  return `/${out.join('/')}`
}

/** Whether the path, or a folder above it, holds `.git` (a folder or, in a worktree, a file). */
async function inGitRepo($: EngineInterface, path: string): Promise<boolean> {
  let dir = path
  for (let depth = 0; depth < MAX_WALK; depth++) {
    if (await $.fs.exists(`${dir === '/' ? '' : dir}/.git`)) return true
    if (dir === '/') return false
    dir = dir.slice(0, Math.max(1, dir.lastIndexOf('/')))
  }
  return true // too deep to tell: keep the checks
}

/**
 * True when the report names edited paths and every one is outside a git repo, as for
 * `~/.claude/settings.json`: there is nothing to commit and no test to run. A relative path is
 * taken from the agent's folder (`$.agent.list()`), a `~` path from HOME. A report that names no
 * path, a path in a repo, or one that cannot be placed (no agent folder, no HOME, a failing
 * lookup, more than MAX_PATHS paths) is false, so the checks stay. A mixed report is false too:
 * the checks then speak for the in-repo paths.
 */
async function editsOnlyOutsideRepo($: EngineInterface, agentId: string, text: string): Promise<boolean> {
  const paths = pathsIn(text)
  if (paths.length === 0 || paths.length > MAX_PATHS) return false
  try {
    const home = (await $.env.get('HOME'))?.replace(/\/+$/, '')
    let cwd: string | undefined
    for (const p of paths) {
      let abs: string
      if (p.startsWith('/')) abs = p
      else if (p.startsWith('~/')) {
        if (!home?.startsWith('/')) return false
        abs = `${home}${p.slice(1)}`
      } else {
        cwd ??= (await $.agent.list()).find(a => a.id === agentId)?.cwd
        if (!cwd?.startsWith('/')) return false
        abs = `${cwd}/${p}`
      }
      if (await inGitRepo($, normalize(abs))) return false
    }
    return true
  } catch {
    return false
  }
}

/**
 * This feature's part of the one `agent.report` hook (index.ts registers the event once, calls
 * `next` once and hands `r`, what it resolved to, to each feature). Returns `r` as it is, or `r`
 * with the block appended. It may throw: index.ts then keeps `r`.
 */
export async function afterworkReport($: EngineInterface, e: AgentReportInput, r: AgentReportResult): Promise<AgentReportResult> {
  if (r.consumed !== undefined) return r
  if (e.parentAgentId !== undefined || e.status !== 'completed' || !WRITERS.has(e.subagentType)) return r
  if (r.text.trim() === '') return r
  if ((await modsOff($)) || (await $.env.get('ANTHOLOGY_AFTERWORK')) === '0') return r
  const mapExists = await mapIsThere($, e.agentId, r.text)
  let result = check(r.text, e.subagentType, mapExists)
  // Asked only when a gap stands: edits outside any git repo have no commit and no test to report.
  if (result.gaps.length > 0 && (await editsOnlyOutsideRepo($, e.agentId, r.text))) {
    result = check(r.text, e.subagentType, mapExists, true)
  }
  const { gaps, reminders } = result
  const dirty = await dirtyFiles($, e.agentId)
  if (dirty !== undefined) gaps.push(dirty)
  if (gaps.length === 0) return r
  const block = [
    'After-work check (anthology; string checks on the report above, which is unchanged):',
    ...gaps.map(g => `- ${g}`),
    ...reminders.map(m => `Reminder: ${m}`),
  ].join('\n')
  // Keep core's review and warning: they describe the report as the agent wrote it.
  return { ...r, text: `${r.text}\n\n${block}` }
}
