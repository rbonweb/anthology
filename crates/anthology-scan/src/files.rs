//! File kinds, decoding and line numbers (Task 2).

use crate::{FileKind, Lang};

/// How much of a file's start decides its kind: a NUL in it means binary.
const HEAD_WINDOW: usize = 8 * 1024;

/// The kind of the file at `path`, from its name and its first bytes `head`.
pub fn classify(path: &str, head: &[u8]) -> FileKind {
    classify_with(path, head, crate::rules::agent::is_instruction(path))
}

/// `classify` with the answer of the agent-instruction test supplied by the caller.
///
/// Order: a known binary magic or a NUL byte (outside UTF-16 text) makes it `Binary`; then an
/// instruction file is `Agent` whatever its extension; then the name and extension decide.
pub(crate) fn classify_with(path: &str, head: &[u8], instruction: bool) -> FileKind {
    let window = &head[..head.len().min(HEAD_WINDOW)];
    if magic(window).is_some() || (window.contains(&0) && !has_utf16_bom(window)) {
        return FileKind::Binary;
    }
    if instruction {
        return FileKind::Agent;
    }
    by_name(path)
}

fn by_name(path: &str) -> FileKind {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if matches!(
        base.as_str(),
        ".npmrc" | "makefile" | "gnumakefile" | "configure"
    ) || base.starts_with(".env")
    {
        return FileKind::Config;
    }
    // A name that only starts with a dot (`.gitmodules`) has no extension.
    let ext = match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => return FileKind::Data,
    };
    match ext {
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => FileKind::Code(Lang::Js),
        "py" | "pyw" | "pyi" | "pyx" | "pth" => FileKind::Code(Lang::Py),
        "sh" | "bash" | "zsh" | "ksh" => FileKind::Code(Lang::Sh),
        "ps1" | "psm1" => FileKind::Code(Lang::Ps),
        "bat" | "cmd" => FileKind::Code(Lang::Bat),
        "rb" => FileKind::Code(Lang::Rb),
        "php" => FileKind::Code(Lang::Php),
        "pl" | "pm" => FileKind::Code(Lang::Pl),
        "go" => FileKind::Code(Lang::Go),
        "rs" => FileKind::Code(Lang::Rs),
        "java" | "kt" | "kts" | "scala" | "groovy" | "gradle" => FileKind::Code(Lang::Jvm),
        "cs" => FileKind::Code(Lang::Cs),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" => FileKind::Code(Lang::C),
        "lua" => FileKind::Code(Lang::Lua),
        "swift" => FileKind::Code(Lang::Swift),
        "md" | "markdown" | "mdx" | "rst" | "txt" | "adoc" => FileKind::Doc,
        "json" | "json5" | "jsonc" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "mk"
        | "m4" => FileKind::Config,
        _ => FileKind::Data,
    }
}

fn has_utf16_bom(b: &[u8]) -> bool {
    b.starts_with(&[0xff, 0xfe]) || b.starts_with(&[0xfe, 0xff])
}

