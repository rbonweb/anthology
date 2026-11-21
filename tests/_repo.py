"""Shared helpers for the repo's tests."""
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
KNOWLEDGE = ROOT / "plugins" / "anthology" / "knowledge"
SUPERPOWERS = ROOT / "plugins" / "anthology-superpowers"


def load_frontmatter(path):
    """Return (frontmatter dict, body) for a markdown file with YAML frontmatter."""
    text = Path(path).read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        raise ValueError(f"{path}: missing frontmatter")
    head, sep, body = text[4:].partition("\n---")
    if not sep:
        raise ValueError(f"{path}: unterminated frontmatter")
    data = yaml.safe_load(head) or {}
    return data, body.lstrip("\n")


def knowledge_path(ref):
    """`x` -> knowledge/x/SKILL.md; `ds/x` -> knowledge/design-skills/skills/x/SKILL.md."""
    if ref.startswith("ds/"):
        return KNOWLEDGE / "design-skills" / "skills" / ref[3:] / "SKILL.md"
    return KNOWLEDGE / ref / "SKILL.md"


def superpowers_skill_path(name):
    return SUPERPOWERS / "skills" / name / "SKILL.md"


# The 22 connector-free knowledge folders (Part 1 section 6): folder name -> (upstream repo
# slug, path inside it). `code-review` is shipped as `code-review-checklist`.
_KWP = "anthropics-knowledge-work-plugins"
_UX = "uxkin-agent"
STRIPPED_KNOWLEDGE = {
    **{n: (_KWP, f"engineering/skills/{n}") for n in (
        "architecture", "system-design", "testing-strategy", "tech-debt", "documentation",
        "deploy-checklist")},
    "code-review-checklist": (_KWP, "engineering/skills/code-review"),
    **{n: (_KWP, f"design/skills/{n}") for n in ("design-handoff", "research-synthesis")},
    **{n: (_KWP, f"product-management/skills/{n}") for n in (
        "write-spec", "sprint-planning", "roadmap-update", "competitive-brief",
        "synthesize-research", "product-brainstorming")},
    **{n: (_KWP, f"marketing/skills/{n}") for n in (
        "seo-audit", "campaign-plan", "content-creation", "email-sequence")},
    **{n: (_UX, f"plugins/website-design/skills/{n}") for n in (
        "landing-page", "web-review", "web-design-system")},
}


# The 11-agent roster (Part 1 section 5). `knowledge`: refs for knowledge_path(), in Startup
# order after caveman. `preloads`: anthology-superpowers skills in the agent's `skills`.
# `edits`: "no" | "yes" | "docs" (docs-only: Markdown in the main clone, no worktree).
_UI_KNOWLEDGE = ["frontend-design", "design-taste-frontend", "ui-taste", "landing-page",
                 "web-design-system", "design-handoff", "ds/design-systems",
                 "ds/interaction-design", "ds/design-elevation"]
EXPECTED_ROSTER = {
    "researcher": {"model": "opus", "edits": "no", "worktree": False, "preloads": [],
                   "knowledge": ["competitive-brief", "synthesize-research",
                                 "research-synthesis", "ds/ux-research", "ds/journey-mapping"]},
    "architect": {"model": "opus", "edits": "no", "worktree": False,
                  "preloads": ["writing-plans"],
                  "knowledge": ["system-design", "architecture", "testing-strategy"]},
    "implementer": {"model": "sonnet", "edits": "yes", "worktree": True,
                    "preloads": ["test-driven-development", "verification-before-completion"],
                    "knowledge": ["karpathy-guidelines"]},
    "debugger": {"model": "sonnet", "edits": "yes", "worktree": True,
                 "preloads": ["systematic-debugging"], "knowledge": ["karpathy-guidelines"]},
    "reviewer": {"model": "opus", "edits": "no", "worktree": False, "preloads": [],
                 "knowledge": ["code-review-checklist", "tech-debt", "deploy-checklist"]},
    "scout": {"model": "haiku", "edits": "no", "worktree": False, "preloads": [],
              "knowledge": []},
    "docs-writer": {"model": "sonnet", "edits": "yes", "worktree": True, "preloads": [],
                    "knowledge": ["documentation", "ds/ux-writing"]},
    "ui-designer": {"model": "sonnet", "edits": "yes", "worktree": True, "preloads": [],
                    "knowledge": _UI_KNOWLEDGE},
    "ui-reviewer": {"model": "opus", "edits": "no", "worktree": False, "preloads": [],
                    "knowledge": ["web-design-guidelines", "web-review", "ds/design-critique",
                                  "ds/accessibility-audit"]},
    "product-planner": {"model": "opus", "edits": "docs", "worktree": False, "preloads": [],
                        "knowledge": ["write-spec", "sprint-planning", "roadmap-update",
                                      "product-brainstorming", "ds/ux-strategy",
                                      "ds/design-ops"]},
    "marketer": {"model": "sonnet", "edits": "docs", "worktree": False, "preloads": [],
                 "knowledge": ["seo-audit", "campaign-plan", "content-creation",
                               "email-sequence"]},
}
