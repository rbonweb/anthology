---
name: 'A typed atl recursive grep over a tree with a secret asks, by design'
description: 'Why atl grep -rn x packages apps --include=*.ts asks in every mode, and what removes the prompt instead'
metadata:
  type: project
  modified: 2026-11-27
---

A typed atl grep -rn PATTERN dir1 dir2 --include=*.ts gets permissionDecision ask from the Claude hook in auto and default mode (atl 1.2.1-anthology.9). The Read() deny rules, two operand folders and the proof roots are not the cause: anthology perm classify refuses the line three times over, each refusal pinned on purpose. In crates/anthology-shell/src/readonly.rs walk refuses an option after an operand (option after an operand, pinned by grep foo a.txt -n) and an unquoted glob in an option (glob in an option, pinned by grep --include *.c foo .). Moved first and quoted, the line still fails in crates/anthology-cli/src/cmd/perm/paths.rs Checker::below: the recursive walk finds a secret-named file (a test .pem fixture in that project) and does not model --include, so it cannot show the search skips it. Without the ask, Bash(atl *) would approve a line Claude Code may prompt for, so asking is correct.

Why: making it provable needs three relaxations of the shared prover, and an --include-aware walk must model GNU grep, BSD grep and ugrep (the snapshot grep) alike; that is a design change for the security review, not a bug fix.

How to apply: for repeated prompts on typed atl lines, steer agents to plain commands (ATL condenses them itself), not the prover. To reproduce, pipe a request with a leading cd into the plugin bin/anthology perm classify; the refusal reason names the step.
