// permissions.ts (post-release mods design §2): the Bash branch of tool.check answers allow only
// when `bin/anthology perm classify` proves the command read-only, and only over an unmarked ask.
// The test's own hooks sit beneath the plugin and stand for Claude Code: `tool.check` is the
// engine's verdict, `process.run` the classifier, `session.*` and `settings.read` the session.
import type { On, ProcessRunResult, ToolCheckResult } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'

type Run = {
  readonly argv: readonly string[]
  readonly init?: { readonly stdin?: string; readonly timeoutMs?: number; readonly env?: { readonly [k: string]: string } }
}

/** Stands for Claude Code beneath the plugin: its verdict, the session and the classifier. */
function engine(
  on: On,
  verdict: ToolCheckResult | (() => ToolCheckResult),
  classify: (run: Run) => ProcessRunResult | Promise<ProcessRunResult>,
  env: Record<string, string> = {},
) {
  const runs: Run[] = []
  mock.env(on, env)
  on('tool.check', () => (typeof verdict === 'function' ? verdict() : verdict))
  on('session.cwd', () => ({ value: '/work/repo/sub' }))
  on('session.root', () => ({ value: '/work/repo' }))
  on('settings.read', () => ({
    value: {
      permissions: {
        additionalDirectories: ['/work/shared', 'relative/dir'],
        deny: ['Read(./secrets/**)', 'Bash(rm:*)', 'Edit(./x)'],
        ask: ['Read(~/.ssh/**)'],
      },
    },
  }))
  on('process.run', async ($, e) => {
    runs.push(e)
    return { value: await classify(e) }
  })
  return runs
}

const PROVEN: ProcessRunResult = {
  exitCode: 0,
  stdout: 'anthology-perm:read-only:v1\n',
  stderr: '',
  isStdoutTruncated: false,
  isStderrTruncated: false,
}

const ASK: ToolCheckResult = { decision: 'ask', reason: 'This command requires approval' }

test('a deny beneath stands and the classifier is not run', async ($, on) => {
  const deny: ToolCheckResult = { decision: 'deny', reason: 'guard', hook: 'PreToolUse' }
  const runs = engine(on, deny, () => PROVEN)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'deny', hook: 'PreToolUse' })
  expect(runs).toHaveLength(0)
})

test('a proven read turns an unmarked ask into allow', async ($, on) => {
  const runs = engine(on, ASK, () => PROVEN)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'cat src/a.rs | head -n 5' } })
  expect(v).toMatchObject({ decision: 'allow', reason: 'anthology: read-only command' })
  expect(runs).toHaveLength(1)
  const run = runs[0]!
  expect(run.argv).toHaveLength(4)
  expect(run.argv[0]).toBe('/bin/sh')
  expect(run.argv[1]).toEndWith('/bin/anthology')
  expect(run.argv.slice(2)).toEqual(['perm', 'classify'])
  expect(run.init?.timeoutMs).toBe(3000)
  expect(run.init?.env).toEqual({ ANTHOLOGY_NO_INSTALL: '1' })
  expect(JSON.parse(run.init?.stdin ?? '')).toEqual({
    command: 'cat src/a.rs | head -n 5',
    cwd: '/work/repo/sub',
    roots: ['/work/repo', '/work/shared'],
    read_rules: ['Read(./secrets/**)', 'Bash(rm:*)', 'Read(~/.ssh/**)'],
  })
})

test('an unproven command keeps the ask', async ($, on) => {
  const runs = engine(on, ASK, () => ({ ...PROVEN, exitCode: 1, stdout: '', stderr: 'not proven: pipe\n' }))
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'rm -rf build' } })
  expect(v).toMatchObject({ decision: 'ask' })
  expect(runs).toHaveLength(1)
})

test('anything but the exact token keeps the ask', async ($, on) => {
  const answers: ProcessRunResult[] = [
    { ...PROVEN, stdout: 'anthology-perm:read-only:v1' },
    { ...PROVEN, stdout: 'anthology-perm:read-only:v1\nextra\n' },
    { ...PROVEN, stdout: ' anthology-perm:read-only:v1\n' },
    { ...PROVEN, stdout: 'anthology-perm:read-only:v2\n' },
    { ...PROVEN, isStdoutTruncated: true },
    { ...PROVEN, exitCode: 2 },
  ]
  let i = 0
  engine(on, ASK, () => answers[i++]!)
  for (const _ of answers) {
    const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
    expect(v).toMatchObject({ decision: 'ask' })
  }
  expect(i).toBe(answers.length)
})

