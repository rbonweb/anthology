//! Finds the test functions in one Rust source file, without compiling it.
//!
//! `ledger check` uses this to confirm that a `ported` row names a real `#[test]` that runs on
//! both Linux and macOS: not `#[ignore]`d, and not behind a platform `cfg` (design §13.3). The
//! scanner skips comments, strings and character literals, then follows `mod name { … }` nesting
//! and the attributes in front of each `fn`.

/// One test function: its path inside the file (`mod` names, then the function name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    pub path: Vec<String>,
    /// `#[ignore]`, or a `cfg_attr` that can add one.
    pub ignored: bool,
    /// Behind a platform `cfg` (on the function, an enclosing module or the whole file).
    pub gated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Punct(char),
}

/// Words that make a `cfg` platform-dependent.
const PLATFORM_WORDS: &[&str] = &[
    "target_os",
    "target_family",
    "target_arch",
    "target_env",
    "target_vendor",
    "target_pointer_width",
    "target_endian",
    "unix",
    "windows",
];

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Skips a string body starting at the opening quote `chars[i] == '"'`; `hashes` is the number
/// of `#` of a raw string (`None` for an ordinary string). Returns the index after the end.
fn skip_string(chars: &[char], mut i: usize, hashes: Option<usize>) -> usize {
    i += 1;
    while i < chars.len() {
        match (chars[i], hashes) {
            ('\\', None) => i += 2,
            ('"', None) => return i + 1,
            ('"', Some(n)) => {
                let closes = (1..=n).all(|k| chars.get(i + k) == Some(&'#'));
                if closes {
                    return i + 1 + n;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

fn tokens(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '"' {
            i = skip_string(&chars, i, None);
        } else if c == '\'' {
            // A char literal ('a', '\n', '\u{1F600}') or a lifetime ('a, 'static).
            if next == Some('\\') {
                i += 2;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if chars.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
            }
        } else if c.is_ascii_digit() {
            while i < chars.len() && (is_ident_char(chars[i]) || chars[i] == '.') {
                // `0..10`: a range, not a float.
                if chars[i] == '.' && chars.get(i + 1) == Some(&'.') {
                    break;
                }
                i += 1;
            }
        } else if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let prefix = matches!(word.as_str(), "r" | "br" | "cr" | "b" | "c");
            let raw = matches!(word.as_str(), "r" | "br" | "cr");
            match chars.get(i) {
                Some('"') if prefix => i = skip_string(&chars, i, raw.then_some(0)),
                Some('#') if raw => {
                    let mut n = 0;
                    while chars.get(i + n) == Some(&'#') {
                        n += 1;
                    }
                    if chars.get(i + n) == Some(&'"') {
                        i = skip_string(&chars, i + n, Some(n));
                    } else if word == "r" && chars.get(i + 1).is_some_and(|&c| is_ident_start(c)) {
                        // A raw identifier: r#name.
                        let s = i + 1;
                        i = s;
                        while i < chars.len() && is_ident_char(chars[i]) {
                            i += 1;
                        }
                        out.push(Tok::Ident(chars[s..i].iter().collect()));
                    } else {
                        out.push(Tok::Ident(word));
                    }
                }
                Some('\'') if word == "b" => {
                    // A byte literal: b'a', b'\n'.
                    i += 1;
                    if chars.get(i + 1) == Some(&'\\') {
                        i += 2;
                    } else {
                        i += 1;
                    }
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                }
                _ => out.push(Tok::Ident(word)),
            }
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

/// The text of the attribute whose `[` is at `toks[open]`, and the index after its `]`.
fn attribute(toks: &[Tok], open: usize) -> (String, usize) {
    let mut depth = 0usize;
    let mut text = String::new();
    let mut i = open;
    let mut last_ident = false;
    while i < toks.len() {
        match &toks[i] {
            Tok::Punct('[') => {
                depth += 1;
                if depth > 1 {
                    text.push('[');
                }
                last_ident = false;
            }
            Tok::Punct(']') => {
                depth -= 1;
                if depth == 0 {
                    return (text, i + 1);
                }
                text.push(']');
                last_ident = false;
            }
            Tok::Punct(c) => {
                text.push(*c);
                last_ident = false;
            }
            Tok::Ident(w) => {
                if last_ident {
                    text.push(' ');
                }
                text.push_str(w);
                last_ident = true;
            }
        }
        i += 1;
    }
    (text, i)
}

/// The attribute's path: `tokio::test` for `tokio::test(flavor = …)`.
fn attr_name(text: &str) -> &str {
    let end = text.find(['(', '=', ' ']).unwrap_or(text.len());
    &text[..end]
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !is_ident_char(c))
        .filter(|w| !w.is_empty())
}

fn is_test_attr(text: &str) -> bool {
    let name = attr_name(text);
    name == "test"
        || name.ends_with("::test")
        || name == "rstest"
        || name.ends_with("::rstest")
        || name == "test_case"
        || name.ends_with("::test_case")
}

fn is_ignore_attr(text: &str) -> bool {
    let name = attr_name(text);
    name == "ignore" || (name == "cfg_attr" && words(text).any(|w| w == "ignore"))
}

fn is_gating_attr(text: &str) -> bool {
    attr_name(text) == "cfg" && words(text).any(|w| PLATFORM_WORDS.contains(&w))
}

/// Every test function in `src`.
pub fn scan(src: &str) -> Vec<TestFn> {
    let toks = tokens(src);
    let mut found = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    // (name, the brace depth inside the module, gated)
    let mut mods: Vec<(String, usize, bool)> = Vec::new();
    let mut depth = 0usize;
    let mut file_gated = false;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Punct('#') => {
                let inner = toks.get(i + 1) == Some(&Tok::Punct('!'));
                let open = if inner { i + 2 } else { i + 1 };
                if toks.get(open) == Some(&Tok::Punct('[')) {
                    let (text, after) = attribute(&toks, open);
                    if inner {
                        if is_gating_attr(&text) {
                            match mods.last_mut() {
                                Some(m) if m.1 == depth => m.2 = true,
                                _ if depth == 0 => file_gated = true,
                                _ => {}
                            }
                        }
                    } else {
                        pending.push(text);
                    }
                    i = after;
                    continue;
                }
                i += 1;
            }
            Tok::Punct('{') => {
                depth += 1;
                pending.clear();
                i += 1;
            }
            Tok::Punct('}') => {
                if mods.last().is_some_and(|m| m.1 == depth) {
                    mods.pop();
                }
                depth = depth.saturating_sub(1);
                pending.clear();
                i += 1;
            }
            Tok::Punct(';') => {
                pending.clear();
                i += 1;
            }
            Tok::Ident(w) if w == "mod" => {
                if let (Some(Tok::Ident(name)), Some(Tok::Punct('{'))) =
                    (toks.get(i + 1), toks.get(i + 2))
                {
                    let gated = pending.iter().any(|a| is_gating_attr(a));
                    depth += 1;
                    mods.push((name.clone(), depth, gated));
                    i += 3;
                } else {
                    i += 1;
                }
                pending.clear();
            }
            Tok::Ident(w) if w == "fn" => {
                if let Some(Tok::Ident(name)) = toks.get(i + 1)
                    && pending.iter().any(|a| is_test_attr(a))
                {
                    let mut path: Vec<String> = mods.iter().map(|m| m.0.clone()).collect();
                    path.push(name.clone());
                    found.push(TestFn {
                        path,
                        ignored: pending.iter().any(|a| is_ignore_attr(a)),
                        gated: file_gated
                            || mods.iter().any(|m| m.2)
                            || pending.iter().any(|a| is_gating_attr(a)),
                    });
                }
                pending.clear();
                i += 1;
            }
            _ => i += 1,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<(String, bool, bool)> {
        scan(src)
            .into_iter()
            .map(|t| (t.path.join("::"), t.ignored, t.gated))
            .collect()
    }

    #[test]
    fn finds_tests_in_modules_with_their_attributes() {
        let src = r##"
            //! #[test] fn not_this() {}
            /* #[test] fn nor_this() { /* nested */ } */
            const S: &str = "#[test] fn in_a_string() {}";
            const R: &str = r#"#[test] fn in_a_raw_string() {"#;
            const C: char = '{';
            fn lifetime<'a>(x: &'a str) -> &'a str { x }
            #[test]
            fn top() { let _ = b'}'; }
            #[cfg(test)]
            mod tests {
                use super::*;
                #[test]
                #[ignore = "slow"]
                fn ignored() {}
                #[test]
                #[cfg(target_os = "linux")]
                fn linux_only() {}
                #[test]
                #[cfg_attr(target_os = "macos", ignore)]
                fn ignored_on_macos() {}
                fn helper() {}
                #[tokio::test(flavor = "current_thread")]
                async fn tokio_test() {}
                #[cfg(unix)]
                mod inner {
                    #[test]
                    fn gated_by_module() {}
                }
                #[test]
                pub(crate) fn after_inner() {}
            }
            proptest::proptest! {
                #[test]
                fn property(x in 0..10u8) { let _ = x; }
            }
        "##;
        assert_eq!(
            names(src),
            vec![
                ("top".into(), false, false),
                ("tests::ignored".into(), true, false),
                ("tests::linux_only".into(), false, true),
                ("tests::ignored_on_macos".into(), true, false),
                ("tests::tokio_test".into(), false, false),
                ("tests::inner::gated_by_module".into(), false, true),
                ("tests::after_inner".into(), false, false),
                ("property".into(), false, false),
            ]
        );
    }

    #[test]
    fn an_inner_platform_cfg_gates_the_whole_file() {
        let src = "#![cfg(target_os = \"macos\")]\n#[test]\nfn a() {}\n";
        assert_eq!(names(src), vec![("a".into(), false, true)]);
    }

    #[test]
    fn a_feature_cfg_is_not_platform_gating() {
        let src = "#[cfg(feature = \"test-faults\")]\n#[test]\nfn a() {}\n";
        assert_eq!(names(src), vec![("a".into(), false, false)]);
    }
}
