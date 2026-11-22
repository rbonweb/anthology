---
name: 'classify_readonly: two readers, test each layer alone'
description: 'How the read-only classifier and the rtk cd/head gate are built, and the rulings behind their refusals'
metadata:
  type: project
  modified: 2026-11-22
---

classify_readonly (crates/anthology-shell/src/readonly.rs) has its own small scanner AND cross-checks anthology_shell::analyze (same stages, words, redirects, nothing opaque). Table tests cannot show that each layer works alone, so in-crate tests call scan() and agrees_with_analyze() directly; sed mutants that removed one layer survived until those tests existed.
Rulings (also in DECISIONS.md ADR-032): a cd target must be absolute or ./ because CDPATH may be set in a shell snapshot a hook cannot see; unquoted ~ ^ # are refused (zsh EXTENDED_GLOB), so git diff HEAD~1 is not proven unless quoted; a glob that can expand to a name starting with - is refused outside --; jq filters naming env or import are refused (environment disclosure). Proven.paths are as written and a git operand counts as a possible path; the classifier never touches the file system, P4 (readonly_paths) does containment, symlinks, secrets and git config.
rtk gate: plain() resolves relative operands against the real post-cd directory but keeps the Read() matcher anchored at the hook cwd, as the rule scan does. rtk builds anthology-shell as a path dependency, so no lock change is needed for parser fixes. A head -n $N count is asked about by the existing rule scan (an expanding operand of a read command), not by the new code.
Why: the next person extending the table or the gate should not weaken a layer that a table test cannot see.
