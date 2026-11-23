---
name: 'perm classify (PR 172) review findings'
description: 'False proofs found in anthology perm classify over two rounds (heads 7b79e56e, 030302ff and c80e0287) and the inputs that prove them'
metadata:
  type: project
  modified: 2026-11-22
---

Security review of PR 172 (crates/anthology-cli/src/cmd/perm/), probed in the pinned rust image (Debian trixie, git 2.47) from git archive, nothing mounted.
Round 1 (7b79e56e), all fixed in round 2: diff of two directories follows a symlink below them; a PATH entry inside a root lets a repo cat or git run, and the check itself ran that git; git branch --format=%(signature) and --sort=signature run gpg; git status -v and git diff --cached print a file dropped from the index but still in HEAD; plus hard links, a symlinked .git/objects, GREP_OPTIONS, RIPGREP_CONFIG_PATH, brace rules and duplicate JSON keys.
Round 2 (030302ff, same classify code as c80e0287), still open: words.rs LOG_PATCH_SHORT compares case-sensitively, so git log -U3 (glued U in the GIT_LOG table) prints the patch; git log --check prints added lines that have trailing whitespace; a PATH directory outside the roots holding a symlink cat that points into a root (npm link style) still runs, because only PATH directories are resolved, not the programs.
Why: the P5 mod allows Bash on these proofs; a fix round must kill each input.
How to apply: re-run these inputs on any later head. For git history filters, list every option in the readonly.rs table that implies a patch (-U, --check), not only the documented patch flags.
