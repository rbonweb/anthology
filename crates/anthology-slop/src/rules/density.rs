//! The density rules: em dashes and lists of three, counted over the new text only.
//!
//! Both read `view.text`. A dash, a list or a word counts when it touches a new span, so a page
//! that was already dense is not blamed for an edit that adds one line. A word is a
//! whitespace-separated token that holds a letter; a MASK byte also ends a token. A short edit
//! has no density: below 150 new words (em dashes) or 300 (lists of three) the rules stay quiet.

use std::sync::LazyLock;

use regex::Regex;

use crate::{Hit, MASK, Span, View};

const EM_DASH: &str = "text.em-dash-density";
const TRIAD: &str = "text.triad-density";

/// At least this many before the density is looked at.
const MIN_COUNT: usize = 3;
/// At most one em dash per this many words.
const WORDS_PER_DASH: usize = 80;
/// At most one list of three per this many words.
const WORDS_PER_TRIAD: usize = 150;
/// New words needed before em dashes are looked at: a short edit has no density.
const MIN_WORDS_DASH: usize = 150;
/// New words needed before lists of three are looked at.
const MIN_WORDS_TRIAD: usize = 300;

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
/// Fewer than `min_words` new words never fire, whatever the count.
fn hit(
    rule: &'static str,
    first: Option<Span>,
    count: usize,
    (per, min_words): (usize, usize),
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
    if words < min_words || count * per <= words {
        return Vec::new();
    }
    vec![Hit {
        rule,
        span,
        parts: Vec::new(),
        excerpt: format!("{count} {what} in {words} words (at most 1 per {per})"),
    }]
}

/// Day and month names, whole or cut short, and the marks of a clock time, that end a range
/// like "Mon – Fri" or "9 am – 5 pm". Lower case, separated by spaces.
const RANGE_WORDS: &str = "monday tuesday wednesday thursday friday saturday sunday \
    mon tue tues wed thu thur thurs fri sat sun \
    january february march april may june july august september october november december \
    jan feb mar apr jun jul aug sep sept oct nov dec \
    am pm a.m p.m";

/// True when `token` can end a range: it holds a digit ("17:00", "2021") or is one of
/// `RANGE_WORDS`. Punctuation around it ("(2019", "Friday:") does not matter.
fn ends_a_range(token: &str) -> bool {
    let token = token.trim_matches(|c: char| !c.is_alphanumeric());
    token.chars().any(char::is_numeric)
        || RANGE_WORDS
            .split_whitespace()
            .any(|word| token.eq_ignore_ascii_case(word))
}

/// True when the spaced en dash ending at `end` (and starting at `i`) joins the two ends of a
/// range ("9:00 – 17:00", "Monday – Friday", "Jan 5 – Feb 3"), which is not punctuation.
fn is_range(text: &str, i: usize, end: usize) -> bool {
    let is_break = |c: char| c.is_whitespace() || c == char::from(MASK);
    let before = text[..i - 1].rsplit(is_break).next().unwrap_or("");
    let after = text[end + 1..].split(is_break).next().unwrap_or("");
    ends_a_range(before) && ends_a_range(after)
}

