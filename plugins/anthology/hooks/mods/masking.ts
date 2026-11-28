// Secret masking. A Bash result goes through the installed `anthology guard mask` before Claude
// reads it, so a key or a password that a command printed never reaches the model or the transcript.
// It hooks `session.append` (door `tool-result`) alone, so index.ts calls `registerMasking` once and
// nothing here is shared. The door sees every Bash result, the main session's and each subagent's,
// before the row is stored; the other tools' results (Read, background output) are left as they are.
//
// Fast path first: a regex of the likely starts of a secret runs in this module, and the masker
// starts only when it matches, so output without a likely secret costs no process. Fails open: no
// installed binary, a binary without `guard mask`, a non-zero exit, a timeout or any error keeps
// the row as it came. ANTHOLOGY_SECRET_MASK=0 (or ANTHOLOGY_MODS=0) turns it off.
import type { EngineInterface, On } from 'claude-code'
import { modsOff, runAnthology } from './util.ts'

/** The likely start of a secret, one alternative per pattern the masker knows (G6). */
const PREFILTER = /AKIA|ASIA|gh[pousr]_|github_pat_|sk-|xox[baprs]-|PRIVATE KEY-----|password|api[_-]?key/i
/** A result longer than this is left as it is: the masker's input is capped well above it. */
const MAX_CHARS = 1_000_000
const MASK_TIMEOUT_MS = 3000

type Block = { type: string; [field: string]: unknown }

/** The masked text, or undefined when there is nothing to change (no likely match, or the masker did not answer). */
async function maskText($: EngineInterface, text: string): Promise<string | undefined> {
  if (text.length > MAX_CHARS || !PREFILTER.test(text)) return undefined
  try {
    const run = await runAnthology($, ['guard', 'mask'], text, MASK_TIMEOUT_MS)
    if (run === undefined || run.exitCode !== 0 || run.isStdoutTruncated) return undefined
    return run.stdout === '' || run.stdout === text ? undefined : run.stdout
  } catch {
    return undefined
  }
}

function isTextPart(part: unknown): part is { type: 'text'; text: string } {
  const p = part as { readonly type?: unknown; readonly text?: unknown } | null
  return typeof p === 'object' && p !== null && p.type === 'text' && typeof p.text === 'string'
}

/** One tool_result block with its text (a string, or each text block) masked; the block itself when nothing changed. */
async function maskResult($: EngineInterface, block: Block): Promise<Block> {
  const content = block.content
  if (typeof content === 'string') {
    const masked = await maskText($, content)
    return masked === undefined ? block : { ...block, content: masked }
  }
  if (!Array.isArray(content)) return block
  let changed = false
  const parts: unknown[] = []
  for (const part of content) {
    const masked = isTextPart(part) ? await maskText($, part.text) : undefined
    changed ||= masked !== undefined
    parts.push(masked === undefined ? part : { ...(part as object), text: masked })
  }
  return changed ? { ...block, content: parts } : block
}

export function registerMasking(on: On): void {
  on('session.append', { door: 'tool-result' }, async ($, e, next) => {
    if (e.origin.kind !== 'tool' || e.origin.tool !== 'Bash') return next(e)
    if ((await modsOff($)) || (await $.env.get('ANTHOLOGY_SECRET_MASK')) === '0') return next(e)
    let changed = false
    const content: Block[] = []
    for (const block of e.message.content) {
      const masked = block.type === 'tool_result' ? await maskResult($, block) : block
      changed ||= masked !== block
      content.push(masked)
    }
    return next(changed ? { ...e, message: { ...e.message, content } } : e)
  }).catch(($, e, next) => next(e))
}
