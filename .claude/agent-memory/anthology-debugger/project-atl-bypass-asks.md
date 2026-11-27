---
name: 'ATL asks for atl bypass: runs_bypass and without_bypass (PR 259)'
description: 'How the Claude hook stops Bash(atl *) from approving atl bypass, what is proven read-only, and the traps'
metadata:
  type: project
  modified: 2026-11-27
---

PR 259 (atl 1.2.1-anthology.9) makes the Claude hook ask for any typed atl bypass or atl proxy line, so the Bash(atl *) allow never approves it. Before, with no deny, ask or Read rule the gate was inert and an already-atl command rewrote to nothing, so the hook said nothing and the host allowed any command behind atl bypass.

Where: anthology_gate.rs runs_bypass (scanner flag set in run_view from AtlInner.bypass and in scan_analysis from entry_via_bypass; runs whether or not any rule exists, gated by mentions_token_layer), wired in decision::decide_for_claude_hook_at as Gate::Ask. read_only_proof.rs proof closure proves without_bypass(cmd) instead of the line, and refuses when runs_bypass still sees a bypass in it. without_bypass strips only the bare word atl or rtk (legacy) at a command start (line start, after && || ; newline or a pipe), atl global flags, bypass or proxy, then an unquoted inner word; a path to atl, env, an assignment, a launcher, quotes all stay and so ask. Newlines are never proven (proven_read_only refuses them).

Why it is built so: the real classifier refuses any atl line, but the test stand-in (FAKE_PROVE_ALL) passes everything, so ATL re-scans the stripped line itself instead of trusting the classifier. A path-qualified atl could be any program, so it is never stripped.

Traps: five existing tests named inert_in_a_bare_sandbox listed atl bypass lines as silent; they now assert an ask (assert_bypass_asks). Not covered, said in SECURITY.md and docs/atl.md: atl run, err, test, summary and atl <program> still run a command under Bash(atl *) when no rule exists. BYPASS_NOTE in crates/anthology-settings/src/ops.rs and the copy in the setup SKILL.md still say the allow approves atl bypass; changing it needs an anthology-bin release and a setup golden bless. Bumping atl/Cargo.toml, decision.rs or hook_cmd.rs moves the dev.rtk-trim golden: bless under umask 022 with scripts/dev.sh golden bless --surface dev.rtk-trim. Run atl tests with scripts/atl-test.sh --test NAME; rustfmt touched files only (cargo fmt --all fails on the trimmed tree); the macOS check is a throwaway draft PR whose only change is a comment line in .github/workflows/atl-release.yml, based on the fix branch.
