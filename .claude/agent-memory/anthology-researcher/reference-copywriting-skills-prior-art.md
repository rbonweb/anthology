---
name: 'Copywriting skills prior art (survey 2026-11-28)'
description: 'Open-source copy skills per content type: landing, UI microcopy, blog/SEO, email/social/PR, with stars and licenses'
metadata:
  type: reference
  modified: 2026-11-28
---

Why: owner wants one copy skill per content type for marketer and docs-writer; this saves re-surveying.
Landing/page copy: coreyhaines31/marketingskills skills/copywriting (63k stars, MIT, 54 evals.json files across skills, copy-editing encodes Copyhackers Seven Sweeps). Lacks PAS/AIDA/StoryBrand picker; OneWave-AI/claude-skills landing-page-copywriter (449 stars, MIT) has that picker.
UI microcopy: cuellarfr/design-skills ux-writing is a variant copy of content-designer/ux-writing-skill (269 stars, MIT), so docs-writer already has it. humbleteam/ux-writing (MIT, 3 stars) is the tightest per-element rule table. No open skill covers dashboard data labels, number formats or truncation; sources are Carbon content and overflow pages (carbon-website repo Apache-2.0), GitLab Pajamas charts, GOV.UK error messages (OGL v3, attribution) and NN/g error and empty-state articles (cite only, not open).
Blog: AgriciDaniel/claude-blog (2.9k stars, MIT, 32 skills, 5 agents, no keys needed; Gemini images optional paid). Its blog-discourse does Reddit/HN/X trend research with WebSearch only. marketingskills content-strategy plans clusters but has no article writer.
Trends: mvanhorn/last30days-skill (76k stars, MIT) needs Python 3.12, auto-installs CLIs and can read browser cookies, which clashes with Anthology browser and Docker rules. Google Trends API still closed alpha in Nov 2026; pytrends archived.
anthropics/knowledge-work-plugins has design/skills/ux-copy and marketing/skills/draft-content, both shallow; anthropics/skills has no copy skill beyond internal-comms.
