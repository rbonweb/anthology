//! Payload rules: content that does not match its name, executables, blobs, decode-and-run (Task 4).

use std::sync::OnceLock;

use regex::Regex;

use super::hidden::{Hits, Rule, cached, excerpt, is_script, line_at, new_finding, report};
use crate::{FileCtx, FileKind, Finding, Group, Severity, files};

/// Extensions of media and fonts: files a build or a browser reads as bytes, never as text.
const MEDIA: [&str; 16] = [
    "woff", "woff2", "ttf", "otf", "eot", "png", "jpg", "jpeg", "gif", "ico", "bmp", "webp", "mp3",
    "mp4", "wav", "pdf",
];

/// Words a program has and a font or an image does not.
const PROGRAM_TEXT: [&str; 5] = ["require(", "function", "import ", "eval(", "exec("];

const NATIVE_EXTENSIONS: [&str; 4] = ["node", "so", "dll", "dylib"];

/// A base64 run this long is data, not a name.
const BLOB_BASE64: usize = 1024;
const BLOB_HEX: usize = 2048;
/// A run of one or two characters repeated is a divider, not data.
const BLOB_DISTINCT_BASE64: usize = 16;
const BLOB_DISTINCT_HEX: usize = 8;

/// A decoder and an executor this many lines apart or less are one decode-and-run.
const NEAR_LINES: usize = 10;

pub(crate) fn check(f: &FileCtx, out: &mut Vec<Finding>) {
    if f.kind == FileKind::Doc {
        return;
    }
    let ext = extension(f.path);
    let exe = executable_format(f.bytes);
    content_mismatch(f, &ext, exe, out);
    native_program(f, &ext, exe, out);
    if ext == "pyc" {
        out.push(Finding {
            why: "Compiled Python cannot be read, and Python runs it as it is.".to_string(),
            seen: f.path.to_string(),
            action: "Delete it, or build it again from source you have read.".to_string(),
            ..new_finding(f, Group::Payload, "payload.pyc", Severity::Medium)
        });
    }
    if let Some(format) = archive_format(f.bytes) {
        out.push(Finding {
            why: "An archive hides what it holds from a scan of the tree.".to_string(),
            seen: format!("{format} archive"),
            action: "List what it holds before you unpack it.".to_string(),
            ..new_finding(f, Group::Payload, "payload.archive", Severity::Low)
        });
    }
    if let (FileKind::Code(lang), Some(text)) = (f.kind, f.text)
        && is_script(lang)
    {
        blob(f, text, out);
        decode_exec(f, text, out);
    }
}

/// The lower-case extension of the file name, or an empty string.
fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Under a folder that holds generated or third-party code, where blobs and decoders are normal.
fn in_generated_folder(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts
        .iter()
        .any(|p| matches!(*p, "dist" | "build" | "vendor"))
}

/// The name of the program format `b` starts with: ELF, Mach-O or a Windows PE file.
fn executable_format(b: &[u8]) -> Option<&'static str> {
    files::magic(b).filter(|m| matches!(*m, "ELF" | "Mach-O" | "PE"))
}

fn archive_format(b: &[u8]) -> Option<&'static str> {
    files::magic(b).filter(|m| matches!(*m, "zip" | "gzip" | "xz" | "7z"))
}

