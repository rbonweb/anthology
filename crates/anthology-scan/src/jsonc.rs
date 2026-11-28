//! JSON with comments (Task 3), the dialect of VS Code and `tsconfig` files.

use anthology_pycompat::json::{Value, loads};

/// Parses `text` as JSON after stripping `//` and `/* */` comments outside strings and trailing
/// commas. A leading byte order mark is ignored. The stripped text keeps every line break of the
/// original, so an error names the right line.
pub fn parse(text: &str) -> Result<Value, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let clean = strip(text)?;
    loads(&clean).map_err(|e| e.py_str(&clean))
}

/// `text` with each comment replaced by spaces (line ends kept) and each trailing comma by a space.
fn strip(text: &str) -> Result<String, String> {
    let src = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    // The last non-space byte written, and where an eligible comma sits in `out`.
    let mut prev = 0u8;
    let mut comma: Option<usize> = None;
    let mut i = 0;
    while i < src.len() {
        let b = src[i];
        match b {
            b'"' => {
                comma = None;
                prev = b;
                out.push(b);
                i += 1;
                while i < src.len() {
                    let c = src[i];
                    out.push(c);
                    i += 1;
                    if c == b'\\' && i < src.len() {
                        out.push(src[i]);
                        i += 1;
                    } else if c == b'"' {
                        break;
                    }
                }
            }
            b'/' if src.get(i + 1) == Some(&b'/') => {
                while i < src.len() && src[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            b'/' if src.get(i + 1) == Some(&b'*') => {
                let Some(len) = text[i + 2..].find("*/") else {
                    return Err("unterminated comment".to_string());
                };
                for &c in &src[i..i + 2 + len + 2] {
                    out.push(if c == b'\n' || c == b'\r' { c } else { b' ' });
                }
                i += 2 + len + 2;
            }
            b' ' | b'\t' | b'\n' | b'\r' => {
                out.push(b);
                i += 1;
            }
            b',' => {
                // A comma right after `[`, `{` or another comma is an error, not a trailing comma.
                comma = if matches!(prev, 0 | b'[' | b'{' | b',') {
                    None
                } else {
                    Some(out.len())
                };
                prev = b;
                out.push(b);
                i += 1;
            }
            b']' | b'}' => {
                if let Some(at) = comma.take() {
                    out[at] = b' ';
                }
                prev = b;
                out.push(b);
                i += 1;
            }
            _ => {
                comma = None;
                prev = b;
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_strips_comments_and_trailing_commas_not_inside_strings() {
        let text = r#"{
            // line comment
            "url": "https://example.invalid/a//b", /* block
            comment */
            "glob": "src/**/*.rs",
            "text": "a /* not a comment */ b, ]",
            "esc": "quote \" // still a string",
            "list": [1, 2, 3,],
            "nested": {"k": [ {"a": 1,}, ], },
        }"#;
        let v = parse(text).unwrap();
        let o = v.as_object().unwrap();
        assert_eq!(
            o.get("url").and_then(|v| v.as_str()),
            Some("https://example.invalid/a//b")
        );
        assert_eq!(o.get("glob").and_then(|v| v.as_str()), Some("src/**/*.rs"));
        assert_eq!(
            o.get("text").and_then(|v| v.as_str()),
            Some("a /* not a comment */ b, ]")
        );
        assert_eq!(
            o.get("esc").and_then(|v| v.as_str()),
            Some("quote \" // still a string")
        );
        assert_eq!(o.get("list").and_then(|v| v.as_array()).unwrap().len(), 3);
        let nested = o.get("nested").and_then(|v| v.as_object()).unwrap();
        assert_eq!(nested.get("k").and_then(|v| v.as_array()).unwrap().len(), 1);
    }

    #[test]
    fn a_comma_before_a_comment_then_a_bracket_is_trailing() {
        let v = parse("[1, 2, // last\n]").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        let v = parse("[1, 2, /* last */ ]").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let v = parse("\u{feff}// editor file\n{\"a\": true}").unwrap();
        assert_eq!(
            v.as_object().unwrap().get("a").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn plain_json_still_parses() {
        let v = parse(r#"{"a": [1, "two", null], "b": {"c": false}}"#).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 2);
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        for bad in [
            "",
            "{",
            "{\"a\": }",
            "[1 2]",
            "{\"a\": 1} trailing",
            "/* never closed",
            "\"unterminated",
            ",",
            "[,]",
            "[1,,2]",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn multibyte_text_survives_stripping() {
        let v = parse("{\"name\": \"caf\u{e9} \u{1f600}\" // note \u{e9}\n}").unwrap();
        assert_eq!(
            v.as_object().unwrap().get("name").and_then(|v| v.as_str()),
            Some("caf\u{e9} \u{1f600}")
        );
    }

    #[test]
    fn a_lone_slash_is_not_a_comment() {
        assert!(parse("[1 / 2]").is_err());
    }
}
