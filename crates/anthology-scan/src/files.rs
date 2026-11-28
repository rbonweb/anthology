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
/// Order, name first: an instruction file is `Agent` whatever its bytes, unless it has a code
/// extension or a `#!` first line (a script under `.claude/commands` is code, and the orchestrator
/// still lists it as an instruction); a `#!` first line makes any other file `Code`; a code, config
/// or known auto-run name stays `Code` or `Config` even with a NUL byte or a binary magic in it,
/// because the tool that runs it reads it anyway (a stray byte must not turn it into a file the
/// rules skip). Any other name is `Binary` on a known magic or a NUL byte (outside UTF-16 text), and
/// otherwise `Doc` or `Data`.
pub(crate) fn classify_with(path: &str, head: &[u8], instruction: bool) -> FileKind {
    let window = &head[..head.len().min(HEAD_WINDOW)];
    if instruction {
        if let Some(lang) = shebang_lang(window) {
            return FileKind::Code(lang);
        }
        return match by_extension(path) {
            kind @ FileKind::Code(_) => kind,
            _ => FileKind::Agent,
        };
    }
    if let Some(lang) = shebang_lang(window) {
        return FileKind::Code(lang);
    }
    match by_name(path) {
        kind @ (FileKind::Code(_) | FileKind::Config) => kind,
        kind => {
            if magic(window).is_some() || (window.contains(&0) && !has_utf16_bom(window)) {
                FileKind::Binary
            } else {
                kind
            }
        }
    }
}

/// Whether the file is one that runs code when a repo is opened, installed, built or used by git:
/// its name alone says so. These stay text even when they hold a NUL byte, and a flaw in their
/// encoding is HIGH.
pub(crate) fn is_autorun_name(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let base = parts.pop().unwrap_or("");
    let parent = parts.last().copied().unwrap_or("");
    let grand = parts.len().checked_sub(2).map_or("", |i| parts[i]);
    if parts.contains(&".husky") || (parent == "hooks" && grand == ".git") {
        return true;
    }
    if parent == ".git" && matches!(base, "config" | "config.worktree") {
        return true;
    }
    if matches!(
        base,
        "package.json"
            | "makefile"
            | "gnumakefile"
            | "configure"
            | "justfile"
            | "taskfile.yml"
            | "taskfile.yaml"
            | ".envrc"
            | ".pre-commit-config.yaml"
            | ".gitattributes"
            | ".gitmodules"
            | ".npmrc"
            | ".yarnrc"
            | ".yarnrc.yml"
            | "setup.py"
            | "build.rs"
            | ".mcp.json"
            | ".devcontainer.json"
            | "devcontainer.json"
    ) {
        return true;
    }
    if matches!(
        base,
        "tasks.json" | "settings.json" | "settings.local.json" | "mcp.json"
    ) && matches!(parent, ".vscode" | ".cursor" | ".claude")
    {
        return true;
    }
    if (matches!(base, "config.toml") && matches!(parent, ".cargo" | ".codex"))
        || (base == "config" && parent == ".cargo")
    {
        return true;
    }
    matches!(
        base.rsplit_once('.').map(|(_, e)| e),
        Some("code-workspace" | "csproj" | "vbproj" | "fsproj" | "props" | "targets" | "mk" | "m4")
    )
}

/// Whether a flaw in the encoding of the file at `path` is HIGH: the file is JSON that a strict
/// parser reads for what runs (`package.json`, `tasks.json`, `settings.json`, `.mcp.json`,
/// `devcontainer.json`, `*.code-workspace`), or is under `.claude`. A parser can stop at a bad byte
/// where the scan reads on. Any other file (a Makefile, `configure`, `setup.py`, a project file) is
/// read by tools that go on past a bad byte, and the rules ran on its lossy text, so nothing is
/// hidden: its flaw is MEDIUM.
pub(crate) fn is_strict_json_name(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let base = parts.pop().unwrap_or("");
    parts.contains(&".claude")
        || matches!(
            base,
            "package.json"
                | "tasks.json"
                | "settings.json"
                | "settings.local.json"
                | ".mcp.json"
                | "mcp.json"
                | ".devcontainer.json"
                | "devcontainer.json"
        )
        || base.ends_with(".code-workspace")
}

