// masking.ts: the Bash result a session keeps goes through `anthology guard mask` first, when a
// cheap text check finds a likely secret. The test's own hooks sit beneath the plugin and stand for
// Claude Code: `session.append` is the row being stored (it records what arrived), `process.run`
// is the installed binary, `fs.*` and `env.get` are the session.
import type { On, SessionAppendInput } from 'claude-code'
import { type Engine, expect, test } from 'claude-code/testing'

const HOME = '/home/dev'
const LOCK = '{"version": "0.1.1", "tag": "anthology-bin--v0.1.1", "repo": "rbonweb/anthology", "sha256": {}}'

// Built at run time, so no whole secret sits in this file.
const AWS = ['AKIA', 'IOSFODNN7', 'EXAMPLE'].join('')
const MASKED = '[masked by anthology: aws-access-key]'

type Answer = { exitCode: number; stdout: string; isStdoutTruncated?: boolean } | 'reject'

type World = {
  env?: { readonly [name: string]: string }
  /** no binary installed: the shim's files are not there */
  noBinary?: boolean
  /** what each call of the binary answers, in order; the last one answers the rest. Unset: the real masker's job done for the AWS key. */
  answers?: readonly Answer[]
}

type Seen = {
  /** the argv (after the binary) and stdin of each process the mod started */
  runs: { argv: readonly string[]; stdin: string | undefined }[]
  /** the rows that reached the bottom of the chain, as the mod handed them on */
  stored: SessionAppendInput[]
}

function engine(on: On, world: World = {}): Seen {
  const seen: Seen = { runs: [], stored: [] }
  const env = { HOME, ...world.env } as { readonly [name: string]: string }
  on('env.get', (_$, e) => ({ value: env[e.name] }))
  on('fs.read', (_$, e) => {
    if (e.path.endsWith('/anthology.lock')) return { value: LOCK }
    throw new Error(`ENOENT: ${e.path}`)
  })
  on('fs.stat', (_$, e) => {
    const ah = `${HOME}/.claude/anthology`
    const known: { [path: string]: { kind: 'dir' | 'file'; size: number } } = {
      [`${ah}/bin`]: { kind: 'dir', size: 0 },
      [`${ah}/bin/0.1.1`]: { kind: 'dir', size: 0 },
      [`${ah}/bin/0.1.1/anthology`]: { kind: 'file', size: 100 },
      [`${ah}/bin/0.1.1/.sha256`]: { kind: 'file', size: 65 },
    }
    const s = world.noBinary ? undefined : known[e.path]
    if (s === undefined) throw new Error(`ENOENT: ${e.path}`)
    return { value: { ...s, mtimeMs: 0, isLink: false } }
  })
  let call = 0
  on('process.run', (_$, e) => {
    seen.runs.push({ argv: e.argv.slice(1), stdin: e.init?.stdin })
    const answer = world.answers?.[Math.min(call, world.answers.length - 1)]
    call += 1
    if (answer === 'reject') throw new Error('timed out')
    const stdout = answer?.stdout ?? (e.init?.stdin ?? '').split(AWS).join(MASKED)
    return {
      value: {
        exitCode: answer?.exitCode ?? 0,
        stdout,
        stderr: '',
        isStdoutTruncated: answer?.isStdoutTruncated ?? false,
        isStderrTruncated: false,
      },
    }
  })
  // The row as the plugin hands it down; calling `next` keeps it, as the engine's own store does.
  on('session.append', (_$, e, next) => {
    seen.stored.push(e)
    return next(e)
  })
  return seen
}

type Block = { type: string; [field: string]: unknown }

function row(content: Block[], options: { tool?: string; agentId?: string } = {}): SessionAppendInput {
  return {
    message: { type: 'user', role: 'user', content },
    door: 'tool-result',
    origin: { kind: 'tool', tool: options.tool ?? 'Bash' },
    uuid: 'u1',
    ...(options.agentId === undefined ? {} : { agentId: options.agentId }),
  }
}

const result = (content: unknown, id = 't1'): Block => ({ type: 'tool_result', tool_use_id: id, content })

function append($: Engine, input: SessionAppendInput) {
  return $.session.append(input)
}

test('no_prefilter_hit_spawns_nothing', async ($, on) => {
  const seen = engine(on)
  const input = row([result('total 8\ndrwxr-xr-x  2 dev dev 4096 Nov 28 10:00 src\n')])
  const r = await append($, input)
  expect(seen.runs).toHaveLength(0)
  expect(r.message?.content).toEqual(input.message.content)
})

test('hit_rewrites_only_that_result', async ($, on) => {
  const seen = engine(on)
  const clean = result('nothing to see', 't1')
  const dirty = result(`id=${AWS}\n`, 't2')
  const r = await append($, row([clean, dirty]))
  expect(seen.runs).toHaveLength(1)
  expect(seen.runs[0]).toEqual({ argv: ['guard', 'mask'], stdin: `id=${AWS}\n` })
  expect(r.message?.content).toEqual([clean, result(`id=${MASKED}\n`, 't2')])
})

