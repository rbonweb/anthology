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
