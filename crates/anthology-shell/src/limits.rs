//! Depth and size limits (SH1). The values are guard_rules.py's (`_MAX_DEPTH`, `_MAX_CWD_DEPTH`).

/// Nested shells and substitutions (`_MAX_DEPTH`).
pub const MAX_DEPTH: usize = 12;

/// `cd` tracking stops for a directory with more `/` than this (`_MAX_CWD_DEPTH`).
pub const MAX_CWD_DEPTH: usize = 64;

/// The run-time limits a test may lower (GC-19). `Limits::default()` is production.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Limits {
    /// fancy-regex's backtrack limit for the lookaround and backreference patterns (GC-16). Hitting it sets
    /// `incomplete`, which the guard blocks on (design §9.5, D-7).
    pub backtrack_limit: usize,
}

/// Work units one `segments` call may spend in the lenient fallback before it stops and sets `incomplete`:
/// each word read by an option rescan and each word or script copied costs 1 plus its length in bytes, so
/// the budget bounds memory as well as time. Python's quadratic-to-cubic `_script_starts` path reaches it
/// at a few hundred `fish -c` words in one unbalanced command (a few KB of text); Rust then blocks where
/// Python would only be slow (D-7). Below it the output is Python's exactly; past it, a correct prefix.
pub(crate) const LEX_BUDGET: usize = 16_000_000;

/// fancy-regex's own default (`RegexBuilder::backtrack_limit`), kept as GC-16 asks.
pub(crate) const DEFAULT_BACKTRACK_LIMIT: usize = 1_000_000;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            backtrack_limit: DEFAULT_BACKTRACK_LIMIT,
        }
    }
}
