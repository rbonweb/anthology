//! The HTML view. `text` is the prose: text outside tags, minus `<script>`, `<style>`, `<pre>`
//! and `<code>` elements, plus a few attribute values. `style` holds the CSS of `<style>`
//! elements and `style` attributes, and `classes` the `class` values.

use std::sync::LazyLock;

use regex::Regex;

use super::View;
use super::common::{blank, into_string, mask_quotes, mask_urls};
use super::css::{keep, masked_code, span_of};
use crate::Span;

static COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--[\s\S]*?-->").expect("html comment regex"));

/// A start or end tag (groups: slash, name, attributes; quoted values may hold `>`), a doctype or
/// a processing instruction (no groups).
static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<(/?)([A-Za-z][A-Za-z0-9:-]*)((?:"[^"]*"|'[^']*'|[^>"'])*)>|<![A-Za-z][^>]*>|<\?[^>]*>"#,
    )
    .expect("tag regex")
});

/// One attribute of a tag (groups: name, double-quoted, single-quoted, unquoted value).
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([^\s"'<>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'<>=`]+)))?"#)
        .expect("attribute regex")
});

/// The elements whose content is code, not prose.
const RAW_NAMES: [&str; 4] = ["script", "style", "pre", "code"];

/// Each raw element with the regex for its closing tag.
static RAW: LazyLock<[(&str, Regex); RAW_NAMES.len()]> = LazyLock::new(|| {
    RAW_NAMES.map(|name| {
        let close = Regex::new(&format!(r"(?i)</{name}\s*>")).expect("closing tag regex");
        (name, close)
    })
});

/// The attributes whose value is prose.
const PROSE_ATTRS: [&str; 5] = ["alt", "title", "placeholder", "aria-label", "label"];

pub(crate) fn view(doc: &str) -> View {
    let comments: Vec<Span> = COMMENT
        .find_iter(doc)
        .map(|m| Span {
            start: m.start(),
            end: m.end(),
        })
        .collect();
    let (code_bytes, waivers) = masked_code(doc, &comments);
    let code = into_string(code_bytes);

    let mut scan = Scan::default();
    let mut unclosed = [false; RAW_NAMES.len()];
    let mut pos = 0;
    while let Some(caps) = TAG.captures_at(&code, pos) {
        let Some(whole) = caps.get(0) else { break };
        pos = whole.end();
        scan.hidden.push(span_of(whole));
        let (Some(slash), Some(name), Some(attrs)) = (caps.get(1), caps.get(2), caps.get(3)) else {
            continue;
        };
        if !slash.as_str().is_empty() {
            continue;
        }
        let name = name.as_str().to_ascii_lowercase();
        scan.attributes(&name, attrs.as_str(), attrs.start());
        let self_closed = attrs.as_str().trim_end().ends_with('/');
        let Some(raw) = RAW.iter().position(|(raw_name, _)| *raw_name == name) else {
            continue;
        };
        if self_closed || unclosed[raw] {
            continue;
        }
        let Some(end) = RAW[raw].1.find_at(&code, whole.end()) else {
            // No closing tag here means none further on: do not search again for each opener.
            unclosed[raw] = true;
            continue;
        };
        if name == "style" {
            scan.style.push(Span {
                start: whole.end(),
                end: end.start(),
            });
        }
        scan.hidden.push(Span {
            start: whole.end(),
            end: end.end(),
        });
        pos = end.end();
    }

    let mut text = code.as_bytes().to_vec();
    blank(
        &mut text,
        Span {
            start: 0,
            end: doc.len(),
        },
    );
    scan.copy_prose(code.as_bytes(), &mut text);
    mask_quotes(&mut text);
    mask_urls(&mut text);
    View {
        text: into_string(text),
        code,
        style: scan.style,
        classes: scan.classes,
        dash_skip: Vec::new(),
        waivers,
    }
}

