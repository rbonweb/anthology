// Helpers the anthology mods share: the kill switch, the call into the installed anthology
// binary and the session's roots and permission settings. Each takes the hook's `$`, so each is a top-level
// function of this file (Claude Code refuses a `$` helper defined inside `register`).
import type { EngineInterface, ProcessRunResult } from 'claude-code'

/** `ANTHOLOGY_MODS=0` turns every anthology mod off; the shell hooks still run. */
export async function modsOff($: EngineInterface): Promise<boolean> {
  return (await $.env.get('ANTHOLOGY_MODS')) === '0'
}

/** The release version in this plugin's anthology.lock, as hooks/lib.sh `anth_version_ok` reads it. */
function lockVersion(text: string): string | undefined {
  let lock: unknown
  try {
    lock = JSON.parse(text)
  } catch {
    return undefined
  }
  const version = typeof lock === 'object' && lock !== null ? (lock as Record<string, unknown>).version : undefined
  return typeof version === 'string' && /^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$/.test(version) ? version : undefined
}

async function isRealDir($: EngineInterface, path: string): Promise<boolean> {
  const s = await $.fs.stat(path)
  return s.kind === 'dir' && !s.isLink
}

/**
 * Runs one feature's part of a hook that index.ts shares between features. A failure leaves
 * `fallback` (what the part's input already was), so it never drops another feature's result and
 * never reaches the hook's `.catch`, which would call `next` a second time.
 */
export async function isolated<T>(part: () => Promise<T>, fallback: T): Promise<T> {
  try {
    return await part()
  } catch {
    return fallback
  }
}

/**
 * The installed anthology binary, found as bin/anthology finds it (lib.sh `anth_paths`,
 * `anth_vpaths`, `anth_installed`): `<ah>/bin/<lock version>/anthology`, `<ah>` being
 * `$CLAUDE_CONFIG_DIR/anthology` or else `$HOME/.claude/anthology` and absolute, with no link on
 * the way, the binary non-empty and its `.sha256` beside it. Undefined when any of that fails:
 * there is no install path here, so a hook never downloads; SessionStart installs.
 */
export async function installedAnthology($: EngineInterface): Promise<string | undefined> {
  try {
    const config = await $.env.get('CLAUDE_CONFIG_DIR')
    const home = await $.env.get('HOME')
    const ah = config ? `${config}/anthology` : home ? `${home}/.claude/anthology` : ''
    if (!ah.startsWith('/')) return undefined
    const version = lockVersion(await $.fs.read(`${$.plugin.root}/anthology.lock`))
    if (version === undefined) return undefined
    const vdir = `${ah}/bin/${version}`
    const bin = `${vdir}/anthology`
    if (!(await isRealDir($, `${ah}/bin`)) || !(await isRealDir($, vdir))) return undefined
    const b = await $.fs.stat(bin)
    if (b.kind !== 'file' || b.isLink || b.size === 0) return undefined
    const sum = await $.fs.stat(`${vdir}/.sha256`)
    return sum.kind === 'file' && sum.size > 0 ? bin : undefined
  } catch {
    return undefined
  }
}

/**
 * Runs the installed anthology binary with `args` directly, never the bin/anthology shim and
 * never through a shell line; undefined, with nothing spawned, when it is not installed.
 * ANTHOLOGY_NO_INSTALL rides along so nothing it starts installs either. Rejects when the child
 * outlives `timeoutMs` (the engine kills it) or cannot start.
 */
export async function runAnthology(
  $: EngineInterface,
  args: readonly string[],
  stdin: string,
  timeoutMs: number,
): Promise<ProcessRunResult | undefined> {
  const bin = await installedAnthology($)
  if (bin === undefined) return undefined
  return $.process.run([bin, ...args], { stdin, timeoutMs, env: { ANTHOLOGY_NO_INSTALL: '1' } })
}

/**
 * A tool found on the absolute entries of the user's PATH only: empty and relative entries (`.`,
 * `::`, `bin`) are skipped, so a repo cannot plant a fake one that a hook then runs from inside it
 * (the same rule the shell hooks follow). `bin` is the absolute path to run; `path` is the PATH
 * to hand the child, with the absolute entries only. Undefined when there is none, as when the
 * tool is not installed. Never throws.
 */
export async function onAbsolutePath(
  $: EngineInterface,
  name: string,
): Promise<{ bin: string; path: string } | undefined> {
  try {
    const dirs = ((await $.env.get('PATH')) ?? '').split(':').filter(d => d.startsWith('/'))
    for (const dir of dirs) {
      const bin = `${dir.replace(/\/+$/, '')}/${name}`
      if (await $.fs.exists(bin)) return { bin, path: dirs.join(':') }
    }
  } catch {
    // fall through: not found
  }
  return undefined
}

/** The `permissions` block of the merged settings, as plain lists of strings. */
export type PermissionSettings = {
  additionalDirectories: string[]
  allow: string[]
  ask: string[]
  deny: string[]
}

function strings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((s): s is string => typeof s === 'string') : []
}

export async function permissionSettings($: EngineInterface): Promise<PermissionSettings> {
  const p = await $.settings.read({ key: 'permissions' })
  const o = (typeof p === 'object' && p !== null ? p : {}) as Record<string, unknown>
  return {
    additionalDirectories: strings(o.additionalDirectories),
    allow: strings(o.allow),
    ask: strings(o.ask),
    deny: strings(o.deny),
  }
}

/**
 * The session's roots: its project root and each absolute `additionalDirectories` entry. A
 * relative or `~` entry is left out, which only narrows what counts as inside.
 */
export async function sessionRoots($: EngineInterface, p: PermissionSettings): Promise<string[]> {
  const roots = [await $.session.root(), ...p.additionalDirectories.filter(d => d.startsWith('/'))]
  return [...new Set(roots)]
}
