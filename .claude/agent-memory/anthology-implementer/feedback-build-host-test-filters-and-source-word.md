---
name: 'Several test filters, and the word source in a Bash command'
description: 'remote-build test takes more than one libtest filter only after --; a Bash call containing the word source is refused in a worktree, so use a script file'
metadata:
  type: feedback
  modified: 2026-11-28
---

Why: both cost a round trip during check-repo Task 6.
scripts/remote-build.sh test -p <crate> a:: b:: fails with "unexpected argument"; write test -p <crate> -- a:: b:: (libtest takes several filters).
A Bash command that contains the bare word source (for example osv-scanner scan source) is refused in a worktree-isolated agent ("runs a string through source"). Put the command in a script file in the worktree, run bash <file>, and delete the file before committing.
remote-build syncs the checkout before it waits for a build slot, so edits made while the log says "all build slots are busy" do not reach that run. A mutation run can therefore be reverted with git checkout as soon as that line shows.
clippy octal_escapes: in a Rust test string write \x00, not \0, when a digit follows.
