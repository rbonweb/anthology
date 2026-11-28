//! Edit replay: the document after a Write, Edit or MultiEdit, and the spans of its new text.

use crate::{Kind, Span};
use std::collections::HashMap;

/// The tool input of a Write, Edit or MultiEdit. A MultiEdit edit is `(old, new, replace_all)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEdit {
    Write {
        content: String,
    },
    Edit {
        old: String,
        new: String,
        replace_all: bool,
    },
    MultiEdit {
        edits: Vec<(String, String, bool)>,
    },
}

impl ToolEdit {
    /// The tool name: "Write", "Edit" or "MultiEdit".
    pub fn tool(&self) -> &'static str {
        match self {
            ToolEdit::Write { .. } => "Write",
            ToolEdit::Edit { .. } => "Edit",
            ToolEdit::MultiEdit { .. } => "MultiEdit",
        }
    }
}

/// How much of a changed line counts as new text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grain {
    /// The words that changed. Prose keeps one paragraph on a line, so the rest of a line that was
    /// edited is somebody else's text and is not checked again.
    Words,
    /// The whole line. Code is checked a line at a time: a rule reads the line around the change.
    Lines,
}

/// The grain of a file kind: words for Markdown, lines for the code kinds.
pub fn grain(kind: Kind) -> Grain {
    match kind {
        Kind::Markdown => Grain::Words,
        Kind::Html | Kind::Jsx | Kind::Css => Grain::Lines,
    }
}

/// `apply_with` at `Grain::Lines`.
pub fn apply(before: Option<&str>, edit: &ToolEdit) -> Option<(String, Vec<Span>)> {
    apply_with(before, edit, Grain::Lines)
}

/// The document after `edit` on `before` (None: the file is missing) and its new spans, or None
/// when the edit would not apply.
///
/// A Write over an existing file and a MultiEdit mark what differs between `before` and the
/// result (see `changes`). An Edit marks what differs between its `old` and `new`, in each place
/// it lands.
pub fn apply_with(
    before: Option<&str>,
    edit: &ToolEdit,
    grain: Grain,
) -> Option<(String, Vec<Span>)> {
    match edit {
        ToolEdit::Write { content } => {
            let spans = match before {
                None => vec![Span {
                    start: 0,
                    end: content.len(),
                }],
                Some(old) => changes(old, content, grain),
            };
            Some((content.clone(), spans))
        }
        ToolEdit::Edit {
            old,
            new,
            replace_all,
        } => {
            let (out, at) = splice(before, old, new, *replace_all)?;
            // The same in every place `old` is replaced.
            let fresh = if old.is_empty() {
                vec![Span {
                    start: 0,
                    end: new.len(),
                }]
            } else {
                changes(old, new, grain)
            };
            let spans = at
                .iter()
                .flat_map(|&pos| {
                    fresh.iter().map(move |s| Span {
                        start: pos + s.start,
                        end: pos + s.end,
                    })
                })
                .collect();
            Some((out, spans))
        }
        ToolEdit::MultiEdit { edits } => {
            let mut text = before.map(str::to_string);
            for (old, new, replace_all) in edits {
                text = Some(splice(text.as_deref(), old, new, *replace_all)?.0);
            }
            let after = text.unwrap_or_default();
            let spans = changes(before.unwrap_or(""), &after, grain);
            Some((after, spans))
        }
    }
}

