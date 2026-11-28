// Secret masking (stub): the real mod replaces this file. It hooks `session.append` (door
// `tool-result`) alone, so index.ts calls `registerMasking` once and nothing here is shared.
import type { On } from 'claude-code'

export function registerMasking(_on: On): void {}
