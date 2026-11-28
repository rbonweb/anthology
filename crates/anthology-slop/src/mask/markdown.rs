//! The Markdown view: everything that is not prose is set to MASK in `text`.
//!
//! One pass over the lines settles the block constructs (frontmatter, fenced code, ignore
//! regions, blockquotes) and, on the lines left, inline code and HTML comments. Regexes then
//! take the inline constructs that never span lines. `style` and `classes` stay empty, and
//! `code` equals `text`.

use std::sync::LazyLock;

use regex::Regex;

use super::{View, Waiver, common};
use crate::Span;

/// An HTML tag: an opening, closing or declaration tag on one line, and an autolink.
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[A-Za-z/!][^>\n]*>").expect("tag regex"));

/// A link target with the bracket that closes the link text: `](...)`.
static LINK_TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\([^)\n]*\)").expect("link target regex"));

/// A reference link definition. A label that starts with `^` is a footnote, which is prose.
static REF_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^ {0,3}\[[^\]^][^\]]*\]:\s*\S+").expect("reference definition regex")
});

/// An MDX import or export line.
static IMPORT_EXPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(?:import|export) [^\n]*").expect("import regex"));

/// A list item that starts with a bold lead term and a dash; group 1 is the dash.
static LEAD_TERM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*[-*+][ \t]+\*\*[^*\n]+\*\*[ \t]*([\u{2014}\u{2013}])")
        .expect("lead term regex")
});

pub(crate) fn view(doc: &str) -> View {
    let (regions, mut waivers) = common::regions(doc);
    let mut scan = Scan {
        doc,
        buf: doc.as_bytes().to_vec(),
        waivers: Vec::new(),
        dash_skip: Vec::new(),
        no_more_close: false,
    };
    for region in &regions {
        scan.blank(region.start, region.end);
    }
    scan.blocks(&regions);
    scan.inline_constructs();
    waivers.append(&mut scan.waivers);
    waivers.sort_by_key(|w| w.line);
    let text = common::into_string(scan.buf);
    View {
        code: text.clone(),
        text,
        style: Vec::new(),
        classes: Vec::new(),
        dash_skip: scan.dash_skip,
        waivers,
    }
}

struct Scan<'a> {
    doc: &'a str,
    buf: Vec<u8>,
    /// The waivers of the HTML comments; the region waivers are added by `view`.
    waivers: Vec<Waiver>,
    dash_skip: Vec<Span>,
    /// Set once no `-->` is left in the doc, so no later `<!--` can close either.
    no_more_close: bool,
}

