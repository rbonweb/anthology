---
name: anthology-user-workflow
description: "How the user works on anthology — approvals, voice dictation, naming, publishing and credit rules"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 1c9fb279-a45d-4d27-8e78-1ddd82c1e344
  modified: 2026-11-20T09:14:01.295Z
---

- **Plan first, approval second, build third.** Every phase waits for an explicit go. Give plans and recommendations, not surveys.
- **Voice-dictated messages.** Interpret the intent ("scope pil cockpit" = "copied skills", "design scale" = "design-skills", "Kie skill" = "caveman skill"). If they change an answer mid-turn, the latest message wins.
- **Builds use subagents matched to roster roles**, not everything in the main session.
- **Naming and repo:** no "Claude" in the product or repo name. GitHub `rbonweb/anthology` is created private and goes public only when the user says so.
- **Credits:** the user adds third-party credits and license notices themselves at the end. Don't do credit work during the build.
- **Product philosophy:** opinionated and preconfigured. No user-facing modes, levels or toggles. Every skill belongs to an agent; no loose skills.

**Why:** These are the user's stated preferences across the 2026-11-20 planning session.

**How to apply:** Follow these for any anthology work. For open decisions, see [[open-decisions-as-questions]].
