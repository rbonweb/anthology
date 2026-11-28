//! The JSX and TSX view. Only JSX text between tags and the string values of a few attributes are
//! `text`; every other string literal is code. Comments and ignore regions are not code.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::common::{blank, into_string, mask_quotes, mask_urls};
use super::css::{comment_spans, keep, masked_code, span_of};
use super::{MASK, View};
use crate::Span;

/// A `className` value: double-quoted, single-quoted, or a brace expression without nested
/// braces (groups 1 to 3).
static CLASS_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bclassName\s*=\s*(?:"([^"]*)"|'([^']*)'|\{([^{}]*)\})"#)
        .expect("className regex")
});

/// Attributes whose string value is prose.
const PROSE_ATTRS: [&[u8]; 5] = [b"alt", b"title", b"placeholder", b"aria-label", b"label"];

/// Words after which an expression, and so a JSX element, can start.
const EXPRESSION_WORDS: [&[u8]; 10] = [
    b"return", b"default", b"case", b"yield", b"await", b"else", b"throw", b"do", b"typeof",
    b"void",
];

pub(crate) fn view(doc: &str) -> View {
    let (code_bytes, waivers) = masked_code(doc, &comment_spans(doc, b";{}(),"));
    let code = into_string(code_bytes);

    let mut text = code.as_bytes().to_vec();
    blank(
        &mut text,
        Span {
            start: 0,
            end: doc.len(),
        },
    );
    for found in Scan::new(&code).run() {
        keep(code.as_bytes(), &mut text, found);
    }
    mask_quotes(&mut text);
    mask_urls(&mut text);

    let classes = CLASS_NAME
        .captures_iter(&code)
        .filter_map(|caps| caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)))
        .map(span_of)
        .collect();
    View {
        text: into_string(text),
        code,
        style: vec![Span {
            start: 0,
            end: doc.len(),
        }],
        classes,
        dash_skip: Vec::new(),
        waivers,
    }
}

/// Whether a stretch between angle brackets is JSX text and not code (`a > b && c < d`): it
/// holds a letter and none of `=`, `;`, `&&`, `||`, `=>`, `(` or `)`.
fn is_text(segment: &str) -> bool {
    segment.chars().any(char::is_alphabetic)
        && !segment.contains(['=', ';', '(', ')'])
        && !segment.contains("&&")
        && !segment.contains("||")
}

/// What the scanner is reading. The bottom of the stack is always `Code`.
enum Frame {
    /// The top level of the file.
    Code,
    /// Code in `{ }`: a block, a JSX expression, or an attribute value.
    Block,
    /// A template literal.
    Template,
    /// An opening tag, from after its `<` to its `>`.
    Tag(TagState),
    /// The children of an element, up to its closing tag.
    Children,
}

/// Where an opening tag stands.
struct TagState {
    /// The tag name (empty for a fragment).
    name: Span,
    /// The last attribute name read.
    attr: Option<Span>,
    /// An `=` came right after `attr`.
    equals: bool,
}

/// One left-to-right pass that finds the spans of JSX text and prose attribute values.
///
/// It reads code, strings and template literals so that only a real element counts: text sits
/// between an opening tag and its closing tag, and a string is text only as the value of a prose
/// attribute. A tag that turns out not to be one (a generic arrow function, a type) is given up
/// at once, and no byte is read twice, so the pass is linear in the size of the file.
struct Scan<'a> {
    code: &'a str,
    bytes: &'a [u8],
    /// Where the last `</name` of each name starts. An opening tag with no closing tag after it
    /// is not an element.
    closers: HashMap<&'a [u8], usize>,
    stack: Vec<Frame>,
    found: Vec<Span>,
    i: usize,
    /// Where the JSX text being read starts.
    segment: usize,
}

impl<'a> Scan<'a> {
    fn new(code: &'a str) -> Self {
        let bytes = code.as_bytes();
        let mut closers = HashMap::new();
        let mut at = 0;
        while at + 1 < bytes.len() {
            if bytes[at] == b'<' && bytes[at + 1] == b'/' {
                let end = name_end(bytes, at + 2);
                closers.insert(&bytes[at + 2..end], at);
                at = end;
            } else {
                at += 1;
            }
        }
        Scan {
            code,
            bytes,
            closers,
            stack: vec![Frame::Code],
            found: Vec::new(),
            i: 0,
            segment: 0,
        }
    }

