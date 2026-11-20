//! The agent roster (`memory_sync.ROSTER`), kept equal to `plugins/anthology/agents/*.md`.
//!
//! `port/difftest/oracle/data.py` compares it with the Python value.

/// The anthology agents, sorted.
pub const ROSTER: [&str; 11] = [
    "architect",
    "debugger",
    "docs-writer",
    "implementer",
    "marketer",
    "product-planner",
    "researcher",
    "reviewer",
    "scout",
    "ui-designer",
    "ui-reviewer",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_is_sorted_and_unique() {
        assert!(ROSTER.windows(2).all(|w| w[0] < w[1]));
    }
}