/// The language named by a `#!` first line, whatever the file is called, so `.husky/*`, `.git/hooks/*`
/// and bin scripts count as code. `env` is looked through (`env -S node`); a name that is not known
/// is a shell script.
fn shebang_lang(head: &[u8]) -> Option<Lang> {
    let line = head.strip_prefix(b"#!")?;
    let line = line.split(|&c| c == b'\n').next().unwrap_or(line);
    let line = String::from_utf8_lossy(line);
    let mut words = line.split_whitespace();
    let mut prog = words
        .next()
        .map_or("", |w| w.rsplit('/').next().unwrap_or(w));
    if prog == "env" {
        prog = words
            .find(|w| !w.starts_with('-') && !w.contains('='))
            .map_or("", |w| w.rsplit('/').next().unwrap_or(w));
    }
    Some(lang_of_program(prog))
}

/// The language of the program `prog` (a name or a path), as the file it runs: a name that is not
/// known is a shell script.
pub(crate) fn lang_of_program(prog: &str) -> Lang {
    let prog = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
    let prog = prog.to_ascii_lowercase();
    if prog.starts_with("node") || prog.starts_with("deno") || prog.starts_with("bun") {
        Lang::Js
    } else if prog.starts_with("python") {
        Lang::Py
    } else if prog.starts_with("ruby") {
        Lang::Rb
    } else if prog.starts_with("perl") {
        Lang::Pl
    } else if prog.starts_with("php") {
        Lang::Php
    } else if prog.starts_with("lua") {
        Lang::Lua
    } else if prog.starts_with("pwsh") || prog.starts_with("powershell") {
        Lang::Ps
    } else {
        Lang::Sh
    }
}

/// The kind a file has when a command runs it: code in the language of its extension, else the
/// language of a `#!` first line in `head`, else `fallback` (the interpreter of the command, or a
/// shell script).
pub(crate) fn code_kind_of_run(path: &str, head: &[u8], fallback: Lang) -> FileKind {
    if let Some(lang) = shebang_lang(&head[..head.len().min(HEAD_WINDOW)]) {
        return FileKind::Code(lang);
    }
    match by_extension(path) {
        kind @ FileKind::Code(_) => kind,
        _ => FileKind::Code(fallback),
    }
}

fn by_name(path: &str) -> FileKind {
    match by_extension(path) {
        FileKind::Data if is_autorun_name(path) => FileKind::Config,
        kind => kind,
    }
}

