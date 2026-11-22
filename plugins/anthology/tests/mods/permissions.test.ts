// permissions.ts (post-release mods design §2): the Bash branch of tool.check answers allow only
// when the installed `anthology perm classify` proves the command read-only, Claude Code allows a
// Read of every path it printed, and only over an unmarked ask a person would answer.
//
// Two kinds of test. The kit tests load the plugin as Claude Code does: their hooks sit beneath
// it and stand for the engine (`tool.check` its verdict, `process.run` the classifier, `fs.*`
// the install, `session.*` and `settings.read` the session). A kit query carries no `approver`
// and no hook may add one, so through the kit the mod never allows. The direct tests call the
// exported hook and its pieces with crafted inputs and a stand-in `$`, with `approver: 'person'`.
import type {
  EngineInterface,
  FsStat,
  On,
  ProcessRunResult,
  SessionVersion,
  ToolCheckInput,
  ToolCheckResult,
} from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'
import { checkBash, classifiedPaths, isSupportedBuild, judgeable } from '../../hooks/mods/permissions.ts'

type Run = {
  readonly argv: readonly string[]
  readonly init?: { readonly stdin?: string; readonly timeoutMs?: number; readonly env?: { readonly [k: string]: string } }
}

const TOKEN = 'anthology-perm:read-only:v1'
const ROOT = '/plug'
const LOCK = `{
  "version": "0.1.1",
  "tag": "anthology-bin--v0.1.1",
  "repo": "rbonweb/anthology",
  "sha256": {}
}
`
const AH = '/home/u/.claude/anthology'
const BIN = `${AH}/bin/0.1.1/anthology`
const DIR: FsStat = { kind: 'dir', size: 0, mtimeMs: 0, isLink: false }
const FILE: FsStat = { kind: 'file', size: 100, mtimeMs: 0, isLink: false }

/** The install the shim would find: <ah>/bin/<version>/anthology and its .sha256. */
function installed(ah: string): Record<string, FsStat> {
  return {
    [`${ah}/bin`]: DIR,
    [`${ah}/bin/0.1.1`]: DIR,
    [`${ah}/bin/0.1.1/anthology`]: FILE,
    [`${ah}/bin/0.1.1/.sha256`]: { ...FILE, size: 65 },
  }
}

const PATHS = ['/work/repo/src/a.rs']

function proven(paths: readonly string[] = PATHS): ProcessRunResult {
  return {
    exitCode: 0,
    stdout: [TOKEN, ...paths].map(l => `${l}\n`).join(''),
    stderr: '',
    isStdoutTruncated: false,
    isStderrTruncated: false,
  }
}

const ASK: ToolCheckResult = { decision: 'ask', reason: 'This command requires approval' }
const ALLOW_READ: ToolCheckResult = { decision: 'allow' }
const ALLOWED: ToolCheckResult = { decision: 'allow', reason: 'anthology: read-only command' }

const PERMISSIONS = {
  additionalDirectories: ['/work/shared', 'relative/dir'],
  deny: ['Read(./secrets/**)', 'Bash(rm:*)', 'Edit(./x)'],
  ask: ['Read(~/.ssh/**)'],
}

// ---- direct tests: a stand-in `$` --------------------------------------------------------------

type World = {
  env?: Record<string, string>
  version?: SessionVersion | (() => SessionVersion)
  policy?: Record<string, unknown>
  files?: Record<string, FsStat>
  lock?: string
  classify?: (run: Run) => ProcessRunResult
  read?: (filePath: string) => ToolCheckResult
}

function world(w: World = {}) {
  const runs: Run[] = []
  const reads: unknown[] = []
  const settingsReads: unknown[] = []
  const env = w.env ?? { HOME: '/home/u' }
  const files = w.files ?? installed(AH)
  const $ = {
    plugin: { root: ROOT },
    env: { get: async (name: string) => env[name] },
    session: {
      version: async () => {
        const v = w.version ?? { version: '2.1.330', base: '2.1.330' }
        return typeof v === 'function' ? v() : v
      },
      cwd: async () => '/work/repo/sub',
      root: async () => '/work/repo',
    },
    settings: {
      read: async (args?: { key?: string; source?: string }) => {
        settingsReads.push(args)
        if (args?.key === 'permissions') return PERMISSIONS
        if (args?.source === 'policy') return w.policy ?? {}
        throw new Error(`unexpected settings read ${JSON.stringify(args)}`)
      },
    },
    fs: {
      read: async (path: string) => {
        if (path === `${ROOT}/anthology.lock`) return w.lock ?? LOCK
        throw new Error(`ENOENT: ${path}`)
      },
      stat: async (path: string) => {
        const s = files[path]
        if (s === undefined) throw new Error(`ENOENT: ${path}`)
        return s
      },
    },
    process: {
      run: async (argv: readonly string[], init?: Run['init']) => {
        const run = { argv, init }
        runs.push(run)
        return (w.classify ?? (() => proven()))(run)
      },
    },
    tool: {
      check: async (q: { tool: string; input: unknown }) => {
        reads.push(q)
        if (q.tool !== 'Read') throw new Error(`unexpected tool.check ${q.tool}`)
        return (w.read ?? (() => ALLOW_READ))((q.input as { file_path: string }).file_path)
      },
    },
  }
  return { $: $ as unknown as EngineInterface, runs, reads, settingsReads }
}

