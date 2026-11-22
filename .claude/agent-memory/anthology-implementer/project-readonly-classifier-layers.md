---
name: 'classify_readonly: two readers, test each layer alone'
description: 'How the read-only classifier and the rtk cd/head gate are built, the review rulings behind their refusals, and the runtime they are written for'
metadata:
  type: project
  modified: 2026-11-22
---

classify_readonly (crates/anthology-shell/src/readonly.rs) has its own small scanner AND cross-checks anthology_shell::analyze (same stages, words, redirects, nothing opaque). Table tests cannot show that each layer works alone, so in-crate tests call scan() and agrees_with_analyze() directly, and prove_with(command, home, scanner) takes the scanner so a test can hand it a lying one. Without that, deleting the analyze call broke no test (review finding I4).
The real runtime is macOS BSD tools under zsh, with bfs for find and ugrep for grep. Rulings (ADR-032): a word starting with a dash after the first operand (or a double dash after one) is not proven for any program, because BSD getopt reads it as a file and uniq a -c writes it. zsh numeric globs and a word that opens with an equals sign after quote removal (the empty-quote form too) are refused, and the rtk gate has the same equals and option-after-operand rules. A cd target must be absolute or start with ./ (CDPATH). Unquoted tilde, caret and hash are refused (EXTENDED_GLOB). A glob that can expand to a dash name is refused outside a double dash. jq filters naming env or import are refused.
Proven carries paths as ReadPath with path and recursive, plus cd and runs_git_config (true when any stage is git; the doc comment lists the git config keys P4 must check). SECRET_PATTERNS and is_secret_path refuse obvious secret paths as defence in depth; a glob needs two literal characters to be held against secret names, or ls i* style commands are refused for files that do not exist. The classifier never touches the file system; P4 (readonly_paths) does containment, symlinks, Read() rules and git config. Nothing may allow a Proven alone.
rtk gate: plain() resolves relative operands against the real post-cd directory but keeps the Read() matcher anchored at the hook cwd. rtk builds anthology-shell as a path dependency, so no lock change is needed for parser fixes.
Why: the next person extending the table or the gate should not weaken a layer that a table test cannot see, and should know which platform the tables are written for.