    fn run(mut self) -> Vec<Span> {
        while self.i < self.bytes.len() {
            match self.stack.last() {
                Some(Frame::Tag(_)) => self.tag(),
                Some(Frame::Children) => self.children(),
                Some(Frame::Template) => self.template(),
                _ => self.code_byte(),
            }
        }
        self.found
    }

    /// Leaves the top frame; children resume their text where the frame ended.
    fn pop(&mut self) {
        self.stack.pop();
        if matches!(self.stack.last(), Some(Frame::Children)) {
            self.segment = self.i;
        }
    }

    /// Records the text from `segment` to `end` when it is prose and not code.
    fn emit(&mut self, end: usize) {
        if let Some(text) = self.code.get(self.segment..end)
            && is_text(text)
        {
            self.found.push(Span {
                start: self.segment,
                end,
            });
        }
    }

    /// One byte of code, at the top level or in braces.
    fn code_byte(&mut self) {
        let at = self.i;
        match self.bytes[at] {
            quote @ (b'"' | b'\'') => self.i = string_end(self.bytes, at, quote),
            b'`' => {
                self.i += 1;
                self.stack.push(Frame::Template);
            }
            b'{' => {
                self.i += 1;
                self.stack.push(Frame::Block);
            }
            b'}' => {
                self.i += 1;
                if matches!(self.stack.last(), Some(Frame::Block)) {
                    self.pop();
                }
            }
            b'<' => match self.open_tag(at) {
                Some(name) if self.expression_at(at) => self.enter_tag(name),
                _ => self.i += 1,
            },
            _ => self.i += 1,
        }
    }

    /// One step in a template literal: its text, or the code of a `${ }`.
    fn template(&mut self) {
        match self.bytes[self.i] {
            b'\\' => self.i += 2,
            b'`' => {
                self.i += 1;
                self.pop();
            }
            b'$' if self.bytes.get(self.i + 1) == Some(&b'{') => {
                self.i += 2;
                self.stack.push(Frame::Block);
            }
            _ => self.i += 1,
        }
    }

    /// One step in the children of an element: text up to the next `<` or `{`.
    fn children(&mut self) {
        let bytes = self.bytes;
        let Some(found) = bytes[self.i..].iter().position(|&b| b == b'<' || b == b'{') else {
            self.i = bytes.len();
            return;
        };
        let at = self.i + found;
        if bytes[at] == b'{' {
            self.emit(at);
            self.i = at + 1;
            self.stack.push(Frame::Block);
        } else if bytes.get(at + 1) == Some(&b'/') {
            self.emit(at);
            let mut end = name_end(bytes, at + 2);
            while bytes.get(end).is_some_and(|&b| is_blank(b)) {
                end += 1;
            }
            if bytes.get(end) == Some(&b'>') {
                end += 1;
            }
            self.i = end;
            self.pop();
        } else if let Some(name) = self.open_tag(at) {
            self.emit(at);
            self.enter_tag(name);
        } else {
            // A `<` that opens nothing is part of the text.
            self.i = at + 1;
        }
    }

    /// One step in an opening tag: a name, an attribute, or the end of the tag.
    fn tag(&mut self) {
        let bytes = self.bytes;
        let at = self.i;
        let byte = bytes[at];
        let Some(Frame::Tag(tag)) = self.stack.last_mut() else {
            self.i += 1;
            return;
        };
        match byte {
            b if is_blank(b) => self.i += 1,
            b'>' => {
                self.i += 1;
                self.finish_tag();
            }
            b'/' if bytes.get(at + 1) == Some(&b'>') => {
                self.i += 2;
                self.pop();
            }
            b'{' => {
                tag.equals = false;
                self.i += 1;
                self.stack.push(Frame::Block);
            }
            b'=' => {
                tag.equals = tag.attr.is_some();
                self.i += 1;
            }
            quote @ (b'"' | b'\'') => {
                let end = string_close(bytes, at, quote).unwrap_or(bytes.len());
                if tag.equals
                    && tag
                        .attr
                        .is_some_and(|a| PROSE_ATTRS.contains(&&bytes[a.start..a.end]))
                {
                    self.found.push(Span { start: at + 1, end });
                }
                tag.equals = false;
                self.i = end + 1;
            }
            b if is_name_byte(b) => {
                let end = name_end(bytes, at);
                tag.attr = Some(Span { start: at, end });
                tag.equals = false;
                self.i = end;
            }
            // Not a tag after all: this byte is code or text again.
            _ => self.pop(),
        }
    }

