//! Finds the test functions and `mod` declarations in one Rust source file, without compiling it.
//!
//! `ledger check` uses this as its static layer: a `ported` row must name a `#[test]` that is not
//! `#[ignore]`d and not behind a `cfg` other than `test` or `feature = …` (a platform `cfg` would
//! hide it on Linux or macOS), in a file that `mod` declarations reach from a test target root.
//! The binding check is the `cargo test -- --list` output (design §13.3); this layer adds what one
//! Linux listing cannot see. The scanner skips comments, strings and character literals, follows
//! `mod name { … }` nesting, and ignores test functions inside a fn body or a `macro_rules!`.

/// One test function: its path inside the file (`mod` names, then the function name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    pub path: Vec<String>,
    /// `#[ignore]`, or a `cfg_attr` that can add one.
    pub ignored: bool,
    /// Behind a `cfg` other than `test`/`feature` (on the fn, an enclosing module or the file).
    pub gated: bool,
}

/// An out-of-line `mod name;` declaration: inline `mod` names, then its own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDecl {
    pub path: Vec<String>,
    /// Behind a gating `cfg` (on the declaration or an enclosing inline module).
    pub gated: bool,
    /// Has a `#[path = …]` attribute (its file is not where the module rules put it).
    pub has_path: bool,
}

/// What one file declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub tests: Vec<TestFn>,
    pub mods: Vec<ModDecl>,
    /// A gating `#![cfg(…)]` at the top of the file.
    pub file_gated: bool,
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

/// Words a `cfg` may use without gating the item: `test`, features, and `all(…)` of them.
const NEUTRAL_CFG_WORDS: &[&str] = &["cfg", "test", "feature", "all"];

/// A `cfg` that can switch the item off in a `cargo test` run (on Linux, macOS, or both): anything
/// but `test` and features, and any `cfg_attr` that names a platform or adds a `cfg`.
fn is_gating_attr(text: &str) -> bool {
    match attr_name(text) {
        "cfg" => words(text).any(|w| !NEUTRAL_CFG_WORDS.contains(&w)),
        "cfg_attr" => words(text).any(|w| PLATFORM_WORDS.contains(&w) || w == "cfg"),
        _ => false,
    }
}

/// Scans `src`.
pub fn scan(src: &str) -> Scan {
    let toks = tokens(src);
    let mut out = Scan::default();
    let mut pending: Vec<String> = Vec::new();
    // (name, the brace depth inside the module, gated)
    let mut mods: Vec<(String, usize, bool)> = Vec::new();
    // Brace depths of fn and macro_rules! bodies: nothing inside them is a test item.
    let mut bodies: Vec<usize> = Vec::new();
    let mut await_body = false;
    let mut depth = 0usize;
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
                                _ if depth == 0 => out.file_gated = true,
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
                if await_body {
                    bodies.push(depth);
                    await_body = false;
                }
                pending.clear();
                i += 1;
            }
            Tok::Punct('}') => {
                if mods.last().is_some_and(|m| m.1 == depth) {
                    mods.pop();
                }
                if bodies.last() == Some(&depth) {
                    bodies.pop();
                }
                depth = depth.saturating_sub(1);
                pending.clear();
                // `struct S { f: fn() }`: a fn pointer type, not a fn with a body.
                await_body = false;
                i += 1;
            }
            Tok::Punct(';') => {
                pending.clear();
                await_body = false;
                i += 1;
            }
            Tok::Ident(w) if w == "macro_rules" => {
                await_body = true;
                pending.clear();
                i += 1;
            }
            Tok::Ident(w) if w == "mod" && bodies.is_empty() => {
                let gated = mods.iter().any(|m| m.2) || pending.iter().any(|a| is_gating_attr(a));
                match (toks.get(i + 1), toks.get(i + 2)) {
                    (Some(Tok::Ident(name)), Some(Tok::Punct('{'))) => {
                        depth += 1;
                        mods.push((name.clone(), depth, gated));
                        i += 3;
                    }
                    (Some(Tok::Ident(name)), Some(Tok::Punct(';'))) => {
                        let mut path: Vec<String> = mods.iter().map(|m| m.0.clone()).collect();
                        path.push(name.clone());
                        out.mods.push(ModDecl {
                            path,
                            gated,
                            has_path: pending.iter().any(|a| attr_name(a) == "path"),
                        });
                        i += 3;
                    }
                    _ => i += 1,
                }
                pending.clear();
            }
            Tok::Ident(w) if w == "fn" => {
                if bodies.is_empty()
                    && let Some(Tok::Ident(name)) = toks.get(i + 1)
                    && pending.iter().any(|a| is_test_attr(a))
                {
                    let mut path: Vec<String> = mods.iter().map(|m| m.0.clone()).collect();
                    path.push(name.clone());
                    out.tests.push(TestFn {
                        path,
                        ignored: pending.iter().any(|a| is_ignore_attr(a)),
                        gated: out.file_gated
                            || mods.iter().any(|m| m.2)
                            || pending.iter().any(|a| is_gating_attr(a)),
                    });
                }
                await_body = true;
                pending.clear();
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<(String, bool, bool)> {
        scan(src)
            .tests
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
                ("tests::ignored_on_macos".into(), true, true),
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
        assert!(scan(src).file_gated);
    }

    #[test]
    fn test_and_feature_cfgs_do_not_gate_but_every_other_cfg_does() {
        for neutral in [
            "test",
            "feature = \"test-faults\"",
            "all(test, feature = \"x\")",
        ] {
            let src = format!("#[cfg({neutral})]\n#[test]\nfn a() {{}}\n");
            assert_eq!(names(&src), vec![("a".into(), false, false)], "{neutral}");
        }
        for gating in [
            "#[cfg(any())]",
            "#[cfg(not(test))]",
            "#[cfg(unix)]",
            "#[cfg(miri)]",
            "#[cfg_attr(target_os = \"linux\", cfg(any()))]",
            "#[cfg_attr(test, cfg(any()))]",
        ] {
            let src = format!("{gating}\n#[test]\nfn a() {{}}\n");
            assert_eq!(names(&src), vec![("a".into(), false, true)], "{gating}");
        }
    }

    #[test]
    fn tests_inside_fn_bodies_and_macro_rules_are_not_test_items() {
        let src = r#"
            fn outer() {
                #[test]
                fn hidden() {}
                mod not_a_module_decl;
            }
            macro_rules! make {
                () => {
                    #[test]
                    fn templated() {}
                };
            }
            trait T { fn decl(&self); }
            #[test]
            fn visible() {}
        "#;
        assert_eq!(names(src), vec![("visible".into(), false, false)]);
        assert!(scan(src).mods.is_empty());
    }

    #[test]
    fn out_of_line_module_declarations() {
        let src = r#"
            mod plain;
            #[cfg(target_os = "linux")]
            mod linux;
            #[path = "elsewhere.rs"]
            mod moved;
            #[cfg(any())]
            mod outer { mod nested; }
        "#;
        let decl = |path: &[&str], gated, has_path| ModDecl {
            path: path.iter().map(|s| s.to_string()).collect(),
            gated,
            has_path,
        };
        assert_eq!(
            scan(src).mods,
            vec![
                decl(&["plain"], false, false),
                decl(&["linux"], true, false),
                decl(&["moved"], false, true),
                decl(&["outer", "nested"], true, false),
            ]
        );
    }
}
