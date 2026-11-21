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

/// The work budget of one analysis (or one `segments` call): `WORK_PER_BYTE` units per byte of the command
/// plus `WORK_BASE`. Every scan charges the bytes it visits and every stored word `WORD` plus its bytes, so the
/// budget bounds time and memory whatever path an input takes. Python's own contract is 24 per character plus
/// 200 000 (`_WORK_PER_CHAR`, `_WORK_BASE`); Rust allows more, so where Python stops ("too complex") Rust may
/// still finish (an oracle-marked D-7), never the reverse for realistic commands.
pub const WORK_PER_BYTE: usize = 64;
/// See `WORK_PER_BYTE`.
pub const WORK_BASE: usize = 1_000_000;

/// The work budget for a command of `len` bytes.
pub fn work_limit(len: usize) -> usize {
    len.saturating_mul(WORK_PER_BYTE).saturating_add(WORK_BASE)
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
