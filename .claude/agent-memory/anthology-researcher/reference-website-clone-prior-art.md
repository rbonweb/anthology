---
name: 'Website clone prior art (survey 2026-11-28)'
description: 'Open-source clone-website skills and design-token extractors, with stars, licenses and browser needs'
metadata:
  type: reference
  modified: 2026-11-28
---

Best vendorable base: JCodesMore/ai-website-cloner-template (MIT, ~40k stars, pushed 2026-11-27). Its .agents/skills/clone-website/SKILL.md is browser-tool-agnostic (no Playwright or Chrome MCP named), so it fits a BrowserOS Neo only rule. Others: dembrandt/dembrandt-skills clone-website and extract-design skills (MIT); dembrandt/dembrandt CLI and Manavarya09/design-extract are Playwright CLIs that download Chromium, so they fail the no-browser-install rule unless run in Docker. FrancosCorporation/ai-site-cloner has the best pixel-diff loop (pixelmatch, 95 percent per section at 390/768/1440) but needs Playwright. abi/screenshot-to-code and firecrawl/open-lovable need LLM API keys; open-lovable also needs Firecrawl and a sandbox. julianromli/ai-skills and santowilem/skills had no license file shown on GitHub API for julianromli. Why saved: avoids repeating this survey. Stars and dates are from 2026-11-28.