/// `b` as text: UTF-8 (optional BOM) or UTF-16 with a BOM; `None` for anything else.
pub fn decode(b: &[u8]) -> Option<String> {
    if let Some(rest) = b.strip_prefix(&[0xff, 0xfe]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = b.strip_prefix(&[0xfe, 0xff]) {
        return utf16(rest, u16::from_be_bytes);
    }
    let b = b.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(b);
    std::str::from_utf8(b).ok().map(str::to_string)
}

fn utf16(b: &[u8], unit: fn([u8; 2]) -> u16) -> Option<String> {
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let units = b.as_chunks::<2>().0.iter().copied().map(unit);
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .ok()
}

/// `b` without a character cut off at its end, for a file read only up to a cap: the half of a
/// UTF-8 sequence, or the odd byte of UTF-16 text. Bytes that are invalid in the middle stay.
pub(crate) fn trim_incomplete(b: &[u8]) -> &[u8] {
    if has_utf16_bom(b) {
        return &b[..b.len() & !1];
    }
    match std::str::from_utf8(b) {
        Err(e) if e.error_len().is_none() => &b[..e.valid_up_to()],
        _ => b,
    }
}

/// The 1-based line of byte offset `at` in `t`; CRLF counts as one line break, so a line is what
/// `str::lines` calls one.
pub fn line_of(t: &str, at: usize) -> usize {
    let end = at.min(t.len());
    1 + t.as_bytes()[..end].iter().filter(|&&c| c == b'\n').count()
}

/// The name of the executable or archive format `b` starts with, if any.
///
/// The names are `ELF`, `Mach-O`, `PE`, `WebAssembly` (executables) and `zip`, `gzip`, `xz`, `7z`
/// (archives). `PE` needs the `PE\0\0` header the DOS header points to, so it needs the first
/// kilobyte or so; a bare `MZ` is not enough.
pub fn magic(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x7fELF") {
        return Some("ELF");
    }
    if [
        &[0xfe, 0xed, 0xfa, 0xce],
        &[0xfe, 0xed, 0xfa, 0xcf],
        &[0xce, 0xfa, 0xed, 0xfe],
        &[0xcf, 0xfa, 0xed, 0xfe],
    ]
    .iter()
    .any(|m| b.starts_with(*m))
    {
        return Some("Mach-O");
    }
    if is_pe(b) {
        return Some("PE");
    }
    if b.starts_with(b"\0asm") {
        return Some("WebAssembly");
    }
    if b.starts_with(b"PK\x03\x04") {
        return Some("zip");
    }
    if b.starts_with(&[0x1f, 0x8b]) {
        return Some("gzip");
    }
    if b.starts_with(b"\xfd7zXZ") {
        return Some("xz");
    }
    if b.starts_with(b"7z\xbc\xaf") {
        return Some("7z");
    }
    None
}

