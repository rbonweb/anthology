// Permission levels (post-release mods design §2). Scaffold: passes every verdict through.
import type { On } from 'claude-code'

export function registerPermissions(on: On): void {
  on('tool.check', { tool: 'Bash' }, ($, e, next) => next(e)).catch(($, e, next) => next(e))
}
