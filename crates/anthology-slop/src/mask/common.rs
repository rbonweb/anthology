//! Helpers every kind view uses: blanking, ignore regions, waiver comments, quotes and URLs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;
use regex::bytes::Regex as BytesRegex;

use super::{MASK, Waiver};
use crate::{Span, rule};

/// A whole-line ignore-region marker in any comment style.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]{0,3}(?:<!--|\{?/\*|//)\s*slop-ignore-(start|end)\b[^\n]*$")
        .expect("marker regex")
});

/// The reason after `slop-ignore-start`, without the comment close.
static MARKER_REASON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"slop-ignore-start\b\s*(.*?)\s*(?:-->|\*/\s*\}?)?\s*$").expect("reason regex")
});

/// A `slop-ok` waiver on one line of a comment.
static WAIVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"slop-ok:\s*([a-z]+\.[a-z0-9-]+)(?:\s+(.*?))?\s*(?:-->|\*/\s*\}?)?\s*$")
        .expect("waiver regex")
});

static URL: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r#"https?://[^\s)>\]"']+"#).expect("url regex"));

/// The shortest reason that makes a waiver valid, in characters.
const MIN_REASON: usize = 8;

/// The longest quoted span that is masked, in characters.
const MAX_QUOTE: usize = 300;

const OPEN_CURLY: &[u8] = "\u{201c}".as_bytes();
const CLOSE_CURLY: &[u8] = "\u{201d}".as_bytes();

fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// Where every line of one document starts, so a byte offset becomes a line number by binary
/// search instead of a scan from the start of the document.
pub(crate) struct LineIndex {
    /// Address and length of the document this index was built for.
    addr: usize,
    len: usize,
    /// `starts[i]` is the offset of 1-based line `i + 1`.
    starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(doc: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(
            doc.bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i + 1),
        );
        LineIndex {
            addr: doc.as_ptr() as usize,
            len: doc.len(),
            starts,
        }
    }

    /// The 1-based line of byte `at`; an offset past the end counts as the end.
    pub(crate) fn line_of(&self, at: usize) -> usize {
        let at = at.min(self.len);
        self.starts.partition_point(|&start| start <= at)
    }

    /// The bytes of 1-based line `line`, without its newline, at least one byte wide so an empty
    /// line can still be touched by a span that covers its newline. A line past the end sits
    /// just after the last byte.
    pub(crate) fn line_span(&self, line: usize) -> Span {
        let i = line.saturating_sub(1);
        let Some(&start) = self.starts.get(i) else {
            return Span {
                start: self.len,
                end: self.len + 1,
            };
        };
        let end = self.starts.get(i + 1).map_or(self.len, |&next| next - 1);
        Span {
            start,
            end: end.max(start + 1),
        }
    }

    fn is_for(&self, doc: &str) -> bool {
        self.addr == doc.as_ptr() as usize && self.len == doc.len()
    }
}

thread_local! {
    /// The index of the document being checked, set by `IndexScope` so the mask code, which is
    /// handed only `doc`, finds the lines of a large document without scanning it again.
    static CURRENT: RefCell<Option<Rc<LineIndex>>> = const { RefCell::new(None) };
}

/// The index a live `IndexScope` holds for this very document, if there is one.
pub(crate) fn scoped_index(doc: &str) -> Option<Rc<LineIndex>> {
    CURRENT.with_borrow(|current| current.as_ref().filter(|i| i.is_for(doc)).cloned())
}

/// The index of `doc`: the scoped one, else a new one.
pub(crate) fn index_for(doc: &str) -> Rc<LineIndex> {
    scoped_index(doc).unwrap_or_else(|| Rc::new(LineIndex::new(doc)))
}

/// While it lives, `index_for` hands out one shared index of the document. The document is
/// borrowed for as long as the scope matters, so the index cannot go stale.
pub(crate) struct IndexScope {
    previous: Option<Rc<LineIndex>>,
}

impl IndexScope {
    pub(crate) fn enter(doc: &str) -> IndexScope {
        let index = index_for(doc);
        let previous = CURRENT.with_borrow_mut(|current| current.replace(index));
        IndexScope { previous }
    }
}