/// `MZ`, and at the offset in the field at 0x3c the bytes `PE\0\0`.
fn is_pe(b: &[u8]) -> bool {
    if !b.starts_with(b"MZ") {
        return false;
    }
    let Some(field) = b.get(0x3c..0x40) else {
        return false;
    };
    let offset = u32::from_le_bytes([field[0], field[1], field[2], field[3]]);
    let Ok(start) = usize::try_from(offset) else {
        return false;
    };
    start
        .checked_add(4)
        .and_then(|end| b.get(start..end))
        .is_some_and(|sig| sig == b"PE\0\0")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lang;

    fn kind(path: &str) -> FileKind {
        classify_with(path, b"hello\n", false)
    }

    #[test]
    fn code_by_extension() {
        let cases = [
            ("a.js", Lang::Js),
            ("a.mjs", Lang::Js),
            ("a.cjs", Lang::Js),
            ("a.jsx", Lang::Js),
            ("a.ts", Lang::Js),
            ("a.tsx", Lang::Js),
            ("a.py", Lang::Py),
            ("a.pyw", Lang::Py),
            ("a.pyi", Lang::Py),
            ("a.pyx", Lang::Py),
            ("evil.pth", Lang::Py),
            ("a.sh", Lang::Sh),
            ("a.bash", Lang::Sh),
            ("a.zsh", Lang::Sh),
            ("a.ksh", Lang::Sh),
            ("a.ps1", Lang::Ps),
            ("a.psm1", Lang::Ps),
            ("a.bat", Lang::Bat),
            ("a.cmd", Lang::Bat),
            ("a.rb", Lang::Rb),
            ("a.php", Lang::Php),
            ("a.pl", Lang::Pl),
            ("a.go", Lang::Go),
            ("a.rs", Lang::Rs),
            ("A.JAVA", Lang::Jvm),
            ("a.kt", Lang::Jvm),
            ("a.cs", Lang::Cs),
            ("a.c", Lang::C),
            ("a.cpp", Lang::C),
            ("a.h", Lang::C),
            ("a.lua", Lang::Lua),
            ("a.swift", Lang::Swift),
            ("deep/dir/a.PY", Lang::Py),
        ];
        for (path, lang) in cases {
            assert_eq!(kind(path), FileKind::Code(lang), "{path}");
        }
    }

    #[test]
    fn docs_configs_and_data() {
        for p in [
            "README.md",
            "a.markdown",
            "a.mdx",
            "a.rst",
            "a.txt",
            "a.adoc",
        ] {
            assert_eq!(kind(p), FileKind::Doc, "{p}");
        }
        for p in [
            "package.json",
            "a.json5",
            "a.jsonc",
            "Cargo.toml",
            "a.yaml",
            "ci.yml",
            "a.ini",
            "a.cfg",
            "a.conf",
            ".npmrc",
            ".env",
            ".env.local",
            "sub/.env.production",
            "Makefile",
            "makefile",
            "GNUmakefile",
            "rules.mk",
            "aclocal.m4",
            "configure",
            "deep/dir/configure",
        ] {
            assert_eq!(kind(p), FileKind::Config, "{p}");
        }
        for p in [
            "a.woff2",
            "a.png",
            "LICENSE",
            "a.lock",
            ".gitmodules",
            ".husky/pre-commit",
            "a",
        ] {
            assert_eq!(kind(p), FileKind::Data, "{p}");
        }
    }

    #[test]
    fn an_instruction_file_is_agent_before_its_extension_counts() {
        assert_eq!(
            classify_with("CLAUDE.md", b"hello\n", true),
            FileKind::Agent
        );
        assert_eq!(
            classify_with(".cursor/rules/a.mdc", b"hello\n", true),
            FileKind::Agent
        );
        assert_eq!(classify_with("CLAUDE.md", b"hello\n", false), FileKind::Doc);
    }

    #[test]
    fn classify_asks_the_agent_rule() {
        for p in [
            "CLAUDE.md",
            "docs/a.md",
            ".claude/commands/x.md",
            ".cursorrules",
            "a.js",
        ] {
            assert_eq!(
                classify(p, b"hello\n") == FileKind::Agent,
                crate::rules::agent::is_instruction(p),
                "{p}"
            );
        }
    }

    #[test]
    fn nul_or_a_known_magic_is_binary() {
        assert_eq!(classify_with("a.txt", b"ab\0cd", false), FileKind::Binary);
        assert_eq!(
            classify_with("a.js", b"\x7fELF\x02\x01", false),
            FileKind::Binary
        );
        assert_eq!(
            classify_with("a.png", b"\x89PNG\r\n\x1a\n\0\0", false),
            FileKind::Binary
        );
        assert_eq!(
            classify_with("a.woff2", b"function(){}", false),
            FileKind::Data
        );
    }

    #[test]
    fn magic_ranks_above_an_instruction_name() {
        assert_eq!(
            classify_with("CLAUDE.md", b"\x7fELF\x02\x01", true),
            FileKind::Binary
        );
    }

    #[test]
    fn utf16_with_a_bom_is_not_binary() {
        let mut le = vec![0xff, 0xfe];
        le.extend("hi\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(classify_with("a.js", &le, false), FileKind::Code(Lang::Js));
        let mut be = vec![0xfe, 0xff];
        be.extend("hi\n".encode_utf16().flat_map(|u| u.to_be_bytes()));
        assert_eq!(classify_with("a.txt", &be, false), FileKind::Doc);
    }

    #[test]
    fn nul_after_the_head_window_does_not_make_binary() {
        let mut b = vec![b'a'; 9000];
        b[8500] = 0;
        assert_eq!(classify_with("a.txt", &b, false), FileKind::Doc);
    }

    #[test]
    fn magic_names_the_format() {
        assert_eq!(magic(b"\x7fELF\x02"), Some("ELF"));
        for m in [
            [0xfe, 0xed, 0xfa, 0xce],
            [0xfe, 0xed, 0xfa, 0xcf],
            [0xce, 0xfa, 0xed, 0xfe],
            [0xcf, 0xfa, 0xed, 0xfe],
        ] {
            assert_eq!(magic(&m), Some("Mach-O"), "{m:x?}");
        }
        assert_eq!(magic(b"\0asm\x01\0\0\0"), Some("WebAssembly"));
        assert_eq!(magic(b"PK\x03\x04rest"), Some("zip"));
        assert_eq!(magic(b"\x1f\x8b\x08"), Some("gzip"));
        assert_eq!(magic(b"\xfd7zXZ\0"), Some("xz"));
        assert_eq!(magic(b"7z\xbc\xaf\x27\x1c"), Some("7z"));
        assert_eq!(magic(b"plain text"), None);
        assert_eq!(magic(b""), None);
    }

    fn pe(e_lfanew: usize, sig: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3c..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
        b[e_lfanew..e_lfanew + sig.len()].copy_from_slice(sig);
        b
    }

    #[test]
    fn magic_wants_the_pe_header_behind_mz() {
        assert_eq!(magic(&pe(0x80, b"PE\0\0")), Some("PE"));
        assert_eq!(magic(&pe(0x80, b"NE\0\0")), None);
        assert_eq!(magic(b"MZ is only two letters"), None);
        let mut far = pe(0x80, b"PE\0\0");
        far[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert_eq!(magic(&far), None);
    }

    #[test]
    fn decodes_utf8_with_and_without_a_bom() {
        assert_eq!(decode(b"abc").as_deref(), Some("abc"));
        assert_eq!(decode(b"\xef\xbb\xbfabc").as_deref(), Some("abc"));
        assert_eq!(decode("héllo".as_bytes()).as_deref(), Some("héllo"));
        assert_eq!(decode(b""), Some(String::new()));
    }

    #[test]
    fn invalid_utf8_does_not_decode() {
        assert_eq!(decode(b"ab\xffcd"), None);
        assert_eq!(decode(b"\xc3"), None);
    }

    #[test]
    fn decodes_utf16_both_ways() {
        let text = "a\r\nb é\n";
        let mut le = vec![0xff, 0xfe];
        le.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let mut be = vec![0xfe, 0xff];
        be.extend(text.encode_utf16().flat_map(|u| u.to_be_bytes()));
        assert_eq!(decode(&le).as_deref(), Some(text));
        assert_eq!(decode(&be).as_deref(), Some(text));
    }

    #[test]
    fn broken_utf16_does_not_decode() {
        // A lone high surrogate, and an odd byte count.
        assert_eq!(decode(&[0xff, 0xfe, 0x00, 0xd8]), None);
        assert_eq!(decode(&[0xff, 0xfe, 0x61]), None);
    }

    #[test]
    fn decodes_utf16_bom_and_counts_crlf() {
        let text = "one\r\ntwo\r\nthree\r\n";
        let mut le = vec![0xff, 0xfe];
        le.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let got = decode(&le).unwrap();
        assert_eq!(got, text);
        assert_eq!(line_of(&got, got.find("three").unwrap()), 3);
    }

    #[test]
    fn line_of_counts_crlf_as_one_break() {
        let t = "a\r\nb\nc\r\n\r\nd";
        assert_eq!(line_of(t, 0), 1);
        assert_eq!(line_of(t, t.find('b').unwrap()), 2);
        assert_eq!(line_of(t, t.find('c').unwrap()), 3);
        assert_eq!(line_of(t, t.find('d').unwrap()), 5);
        assert_eq!(line_of(t, t.len()), 5);
        assert_eq!(line_of(t, usize::MAX), 5);
        assert_eq!(line_of("", 0), 1);
    }

    #[test]
    fn trim_incomplete_drops_a_cut_character_only() {
        let s = "aé".as_bytes();
        assert_eq!(trim_incomplete(&s[..2]), b"a");
        assert_eq!(trim_incomplete(s), s);
        // Invalid in the middle stays: it is not a cut.
        assert_eq!(trim_incomplete(b"a\xffb"), b"a\xffb");
        let mut le = vec![0xff, 0xfe, 0x61, 0x00, 0x62];
        assert_eq!(trim_incomplete(&le), &[0xff, 0xfe, 0x61, 0x00]);
        le.pop();
        assert_eq!(trim_incomplete(&le), &[0xff, 0xfe, 0x61, 0x00]);
    }
}
