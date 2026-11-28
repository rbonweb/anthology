---
name: 'Slop gate pre-release reviews (494ccb71, 28f45a5b, 1dec08a9)'
description: 'Round 3 at 1dec08a9: all review-2 repros fixed or as ruled, deadline works; open I7 em-dash after a clock-time or bare bold label'
metadata:
  type: project
  modified: 2026-11-28
---

Reviews live in .superpowers/sdd/2026-11-28-anti-slop/review-1.md, review-2.md and review-3.md. Round 1 (494ccb71) was closed by PRs 304 to 312, round 2 (28f45a5b) by 319 to 323.
Round 3 (1dec08a9) open items:
- I7 rules/density.rs LEAD_DASH skips date, version and list bold labels only. A clock-time lead (- 13:55 — ..., incident timelines, agendas), a bare **Term** — line (glossary) and a bold label followed by (...), : value or a code span still count; 6 of 38 repo dash hits come only from these.
- M10 plain list labels before a dash still count (owner call); M11 a triad can start on a clause-ending word (when relevant, navigation and motion).
- anthology.lock still pinned 0.2.9, so the gate stays inert until the lock bump.
Why: repeat these probes on the next round, and use the 14 fresh normal-work inputs of review-3 section 4 as a regression set.
Method: Zorin for build and tests (scripts/remote-build.sh run sh -c ... or test -p ...); one local rust-test.sh session for the hook probes, a node harness docker-copied in. The guardrails G0 refuses long printf payloads, so write harness files in chunks of about 1 KB. To judge density hits, rewrite label-led dashes as colons in a temp repo copy and re-run slop check. --clean-ours can race another agent run on Zorin; check docker ps and our labeled volumes before repeating it.