    /// Enters the opening tag whose name is `name`, just after its `<`.
    fn enter_tag(&mut self, name: Span) {
        self.i = name.end;
        self.stack.push(Frame::Tag(TagState {
            name,
            attr: None,
            equals: false,
        }));
    }

    /// The `>` of an opening tag was read: the element has children when a closing tag follows.
    fn finish_tag(&mut self) {
        let Some(Frame::Tag(tag)) = self.stack.last() else {
            return;
        };
        let name = tag.name;
        self.pop();
        let closes = self
            .closers
            .get(&self.bytes[name.start..name.end])
            .is_some_and(|&close| close >= self.i);
        if closes {
            self.stack.push(Frame::Children);
            self.segment = self.i;
        }
    }

    /// The name of the tag whose `<` is at `at`, when one can start there: a name followed by
    /// space, `>` or `/` (so not `<T,>` or `<T = any>`), and not `<T extends X>`; or `<>`.
    fn open_tag(&self, at: usize) -> Option<Span> {
        let bytes = self.bytes;
        let start = at + 1;
        let first = *bytes.get(start)?;
        if first == b'>' {
            return Some(Span { start, end: start });
        }
        if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') {
            return None;
        }
        let end = name_end(bytes, start);
        if !bytes
            .get(end)
            .is_some_and(|&b| is_blank(b) || b == b'>' || b == b'/')
        {
            return None;
        }
        let mut word = end;
        while bytes.get(word).is_some_and(|&b| is_blank(b)) {
            word += 1;
        }
        let generic = bytes[word..].starts_with(b"extends")
            && !bytes.get(word + 7).is_some_and(|&b| is_name_byte(b));
        (!generic).then_some(Span { start, end })
    }

    /// Whether code can start an expression at `at`: after `(`, `,`, `=`, `=>`, `:`, `?`, `[`,
    /// `{`, `;`, `&`, `|`, `!`, `return` and the like, or at the start of the file. After a
    /// name or a bracket the `<` compares or opens type arguments.
    fn expression_at(&self, at: usize) -> bool {
        let bytes = self.bytes;
        let mut end = at;
        while end > 0 && is_blank(bytes[end - 1]) {
            end -= 1;
        }
        if end == 0 {
            return true;
        }
        match bytes[end - 1] {
            b'(' | b',' | b'=' | b':' | b'?' | b'[' | b'{' | b';' | b'&' | b'|' | b'!' => true,
            b'>' => end >= 2 && bytes[end - 2] == b'=',
            b if is_name_byte(b) => {
                let mut start = end;
                while start > 0 && end - start <= 8 && is_name_byte(bytes[start - 1]) {
                    start -= 1;
                }
                EXPRESSION_WORDS.contains(&&bytes[start..end])
            }
            _ => false,
        }
    }
}

/// Whitespace, or a masked comment.
fn is_blank(b: u8) -> bool {
    b.is_ascii_whitespace() || b == MASK
}

/// A byte of a tag or attribute name (any non-ASCII byte counts).
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'-' | b'.' | b':') || !b.is_ascii()
}

/// Where the name that starts at `from` ends.
fn name_end(bytes: &[u8], from: usize) -> usize {
    bytes[from.min(bytes.len())..]
        .iter()
        .position(|&b| !is_name_byte(b))
        .map_or(bytes.len(), |n| from + n)
}

/// Where the quote closing the string that opens at `at` is, if it does.
fn string_close(bytes: &[u8], at: usize, quote: u8) -> Option<usize> {
    bytes[at + 1..]
        .iter()
        .position(|&b| b == quote)
        .map(|n| at + 1 + n)
}

