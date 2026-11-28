//! The repository root and its files (F1, minimal; m-R11). BS1 and RL1 use the root helpers as
//! is; MT1 extends it with the `tests/_repo.py` data.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root: two levels above this crate's manifest.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `rel` (a `/`-separated path) under the repository root.
pub fn path(rel: &str) -> PathBuf {
    root().join(rel)
}

/// The UTF-8 text of `rel`; panics with the path when it cannot be read.
pub fn read(rel: &str) -> String {
    let p = path(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The tracked paths (`git ls-files`), or `None` when git cannot list them here: no git, or a
/// linked worktree whose git directory is outside the container.
pub fn tracked_files() -> Option<Vec<String>> {
    let out = Command::new("git")
        .args(["-c", "core.fsmonitor=false", "ls-files", "-z"])
        .current_dir(root())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    Some(
        listing
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// The knowledge folders of the anthology plugin (`KNOWLEDGE` in `tests/_repo.py`).
pub const KNOWLEDGE: &str = "plugins/anthology/knowledge";
/// The anthology-superpowers plugin folder (`SUPERPOWERS`).
pub const SUPERPOWERS: &str = "plugins/anthology-superpowers";

/// `x` is `knowledge/x/SKILL.md`; `ds/x` is `knowledge/design-skills/skills/x/SKILL.md`
/// (repo-relative, `/`-separated).
pub fn knowledge_path(reference: &str) -> String {
    match reference.strip_prefix("ds/") {
        Some(name) => format!("{KNOWLEDGE}/design-skills/skills/{name}/SKILL.md"),
        None => format!("{KNOWLEDGE}/{reference}/SKILL.md"),
    }
}

/// The SKILL.md of an anthology-superpowers skill (repo-relative).
pub fn superpowers_skill_path(name: &str) -> String {
    format!("{SUPERPOWERS}/skills/{name}/SKILL.md")
}

const KWP: &str = "anthropics-knowledge-work-plugins";
const UX: &str = "uxkin-agent";

/// The 22 connector-free knowledge folders (Part 1 section 6): folder name, upstream repo slug,
/// path inside it. `code-review` is shipped as `code-review-checklist`. Same order as
/// `STRIPPED_KNOWLEDGE` in `tests/_repo.py`.
pub fn stripped_knowledge() -> Vec<(String, &'static str, String)> {
    let mut out = Vec::new();
    for n in [
        "architecture",
        "system-design",
        "testing-strategy",
        "tech-debt",
        "documentation",
        "deploy-checklist",
    ] {
        out.push((n.to_string(), KWP, format!("engineering/skills/{n}")));
    }
    out.push((
        "code-review-checklist".to_string(),
        KWP,
        "engineering/skills/code-review".to_string(),
    ));
    for n in ["design-handoff", "research-synthesis"] {
        out.push((n.to_string(), KWP, format!("design/skills/{n}")));
    }
    for n in [
        "write-spec",
        "sprint-planning",
        "roadmap-update",
        "competitive-brief",
        "synthesize-research",
        "product-brainstorming",
    ] {
        out.push((n.to_string(), KWP, format!("product-management/skills/{n}")));
    }
    for n in [
        "seo-audit",
        "campaign-plan",
        "content-creation",
        "email-sequence",
    ] {
        out.push((n.to_string(), KWP, format!("marketing/skills/{n}")));
    }
    for n in ["landing-page", "web-review", "web-design-system"] {
        out.push((
            n.to_string(),
            UX,
            format!("plugins/website-design/skills/{n}"),
        ));
    }
    out
}

/// The folders `test_knowledge.UNCHANGED` names.
pub const UNCHANGED_KNOWLEDGE: [&str; 3] =
    ["frontend-design", "web-design-guidelines", "design-skills"];

/// `test_knowledge.EXPECTED_KNOWLEDGE`: the stripped 22, the unchanged 3 and seven more.
pub fn expected_knowledge() -> BTreeSet<String> {
    let mut set: BTreeSet<String> = stripped_knowledge()
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();
    set.extend(UNCHANGED_KNOWLEDGE.iter().map(|s| (*s).to_string()));
    set.extend(
        [
            "ui-taste",
            "caveman",
            "design-taste-frontend",
            "karpathy-guidelines",
            "website-clone",
            "humanizer",
            "avoid-ai-design",
        ]
        .map(String::from),
    );
    set
}

/// What an agent of the roster must be (`EXPECTED_ROSTER` in `tests/_repo.py`).
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub name: &'static str,
    pub model: &'static str,
    /// `"no"`, `"yes"` or `"docs"` (docs-only: Markdown in the main clone, no worktree).
    pub edits: &'static str,
    pub worktree: bool,
    /// anthology-superpowers skills in the agent's `skills`.
    pub preloads: Vec<&'static str>,
    /// Refs for [`knowledge_path`] read on every start (the always lines), in Startup order
    /// after caveman.
    pub knowledge: Vec<&'static str>,
    /// Refs on conditional Startup lines ("`, only ..."), in Startup order after `knowledge`.
    pub conditional: Vec<&'static str>,
}

/// The 11-agent roster (Part 1 section 5), in the order of `EXPECTED_ROSTER`.
pub fn expected_roster() -> Vec<AgentSpec> {
    let spec = |name, model, edits, worktree, preloads, knowledge, conditional| AgentSpec {
        name,
        model,
        edits,
        worktree,
        preloads,
        knowledge,
        conditional,
    };
    vec![
        spec(
            "researcher",
            "opus",
            "no",
            false,
            vec![],
            vec![
                "competitive-brief",
                "synthesize-research",
                "research-synthesis",
                "ds/ux-research",
                "ds/journey-mapping",
            ],
            vec![],
        ),
        spec(
            "architect",
            "opus",
            "no",
            false,
            vec!["writing-plans"],
            vec!["system-design", "architecture", "testing-strategy"],
            vec![],
        ),
        spec(
            "implementer",
            "sonnet",
            "yes",
            true,
            vec!["test-driven-development", "verification-before-completion"],
            vec!["karpathy-guidelines"],
            vec![],
        ),
        spec(
            "debugger",
            "sonnet",
            "yes",
            true,
            vec!["systematic-debugging"],
            vec!["karpathy-guidelines"],
            vec![],
        ),
        spec(
            "reviewer",
            "opus",
            "no",
            false,
            vec![],
            vec!["code-review-checklist", "karpathy-guidelines"],
            vec!["tech-debt", "deploy-checklist"],
        ),
        spec("scout", "haiku", "no", false, vec![], vec![], vec![]),
        spec(
            "docs-writer",
            "sonnet",
            "yes",
            true,
            vec![],
            vec!["documentation", "ds/ux-writing", "humanizer"],
            vec![],
        ),
        spec(
            "ui-designer",
            "sonnet",
            "yes",
            true,
            vec![],
            vec![
                "frontend-design",
                "design-taste-frontend",
                "ui-taste",
                "landing-page",
                "web-design-system",
                "design-handoff",
                "ds/design-systems",
                "ds/interaction-design",
                "ds/design-elevation",
                "avoid-ai-design",
            ],
            vec!["website-clone"],
        ),
        spec(
            "ui-reviewer",
            "opus",
            "no",
            false,
            vec![],
            vec![
                "web-design-guidelines",
                "web-review",
                "ds/design-critique",
                "ds/accessibility-audit",
                "avoid-ai-design",
            ],
            vec![],
        ),
        spec(
            "product-planner",
            "opus",
            "docs",
            false,
            vec![],
            vec![
                "write-spec",
                "sprint-planning",
                "roadmap-update",
                "product-brainstorming",
                "ds/ux-strategy",
                "ds/design-ops",
                "humanizer",
            ],
            vec![],
        ),
        spec(
            "marketer",
            "sonnet",
            "docs",
            false,
            vec![],
            vec![
                "seo-audit",
                "campaign-plan",
                "content-creation",
                "email-sequence",
                "humanizer",
            ],
            vec![],
        ),
    ]
}

/// The roster entry called `name`; panics when there is none.
pub fn roster_agent(name: &str) -> AgentSpec {
    expected_roster()
        .into_iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the roster"))
}

/// The sorted names of the entries of `rel` (a directory); panics when it cannot be read.
pub fn list_dir(rel: &str) -> Vec<String> {
    let p = path(rel);
    let mut names: Vec<String> = std::fs::read_dir(&p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_data_matches_tests_repo_py() {
        assert_eq!(expected_roster().len(), 11);
        assert_eq!(stripped_knowledge().len(), 22);
        assert_eq!(expected_knowledge().len(), 32);
        assert_eq!(roster_agent("ui-designer").knowledge.len(), 10);
        assert_eq!(roster_agent("ui-designer").conditional.len(), 1);
        assert!(knowledge_path("ds/x").ends_with("design-skills/skills/x/SKILL.md"));
    }

    #[test]
    fn the_root_holds_the_workspace_and_atl() {
        assert!(path("Cargo.toml").is_file());
        assert!(path("atl/Cargo.toml").is_file());
        assert!(read("rust-toolchain.toml").contains("[toolchain]"));
    }
}
