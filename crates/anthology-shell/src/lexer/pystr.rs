//! The CPython `str` semantics the parser needs (lead ruling R1): `anthology-shell` depends on regex and
//! fancy-regex only (design §7.1), so it cannot use `anthology-pycompat`. The tables come from CPython 3.12
//! itself (`port/extract/shell_unicode.py`); Rust's own `char` predicates use other definitions and another
//! Unicode version, so they are never used for these.
//!
//! `str.isalpha` is not here: the parser's one use (`_skip_flags`) is `token[-1].isalpha() and "-" +
//! token[-1] in value_flags`, and every value flag is `-` plus an ASCII letter, so the membership test alone
//! decides it.

use super::pystr_tables::{CASE_IGNORABLE, CASED, DIGIT, LOWER, SPACE};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `c.isspace()`.
pub(crate) fn is_space(c: char) -> bool {
    in_ranges(SPACE, c)
}

/// `s.isdigit()`: false for the empty string.
pub(crate) fn is_digit_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| in_ranges(DIGIT, c))
}

/// `s.strip()`.
pub(crate) fn strip_ws(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.split()`.
pub(crate) fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|t| !t.is_empty()).collect()
}

const CAPITAL_SIGMA: char = '\u{3A3}';

/// CPython's `handle_capital_sigma`: U+03A3 at char index `i` of `chars` is final when a cased character
/// precedes it and none follows (case-ignorable characters skipped both ways).
fn final_sigma(chars: &[char], i: usize) -> bool {
    let before = chars[..i]
        .iter()
        .rev()
        .find(|&&c| !in_ranges(CASE_IGNORABLE, c));
    if !before.is_some_and(|&c| in_ranges(CASED, c)) {
        return false;
    }
    let after = chars[i + 1..]
        .iter()
        .find(|&&c| !in_ranges(CASE_IGNORABLE, c));
    !after.is_some_and(|&c| in_ranges(CASED, c))
}

/// `s.lower()`, with full mappings and the Final_Sigma rule.
pub(crate) fn lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == CAPITAL_SIGMA {
            out.push(if final_sigma(&chars, i) {
                '\u{3C2}'
            } else {
                '\u{3C3}'
            });
        } else if let Ok(k) = LOWER.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
            out.push_str(LOWER[k].1);
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_from_cpython_3_12() {
        assert_eq!(super::super::pystr_tables::UNICODE_VERSION, "15.0.0");
    }

    #[test]
    fn python_whitespace_includes_the_separators() {
        for c in [
            '\t', '\n', '\x0b', '\x0c', '\r', '\x1c', '\x1f', ' ', '\u{85}', '\u{a0}', '\u{3000}',
        ] {
            assert!(is_space(c), "{c:?}");
        }
        for c in ['\u{180e}', '\u{200b}', 'a', '\0'] {
            assert!(!is_space(c), "{c:?}");
        }
        assert_eq!(strip_ws("\u{a0} x \x1c"), "x");
        assert_eq!(split_ws(" a\x1cb  c\u{85}"), vec!["a", "b", "c"]);
        assert!(split_ws(" \t ").is_empty());
    }

    #[test]
    fn python_digits() {
        assert!(is_digit_str("2"));
        assert!(is_digit_str("\u{b2}"));
        assert!(is_digit_str("\u{663}"));
        assert!(!is_digit_str(""));
        assert!(!is_digit_str("\u{bd}"));
        assert!(!is_digit_str("2a"));
    }

    #[test]
    fn python_lower() {
        assert_eq!(lower("GIT"), "git");
        assert_eq!(lower("\u{130}"), "i\u{307}");
        assert_eq!(lower("\u{212a}"), "k");
        assert_eq!(lower("\u{3a3}"), "\u{3c3}");
        assert_eq!(lower("\u{391}\u{3a3}"), "\u{3b1}\u{3c2}");
        assert_eq!(lower("\u{391}\u{3a3} \u{391}"), "\u{3b1}\u{3c2} \u{3b1}");
        assert_eq!(lower("\u{391}\u{3a3}\u{391}"), "\u{3b1}\u{3c3}\u{3b1}");
        assert_eq!(lower("\u{391}'\u{3a3}"), "\u{3b1}'\u{3c2}");
    }
}
