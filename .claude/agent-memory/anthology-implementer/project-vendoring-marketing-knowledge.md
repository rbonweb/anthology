---
name: 'Vendoring marketing knowledge: pin handling and extra patch spots'
description: 'vendor sync re-resolves a branch ref; marketingskills v2.14.29 holds more tool links than the plan table lists'
metadata:
  type: project
  modified: 2026-11-28
---

For an entry whose ref is a branch (main), `scripts/dev.sh vendor sync <name> --ref main` re-resolves to the branch head and `--ref <sha>` rewrites the ref field to the sha. To keep ref main with the pinned commit, add the entry, then run `scripts/dev.sh vendor fetch <name>` (also writes vendor/licenses) and `scripts/dev.sh vendor build <name>`. A tag ref (v2.14.29) is safe with sync: it re-resolves to the same commit and proves the pin. The patch recipe is: edit the built folder, `scripts/dev.sh vendor diff <name> > vendor/patches/<name>.patch`, add the {"patch": ...} transform after drop-paths, run `vendor build` then `vendor check` (prints "vendor check: clean").
Why: the knowledge_marketing guard test (no tools/registry.md, tools/integrations/, dev-browser, brew install, ~~, unresolved links) finds spots the plan table did not list: tools/REGISTRY.md links in social/references/short-form-video.md, ad-creative SKILL.md, launch/references/site-launch-qa.md, programmatic-seo implementation-platforms.md, ai-seo SKILL.md and agent-readiness.md; a strikethrough in ai-seo SKILL.md; ../../ads/ links in ad-creative; a stale "Publishing From Your Agent" pointer in social/references/ship-post.md. Re-run the guard test after each upstream bump and patch what it flags.
