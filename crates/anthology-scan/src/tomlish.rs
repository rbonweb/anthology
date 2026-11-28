//! A TOML detector, not a parser (Task 3): unknown syntax yields no entry, never an error.

/// One `key = value` line with the table header above it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kv {
    pub table: String,
    pub key: String,
    pub value: String,
    pub line: usize,
}

/// The entries of `text`, in order. A `[table]` or `[[table]]` header sets the table of the
/// entries below it (the name without brackets, as written). A key is kept as written, so a dotted
/// key stays `a.b`; a key that is one quoted string loses its quotes. A value is the text after
/// the `=` without a trailing comment. A multi-line string gives no entry and its lines are
/// skipped; a multi-line array or inline table is one entry, its lines joined by single spaces.
/// Anything this does not read gives no entry.
pub fn entries(text: &str) -> Vec<Kv> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    let mut table = String::new();
    let mut lines = text.lines().enumerate();
    while let Some((index, raw)) = lines.next() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if let Some(name) = header(line) {
                table = name;
            }
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(delim) = opens_multiline_string(rest) {
            // Skip to the line that closes the string (or to the end).
            for (_, next) in lines.by_ref() {
                if next.contains(delim) {
                    break;
                }
            }
            continue;
        }
        let (value, open) = strip_comment(rest);
        let mut level = bracket_depth(value);
        let mut joined = value.to_string();
        if level > 0 {
            for (_, next) in lines.by_ref() {
                let (piece, _) = strip_comment(next.trim());
                if !piece.is_empty() {
                    joined.push(' ');
                    joined.push_str(piece);
                }
                level += bracket_depth(piece);
                if level <= 0 {
                    break;
                }
            }
            if level > 0 {
                continue;
            }
        } else if value.is_empty() || open {
            continue;
        }
        out.push(Kv {
            table: table.clone(),
            key,
            value: joined,
            line: index + 1,
        });
    }
    out
}

/// The quote of a `"""` or `'''` string that starts `value` and does not end on the same line.
fn opens_multiline_string(value: &str) -> Option<&'static str> {
    let delim = ["\"\"\"", "'''"]
        .into_iter()
        .find(|d| value.starts_with(*d))?;
    (!value[3..].contains(delim)).then_some(delim)
}

/// The name in a `[name]` or `[[name]]` line, comment removed; `None` when the line is not one.
fn header(line: &str) -> Option<String> {
    let (line, open) = strip_comment(line);
    if open {
        return None;
    }
    let inner = match line.strip_prefix("[[") {
        Some(rest) => rest.strip_suffix("]]")?,
        None => line.strip_prefix('[')?.strip_suffix(']')?,
    };
    let inner = inner.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// Splits `key = value` at the first `=` outside quotes. The key must be bare words, dots and
/// quoted parts; a key that is one quoted string loses its quotes.
fn split_key(line: &str) -> Option<(String, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut eq = None;
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if q == '"' && c == '\\' && !escaped {
                    escaped = true;
                    continue;
                }
                if c == q && !escaped {
                    quote = None;
                }
                escaped = false;
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '=' => {
                    eq = Some(i);
                    break;
                }
                _ => {}
            },
        }
    }
    let eq = eq?;
    let key = line[..eq].trim();
    if key.is_empty() || !key_is_well_formed(key) {
        return None;
    }
    let whole = ['"', '\'']
        .into_iter()
        .find(|q| key.len() >= 2 && key.starts_with(*q) && key.ends_with(*q))
        .map(|_| &key[1..key.len() - 1])
        .filter(|inner| !inner.is_empty() && !inner.contains(['"', '\'']));
    Some((whole.unwrap_or(key).to_string(), &line[eq + 1..]))
}

/// Bare key characters, dots and quoted parts only: no space outside quotes.
fn key_is_well_formed(key: &str) -> bool {
    let mut quote: Option<char> = None;
    for c in key.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                c if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') => {}
                _ => return false,
            },
        }
    }
    quote.is_none()
}

/// `s` without a trailing `# comment`, and whether a string is still open at its end.
fn strip_comment(s: &str) -> (&str, bool) {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) => {
                if q == '"' && c == '\\' && !escaped {
                    escaped = true;
                    continue;
                }
                if c == q && !escaped {
                    quote = None;
                }
                escaped = false;
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' => return (s[..i].trim_end(), false),
                _ => {}
            },
        }
    }
    (s.trim_end(), quote.is_some())
}