function bash(command: string, extra: Partial<ToolCheckInput> = {}): ToolCheckInput {
  return { tool: 'Bash', input: { command }, approver: 'person', ...extra }
}

async function decide($: EngineInterface, e: ToolCheckInput, v: ToolCheckResult = ASK) {
  let nexts = 0
  const r = await checkBash($, e, async () => {
    nexts++
    return v
  })
  expect(nexts).toBe(1)
  return r
}

describe('judgeable', () => {
  test('an unmarked ask on a Bash command a person answers is judged', () => {
    expect(judgeable(bash('ls -la'), ASK)).toBe('ls -la')
  })

  test('only a person as approver is judged', () => {
    expect(judgeable(bash('ls', { approver: 'person' }), ASK)).toBe('ls')
    expect(judgeable(bash('ls', { approver: 'host' }), ASK)).toBeUndefined()
    expect(judgeable({ tool: 'Bash', input: { command: 'ls' } }, ASK)).toBeUndefined()
  })

  test('only the Bash tool is judged', () => {
    expect(judgeable({ ...bash('ls'), tool: 'Read' }, ASK)).toBeUndefined()
    expect(judgeable({ ...bash('ls'), tool: 'mcp__x__Bash' }, ASK)).toBeUndefined()
  })

  test('only an ask is judged', () => {
    expect(judgeable(bash('ls'), { decision: 'allow' })).toBeUndefined()
    expect(judgeable(bash('ls'), { decision: 'deny' })).toBeUndefined()
  })

  test('an ask with any mark on the verdict is not judged', () => {
    expect(judgeable(bash('ls'), { ...ASK, rule: 'Bash(ls:*)' })).toBeUndefined()
    expect(judgeable(bash('ls'), { ...ASK, setting: 'permissions.blockReadsOutsideWorkingDirectories' })).toBeUndefined()
    expect(judgeable(bash('ls'), { ...ASK, hook: 'PreToolUse' })).toBeUndefined()
    expect(judgeable(bash('ls'), { ...ASK, plugin: 'asking-mod' })).toBeUndefined()
    expect(judgeable(bash('ls'), { ...ASK, ceiling: 'ask' })).toBeUndefined()
  })

  test('a question with an organization ceiling or a plugin op is not judged', () => {
    expect(judgeable(bash('ls', { ceiling: 'ask' }), ASK)).toBeUndefined()
    expect(judgeable(bash('ls', { ceiling: 'deny' }), ASK)).toBeUndefined()
    expect(judgeable(bash('ls', { isCeilingUnread: true }), ASK)).toBeUndefined()
    expect(judgeable(bash('ls', { op: 'process.run' }), ASK)).toBeUndefined()
  })

  test('inputs the mod never judges', () => {
    const inputs: unknown[] = [
      { command: 'ls', dangerouslyDisableSandbox: true },
      { command: 'ls', dangerouslyDisableSandbox: 'yes' },
      { command: 'ls\ncat a' },
      { command: 'ls\rcat a' },
      { command: `cat ${'a'.repeat(4096)}` },
      { command: '' },
      { command: '   ' },
      { command: 7 },
      {},
      null,
      'ls',
    ]
    for (const input of inputs) expect(judgeable({ ...bash('x'), input }, ASK)).toBeUndefined()
    expect(judgeable({ ...bash('x'), input: { command: 'ls', dangerouslyDisableSandbox: false } }, ASK)).toBe('ls')
  })
})

