//! The two regular expressions of `Trim.drop`, as a hand-written scanner (no regex crate).
//!
//! * `\b(fn|struct|enum|mod|const|static|type)\s+(\w+)` searched in a signature: [`first_declaration`].
//! * `\b<keyword>\s+<name>\b` searched in a whole file, with the two groups of the first one
//!   filled in (this is the `Trim.drop:present` row of `port/regex-inventory.tsv`, which had no
//!   Rust shape because the pattern is built at run time): [`declaration_present`].
//!
//! Neither pattern has a lookaround or a backreference. `\w` and `\s` are Python's
//! (`pycompat::str::is_word`, `pycompat::str::is_space`). The quirk of the first one is kept on purpose: in
//! `const fn foo` the keyword is `const` and the name is `fn`.

use anthology_pycompat::str::{is_space, is_word};

/// The keyword alternatives, in the order of the pattern.
const KEYWORDS: [&str; 7] = ["fn", "struct", "enum", "mod", "const", "static", "type"];

/// Whether `\b` holds before byte offset `at` of `s`, where `s[at..]` starts with a word
/// character (so a boundary means: nothing before it, or a non-word character).
fn boundary_before_word(s: &str, at: usize) -> bool {
    s[..at].chars().next_back().is_none_or(|c| !is_word(c))
}

/// `re.search(r"\b(fn|struct|enum|mod|const|static|type)\s+(\w+)", sig)`: the keyword and the
/// name of the leftmost match.
pub fn first_declaration(sig: &str) -> Option<(&'static str, &str)> {
    for (at, _) in sig.char_indices() {
        if !boundary_before_word(sig, at) {
            continue;
        }
        for keyword in KEYWORDS {
            let Some(rest) = sig[at..].strip_prefix(keyword) else {
                continue;
            };
            let after_space = rest.trim_start_matches(is_space);
            if after_space.len() == rest.len() {
                continue;
            }
            let name_len = after_space
                .char_indices()
                .find(|&(_, c)| !is_word(c))
                .map_or(after_space.len(), |(i, _)| i);
            if name_len > 0 {
                return Some((keyword, &after_space[..name_len]));
            }
        }
    }
    None
}

/// `re.search(r"\b%s\s+%s\b" % (keyword, name), text) is not None`, for a `keyword` and a `name`
/// that are made of word characters only (as `first_declaration` returns them).
///
/// `\b` is Python's: a non-ASCII letter or digit next to the keyword or the name is part of a
/// longer word (no boundary), a combining mark is not.
pub fn declaration_present(text: &str, keyword: &str, name: &str) -> bool {
    text.match_indices(keyword).any(|(at, _)| {
        if !boundary_before_word(text, at) {
            return false;
        }
        let rest = &text[at + keyword.len()..];
        let after_space = rest.trim_start_matches(is_space);
        if after_space.len() == rest.len() {
            return false;
        }
        after_space
            .strip_prefix(name)
            .is_some_and(|tail| tail.chars().next().is_none_or(|c| !is_word(c)))
    })
}