test('a missing binary keeps the ask', async ($, on) => {
  engine(on, ASK, () => ({ ...PROVEN, exitCode: 127, stdout: '', stderr: 'not installed\n' }))
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'ask' })
})

test('a classifier that times out or cannot start keeps the ask', async ($, on) => {
  engine(on, ASK, () => {
    throw new Error('process.run: timed out after 3000 ms')
  })
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'ask' })
})

// `ceiling` is not among these: the engine sets it from the question and drops one a hook
// writes, and a test's query carries none, so the kit cannot raise it (the mod still checks it).
test('a marked ask stands and the classifier is not run', async ($, on) => {
  const marked: ToolCheckResult[] = [
    { ...ASK, rule: 'Bash(ls:*)' },
    { ...ASK, setting: 'permissions.blockReadsOutsideWorkingDirectories' },
    { ...ASK, hook: 'PreToolUse' },
  ]
  let i = 0
  const runs = engine(on, () => marked[i++]!, () => PROVEN)
  const seen: ToolCheckResult[] = []
  for (const _ of marked) seen.push(await $.tool.check({ tool: 'Bash', input: { command: 'ls' } }))
  expect(seen).toEqual(marked)
  expect(runs).toHaveLength(0)
})

// The engine sets `plugin` on an ask another mod made, so that mod is loaded beneath this one.
const ASKING_MOD = {
  name: 'asking-mod',
  tier: 'append',
  register(on: On) {
    on('tool.check', () => ({ decision: 'ask', reason: 'asking-mod wants a person' }))
  },
} as const

test('another mod\'s ask beneath stands', { plugins: [ASKING_MOD] }, async ($, on) => {
  const runs = engine(on, ASK, () => PROVEN)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'ask', plugin: 'asking-mod' })
  expect(runs).toHaveLength(0)
})

test('an allow beneath stands and the classifier is not run', async ($, on) => {
  const allow: ToolCheckResult = { decision: 'allow', rule: 'Bash(rtk *)' }
  const runs = engine(on, allow, () => PROVEN)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'rtk ls' } })
  expect(v).toMatchObject({ decision: 'allow', rule: 'Bash(rtk *)' })
  expect(runs).toHaveLength(0)
})

test('an unmarked deny beneath (a mode that refuses) stands', async ($, on) => {
  const runs = engine(on, { decision: 'deny', reason: 'denied by mode' }, () => PROVEN)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'deny' })
  expect(runs).toHaveLength(0)
})

test('a deny rule beneath stands even for a command the classifier would prove', async ($, on) => {
  const runs = engine(on, { decision: 'deny', rule: 'Bash(cat:*)' }, () => PROVEN)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'cat a' } })
  expect(v).toMatchObject({ decision: 'deny', rule: 'Bash(cat:*)' })
  expect(runs).toHaveLength(0)
})

test('inputs the mod never judges keep the ask', async ($, on) => {
  const runs = engine(on, ASK, () => PROVEN)
  const inputs: unknown[] = [
    { command: 'ls', dangerouslyDisableSandbox: true },
    { command: 'ls\ncat a' },
    { command: 'ls\rcat a' },
    { command: `cat ${'a'.repeat(4096)}` },
    { command: '' },
    { command: 7 },
    {},
  ]
  for (const input of inputs) {
    const v = await $.tool.check({ tool: 'Bash', input })
    expect(v).toMatchObject({ decision: 'ask' })
  }
  expect(runs).toHaveLength(0)
})

test('ANTHOLOGY_MODS=0 turns the mod off', async ($, on) => {
  const runs = engine(on, ASK, () => PROVEN, { ANTHOLOGY_MODS: '0' })
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'ask' })
  expect(runs).toHaveLength(0)
})

test('a non-Bash tool keeps its unmarked ask', async ($, on) => {
  const runs = engine(on, ASK, () => PROVEN)
  const v = await $.tool.check({ tool: 'Read', input: { file_path: '/work/repo/a' } })
  expect(v).toMatchObject({ decision: 'ask' })
  expect(runs).toHaveLength(0)
})