describe('isSupportedBuild', () => {
  test('2.1.330 and later are supported', () => {
    for (const base of ['2.1.330', '2.1.330-dev', '2.1.331', '2.1.1000', '2.2.0', '3.0.0']) {
      expect(isSupportedBuild(base)).toBe(true)
    }
  })

  test('earlier, unknown or unparsable builds are not', () => {
    for (const base of [undefined, '', '2.1.329', '2.1.329-dev', '2.0.999', '1.9.999', '2.1', 'v2.1.330', 'x']) {
      expect(isSupportedBuild(base)).toBe(false)
    }
  })
})

describe('classifiedPaths', () => {
  test('the token line then one absolute path per line', () => {
    expect(classifiedPaths(`${TOKEN}\n/a\n/b c\n`)).toEqual(['/a', '/b c'])
  })

  test('anything else is no proof', () => {
    const outs = [
      `${TOKEN}\n`,
      `${TOKEN}`,
      `${TOKEN}\n/a`,
      ` ${TOKEN}\n/a\n`,
      `anthology-perm:read-only:v2\n/a\n`,
      `${TOKEN}\nrelative\n`,
      `${TOKEN}\n/a\n\n`,
      `${TOKEN}\n/a\r\n`,
      `${TOKEN}\n/a\0b\n`,
      `/a\n${TOKEN}\n`,
      '',
    ]
    for (const out of outs) expect(classifiedPaths(out)).toBeUndefined()
  })

  test('at most 64 paths', () => {
    const paths = (n: number) => Array.from({ length: n }, (_, i) => `/p/${i}`)
    expect(classifiedPaths(proven(paths(64)).stdout)).toEqual(paths(64))
    expect(classifiedPaths(proven(paths(65)).stdout)).toBeUndefined()
  })
})

