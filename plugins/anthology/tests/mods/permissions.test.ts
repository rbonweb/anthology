// permissions.ts (post-release mods design §2): the Bash branch of tool.check answers allow only
// when `bin/anthology perm classify` proves the command read-only, and only over an unmarked ask.
// The test's own hooks sit beneath the plugin and stand for Claude Code: `tool.check` is the
// engine's verdict, `process.run` the classifier, `session.*` and `settings.read` the session.
import type { On, ProcessRunResult, ToolCheckResult } from 'claude-code'
import { expect, test } from 'claude-code/testing'

type Run = {
  readonly argv: readonly string[]
  readonly init?: { readonly stdin?: string; readonly timeoutMs?: number; readonly env?: { readonly [k: string]: string } }
}

/** Stands for Claude Code beneath the plugin: its verdict, the session and the classifier. */
function engine(on: On, verdict: ToolCheckResult, classify: (run: Run) => ProcessRunResult | Promise<ProcessRunResult>) {
  const runs: Run[] = []
  on('tool.check', () => verdict)
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

test('a non-Bash tool keeps its unmarked ask', async ($, on) => {
  const runs = engine(on, ASK, () => PROVEN)
  const v = await $.tool.check({ tool: 'Read', input: { file_path: '/work/repo/a' } })
  expect(v).toMatchObject({ decision: 'ask' })
  expect(runs).toHaveLength(0)
})