/// One Edit applied: the new text, and the offset in it of each inserted `new`. An empty `old`
/// only creates a file that is missing or empty; otherwise `old` must occur once, or any number
/// of times with `replace_all`.
fn splice(
    before: Option<&str>,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<(String, Vec<usize>)> {
    if old.is_empty() {
        return match before {
            None | Some("") => Some((new.to_string(), vec![0])),
            Some(_) => None,
        };
    }
    let text = before?;
    let mut found = text.match_indices(old).map(|(at, _)| at);
    let first = found.next()?;
    let at: Vec<usize> = if replace_all {
        std::iter::once(first).chain(found).collect()
    } else if found.next().is_none() {
        vec![first]
    } else {
        return None;
    };
    let mut out = String::with_capacity(text.len() + new.len() * at.len());
    let mut landed = Vec::with_capacity(at.len());
    let mut from = 0;
    for pos in at {
        out.push_str(&text[from..pos]);
        landed.push(out.len());
        out.push_str(new);
        from = pos + old.len();
    }
    out.push_str(&text[from..]);
    Some((out, landed))
}

/// The spans of `after` that are new against `before`, at `grain`.
fn changes(before: &str, after: &str, grain: Grain) -> Vec<Span> {
    match grain {
        Grain::Lines => added_lines(before, after),
        Grain::Words => changed_words(before, after),
    }
}

/// The lines of `doc` as `(start byte, text)`, without their line endings (`\n` or `\r\n`).
fn split_lines(doc: &str) -> Vec<(usize, &str)> {
    let mut start = 0;
    doc.split_inclusive('\n')
        .map(|raw| {
            let text = raw.strip_suffix('\n').unwrap_or(raw);
            let text = text.strip_suffix('\r').unwrap_or(text);
            let line = (start, text);
            start += raw.len();
            line
        })
        .collect()
}

/// The spans of the lines of `after` that are not lines of `before`.
///
/// Each line of `before` is used up by at most one equal line of `after`, in order, so a line kept
/// unchanged is never new even when it appears twice, and a second copy of it is. Blank lines are
/// skipped, and new lines that follow each other make one span.
pub fn added_lines(before: &str, after: &str) -> Vec<Span> {
    let mut unused: HashMap<&str, usize> = HashMap::new();
    for (_, text) in split_lines(before) {
        *unused.entry(text).or_default() += 1;
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut previous_new = None;
    for (index, (start, text)) in split_lines(after).into_iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        match unused.get_mut(text) {
            Some(left) if *left > 0 => {
                *left -= 1;
                continue;
            }
            _ => {}
        }
        let end = start + text.len();
        match spans.last_mut() {
            Some(last) if previous_new.is_some_and(|p| p + 1 == index) => last.end = end,
            _ => spans.push(Span { start, end }),
        }
        previous_new = Some(index);
    }
    spans
}

/// How many removed lines after the last paired one a new line may pair with.
const PAIR_WINDOW: usize = 8;

/// The most cells of the word table (old words times new words) filled for one pair of lines.
/// Past it every word between the common start and the common end counts as changed.
const TABLE_CELLS: usize = 90_000;

/// The spans of the words of `after` that are not in `before`.
///
/// Lines are matched as in `added_lines`; a line of `after` with no equal line in `before` is then
/// paired with the removed line that it most resembles, and only the words that differ from that
/// line are new. A line with no resemblance to any removed line is new as a whole. Whitespace is
/// not a change, and blank lines are skipped.
fn changed_words(before: &str, after: &str) -> Vec<Span> {
    let old_lines = split_lines(before);
    let mut unused: HashMap<&str, usize> = HashMap::new();
    for &(_, text) in &old_lines {
        *unused.entry(text).or_default() += 1;
    }
    // The lines of `after` with no equal line left in `before`, and how many of each text matched.
    let mut fresh: Vec<(usize, &str)> = Vec::new();
    let mut matched: HashMap<&str, usize> = HashMap::new();
    for (start, text) in split_lines(after) {
        if text.trim().is_empty() {
            continue;
        }
        match unused.get_mut(text) {
            Some(left) if *left > 0 => {
                *left -= 1;
                *matched.entry(text).or_default() += 1;
            }
            _ => fresh.push((start, text)),
        }
    }
    // The lines of `before` that nothing matched, in order: the first copies are the matched ones.
    let mut removed: Vec<&str> = Vec::new();
    for &(_, text) in &old_lines {
        match matched.get_mut(text) {
            Some(left) if *left > 0 => *left -= 1,
            _ if text.trim().is_empty() => {}
            _ => removed.push(text),
        }
    }
    let mut spans = Vec::new();
    let mut next = 0;
    for (start, text) in fresh {
        let window = &removed[next..removed.len().min(next + PAIR_WINDOW)];
        match closest(text, window) {
            Some(at) => {
                changed_in_line(removed[next + at], text, start, &mut spans);
                next += at + 1;
            }
            None => spans.push(Span {
                start,
                end: start + text.len(),
            }),
        }
    }
    spans
}

/// The index in `candidates` of the line that `line` resembles most: at least half of the words
/// of `line` are in it. A line far longer than `line` is not tried, so the cost of a pairing is
/// bounded by the length of `line`, however long the removed lines are.
fn closest(line: &str, candidates: &[&str]) -> Option<usize> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut count: HashMap<&str, usize> = HashMap::new();
    for word in &words {
        *count.entry(word).or_default() += 1;
    }
    let mut best: Option<(usize, usize)> = None;
    for (at, candidate) in candidates.iter().enumerate() {
        if candidate.len() > line.len().saturating_mul(4).saturating_add(256) {
            continue;
        }
        let mut left = count.clone();
        let shared = candidate
            .split_whitespace()
            .filter(|word| match left.get_mut(word) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            })
            .count();
        if shared > 0 && shared * 2 >= words.len() && best.is_none_or(|(_, most)| shared > most) {
            best = Some((at, shared));
        }
    }
    best.map(|(at, _)| at)
}

