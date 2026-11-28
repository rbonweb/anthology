---
name: 'Delegation mod: the lead may write a gitignored, untracked file'
description: 'Why isLocalSetup exists in delegation.ts, how git decides, and the test-mock traps (process.run cwd is e.init.cwd)'
metadata:
  type: project
  modified: 2026-11-28
---

delegation.ts isLocalSetup lets the lead Write, Edit or NotebookEdit a file that git ignores and does not track. Why: such a file (for example .env.NAME.local) exists only in the main checkout, and Claude Code refuses a worktree agent any write to a main-checkout path, so delegating could never make it. Rule: git check-ignore -q must exit 0 and git ls-files --error-unmatch must exit exactly 1 (128 is git failing), both from the nearest folder holding .git, with the git found on absolute PATH entries only (onAbsolutePath) and core.fsmonitor off. Never a path with a .git segment (compared lowercase), never a symbolic link (fs.stat isLink), any failure keeps the refusal. Real git: a tracked file gives check-ignore 1; a path beyond a symlinked folder gives check-ignore 128. Test traps in delegation.test.ts: the process.run event carries the folder as e.init.cwd, not e.cwd; World.git puts git on PATH (default worlds have none, so older tests never spawn git); the mock check-ignore reports tracked files as ignored on purpose, so the index check is what keeps them refused. Version: plugin.json was already above the last tag, so only a changelog.d fragment was needed.