/// Where the code string that opens at `at` ends: after its closing quote, or at the end of the
/// line when it has none (a string cannot span lines).
fn string_end(bytes: &[u8], at: usize, quote: u8) -> usize {
    let mut j = at + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'\n' => return j,
            b if b == quote => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::super::css::support::{assert_aligned, runs, slices};
    use super::*;
    use crate::Span;
    use crate::mask::Waiver;

    fn waiver(line: usize, rule: Option<&'static str>, reason: &str, valid: bool) -> Waiver {
        Waiver {
            line,
            rule,
            reason: reason.to_string(),
            valid,
        }
    }

    fn checked(doc: &str) -> View {
        let v = view(doc);
        assert_aligned(doc, &v);
        v
    }

    #[test]
    fn jsx_text_between_tags_is_text() {
        let doc = "export const A = () => (\n\
            \x20 <div>\n\
            \x20   <h1>Hello world</h1>\n\
            \x20   <p>Hi {name}, welcome back</p>\n\
            \x20   <>fragment text</>\n\
            \x20 </div>\n\
            );\n";
        let v = checked(doc);
        assert_eq!(
            runs(&v.text),
            ["Hello world", "Hi", ", welcome back", "fragment text"]
        );
    }

    #[test]
    fn code_between_angle_brackets_is_not_text() {
        let doc = "if (a > b && c < d) {}\n\
            const f = (x) => x + 1;\n\
            const g = a < b;\n\
            const t: Array<string> = [];\n\
            type R = Promise<void>\n\
            export default function App() {\n\
            \x20 const ok = a > 1 || b < 2;\n\
            \x20 return null;\n\
            }\n";
        let v = checked(doc);
        assert!(runs(&v.text).is_empty(), "{:?}", runs(&v.text));
    }

    #[test]
    fn typescript_after_a_brace_is_not_text() {
        let doc = "}\n\nexport function elevate<T>(x: T): T { return x }\n";
        let v = checked(doc);
        assert!(runs(&v.text).is_empty(), "{:?}", runs(&v.text));
    }

    #[test]
    fn string_literals_with_markup_are_not_text() {
        let doc = "const help = \"<p>I hope this helps</p>\"\n\
            const single = '<b>seamless</b>';\n\
            const tpl = `<i>${x} delve</i>`;\n\
            const attr = 'title=\"seamless delve\" alt=\"robust\"';\n";
        let v = checked(doc);
        assert!(runs(&v.text).is_empty(), "{:?}", runs(&v.text));
    }

    #[test]
    fn generic_arrows_and_types_are_not_jsx() {
        let doc = "const id = <T,>(x: T) => x;\n\
            const f = <T extends object>(x: T) => x;\n\
            type G = <T>(a: T) => void\n\
            export default Foo\n\
            const m = new Map<string, number>()\n\
            const r = useRef<HTMLDivElement>(null)\n";
        let v = checked(doc);
        assert!(runs(&v.text).is_empty(), "{:?}", runs(&v.text));
    }

    #[test]
    fn a_tag_without_a_closing_tag_is_not_an_element() {
        let doc = "type G = <T>(a: T) => void\n\
            const o = {}\n\
            export default class Elevate {}\n";
        let v = checked(doc);
        assert!(runs(&v.text).is_empty(), "{:?}", runs(&v.text));
    }

    #[test]
    fn type_arguments_and_comparisons_are_not_elements() {
        let doc = "const a: Array<b> = []\n\
            const o = {}\n\
            export default class Elevate {}\n\
            const z = <b>ok</b>;\n";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["ok"]);
    }

    #[test]
    fn generic_arrow_with_extends_is_not_an_element() {
        let doc = "const f = <T extends object>(x: T) => x\n\
            const o = {}\n\
            export default class Elevate {}\n\
            const z = <T>ok</T>;\n";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["ok"]);
    }

    #[test]
    fn code_after_jsx_is_not_text() {
        let doc = "export function A() {\n\
            \x20 return <p>Hello there</p>\n\
            }\n\
            \n\
            export function elevate() {}\n\
            const b = cond ? <b>yes</b> : <i>no</i>;\n\
            const label = \"Plain\";\n";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["Hello there", "yes", "no"]);
    }

    #[test]
    fn jsx_inside_expression_containers_is_text() {
        let doc = "const a = (\n\
            \x20 <ul>\n\
            \x20   {items.map((i) => <li key={i}>Don't delve</li>)}\n\
            \x20   {ok && <b>Yes</b>}\n\
            \x20   {`t ${<s>in template</s>}`}\n\
            \x20   <a title=\"a > b\" href={`/x/${id}`}>link</a>\n\
            \x20 </ul>\n\
            );\n";
        let v = checked(doc);
        assert_eq!(
            runs(&v.text),
            ["Don't delve", "Yes", "in template", "a > b", "link"]
        );
    }

    #[test]
    fn listed_string_attributes_are_text() {
        let doc = "const s = \"delve into\";\n\
            const el = (\n\
            \x20 <div>\n\
            \x20   <img alt=\"Delve deep\" className=\"x\" src=\"a.png\" />\n\
            \x20   <input placeholder='Your name' aria-label=\"Search\" data-title=\"no\" />\n\
            \x20   <Tab label=\"Home\" title=\"Go home\" />\n\
            \x20 </div>\n\
            );\n";
        let v = checked(doc);
        assert_eq!(
            runs(&v.text),
            ["Delve deep", "Your name", "Search", "Home", "Go home"]
        );
    }

    #[test]
    fn quotes_and_urls_are_masked_in_text() {
        let doc = "const a = <p>He said \"delve\" and see https://x.example/delve here</p>;\n\
            const b = <img alt=\"see http://y.example/c\" />;\n";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["He said \"", "\" and see", "here", "see"]);
    }

    #[test]
    fn class_name_values_in_all_three_forms_are_class_spans() {
        let doc = "const a = (\n\
            \x20 <div className=\"a b\">\n\
            \x20   <p className='c'>x</p>\n\
            \x20   <span className={cn(\"d\", cond && \"e\")}>y</span>\n\
            \x20   <i wrapperClassName=\"no\" />\n\
            \x20   <u className={`x ${y}`} />\n\
            \x20   {/* <b className=\"hidden\" /> */}\n\
            \x20 </div>\n\
            );\n";
        let v = checked(doc);
        assert_eq!(
            slices(doc, &v.classes),
            ["a b", "c", "cn(\"d\", cond && \"e\")"]
        );
    }

    #[test]
    fn comments_are_masked_but_urls_in_strings_are_not() {
        let doc = "// delve one\n\
            const u = 'https://example.com/seamless';\n\
            const v = 1; // seamless two\n\
            /* block café\n   delve three */\n\
            fn(a,// after comma\n b);\n\
            const w = <a href=\"https://example.com/x\" /> /* delve four */;\n\
            {/* delve five */}\n\
            \x20 // delve six\n";
        let v = checked(doc);
        for kept in [
            "'https://example.com/seamless'",
            "const v = 1;",
            "fn(a,",
            " b);",
            "href=\"https://example.com/x\"",
        ] {
            assert!(v.code.contains(kept), "{kept} is code: {:?}", v.code);
        }
        for gone in [
            "delve",
            "seamless two",
            "block",
            "café",
            "after comma",
            "one",
            "two",
        ] {
            assert!(!v.code.contains(gone), "{gone} is a comment: {:?}", v.code);
        }
    }

    #[test]
    fn broken_markup_is_scanned_once() {
        let shapes = [
            "x = <a b={".repeat(30_000),
            "x = <a b ;\n".repeat(30_000),
            "x = <a b=\"y\" ;\n".repeat(30_000),
            "x = <a>w ".repeat(30_000),
            "x = <a b={`${".repeat(30_000),
            format!("x = <a {}>", " ".repeat(200_000)).repeat(3),
            "<".repeat(200_000),
        ];
        for doc in shapes {
            let started = std::time::Instant::now();
            checked(&doc);
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "took {:?} on {:?}",
                started.elapsed(),
                &doc[..20]
            );
        }
    }

    #[test]
    fn multibyte_text_keeps_offsets() {
        let doc = "// naïve\nconst a = <p title=\"Zoë\">café “délve” — ok</p>; /* ü */\n";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["Zoë", "café “", "” — ok"]);
    }

    #[test]
    fn style_is_the_whole_code() {
        let doc = "const a = <div style={{ top: 0 }} />;\n";
        let v = checked(doc);
        assert_eq!(
            v.style,
            [Span {
                start: 0,
                end: doc.len()
            }]
        );
        assert!(v.dash_skip.is_empty());
    }

    #[test]
    fn jsx_comment_waivers_are_kept() {
        let doc = "const a = <div className=\"x\" />; {/* slop-ok: ui.purple-gradient brand colour from the style guide */}\n\
            // slop-ok: text.ai-vocab named by the user\n\
            /* slop-ok: ui.glow-orb short */\n\
            /* slop-ignore-start quoted */\n\
            <p>delve</p>\n\
            /* slop-ignore-end */\n\
            const b = <p>after</p>;\n";
        let v = checked(doc);
        assert_eq!(
            v.waivers,
            [
                waiver(
                    1,
                    Some("ui.purple-gradient"),
                    "brand colour from the style guide",
                    true
                ),
                waiver(2, Some("text.ai-vocab"), "named by the user", true),
                waiver(3, Some("ui.glow-orb"), "short", false),
                waiver(4, None, "quoted", true),
            ]
        );
        assert_eq!(runs(&v.text), ["after"], "the region is not text");
        assert!(!v.code.contains("delve"), "the region is not code");
        assert!(!v.code.contains("slop-ok"), "the comments are not code");
    }
}