/// What the tag scan finds: ranges that are not prose (`hidden`, in document order), attribute
/// values that are (`prose`), and the style and class spans.
#[derive(Default)]
struct Scan {
    hidden: Vec<Span>,
    prose: Vec<Span>,
    style: Vec<Span>,
    classes: Vec<Span>,
}

impl Scan {
    /// Reads the attributes of one start tag; `base` is the offset of `attrs` in the document.
    fn attributes(&mut self, tag: &str, attrs: &str, base: usize) {
        let (mut meta_name, mut meta_content) = (None, None);
        for caps in ATTR.captures_iter(attrs) {
            let (Some(name), Some(value)) = (
                caps.get(1),
                caps.get(2).or_else(|| caps.get(3)).or_else(|| caps.get(4)),
            ) else {
                continue;
            };
            let place = Span {
                start: base + value.start(),
                end: base + value.end(),
            };
            let name = name.as_str().to_ascii_lowercase();
            match name.as_str() {
                "class" => self.classes.push(place),
                "style" => self.style.push(place),
                n if PROSE_ATTRS.contains(&n) => self.prose.push(place),
                "name" | "property" if tag == "meta" => {
                    meta_name = Some(value.as_str().to_ascii_lowercase());
                }
                "content" if tag == "meta" => meta_content = Some(place),
                _ => {}
            }
        }
        if let (Some(name), Some(content)) = (meta_name, meta_content)
            && matches!(name.as_str(), "description" | "og:title" | "og:description")
        {
            self.prose.push(content);
        }
    }

    /// Copies the prose from `code` into `text`: everything outside the hidden ranges, and the
    /// prose attribute values.
    fn copy_prose(&self, code: &[u8], text: &mut [u8]) {
        let mut from = 0;
        for hidden in &self.hidden {
            keep(
                code,
                text,
                Span {
                    start: from,
                    end: hidden.start,
                },
            );
            from = hidden.end;
        }
        keep(
            code,
            text,
            Span {
                start: from,
                end: code.len(),
            },
        );
        for value in &self.prose {
            keep(code, text, *value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::css::support::{assert_aligned, runs, slices};
    use super::*;
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
    fn text_outside_tags_is_text() {
        let doc = "<!DOCTYPE html>\n<h1>Title</h1>\n<p>Hello <b>big</b> world, a < b</p>\n";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["Title", "Hello", "big", "world, a < b"]);
        assert_eq!(
            v.code, doc,
            "code keeps everything but comments and regions"
        );
    }

    #[test]
    fn script_style_pre_code_and_comments_are_not_text() {
        let doc = "<p>keep one</p>\n\
            <script src=\"a.js\" async></script>\n\
            <script>var s = \"delve\";</script>\n\
            <STYLE>.a { color: red }</STYLE>\n\
            <pre>pre text</pre>\n\
            <p>a <code>inline code</code> b</p>\n\
            <!-- comment\n   text -->\n\
            <pre><code>nested</code></pre>\n\
            <codex>kept</codex>\n\
            <p>keep two</p>";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["keep one", "a", "b", "kept", "keep two"]);
        assert!(
            v.code.contains("var s = \"delve\";"),
            "scripts stay in code"
        );
        assert!(!v.code.contains("comment"), "comments leave code");
    }

    #[test]
    fn listed_attributes_and_meta_descriptions_are_text() {
        let doc = "<img alt=\"A cat\" src=\"cat.png\" class=\"hero\" data-title=\"not prose\">\n\
            <input placeholder='Your name' aria-label=\"Search box\">\n\
            <option label=\"Pick one\">x</option>\n\
            <a title=\"Hover tip\" href=\"/a\">go</a>\n\
            <meta name=\"description\" content=\"A short page summary\">\n\
            <meta content=\"Swapped order\" name=\"Description\">\n\
            <meta property=\"og:title\" content=\"Open Graph title\">\n\
            <meta property=\"og:description\" content=\"Open Graph words\">\n\
            <meta name=\"viewport\" content=\"width=device-width\">\n\
            <meta name=\"author\" content=\"Somebody\">\n\
            <div data-x=\"title='fake'\">y</div>\n\
            <p title=\"a > b\">z</p>";
        let v = checked(doc);
        assert_eq!(
            runs(&v.text),
            [
                "A cat",
                "Your name",
                "Search box",
                "Pick one",
                "x",
                "Hover tip",
                "go",
                "A short page summary",
                "Swapped order",
                "Open Graph title",
                "Open Graph words",
                "y",
                "a > b",
                "z",
            ]
        );
    }

    #[test]
    fn attributes_inside_raw_elements_are_not_text() {
        let doc = "<pre title=\"Own title\">\n<img alt=\"example alt\">\n</pre>\n<p>after</p>";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["Own title", "after"]);
    }

