//! The Markdown view: everything that is not prose is set to MASK in `text`.
//!
//! One pass over the lines settles the block constructs (frontmatter, fenced and indented code,
//! ignore regions, blockquotes, MDX `import` and `export` statements) and, on the lines left,
//! inline code, HTML comments and `<pre>` blocks. Scans and regexes then take the tags, links,
//! URLs and quoted spans. Every scan moves forward and never rereads a span it has settled, so
//! the whole view is linear in the size of the doc. `style` and `classes` stay empty, and `code`
//! equals `text`.

use std::sync::LazyLock;

use regex::Regex;

use super::{View, Waiver, common};
use crate::Span;

/// The longest quoted span that is masked, in characters.
const MAX_QUOTE: usize = 300;

const OPEN_DOUBLE: &[u8] = "\u{201c}".as_bytes();
const CLOSE_DOUBLE: &[u8] = "\u{201d}".as_bytes();
const OPEN_SINGLE: &[u8] = "\u{2018}".as_bytes();
const CLOSE_SINGLE: &[u8] = "\u{2019}".as_bytes();

/// An HTML tag: an opening, closing or declaration tag on one line, and an autolink. A tag that
/// starts its line is read by `mask_line_start_tags` first, which also takes tags over several
/// lines.
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[A-Za-z/!][^>\n]*>").expect("tag regex"));

