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
