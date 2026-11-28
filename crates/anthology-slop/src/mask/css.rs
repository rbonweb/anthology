//! The CSS and SCSS view: no prose, the whole file is style, comments and ignore regions are not
//! code. The comment and region helpers here are shared with the HTML and JSX views.

use super::common::{blank, comment_waivers, into_string, regions};
use super::{View, Waiver};
use crate::Span;

pub(crate) fn view(doc: &str) -> View {
    let whole = Span {
        start: 0,
        end: doc.len(),
    };
    let (code, waivers) = masked_code(doc, &comment_spans(doc, b";{},"));
    let mut text = doc.as_bytes().to_vec();
    blank(&mut text, whole);
    View {
        text: into_string(text),
        code: into_string(code),
        style: vec![whole],
        classes: Vec::new(),
        dash_skip: Vec::new(),
        waivers,
    }
}

/// The comments of `doc`: `/* */` blocks, and `//` to the end of the line where the `//` starts a
/// line or follows whitespace or one of `after`, so `'https://x'` and `url(//cdn/x)` are not
/// comments.
///
/// One left-to-right scan, so a long line of `://` or an unclosed `/*` costs one pass, not one
/// per slash (the hook has a 60 s limit and blocks when it runs out).
pub(super) fn comment_spans(doc: &str, after: &[u8]) -> Vec<Span> {
    let bytes = doc.as_bytes();
    let mut spans = Vec::new();
    // Once a `/*` finds no `*/`, no later one will.
    let mut unclosed = false;
    let mut i = 0;
    while i + 1 < bytes.len() {
        match (bytes[i], bytes[i + 1]) {
            (b'/', b'*') if !unclosed => match block_end(bytes, i + 2) {
                Some(end) => {
                    spans.push(Span { start: i, end });
                    i = end;
                }
                None => {
                    unclosed = true;
                    i += 1;
                }
            },
            (b'/', b'/')
                if i == 0
                    || bytes[i - 1].is_ascii_whitespace()
                    || after.contains(&bytes[i - 1]) =>
            {
                let end = bytes[i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |n| i + n);
                spans.push(Span { start: i, end });
                i = end;
            }
            _ => i += 1,
        }
    }
    spans
}

/// The offset after the `*/` that closes a block comment whose body starts at `from`.
fn block_end(bytes: &[u8], from: usize) -> Option<usize> {
    let body = bytes.get(from..)?;
    body.windows(2)
        .position(|w| w == b"*/")
        .map(|p| from + p + 2)
}

/// The byte range of a regex match.
pub(super) fn span_of(m: regex::Match<'_>) -> Span {
    Span {
        start: m.start(),
        end: m.end(),
    }
}

/// Copies `span` of `code` into `text`, which started as all MASK.
pub(super) fn keep(code: &[u8], text: &mut [u8], span: Span) {
    if span.start < span.end {
        text[span.start..span.end].copy_from_slice(&code[span.start..span.end]);
    }
}

/// `doc` as bytes with `comments` and the ignore regions set to MASK, and the waivers of both
/// (comment waivers and region starts) in line order.
pub(super) fn masked_code(doc: &str, comments: &[Span]) -> (Vec<u8>, Vec<Waiver>) {
    let (ignored, mut waivers) = regions(doc);
    let mut code = doc.as_bytes().to_vec();
    for &comment in comments {
        waivers.extend(comment_waivers(doc, comment));
        blank(&mut code, comment);
    }
    for region in ignored {
        blank(&mut code, region);
    }
    waivers.sort_by_key(|w| w.line);
    (code, waivers)
}

/// Helpers the view tests of `css`, `html` and `jsx` share.
#[cfg(test)]
pub(super) mod support {
    use super::super::{MASK, View};
    use crate::Span;

    /// The contract every view keeps: `text` and `code` have the doc's length, newlines stay in
    /// place, and every other byte is the doc's own byte or MASK.
    pub(crate) fn assert_aligned(doc: &str, view: &View) {
        for (name, s) in [("text", &view.text), ("code", &view.code)] {
            assert_eq!(s.len(), doc.len(), "{name} length of {doc:?}");
            for (i, (got, want)) in s.bytes().zip(doc.bytes()).enumerate() {
                assert_eq!(
                    got == b'\n',
                    want == b'\n',
                    "{name} newline at {i} of {doc:?}"
                );
                assert!(
                    got == want || got == MASK,
                    "{name} byte {i} of {doc:?} is {got:#x}, not the doc's {want:#x} or MASK"
                );
            }
        }
    }

    /// The runs of unmasked text, with whitespace collapsed and empty runs dropped.
    pub(crate) fn runs(text: &str) -> Vec<String> {
        text.split(char::from(MASK))
            .map(|run| run.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|run| !run.is_empty())
            .collect()
    }

