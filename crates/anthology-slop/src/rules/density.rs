//! The density rules: em dashes and lists of three, counted over the new text only.
//!
//! Both read `view.text`. A dash, a list or a word counts when it touches a new span, so a page
//! that was already dense is not blamed for an edit that adds one line. A word is a
//! whitespace-separated token that holds a letter; a MASK byte also ends a token.

use std::sync::LazyLock;

use regex::Regex;

use crate::{Hit, MASK, Span, View};

const EM_DASH: &str = "text.em-dash-density";
const TRIAD: &str = "text.triad-density";

/// At least this many before the density is looked at.
const MIN_COUNT: usize = 3;
/// At most one em dash per this many words.
const WORDS_PER_DASH: usize = 120;
/// At most one list of three per this many words.
const WORDS_PER_TRIAD: usize = 150;

/// `spans` sorted, with the empty ones dropped and the touching ones joined.
fn merged(spans: &[Span]) -> Vec<Span> {
    let mut sorted: Vec<Span> = spans.iter().copied().filter(|s| s.start < s.end).collect();
    sorted.sort();
    let mut out: Vec<Span> = Vec::with_capacity(sorted.len());
    for span in sorted {
        match out.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => out.push(span),
        }
    }
    out
}

/// True when `span` overlaps one of `spans` (as `merged` returns them).
fn touches(spans: &[Span], span: Span) -> bool {
    let i = spans.partition_point(|s| s.end <= span.start);
    spans.get(i).is_some_and(|s| s.start < span.end)
}

/// The words of `text` that touch `new`.
fn words(text: &str, new: &[Span]) -> usize {
    let mut count = 0;
    let mut start = None;
    let mut letter = false;
    // A closing space makes the last token end like the others.
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        if c.is_whitespace() || c == char::from(MASK) {
            if let Some(from) = start.take()
                && letter
                && touches(
                    new,
                    Span {
                        start: from,
                        end: i,
                    },
                )
            {
                count += 1;
            }
            letter = false;
        } else {
            start.get_or_insert(i);
            letter |= c.is_alphabetic();
        }
    }
    count
}

/// A density hit on `first`, when `count` lists or dashes are too many for `words` words.
fn hit(
    rule: &'static str,
    first: Option<Span>,
    count: usize,
    per: usize,
    words: impl FnOnce() -> usize,
    what: &str,
) -> Vec<Hit> {
    let Some(span) = first else {
        return Vec::new();
    };
    if count < MIN_COUNT {
        return Vec::new();
    }
    let words = words();
    if count * per <= words {
        return Vec::new();
    }
    vec![Hit {
        rule,
        span,
        parts: Vec::new(),
        excerpt: format!("{count} {what} in {words} words (at most 1 per {per})"),
    }]
}

/// Em dashes (U+2014) and spaced en dashes (a space, U+2013, a space) outside `dash_skip`.
pub(crate) fn em_dash(view: &View, new: &[Span]) -> Vec<Hit> {
    let new = merged(new);
    let skip = merged(&view.dash_skip);
    let text = &view.text;
    let mut count = 0;
    let mut first = None;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        let dash = match c {
            '\u{2014}' => true,
            '\u{2013}' => text[..i].ends_with(' ') && text[end..].starts_with(' '),
            _ => false,
        };
        let span = Span { start: i, end };
        if dash && touches(&new, span) && !touches(&skip, span) {
            count += 1;
            first.get_or_insert(span);
        }
    }
    hit(
        EM_DASH,
        first,
        count,
        WORDS_PER_DASH,
        || words(text, &new),
        "em dashes",
    )
}

static LISTS_OF_THREE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[a-z]{4,},\s+[a-z]{4,},?\s+(?:and|or)\s+[a-z]{4,}\b").expect("triad pattern")
});