    #[test]
    fn quotes_and_urls_are_masked_in_text() {
        let doc = "<p>He said \"delve deep\" and see https://x.example/a-b now</p>\n\
            <img alt=\"see http://y.example/c\">";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["He said \"", "\" and see", "now", "see"]);
    }

    #[test]
    fn multibyte_text_keeps_offsets() {
        let doc = "<p title=\"Zoë\">café “délve” — ok</p>\n<!-- naïve -->\n<b>ü</b>";
        let v = checked(doc);
        assert_eq!(runs(&v.text), ["Zoë", "café “", "” — ok", "ü"]);
    }

    #[test]
    fn unclosed_raw_elements_are_searched_once() {
        let started = std::time::Instant::now();
        let doc = "<code>x ".repeat(15_000);
        let v = checked(&doc);
        // No closing tag: the openers are plain tags and the words between them are text.
        assert_eq!(runs(&v.text).len(), 15_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn style_blocks_and_style_attributes_are_style_spans() {
        let doc = "<style>.a { color: red }</style>\n\
            <div style=\"color: blue\">x</div>\n\
            <p STYLE='margin: 0'>y</p>\n\
            <style media=\"print\">.b { top: 0 }</style>\n\
            <!-- <style>.hidden { top: 1px }</style> -->\n\
            <pre>\n<style>.example { top: 2px }</style>\n</pre>";
        let v = checked(doc);
        assert_eq!(
            slices(doc, &v.style),
            [
                ".a { color: red }",
                "color: blue",
                "margin: 0",
                ".b { top: 0 }"
            ]
        );
    }

    #[test]
    fn class_values_are_class_spans() {
        let doc = "<div class=\"a b\" id=x>\n\
            <p class='c'></p>\n\
            <span class=d></span>\n\
            <b data-class=\"no\" classy=\"no\"></b>\n\
            <pre class=\"p1\"><code class=\"language-js\">x</code></pre>\n\
            <!-- <i class=\"hidden\"></i> -->\n\
            </div>";
        let v = checked(doc);
        assert_eq!(slices(doc, &v.classes), ["a b", "c", "d", "p1"]);
    }

    #[test]
    fn comment_waivers_are_kept() {
        let doc = "<p>x</p> <!-- slop-ok: text.ai-vocab named by the user -->\n\
            <!-- slop-ok: text.hype-cta short -->\n\
            <!-- slop-ignore-start quoted -->\n<p>delve</p>\n<!-- slop-ignore-end -->\n\
            <p>after</p>\n";
        let v = checked(doc);
        assert_eq!(
            v.waivers,
            [
                waiver(1, Some("text.ai-vocab"), "named by the user", true),
                waiver(2, Some("text.hype-cta"), "short", false),
                waiver(3, None, "quoted", true),
            ]
        );
        assert_eq!(runs(&v.text), ["x", "after"], "the region is not text");
        assert!(!v.code.contains("delve"), "the region is not code");
        assert!(!v.code.contains("slop-ok"), "the comment is not code");
    }
}