fn by_extension(path: &str) -> FileKind {
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

/// `b` as text with every flaw replaced, so no rule is skipped for one bad byte, and the first flaw
/// as its kind and byte offset: `invalid UTF-8`, `invalid UTF-16` or `NUL byte`. UTF-16 is read
/// when a BOM says so; a UTF-8 BOM is dropped and is not a flaw. A strict reader (a JSON parser, a
/// shell) may stop where this text goes on, which is why a flaw in a file that runs code is itself
/// a finding.
pub(crate) fn decode_lossy(b: &[u8]) -> (String, Option<(&'static str, usize)>) {
    let (text, mut flaw) = if let Some(rest) = b.strip_prefix(&[0xff, 0xfe]) {
        utf16_lossy(rest, u16::from_le_bytes)
    } else if let Some(rest) = b.strip_prefix(&[0xfe, 0xff]) {
        utf16_lossy(rest, u16::from_be_bytes)
    } else {
        let body = b.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(b);
        match std::str::from_utf8(body) {
            Ok(s) => (s.to_string(), None),
            Err(e) => (
                String::from_utf8_lossy(body).into_owned(),
                Some(("invalid UTF-8", e.valid_up_to())),
            ),
        }
    };
    if flaw.is_none()
        && let Some(at) = text.find('\0')
    {
        flaw = Some(("NUL byte", at));
    }
    (text, flaw)
}

fn utf16_lossy(b: &[u8], unit: fn([u8; 2]) -> u16) -> (String, Option<(&'static str, usize)>) {
    let (pairs, rest) = b.as_chunks::<2>();
    let mut flaw = (!rest.is_empty()).then_some(("invalid UTF-16", b.len() - 1));
    let mut text = String::with_capacity(pairs.len());
    for (i, c) in char::decode_utf16(pairs.iter().copied().map(unit)).enumerate() {
        text.push(c.unwrap_or_else(|_| {
            flaw.get_or_insert(("invalid UTF-16", i * 2));
            '\u{fffd}'
        }));
    }
    (text, flaw)
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
            ".gitmodules",
            ".husky/pre-commit",
            ".git/config",
        ] {
            assert_eq!(kind(p), FileKind::Config, "{p}");
        }
        for p in ["a.woff2", "a.png", "LICENSE", "a.lock", "bin/tool", "a"] {
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
    fn nul_or_a_known_magic_is_binary_for_a_name_that_is_not_code_or_config() {
        assert_eq!(classify_with("a.txt", b"ab\0cd", false), FileKind::Binary);
        assert_eq!(
            classify_with("a.dat", b"\x7fELF\x02\x01", false),
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
    fn a_shebang_makes_code_whatever_the_name() {
        let cases: [(&str, &[u8], Lang); 12] = [
            (".husky/pre-commit", b"#!/bin/sh\necho hi\n", Lang::Sh),
            (".git/hooks/pre-push", b"#!/usr/bin/env bash\n", Lang::Sh),
            ("bin/run", b"#!/usr/bin/env node\n", Lang::Js),
            (
                "bin/run2",
                b"#!/usr/bin/env -S node --no-warnings\n",
                Lang::Js,
            ),
            ("tool", b"#!/usr/bin/python3.11\n", Lang::Py),
            ("tool2", b"#!/usr/bin/env python\n", Lang::Py),
            ("tool3", b"#!/usr/bin/ruby\n", Lang::Rb),
            ("tool4", b"#!/usr/bin/perl -w\n", Lang::Pl),
            ("tool5", b"#!/usr/bin/env pwsh\n", Lang::Ps),
            ("notes.txt", b"#!/bin/sh\ncurl x | sh\n", Lang::Sh),
            ("configure", b"#! /bin/sh\n", Lang::Sh),
            ("odd", b"#!/opt/custom/interp\n", Lang::Sh),
        ];
        for (path, head, lang) in cases {
            assert_eq!(
                classify_with(path, head, false),
                FileKind::Code(lang),
                "{path}"
            );
        }
    }

    #[test]
    fn a_hash_without_a_bang_is_not_a_shebang() {
        assert_eq!(kind("bin/tool"), FileKind::Data);
        assert_eq!(classify_with("a.md", b"# !/bin/sh\n", false), FileKind::Doc);
        assert_eq!(classify_with("a.md", b" #!/bin/sh\n", false), FileKind::Doc);
        assert_eq!(
            classify_with("a.md", b"x\n#!/bin/sh\n", false),
            FileKind::Doc
        );
        assert_eq!(classify_with("a", b"# comment\n", false), FileKind::Data);
    }

    #[test]
    fn an_instruction_file_is_agent_unless_it_is_code_by_extension_or_shebang() {
        // N5: a script in an instruction folder is code, so the content rules read it as code.
        let cases = [
            ("CLAUDE.md", &b"read this\n"[..], FileKind::Agent),
            (".claude/commands/x.md", b"# hi\0\n", FileKind::Agent),
            ("CLAUDE.md", b"#!/bin/sh\necho\n", FileKind::Code(Lang::Sh)),
            (
                ".claude/commands/run",
                b"#!/usr/bin/env python3\n",
                FileKind::Code(Lang::Py),
            ),
            (
                ".claude/skills/x/scripts/a.py",
                b"print(1)\n",
                FileKind::Code(Lang::Py),
            ),
            (
                ".claude/agents/hook.SH",
                b"echo\n",
                FileKind::Code(Lang::Sh),
            ),
            (".cursor/rules/tool.js", b"//\n", FileKind::Code(Lang::Js)),
        ];
        for (path, bytes, want) in cases {
            assert_eq!(classify_with(path, bytes, true), want, "{path}");
        }
    }

    #[test]
    fn only_json_and_the_agent_folder_are_strict_names() {
        for p in [
            "package.json",
            "a/package.json",
            ".vscode/tasks.json",
            ".vscode/settings.json",
            ".mcp.json",
            ".devcontainer/devcontainer.json",
            "devcontainer.json",
            "x.code-workspace",
            ".claude/commands/x.md",
            ".Claude/settings.local.json",
            "PACKAGE.JSON",
        ] {
            assert!(is_strict_json_name(p), "{p}");
        }
        for p in [
            "Makefile",
            "configure",
            "a.m4",
            "setup.py",
            "x.csproj",
            ".husky/pre-commit",
            ".npmrc",
            "CLAUDE.md",
            ".envrc",
            "build.rs",
        ] {
            assert!(!is_strict_json_name(p), "{p}");
        }
    }

    #[test]
    fn a_file_a_command_runs_is_code_by_extension_then_shebang_then_the_interpreter() {
        assert_eq!(
            code_kind_of_run("notes.txt", b"echo\n", Lang::Js),
            FileKind::Code(Lang::Js)
        );
        assert_eq!(
            code_kind_of_run("tool.py", b"echo\n", Lang::Sh),
            FileKind::Code(Lang::Py)
        );
        assert_eq!(
            code_kind_of_run("bin/tool", b"#!/usr/bin/env ruby\n", Lang::Sh),
            FileKind::Code(Lang::Rb)
        );
        assert_eq!(lang_of_program("/usr/bin/python3.11"), Lang::Py);
        assert_eq!(lang_of_program("bash"), Lang::Sh);
    }

    #[test]
    fn a_nul_or_a_magic_does_not_hide_code_config_or_an_instruction_file() {
        // C1: one stray byte must not turn a file the tools still read into a binary.
        let cases: [(&str, &[u8], FileKind); 13] = [
            ("a.js", b"//\0\nrequire('x')", FileKind::Code(Lang::Js)),
            ("a.js", b"\x7fELF\x02\x01", FileKind::Code(Lang::Js)),
            (
                "run.sh",
                b"PK\x03\x04\ncurl x | sh\n",
                FileKind::Code(Lang::Sh),
            ),
            ("a.py", b"\0import os\n", FileKind::Code(Lang::Py)),
            ("package.json", b"{\"a\":\"\0\"}", FileKind::Config),
            (".vscode/tasks.json", b"//\0\n{}", FileKind::Config),
            (".git/config", b"[core]\n\0", FileKind::Config),
            (".git/hooks/pre-commit", b"\0echo\n", FileKind::Config),
            (".husky/pre-commit", b"\0echo\n", FileKind::Config),
            ("deep/Makefile", b"all:\n\0", FileKind::Config),
            ("x.csproj", b"<a>\0</a>", FileKind::Config),
            (".gitattributes", b"\0* filter=x\n", FileKind::Config),
            ("CLAUDE.md", b"\x7fELF\x02\x01", FileKind::Agent),
        ];
        for (path, head, kind) in cases {
            let agent = path == "CLAUDE.md";
            assert_eq!(classify_with(path, head, agent), kind, "{path}");
        }
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

    #[test]
    fn lossy_decoding_keeps_every_rule_running_and_names_the_flaw() {
        assert_eq!(decode_lossy(b"abc"), ("abc".to_string(), None));
        assert_eq!(
            decode_lossy(b"\xef\xbb\xbfabc"),
            ("abc".to_string(), None),
            "a BOM is not a flaw"
        );
        let (text, flaw) = decode_lossy(b"{\"d\":\"\xff\",\"scripts\":1}");
        assert_eq!(text, "{\"d\":\"\u{fffd}\",\"scripts\":1}");
        assert_eq!(flaw, Some(("invalid UTF-8", 6)));
        let (text, flaw) = decode_lossy(b"a\0b");
        assert_eq!(text, "a\0b");
        assert_eq!(flaw, Some(("NUL byte", 1)));
        let (text, flaw) = decode_lossy(&[0xff, 0xfe, 0x61, 0x00, 0x00, 0xd8, 0x62, 0x00]);
        assert_eq!(text, "a\u{fffd}b");
        assert_eq!(flaw.map(|f| f.0), Some("invalid UTF-16"));
        let mut le = vec![0xff, 0xfe];
        le.extend("hi\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode_lossy(&le), ("hi\n".to_string(), None));
    }

    #[test]
    fn the_auto_run_names_are_known_by_path() {
        for p in [
            "package.json",
            "a/b/package.json",
            ".vscode/tasks.json",
            ".cursor/tasks.json",
            ".vscode/settings.json",
            ".claude/settings.json",
            ".claude/settings.local.json",
            "sub/.claude/settings.json",
            ".mcp.json",
            ".cursor/mcp.json",
            ".devcontainer/devcontainer.json",
            ".devcontainer.json",
            "x.code-workspace",
            ".git/config",
            "sub/.git/config",
            ".git/config.worktree",
            ".git/hooks/pre-commit",
            ".husky/pre-commit",
            ".envrc",
            "Makefile",
            "rules.mk",
            "aclocal.m4",
            "configure",
            "Justfile",
            "Taskfile.yml",
            ".pre-commit-config.yaml",
            ".gitattributes",
            ".gitmodules",
            ".npmrc",
            "setup.py",
            "build.rs",
            "a/b.csproj",
            "Directory.Build.props",
            "x.targets",
            ".cargo/config.toml",
            ".cargo/config",
            ".codex/config.toml",
        ] {
            assert!(is_autorun_name(p), "{p}");
        }
        for p in [
            "settings.json",
            "src/tasks.json",
            "README.md",
            "a.js",
            "config",
            "lib/config.toml",
            ".git/HEAD",
            ".git/hooks",
        ] {
            assert!(!is_autorun_name(p), "{p}");
        }
    }
}
