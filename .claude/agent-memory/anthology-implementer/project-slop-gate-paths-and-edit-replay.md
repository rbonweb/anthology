---
name: 'Slop gate Task 6: path and replay rulings'
description: 'An empty config dir must not exempt everything; how added lines, line spans and shown paths are decided in anthology-slop'
metadata:
  type: project
  modified: 2026-11-28
---

In crates/anthology-slop/src/paths.rs, Path::starts_with of an empty path or "/" is true for every path, so exempt() ignores a config_dir with no normal component (an unset or empty CLAUDE_CONFIG_DIR would otherwise switch the gate off for every file). Only the part below the repo root is read for EXEMPT_DIRS, which is what keeps a worktree under .claude/worktrees/id gated. exempt() does not normalise ".." lexically: .claude/../docs/a.md reads as exempt, so the CLI should hand it an absolute, normalised path. shown() escapes control characters because the path is printed into messages the agent reads. In edit.rs, added_lines treats blank lines as neither new nor merging, merges only consecutive new lines, and compares lines with the line ending removed; line_spans drops a range that starts at 0, ends before it starts or starts past the last line. rules_table() ends every line with a newline, so the caller prints it without adding one. Why: these decisions are not in the plan text and Task 7 and Task 10 build on them.
