---
name: 'Malicious-repo scan prior art (survey 2026-11-28)'
description: 'Which local tools detect malicious code in a cloned repo, their network and repo-config traps, and when Claude Code runs repo hooks'
metadata:
  type: reference
  modified: 2026-11-28
---

Full report: .superpowers/sdd/2026-11-28-check-repo/research.md. Why: /anthology:check-repo design depends on these facts; re-surveying costs hours.
Best rule tool: GuardDog (Apache-2.0, YARA source rules, F1 93.32, about 2.5% FP on arXiv 2603.27549 npm benchmark). Its rules can run in the yara-x Rust crate (BSD-3).
OSV-Scanner matches OSSF MAL- malicious versions; default mode sends package names to OSV.dev, use --offline with a downloaded DB.
Repo-config traps: gitleaks loads target .gitleaks.toml unless --config; zizmor loads repo zizmor.yml unless --no-config; semgrep honors .semgrepignore and nosemgrep; trufflehog verifies secrets online unless --no-verification. Snyk agent-scan uploads content and launches MCP servers: avoid.
Claude Code (permissions docs, What runs before you trust a folder): -p and SDK runs use repo hooks, env, apiKeyHelper and connect .mcp.json servers with no trust dialog; interactive runs use them once trust is accepted. So a check inside a trusted session is too late for agent-config attacks.
Reading a file under a folder auto-loads that folder CLAUDE.md into context (seen with atl/CLAUDE.md), so an LLM scanner must not Read the target repo itself.
Some security-themed WebSearch queries tripped the model safety filter; neutral, defensive wording worked.