/// The words of `new` that `old` does not hold, in order, as spans of the document (`start` is
/// where `new` begins in it). A run of changed words next to each other is one span.
fn changed_in_line(old: &str, new: &str, start: usize, out: &mut Vec<Span>) {
    let old_words = words_of(old);
    let new_words = words_of(new);
    let same = |a: &(usize, usize), b: &(usize, usize)| old[a.0..a.1] == new[b.0..b.1];
    let head = old_words
        .iter()
        .zip(&new_words)
        .take_while(|(a, b)| same(a, b))
        .count();
    let tail = old_words[head..]
        .iter()
        .rev()
        .zip(new_words[head..].iter().rev())
        .take_while(|(a, b)| same(a, b))
        .count();
    let old_mid = &old_words[head..old_words.len() - tail];
    let new_mid = &new_words[head..new_words.len() - tail];
    // Which words of `new_mid` are also in `old_mid`, in the same order.
    let mut kept = vec![false; new_mid.len()];
    if !old_mid.is_empty() && old_mid.len().saturating_mul(new_mid.len()) <= TABLE_CELLS {
        let width = new_mid.len() + 1;
        // table[i * width + j]: the longest run shared by old_mid[i..] and new_mid[j..]
        let mut table = vec![0u32; (old_mid.len() + 1) * width];
        for i in (0..old_mid.len()).rev() {
            for j in (0..new_mid.len()).rev() {
                table[i * width + j] = if same(&old_mid[i], &new_mid[j]) {
                    table[(i + 1) * width + j + 1] + 1
                } else {
                    table[(i + 1) * width + j].max(table[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < old_mid.len() && j < new_mid.len() {
            if same(&old_mid[i], &new_mid[j]) {
                kept[j] = true;
                i += 1;
                j += 1;
            } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut run: Option<(usize, usize)> = None;
    for (word, kept) in new_mid.iter().zip(kept) {
        if kept {
            out.extend(run.take().map(|(from, to)| Span {
                start: start + from,
                end: start + to,
            }));
        } else {
            run = Some((run.map_or(word.0, |(from, _)| from), word.1));
        }
    }
    out.extend(run.map(|(from, to)| Span {
        start: start + from,
        end: start + to,
    }));
}

/// The `(start, end)` byte range of each whitespace-separated word of `text`.
fn words_of(text: &str) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut from = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), from) {
            (true, Some(start)) => {
                words.push((start, at));
                from = None;
            }
            (false, None) => from = Some(at),
            _ => {}
        }
    }
    if let Some(start) = from {
        words.push((start, text.len()));
    }
    words
}

/// The spans of 1-based inclusive line ranges of `doc`. A range that starts at line 0, ends before
/// it starts or starts past the last line is dropped; one that ends past the last line stops there.
pub fn line_spans(doc: &str, ranges: &[(usize, usize)]) -> Vec<Span> {
    let lines = split_lines(doc);
    ranges
        .iter()
        .filter_map(|&(first, last)| {
            if first == 0 || last < first || first > lines.len() {
                return None;
            }
            let (start, _) = lines[first - 1];
            let (last_start, last_text) = lines[last.min(lines.len()) - 1];
            Some(Span {
                start,
                end: last_start + last_text.len(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    fn write(content: &str) -> ToolEdit {
        ToolEdit::Write {
            content: content.to_string(),
        }
    }

    fn edit(old: &str, new: &str, replace_all: bool) -> ToolEdit {
        ToolEdit::Edit {
            old: old.to_string(),
            new: new.to_string(),
            replace_all,
        }
    }

    fn multi(edits: &[(&str, &str, bool)]) -> ToolEdit {
        ToolEdit::MultiEdit {
            edits: edits
                .iter()
                .map(|&(o, n, a)| (o.to_string(), n.to_string(), a))
                .collect(),
        }
    }

    fn done(after: &str, spans: &[Span]) -> Option<(String, Vec<Span>)> {
        Some((after.to_string(), spans.to_vec()))
    }

    #[test]
    fn tool_names() {
        assert_eq!(write("").tool(), "Write");
        assert_eq!(edit("a", "b", false).tool(), "Edit");
        assert_eq!(multi(&[]).tool(), "MultiEdit");
    }

    #[test]
    fn write_new_file_is_all_new() {
        assert_eq!(
            apply(None, &write("Hello\nworld\n")),
            done("Hello\nworld\n", &[sp(0, 12)])
        );
    }

    #[test]
    fn write_over_file_marks_only_changed_lines() {
        let before = "keep one\nold line\nkeep two\n";
        let after = "keep one\nnew line\nkeep two\nextra\n";
        // "new line" is bytes 9..17 and "extra" is 27..32; the two kept lines are untouched.
        assert_eq!(
            apply(Some(before), &write(after)),
            done(after, &[sp(9, 17), sp(27, 32)])
        );
        assert_eq!(apply(Some(before), &write(before)), done(before, &[]));
    }

    #[test]
    fn duplicate_unchanged_lines_are_not_new() {
        // one "same" was already there, so only the second copy is new
        assert_eq!(
            added_lines("same\nother\n", "same\nsame\nother\n"),
            [sp(5, 9)]
        );
        // both copies were already there
        assert!(added_lines("same\nsame\n", "same\nsame\n").is_empty());
        assert_eq!(added_lines("same\nsame\n", "same\nsame\nx\n"), [sp(10, 11)]);
        // a copy dropped: nothing new
        assert!(added_lines("same\nsame\nx\n", "same\nx\n").is_empty());
    }

    #[test]
    fn added_lines_skips_blank_lines_and_merges_neighbors() {
        let after = "a\nnew one\nnew two\n\n  \nnew three\nb\n";
        // "new one\nnew two" is 2..17; the blank lines break the run; "new three" is 22..31
        assert_eq!(added_lines("a\nb\n", after), [sp(2, 17), sp(22, 31)]);
    }

    #[test]
    fn added_lines_ignores_line_endings_and_a_missing_final_newline() {
        assert!(added_lines("a\r\nb\r\n", "a\nb").is_empty());
        assert_eq!(added_lines("a\n", "a\r\nnew\r\n"), [sp(3, 6)]);
        assert_eq!(added_lines("", "no newline"), [sp(0, 10)]);
        assert!(added_lines("", "").is_empty());
        assert!(added_lines("gone\n", "").is_empty());
    }

    #[test]
    fn an_indented_line_is_a_changed_line() {
        assert_eq!(added_lines("item\n", "  item\n"), [sp(0, 6)]);
    }

    #[test]
    fn edit_spans_cover_the_inserted_text() {
        assert_eq!(
            apply(Some("one two three"), &edit("two", "2 bits", false)),
            done("one 2 bits three", &[sp(4, 10)])
        );
    }

    #[test]
    fn an_edit_marks_only_the_lines_it_changed() {
        // review repro: line 1 is kept word for word, only line 2 changes
        let before = "Sync is seamless across devices.\nIt runs nightly.";
        let old = before;
        let new = "Sync is seamless across devices.\nIt runs hourly.";
        // "It runs hourly." is bytes 33..48
        assert_eq!(
            apply(Some(before), &edit(old, new, false)),
            done(new, &[sp(33, 48)])
        );
        // the kept line has a tell, so the whole new text would have been refused
        let (after, spans) = apply(Some(before), &edit(old, new, false)).unwrap();
        assert!(crate::check(crate::Kind::Markdown, &after, &spans).is_empty());
        assert!(!crate::check_all(crate::Kind::Markdown, &after).is_empty());
    }

    #[test]
    fn an_edit_still_marks_a_new_line_next_to_kept_ones() {
        let before = "intro\nkept\noutro\n";
        let after = "intro\nkept\nfresh start\nkept again\noutro\n";
        // "fresh start\nkept again" is bytes 11..33
        assert_eq!(
            apply(
                Some(before),
                &edit("kept\n", "kept\nfresh start\nkept again\n", false)
            ),
            done(after, &[sp(11, 33)])
        );
        // a copy of a kept line is new: only one copy was in `old`
        assert_eq!(
            apply(Some("a\nb\n"), &edit("a\n", "a\na\n", false)),
            done("a\na\nb\n", &[sp(2, 3)])
        );
    }

    #[test]
    fn a_replace_all_line_diffs_each_insertion() {
        // each occurrence keeps its unchanged line; "two" is new in both
        assert_eq!(
            apply(
                Some("keep\nx\nmid\nkeep\nx\n"),
                &edit("keep\nx", "keep\ntwo", true)
            ),
            done("keep\ntwo\nmid\nkeep\ntwo\n", &[sp(5, 8), sp(18, 21)])
        );
    }

    #[test]
    fn an_edit_that_only_deletes_adds_no_span() {
        assert_eq!(
            apply(Some("one two three"), &edit(" two", "", false)),
            done("one three", &[])
        );
    }

    #[test]
    fn replace_all_spans_every_insertion() {
        assert_eq!(
            apply(Some("a x b x c"), &edit("x", "yy", true)),
            done("a yy b yy c", &[sp(2, 4), sp(7, 9)])
        );
        // one occurrence is also fine, with or without the flag
        assert_eq!(
            apply(Some("a x b"), &edit("x", "yy", true)),
            done("a yy b", &[sp(2, 4)])
        );
    }

    #[test]
    fn missing_or_ambiguous_old_is_none() {
        assert_eq!(apply(Some("one two"), &edit("three", "x", false)), None);
        assert_eq!(apply(Some("one two"), &edit("three", "x", true)), None);
        assert_eq!(apply(Some("x and x"), &edit("x", "y", false)), None);
        assert_eq!(apply(None, &edit("x", "y", false)), None);
        assert_eq!(apply(Some(""), &edit("x", "y", false)), None);
        // an empty old on a file that has text is not an insertion point
        assert_eq!(apply(Some("text"), &edit("", "y", false)), None);
    }

    #[test]
    fn empty_old_on_a_missing_file_creates_it() {
        assert_eq!(
            apply(None, &edit("", "Hello", false)),
            done("Hello", &[sp(0, 5)])
        );
        assert_eq!(
            apply(Some(""), &edit("", "Hello", false)),
            done("Hello", &[sp(0, 5)])
        );
    }

    #[test]
    fn multiedit_applies_in_order_and_diffs_lines() {
        let before = "alpha\nbeta\ngamma\n";
        let got = apply(
            Some(before),
            &multi(&[("beta", "beta two", false), ("gamma", "delta", false)]),
        );
        // "beta two\ndelta" is bytes 6..20
        assert_eq!(got, done("alpha\nbeta two\ndelta\n", &[sp(6, 20)]));
        // the second edit sees the result of the first
        assert_eq!(
            apply(Some("a\n"), &multi(&[("a", "b", false), ("b", "c", false)])),
            done("c\n", &[sp(0, 1)])
        );
    }

    #[test]
    fn multiedit_marks_a_line_only_when_the_net_result_changed_it() {
        // the edits swap two lines back: nothing is new
        let got = apply(Some("x\n"), &multi(&[("x", "y", false), ("y", "x", false)]));
        assert_eq!(got, done("x\n", &[]));
    }

    #[test]
    fn multiedit_can_create_a_missing_file() {
        assert_eq!(
            apply(None, &multi(&[("", "Hi\n", false)])),
            done("Hi\n", &[sp(0, 2)])
        );
        // once the file has text, an empty old is no longer a creation
        assert_eq!(
            apply(None, &multi(&[("", "Hi\n", false), ("", "more", false)])),
            None
        );
    }

    #[test]
    fn multiedit_with_a_missing_old_is_none() {
        assert_eq!(
            apply(
                Some("one two"),
                &multi(&[("one", "1", false), ("three", "3", false)])
            ),
            None
        );
        assert_eq!(apply(Some("x x"), &multi(&[("x", "y", false)])), None);
        assert_eq!(apply(None, &multi(&[("a", "b", false)])), None);
    }

    #[test]
    fn line_spans_map_inclusive_ranges() {
        let doc = "a\nbb\nccc\n";
        assert_eq!(line_spans(doc, &[(1, 1), (2, 3)]), [sp(0, 1), sp(2, 8)]);
        assert_eq!(line_spans(doc, &[(3, 3)]), [sp(5, 8)]);
        // past the end is cut back; wholly past the end, or not a range at all, is dropped
        assert_eq!(line_spans(doc, &[(3, 99)]), [sp(5, 8)]);
        assert!(line_spans(doc, &[(4, 4), (0, 2), (3, 2)]).is_empty());
        assert!(line_spans(doc, &[]).is_empty());
        assert!(line_spans("", &[(1, 1)]).is_empty());
        // line endings are not part of a span, and a last line needs no newline
        assert_eq!(line_spans("a\r\nb\r\n", &[(1, 2)]), [sp(0, 4)]);
        assert_eq!(line_spans("a\nb", &[(2, 2)]), [sp(2, 3)]);
    }

    /// The text under each span of a Words replay, so a test reads as "these words are new".
    fn marked(before: Option<&str>, tool: &ToolEdit) -> Vec<String> {
        let (after, spans) = apply_with(before, tool, Grain::Words).expect("the edit applies");
        spans
            .iter()
            .map(|s| after[s.start..s.end].to_string())
            .collect()
    }

    /// The review repro: one paragraph per line, an old tell on the line, one typo fixed.
    const PARAGRAPH: &str =
        "Intro.\n\nOur sync is seamless across devices, and it runs nighly at 2 am.\n";
    const FIXED: &str = "Our sync is seamless across devices, and it runs nightly at 2 am.";
    const TYPO: &str = "Our sync is seamless across devices, and it runs nighly at 2 am.";

    #[test]
    fn a_one_word_edit_marks_only_that_word() {
        let got = marked(Some(PARAGRAPH), &edit(TYPO, FIXED, false));
        assert_eq!(got, ["nightly"]);
        let (after, spans) =
            apply_with(Some(PARAGRAPH), &edit(TYPO, FIXED, false), Grain::Words).unwrap();
        assert!(crate::check(crate::Kind::Markdown, &after, &spans).is_empty());
        // the whole line is still refused when it is new
        assert!(!crate::check_all(crate::Kind::Markdown, &after).is_empty());
    }

    #[test]
    fn a_smaller_old_string_gives_the_same_words() {
        let got = marked(
            Some(PARAGRAPH),
            &edit("runs nighly at", "runs nightly at", false),
        );
        assert_eq!(got, ["nightly"]);
    }

    #[test]
    fn a_write_that_fixes_the_typo_marks_only_that_word() {
        let after = PARAGRAPH.replace("nighly", "nightly");
        assert_eq!(marked(Some(PARAGRAPH), &write(&after)), ["nightly"]);
    }

    #[test]
    fn a_write_that_makes_the_line_a_list_item_marks_only_the_marker() {
        let before = "Intro.\n\nSync is seamless across devices.\n";
        let after = "Intro.\n\n- Sync is seamless across devices.\n";
        assert_eq!(marked(Some(before), &write(after)), ["-"]);
    }

    #[test]
    fn a_multiedit_marks_only_the_words_it_changed() {
        let before = "Our sync is seamless on desktop, and it runs nighly.\nSecond line stays.\n";
        let got = marked(
            Some(before),
            &multi(&[("nighly", "nightly", false), ("desktop", "laptop", false)]),
        );
        assert_eq!(got, ["laptop,", "nightly."]);
    }

    #[test]
    fn two_fixes_on_one_line_leave_the_old_tell_between_them_unmarked() {
        let before = "A tpyo, then a seamless tell, then another tpyo.\n";
        let after = "A typo, then a seamless tell, then another typo.\n";
        assert_eq!(marked(Some(before), &write(after)), ["typo,", "typo."]);
    }

    #[test]
    fn a_word_added_at_the_end_is_marked_with_the_word_it_changed() {
        let longer = "Our sync is seamless across devices, and it runs nighly at 2 am daily.";
        // "am." became "am" and "daily." is new: the two neighbors make one span
        assert_eq!(
            marked(Some(PARAGRAPH), &edit(TYPO, longer, false)),
            ["am daily."]
        );
    }

    #[test]
    fn a_tell_added_in_the_edit_is_still_new() {
        let before = "Our sync is quick across devices.\n";
        let tool = edit("is quick", "is seamless", false);
        let (after, spans) = apply_with(Some(before), &tool, Grain::Words).unwrap();
        let found = crate::check(crate::Kind::Markdown, &after, &spans);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, "text.ai-vocab");
    }

    #[test]
    fn a_phrase_tell_finished_by_the_edit_is_still_new() {
        // "delve" was there; the edit adds the word that makes it a tell with the old one
        let before = "We delve.\n";
        let after = "We delve into caching.\n";
        let (doc, spans) = apply_with(Some(before), &write(after), Grain::Words).unwrap();
        assert_eq!(crate::check(crate::Kind::Markdown, &doc, &spans).len(), 1);
    }

    #[test]
    fn an_unrelated_new_line_does_not_stop_a_later_line_pairing_with_its_old_one() {
        let before = "Intro.\n\nOur sync is seamless across devices, and it runs nighly at 2 am.\n";
        let after = "Intro.\n\nA brand new paragraph about something else.\n\nOur sync is seamless across devices, and it runs nightly at 2 am.\n";
        assert_eq!(
            marked(Some(before), &write(after)),
            ["A brand new paragraph about something else.", "nightly"]
        );
    }

    #[test]
    fn a_rewritten_line_is_new_as_a_whole() {
        let before = "The cache keeps the last ten answers.\n";
        let after = "Seamless handoff between every device you own.\n";
        assert_eq!(marked(Some(before), &write(after)), [after.trim_end()]);
    }

    #[test]
    fn a_whitespace_only_change_marks_nothing_in_words() {
        assert!(marked(Some("a  b\n"), &write("a b\n")).is_empty());
        assert!(marked(Some("- item\n"), &write("  - item\n")).is_empty());
    }

    #[test]
    fn lines_grain_still_marks_the_whole_line() {
        let (after, spans) =
            apply_with(Some(PARAGRAPH), &edit(TYPO, FIXED, false), Grain::Lines).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(&after[spans[0].start..spans[0].end], FIXED);
        // `apply` is the lines replay
        assert_eq!(
            apply(Some(PARAGRAPH), &edit(TYPO, FIXED, false)),
            Some((after, spans))
        );
    }

    #[test]
    fn replace_all_marks_the_changed_word_in_every_place() {
        let before = "one seamless line nighly\ntwo seamless line nighly\n";
        assert_eq!(
            marked(Some(before), &edit("line nighly", "line nightly", true)),
            ["nightly", "nightly"]
        );
    }

    #[test]
    fn grain_follows_the_file_kind() {
        use crate::Kind;
        assert_eq!(grain(Kind::Markdown), Grain::Words);
        for kind in [Kind::Html, Kind::Jsx, Kind::Css] {
            assert_eq!(grain(kind), Grain::Lines, "{kind:?}");
        }
    }

    #[test]
    fn a_big_rewrite_is_not_quadratic() {
        // 1 MiB: every line changes, then one line of 150k words with one word changed
        let before: String = (0..20_000)
            .map(|i| format!("line {i} of the old text here\n"))
            .collect();
        let after: String = (0..20_000)
            .map(|i| format!("line {i} of the new text here\n"))
            .collect();
        let start = std::time::Instant::now();
        let spans = apply_with(Some(&before), &write(&after), Grain::Words)
            .unwrap()
            .1;
        assert_eq!(spans.len(), 20_000);
        let one_line = "word ".repeat(150_000);
        let changed = format!(
            "{}other{}",
            &one_line[..5 * 70_000],
            &one_line[5 * 70_000..]
        );
        let spans = apply_with(Some(&one_line), &write(&changed), Grain::Words)
            .unwrap()
            .1;
        assert!(!spans.is_empty());
        // the same line with every other word changed: the table of the middle is capped
        let alternating: String = (0..150_000)
            .map(|i| if i % 2 == 0 { "word " } else { "from " })
            .collect();
        let other: String = (0..150_000)
            .map(|i| if i % 2 == 0 { "word " } else { "into " })
            .collect();
        let spans = apply_with(Some(&alternating), &write(&other), Grain::Words)
            .unwrap()
            .1;
        assert!(!spans.is_empty());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            start.elapsed()
        );
    }
}
