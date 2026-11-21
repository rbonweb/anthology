//! Cases, the deterministic generator and the `Surface` trait (F4, plan §3.7).
//!
//! A surface lane fills `surfaces/<surface>.rs` with a type implementing [`Surface`] and returns
//! it from that file's `surface()`:
//!
//! - `seeds()`: the fixed cases (from the committed goldens, or extracted Python test inputs);
//! - `generate(rng, n)`: `n` random cases, drawn only from `rng` (so a run is reproducible from
//!   `--seed`);
//! - `run_python(case, oracle)`: what Python does, usually
//!   `observe::from_oracle(&oracle.call("<oracle module>", &case.data))`;
//! - `run_rust(case)`: what Rust does, in-process or through the built binaries.
//!
//! Case ids are `golden:<line>` for seeds read from a golden file and `gen:<n>` for generated
//! cases, so a divergence entry can name them (`port/difftest/divergences/<surface>.toml`).
#![allow(
    dead_code,
    reason = "published for the surface lanes; not every item has a caller yet"
)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anthology_pycompat::json::{self, Value};

use crate::observe::Observed;
use crate::oracle::Oracle;

/// One input of a surface: an id and the data both sides get.
#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub data: Value,
}

impl Case {
    pub fn new(id: impl Into<String>, data: Value) -> Case {
        Case {
            id: id.into(),
            data,
        }
    }

    /// The data as one JSON line, cut to `max` characters (for reports).
    pub fn compact(&self, max: usize) -> String {
        cut(&json::dumps(&self.data, &json::PY_DEFAULT_UNICODE), max)
    }
}

/// `s` cut to `max` characters, with `…(N more)` when cut.
pub fn cut(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…({} more)", n - max)
}

/// A difftest surface (plan §3.7, plus the defaulted `rust_stricter`).
pub trait Surface {
    fn name(&self) -> &'static str;
    fn seeds(&self) -> Vec<Case>;
    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case>;
    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed;
    fn run_rust(&self, c: &Case) -> Observed;

    /// For a case whose two sides differ: whether Rust is the stricter side (it blocks, or reports
    /// more to block on, where Python allows). A `D-8` entry waives such a diff only (final review
    /// M-3); `Some(false)` makes any other diff on a D-8 case unexplained. `None` (the default):
    /// the surface has no direction to check, and its D-8 entries waive as declared.
    fn rust_stricter(&self, _c: &Case, _python: &Observed, _rust: &Observed) -> Option<bool> {
        None
    }
}

/// A deterministic generator (SplitMix64). The same seed gives the same sequence everywhere.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// A generator for one surface: `seed` mixed with the surface's name, so surfaces do not
    /// share a sequence.
    pub fn for_surface(seed: u64, name: &str) -> Rng {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        Rng(seed ^ h)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; panics when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng::below(0)");
        // Lemire's method with rejection: no modulo bias.
        loop {
            let m = u128::from(self.next_u64()) * u128::from(n);
            let low = m as u64;
            if low >= n.wrapping_neg() % n {
                return (m >> 64) as u64;
            }
        }
    }

    /// Uniform in `lo..hi` (`lo < hi`).
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo < hi, "Rng::range({lo}, {hi})");
        lo.wrapping_add(self.below(hi.wrapping_sub(lo) as u64) as i64)
    }

    /// Uniform in `0..n` as a `usize`.
    pub fn index(&mut self, n: usize) -> usize {
        self.below(n as u64) as usize
    }

    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `true` with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    /// One element of `xs` (not empty).
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.index(xs.len())]
    }

    /// Shuffles `xs` in place.
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            xs.swap(i, self.index(i + 1));
        }
    }

    /// A finite double from a random 64-bit pattern (all exponents, subnormals, `-0.0`).
    pub fn float_bits(&mut self) -> f64 {
        loop {
            let f = f64::from_bits(self.next_u64());
            if f.is_finite() {
                return f;
            }
        }
    }

    /// A printable ASCII character.
    pub fn ascii_char(&mut self) -> char {
        char::from(0x20 + self.below(0x5f) as u8)
    }

    /// A character from a mix: printable ASCII, controls, Latin-1, the BMP, the specials
    /// (U+2028, U+FEFF, U+FFFF, NBSP, NEL, U+3000) and astral planes. Never a surrogate.
    pub fn unicode_char(&mut self) -> char {
        let k = self.f64();
        let c = if k < 0.45 {
            0x20 + self.below(0x5f) as u32
        } else if k < 0.55 {
            self.below(0x20) as u32
        } else if k < 0.62 {
            0x7f + self.below(0x81) as u32
        } else if k < 0.80 {
            // 0x100..0xFFFE without the surrogates.
            let c = 0x100 + self.below(0xfefe - 0x800) as u32;
            if c >= 0xd800 { c + 0x800 } else { c }
        } else if k < 0.85 {
            *self.pick(&[
                0x2028, 0x2029, 0xfeff, 0xffff, 0xfffe, 0xa0, 0x85, 0x3000, 0xfffd, 0x1c,
            ])
        } else {
            0x10000 + self.below(0x100000) as u32
        };
        char::from_u32(c).unwrap_or('\u{fffd}')
    }

    /// Up to `max_len` characters from [`Rng::unicode_char`].
    pub fn string(&mut self, max_len: usize) -> String {
        let n = self.index(max_len + 1);
        (0..n).map(|_| self.unicode_char()).collect()
    }

    /// Up to `max_len` printable ASCII characters.
    pub fn ascii_string(&mut self, max_len: usize) -> String {
        let n = self.index(max_len + 1);
        (0..n).map(|_| self.ascii_char()).collect()
    }
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Sets the repository root (`--root`); only the first call counts.
pub fn set_repo_root(path: PathBuf) {
    let _ = ROOT.set(path);
}