/// The file's text, even when the text cap cut a multi-byte character at its end.
fn utf8_text<'a>(f: &FileCtx<'a>) -> Option<&'a str> {
    match std::str::from_utf8(f.bytes) {
        Ok(s) => Some(s),
        Err(e) if f.truncated && e.error_len().is_none() => {
            std::str::from_utf8(&f.bytes[..e.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

fn content_mismatch(f: &FileCtx, ext: &str, exe: Option<&str>, out: &mut Vec<Finding>) {
    if !MEDIA.contains(&ext) {
        return;
    }
    let rule = |seen: String, line: usize| {
        Finding {
        line,
        why: "The name says media or a font, but the content is a program. Code that loads this file as a font or an image runs it instead.".to_string(),
        seen,
        action: "Do not run anything that loads this file. Compare it with a real font or image.".to_string(),
        ..new_finding(f, Group::Payload, "payload.content-mismatch", Severity::High)
    }
    };
    if let Some(format) = exe {
        out.push(rule(format!("{format} program header"), 0));
        return;
    }
    let Some(text) = utf8_text(f) else { return };
    let first = PROGRAM_TEXT.iter().filter_map(|m| text.find(m)).min();
    if let Some(at) = first {
        out.push(rule(excerpt(text, at, 200), line_at(text, at)));
    }
}

fn native_program(f: &FileCtx, ext: &str, exe: Option<&str>, out: &mut Vec<Finding>) {
    let seen = match exe {
        Some(format) => format!("{format} program"),
        None if NATIVE_EXTENSIONS.contains(&ext) => format!("native .{ext} file"),
        None => return,
    };
    out.push(Finding {
        why: "A compiled program or native library can do anything once it runs, and it cannot be read here.".to_string(),
        seen,
        action: "Build it again from source you have read, or do not run it.".to_string(),
        ..new_finding(f, Group::Payload, "payload.executable", Severity::Medium)
    });
}

/// The runs of `alphabet` bytes in `b` that are long and varied enough to be data. Returns the
/// offset and length of the first, and how many there are.
fn data_runs(
    b: &[u8],
    alphabet: fn(u8) -> bool,
    min_len: usize,
    min_distinct: usize,
    needs_non_hex: bool,
) -> Option<(usize, usize, usize)> {
    let (mut first, mut count) = (None, 0);
    let mut i = 0;
    while i < b.len() {
        if !alphabet(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let (mut seen, mut distinct, mut non_hex) = ([false; 128], 0, false);
        while i < b.len() && alphabet(b[i]) {
            let c = b[i] as usize;
            if !seen[c] {
                seen[c] = true;
                distinct += 1;
            }
            non_hex |= !b[i].is_ascii_hexdigit();
            i += 1;
        }
        if i - start >= min_len && distinct >= min_distinct && (non_hex || !needs_non_hex) {
            first.get_or_insert((start, i - start));
            count += 1;
        }
    }
    first.map(|(at, len)| (at, len, count))
}

fn is_base64(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/'
}

fn blob(f: &FileCtx, text: &str, out: &mut Vec<Finding>) {
    if in_generated_folder(f.path) || f.path.ends_with(".min.js") {
        return;
    }
    let b = text.as_bytes();
    // A base64 run has a letter beyond a-f: a run of hex digits alone is hex data, and a hex blob
    // has to be twice as long.
    let found = data_runs(b, is_base64, BLOB_BASE64, BLOB_DISTINCT_BASE64, true)
        .map(|r| ("base64", r))
        .or_else(|| {
            data_runs(
                b,
                |c| c.is_ascii_hexdigit(),
                BLOB_HEX,
                BLOB_DISTINCT_HEX,
                false,
            )
            .map(|r| ("hex", r))
        });
    let Some((kind, (at, len, count))) = found else {
        return;
    };
    let head: String = text[at..].chars().take(48).collect();
    out.push(Finding {
        line: line_at(text, at),
        why: "A very long encoded string in a script is a hidden file: a program, a key or a payload.".to_string(),
        seen: format!("{len} {kind} characters in a row, starting {head}"),
        action: "Decode it in a sandbox you control and read it before you run this script.".to_string(),
        count,
        ..new_finding(f, Group::Payload, "payload.blob", Severity::Medium)
    });
}

fn decoders() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r#"\batob\s*\(|Buffer\.from\([^\n]*?['"]base64['"]|\bb64decode\b|\bfromhex\b|zlib\.decompress|marshal\.loads"#,
    )
}

/// What runs text. `eval(`, `Function(` and `exec(` count as calls, not as the methods of a regex, a
/// browser tab or a cursor: no `.` or word character before them (group 1), except on the global
/// object (group 2). Group 3 holds the process APIs.
fn executors() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"(?m)(?:^|[^.\w])(eval\s*\(|Function\s*\(|exec\s*\()|\b((?:window|globalThis|global|self)\.(?:eval|Function)\s*\()|\b(child_process|subprocess|os\.system|vm\.runIn\w*)",
    )
}

/// The 1-based line of each offset, which must be in ascending order.
fn lines_of(text: &str, offsets: &[usize]) -> Vec<usize> {
    let (mut line, mut at) = (1, 0);
    offsets
        .iter()
        .map(|&off| {
            line += text.as_bytes()[at..off]
                .iter()
                .filter(|b| **b == b'\n')
                .count();
            at = off;
            line
        })
        .collect()
}

fn decode_exec(f: &FileCtx, text: &str, out: &mut Vec<Finding>) {
    let decoder_at: Vec<usize> = decoders().find_iter(text).map(|m| m.start()).collect();
    if decoder_at.is_empty() {
        return;
    }
    let decoder_lines = lines_of(text, &decoder_at);
    let executor_at: Vec<usize> = executors()
        .captures_iter(text)
        .filter_map(|caps| caps.iter().skip(1).flatten().next().map(|m| m.start()))
        .collect();
    let executor_lines = lines_of(text, &executor_at);
    let mut hits = Hits::default();
    for (at, line) in executor_at.iter().zip(&executor_lines) {
        let nearest = decoder_lines.partition_point(|d| d < line);
        let close = [nearest.checked_sub(1), Some(nearest)]
            .into_iter()
            .flatten()
            .filter_map(|n| decoder_lines.get(n))
            .any(|d| d.abs_diff(*line) <= NEAR_LINES);
        if close {
            hits.add(*at);
        }
    }
    let action =
        "Decode the data by hand in a sandbox you control and read it before you run this script.";
    if hits.first.is_some() {
        let severity = if in_generated_folder(f.path) {
            Severity::Medium
        } else {
            Severity::High
        };
        let rule = Rule {
            group: Group::Payload,
            id: "payload.decode-exec",
            severity,
            why: "The script decodes data and runs it a few lines later: the shape of a hidden payload loader.",
            action,
        };
        out.extend(report(f, text, &hits, &rule));
        return;
    }
    // A decoder and an executor farther apart: the same loader with its calls spread out.
    let mut far = Hits::default();
    for at in &executor_at {
        far.add(*at);
    }
    let rule = Rule {
        group: Group::Payload,
        id: "payload.decode-exec",
        severity: Severity::Medium,
        why: "The script decodes data and also runs text, far apart: a payload loader can spread the two calls to avoid notice.",
        action,
    };
    out.extend(report(f, text, &far, &rule));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, Group, Lang, Severity};

    fn run(path: &str, kind: FileKind, bytes: &[u8]) -> Vec<Finding> {
        let text = std::str::from_utf8(bytes).ok();
        let f = FileCtx {
            path,
            bytes,
            text,
            kind,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        out
    }

    fn code(path: &str, lang: Lang, text: &str) -> Vec<Finding> {
        run(path, FileKind::Code(lang), text.as_bytes())
    }

    fn rules(v: &[Finding]) -> Vec<&str> {
        v.iter().map(|f| f.rule.as_str()).collect()
    }

    fn one<'a>(v: &'a [Finding], rule: &str) -> &'a Finding {
        let hits: Vec<&Finding> = v.iter().filter(|f| f.rule == rule).collect();
        assert_eq!(hits.len(), 1, "{rule} in {:?}", rules(v));
        hits[0]
    }

    fn elf() -> Vec<u8> {
        let mut b = b"\x7fELF\x02\x01\x01\x00".to_vec();
        b.extend([0u8; 56]);
        b
    }

    fn pe() -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3C] = 0x40;
        b[0x40..0x44].copy_from_slice(b"PE\0\0");
        b
    }

    #[test]
    fn javascript_in_a_font_is_a_content_mismatch() {
        let js = "const cp = require('child_process');\ncp.exec('id');\n";
        let v = run("assets/fa-solid.woff2", FileKind::Data, js.as_bytes());
        let f = one(&v, "payload.content-mismatch");
        assert_eq!(
            (f.severity, f.group, f.line),
            (Severity::High, Group::Payload, 1)
        );
        assert!(f.seen.contains("require"), "{:?}", f.seen);
        for (name, body) in [
            ("a.png", "function go() {}"),
            ("a.jpg", "import os"),
            ("a.ttf", "eval(x)"),
            ("a.mp3", "exec(x)"),
        ] {
            let v = run(name, FileKind::Data, body.as_bytes());
            assert_eq!(
                one(&v, "payload.content-mismatch").severity,
                Severity::High,
                "{name}"
            );
        }
    }

    #[test]
    fn a_program_named_as_media_is_a_content_mismatch() {
        for (name, bytes) in [
            ("a.png", elf()),
            ("b.woff2", pe()),
            ("c.gif", b"\xcf\xfa\xed\xfe\x07\x00".to_vec()),
        ] {
            let v = run(name, FileKind::Binary, &bytes);
            assert_eq!(
                one(&v, "payload.content-mismatch").severity,
                Severity::High,
                "{name}"
            );
        }
    }

    #[test]
    fn a_real_font_or_image_and_other_text_are_clean() {
        let woff2 = b"wOF2\x00\x01\x00\x00\x00\x00\x35\xe4\xff\xfe\x80\xc3".to_vec();
        assert!(run("a.woff2", FileKind::Binary, &woff2).is_empty());
        assert!(
            run(
                "a.png",
                FileKind::Binary,
                b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"
            )
            .is_empty()
        );
        assert!(run("a.png", FileKind::Data, b"").is_empty());
        // Text with none of the markers, an LFS-style pointer, and a vector image with a style import.
        assert!(
            run(
                "a.png",
                FileKind::Data,
                b"version https://git-lfs.github.com/spec/v1\n"
            )
            .is_empty()
        );
        assert!(
            run(
                "a.svg",
                FileKind::Data,
                b"<svg><style>@import url(x);</style></svg>"
            )
            .is_empty()
        );
        // A code file is allowed to say `function`.
        assert!(code("a.js", Lang::Js, "function go() {}").is_empty());
    }

    #[test]
    fn a_cut_multibyte_tail_does_not_hide_a_mismatch() {
        let mut b = b"require('x');\n".to_vec();
        b.extend_from_slice(&"é".as_bytes()[..1]);
        let f = FileCtx {
            path: "a.woff",
            bytes: &b,
            text: None,
            kind: FileKind::Data,
            truncated: true,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        assert_eq!(
            one(&out, "payload.content-mismatch").severity,
            Severity::High
        );
    }

    #[test]
    fn programs_are_medium() {
        let macho = b"\xfe\xed\xfa\xcf\x01\x00\x00\x00".to_vec();
        for (name, bytes) in [
            ("bin/tool", elf()),
            ("bin/tool.exe", pe()),
            ("bin/mac", macho),
        ] {
            let v = run(name, FileKind::Binary, &bytes);
            let f = one(&v, "payload.executable");
            assert_eq!(
                (f.severity, f.group),
                (Severity::Medium, Group::Payload),
                "{name}"
            );
        }
        for name in [
            "build/addon.node",
            "lib/libx.so",
            "bin/x.dll",
            "lib/x.dylib",
        ] {
            let v = run(name, FileKind::Binary, b"\x00\x01\x02");
            assert_eq!(
                one(&v, "payload.executable").severity,
                Severity::Medium,
                "{name}"
            );
        }
    }

    #[test]
    fn text_and_media_are_not_programs() {
        assert!(run("a.sh", FileKind::Code(Lang::Sh), b"#!/bin/sh\necho hi\n").is_empty());
        assert!(run("a.dat", FileKind::Binary, b"MZ").is_empty());
        assert!(run("a.sol", FileKind::Data, b"abc").is_empty());
    }

    #[test]
    fn compiled_python_is_medium() {
        let v = run(
            "pkg/__pycache__/x.cpython-311.pyc",
            FileKind::Binary,
            b"\xa7\x0d\x0d\x0a\x00\x00",
        );
        let f = one(&v, "payload.pyc");
        assert_eq!((f.severity, f.group), (Severity::Medium, Group::Payload));
        assert!(run("pkg/x.py", FileKind::Code(Lang::Py), b"pass").is_empty());
    }

    #[test]
    fn archives_are_low() {
        for (name, bytes) in [
            ("a.zip", &b"PK\x03\x04\x14\x00"[..]),
            ("a.gz", b"\x1f\x8b\x08\x00"),
            ("a.xz", b"\xfd7zXZ\x00\x00"),
            ("a.7z", b"7z\xbc\xaf\x27\x1c"),
        ] {
            let v = run(name, FileKind::Binary, bytes);
            let f = one(&v, "payload.archive");
            assert_eq!(
                (f.severity, f.group),
                (Severity::Low, Group::Payload),
                "{name}"
            );
        }
        assert!(run("a.txt", FileKind::Data, b"PK is a text").is_empty());
    }

    fn b64(n: usize) -> String {
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        alphabet.chars().cycle().take(n).collect()
    }

    fn hex(n: usize) -> String {
        "0123456789abcdef".chars().cycle().take(n).collect()
    }

    #[test]
    fn a_long_base64_run_in_a_script_is_a_blob() {
        let t = format!("const p = \"{}\";\n", b64(1024));
        let f = &code("src/a.js", Lang::Js, &t);
        let hit = one(f, "payload.blob");
        assert_eq!(
            (hit.severity, hit.group, hit.line),
            (Severity::Medium, Group::Payload, 1)
        );
        assert!(hit.seen.contains("1024"), "{:?}", hit.seen);
        assert!(hit.seen.chars().count() < 200);
        assert_eq!(
            one(&code("a.py", Lang::Py, &t), "payload.blob").severity,
            Severity::Medium
        );
    }

    #[test]
    fn a_long_hex_run_in_a_script_is_a_blob() {
        // Only hex digits: it is a blob at 2048, not at 1024.
        let t = format!("data = '{}'\n", hex(2048));
        assert_eq!(
            one(&code("a.py", Lang::Py, &t), "payload.blob").severity,
            Severity::Medium
        );
    }

    #[test]
    fn short_or_monotone_runs_and_other_places_are_not_blobs() {
        assert!(code("a.js", Lang::Js, &format!("x = '{}'", b64(1023))).is_empty());
        assert!(code("a.py", Lang::Py, &format!("x = '{}'", hex(2047))).is_empty());
        // A rule of dashes or slashes is a divider, not data.
        assert!(code("a.js", Lang::Js, &format!("//{}\n", "/".repeat(2000))).is_empty());
        assert!(code("a.sh", Lang::Sh, &format!("# {}\n", "=".repeat(2000))).is_empty());
        // Under dist, build, vendor, a minified file, a non-script language and a doc.
        let t = format!("x = '{}'", b64(2000));
        for path in ["dist/a.js", "pkg/build/a.js", "vendor/a.js", "a.min.js"] {
            assert!(code(path, Lang::Js, &t).is_empty(), "{path}");
        }
        assert!(code("a.rs", Lang::Rs, &t).is_empty());
        assert!(run("a.md", FileKind::Doc, t.as_bytes()).is_empty());
    }

    #[test]
    fn decode_then_run_in_a_script_is_high() {
        let js = "const s = atob(\"ZWNobyBoaQ==\");\neval(s);\n";
        let v = code("src/a.js", Lang::Js, js);
        let f = one(&v, "payload.decode-exec");
        assert_eq!(
            (f.severity, f.group, f.line),
            (Severity::High, Group::Payload, 2)
        );
        assert!(f.seen.contains("eval"), "{:?}", f.seen);
        let py = "import base64\nexec(base64.b64decode('cHJpbnQoMSk='))\n";
        assert_eq!(
            one(&code("main.py", Lang::Py, py), "payload.decode-exec").severity,
            Severity::High
        );
        let node = "require('child_process').exec(Buffer.from(x, 'base64').toString());\n";
        assert_eq!(
            one(&code("a.js", Lang::Js, node), "payload.decode-exec").severity,
            Severity::High
        );
        let marshal = "import marshal\nmarshal.loads(d)\nsubprocess.run(x)\n";
        assert_eq!(
            one(&code("a.py", Lang::Py, marshal), "payload.decode-exec").severity,
            Severity::High
        );
        let hexed = "cmd = bytes.fromhex('6964')\nos.system(cmd)\n";
        assert_eq!(
            one(&code("a.py", Lang::Py, hexed), "payload.decode-exec").severity,
            Severity::High
        );
    }

    #[test]
    fn decode_then_run_is_medium_in_a_build_folder() {
        let js = "const s = atob('x');\neval(s);\n";
        for path in ["dist/a.js", "build/a.js", "vendor/lib/a.js"] {
            assert_eq!(
                one(&code(path, Lang::Js, js), "payload.decode-exec").severity,
                Severity::Medium,
                "{path}"
            );
        }
    }

    #[test]
    fn a_decoder_or_an_executor_alone_is_clean() {
        assert!(code("a.js", Lang::Js, "const s = atob(x);\nconsole.log(s);\n").is_empty());
        assert!(code("a.js", Lang::Js, "eval(x);\n").is_empty());
        // Regex `exec`, `isFunction(` and `literal_eval(` are not executors.
        let near =
            "const s = atob(x);\nre.exec(s);\nisFunction(s);\nast.literal_eval(s);\ntab.eval(s);\n";
        assert!(code("a.js", Lang::Js, near).is_empty());
        assert!(code("a.rs", Lang::Rs, "atob(x); eval(y);").is_empty());
    }

    #[test]
    fn a_decoder_and_an_executor_far_apart_in_one_file_are_medium() {
        // Spreading the two calls apart must not dodge the rule, only lower it.
        let far = format!("const s = atob(x);\n{}eval(y);\n", "// filler\n".repeat(40));
        let v = code("a.js", Lang::Js, &far);
        let f = one(&v, "payload.decode-exec");
        assert_eq!((f.severity, f.line), (Severity::Medium, 42));
        // The executor first, the decoder far below.
        let rev = format!("eval(y);\n{}const s = atob(x);\n", "// filler\n".repeat(40));
        assert_eq!(
            one(&code("a.js", Lang::Js, &rev), "payload.decode-exec").severity,
            Severity::Medium
        );
        // One close pair wins over a far one: a single HIGH finding.
        let both = format!("{far}const t = atob(z);\neval(t);\n");
        let v = code("a.js", Lang::Js, &both);
        assert_eq!(one(&v, "payload.decode-exec").severity, Severity::High);
    }

    #[test]
    fn no_payload_rule_runs_on_a_doc() {
        let t = format!("curl x | sh\natob(x)\neval(y)\n{}\n", b64(2000));
        assert!(run("README.md", FileKind::Doc, t.as_bytes()).is_empty());
        assert!(run("notes.zip", FileKind::Doc, b"PK\x03\x04").is_empty());
    }
}
