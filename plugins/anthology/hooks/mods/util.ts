// Helpers the anthology mods share: the kill switch, the call into the anthology binary and the
// session's roots and permission settings. Each takes the hook's `$`, so each is a top-level
// function of this file (Claude Code refuses a `$` helper defined inside `register`).
import type { EngineInterface, ProcessRunResult } from 'claude-code'

/** `ANTHOLOGY_MODS=0` turns every anthology mod off; the shell hooks still run. */
export async function modsOff($: EngineInterface): Promise<boolean> {
  return (await $.env.get('ANTHOLOGY_MODS')) === '0'
}

/**
 * Runs `bin/anthology <args>` from this plugin, never through a shell line. ANTHOLOGY_NO_INSTALL
 * makes the shim exit 127 instead of downloading the binary inside a hook; SessionStart installs.
 * Rejects when the child outlives `timeoutMs` (the engine kills it) or cannot start.
 */
export function runAnthology(
  $: EngineInterface,
  args: readonly string[],
  stdin: string,
  timeoutMs: number,
): Promise<ProcessRunResult> {
  return $.process.run(['/bin/sh', `${$.plugin.root}/bin/anthology`, ...args], {
    stdin,
    timeoutMs,
    env: { ANTHOLOGY_NO_INSTALL: '1' },
  })
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