describe('checkBash', () => {
  test('a proven read whose every path Claude Code allows to Read is allowed', async () => {
    const w = world()
    expect(await decide(w.$, bash('cat src/a.rs | head -n 5'))).toEqual(ALLOWED)
    expect(w.runs).toHaveLength(1)
    const run = w.runs[0]!
    expect(run.argv).toEqual([BIN, 'perm', 'classify'])
    expect(run.init?.timeoutMs).toBe(3000)
    expect(run.init?.env).toEqual({ ANTHOLOGY_NO_INSTALL: '1' })
    expect(JSON.parse(run.init?.stdin ?? '')).toEqual({
      command: 'cat src/a.rs | head -n 5',
      cwd: '/work/repo/sub',
      roots: ['/work/repo', '/work/shared'],
      read_rules: ['Read(./secrets/**)', 'Bash(rm:*)', 'Read(~/.ssh/**)'],
    })
    expect(w.reads).toEqual([{ tool: 'Read', input: { file_path: '/work/repo/src/a.rs' } }])
  })

  test('every printed path is checked as a Read', async () => {
    const paths = Array.from({ length: 64 }, (_, i) => `/work/repo/f${i}`)
    const w = world({ classify: () => proven(paths) })
    expect(await decide(w.$, bash('cat f*'))).toEqual(ALLOWED)
    expect(w.reads).toEqual(paths.map(file_path => ({ tool: 'Read', input: { file_path } })))
  })

  test('a Read that Claude Code would not allow keeps the verdict', async () => {
    const paths = ['/work/repo/a', '/work/repo/secret', '/work/repo/b']
    for (const answer of [ASK, { decision: 'deny' } as const, { decision: 'ask', rule: 'Read(./secret)' } as const]) {
      const w = world({
        classify: () => proven(paths),
        read: p => (p.endsWith('secret') ? answer : ALLOW_READ),
      })
      expect(await decide(w.$, bash('cat a secret b'))).toEqual(ASK)
    }
  })

  test('no printed path keeps the verdict and checks no Read', async () => {
    const w = world({ classify: () => proven([]) })
    expect(await decide(w.$, bash('ls'))).toEqual(ASK)
    expect(w.reads).toHaveLength(0)
  })

  test('more than 64 printed paths keeps the verdict and checks no Read', async () => {
    const paths = Array.from({ length: 65 }, (_, i) => `/work/repo/f${i}`)
    const w = world({ classify: () => proven(paths) })
    expect(await decide(w.$, bash('cat f*'))).toEqual(ASK)
    expect(w.reads).toHaveLength(0)
  })

  test('an unproven command keeps the verdict', async () => {
    const w = world({ classify: () => ({ ...proven(), exitCode: 1, stdout: '', stderr: 'not proven: pipe\n' }) })
    expect(await decide(w.$, bash('rm -rf build'))).toEqual(ASK)
    expect(w.runs).toHaveLength(1)
    expect(w.reads).toHaveLength(0)
  })

  test('anything but the token and paths keeps the verdict', async () => {
    const answers: ProcessRunResult[] = [
      { ...proven(), stdout: `${TOKEN}\n/work/repo/a` },
      { ...proven(), stdout: ` ${TOKEN}\n/work/repo/a\n` },
      { ...proven(), stdout: `anthology-perm:read-only:v2\n/work/repo/a\n` },
      { ...proven(), isStdoutTruncated: true },
      { ...proven(), exitCode: 2 },
    ]
    for (const answer of answers) {
      const w = world({ classify: () => answer })
      expect(await decide(w.$, bash('ls'))).toEqual(ASK)
      expect(w.reads).toHaveLength(0)
    }
  })

  test('a classifier that times out or cannot start keeps the verdict', async () => {
    const w = world({
      classify: () => {
        throw new Error('process.run: timed out after 3000 ms')
      },
    })
    expect(await decide(w.$, bash('ls'))).toEqual(ASK)
  })

  test('a missing binary spawns nothing, not the shim either, and keeps the verdict', async () => {
    const w = world({ files: {} })
    expect(await decide(w.$, bash('ls'))).toEqual(ASK)
    expect(w.runs).toHaveLength(0)
  })

  test('an install the shim would not run spawns nothing', async () => {
    const broken: Record<string, FsStat>[] = [
      { ...installed(AH), [BIN]: { ...FILE, isLink: true } },
      { ...installed(AH), [BIN]: { ...FILE, size: 0 } },
      { ...installed(AH), [BIN]: DIR },
      { ...installed(AH), [`${AH}/bin/0.1.1/.sha256`]: { ...FILE, size: 0 } },
      { ...installed(AH), [`${AH}/bin/0.1.1`]: { ...DIR, isLink: true } },
      { ...installed(AH), [`${AH}/bin`]: { ...DIR, isLink: true } },
    ]
    for (const files of broken) {
      const w = world({ files })
      expect(await decide(w.$, bash('ls'))).toEqual(ASK)
      expect(w.runs).toHaveLength(0)
    }
  })

  test('a lock the shim would not read spawns nothing', async () => {
    for (const lock of ['', 'not json', '{"version": "../0.1.1"}', '{"version": 1}', '{}']) {
      const w = world({ lock })
      expect(await decide(w.$, bash('ls'))).toEqual(ASK)
      expect(w.runs).toHaveLength(0)
    }
  })

  test('the binary is found under CLAUDE_CONFIG_DIR first, as the shim finds it', async () => {
    const w = world({ env: { CLAUDE_CONFIG_DIR: '/cfg', HOME: '/home/u' }, files: installed('/cfg/anthology') })
    expect(await decide(w.$, bash('ls'))).toEqual(ALLOWED)
    expect(w.runs[0]!.argv[0]).toBe('/cfg/anthology/bin/0.1.1/anthology')
  })

  test('a relative or missing home spawns nothing', async () => {
    const envs: Record<string, string>[] = [{}, { HOME: 'home/u' }, { CLAUDE_CONFIG_DIR: 'cfg', HOME: '/home/u' }]
    for (const env of envs) {
      const w = world({ env, files: { ...installed(AH), ...installed('home/u/.claude/anthology'), ...installed('cfg/anthology') } })
      expect(await decide(w.$, bash('ls'))).toEqual(ASK)
      expect(w.runs).toHaveLength(0)
    }
  })

  test('a build before 2.1.330 keeps the verdict and spawns nothing', async () => {
    for (const version of [{ version: '2.1.329', base: '2.1.329' }, { version: 'local' }]) {
      const w = world({ version })
      expect(await decide(w.$, bash('ls'))).toEqual(ASK)
      expect(w.runs).toHaveLength(0)
    }
  })

  test('a build without session.version throws, for the .catch to keep the verdict', async () => {
    const w = world({
      version: () => {
        throw new Error('no implementation for session.version')
      },
    })
    await expect(checkBash(w.$, bash('ls'), async () => ASK)).rejects.toThrow('session.version')
    expect(w.runs).toHaveLength(0)
  })

  test('an organization that allows only managed permission rules keeps the verdict', async () => {
    const w = world({ policy: { allowManagedPermissionRulesOnly: true } })
    expect(await decide(w.$, bash('ls'))).toEqual(ASK)
    expect(w.runs).toHaveLength(0)
    expect(w.settingsReads).toContainEqual({ source: 'policy' })
  })

  test('a policy that does not set managed rules only leaves the mod on', async () => {
    for (const policy of [{}, { allowManagedPermissionRulesOnly: false }]) {
      const w = world({ policy })
      expect(await decide(w.$, bash('ls'))).toEqual(ALLOWED)
    }
  })

  test('ANTHOLOGY_MODS=0 turns the mod off', async () => {
    const w = world({ env: { HOME: '/home/u', ANTHOLOGY_MODS: '0' } })
    expect(await decide(w.$, bash('ls'))).toEqual(ASK)
    expect(w.runs).toHaveLength(0)
  })

  test('a verdict the mod does not judge is returned as it came, with nothing run', async () => {
    const verdicts: ToolCheckResult[] = [
      { decision: 'deny', reason: 'guard', hook: 'PreToolUse' },
      { decision: 'deny', rule: 'Bash(cat:*)' },
      { decision: 'deny', reason: 'denied by mode' },
      { decision: 'allow', rule: 'Bash(rtk *)' },
      { ...ASK, rule: 'Bash(ls:*)' },
      { ...ASK, setting: 'permissions.blockReadsOutsideWorkingDirectories' },
      { ...ASK, hook: 'PreToolUse' },
      { ...ASK, plugin: 'asking-mod' },
      { ...ASK, ceiling: 'ask' },
    ]
    for (const v of verdicts) {
      const w = world()
      expect(await decide(w.$, bash('ls'), v)).toBe(v)
      expect(w.runs).toHaveLength(0)
    }
  })

  test('a host or no approver keeps the verdict with nothing run', async () => {
    for (const e of [bash('ls', { approver: 'host' }), { tool: 'Bash', input: { command: 'ls' } }]) {
      const w = world()
      expect(await decide(w.$, e)).toBe(ASK)
      expect(w.runs).toHaveLength(0)
    }
  })
})