/// Lists of three words of four letters or more ("plans, specs and roadmaps").
pub(crate) fn triad(view: &View, new: &[Span]) -> Vec<Hit> {
    let new = merged(new);
    let text = &view.text;
    let mut count = 0;
    let mut first = None;
    for m in LISTS_OF_THREE.find_iter(text) {
        let span = Span {
            start: m.start(),
            end: m.end(),
        };
        if touches(&new, span) {
            count += 1;
            first.get_or_insert(span);
        }
    }
    hit(
        TRIAD,
        first,
        count,
        WORDS_PER_TRIAD,
        || words(text, &new),
        "lists of three",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MASK;

    const DASH: char = '\u{2014}';
    const EN: char = '\u{2013}';

    fn whole(doc: &str) -> Vec<Span> {
        vec![Span {
            start: 0,
            end: doc.len(),
        }]
    }

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    /// `n` filler words, each `word`.
    fn filler(n: usize) -> String {
        vec!["word"; n].join(" ")
    }

    /// The em-dash hits of a plain view with the whole document new.
    fn dashes(doc: &str) -> Vec<Hit> {
        em_dash(&View::plain(doc), &whole(doc))
    }

    fn triads(doc: &str) -> Vec<Hit> {
        triad(&View::plain(doc), &whole(doc))
    }

    // Twenty words with three dashes: "one {DASH} two ..." and so on.
    fn dashed_twenty() -> String {
        format!(
            "alpha {DASH} beta gamma {DASH} delta epsilon {DASH} zeta {}",
            filler(14)
        )
    }

    #[test]
    fn three_dashes_in_twenty_words_fire() {
        let doc = dashed_twenty();
        let hits = dashes(&doc);
        assert_eq!(
            hits,
            [Hit {
                rule: "text.em-dash-density",
                span: sp(6, 6 + DASH.len_utf8()),
                parts: Vec::new(),
                excerpt: "3 em dashes in 20 words (at most 1 per 120)".to_string(),
            }]
        );
    }

    #[test]
    fn ten_dashes_in_two_thousand_words_pass() {
        let some = |n: usize, rest: usize| {
            format!(
                "{} {}",
                vec![format!("a {DASH}"); n].join(" "),
                filler(rest)
            )
        };
        // 10 dashes in 10 + 1990 = 2000 words
        assert!(dashes(&some(10, 1990)).is_empty());
        // 17 dashes in 2000 words is over 1 per 120; 16 is not
        assert_eq!(dashes(&some(17, 1983)).len(), 1);
        assert!(dashes(&some(16, 1984)).is_empty());
    }

    #[test]
    fn the_limit_is_a_strict_comparison() {
        // 3 dashes: fires while words < 360, passes at 360
        let at = |words: usize| format!("{DASH}{DASH}{DASH} {}", filler(words));
        assert_eq!(dashes(&at(359)).len(), 1);
        assert!(dashes(&at(360)).is_empty());
    }

    #[test]
    fn two_dashes_never_fire() {
        assert!(dashes(&format!("a {DASH} b {DASH} c")).is_empty());
        assert!(dashes(&format!("{DASH}{DASH}")).is_empty());
    }

    #[test]
    fn a_spaced_en_dash_counts_and_a_bare_one_does_not() {
        let doc = format!("a {EN} b c {EN} d e {EN} f");
        assert_eq!(dashes(&doc).len(), 1);
        // ranges and compounds: no space on both sides
        let doc = format!("1{EN}2 3{EN}4 5{EN}6 a {EN}b c{EN} d");
        assert!(dashes(&doc).is_empty());
        // mixed: two em dashes and one spaced en dash
        let doc = format!("a {DASH} b {DASH} c {EN} d");
        assert_eq!(dashes(&doc).len(), 1);
    }

    #[test]
    fn the_hit_sits_on_the_first_counted_dash() {
        let doc = format!("one\ntwo {DASH} a {DASH} b {DASH} c");
        let hits = dashes(&doc);
        assert_eq!(hits.len(), 1);
        let at = doc.find(DASH).unwrap();
        assert_eq!(hits[0].span, sp(at, at + DASH.len_utf8()));
    }

    #[test]
    fn dashes_in_dash_skip_spans_do_not_count() {
        let doc = format!("a {DASH} b {DASH} c {DASH} d");
        let all: Vec<usize> = doc.match_indices(DASH).map(|(i, _)| i).collect();
        let skip = |idx: &[usize]| -> Vec<Span> {
            idx.iter().map(|&i| sp(i, i + DASH.len_utf8())).collect()
        };
        let with_skip = |dash_skip| View {
            dash_skip,
            ..View::plain(&doc)
        };
        assert_eq!(em_dash(&with_skip(Vec::new()), &whole(&doc)).len(), 1);
        // one dash skipped: two are left, which never fires
        assert!(em_dash(&with_skip(skip(&all[..1])), &whole(&doc)).is_empty());
        // a skip span wider than the dash (a table row) covers it too
        assert!(em_dash(&with_skip(vec![sp(0, doc.len())]), &whole(&doc)).is_empty());
        // a skip span that ends where the dash starts does not cover it
        let first = all[0];
        assert_eq!(
            em_dash(&with_skip(vec![sp(0, first)]), &whole(&doc)).len(),
            1
        );
        // the first counted dash moves to the first one not skipped
        let more = format!("{doc} {DASH} e");
        let view = View {
            dash_skip: vec![sp(first, first + DASH.len_utf8())],
            ..View::plain(&more)
        };
        let hits = em_dash(&view, &whole(&more));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].span.start, all[1]);
    }

    #[test]
    fn only_dashes_in_new_text_count() {
        let doc = format!("a {DASH} b {DASH} c {DASH} d");
        let first_two = doc.match_indices(DASH).nth(1).unwrap().0 + DASH.len_utf8();
        // new text holds two of the three dashes
        assert!(em_dash(&View::plain(&doc), &[sp(0, first_two)]).is_empty());
        // and all three
        assert_eq!(em_dash(&View::plain(&doc), &whole(&doc)).len(), 1);
        // no new text
        assert!(em_dash(&View::plain(&doc), &[]).is_empty());
    }

    #[test]
    fn only_words_in_new_text_count() {
        // a long human page, then a short new tail with three dashes
        let old = filler(2000);
        let tail = format!("\nx {DASH} y {DASH} z {DASH} w");
        let doc = format!("{old}{tail}");
        let new = [sp(old.len(), doc.len())];
        let hits = em_dash(&View::plain(&doc), &new);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "3 em dashes in 4 words (at most 1 per 120)"
        );
        // the same dashes spread over the whole page pass
        assert!(dashes(&doc).is_empty());
    }

    #[test]
    fn a_word_needs_a_letter_and_mask_bytes_split_words() {
        let mask = char::from(MASK);
        // "123", "-" and the dashes are not words; "a{mask}b" is two
        let doc = format!("123 - {DASH} a{mask}b {DASH} {DASH}");
        let hits = dashes(&doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "3 em dashes in 2 words (at most 1 per 120)"
        );
    }

    #[test]
    fn a_dash_beside_a_mask_byte_still_counts() {
        let mask = char::from(MASK);
        let doc = format!("a{mask}{DASH}{mask}b c{DASH}d e {DASH} f");
        assert_eq!(dashes(&doc).len(), 1);
    }

    #[test]
    fn dash_hits_are_empty_without_text() {
        assert!(dashes("").is_empty());
        assert!(em_dash(&View::plain(""), &[]).is_empty());
    }

    // Three lists of three in a hundred words.
    fn triad_page() -> String {
        format!(
            "Plans, specs and roadmaps go first. Pages, themes and widgets go next. \
             Tests, builds, or releases go last. {}",
            filler(80)
        )
    }

    #[test]
    fn three_triads_in_a_hundred_words_fire() {
        let doc = triad_page();
        let words = doc
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphabetic))
            .count();
        let hits = triads(&doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule, "text.triad-density");
        assert_eq!(
            hits[0].excerpt,
            format!("3 lists of three in {words} words (at most 1 per 150)")
        );
        assert_eq!(hits[0].span.start, 0);
        assert!(hits[0].parts.is_empty());
    }

    #[test]
    fn two_triads_never_fire() {
        let doc = "Plans, specs and roadmaps go first. Pages, themes and widgets go next.";
        assert!(triads(doc).is_empty());
    }

    #[test]
    fn triads_spread_over_enough_words_pass() {
        // 3 triads need under 450 words to fire
        let doc = format!(
            "Plans, specs and roadmaps. Pages, themes and widgets. Tests, builds and releases. {}",
            filler(450)
        );
        assert!(triads(&doc).is_empty());
        let doc = format!(
            "Plans, specs and roadmaps. Pages, themes and widgets. Tests, builds and releases. {}",
            filler(300)
        );
        assert_eq!(triads(&doc).len(), 1);
    }

    #[test]
    fn a_triad_needs_three_words_of_four_letters() {
        // each shape three times: if it counted as a list of three, the rule would fire
        for shape in [
            "a, b and c.",
            "cat, dog and fox.",
            "red, tan, and sky.",
            "Plans and specs, then pages.",
            "Plans, specs and fox.",
            "Plans, specs. Pages, themes.",
            "one two three, four five, six",
            "Plans, specs also roadmaps.",
        ] {
            let doc = [shape; 3].join(" ");
            assert!(triads(&doc).is_empty(), "{shape}");
        }
        // the control: a real list, three times
        assert_eq!(
            triads(&["Plans, specs and roadmaps."; 3].join(" ")).len(),
            1
        );
    }

    #[test]
    fn the_oxford_comma_is_optional_and_or_works() {
        let doc =
            "Plans, specs, and roadmaps. Pages, themes or widgets. Tests, builds, or releases.";
        let hits = triads(doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "3 lists of three in 12 words (at most 1 per 150)"
        );
    }

    #[test]
    fn triads_do_not_overlap() {
        // read with overlaps this chain holds three lists; counted apart it holds one
        let doc = "Plans, specs and roadmaps, pages and themes, tests and builds.";
        assert!(triads(doc).is_empty());
    }

    #[test]
    fn only_triads_in_new_text_count() {
        let doc = triad_page();
        let second = doc.find("Pages").unwrap();
        // the new text holds two of the three lists
        assert!(triad(&View::plain(&doc), &[sp(second, doc.len())]).is_empty());
        // the whole text holds three
        assert_eq!(triads(&doc).len(), 1);
        assert!(triad(&View::plain(&doc), &[]).is_empty());
    }

    #[test]
    fn triads_across_masked_bytes_are_not_lists() {
        let mask = char::from(MASK);
        // each break, three times: a mask byte never stands for a space or a comma
        for shape in [
            format!("Plans, specs{mask} and roadmaps."),
            format!("Plans,{mask}specs and roadmaps."),
            format!("Plans, specs and{mask}roadmaps."),
            format!("Plans{mask}, specs and roadmaps."),
            format!("Plans, spe{mask}cs and roadmaps."),
        ] {
            let doc = [shape.as_str(); 3].join(" ");
            assert!(triads(&doc).is_empty(), "{shape:?}");
        }
    }

    #[test]
    fn triad_hits_are_empty_without_text() {
        assert!(triads("").is_empty());
    }
}
