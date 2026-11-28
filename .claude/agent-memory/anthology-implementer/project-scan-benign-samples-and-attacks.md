---
name: 'check-repo attack and benign tests: how the samples were fetched, what to expect'
description: 'tests/attacks.rs and tests/benign.rs layout, the fetch recipe for real sample files, and the MCP dnx false alarm'
metadata:
  type: project
  modified: 2026-11-28
---

crates/anthology-scan/tests/attacks.rs builds each A-case in a tempdir and asserts rule, severity and report::exit_code. tests/benign/<name>/ folders are scanned in place; SOURCES.md must have a row with the backticked folder name (a test checks it) and tests/benign/LICENSES holds the license texts. Fetch recipe that passes the no-cd, no-git-C rule: git clone --depth 1 --filter=blob:none --no-checkout URL /tmp/scratch/x, then git --git-dir=/tmp/scratch/x/.git --work-tree=/tmp/scratch/x checkout HEAD -- path (a tag works after git --git-dir=... fetch --depth 1 --filter=blob:none origin tag T), then cp. Find real files with gh api -X GET search/code -f q=... (for example PostToolUse prettier filename:settings.json path:.claude). Real finds: copilot extension tasks.json inside microsoft/vscode has a folderOpen task with reveal never; anthropics/claude-code-action has a bunx prettier@3.5.3 hook; ChilliCream/graphql-platform .mcp.json is pinned. Why it matters: a pinned MCP server with an https URL in its args (dotnet dnx NuGet.Mcp.Server@1.4.16 --source https://api.nuget.org/...) classifies as Network, so Open plus Network gives HIGH: a real false alarm the lead has to rule on. A homoglyph test name must keep every Latin letter but the swapped one (gradl plus Cyrillic ie), or the twin lookup finds no twin.