// ---- kit tests: the plugin as Claude Code loads it ---------------------------------------------

/** Stands for Claude Code beneath the plugin, answering every call the mod could make. */
function engine(on: On, bashVerdict: () => ToolCheckResult, readVerdict = () => ALLOW_READ) {
  const runs: unknown[] = []
  mock.env(on, { HOME: '/home/u' })
  on('tool.check', ($, e) => (e.tool === 'Bash' ? bashVerdict() : readVerdict()))
  on('session.version', () => ({ value: { version: '2.1.330', base: '2.1.330' } }))
  on('session.cwd', () => ({ value: '/work/repo/sub' }))
  on('session.root', () => ({ value: '/work/repo' }))
  on('settings.read', ($, e) => ({ value: e.source === 'policy' ? {} : { permissions: PERMISSIONS } }))
  on('fs.read', () => ({ value: LOCK }))
  on('fs.stat', ($, e) => {
    const s = installed(AH)[e.path]
    if (s === undefined) throw new Error(`ENOENT: ${e.path}`)
    return { value: s }
  })
  on('process.run', ($, e) => {
    runs.push(e)
    return { value: proven() }
  })
  return runs
}

test('kit: a query has no approver, so an unmarked ask stands with nothing run', async ($, on) => {
  const runs = engine(on, () => ASK)
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'ask' })
  expect(runs).toHaveLength(0)
})

test('kit: a deny beneath stands', async ($, on) => {
  const runs = engine(on, () => ({ decision: 'deny', reason: 'guard', hook: 'PreToolUse' }))
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toMatchObject({ decision: 'deny', hook: 'PreToolUse' })
  expect(runs).toHaveLength(0)
})

// Nothing answers env.get here, so the mod's first call on `$` throws; its .catch replays the
// verdict beneath instead of the engine failing the check closed.
test('kit: a failing hook replays the verdict beneath (the .catch)', async ($, on) => {
  let calls = 0
  on('tool.check', () => {
    calls++
    return ASK
  })
  const v = await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })
  expect(v).toEqual(ASK)
  expect(calls).toBe(1)
})

// The kill switch is the mod's first call, so an env.get shows whether the mod saw the check.
test('kit: the mod hooks Bash alone', async ($, on) => {
  const gets: string[] = []
  on('env.get', ($, e) => {
    gets.push(e.name)
    return { value: undefined }
  })
  on('tool.check', ($, e) => (e.tool === 'Bash' ? ASK : ALLOW_READ))
  expect(await $.tool.check({ tool: 'Read', input: { file_path: '/work/repo/a' } })).toEqual(ALLOW_READ)
  expect(gets).toEqual([])
  expect(await $.tool.check({ tool: 'Bash', input: { command: 'ls' } })).toEqual(ASK)
  expect(gets).toEqual(['ANTHOLOGY_MODS'])
})
