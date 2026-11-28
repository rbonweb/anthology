---
name: 'check-repo optional tools: exit codes, JSON shapes, missing-DB signal'
description: 'What osv-scanner, gitleaks and zizmor really print and exit with, recorded from their images (tools.rs maps them)'
metadata:
  type: project
  modified: 2026-11-28
---

Why: crates/anthology-scan/src/tools.rs maps tool output to findings; these facts came from running the real images with --network none, not from docs. Re-check them if a tool is bumped.
osv-scanner (scan source -r --offline --config=<empty toml> --format json <target>): exit 0 none, 1 vulns found (JSON on stdout), 128 no lockfile found (no JSON), 127 error. No offline database: exit 127, stdout has an empty results list, stderr says "no offline version of the OSV database is available". JSON: results[].source.path (absolute, as passed), packages[].package{name,version}, packages[].vulnerabilities[].id and aliases. A MAL- record can be the id or only an alias; the same malware also arrives as GHSA and RUSTSEC ids. The crates.io database is only 4 MB, so it is the cheap one to download once for fixtures (rustdecimal 1.26.1 is a real MAL-2022-1 record). osv-scanner loads osv-scanner.toml per lockfile folder; an explicit --config wins.
gitleaks dir ... --redact --report-format json --report-path <file> --exit-code 0: exit 0 even with leaks; the report is a JSON array (RuleID, Description, StartLine, File absolute, Secret and Match redacted). Low-entropy or EXAMPLE tokens are not reported, so test fixtures need realistic ones. A .gitleaks.toml in the target is ignored once --config is given.
zizmor --offline --no-config --format json <target>: logs on stderr, JSON array on stdout; exit 14 with a High finding (0 none, 10-13 lower), exit 3 and no stdout when the tree has no workflow ("no inputs collected"). Path is locations[].symbolic.key.Local.verbatim_path (older releases: given_path), line is concrete.location.start_point.row plus 1, the finding location is the one with symbolic.kind Primary. A zizmor.yml in the repo hides findings unless --no-config is given.