impl Scan<'_> {
    fn blank(&mut self, start: usize, end: usize) {
        common::blank(&mut self.buf, Span { start, end });
    }

    /// Frontmatter, fences, regions, blockquotes, table rows, lead terms, inline code and
    /// comments, line by line.
    fn blocks(&mut self, regions: &[Span]) {
        let doc = self.doc;
        let mut pos = self.frontmatter();
        let mut fence: Option<(u8, usize, usize)> = None;
        let mut next_region = 0;
        while pos < doc.len() {
            let end = line_end(doc, pos);
            let line = &doc[pos..end];
            if let Some((mark, len, start)) = fence {
                if closes_fence(line, mark, len) {
                    self.blank(start, end);
                    fence = None;
                }
                pos = end + 1;
                continue;
            }
            // A region is masked already and is never read: a fence or comment marker in it opens
            // nothing.
            while regions.get(next_region).is_some_and(|r| r.end <= pos) {
                next_region += 1;
            }
            if let Some(region) = regions.get(next_region)
                && region.start <= pos
            {
                pos = region.end.max(end) + 1;
                continue;
            }
            if let Some((mark, len)) = opens_fence(line) {
                fence = Some((mark, len, pos));
                pos = end + 1;
                continue;
            }
            if is_blockquote(line) {
                self.blank(pos, end);
                pos = end + 1;
                continue;
            }
            if line.trim_start().starts_with('|') {
                self.dash_skip.push(Span { start: pos, end });
            } else if let Some(dash) = LEAD_TERM.captures(line).and_then(|c| c.get(1)) {
                self.dash_skip.push(Span {
                    start: pos + dash.start(),
                    end: pos + dash.end(),
                });
            }
            pos = self.inline(pos, end);
        }
        if let Some((_, _, start)) = fence {
            self.blank(start, doc.len());
        }
    }

    /// Masks the frontmatter and returns where the lines after it start. The doc must start with
    /// a `---` line and have a `---` or `...` line later; otherwise there is no frontmatter.
    fn frontmatter(&mut self) -> usize {
        let doc = self.doc;
        let first_end = line_end(doc, 0);
        if doc[..first_end].trim_end() != "---" {
            return 0;
        }
        let mut pos = first_end + 1;
        while pos < doc.len() {
            let end = line_end(doc, pos);
            let line = doc[pos..end].trim_end();
            if line == "---" || line == "..." {
                self.blank(0, end);
                return end + 1;
            }
            pos = end + 1;
        }
        0
    }

    /// Masks the inline code and HTML comments of one line, from `from` to `end`, and returns where
    /// the next line to read starts. A comment may close on a later line; the rest of that line is
    /// read too.
    fn inline(&mut self, from: usize, end: usize) -> usize {
        let doc = self.doc;
        let bytes = doc.as_bytes();
        let (mut cur, mut end) = (from, end);
        while cur < end {
            let Some(at) =
                (cur..end).find(|&k| bytes[k] == b'`' || bytes[k..end].starts_with(b"<!--"))
            else {
                break;
            };
            if bytes[at] == b'`' {
                let ticks = run_of_ticks(bytes, at, end);
                match closing_ticks(bytes, at + ticks, end, ticks) {
                    Some(close) => {
                        self.blank(at, close + ticks);
                        cur = close + ticks;
                    }
                    None => cur = at + ticks,
                }
            } else if let Some(found) = self.comment_close(at + 4) {
                let close = found + 3;
                self.blank(at, close);
                self.waivers.extend(common::comment_waivers(
                    doc,
                    Span {
                        start: at,
                        end: close,
                    },
                ));
                if close > end {
                    end = line_end(doc, close);
                }
                cur = close;
            } else {
                cur = at + 4;
            }
        }
        end + 1
    }

    /// Where the next `-->` at or after `from` starts.
    fn comment_close(&mut self, from: usize) -> Option<usize> {
        if self.no_more_close {
            return None;
        }
        let found = self.doc.get(from..)?.find("-->").map(|i| from + i);
        self.no_more_close = found.is_none();
        found
    }

    /// The constructs that sit inside one line, on what the line pass left.
    fn inline_constructs(&mut self) {
        self.mask_all(&TAG);
        self.mask_all(&LINK_TARGET);
        self.mask_all(&REF_DEF);
        self.mask_all(&IMPORT_EXPORT);
        common::mask_urls(&mut self.buf);
        common::mask_quotes(&mut self.buf);
    }

    fn mask_all(&mut self, re: &Regex) {
        let Ok(text) = std::str::from_utf8(&self.buf) else {
            return;
        };
        let found: Vec<Span> = re
            .find_iter(text)
            .map(|m| Span {
                start: m.start(),
                end: m.end(),
            })
            .collect();
        for span in found {
            common::blank(&mut self.buf, span);
        }
    }
}

/// The end of the line that holds `from`: the offset of its `\n`, or the end of the doc.
fn line_end(doc: &str, from: usize) -> usize {
    doc.get(from..)
        .and_then(|rest| rest.find('\n'))
        .map_or(doc.len(), |i| from + i)
}

/// How many backticks start at `from`, within `..end`.
fn run_of_ticks(bytes: &[u8], from: usize, end: usize) -> usize {
    bytes[from..end].iter().take_while(|&&b| b == b'`').count()
}

/// The start of the next run of exactly `ticks` backticks in `from..end`.
fn closing_ticks(bytes: &[u8], from: usize, end: usize, ticks: usize) -> Option<usize> {
    let mut at = from;
    while at < end {
        if bytes[at] == b'`' {
            let run = run_of_ticks(bytes, at, end);
            if run == ticks {
                return Some(at);
            }
            at += run;
        } else {
            at += 1;
        }
    }
    None
}

/// The indent of a line in spaces, when it is at most 3.
fn indent_of(line: &str) -> Option<usize> {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    (indent <= 3).then_some(indent)
}

