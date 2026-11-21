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

/// The work budget of one analysis (or one `segments` call), fix-wave H's port contract: `WORK_PER_CHAR` units
/// per character of the command plus `WORK_BASE` (guard_rules.py `_WORK_PER_CHAR`, `_WORK_BASE`). It is spent
/// where Python's `_charge` spends it, in bytes (never fewer than Python's characters), so whatever Python
/// finds too complex, Rust does too (D-7: Rust may block more, never less).
pub const WORK_PER_CHAR: usize = 24;
/// See `WORK_PER_CHAR`.
pub const WORK_BASE: usize = 200_000;

/// The budget of the work Rust does beyond Python's charges (words stored in entries, alternates, GNU parallel
/// lines, the shell option rescans): `EXTRA_PER_BYTE` per byte plus `EXTRA_BASE`. It bounds time and memory on
/// those paths; running out is "too complex" too.
pub const EXTRA_PER_BYTE: usize = 128;
/// See `EXTRA_PER_BYTE`.
pub const EXTRA_BASE: usize = 4_000_000;

/// The work budget for `command` (Python's figures, by character).
pub fn work_limit(command: &str) -> usize {
    command
        .chars()
        .count()
        .saturating_mul(WORK_PER_CHAR)
        .saturating_add(WORK_BASE)
}

/// The Rust-only budget for `command`.
pub fn extra_limit(command: &str) -> usize {
    command
        .len()
        .saturating_mul(EXTRA_PER_BYTE)
        .saturating_add(EXTRA_BASE)
}

/// fancy-regex's own default (`RegexBuilder::backtrack_limit`), kept as GC-16 asks.
pub(crate) const DEFAULT_BACKTRACK_LIMIT: usize = 1_000_000;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            backtrack_limit: DEFAULT_BACKTRACK_LIMIT,
        }
    }
}