/// The repository root: `--root`, else the first folder up from the working folder that holds
/// `port/difftest/oracle/_driver.py`, else the one this binary was built from.
pub fn repo_root() -> &'static Path {
    ROOT.get_or_init(|| {
        if let Ok(mut dir) = std::env::current_dir() {
            loop {
                if dir.join("port/difftest/oracle/_driver.py").is_file() {
                    return dir;
                }
                if !dir.pop() {
                    break;
                }
            }
        }
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    })
}

/// The `case` of every line of a golden file (`{"case": ..., "result": ...}` per line) under
/// the repository root, as seeds `golden:<line>`. Panics with the path when it is unreadable.
pub fn golden_seeds(rel: &str) -> Vec<Case> {
    let path = repo_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("difftest: cannot read {}: {e}", path.display()));
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, line)| {
            let mut v =
                json::loads(line).unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), i + 1));
            let case = v
                .as_object_mut()
                .and_then(|o| o.remove("case"))
                .unwrap_or_else(|| panic!("{}:{}: no \"case\"", path.display(), i + 1));
            Case::new(format!("golden:{}", i + 1), case)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic_and_splitmix64() {
        // SplitMix64's published first outputs for seed 0.
        let mut r = Rng::new(0);
        assert_eq!(r.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.next_u64(), 0x6e78_9e6a_a1b9_65f4);
        let a: Vec<u64> = (0..5).map(|_| Rng::new(7).next_u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        assert_ne!(
            Rng::for_surface(1, "core.state").next_u64(),
            Rng::for_surface(1, "core.lock").next_u64()
        );
    }

    #[test]
    fn below_range_chance_and_pick_stay_in_bounds() {
        let mut r = Rng::new(42);
        let mut seen = [0usize; 7];
        for _ in 0..7000 {
            seen[r.below(7) as usize] += 1;
            let x = r.range(-3, 4);
            assert!((-3..4).contains(&x));
            let f = r.f64();
            assert!((0.0..1.0).contains(&f));
        }
        assert!(seen.iter().all(|&n| n > 800), "{seen:?}");
        assert!(!r.chance(0.0));
        assert!(r.chance(1.0));
        assert_eq!(*r.pick(&[5]), 5);
        let mut xs = [1, 2, 3, 4, 5];
        r.shuffle(&mut xs);
        xs.sort_unstable();
        assert_eq!(xs, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn strings_mix_planes_and_never_hold_surrogates() {
        let mut r = Rng::new(3);
        let s: String = (0..2000).map(|_| r.unicode_char()).collect();
        assert!(s.chars().any(|c| c as u32 > 0xffff));
        assert!(s.chars().any(|c| (c as u32) < 0x20));
        assert!(s.chars().any(|c| (0x100..0xd800).contains(&(c as u32))));
        assert!(r.string(5).chars().count() <= 5);
        assert!(r.ascii_string(9).chars().all(|c| (' '..='~').contains(&c)));
        assert!(r.float_bits().is_finite());
    }

    #[test]
    fn the_repo_root_holds_the_oracle_driver() {
        assert!(
            repo_root()
                .join("port/difftest/oracle/_driver.py")
                .is_file()
        );
    }

    #[test]
    fn golden_seeds_read_the_cases_in_order() {
        let seeds = golden_seeds("crates/anthology-core/tests/golden/lock.jsonl");
        assert!(seeds.len() > 500);
        assert_eq!(seeds[0].id, "golden:1");
        assert!(
            seeds[0]
                .data
                .as_object()
                .is_some_and(|o| o.get("prefix").is_some())
        );
        assert_eq!(cut("abcdef", 3), "abc…(3 more)");
        assert_eq!(cut("abc", 3), "abc");
    }
}
