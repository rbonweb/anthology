//! The JSX and TSX view. Only JSX text between tags and the string values of a few attributes are
//! `text`; every other string literal is code. Comments and ignore regions are not code.

use std::sync::LazyLock;

use regex::Regex;

use super::View;
use super::common::{blank, into_string, mask_quotes, mask_urls};
use super::css::{comment_spans, keep, masked_code, span_of};
use crate::Span;

/// A `className` value: double-quoted, single-quoted, or a brace expression without nested
/// braces (groups 1 to 3).
static CLASS_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bclassName\s*=\s*(?:"([^"]*)"|'([^']*)'|\{([^{}]*)\})"#)
        .expect("className regex")
});

/// Text after a `>` up to the next `<` or `{` (group 1).
static AFTER_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r">([^<>{}]+)[<{]").expect("jsx text regex"));

/// Text after a `}` up to the next `<` (group 1).
static AFTER_BRACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\}([^<>{}]+)<").expect("jsx text regex"));

/// A string value of an attribute that holds prose (groups 1 and 2: double and single quotes).
static PROSE_ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:^|\s)(?:alt|title|placeholder|aria-label|label)\s*=\s*(?:"([^"]*)"|'([^']*)')"#,
    )
    .expect("prose attribute regex")
});

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
    let segments = AFTER_TAG
        .captures_iter(&code)
        .chain(AFTER_BRACE.captures_iter(&code))
        .filter_map(|caps| caps.get(1))
        .filter(|segment| is_text(segment.as_str()));
    let attributes = PROSE_ATTR
        .captures_iter(&code)
        .filter_map(|caps| caps.get(1).or_else(|| caps.get(2)));
    for found in segments.chain(attributes) {
        keep(code.as_bytes(), &mut text, span_of(found));
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