impl Drop for IndexScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT.with_borrow_mut(|current| *current = previous);
    }
}

/// Every byte of `span` but `b'\n'` becomes MASK. The span is widened to whole characters, so
/// the buffer stays UTF-8.
pub(crate) fn blank(buf: &mut [u8], span: Span) {
    let mut end = span.end.min(buf.len());
    let mut start = span.start.min(end);
    while start > 0 && start < buf.len() && is_continuation(buf[start]) {
        start -= 1;
    }
    while end < buf.len() && is_continuation(buf[end]) {
        end += 1;
    }
    for b in &mut buf[start..end] {
        if *b != b'\n' {
            *b = MASK;
        }
    }
}

/// The ignore regions of `doc` and the waiver of each region start. A region runs from the
/// start of its start-marker line to the end of its end-marker line; an unclosed one runs to the
/// end of the doc, and an end with no open region is ignored.
pub(crate) fn regions(doc: &str) -> (Vec<Span>, Vec<Waiver>) {
    let mut spans = Vec::new();
    let mut waivers = Vec::new();
    let mut open: Option<usize> = None;
    let index = index_for(doc);
    for caps in MARKER.captures_iter(doc) {
        let (Some(whole), Some(which)) = (caps.get(0), caps.get(1)) else {
            continue;
        };
        match (which.as_str(), open) {
            ("start", None) => {
                open = Some(whole.start());
                let reason = MARKER_REASON
                    .captures(whole.as_str())
                    .and_then(|c| c.get(1))
                    .map_or("", |m| m.as_str());
                waivers.push(Waiver {
                    line: index.line_of(whole.start()),
                    rule: None,
                    reason: reason.to_string(),
                    valid: true,
                });
            }
            ("end", Some(start)) => {
                spans.push(Span {
                    start,
                    end: whole.end(),
                });
                open = None;
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        spans.push(Span {
            start,
            end: doc.len(),
        });
    }
    (spans, waivers)
}

/// The `slop-ok` waivers inside the comment at `comment`, one per line at most. A waiver for an
/// unknown rule id waives nothing and is not returned; one whose trimmed reason is shorter than
/// 8 characters is returned with `valid` false.
pub(crate) fn comment_waivers(doc: &str, comment: Span) -> Vec<Waiver> {
    let end = comment.end.min(doc.len());
    let Some(text) = doc.get(comment.start.min(end)..end) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut offset = comment.start;
    for line in text.split('\n') {
        if let Some(caps) = WAIVER.captures(line)
            && let (Some(whole), Some(id)) = (caps.get(0), caps.get(1))
            && let Some(known) = rule(id.as_str())
        {
            let reason = caps.get(2).map_or("", |m| m.as_str()).trim();
            out.push(Waiver {
                line: index_for(doc).line_of(offset + whole.start()),
                rule: Some(known.id),
                reason: reason.to_string(),
                valid: reason.chars().count() >= MIN_REASON,
            });
        }
        offset += line.len() + 1;
    }
    out
}

/// Masks the content of double-quoted spans, keeping the quote marks. A `"` or `“` opens a span
/// only when the byte before it is not an ASCII letter or digit; the span closes at the next `"`
/// or `”` on the same line, within 300 characters, with no backtick between.
pub(crate) fn mask_quotes(buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let open = if buf[i] == b'"' {
            1
        } else if buf[i..].starts_with(OPEN_CURLY) {
            OPEN_CURLY.len()
        } else {
            i += 1;
            continue;
        };
        if i > 0 && buf[i - 1].is_ascii_alphanumeric() {
            i += open;
            continue;
        }
        let content = i + open;
        match quote_close(buf, content) {
            Some((close, len)) => {
                blank(
                    buf,
                    Span {
                        start: content,
                        end: close,
                    },
                );
                i = close + len;
            }
            None => i = content,
        }
    }
}

/// Where the quote whose content starts at `from` closes, and the close mark's length.
fn quote_close(buf: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut chars = 0;
    let mut j = from;
    while j < buf.len() {
        let b = buf[j];
        if b == b'"' {
            return Some((j, 1));
        }
        if buf[j..].starts_with(CLOSE_CURLY) {
            return Some((j, CLOSE_CURLY.len()));
        }
        if b == b'\n' || b == b'`' {
            return None;
        }
        if !is_continuation(b) {
            chars += 1;
            if chars > MAX_QUOTE {
                return None;
            }
        }
        j += 1;
    }
    None
}

/// Masks URLs (`https?://` up to whitespace, a closing bracket or a quote).
pub(crate) fn mask_urls(buf: &mut [u8]) {
    let found: Vec<Span> = URL
        .find_iter(buf)
        .map(|m| Span {
            start: m.start(),
            end: m.end(),
        })
        .collect();
    for span in found {
        blank(buf, span);
    }
}

/// The masked buffer as a string. Masking replaces whole characters, so this cannot fail; if it
/// ever did, non-ASCII bytes become MASK and the length is kept.
pub(crate) fn into_string(buf: Vec<u8>) -> String {
    String::from_utf8(buf).unwrap_or_else(|e| {
        let ascii: Vec<u8> = e
            .into_bytes()
            .into_iter()
            .map(|b| if b.is_ascii() { b } else { MASK })
            .collect();
        String::from_utf8(ascii).unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    fn masked(doc: &str, f: fn(&mut [u8])) -> String {
        let mut buf = doc.as_bytes().to_vec();
        f(&mut buf);
        let out = into_string(buf);
        assert_eq!(out.len(), doc.len(), "{doc:?}");
        out
    }

    fn waiver(line: usize, rule: Option<&'static str>, reason: &str, valid: bool) -> Waiver {
        Waiver {
            line,
            rule,
            reason: reason.to_string(),
            valid,
        }
    }

    #[test]
    fn blank_keeps_newlines_and_length() {
        let mut buf = b"ab\ncd".to_vec();
        blank(&mut buf, sp(1, 4));
        assert_eq!(buf, b"a\x01\n\x01d");
        blank(&mut buf, sp(4, 99));
        assert_eq!(buf, b"a\x01\n\x01\x01");
    }

    #[test]
    fn blank_masks_whole_chars() {
        let mut buf = "aéb".as_bytes().to_vec();
        blank(&mut buf, sp(2, 3));
        assert_eq!(into_string(buf), "a\u{1}\u{1}b");
    }

    #[test]
    fn regions_from_whole_line_markers_in_every_comment_style() {
        let doc = "a\n\
            <!-- slop-ignore-start quoted specimen -->\nx\n<!-- slop-ignore-end -->\nb\n\
            /* slop-ignore-start */\ny\n/* slop-ignore-end */\n\
            {/* slop-ignore-start */}\nz\n   {/* slop-ignore-end */}\n\
            // slop-ignore-start\nw\n// slop-ignore-end\n\
            \x20   <!-- slop-ignore-start -->\nlast";
        let region = |start: &str, end: &str| {
            let s = doc.find(start).unwrap();
            let e = doc[s..].find(end).unwrap() + s + end.len();
            sp(s, e)
        };
        let (spans, waivers) = regions(doc);
        assert_eq!(
            spans,
            [
                region("<!-- slop-ignore-start", "<!-- slop-ignore-end -->"),
                region("/* slop-ignore-start", "/* slop-ignore-end */"),
                region("{/* slop-ignore-start", "   {/* slop-ignore-end */}"),
                region("// slop-ignore-start", "// slop-ignore-end"),
            ]
        );
        assert_eq!(
            waivers,
            [
                waiver(2, None, "quoted specimen", true),
                waiver(6, None, "", true),
                waiver(9, None, "", true),
                waiver(12, None, "", true),
            ]
        );
    }

    #[test]
    fn an_unclosed_region_runs_to_the_end() {
        let doc = "a\n  <!-- slop-ignore-start -->\nb\nc";
        let (spans, waivers) = regions(doc);
        assert_eq!(spans, [sp(2, doc.len())]);
        assert_eq!(waivers, [waiver(2, None, "", true)]);
        // an end with no start is not a region
        let (spans, waivers) = regions("<!-- slop-ignore-end -->\nx");
        assert!(spans.is_empty() && waivers.is_empty());
        // a marker that is not the whole line is not a marker
        let (spans, _) = regions("text <!-- slop-ignore-start -->\nx");
        assert!(spans.is_empty());
    }

    #[test]
    fn quotes_are_masked_up_to_300_chars_on_one_line() {
        assert_eq!(
            masked("say \"hello there\" now", mask_quotes),
            "say \"\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\u{1}\" now"
        );
        assert_eq!(
            masked("say “hé” now", mask_quotes),
            "say “\u{1}\u{1}\u{1}” now"
        );
        assert_eq!(masked("(\"a\")", mask_quotes), "(\"\u{1}\")");
        let at_cap = format!("\"{}\"", "x".repeat(300));
        assert_eq!(
            masked(&at_cap, mask_quotes),
            format!("\"{}\"", "\u{1}".repeat(300))
        );
        let over = format!("\"{}\"", "x".repeat(301));
        assert_eq!(masked(&over, mask_quotes), over);
        let wrapped = "\"a\nb\"";
        assert_eq!(masked(wrapped, mask_quotes), wrapped);
    }

    #[test]
    fn a_quote_after_a_digit_does_not_open() {
        assert_eq!(masked("a 15\" screen", mask_quotes), "a 15\" screen");
        assert_eq!(
            masked("a 15\" screen and \"b\"", mask_quotes),
            "a 15\" screen and \"\u{1}\""
        );
        assert_eq!(masked("x\"y\"", mask_quotes), "x\"y\"");
    }

    #[test]
    fn a_quote_span_never_crosses_a_backtick() {
        let doc = "\"a `b` c\"";
        assert_eq!(masked(doc, mask_quotes), doc);
    }

    #[test]
    fn urls_are_masked() {
        assert_eq!(
            masked("see https://example.com/seamless-sync now", mask_urls),
            format!("see {} now", "\u{1}".repeat(33))
        );
        assert_eq!(
            masked("(http://a.b/c)", mask_urls),
            format!("({})", "\u{1}".repeat(12))
        );
        assert_eq!(masked("[x](<https://a.b>)", mask_urls).len(), 18);
        assert_eq!(
            masked("no url: example.com", mask_urls),
            "no url: example.com"
        );
    }

    fn parse(doc: &str) -> Vec<Waiver> {
        comment_waivers(doc, sp(0, doc.len()))
    }

    #[test]
    fn comment_waivers_are_parsed() {
        assert_eq!(
            parse("<!-- slop-ok: text.ai-vocab the product is called Seamless Sync -->"),
            [waiver(
                1,
                Some("text.ai-vocab"),
                "the product is called Seamless Sync",
                true
            )]
        );
        for doc in [
            "{/* slop-ok: ui.purple-gradient brand color from the style guide */}",
            "/* slop-ok: ui.purple-gradient brand color from the style guide */",
            "// slop-ok: ui.purple-gradient brand color from the style guide",
        ] {
            assert_eq!(
                parse(doc),
                [waiver(
                    1,
                    Some("ui.purple-gradient"),
                    "brand color from the style guide",
                    true
                )],
                "{doc}"
            );
        }
        assert_eq!(
            parse("/* slop-ok: ui.glow-orb short */"),
            [waiver(1, Some("ui.glow-orb"), "short", false)]
        );
        assert_eq!(
            parse("<!-- slop-ok: ui.glow-orb -->"),
            [waiver(1, Some("ui.glow-orb"), "", false)]
        );
        assert!(parse("<!-- slop-ok: ui.no-such long enough reason -->").is_empty());
        assert!(parse("<!-- nothing here -->").is_empty());
        assert_eq!(
            parse("<!--\nslop-ok: text.ai-vocab named by the user\n-->"),
            [waiver(2, Some("text.ai-vocab"), "named by the user", true)]
        );
        let doc = "one\nx <!-- slop-ok: text.hype-cta the client wrote it -->";
        let start = doc.find("<!--").unwrap();
        assert_eq!(
            comment_waivers(doc, sp(start, doc.len())),
            [waiver(
                2,
                Some("text.hype-cta"),
                "the client wrote it",
                true
            )]
        );
    }
}