/// The character and length of a fence that opens on `line`: at most 3 spaces of indent, then 3 or
/// more backticks or tildes. The info string of a backtick fence holds no backtick.
fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let indent = indent_of(line)?;
    let rest = &line.as_bytes()[indent..];
    let mark = *rest.first()?;
    if mark != b'`' && mark != b'~' {
        return None;
    }
    let len = rest.iter().take_while(|&&b| b == mark).count();
    if len < 3 || (mark == b'`' && rest[len..].contains(&b'`')) {
        return None;
    }
    Some((mark, len))
}

/// Whether `line` closes a fence of `mark` repeated `len` times: at most 3 spaces of indent, at
/// least `len` marks, and only blanks after them.
fn closes_fence(line: &str, mark: u8, len: usize) -> bool {
    let Some(indent) = indent_of(line) else {
        return false;
    };
    let rest = &line.as_bytes()[indent..];
    let run = rest.iter().take_while(|&&b| b == mark).count();
    run >= len && rest[run..].iter().all(|b| b.is_ascii_whitespace())
}

/// `>` after at most 3 spaces, unless a `=` or digit follows (a comparison).
fn is_blockquote(line: &str) -> bool {
    let Some(indent) = indent_of(line) else {
        return false;
    };
    let bytes = line.as_bytes();
    bytes.get(indent) == Some(&b'>') && !matches!(bytes.get(indent + 1), Some(b'=' | b'0'..=b'9'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;
    use crate::mask::{MASK, Waiver};

    /// `doc` with each needle's bytes set to MASK (newlines kept). The needles are found in
    /// order, each after the end of the one before.
    fn expect(doc: &str, masked: &[&str]) -> String {
        let mut out = doc.as_bytes().to_vec();
        let mut from = 0;
        for needle in masked {
            let at = doc[from..]
                .find(needle)
                .unwrap_or_else(|| panic!("{needle:?} is not in the doc after byte {from}"))
                + from;
            for b in &mut out[at..at + needle.len()] {
                if *b != b'\n' {
                    *b = MASK;
                }
            }
            from = at + needle.len();
        }
        String::from_utf8(out).expect("masking whole needles keeps UTF-8")
    }

    /// MASK bytes shown as a visible character, so a failed comparison can be read.
    fn show(text: &str) -> String {
        text.replace('\u{1}', "\u{2592}")
    }

    fn newlines(s: &str) -> Vec<usize> {
        s.bytes()
            .enumerate()
            .filter(|&(_, b)| b == b'\n')
            .map(|(i, _)| i)
            .collect()
    }

    /// Builds the view and asserts that exactly the needles are masked, that every string keeps
    /// the doc's length and newlines, and that `style` and `classes` are empty.
    fn check(doc: &str, masked: &[&str]) -> View {
        let v = view(doc);
        assert_eq!(show(&v.text), show(&expect(doc, masked)), "doc {doc:?}");
        assert_eq!(v.code, v.text, "code is text for Markdown");
        assert!(v.style.is_empty() && v.classes.is_empty(), "doc {doc:?}");
        assert_eq!(v.text.len(), doc.len(), "doc {doc:?}");
        assert_eq!(newlines(&v.text), newlines(doc), "doc {doc:?}");
        v
    }

    fn at(doc: &str, needle: &str) -> Span {
        let start = doc.find(needle).expect("needle in doc");
        Span {
            start,
            end: start + needle.len(),
        }
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
    fn frontmatter_is_masked() {
        check(
            "---\ntitle: Seamless delve\ntags: [a]\n---\nBody text\n",
            &["---\ntitle: Seamless delve\ntags: [a]\n---"],
        );
        check("---\nx: 1\n...\nBody", &["---\nx: 1\n..."]);
        check("---\n---\nBody", &["---\n---"]);
        // the delimiters may carry trailing blanks
        check("---  \nx: 1\n---\t\nBody", &["---  \nx: 1\n---\t"]);
        // a fence marker inside frontmatter opens nothing
        check("---\n```\n---\nBody delve\n", &["---\n```\n---"]);
    }

    #[test]
    fn frontmatter_needs_a_close_and_the_first_line() {
        // no closing line: a rule at the top, not frontmatter
        check("---\nx: 1\nBody", &[]);
        // not at the start of the doc
        check("intro\n---\nx: 1\n---\nBody", &[]);
        // four dashes are not a delimiter
        check("----\nx\n----\nBody", &[]);
        check("", &[]);
        check("---", &[]);
    }

    #[test]
    fn fenced_code_is_masked_until_its_own_close() {
        // a ``` fence inside a ~~~ fence stays code
        check(
            "intro\n~~~text\n```\ninner seamless\n```\nstill code\n~~~\nprose after\n",
            &["~~~text\n```\ninner seamless\n```\nstill code\n~~~"],
        );
        // a shorter run does not close a longer fence
        check(
            "````md\n```\nx\n```\nmore\n````\nprose",
            &["````md\n```\nx\n```\nmore\n````"],
        );
        // a longer run closes
        check("```\ncode\n`````\nprose", &["```\ncode\n`````"]);
        // up to three spaces of indent on either line
        check("  ```rs\ncode\n   ```\nprose", &["  ```rs\ncode\n   ```"]);
        // a closing line carries no info string
        check(
            "```\ncode\n```js\nmore\n```\nprose",
            &["```\ncode\n```js\nmore\n```"],
        );
        // a tilde fence is not closed by backticks, nor the reverse
        check(
            "~~~\ncode\n```\nmore\n~~~\nprose",
            &["~~~\ncode\n```\nmore\n~~~"],
        );
        check(
            "```\ncode\n~~~\nmore\n```\nprose",
            &["```\ncode\n~~~\nmore\n```"],
        );
        // two fences
        check(
            "a\n```\nx\n```\nb delve\n~~~\ny\n~~~\nc",
            &["```\nx\n```", "~~~\ny\n~~~"],
        );
    }

    #[test]
    fn something_that_is_not_a_fence_opens_nothing() {
        // four spaces of indent
        check("    ```\nprose delve\n", &[]);
        // two backticks
        check("``\nprose delve\n", &[]);
        // a backtick fence whose info string holds a backtick is inline code
        check("```a`b\nprose delve\n", &[]);
        // a tilde fence may hold a tilde in its info string
        check("~~~a~b\ncode\n~~~\nprose", &["~~~a~b\ncode\n~~~"]);
    }

    #[test]
    fn an_unclosed_fence_masks_to_the_end() {
        check("intro\n```\ncode\nmore", &["```\ncode\nmore"]);
        check("intro\n~~~\ncode\n", &["~~~\ncode"]);
    }

    #[test]
    fn inline_code_is_masked() {
        check(
            "use `delve` now, ``a ` b`` there, ```x``` ok",
            &["`delve`", "``a ` b``", "```x```"],
        );
        // a run closes only at a run of the same length
        check("`a`` b` c", &["`a`` b`"]);
        // an unclosed run is plain text
        check("`unclosed seamless\nnext line `", &[]);
        // a span closes on its own line only
        check("`a\nb`", &[]);
    }

    #[test]
    fn html_comments_are_masked_and_their_waivers_kept() {
        let doc = "intro <!-- slop-ok: text.ai-vocab named by the user --> tail\n\
                   <!-- note\nmore ``` words\n-->\nafter\n\
                   x <!-- slop-ok: ui.glow-orb short --> y\n\
                   <!-- slop-ok: ui.no-such-rule long enough reason -->\n";
        let v = check(
            doc,
            &[
                "<!-- slop-ok: text.ai-vocab named by the user -->",
                "<!-- note\nmore ``` words\n-->",
                "<!-- slop-ok: ui.glow-orb short -->",
                "<!-- slop-ok: ui.no-such-rule long enough reason -->",
            ],
        );
        assert_eq!(
            v.waivers,
            [
                waiver(1, Some("text.ai-vocab"), "named by the user", true),
                waiver(6, Some("ui.glow-orb"), "short", false),
            ]
        );
    }

    #[test]
    fn comment_edges() {
        // an unclosed comment is plain text, and so is a second one after it
        check("a <!-- b\nc\nd <!-- e\n", &[]);
        // a comment hides a fence marker; the fence does not swallow the rest
        check("<!--\n```\n-->\nprose delve\n", &["<!--\n```\n-->"]);
        // a fence hides a comment: no waiver
        let v = check(
            "```\n<!-- slop-ok: text.ai-vocab named by the user -->\n```\nprose",
            &["```\n<!-- slop-ok: text.ai-vocab named by the user -->\n```"],
        );
        assert!(v.waivers.is_empty());
        // the leftmost opener wins: a comment marker inside inline code is code
        check("see `<!--` then seamless -->", &["`<!--`"]);
        let v = check(
            "`<!-- a -->` and <!-- slop-ok: text.hype-cta the client wrote it -->",
            &[
                "`<!-- a -->`",
                "<!-- slop-ok: text.hype-cta the client wrote it -->",
            ],
        );
        assert_eq!(
            v.waivers,
            [waiver(
                1,
                Some("text.hype-cta"),
                "the client wrote it",
                true
            )]
        );
        // text after a multi-line comment on its closing line is still read
        check("<!-- a\nb --> `code` rest\n", &["<!-- a\nb -->", "`code`"]);
    }

    #[test]
    fn blockquote_lines_are_masked_but_comparisons_are_not() {
        check(
            "> Let me know if you have questions\n   > indented quote\n>\nplain",
            &[
                "> Let me know if you have questions",
                "   > indented quote",
                ">",
            ],
        );
        // `>=` and `>5` are comparisons, four spaces of indent is code, and a `>` mid-line is text
        check(
            ">= 5 stays\n>5 stays\n    > four spaces stays\nx > y stays\n",
            &[],
        );
    }

    #[test]
    fn urls_and_link_targets_are_masked_link_text_is_not() {
        let doc = "See [seamless link](https://a.b/c) and https://x.y/z now, ![alt](pic.png).\n\
                   [ref]: https://a.b/ref \"Ref title\"\n\
                   [^1]: Footnote prose stays.\n\
                   A [bare](no-scheme-target) link.\n";
        check(
            doc,
            &[
                "](https://a.b/c)",
                "https://x.y/z",
                "](pic.png)",
                "[ref]: https://a.b/ref",
                "Ref title",
                "](no-scheme-target)",
            ],
        );
    }

    #[test]
    fn reference_definitions_may_be_indented_up_to_three_spaces() {
        check(
            "   [a b]: <https://x.y>\n    [c]: not a definition\ntext [d]: not at the start\n",
            &["   [a b]: <https://x.y>"],
        );
    }

    #[test]
    fn quoted_spans_are_masked() {
        check(
            "He said \"seamless delve\" twice, and \u{201c}curly words\u{201d} too; a 15\" screen and \"bee\".",
            &["seamless delve", "curly words", "bee"],
        );
        // a quote never crosses a line
        check("\"a\nb\"", &[]);
        // a quote inside a tag or inline code is not read as a quote
        check(
            "<a href=\"x\">go</a> `\"q\"` it's",
            &["<a href=\"x\">", "</a>", "`\"q\"`"],
        );
    }

    #[test]
    fn ignore_regions_are_masked() {
        let doc = "before\n<!-- slop-ignore-start quoting the brief -->\n```\nUnlock the full potential\n\
                   <!-- slop-ignore-end -->\nafter delve\n";
        let v = check(
            doc,
            &[
                "<!-- slop-ignore-start quoting the brief -->\n```\nUnlock the full potential\n<!-- slop-ignore-end -->",
            ],
        );
        assert_eq!(v.waivers, [waiver(2, None, "quoting the brief", true)]);

        // an unclosed region runs to the end; a fence inside it opens nothing
        let v = check(
            "a\n  <!-- slop-ignore-start -->\nb\n```\nc",
            &["  <!-- slop-ignore-start -->\nb\n```\nc"],
        );
        assert_eq!(v.waivers, [waiver(2, None, "", true)]);

        // a region ends where its own end marker is, whatever the fences around it do
        check(
            "x\n/* slop-ignore-start */\n~~~\n/* slop-ignore-end */\nprose delve\n",
            &["/* slop-ignore-start */\n~~~\n/* slop-ignore-end */"],
        );
    }

    #[test]
    fn waivers_are_in_line_order() {
        let v = check(
            "<!-- slop-ok: text.hype-cta the client wrote it -->\n\
             <!-- slop-ignore-start -->\nx\n<!-- slop-ignore-end -->\n\
             y <!-- slop-ok: text.ai-vocab named by the user -->\n",
            &[
                "<!-- slop-ok: text.hype-cta the client wrote it -->",
                "<!-- slop-ignore-start -->\nx\n<!-- slop-ignore-end -->",
                "<!-- slop-ok: text.ai-vocab named by the user -->",
            ],
        );
        let lines: Vec<usize> = v.waivers.iter().map(|w| w.line).collect();
        assert_eq!(lines, [1, 2, 5]);
        assert_eq!(v.waivers[1].rule, None);
    }

    #[test]
    fn table_rows_and_lead_term_dashes_are_dash_skip() {
        let doc = "| a | \u{2014} |\n  |---|---|\ntext \u{2014} here\n\
                   - **Term** \u{2014} desc\n* **Two** \u{2013} desc\n  + **Three**\u{2014}d\n\
                   - Plain \u{2014} no bold\n**Bold** \u{2014} no bullet\n\
                   - **A** and **B** \u{2014} two terms\n";
        let v = view(doc);
        let lead = |from: &str, dash: &str| {
            let start = doc.find(from).unwrap() + from.find(dash).unwrap();
            Span {
                start,
                end: start + dash.len(),
            }
        };
        assert_eq!(
            v.dash_skip,
            [
                at(doc, "| a | \u{2014} |"),
                at(doc, "  |---|---|"),
                lead("- **Term** \u{2014} desc", "\u{2014}"),
                lead("* **Two** \u{2013} desc", "\u{2013}"),
                lead("+ **Three**\u{2014}d", "\u{2014}"),
            ]
        );
        // the rows and the prose stay readable text
        assert_eq!(v.text, doc);
    }

    #[test]
    fn dash_skip_ignores_rows_in_code_and_comments() {
        let v = check(
            "```\n| a | \u{2014} |\n```\n<!--\n| b | \u{2014} |\n-->\n> | c | \u{2014} |\n",
            &[
                "```\n| a | \u{2014} |\n```",
                "<!--\n| b | \u{2014} |\n-->",
                "> | c | \u{2014} |",
            ],
        );
        assert!(v.dash_skip.is_empty());
    }

    #[test]
    fn mdx_import_and_export_lines_are_masked() {
        check(
            "import Foo from './foo'\nexport const meta = { title: 'x' }\nImport is capitalised prose\nimporting is prose\nexported prose\n",
            &[
                "import Foo from './foo'",
                "export const meta = { title: 'x' }",
            ],
        );
        // only at the start of a line
        check("we import x from y\n  import z\n", &[]);
    }

    #[test]
    fn html_tags_are_masked() {
        check(
            "<div class=\"a\">Hello seamless</div> <br/> <!DOCTYPE html> <https://a.b/c> a <b and c> d",
            &[
                "<div class=\"a\">",
                "</div>",
                "<br/>",
                "<!DOCTYPE html>",
                "<https://a.b/c>",
                "<b and c>",
            ],
        );
        // comparisons and arrows are not tags
        check("if x < y and a -> b > c, 3<4 and 5 > 2", &[]);
    }

    #[test]
    fn offsets_and_newlines_are_preserved() {
        let doc = "---\ntitle: caf\u{e9} \u{2014} bar\n---\r\n\
                   Intro \u{201c}curly \u{2014} quote\u{201d} and `c\u{f6}de` \u{1f600} <b>tag</b> \u{2014} done.\r\n\
                   > quote \u{2014} line\n\
                   | \u{e9} | \u{2014} |\n\
                   [l\u{e9}nk](https://\u{e9}.example/\u{2014}) and https://x.y/\u{e9} end\n\
                   [r\u{e9}f]: https://x.y/\u{e9} \"t\u{eb}xt\"\n\
                   import X from 'x\u{e9}'\n\
                   ```rs\n\u{e9}\u{2014}\n```\n\
                   <!-- c\u{f6}mment\n\u{2014} -->\n\
                   <!-- slop-ignore-start -->\n\u{2014}\n<!-- slop-ignore-end -->\n\
                   ~~~\nunclosed \u{2014}";
        let v = view(doc);
        assert_eq!(v.text.len(), doc.len());
        assert_eq!(v.code.len(), doc.len());
        assert_eq!(newlines(&v.text), newlines(doc));
        // every byte is either unchanged or MASK
        assert!(
            v.text
                .bytes()
                .zip(doc.bytes())
                .all(|(t, d)| t == d || t == MASK),
            "{}",
            show(&v.text)
        );
        // something real was masked, and some prose is left
        assert!(v.text.contains('\u{1}'));
        assert!(v.text.contains("Intro "));
        assert!(!v.text.contains("unclosed"));
        // CRLF: the carriage return of a masked line goes with it
        let crlf = "---\r\ntitle: x\r\n---\r\nBody\r\n```\r\ncode\r\n```\r\nafter\r\n";
        check(crlf, &["---\r\ntitle: x\r\n---\r", "```\r\ncode\r\n```\r"]);
    }
}