/// Em dashes (U+2014) and spaced en dashes (a space, U+2013, a space) outside `dash_skip`.
/// A spaced en dash between two range ends (digits, times, day or month names) is a range.
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
            '\u{2013}' => {
                text[..i].ends_with(' ') && text[end..].starts_with(' ') && !is_range(text, i, end)
            }
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
        (WORDS_PER_DASH, MIN_WORDS_DASH),
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
        (WORDS_PER_TRIAD, MIN_WORDS_TRIAD),
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

    // 150 words with three dashes: the least a page needs to be judged.
    fn dashed_page() -> String {
        format!(
            "alpha {DASH} beta gamma {DASH} delta epsilon {DASH} zeta {}",
            filler(144)
        )
    }

    #[test]
    fn three_dashes_in_a_hundred_and_fifty_words_fire() {
        let doc = dashed_page();
        let hits = dashes(&doc);
        assert_eq!(
            hits,
            [Hit {
                rule: "text.em-dash-density",
                span: sp(6, 6 + DASH.len_utf8()),
                parts: Vec::new(),
                excerpt: "3 em dashes in 150 words (at most 1 per 80)".to_string(),
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
        // 26 dashes in 2000 words is over 1 per 80; 25 is not
        assert_eq!(dashes(&some(26, 1974)).len(), 1);
        assert!(dashes(&some(25, 1975)).is_empty());
    }

    #[test]
    fn the_limit_is_a_strict_comparison() {
        // 3 dashes: fires while words < 240, passes at 240
        let at = |words: usize| format!("{DASH}{DASH}{DASH} {}", filler(words));
        assert_eq!(dashes(&at(239)).len(), 1);
        assert!(dashes(&at(240)).is_empty());
    }

    #[test]
    fn two_dashes_never_fire() {
        assert!(dashes(&format!("a {DASH} b {DASH} c")).is_empty());
        assert!(dashes(&format!("{DASH}{DASH}")).is_empty());
    }

    #[test]
    fn a_spaced_en_dash_counts_and_a_bare_one_does_not() {
        let page = filler(150);
        let doc = format!("a {EN} b c {EN} d e {EN} f {page}");
        assert_eq!(dashes(&doc).len(), 1);
        // compounds: no space on both sides
        let doc = format!("1{EN}2 3{EN}4 5{EN}6 a {EN}b c{EN} d {page}");
        assert!(dashes(&doc).is_empty());
        let doc = format!("plans{EN}specs a{EN}b c{EN}d e {EN}f g{EN} h {page}");
        assert!(dashes(&doc).is_empty());
        // mixed: two em dashes and one spaced en dash
        let doc = format!("a {DASH} b {DASH} c {EN} d {page}");
        assert_eq!(dashes(&doc).len(), 1);
    }

    #[test]
    fn the_hit_sits_on_the_first_counted_dash() {
        let doc = format!("one\ntwo {DASH} a {DASH} b {DASH} c {}", filler(150));
        let hits = dashes(&doc);
        assert_eq!(hits.len(), 1);
        let at = doc.find(DASH).unwrap();
        assert_eq!(hits[0].span, sp(at, at + DASH.len_utf8()));
    }

    #[test]
    fn dashes_in_dash_skip_spans_do_not_count() {
        let doc = format!("a {DASH} b {DASH} c {DASH} d {}", filler(150));
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
        let doc = format!("a {DASH} b {DASH} c {DASH} d {}", filler(150));
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
        // a long human page, then a new tail of 150 words with three dashes
        let old = filler(2000);
        let tail = format!("\nx {DASH} y {DASH} z {DASH} w {}", filler(146));
        let doc = format!("{old}{tail}");
        let new = [sp(old.len(), doc.len())];
        let hits = em_dash(&View::plain(&doc), &new);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "3 em dashes in 150 words (at most 1 per 80)"
        );
        // the same dashes spread over the whole page pass
        assert!(dashes(&doc).is_empty());
    }

    #[test]
    fn a_word_needs_a_letter_and_mask_bytes_split_words() {
        let mask = char::from(MASK);
        // "123", "-" and the dashes are not words; "a{mask}b" is two
        let doc = format!("123 - {DASH} a{mask}b {DASH} {DASH} {}", filler(148));
        let hits = dashes(&doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "3 em dashes in 150 words (at most 1 per 80)"
        );
    }

    #[test]
    fn a_dash_beside_a_mask_byte_still_counts() {
        let mask = char::from(MASK);
        let doc = format!("a{mask}{DASH}{mask}b c{DASH}d e {DASH} f {}", filler(150));
        assert_eq!(dashes(&doc).len(), 1);
    }

    #[test]
    fn dash_hits_are_empty_without_text() {
        assert!(dashes("").is_empty());
        assert!(em_dash(&View::plain(""), &[]).is_empty());
    }

    /// `n` words in all: six `a {DASH}` pairs, then filler.
    fn six_dashes_in(n: usize) -> String {
        format!(
            "{} {}",
            vec![format!("a {DASH}"); 6].join(" "),
            filler(n - 6)
        )
    }

    #[test]
    fn a_short_edit_with_dashes_never_fires() {
        // the opening hours line, as a short edit: ranges, and too few words
        let hours = format!("- Monday {EN} Friday: 9:00 {EN} 17:00 / Saturday: 10:00 {EN} 14:00");
        assert!(dashes(&hours).is_empty());
        // three real dashes in a short edit
        assert!(dashes(&format!("alpha {DASH} beta {DASH} gamma {DASH} delta")).is_empty());
    }

    #[test]
    fn an_81_word_paragraph_with_four_dashes_never_fires() {
        let doc = format!(
            "{} {}",
            vec![format!("word {DASH} word"); 4].join(" "),
            filler(73)
        );
        assert_eq!(doc.split_whitespace().filter(|w| *w == "word").count(), 81);
        assert!(dashes(&doc).is_empty());
    }

    #[test]
    fn dashes_need_a_hundred_and_fifty_new_words() {
        assert!(dashes(&six_dashes_in(149)).is_empty());
        assert_eq!(dashes(&six_dashes_in(150)).len(), 1);
    }

    #[test]
    fn a_long_dash_heavy_paragraph_still_fires() {
        let doc = format!(
            "{} {}",
            vec![format!("word {DASH} word"); 8].join(" "),
            filler(184)
        );
        let hits = dashes(&doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "8 em dashes in 200 words (at most 1 per 80)"
        );
    }

    #[test]
    fn a_short_edit_in_a_long_page_never_fires() {
        // the page is long and dense, the new text is four words
        let old = format!("{} ", vec![format!("a {DASH}"); 40].join(" "));
        let tail = format!("x {DASH} y {DASH} z {DASH} w");
        let doc = format!("{old}{}{tail}", filler(2000));
        let new = [sp(doc.len() - tail.len(), doc.len())];
        assert!(em_dash(&View::plain(&doc), &new).is_empty());
    }

    #[test]
    fn spaced_en_dash_ranges_are_not_dashes() {
        // each range, three times, in a page long enough to fire on real dashes
        for range in [
            format!("9:00 {EN} 17:00"),
            format!("Monday {EN} Friday"),
            format!("Mon {EN} Fri:"),
            format!("Jan 5 {EN} Feb 3"),
            format!("9 am {EN} 5 pm"),
            format!("10 {EN} 20"),
            format!("May {EN} June"),
            format!("sat {EN} SUN"),
            format!("(2019 {EN} 2021)"),
            format!("Fri 17:00 {EN} Sat 10:00"),
        ] {
            let doc = format!("{} {}", [range.as_str(); 3].join(" and then "), filler(160));
            assert!(dashes(&doc).is_empty(), "{range}");
        }
        // the control: the same dash as punctuation still counts
        let doc = format!("plans {EN} specs {EN} roadmaps {EN} pages {}", filler(160));
        assert_eq!(dashes(&doc).len(), 1);
        // one side is not a range end
        let doc = format!(
            "The sun {EN} bright. It sat {EN} quietly. A May {EN} the best. {}",
            filler(160)
        );
        assert_eq!(dashes(&doc).len(), 1);
    }

    #[test]
    fn a_range_does_not_hide_the_dashes_around_it() {
        // two real dashes and a range make two counted dashes: no hit
        let doc = format!("a {DASH} b {DASH} c 9:00 {EN} 17:00 {}", filler(160));
        assert!(dashes(&doc).is_empty());
        // three real dashes and a range make three
        let doc = format!(
            "a {DASH} b {DASH} c {DASH} d 9:00 {EN} 17:00 {}",
            filler(160)
        );
        let hits = dashes(&doc);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].excerpt.starts_with("3 em dashes in "));
    }

    // Three lists of three in 300 words, the least a page needs to be judged.
    fn triad_page() -> String {
        format!(
            "Plans, specs and roadmaps go first. Pages, themes and widgets go next. \
             Tests, builds, or releases go last. {}",
            filler(282)
        )
    }

    #[test]
    fn three_triads_in_three_hundred_words_fire() {
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
        // each shape three times, in 300 words of filler: if it counted as a list of three,
        // the rule would fire
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
            let doc = format!("{} {}", [shape; 3].join(" "), filler(300));
            assert!(triads(&doc).is_empty(), "{shape}");
        }
        // the control: a real list, three times
        let doc = format!(
            "{} {}",
            ["Plans, specs and roadmaps."; 3].join(" "),
            filler(300)
        );
        assert_eq!(triads(&doc).len(), 1);
    }

    #[test]
    fn the_oxford_comma_is_optional_and_or_works() {
        let doc = format!(
            "Plans, specs, and roadmaps. Pages, themes or widgets. Tests, builds, or releases. {}",
            filler(288)
        );
        let hits = triads(&doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "3 lists of three in 300 words (at most 1 per 150)"
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
        // each break, three times, in 300 words of filler: a mask byte never stands for a space
        // or a comma
        for shape in [
            format!("Plans, specs{mask} and roadmaps."),
            format!("Plans,{mask}specs and roadmaps."),
            format!("Plans, specs and{mask}roadmaps."),
            format!("Plans{mask}, specs and roadmaps."),
            format!("Plans, spe{mask}cs and roadmaps."),
        ] {
            let doc = format!("{} {}", [shape.as_str(); 3].join(" "), filler(300));
            assert!(triads(&doc).is_empty(), "{shape:?}");
        }
    }

    #[test]
    fn triad_hits_are_empty_without_text() {
        assert!(triads("").is_empty());
    }

    #[test]
    fn a_short_edit_with_three_triads_never_fires() {
        let doc = "Linux, macOS and Windows are supported. Bash, Fish and Nushell work. \
                   JSON, YAML or TOML files load.";
        assert_eq!(LISTS_OF_THREE.find_iter(doc).count(), 3);
        assert!(triads(doc).is_empty());
    }

    #[test]
    fn triads_need_three_hundred_new_words() {
        // three triads (12 words) and filler
        let at = |words: usize| {
            format!(
                "{} {}",
                ["Plans, specs and roadmaps."; 3].join(" "),
                filler(words - 12)
            )
        };
        assert!(triads(&at(299)).is_empty());
        assert_eq!(triads(&at(300)).len(), 1);
    }

    #[test]
    fn a_long_triad_heavy_paragraph_still_fires() {
        let doc = format!(
            "{} {}",
            ["Plans, specs and roadmaps."; 8].join(" "),
            filler(300)
        );
        let hits = triads(&doc);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].excerpt,
            "8 lists of three in 332 words (at most 1 per 150)"
        );
    }

    #[test]
    fn a_short_triad_edit_in_a_long_page_never_fires() {
        let old = filler(2000);
        let tail =
            "\nPlans, specs and roadmaps. Pages, themes and widgets. Tests, builds or releases.";
        let doc = format!("{old}{tail}");
        let new = [sp(old.len(), doc.len())];
        assert!(triad(&View::plain(&doc), &new).is_empty());
    }
}
