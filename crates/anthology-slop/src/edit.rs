//! Edit replay: the document after a Write, Edit or MultiEdit, and the spans of its new text.

use crate::Span;
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

/// The document after `edit` on `before` (None: the file is missing) and its new spans, or None
/// when the edit would not apply.
///
/// A Write over an existing file and a MultiEdit mark the lines of the result that are not lines
/// of `before` (see `added_lines`). An Edit marks the lines of its `new` that are not lines of its
/// `old`.
pub fn apply(before: Option<&str>, edit: &ToolEdit) -> Option<(String, Vec<Span>)> {
    match edit {
        ToolEdit::Write { content } => {
            let spans = match before {
                None => vec![Span {
                    start: 0,
                    end: content.len(),
                }],
                Some(old) => added_lines(old, content),
            };
            Some((content.clone(), spans))
        }
        ToolEdit::Edit {
            old,
            new,
            replace_all,
        } => replace(before, old, new, *replace_all),
        ToolEdit::MultiEdit { edits } => {
            let mut text = before.map(str::to_string);
            for (old, new, replace_all) in edits {
                text = Some(replace(text.as_deref(), old, new, *replace_all)?.0);
            }
            let after = text.unwrap_or_default();
            let spans = added_lines(before.unwrap_or(""), &after);
            Some((after, spans))
        }
    }
}

/// One Edit. An empty `old` only creates a file that is missing or empty; otherwise `old` must
/// occur once, or any number of times with `replace_all`. The spans are the lines of each inserted
/// `new` that are not lines of `old` (see `added_lines`), so a line the Edit keeps is not new.
fn replace(
    before: Option<&str>,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<(String, Vec<Span>)> {
    if old.is_empty() {
        return match before {
            None | Some("") => Some((
                new.to_string(),
                vec![Span {
                    start: 0,
                    end: new.len(),
                }],
            )),
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
    // The same in every place `old` is replaced: the lines of `new` that are not lines of `old`.
    let fresh = added_lines(old, new);
    let mut out = String::with_capacity(text.len() + new.len() * at.len());
    let mut spans = Vec::new();
    let mut from = 0;
    for pos in at {
        out.push_str(&text[from..pos]);
        spans.extend(fresh.iter().map(|s| Span {
            start: out.len() + s.start,
            end: out.len() + s.end,
        }));
        out.push_str(new);
        from = pos + old.len();
    }
    out.push_str(&text[from..]);
    Some((out, spans))
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
    fn added_lines_skips_blank_lines_and_merges_neighbours() {
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
}