/// The first line of an MDX `import` or `export` statement that can go on over several lines.
static ESM_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?:export\s+(?:default\b|const\b|let\b|var\b|function\b|async\b|class\b|type\b|interface\b|enum\b|[{*])|import\s+(?:type\s+)?(?:[\w$]+\s*,\s*[{*]|[\w$]+\s+from\b|[{*'"]))"#,
    )
    .expect("esm start regex")
});

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
        no_more_pre_close: false,
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
    /// Set once no `</pre>` is left in the doc.
    no_more_pre_close: bool,
}

/// A fence that is open.
#[derive(Clone, Copy)]
struct Fence {
    mark: u8,
    len: usize,
    /// Where the opening line starts.
    start: usize,
    /// The widest indent, in columns, a closing line may have.
    close_indent: usize,
}

/// What the line pass knows about the lines it has read.
struct Flow {
    /// No paragraph is open, so an indented line starts code: the start of the doc, a blank line,
    /// a heading, a rule or the end of a block.
    no_paragraph: bool,
    /// The last line read was blank, or the end of a block.
    after_blank: bool,
    /// A list item is open. In one, a fence may be indented any amount, and a line that is
    /// indented is a continuation, not code.
    in_list: bool,
    /// The indented code block being read: from its first line to the end of its last.
    code: Option<Span>,
}

impl Scan<'_> {
    fn blank(&mut self, start: usize, end: usize) {
        common::blank(&mut self.buf, Span { start, end });
    }

    /// Frontmatter, fences, indented code, regions, MDX statements, blockquotes, table rows, lead
    /// terms, inline code, comments and `<pre>` blocks, line by line.
    fn blocks(&mut self, regions: &[Span]) {
        let doc = self.doc;
        let mut pos = self.frontmatter();
        let mut flow = Flow {
            no_paragraph: true,
            after_blank: true,
            in_list: false,
            code: None,
        };
        let mut fence: Option<Fence> = None;
        let mut esm = Esm::default();
        let mut next_region = 0;
        while pos < doc.len() {
            let end = line_end(doc, pos);
            let line = &doc[pos..end];
            if let Some(open) = fence {
                if closes_fence(line, &open) {
                    self.blank(open.start, end);
                    fence = None;
                    flow.no_paragraph = true;
                    flow.after_blank = true;
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
                self.end_code(&mut flow);
                flow.no_paragraph = true;
                flow.after_blank = true;
                pos = region.end.max(end) + 1;
                continue;
            }
            // The rest of a statement is masked, blank lines inside it included.
            if esm.active() {
                esm.feed(line);
                self.blank(pos, end);
                pos = end + 1;
                continue;
            }
            let (cols, lead) = leading(line);
            let text = &line[lead..];
            if text.bytes().all(|b| b.is_ascii_whitespace()) {
                flow.no_paragraph = true;
                flow.after_blank = true;
                pos = end + 1;
                continue;
            }
            let after_blank = std::mem::take(&mut flow.after_blank);
            if cols >= 4 && !flow.in_list && flow.no_paragraph {
                let code = flow.code.get_or_insert(Span { start: pos, end });
                code.end = end;
                pos = end + 1;
                continue;
            }
            self.end_code(&mut flow);
            let rule = is_rule(text);
            let heading = is_heading(text);
            if (cols < 4 || flow.in_list) && !rule && is_list_marker(text) {
                flow.in_list = true;
            } else if cols < 2
                && (after_blank
                    || rule
                    || heading
                    || text.starts_with("```")
                    || text.starts_with("~~~"))
            {
                flow.in_list = false;
            }
            if (cols <= 3 || flow.in_list)
                && let Some((mark, len)) = opens_fence(text)
            {
                fence = Some(Fence {
                    mark,
                    len,
                    start: pos,
                    close_indent: if flow.in_list { usize::MAX } else { 3 },
                });
                flow.no_paragraph = false;
                pos = end + 1;
                continue;
            }
            flow.no_paragraph = rule || heading;
            if lead == 0 && ESM_START.is_match(line) {
                esm.feed(line);
                self.blank(pos, end);
                pos = end + 1;
                continue;
            }
            if is_blockquote(text, cols) {
                self.blank(pos, end);
                pos = end + 1;
                continue;
            }
            if text.starts_with('|') {
                self.dash_skip.push(Span { start: pos, end });
            } else if let Some(dash) = LEAD_TERM.captures(line).and_then(|c| c.get(1)) {
                self.dash_skip.push(Span {
                    start: pos + dash.start(),
                    end: pos + dash.end(),
                });
            }
            pos = self.inline(pos, end);
        }
        self.end_code(&mut flow);
        if let Some(open) = fence {
            self.blank(open.start, doc.len());
        }
    }

    /// Masks the indented code block that was being read, if any.
    fn end_code(&mut self, flow: &mut Flow) {
        if let Some(code) = flow.code.take() {
            self.blank(code.start, code.end);
        }
    }

    /// Masks the frontmatter and returns where the lines after it start. The doc must start with
    /// a `---` line and have a `---` or `...` line later, or start with a `+++` line (TOML) and
    /// have a `+++` line later; otherwise there is no frontmatter.
    fn frontmatter(&mut self) -> usize {
        let doc = self.doc;
        let first_end = line_end(doc, 0);
        let closers: &[&str] = match doc[..first_end].trim_end() {
            "---" => &["---", "..."],
            "+++" => &["+++"],
            _ => return 0,
        };
        let mut pos = first_end + 1;
        while pos < doc.len() {
            let end = line_end(doc, pos);
            if closers.contains(&doc[pos..end].trim_end()) {
                self.blank(0, end);
                return end + 1;
            }
            pos = end + 1;
        }
        0
    }

    /// Masks the inline code, HTML comments and `<pre>` blocks of one line, from `from` to `end`,
    /// and returns where the next line to read starts. A comment or a `<pre>` block may close on a
    /// later line; the rest of that line is read too.
    fn inline(&mut self, from: usize, end: usize) -> usize {
        let doc = self.doc;
        let bytes = doc.as_bytes();
        let (mut cur, mut end) = (from, end);
        while cur < end {
            let Some(at) = (cur..end).find(|&k| match bytes[k] {
                b'`' => true,
                b'<' => bytes[k..end].starts_with(b"<!--") || is_pre_open(&bytes[k..end]),
                _ => false,
            }) else {
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
            } else if bytes[at..end].starts_with(b"<!--") {
                if let Some(found) = self.comment_close(at + 4) {
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
            } else if let Some(close) = self.pre_close(at + 4) {
                self.blank(at, close);
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

    /// Where the next `</pre>` at or after `from` ends.
    fn pre_close(&mut self, from: usize) -> Option<usize> {
        if self.no_more_pre_close {
            return None;
        }
        let found = find_pre_close(self.doc.as_bytes(), from);
        self.no_more_pre_close = found.is_none();
        found
    }

    /// The constructs that sit inside one line, on what the line pass left.
    fn inline_constructs(&mut self) {
        self.mask_line_start_tags();
        self.mask_all(&TAG);
        self.mask_all(&LINK_TARGET);
        self.mask_all(&REF_DEF);
        self.mask_all(&IMPORT_EXPORT);
        common::mask_urls(&mut self.buf);
        mask_quotes(&mut self.buf);
    }

    /// Masks each tag that starts its line, over as many lines as it takes: the next `>` that is
    /// not inside a quoted attribute value or a `{...}` expression ends it. A tag with no such
    /// `>` before a blank line is left to `TAG`, and the lines it covered are not tried again.
    fn mask_line_start_tags(&mut self) {
        let doc = self.doc.as_bytes();
        let mut spans = Vec::new();
        let mut from = 0;
        while let Some(i) = self.buf[from..].iter().position(|&b| b == b'<') {
            let at = from + i;
            from = at + 1;
            if !doc.get(at + 1).is_some_and(u8::is_ascii_alphabetic) || !starts_line(doc, at) {
                continue;
            }
            match tag_end(doc, at) {
                Ok(end) => {
                    spans.push(Span { start: at, end });
                    from = end;
                }
                Err(stop) => from = from.max(stop),
            }
        }
        for span in spans {
            common::blank(&mut self.buf, span);
        }
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

/// The indent of `line` in columns (a tab goes to the next multiple of 4) and in bytes.
fn leading(line: &str) -> (usize, usize) {
    let mut cols = 0;
    let mut bytes = 0;
    for b in line.bytes() {
        match b {
            b' ' => cols += 1,
            b'\t' => cols += 4 - cols % 4,
            _ => break,
        }
        bytes += 1;
    }
    (cols, bytes)
}

/// The character and length of a fence that opens on `text`, a line without its indent: 3 or more
/// backticks or tildes. The info string of a backtick fence holds no backtick. The caller checks
/// the indent.
fn opens_fence(text: &str) -> Option<(u8, usize)> {
    let rest = text.as_bytes();
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

/// Whether `line` closes `fence`: at most its indent limit, at least as many marks as it opened
/// with, and only blanks after them.
fn closes_fence(line: &str, fence: &Fence) -> bool {
    let (cols, lead) = leading(line);
    if cols > fence.close_indent {
        return false;
    }
    let rest = &line.as_bytes()[lead..];
    let run = rest.iter().take_while(|&&b| b == fence.mark).count();
    run >= fence.len && rest[run..].iter().all(|b| b.is_ascii_whitespace())
}

/// `>` after at most 3 columns of indent, unless a `=` or digit follows (a comparison). `text` is
/// the line without its indent, `cols` the width of the indent.
fn is_blockquote(text: &str, cols: usize) -> bool {
    let bytes = text.as_bytes();
    cols <= 3 && bytes.first() == Some(&b'>') && !matches!(bytes.get(1), Some(b'=' | b'0'..=b'9'))
}

/// An ATX heading: 1 to 6 `#`, then a blank or the end of the line.
fn is_heading(text: &str) -> bool {
    let hashes = text.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes)
        && text
            .as_bytes()
            .get(hashes)
            .is_none_or(|b| b.is_ascii_whitespace())
}

/// A thematic break or a setext underline: 3 or more of one of `-`, `*`, `_`, `=` and blanks.
fn is_rule(text: &str) -> bool {
    let mut marks = text.bytes().filter(|b| !b.is_ascii_whitespace());
    let Some(mark) = marks
        .next()
        .filter(|b| matches!(b, b'-' | b'*' | b'_' | b'='))
    else {
        return false;
    };
    let mut count = 1;
    for b in marks {
        if b != mark {
            return false;
        }
        count += 1;
    }
    count >= 3
}

/// A bullet or an ordered-list marker followed by a blank or the end of the line.
fn is_list_marker(text: &str) -> bool {
    let bytes = text.as_bytes();
    let after = match bytes.first() {
        Some(b'-' | b'*' | b'+') => 1,
        Some(b'0'..=b'9') => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return false;
            }
            digits + 1
        }
        _ => return false,
    };
    bytes.get(after).is_none_or(|b| b.is_ascii_whitespace())
}

/// Whether only blanks lie between the start of the line and `at`.
fn starts_line(doc: &[u8], at: usize) -> bool {
    let before = &doc[..at];
    let blanks = before
        .iter()
        .rev()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    blanks == before.len() || before[before.len() - blanks - 1] == b'\n'
}

/// Whether the line that starts at `from` holds nothing but blanks, or the doc ends there.
fn blank_line_at(doc: &[u8], from: usize) -> bool {
    let rest = doc.get(from..).unwrap_or_default();
    rest.iter()
        .find(|&&b| b != b' ' && b != b'\t' && b != b'\r')
        .is_none_or(|&b| b == b'\n')
}

/// Where the tag that opens at `at` ends: just after its `>`, which is not inside a quoted
/// attribute value (after an `=`) or a `{...}` expression, whose strings are quoted too. When the
/// doc ends or a blank line comes first, the offset where the scan stopped is the error.
fn tag_end(doc: &[u8], at: usize) -> Result<usize, usize> {
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut last = b'<';
    for (j, &b) in doc.iter().enumerate().skip(at + 1) {
        if b == b'\n' && blank_line_at(doc, j + 1) {
            return Err(j);
        }
        if let Some(open) = quote {
            if b == open {
                quote = None;
                last = b;
            }
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' if depth > 0 || (b != b'`' && last == b'=') => quote = Some(b),
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'>' if depth == 0 => return Ok(j + 1),
            _ => {}
        }
        if !b.is_ascii_whitespace() {
            last = b;
        }
    }
    Err(doc.len())
}

/// Whether `rest` starts with `<pre` and then a blank, `>`, `/` or the end: not `<prefix>`.
fn is_pre_open(rest: &[u8]) -> bool {
    rest.len() >= 4
        && rest[1..4].eq_ignore_ascii_case(b"pre")
        && rest
            .get(4)
            .is_none_or(|&b| b == b'>' || b == b'/' || b.is_ascii_whitespace())
}

/// Where the first `</pre>` (any case, blanks allowed before the `>`) at or after `from` ends.
fn find_pre_close(doc: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    while let Some(i) = doc.get(at..)?.iter().position(|&b| b == b'<') {
        let open = at + i;
        at = open + 1;
        let rest = &doc[open..];
        if rest.len() >= 5 && rest[1] == b'/' && rest[2..5].eq_ignore_ascii_case(b"pre") {
            let blanks = rest[5..]
                .iter()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            if rest.get(5 + blanks) == Some(&b'>') {
                return Some(open + 5 + blanks + 1);
            }
        }
    }
    None
}

/// How far a multi-line `import` or `export` statement has got: the brackets still open and
/// whether a template literal or a block comment is. Strings, `//` comments and the text of
/// template literals and block comments hold no brackets.
#[derive(Default)]
struct Esm {
    depth: usize,
    template: bool,
    comment: bool,
}

impl Esm {
    /// Whether the statement goes on after the last line fed.
    fn active(&self) -> bool {
        self.depth > 0 || self.template || self.comment
    }

    fn feed(&mut self, line: &str) {
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if self.comment {
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    self.comment = false;
                    i += 1;
                }
            } else if self.template {
                match c {
                    b'\\' => i += 1,
                    b'`' => self.template = false,
                    _ => {}
                }
            } else {
                match c {
                    b'{' | b'[' | b'(' => self.depth += 1,
                    b'}' | b']' | b')' => self.depth = self.depth.saturating_sub(1),
                    b'`' => self.template = true,
                    b'\'' | b'"' => {
                        i += 1;
                        while i < b.len() && b[i] != c {
                            i += if b[i] == b'\\' { 2 } else { 1 };
                        }
                    }
                    b'/' if b.get(i + 1) == Some(&b'/') => return,
                    b'/' if b.get(i + 1) == Some(&b'*') => {
                        self.comment = true;
                        i += 1;
                    }
                    _ => {}
                }
            }
            i += 1;
        }
    }
}

fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// Masks the content of quoted spans, keeping the marks. A `"` or `“` opens a span that closes at
/// the next `"` or `”`, and a `‘` one that closes at the next `’` that no letter or digit follows
/// (an apostrophe, as in `don’t`, is part of the span). A mark opens a span only when the byte
/// before it is not an ASCII letter or digit. A span may wrap onto the next lines, but it ends at
/// a blank line, at a backtick and after 300 characters, line breaks included.
fn mask_quotes(buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let (open, single) = if buf[i] == b'"' {
            (1, false)
        } else if buf[i..].starts_with(OPEN_DOUBLE) {
            (OPEN_DOUBLE.len(), false)
        } else if buf[i..].starts_with(OPEN_SINGLE) {
            (OPEN_SINGLE.len(), true)
        } else {
            i += 1;
            continue;
        };
        if i > 0 && buf[i - 1].is_ascii_alphanumeric() {
            i += open;
            continue;
        }
        let content = i + open;
        match quote_close(buf, content, single) {
            Some((close, len)) => {
                common::blank(
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
fn quote_close(buf: &[u8], from: usize, single: bool) -> Option<(usize, usize)> {
    let mut chars = 0;
    let mut j = from;
    while j < buf.len() {
        let b = buf[j];
        if single {
            if buf[j..].starts_with(CLOSE_SINGLE)
                && !buf
                    .get(j + CLOSE_SINGLE.len())
                    .is_some_and(u8::is_ascii_alphanumeric)
            {
                return Some((j, CLOSE_SINGLE.len()));
            }
        } else if b == b'"' {
            return Some((j, 1));
        } else if buf[j..].starts_with(CLOSE_DOUBLE) {
            return Some((j, CLOSE_DOUBLE.len()));
        }
        if b == b'`' || (b == b'\n' && blank_line_at(buf, j + 1)) {
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
        // four spaces of indent under a paragraph line: a lazy continuation, not a fence
        check("text\n    ```\nprose delve\n", &[]);
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
        // a quote wraps onto the next line, and stops at a blank line
        check("\"a\nb\"", &["a\nb"]);
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
            "import Foo from './foo'\nexport const meta = { title: 'x' }\nImport is capitalized prose\nimporting is prose\nexported prose\n",
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
    fn fences_in_list_items_may_be_indented_more_than_three_spaces() {
        // MkDocs: four spaces under a numbered item
        check(
            "1. Install the client:\n\n    ```python\n    client = Client(delve=True)\n    ```\n\nprose delve\n",
            &["    ```python\n    client = Client(delve=True)\n    ```"],
        );
        // five spaces under a bullet, then the next item
        check(
            "- a\n\n     ```sh\n     x\n     ```\n- b\n",
            &["     ```sh\n     x\n     ```"],
        );
        // six spaces, tildes, no blank line before the fence
        check(
            "* a\n      ~~~\n      x\n      ~~~\ntext",
            &["      ~~~\n      x\n      ~~~"],
        );
        // a tab, and a nested item
        check(
            "- a\n\n\t```sh\n\tx\n\t```\nafter",
            &["\t```sh\n\tx\n\t```"],
        );
        check(
            "- a\n  - b\n\n        ```\n        x\n        ```\nafter",
            &["        ```\n        x\n        ```"],
        );
        // an unclosed one runs to the end
        check("- a\n\n    ```\n    x\n", &["    ```\n    x"]);
        // other indented lines in an item are prose
        check("- item\n\n    continued delve\n", &[]);
    }

    #[test]
    fn indented_code_blocks_are_masked() {
        // after a blank line, with a blank line inside; the blank lines after it are not code
        check(
            "Intro paragraph.\n\n    client = Client(delve=True)\n    more\n\n    after blank\n\nprose delve\n",
            &["    client = Client(delve=True)\n    more\n\n    after blank"],
        );
        // a tab, at the start of the doc, after a heading, after a fence
        check(
            "Intro\n\n\tcode with a tab\nprose delve\n",
            &["\tcode with a tab"],
        );
        check("    code at start\n\nprose", &["    code at start"]);
        check("# Title\n    code\nprose", &["    code"]);
        check(
            "```\nx\n```\n    code delve\nprose",
            &["```\nx\n```", "    code delve"],
        );
        // a paragraph line makes the next indented line a continuation, not code
        check("para\n    still prose delve\n", &[]);
        // in a list item it is prose; after the list ends it is code again
        check("- item\n\n    continued delve\n", &[]);
        check("- item\n\nplain\n\n    code\n", &["    code"]);
        // a whitespace-only line between two code lines is inside the block
        check("x\n\n    a\n  \n    b\nprose", &["    a\n  \n    b"]);
        // code is not a table row, a heading or a lead term
        let v = check(
            "text\n\n    | a | \u{2014} |\n    - **Term** \u{2014} x\n",
            &["    | a | \u{2014} |\n    - **Term** \u{2014} x"],
        );
        assert!(v.dash_skip.is_empty());
    }

    #[test]
    fn html_pre_blocks_are_masked() {
        check(
            "before <pre>\ncode seamless\n```\n> not a quote\n</pre> after delve\n",
            &["<pre>\ncode seamless\n```\n> not a quote\n</pre>"],
        );
        // any case, attributes, nested code, two blocks
        check(
            "a <PRE class=\"x\">delve</PRE> b\n<pre><code>seamless\nmore</code></pre>\nc",
            &[
                "<PRE class=\"x\">delve</PRE>",
                "<pre><code>seamless\nmore</code></pre>",
            ],
        );
        // another element that starts with pre is not a pre block
        check("<prefix>delve</prefix>", &["<prefix>", "</prefix>"]);
        // no closing tag: only the tag is masked
        check("<pre>\ndelve\n", &["<pre>"]);
        // code spans come first
        check("`<pre>` delve </pre>", &["`<pre>`", "</pre>"]);
    }

    #[test]
    fn toml_frontmatter_is_masked() {
        check(
            "+++\ntitle = \"Seamless delve\"\ntags = [\"a\"]\n+++\nBody delve\n",
            &["+++\ntitle = \"Seamless delve\"\ntags = [\"a\"]\n+++"],
        );
        // only +++ closes it
        check(
            "+++\nx = 1\n---\n...\n+++\nBody",
            &["+++\nx = 1\n---\n...\n+++"],
        );
        // no close, or four plus signs: not frontmatter
        check("+++\nx = 1\nBody", &[]);
        check("++++\nx\n++++\nBody", &[]);
    }

    #[test]
    fn multi_line_mdx_export_and_import_statements_are_masked() {
        check(
            "export const meta = {\n  title: 'Seamless sync',\n  tags: ['delve'],\n}\n\nProse delve.\n",
            &["export const meta = {\n  title: 'Seamless sync',\n  tags: ['delve'],\n}"],
        );
        check(
            "import {\n  Alpha,\n  Beta,\n} from './x'\nProse delve\n",
            &["import {\n  Alpha,\n  Beta,\n} from './x'"],
        );
        check(
            "export default function Layout({ children }) {\n  return <div>{children}</div>\n}\nTail delve\n",
            &["export default function Layout({ children }) {\n  return <div>{children}</div>\n}"],
        );
        // a blank line inside, brackets in strings and comments, a template literal over lines
        let statement =
            "export const a = {\n  b: ')',\n\n  // }\n  c: \"(\", /* ] */\n  d: `\n}\n`,\n}";
        check(&format!("{statement}\nProse delve\n"), &[statement]);
        // an unclosed statement runs to the end
        check("export const a = {\n  x\n", &["export const a = {\n  x"]);
        // prose that starts with the word masks its own line only, whatever its brackets
        check(
            "import duties (the tax\nrise delve\n",
            &["import duties (the tax"],
        );
        // not at the start of the line
        check("  export const a = {\n  x\n}\n", &[]);
    }

    #[test]
    fn multi_line_jsx_tags_are_masked() {
        check(
            "<Callout\n  type=\"info\"\n  title=\"Seamless sync\"\n>\nBody delve stays\n</Callout>\n",
            &[
                "<Callout\n  type=\"info\"\n  title=\"Seamless sync\"\n>",
                "</Callout>",
            ],
        );
        // an arrow function and a quoted > do not end the tag; indented, self-closing
        check(
            "  <Foo\n    onClick={() => go('a')}\n    title=\"a > b\"\n  />\nText delve\n",
            &["<Foo\n    onClick={() => go('a')}\n    title=\"a > b\"\n  />"],
        );
        // a quoted > on one line
        check("<a href=\"x>y\">z</a>", &["<a href=\"x>y\">", "</a>"]);
        // no closing > before a blank line: not a tag
        check("<b and\nprose delve\n\nmore >\n", &[]);
        // only a tag that starts its line runs over lines
        check("see <b and\nmore > here", &[]);
        check("<Foo\n  a=b", &[]);
    }

    #[test]
    fn wrapped_double_quoted_spans_are_masked() {
        check(
            "He said \"seamless\nsync\" twice, \u{201c}curly\nwrap\u{201d} too.\n",
            &["seamless\nsync", "curly\nwrap"],
        );
        // a blank or whitespace-only line ends the search
        check("\"a\n\nb\"", &[]);
        check("\"a\n  \nb\"", &[]);
        check("\"a\r\n\r\nb\"", &[]);
        // the 300 character limit counts the line breaks too
        let long = format!("\"{}\n{}\"", "x".repeat(200), "y".repeat(200));
        check(&long, &[]);
        let fits = format!("\"{}\n{}\"", "x".repeat(100), "y".repeat(100));
        check(&fits, &[&fits[1..fits.len() - 1]]);
    }

    #[test]
    fn curly_single_quoted_spans_are_masked() {
        check(
            "the \u{2018}seamless\u{2019} sync and \u{2018}wrapped\nlines\u{2019} too",
            &["seamless", "wrapped\nlines"],
        );
        // an apostrophe inside the span does not close it
        check(
            "\u{2018}don\u{2019}t stop\u{2019} now",
            &["don\u{2019}t stop"],
        );
        // apostrophes alone, a mark after a letter, straight single quotes
        check(
            "it\u{2019}s fine, the users\u{2019} guide, don\u{2019}t",
            &[],
        );
        check("x\u{2018}y\u{2019} z", &[]);
        check("it's 'plain' text", &[]);
        // a blank line ends the search
        check("\u{2018}a\n\nb\u{2019}", &[]);
    }

    #[test]
    fn new_constructs_keep_offsets_and_newlines() {
        let doc = "+++\nt = \"caf\u{e9}\"\n+++\nIntro\n\n    caf\u{e9} \u{2014} code\n\n\
                   <pre>\u{e9}\n\u{2014}</pre> \u{2018}\u{e9}\n\u{e9}\u{2019}\n\
                   export const a = {\n  b: '\u{e9}',\n}\n\
                   <Foo\n  a=\"\u{e9}\"\n/>\n- x\n\n    ```\n    \u{e9}\n    ```\n\
                   \"w\u{e9}\nx\" y\r\n";
        let v = view(doc);
        assert_eq!(v.text.len(), doc.len());
        assert_eq!(newlines(&v.text), newlines(doc));
        assert!(
            v.text
                .bytes()
                .zip(doc.bytes())
                .all(|(t, d)| t == d || t == MASK),
            "{}",
            show(&v.text)
        );
        assert!(v.text.contains("Intro"), "{}", show(&v.text));
        for gone in ["caf", "export", "<Foo", "\u{e9}"] {
            assert!(!v.text.contains(gone), "{gone}: {}", show(&v.text));
        }
    }

    /// Runs `f` and fails when it takes longer than `secs`: a quadratic scan on 1 MiB takes minutes.
    fn within(secs: u64, what: &str, f: impl FnOnce()) {
        let start = std::time::Instant::now();
        f();
        let took = start.elapsed();
        assert!(took.as_secs() < secs, "{what} took {took:?}");
    }

    #[test]
    fn masking_stays_linear_on_hostile_input() {
        let mib = 1 << 20;
        let cases = [
            ("unclosed tags", "<Foo\n".repeat(mib / 5)),
            ("unclosed attribute quotes", "<Foo a=\"\n".repeat(mib / 9)),
            ("tags that give up", "<a\n\n".repeat(mib / 4)),
            ("unclosed pre", "<pre>\n".repeat(mib / 6)),
            ("closed pre", "<pre>x</pre>\n".repeat(mib / 13)),
            ("unclosed export", "export const a = [\n".repeat(mib / 18)),
            ("export lookalikes", "import duties (\n".repeat(mib / 16)),
            ("indented code", "    x\n\n".repeat(mib / 7)),
            (
                "list fences",
                "- a\n    ```\n    x\n    ```\n".repeat(mib / 28),
            ),
            ("wrapped quotes", "\"x\ny\n".repeat(mib / 5)),
            ("unclosed quotes", "\" ".repeat(mib / 2)),
            ("curly single quotes", "\u{2018}x\n".repeat(mib / 5)),
        ];
        for (what, doc) in &cases {
            within(10, what, || {
                let v = view(doc);
                assert_eq!(v.text.len(), doc.len(), "{what}");
            });
        }
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