/// Opening minus closing brackets and braces outside strings.
fn bracket_depth(s: &str) -> i32 {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut depth = 0;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if q == '"' && c == '\\' && !escaped {
                    escaped = true;
                    continue;
                }
                if c == q && !escaped {
                    quote = None;
                }
                escaped = false;
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(table: &str, key: &str, value: &str, line: usize) -> Kv {
        Kv {
            table: table.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            line,
        }
    }

    #[test]
    fn tomlish_reads_tables_and_dotted_keys() {
        let text = "\
# a comment
name = \"root\"

[package]
name = \"x\" # trailing comment
build = 'build.rs'

[target.'cfg(unix)'.dependencies]
a.b = 1

[[bin]]
path = \"src/main.rs\"

[env]
\"QUOTED\" = \"v\"
[alias]
";
        assert_eq!(
            entries(text),
            vec![
                kv("", "name", "\"root\"", 2),
                kv("package", "name", "\"x\"", 5),
                kv("package", "build", "'build.rs'", 6),
                kv("target.'cfg(unix)'.dependencies", "a.b", "1", 9),
                kv("bin", "path", "\"src/main.rs\"", 12),
                kv("env", "QUOTED", "\"v\"", 15),
            ]
        );
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let got = entries(
            "[git]\nurl = \"https://example.invalid/#frag\" # real comment\nlit = 'a # b'\n",
        );
        assert_eq!(
            got,
            vec![
                kv("git", "url", "\"https://example.invalid/#frag\"", 2),
                kv("git", "lit", "'a # b'", 3),
            ]
        );
    }

    #[test]
    fn multi_line_strings_are_skipped_with_their_lines() {
        let text = "\
[a]
x = \"\"\"
not = \"an entry\"
[not.a.table]
\"\"\"
y = 1
z = '''one line'''
w = '''
also = 'skipped'
'''
v = 2
";
        assert_eq!(
            entries(text),
            vec![
                kv("a", "y", "1", 6),
                kv("a", "z", "'''one line'''", 7),
                kv("a", "v", "2", 11),
            ]
        );
    }

    #[test]
    fn a_multi_line_array_is_one_entry() {
        let text = "[pkg]\nfiles = [\n  \"a\", # first\n  \"b = c\",\n]\nnext = true\n";
        assert_eq!(
            entries(text),
            vec![
                kv("pkg", "files", "[ \"a\", \"b = c\", ]", 2),
                kv("pkg", "next", "true", 6),
            ]
        );
    }

    #[test]
    fn crlf_and_a_byte_order_mark_are_fine() {
        let got = entries("\u{feff}[a]\r\nk = 1\r\n");
        assert_eq!(got, vec![kv("a", "k", "1", 2)]);
    }

    #[test]
    fn unknown_syntax_yields_no_entry_and_no_panic() {
        for junk in [
            "",
            "= 1",
            "key =",
            "a b = 1",
            "just words",
            "[unclosed",
            "[[unclosed]",
            "{ not = toml }",
            "\u{0}\u{1}\u{2}",
            "x = [",
            "x = \"unterminated",
            "=",
            "[]",
        ] {
            let _ = entries(junk);
        }
        assert!(entries("= 1\nkey =\na b = 1\njust words\n{ not = toml }\n").is_empty());
    }

    #[test]
    fn dotted_keys_and_quoted_keys_for_cargo_config() {
        let text = "[build]\nrustc-wrapper = \"/tmp/x\"\n\"rustc\" = \"y\"\n[target.x86_64-unknown-linux-gnu]\nrunner = \"sh run.sh\"\n";
        let got = entries(text);
        assert_eq!(got[0], kv("build", "rustc-wrapper", "\"/tmp/x\"", 2));
        assert_eq!(got[1], kv("build", "rustc", "\"y\"", 3));
        assert_eq!(
            got[2],
            kv(
                "target.x86_64-unknown-linux-gnu",
                "runner",
                "\"sh run.sh\"",
                5
            )
        );
    }
}