test('text blocks of a result are masked one by one and other kinds stay', async ($, on) => {
  const seen = engine(on)
  const image = { type: 'image', source: { type: 'base64', data: 'AAAA' } }
  const r = await append(
    $,
    row([result([{ type: 'text', text: 'ok' }, { type: 'text', text: `k ${AWS}` }, image])]),
  )
  expect(seen.runs).toHaveLength(1)
  expect(r.message?.content).toEqual([
    result([{ type: 'text', text: 'ok' }, { type: 'text', text: `k ${MASKED}` }, image]),
  ])
})

test('a text block outside a tool_result is left alone', async ($, on) => {
  const seen = engine(on)
  const text: Block = { type: 'text', text: `id ${AWS}` }
  const r = await append($, row([text]))
  expect(seen.runs).toHaveLength(0)
  expect(r.message?.content).toEqual([text])
})

test('non_bash_rows_untouched', async ($, on) => {
  const seen = engine(on)
  const input = row([result(`id=${AWS}`)], { tool: 'Read' })
  const r = await append($, input)
  expect(seen.runs).toHaveLength(0)
  expect(r.message?.content).toEqual(input.message.content)
})

test('a row that no tool made is untouched', async ($, on) => {
  const seen = engine(on)
  const input: SessionAppendInput = { ...row([result(`id=${AWS}`)]), origin: { kind: 'model', model: 'm' } }
  const r = await append($, input)
  expect(seen.runs).toHaveLength(0)
  expect(r.message?.content).toEqual(input.message.content)
})

test('binary_failure_keeps_row', async ($, on) => {
  const seen = engine(on, {
    answers: [
      { exitCode: 3, stdout: '' },
      'reject',
      { exitCode: 2, stdout: 'masked?' },
      { exitCode: 0, stdout: '' },
      { exitCode: 0, stdout: 'cut', isStdoutTruncated: true },
    ],
  })
  for (let i = 0; i < 5; i++) {
    const input = row([result(`id=${AWS}`)])
    const r = await append($, input)
    expect(r.message?.content).toEqual(input.message.content)
  }
  expect(seen.runs).toHaveLength(5)
})

test('an unchanged answer keeps the row', async ($, on) => {
  const seen = engine(on, { answers: [{ exitCode: 0, stdout: `id=${AWS}` }] })
  const input = row([result(`id=${AWS}`)])
  const r = await append($, input)
  expect(seen.runs).toHaveLength(1)
  expect(r.message?.content).toEqual(input.message.content)
})

test('no installed binary keeps the row and starts nothing', async ($, on) => {
  const seen = engine(on, { noBinary: true })
  const input = row([result(`id=${AWS}`)])
  const r = await append($, input)
  expect(seen.runs).toHaveLength(0)
  expect(r.message?.content).toEqual(input.message.content)
})

test('a result over a million characters is left as it is', async ($, on) => {
  const seen = engine(on)
  const big = `${AWS}${'x'.repeat(1_000_000)}`
  const input = row([result(big)])
  const r = await append($, input)
  expect(seen.runs).toHaveLength(0)
  expect(r.message?.content).toEqual(input.message.content)
})

for (const [name, env] of [
  ['switch_off', { ANTHOLOGY_SECRET_MASK: '0' }],
  ['switch_off_with_all_mods_off', { ANTHOLOGY_MODS: '0' }],
] as const) {
  test(name, async ($, on) => {
    const seen = engine(on, { env })
    const input = row([result(`id=${AWS}`)])
    const r = await append($, input)
    expect(seen.runs).toHaveLength(0)
    expect(r.message?.content).toEqual(input.message.content)
  })
}

test('subagent_row_masked', async ($, on) => {
  const seen = engine(on)
  const r = await append($, row([result(`id=${AWS}`)], { agentId: 'a1' }))
  expect(seen.runs).toHaveLength(1)
  expect(r.message?.content).toEqual([result(`id=${MASKED}`)])
  expect(seen.stored[0]?.agentId).toBe('a1')
})

test('the prefilter is the likely-match check', async ($, on) => {
  const seen = engine(on)
  const likely = ['ghp_x', 'github_pat_x', 'sk-x', 'xoxb-x', '-----BEGIN PRIVATE KEY-----', 'DB_PASSWORD=x', 'Api-Key: x', 'ASIA0', 'akia0']
  for (const text of likely) await append($, row([result(text)]))
  expect(seen.runs).toHaveLength(likely.length)
  const before = seen.runs.length
  for (const text of ['sha512-abc', 'git log', 'sk', 'xox-']) await append($, row([result(text)]))
  expect(seen.runs).toHaveLength(before)
})