    /// The slices of `doc` the spans cover.
    pub(crate) fn slices<'a>(doc: &'a str, spans: &[Span]) -> Vec<&'a str> {
        spans.iter().map(|s| &doc[s.start..s.end]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::support::{assert_aligned, runs};
    use super::*;
    use crate::Span;
    use crate::mask::{MASK, Waiver};

    fn waiver(line: usize, rule: Option<&'static str>, reason: &str, valid: bool) -> Waiver {
        Waiver {
            line,
            rule,
            reason: reason.to_string(),
            valid,
        }
    }

    #[test]
    fn comments_are_masked_and_waivers_kept() {
        let doc = ".a { color: red; } /* slop-ok: ui.purple-gradient brand colour from the style guide */\n\
            /* plain café\n   comment with linear-gradient(#6366f1, #a855f7) */\n\
            .b { margin: 0; } // slop-ok: ui.glow-orb the brand shows an orb\n\
            // full line note\n\
            .c { background: url(//cdn.example/x.png); }\n\
            .d { background: url(http://cdn.example/y.png); } /* short */\n";
        let v = view(doc);
        assert_aligned(doc, &v);
        for kept in [
            ".a { color: red; }",
            ".b { margin: 0; }",
            "url(//cdn.example/x.png)",
            "url(http://cdn.example/y.png)",
        ] {
            assert!(v.code.contains(kept), "{kept} is code: {:?}", v.code);
        }
        for gone in [
            "slop-ok",
            "plain",
            "café",
            "linear-gradient",
            "full line note",
            "short",
        ] {
            assert!(!v.code.contains(gone), "{gone} is a comment: {:?}", v.code);
        }
        assert_eq!(
            v.waivers,
            [
                waiver(
                    1,
                    Some("ui.purple-gradient"),
                    "brand colour from the style guide",
                    true
                ),
                waiver(4, Some("ui.glow-orb"), "the brand shows an orb", true),
            ]
        );
    }

    #[test]
    fn ignore_regions_are_masked_and_their_start_is_a_waiver() {
        let doc = ".a { color: red }\n/* slop-ignore-start quoted specimen */\n.b { color: #6366f1 }\n/* slop-ignore-end */\n.c { color: blue }\n";
        let v = view(doc);
        assert_aligned(doc, &v);
        assert!(v.code.contains(".a { color: red }") && v.code.contains(".c { color: blue }"));
        assert!(!v.code.contains("#6366f1"), "{:?}", v.code);
        assert_eq!(v.waivers, [waiver(2, None, "quoted specimen", true)]);
    }

    #[test]
    fn comment_scanning_stays_linear_on_long_lines() {
        let started = std::time::Instant::now();
        // Not comments: a `//` after a colon, and a `/*` with no close.
        let urls = "a://b ".repeat(40_000);
        assert!(comment_spans(&urls, b";{},").is_empty());
        let unclosed = format!("/* {urls}");
        assert!(comment_spans(&unclosed, b";{},").is_empty());
        let many = " /*x*/".repeat(40_000);
        assert_eq!(comment_spans(&many, b";{},").len(), 40_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn comment_spans_follow_the_rules_for_slashes() {
        let sp = |start, end| Span { start, end };
        // `/*/` is not closed by its own slash; `*/` closes the next block.
        assert_eq!(comment_spans("/*/ a */ b", b""), [sp(0, 8)]);
        assert!(comment_spans("/*/", b"").is_empty());
        // A line comment needs a line start, whitespace or an allowed byte before it.
        assert!(comment_spans("a//b", b";").is_empty());
        assert_eq!(comment_spans("a;//b\nc", b";"), [sp(2, 5)]);
        assert_eq!(comment_spans("//b\nc", b""), [sp(0, 3)]);
        // A rejected `//` leaves its second slash free to open a block comment.
        assert_eq!(comment_spans("a//* b */", b""), [sp(2, 9)]);
        // A `/*` inside a line comment is part of it.
        assert_eq!(comment_spans("// a /* b\nc */", b""), [sp(0, 9)]);
    }

    #[test]
    fn the_whole_file_is_style() {
        let doc = ".a { color: red }\n/* note */\n";
        let v = view(doc);
        assert_eq!(
            v.style,
            [Span {
                start: 0,
                end: doc.len()
            }]
        );
        assert!(v.classes.is_empty() && v.dash_skip.is_empty());
        let empty = view("");
        assert_eq!(empty.style, [Span { start: 0, end: 0 }]);
        assert!(empty.text.is_empty() && empty.code.is_empty());
    }

    #[test]
    fn text_is_all_masked() {
        let doc = ".a::after { content: \"We delve into caching.\"; }\n/* It's seamless */\n";
        let v = view(doc);
        assert_aligned(doc, &v);
        assert!(
            v.text.bytes().all(|b| b == MASK || b == b'\n'),
            "{:?}",
            v.text
        );
        assert!(runs(&v.text).is_empty());
        assert_eq!(v.text.matches('\n').count(), 2);
    }
}
