//! `anthology-dev atl-trim` (DV4): the ported `tests/test_rtk_trim.py` (one `#[test]` per Python
//! method, same name, inside a module named after the Python class), and the tests of what the
//! port adds: the Python-compatible helpers, the declaration scanner that replaces the run-time
//! regex of `Trim.drop`, the `../crates/` lockfile allowlist, the CLI, and the fixed point of the
//! real `atl/` tree.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anthology_dev::atl_trim::{self, Op, Trim};
use anthology_dev::{Output, dispatch};

const RUST: &str = "use std::io;

/// Docs of keep.
fn keep() {
    one();
}

/// Docs of gone.
#[inline]
fn gone(x: u8) -> u8 {
    if x > 0 {
        x
    } else {
        0
    }
}

mod tests {
    #[test]
    fn test_gone() {
        assert_eq!(gone(1), 1);
    }

    #[test]
    fn test_keep() {
        keep();
    }
}
";

/// A temp directory holding the tree a test trims.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().expect("temp dir");
        let root = tmp.path().to_path_buf();
        Fixture { _tmp: tmp, root }
    }

    fn trim(&self) -> Trim {
        Trim::new(&self.root)
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, text).expect("write");
        path
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).expect("read")
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }
}

fn err_text<T: std::fmt::Debug>(result: Result<T, atl_trim::TrimError>) -> String {
    result.expect_err("an error").0
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn run_cli(list: &[&str], stdin: &str) -> Output {
    let mut input = stdin.as_bytes();
    dispatch(&args(list), &mut input)
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("utf-8 output")
}

mod rm_test {
    use super::*;

    #[test]
    fn test_removes_files_and_directories_once() {
        let f = Fixture::new();
        f.write("a.txt", "a");
        f.write("d/sub/b.txt", "b");
        let mut t = f.trim();
        t.rm(&["a.txt", "d", "never-there"]).unwrap();
        assert!(!f.exists("a.txt"));
        assert!(!f.exists("d"));
        assert_eq!(t.changed, ["rm a.txt", "rm d"]);
        let mut again = f.trim();
        again.rm(&["a.txt", "d", "never-there"]).unwrap();
        assert!(again.changed.is_empty());
    }

    #[test]
    fn a_symlink_to_a_directory_is_unlinked_and_a_broken_one_too() {
        let f = Fixture::new();
        f.write("real/keep.txt", "k");
        std::os::unix::fs::symlink("real", f.root.join("link")).unwrap();
        std::os::unix::fs::symlink("nowhere", f.root.join("broken")).unwrap();
        let mut t = f.trim();
        t.rm(&["link", "broken"]).unwrap();
        assert_eq!(t.changed, ["rm link", "rm broken"]);
        assert!(fs::symlink_metadata(f.root.join("link")).is_err());
        assert!(fs::symlink_metadata(f.root.join("broken")).is_err());
        assert_eq!(f.read("real/keep.txt"), "k");
    }
}

mod sub_test {
    use super::*;

    #[test]
    fn test_applies_once_then_is_a_no_op() {
        let f = Fixture::new();
        f.write("f.rs", "a\nold line\nb\n");
        let mut t = f.trim();
        t.sub("f.rs", "old line\n", "new line\n", None).unwrap();
        assert_eq!(f.read("f.rs"), "a\nnew line\nb\n");
        assert_eq!(t.changed, ["edit f.rs"]);
        let mut again = f.trim();
        again.sub("f.rs", "old line\n", "new line\n", None).unwrap();
        assert!(again.changed.is_empty());
        assert_eq!(f.read("f.rs"), "a\nnew line\nb\n");
    }

    #[test]
    fn test_new_that_extends_old_is_applied_once() {
        let f = Fixture::new();
        f.write("Cargo.toml", "[lints.rust]\nwarnings = \"deny\"\n");
        let old = "[lints.rust]\nwarnings = \"deny\"\n";
        let new = "[lints.rust]\nwarnings = \"deny\"\nx = 1\n";
        f.trim().sub("Cargo.toml", old, new, None).unwrap();
        f.trim().sub("Cargo.toml", old, new, None).unwrap();
        assert_eq!(f.read("Cargo.toml"), new);
    }

    #[test]
    fn test_missing_anchor_raises_with_file_and_anchor() {
        let f = Fixture::new();
        f.write("f.rs", "something else\n");
        let message = err_text(f.trim().sub("f.rs", "old line\nmore\n", "new line\n", None));
        assert!(message.contains("f.rs"), "{message}");
        assert!(message.contains("anchor missing"), "{message}");
        assert!(message.contains("old line"), "{message}");
        assert_eq!(f.read("f.rs"), "something else\n");
    }

    #[test]
    fn test_changed_anchor_is_not_taken_for_applied() {
        // The anchor block changed upstream (a new line in it): the replacement is not
        // present either, so the trim stops instead of skipping.
        let f = Fixture::new();
        f.write("f.rs", "enum E {\n    A,\n    New,\n    B,\n}\n");
        let result = f.trim().sub(
            "f.rs",
            "enum E {\n    A,\n    B,\n}\n",
            "enum E {\n    A,\n}\n// trimmed\n",
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_ambiguous_anchor_raises() {
        let f = Fixture::new();
        f.write("f.rs", "x\nx\n");
        let message = err_text(f.trim().sub("f.rs", "x\n", "y\n", None));
        assert!(message.contains("2 times"), "{message}");
    }

    #[test]
    fn test_new_inside_old_is_rejected_without_done() {
        let f = Fixture::new();
        f.write("f.rs", "a\nb\n");
        let message = err_text(f.trim().sub("f.rs", "a\nb\n", "a\n", None));
        assert!(message.contains("bad anchor design"), "{message}");
    }

    #[test]
    fn test_done_marker_decides_applied_for_chained_edits() {
        let f = Fixture::new();
        f.write("f.rs", "if x {\n    old();\n} else {\n");
        let mut t = f.trim();
        t.sub(
            "f.rs",
            "if x {\n    old();\n",
            "if x {\n    mid();\n",
            Some("mid();"),
        )
        .unwrap();
        // A later edit rewrites the replacement; the done marker still says "applied".
        t.sub(
            "f.rs",
            "    mid();\n} else {\n",
            "    final_call();\n} else {\n",
            None,
        )
        .unwrap();
        let mut again = f.trim();
        assert!(
            again
                .sub(
                    "f.rs",
                    "if x {\n    old();\n",
                    "if x {\n    mid();\n",
                    Some("mid();"),
                )
                .is_err()
        );
        again
            .sub(
                "f.rs",
                "if x {\n    old();\n",
                "if x {\n    mid();\n",
                Some("final_call();"),
            )
            .unwrap();
        assert!(again.changed.is_empty());
    }

    #[test]
    fn test_missing_file_raises() {
        let f = Fixture::new();
        let message = err_text(f.trim().sub("nope.rs", "a", "b", None));
        assert!(message.contains("nope.rs"), "{message}");
        assert!(message.contains("file missing"), "{message}");
    }

    #[test]
    fn an_empty_done_marker_is_falsy_and_never_means_applied() {
        let f = Fixture::new();
        f.write("f.rs", "other\n");
        // `done=""`: no `new in old` design check, and the empty marker does not count as present.
        let message = err_text(f.trim().sub("f.rs", "old\n", "new\n", Some("")));
        assert!(message.contains("anchor missing"), "{message}");
        // An empty `new` without `done` is a design error only when it is inside `old`: it is.
        let message = err_text(f.trim().sub("f.rs", "old\n", "", None));
        assert!(message.contains("bad anchor design"), "{message}");
    }

    #[test]
    fn the_design_check_comes_before_the_file_is_read() {
        let f = Fixture::new();
        let message = err_text(f.trim().sub("nope.rs", "a\nb\n", "a\n", None));
        assert!(message.contains("bad anchor design"), "{message}");
    }

    #[test]
    fn the_count_is_non_overlapping_and_the_message_names_it() {
        let f = Fixture::new();
        f.write("f.rs", "aaaa");
        // "aa" occurs twice without overlap (not three times).
        let message = err_text(f.trim().sub("f.rs", "aa", "b", None));
        assert!(
            message.ends_with("anchor occurs 2 times: 'aa'"),
            "{message}"
        );
    }

    #[test]
    fn text_is_utf8_and_newlines_are_not_translated() {
        let f = Fixture::new();
        f.write("f.txt", "caf\u{e9}\r\nold\r\nx\ry\n");
        f.trim().sub("f.txt", "old\r\n", "new\r\n", None).unwrap();
        assert_eq!(
            fs::read(f.root.join("f.txt")).unwrap(),
            "caf\u{e9}\r\nnew\r\nx\ry\n".as_bytes()
        );
    }
}

mod cut_test {
    use super::*;

    #[test]
    fn test_applies_once_then_is_a_no_op() {
        let f = Fixture::new();
        f.write("f.rs", "a\npub mod telemetry;\nb\n");
        f.trim()
            .cut("f.rs", "pub mod telemetry;\n", "mod telemetry")
            .unwrap();
        assert_eq!(f.read("f.rs"), "a\nb\n");
        let mut again = f.trim();
        again
            .cut("f.rs", "pub mod telemetry;\n", "mod telemetry")
            .unwrap();
        assert!(again.changed.is_empty());
    }

    #[test]
    fn test_missing_anchor_with_the_token_still_present_raises() {
        let f = Fixture::new();
        f.write("f.rs", "a\npub(crate) mod telemetry;\nb\n");
        let message = err_text(
            f.trim()
                .cut("f.rs", "pub mod telemetry;\n", "mod telemetry"),
        );
        assert!(message.contains("f.rs"), "{message}");
        assert!(message.contains("still present"), "{message}");
        assert!(message.contains("pub mod telemetry;"), "{message}");
        assert_eq!(
            message,
            "f.rs: anchor missing but 'mod telemetry' still present: 'pub mod telemetry;'"
        );
    }

    #[test]
    fn test_ambiguous_anchor_raises() {
        let f = Fixture::new();
        f.write("f.rs", "x\nx\n");
        assert!(f.trim().cut("f.rs", "x\n", "x").is_err());
    }
}

mod drop_test {
    use super::*;

    #[test]
    fn test_drops_item_with_attributes_and_docs_once() {
        let f = Fixture::new();
        f.write("f.rs", RUST);
        f.trim()
            .drop_item("f.rs", "fn gone(x: u8) -> u8 {", None)
            .unwrap();
        let text = f.read("f.rs");
        assert!(!text.contains("fn gone"));
        assert!(!text.contains("Docs of gone"));
        assert!(!text.contains("#[inline]"));
        assert!(text.contains("/// Docs of keep.\nfn keep() {\n    one();\n}\n\nmod tests {"));
        let mut again = f.trim();
        again
            .drop_item("f.rs", "fn gone(x: u8) -> u8 {", None)
            .unwrap();
        assert!(again.changed.is_empty());
    }

    #[test]
    fn test_drops_an_indented_test_with_its_attribute() {
        let f = Fixture::new();
        f.write("f.rs", RUST);
        let mut t = f.trim();
        t.drop_item("f.rs", "    fn test_gone() {", None).unwrap();
        let text = f.read("f.rs");
        assert!(!text.contains("test_gone"));
        assert_eq!(text.matches("#[test]").count(), 1);
        assert!(text.contains("    #[test]\n    fn test_keep() {\n        keep();\n    }\n}\n"));
        assert_eq!(t.changed, ["drop f.rs fn test_gone() {"]);
    }

    #[test]
    fn test_body_changes_are_tolerated() {
        let f = Fixture::new();
        f.write(
            "f.rs",
            &RUST.replace("        x\n    } else {", "        x + 0\n    } else {"),
        );
        f.trim()
            .drop_item("f.rs", "fn gone(x: u8) -> u8 {", None)
            .unwrap();
        assert!(!f.read("f.rs").contains("fn gone"));
    }

    #[test]
    fn test_changed_signature_raises() {
        let f = Fixture::new();
        f.write(
            "f.rs",
            &RUST.replace("fn gone(x: u8) -> u8 {", "fn gone(x: u16) -> u16 {"),
        );
        let message = err_text(f.trim().drop_item("f.rs", "fn gone(x: u8) -> u8 {", None));
        assert!(message.contains("f.rs"), "{message}");
        assert!(message.contains("signature changed?"), "{message}");
        assert!(message.contains("fn gone"), "{message}");
        assert_eq!(
            message,
            "f.rs: signature changed? 'fn gone' present but not as this exact line: \
             'fn gone(x: u8) -> u8 {'"
        );
    }

    #[test]
    fn test_duplicate_signature_raises() {
        let f = Fixture::new();
        f.write("f.rs", "fn a() {\n}\nfn a() {\n}\n");
        let message = err_text(f.trim().drop_item("f.rs", "fn a() {", None));
        assert!(message.contains("signature occurs 2 times"), "{message}");
    }

    #[test]
    fn test_item_without_closing_brace_raises() {
        let f = Fixture::new();
        f.write("f.rs", "    fn a() {\n        x();\nfn b() {}\n");
        let message = err_text(f.trim().drop_item("f.rs", "    fn a() {", None));
        assert!(
            message.contains("item has no closing brace at its indentation"),
            "{message}"
        );
    }

    #[test]
    fn a_missing_end_and_a_signature_without_a_keyword() {
        let f = Fixture::new();
        f.write("f.rs", "fn a() {\n    x();\n");
        let message = err_text(f.trim().drop_item("f.rs", "fn a() {", None));
        assert!(message.contains("no end for item"), "{message}");
        let message = err_text(f.trim().drop_item("f.rs", "let a = 1;", None));
        assert_eq!(
            message,
            "f.rs: drop needs gone= for this signature: 'let a = 1;'"
        );
    }

    #[test]
    fn a_one_line_item_and_the_gone_token() {
        let f = Fixture::new();
        f.write("f.rs", "a\n\nconst X: u8 = 1;\n\nb\n");
        let mut t = f.trim();
        t.drop_item("f.rs", "const X: u8 = 1;", None).unwrap();
        assert_eq!(f.read("f.rs"), "a\n\nb\n");
        assert_eq!(t.changed, ["drop f.rs const X: u8 = 1;"]);
        // `gone=` names the token; it still occurs but not as that line: stop.
        f.write("g.rs", "xx token_here yy\n");
        let message = err_text(f.trim().drop_item("g.rs", "fn q() {", Some("token_here")));
        assert_eq!(
            message,
            "g.rs: signature changed? 'token_here' present but not as this exact line: 'fn q() {'"
        );
        f.trim()
            .drop_item("g.rs", "fn q() {", Some("absent_token"))
            .unwrap();
    }

    #[test]
    fn closers_are_the_four_forms_at_the_indent_and_one_blank_line_goes() {
        for closer in ["}", "},", "});", "};"] {
            let f = Fixture::new();
            f.write(
                "f.rs",
                &format!("a\n\n    // note\n    let x = f(|| {{\n        1\n    {closer}\n\nb\n"),
            );
            f.trim()
                .drop_item("f.rs", "    let x = f(|| {", None)
                .unwrap();
            assert_eq!(f.read("f.rs"), "a\n\nb\n", "closer {closer}");
        }
    }

    #[test]
    fn the_const_fn_quirk_reads_const_and_fn() {
        // `const fn foo` yields the keyword `const` and the name `fn`: with the signature line
        // gone and `const fn` still in the file, the trim stops.
        let f = Fixture::new();
        f.write("f.rs", "pub const fn foo(x: u8) -> u8 { x }\n");
        let message = err_text(f.trim().drop_item("f.rs", "const fn foo() {", None));
        assert!(message.contains("'const fn' present"), "{message}");
    }
}

mod lock_guard_test {
    use super::*;

    const LOCK: &str = "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n\
        [[package]]\nname = \"b\"\nversion = \"2.0.0\"\ndependencies = [\n \"a\",\n]\n";

    fn pair(n: &str, v: &str) -> (String, String) {
        (n.to_string(), v.to_string())
    }

    #[test]
    fn test_packages() {
        let f = Fixture::new();
        let path = f.write("Cargo.lock", LOCK);
        let packages = atl_trim::lock_packages(&path).unwrap();
        assert_eq!(
            packages.into_iter().collect::<Vec<_>>(),
            [pair("a", "1.0.0"), pair("b", "2.0.0")]
        );
    }

    #[test]
    fn test_pruning_passes_and_reports_removals() {
        let f = Fixture::new();
        let before = f.write("before.lock", LOCK);
        let after = f.write(
            "after.lock",
            "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n",
        );
        assert_eq!(
            atl_trim::lock_guard(&before, &after).unwrap(),
            [pair("b", "2.0.0")]
        );
    }

    #[test]
    fn test_changed_or_added_version_raises() {
        let f = Fixture::new();
        let before = f.write("before.lock", LOCK);
        let after = f.write("after.lock", &LOCK.replace("\"2.0.0\"", "\"2.0.1\""));
        let message = err_text(atl_trim::lock_guard(&before, &after));
        assert!(message.contains("b 2.0.1"), "{message}");
        assert_eq!(
            message,
            "Cargo.lock: the lockfile step added or changed packages: b 2.0.1"
        );
    }

    #[test]
    fn text_mode_lines_and_the_exact_line_shape() {
        // \r\n and a lone \r are line ends; a line with another shape is not a package line;
        // a version before any name is ignored; the sorted list is by code point.
        let text = "[[package]]\r\nname = \"z\"\rversion = \"1\"\r\n\
            [[package]]\nversion = \"9\"\nname = \"y\"\nversion = \"2\"\n\
            [[package]]\nname = \"x\" \nversion = \"3\"\nname = \"w\"\nversion = \"a\"b\"\n\
            [other]\nname = \"v\"\nversion = \"4\"\n";
        assert_eq!(
            atl_trim::lock_packages_text(text)
                .into_iter()
                .collect::<Vec<_>>(),
            [pair("y", "2"), pair("z", "1")]
        );
    }

    #[test]
    fn an_unreadable_lockfile_is_an_io_error_naming_the_path() {
        let f = Fixture::new();
        let missing = f.root.join("nope.lock");
        let message = err_text(atl_trim::lock_packages(&missing));
        assert!(
            message.starts_with(&missing.display().to_string()),
            "{message}"
        );
    }
}

mod trimmed_conflicts_test {
    use super::*;

    #[test]
    fn test_only_deleted_by_us_paths_inside_removed_paths() {
        let status = [
            "DU atl/engine/src/lib.rs",       // upstream edited a deleted crate
            "DU atl/src/core/telemetry.rs",   // upstream edited a deleted file
            "UU atl/src/lib.rs",              // a real content conflict: never resolved here
            "DU atl/src/core/telemetry_x.rs", // not a removed path (prefix only)
            "DU other/engine/x.rs",           // outside the subtree
            "DU \"atl/engine/odd name.rs\"",  // quoted: left for a human
            " M atl/Cargo.toml",
        ]
        .join("\n");
        assert_eq!(
            atl_trim::trimmed_conflicts(&status, &["engine", "src/core/telemetry.rs"], "atl/"),
            ["atl/engine/src/lib.rs", "atl/src/core/telemetry.rs"]
        );
    }

    #[test]
    fn test_removed_paths_come_from_the_operations() {
        let removed = atl_trim::removed_paths();
        for path in [
            "engine",
            "plugin",
            "proxy",
            "src/core/telemetry.rs",
            "tests/fork_8_upstream_headers.rs",
            "src/hooks/init/codex.rs",
        ] {
            assert!(removed.contains(&path), "{path}");
        }
        assert!(!removed.contains(&"src/hooks/init/claude.rs"));
    }
}

mod operations_test {
    use super::*;

    #[test]
    fn test_an_unexpected_tree_stops_on_the_first_missing_file() {
        let f = Fixture::new();
        let message = err_text(f.trim().run_operations());
        assert!(message.contains("Cargo.toml: file missing"), "{message}");
    }

    #[test]
    fn test_cli_exits_1_with_a_named_message() {
        let f = Fixture::new();
        let root = f.root.to_str().unwrap();
        let out = run_cli(&["atl-trim", "--root", root], "");
        assert_eq!(out.code, 1, "{}", text(&out.stderr));
        assert!(
            text(&out.stderr).starts_with("atl-trim: Cargo.toml: file missing"),
            "{}",
            text(&out.stderr)
        );
        assert!(out.stdout.is_empty());
    }
}

// ------------------------------------------------------------------------------------------
// The Python-compatible helpers

mod pystr_test {
    use anthology_dev::atl_trim::pystr::*;

    #[test]
    fn strip_uses_pythons_whitespace() {
        // U+001C..U+001F, NBSP, U+3000 and U+2028 are whitespace for Python; U+200B is not.
        assert_eq!(strip("\u{1c}\u{a0} x y\u{3000}\u{2028}\n"), "x y");
        assert_eq!(lstrip("\u{1f} \tx "), "x ");
        assert_eq!(rstrip(" x\u{1e}\r\n"), " x");
        assert_eq!(strip("\u{200b}x"), "\u{200b}x");
        assert_eq!(strip(" \t\n"), "");
    }

    #[test]
    fn splitlines_knows_every_python_boundary() {
        assert_eq!(
            splitlines("a\r\nb\rc\x0bd\x0ce\x1cf\x1dg\x1eh\u{85}i\u{2028}j\u{2029}k\n"),
            ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"]
        );
        assert_eq!(splitlines("\n"), [""]);
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("a\n\n"), ["a", ""]);
        assert_eq!(splitlines("\r\n"), [""]);
        assert_eq!(splitlines("a\r"), ["a"]);
        assert_eq!(splitlines("a\r\r\nb"), ["a", "", "b"]);
    }

    #[test]
    fn first_line_is_the_first_line_of_a_non_blank_text() {
        assert_eq!(first_line("a\nb"), "a");
        assert_eq!(first_line("\n\nx"), "");
        assert_eq!(first_line("  "), "  ");
        assert_eq!(first_line(""), "");
        assert_eq!(first_line("old line\nmore\n"), "old line");
    }

    #[test]
    fn repr_follows_pythons_quote_choice_and_escapes() {
        assert_eq!(repr("fn a()"), "'fn a()'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("it's \"x\""), "'it\\'s \"x\"'");
        assert_eq!(repr("\"q\""), "'\"q\"'");
        assert_eq!(repr("a\\b\n\r\t"), "'a\\\\b\\n\\r\\t'");
        assert_eq!(repr("\u{1}\u{7f}"), "'\\x01\\x7f'");
        assert_eq!(repr("\u{1c}"), "'\\x1c'");
        assert_eq!(repr("caf\u{e9}"), "'caf\u{e9}'");
        assert_eq!(repr("\u{a0}"), "'\\xa0'");
        assert_eq!(repr("\u{200a}"), "'\\u200a'");
        assert_eq!(repr("\u{2028}"), "'\\u2028'");
        assert_eq!(repr("\u{1f600}"), "'\u{1f600}'");
        assert_eq!(repr("tab\there"), "'tab\\there'");
        assert_eq!(repr(""), "''");
    }
}

mod decl_test {
    use anthology_dev::atl_trim::decl::*;

    #[test]
    fn the_first_match_of_the_signature_pattern() {
        assert_eq!(
            first_declaration("pub fn foo(x: u8) {"),
            Some(("fn", "foo"))
        );
        assert_eq!(
            first_declaration("    pub(crate) struct Bar;"),
            Some(("struct", "Bar"))
        );
        assert_eq!(
            first_declaration("#[derive(Debug)] enum E {"),
            Some(("enum", "E"))
        );
        assert_eq!(
            first_declaration("pub mod telemetry;"),
            Some(("mod", "telemetry"))
        );
        assert_eq!(
            first_declaration("static FOO: u8 = 1;"),
            Some(("static", "FOO"))
        );
        assert_eq!(first_declaration("type T = u8;"), Some(("type", "T")));
        // The quirk: `const fn foo` yields the keyword `const` and the name `fn`.
        assert_eq!(
            first_declaration("pub const fn foo() {"),
            Some(("const", "fn"))
        );
        assert_eq!(first_declaration("const X: u8 = 1;"), Some(("const", "X")));
    }

    #[test]
    fn whitespace_between_keyword_and_name_is_any_whitespace() {
        assert_eq!(first_declaration("type\tT"), Some(("type", "T")));
        assert_eq!(first_declaration("mod\n  name {"), Some(("mod", "name")));
        assert_eq!(first_declaration("fn\u{a0}x"), Some(("fn", "x")));
        assert_eq!(first_declaration("fn\u{1c}x"), Some(("fn", "x")));
    }

    #[test]
    fn word_boundaries_in_the_signature() {
        assert_eq!(first_declaration("fnord x"), None);
        assert_eq!(first_declaration("prefn foo"), None);
        assert_eq!(first_declaration("fn_x foo"), None);
        assert_eq!(first_declaration("fn (x)"), None);
        assert_eq!(first_declaration("fn"), None);
        assert_eq!(first_declaration("let a = 1;"), None);
        // A failed attempt moves on to the next keyword.
        assert_eq!(first_declaration("fn (x) mod m"), Some(("mod", "m")));
        assert_eq!(
            first_declaration("fn caf\u{e9}()"),
            Some(("fn", "caf\u{e9}"))
        );
        assert_eq!(first_declaration("x.fn foo"), Some(("fn", "foo")));
    }

    #[test]
    fn presence_needs_a_boundary_at_both_ends() {
        assert!(declaration_present("fn gone(x: u16) {", "fn", "gone"));
        assert!(declaration_present("a\nfn\t\n gone\n", "fn", "gone"));
        assert!(declaration_present("fn  gone", "fn", "gone"));
        assert!(declaration_present(
            "fn other() {}\nfn gone() {}",
            "fn",
            "gone"
        ));
        // Python's `\w`: a combining mark beside the name is a boundary, a letter is not.
        assert!(declaration_present("fn gone\u{93e}() {}", "fn", "gone"));
        assert!(!declaration_present("fn gone\u{e9}() {}", "fn", "gone"));
        assert!(!declaration_present("\u{e9}fn gone() {}", "fn", "gone"));
        assert!(declaration_present("\u{93e}fn gone() {}", "fn", "gone"));
        // A name that is a prefix of a longer one is not the name.
        assert!(!declaration_present("fn gone_more() {}", "fn", "gone"));
        assert!(!declaration_present("fn gone2() {}", "fn", "gone"));
        assert!(!declaration_present("fn gonee() {}", "fn", "gone"));
        // The keyword needs a boundary before it, and whitespace after it.
        assert!(!declaration_present("xfn gone() {}", "fn", "gone"));
        assert!(!declaration_present("fngone", "fn", "gone"));
        assert!(!declaration_present("fn", "fn", "gone"));
        assert!(!declaration_present("struct gone", "fn", "gone"));
        assert!(declaration_present("pub const fn x()", "const", "fn"));
        assert!(declaration_present("fn gone", "fn", "gone"));
    }
}

// ------------------------------------------------------------------------------------------
// The CLI

mod cli_test {
    use super::*;

    #[test]
    fn a_root_that_is_not_a_directory() {
        let f = Fixture::new();
        let file = f.write("file", "x");
        let out = run_cli(&["atl-trim", "--root", file.to_str().unwrap()], "");
        assert_eq!(out.code, 1);
        assert_eq!(
            text(&out.stderr),
            format!("atl-trim: {}: not a directory: ''\n", file.display())
        );
        assert!(out.stdout.is_empty());
        // The default root is `atl` of the working directory: absent here.
        let out = run_cli(
            &["atl-trim", "--root", f.root.join("nope").to_str().unwrap()],
            "",
        );
        assert_eq!(out.code, 1);
    }

    #[test]
    fn usage_errors_exit_2() {
        for list in [
            &["atl-trim", "--bogus"][..],
            &["atl-trim", "stray"],
            &["atl-trim", "--lock-guard", "only-one"],
            &["atl-trim", "--removed-paths", "--trimmed-conflicts"],
            &["atl-trim", "--lock-guard", "a", "b", "--removed-paths"],
            &["atl-trim", "--root"],
        ] {
            let out = run_cli(list, "");
            assert_eq!(out.code, 2, "{list:?}: {}", text(&out.stderr));
            assert!(text(&out.stderr).contains("usage:"), "{list:?}");
            assert!(out.stdout.is_empty());
        }
        let out = run_cli(&["atl-trim", "--help"], "");
        assert_eq!(out.code, 0);
        assert!(text(&out.stdout).starts_with("usage:"));
    }

    /// `--removed-paths` of the Python at the table's commit: 77 lines.
    const PY_REMOVED: &str = "engine
plugin
proxy
examples
src/example_plugins.rs
src/core/call_context.rs
src/core/classify.rs
src/core/pipeline_cmd.rs
src/core/pipeline_config.rs
src/core/pipeline_log.rs
src/core/pipeline_remote.rs
src/core/plugins_cmd.rs
src/core/task_cmd.rs
src/hooks/posthook.rs
src/hooks/session_start.rs
src/hooks/hook_cmd
src/core/telemetry.rs
src/core/telemetry_cmd.rs
docs/TELEMETRY.md
docs/guide/resources/telemetry.md
src/hooks/init/antigravity.rs
src/hooks/init/codex.rs
src/hooks/init/copilot.rs
src/hooks/init/cursor.rs
src/hooks/init/droid.rs
src/hooks/init/gemini.rs
src/hooks/init/hermes.rs
src/hooks/init/instructions_agents.rs
src/hooks/init/opencode.rs
src/hooks/init/pi.rs
src/hooks/init/trae.rs
src/hooks/init/vibe.rs
README_es.md
README_fr.md
README_ja.md
README_ko.md
README_pt.md
README_zh.md
Formula
install.sh
INSTALL.md
.release-please-manifest.json
release-please-config.json
.claude
.github
.semgrep.yml
openclaw
scripts
hooks/antigravity
hooks/cline
hooks/codex
hooks/copilot
hooks/cursor
hooks/hermes
hooks/kilocode
hooks/opencode
hooks/pi
hooks/vibe
hooks/windsurf
docs/guide/pipelines
docs/guide/proxy
docs/guide/resources/agent-coverage.md
docs/guide/resources/editions.md
docs/guide/getting-started/supported-agents.md
tests/agent_coverage_doc_test.rs
tests/antigravity_hook_test.rs
tests/copilot_repo_hook_test.rs
tests/copilot_selfheal_test.rs
tests/fork_8_upstream_headers.rs
tests/hook_detection_multi_agent_test.rs
tests/init_agent_cursor_test.rs
tests/init_codex_uninstall_test.rs
tests/init_opencode_test.rs
tests/omp_init_test.rs
tests/trae_hook_test.rs
tests/hook_config_io_test.rs
tests/init_uninstall_exit_code_test.rs
";

    #[test]
    fn removed_paths_prints_exactly_the_pythons_list() {
        let out = run_cli(&["atl-trim", "--removed-paths"], "");
        assert_eq!(out.code, 0);
        assert_eq!(text(&out.stdout), PY_REMOVED);
        assert_eq!(PY_REMOVED.lines().count(), 77);
        assert!(out.stderr.is_empty());
    }

    #[test]
    fn removed_paths_keep_duplicates_and_order() {
        let removed = atl_trim::removed_paths();
        let expected: Vec<&str> = PY_REMOVED.lines().collect();
        assert_eq!(removed, expected);
    }

    #[test]
    fn trimmed_conflicts_reads_the_status_on_stdin() {
        let status = "DU atl/engine/src/lib.rs\r\n M atl/Cargo.toml\nDU atl/src/core/telemetry.rs\n\
            DU atl/src/core/telemetry_x.rs\nUU atl/engine\nDU \"atl/engine/a b\"\nDU other/engine/x\n";
        let out = run_cli(&["atl-trim", "--trimmed-conflicts"], status);
        assert_eq!(out.code, 0);
        assert_eq!(
            text(&out.stdout),
            "atl/engine/src/lib.rs\natl/src/core/telemetry.rs\n"
        );
        // Nothing to report prints nothing at all.
        let out = run_cli(&["atl-trim", "--trimmed-conflicts"], " M atl/Cargo.toml\n");
        assert_eq!(out.code, 0);
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
        let out = run_cli(&["atl-trim", "--trimmed-conflicts"], "");
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn an_io_failure_prints_the_path_and_exits_1() {
        let f = Fixture::new();
        // A file that is not UTF-8: Python dies with a traceback (status 1); we name the file.
        f.write("Cargo.toml", "x");
        fs::write(f.root.join("Cargo.toml"), [0xff, 0xfe, 0x00]).unwrap();
        let out = run_cli(&["atl-trim", "--root", f.root.to_str().unwrap()], "");
        assert_eq!(out.code, 1);
        assert!(
            text(&out.stderr).starts_with(&format!(
                "atl-trim: {}: ",
                f.root.join("Cargo.toml").display()
            )),
            "{}",
            text(&out.stderr)
        );
        let out = run_cli(
            &[
                "atl-trim",
                "--lock-guard",
                "/nonexistent/a",
                "/nonexistent/b",
            ],
            "",
        );
        assert_eq!(out.code, 1);
        assert!(text(&out.stderr).starts_with("atl-trim: /nonexistent/a: "));
    }

    #[test]
    fn the_binary_runs_the_same_command() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new(env!("CARGO_BIN_EXE_anthology-dev"))
            .args(["atl-trim", "--trimmed-conflicts"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"DU atl/plugin/x.rs\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(text(&output.stdout), "atl/plugin/x.rs\n");
    }
}

// ------------------------------------------------------------------------------------------
// The lockfile allowlist (`--lock-guard` with a `../crates/` path dependency)

mod allowlist_test {
    use super::*;

    /// `<tmp>/atl` (the `--root`), `<tmp>/crates/anthology-shell` (the path crate), and lockfiles.
    struct Repo {
        f: Fixture,
    }

    fn pkg(name: &str, version: &str, source: bool, deps: &[&str]) -> String {
        let mut out = format!("[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n");
        if source {
            out.push_str("source = \"registry+https://github.com/rust-lang/crates.io-index\"\n");
            out.push_str("checksum = \"00\"\n");
        }
        if !deps.is_empty() {
            out.push_str("dependencies = [\n");
            for dep in deps {
                out.push_str(&format!(" \"{dep}\",\n"));
            }
            out.push_str("]\n");
        }
        out.push('\n');
        out
    }

    fn lock(packages: &[String]) -> String {
        format!("version = 4\n\n{}", packages.concat())
    }

    impl Repo {
        fn new(manifest_deps: &str) -> Repo {
            let f = Fixture::new();
            f.write(
                "atl/Cargo.toml",
                &format!("[package]\nname = \"atl\"\nversion = \"1.0.0\"\n\n{manifest_deps}"),
            );
            f.write(
                "crates/anthology-shell/Cargo.toml",
                "[package]\nname = \"anthology-shell\" # the path crate\nversion = \"0.1.0\"\n",
            );
            Repo { f }
        }

        fn guard(&self, before: &str, after: &str) -> Output {
            let b = self.f.write("before.lock", before);
            let a = self.f.write("after.lock", after);
            let root = self.f.root.join("atl");
            run_cli(
                &[
                    "atl-trim",
                    "--lock-guard",
                    b.to_str().unwrap(),
                    a.to_str().unwrap(),
                    "--root",
                    root.to_str().unwrap(),
                ],
                "",
            )
        }
    }

    const SHELL_DEP: &str = "[dependencies]\nanyhow = \"1\"\nanthology-shell = { path = \"../crates/anthology-shell\" }\n";

    fn before_lock() -> String {
        lock(&[
            pkg("a", "1.0.0", true, &[]),
            pkg("b", "2.0.0", true, &["a"]),
            pkg("serde", "1.0.0", true, &[]),
            pkg("atl", "1.0.0", false, &["a"]),
        ])
    }

    fn after_with_closure() -> String {
        lock(&[
            pkg("a", "1.0.0", true, &[]),
            pkg("serde", "1.0.0", true, &[]),
            pkg("atl", "1.0.0", false, &["a", "anthology-shell"]),
            pkg(
                "anthology-shell",
                "0.1.0",
                false,
                &["fancy-regex", "serde 1.0.0"],
            ),
            pkg("fancy-regex", "0.20.2", true, &["bit-set"]),
            pkg(
                "bit-set",
                "0.8.0",
                true,
                &["bit-vec 0.8.0 (registry+https://x)"],
            ),
            pkg("bit-vec", "0.8.0", true, &[]),
        ])
    }

    #[test]
    fn a_path_dependency_and_its_closure_pass() {
        let repo = Repo::new(SHELL_DEP);
        let out = repo.guard(&before_lock(), &after_with_closure());
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        assert_eq!(
            text(&out.stdout),
            "atl-trim: Cargo.lock: 4 packages added by the ../crates/ allowlist: \
             anthology-shell 0.1.0, bit-set 0.8.0, bit-vec 0.8.0, fancy-regex 0.20.2\n\
             atl-trim: Cargo.lock: 1 packages pruned, none added or changed: b 2.0.0\n"
        );
        assert!(out.stderr.is_empty());
    }

    #[test]
    fn the_other_manifest_spellings_allow_the_same() {
        for deps in [
            "[dependencies.anthology-shell]\npath = \"../crates/anthology-shell\"\n",
            "[dependencies]\nanthology-shell.path = \"../crates/anthology-shell\"\n",
            "[target.'cfg(unix)'.dependencies]\nanthology-shell = { version = \"0.1\", path = '../crates/anthology-shell' }\n",
            "[dev-dependencies]\nanthology-shell = { path = \"../crates/anthology-shell\", features = [\"a\", \"b\"] } # c\n",
            // The key may differ from the crate name: the name comes from the crate's manifest.
            "[dependencies]\nshell = { path = \"../crates/anthology-shell\" }\n",
        ] {
            let repo = Repo::new(deps);
            let out = repo.guard(&before_lock(), &after_with_closure());
            assert_eq!(out.code, 0, "{deps}: {}", text(&out.stderr));
            assert!(text(&out.stdout).contains("4 packages added by"), "{deps}");
        }
    }

    #[test]
    fn an_unrelated_added_package_still_fails() {
        let repo = Repo::new(SHELL_DEP);
        let mut after = after_with_closure();
        after.push_str(&pkg("evil", "1.0.0", true, &[]));
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 1);
        assert!(out.stdout.is_empty());
        assert_eq!(
            text(&out.stderr),
            "atl-trim: Cargo.lock: the lockfile step added or changed packages: evil 1.0.0\n"
        );
    }

    #[test]
    fn a_closure_package_with_another_version_passes_only_inside_the_closure() {
        let repo = Repo::new(SHELL_DEP);
        // bit-vec changes 0.6.3 -> 0.8.0 and is in the closure: accepted.
        let before = format!("{}{}", before_lock(), pkg("bit-vec", "0.6.3", true, &[]));
        let out = repo.guard(&before, &after_with_closure());
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        // A version nobody in the closure asks for is not allowed, even for an allowed name.
        let mut after = after_with_closure();
        after.push_str(&pkg("bit-vec", "0.6.3", true, &[]));
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 1);
        assert_eq!(
            text(&out.stderr),
            "atl-trim: Cargo.lock: the lockfile step added or changed packages: bit-vec 0.6.3\n"
        );
        // A package outside the closure that changes version fails.
        let after = after_with_closure().replace(
            "name = \"a\"\nversion = \"1.0.0\"",
            "name = \"a\"\nversion = \"1.0.1\"",
        );
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 1);
        assert_eq!(
            text(&out.stderr),
            "atl-trim: Cargo.lock: the lockfile step added or changed packages: a 1.0.1\n"
        );
    }

    #[test]
    fn a_path_dependency_outside_the_crates_directory_allows_nothing() {
        for deps in [
            "[dependencies]\nanthology-shell = { path = \"../elsewhere/anthology-shell\" }\n",
            "[dependencies]\nanthology-shell = { path = \"../crates/../anthology-shell\" }\n",
            "[dependencies]\nanthology-shell = { path = \"crates/anthology-shell\" }\n",
            "[dependencies]\nanthology-shell = { path = \"../crates/\" }\n",
            "[dependencies]\nanthology-shell = \"0.1\"\n",
            "[package.metadata]\nanthology-shell = { path = \"../crates/anthology-shell\" }\n",
            "[[bin]]\nname = \"x\"\npath = \"../crates/anthology-shell\"\n",
        ] {
            let repo = Repo::new(deps);
            // The crates the paths point at exist for the first three.
            fs::create_dir_all(repo.f.root.join("elsewhere/anthology-shell")).unwrap();
            let out = repo.guard(&before_lock(), &after_with_closure());
            assert_eq!(out.code, 1, "{deps}");
            assert!(
                text(&out.stderr).starts_with(
                    "atl-trim: Cargo.lock: the lockfile step added or changed packages: "
                ),
                "{deps}: {}",
                text(&out.stderr)
            );
        }
    }

    #[test]
    fn a_missing_crate_manifest_allows_nothing_and_a_nameless_one_allows_nothing() {
        let repo = Repo::new(SHELL_DEP);
        fs::remove_file(repo.f.root.join("crates/anthology-shell/Cargo.toml")).unwrap();
        let out = repo.guard(&before_lock(), &after_with_closure());
        assert_eq!(out.code, 1);
        // A manifest without a readable `[package]` name allows nothing (no fallback to the
        // dependency key, which a renamed dependency could use to name the root package).
        repo.f
            .write("crates/anthology-shell/Cargo.toml", "[lib]\npath = \"x\"\n");
        let out = repo.guard(&before_lock(), &after_with_closure());
        assert_eq!(out.code, 1, "{}", text(&out.stderr));
    }

    #[test]
    fn a_registry_package_of_the_same_name_is_not_the_path_package() {
        // `anthology-shell` with a source is a registry crate: it seeds nothing.
        let repo = Repo::new(SHELL_DEP);
        let after = lock(&[
            pkg("a", "1.0.0", true, &[]),
            pkg("anthology-shell", "0.1.0", true, &["bit-vec"]),
            pkg("bit-vec", "0.8.0", true, &[]),
        ]);
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 1);
    }

    #[test]
    fn without_a_path_dependency_the_outputs_are_the_pythons() {
        let repo = Repo::new("[dependencies]\nanyhow = \"1\"\n");
        // Pruning only: the one Python line, nothing else.
        let after = lock(&[
            pkg("a", "1.0.0", true, &[]),
            pkg("serde", "1.0.0", true, &[]),
        ]);
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 0);
        assert_eq!(
            text(&out.stdout),
            "atl-trim: Cargo.lock: 2 packages pruned, none added or changed: atl 1.0.0, b 2.0.0\n"
        );
        // Nothing pruned: nothing printed.
        let out = repo.guard(&before_lock(), &before_lock());
        assert_eq!((out.code, out.stdout.len(), out.stderr.len()), (0, 0, 0));
        // An addition: the Python message, sorted.
        let after = format!(
            "{}{}{}",
            before_lock(),
            pkg("z", "1", true, &[]),
            pkg("c", "3", true, &[])
        );
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 1);
        assert_eq!(
            text(&out.stderr),
            "atl-trim: Cargo.lock: the lockfile step added or changed packages: c 3, z 1\n"
        );
        // No Cargo.toml under the root at all: the same.
        fs::remove_file(repo.f.root.join("atl/Cargo.toml")).unwrap();
        let out = repo.guard(&before_lock(), &after);
        assert_eq!(out.code, 1);
    }

    #[test]
    fn the_default_root_is_atl_of_the_working_directory() {
        // Without `--root` the guard reads `atl/Cargo.toml` of the working directory: none
        // exists here, so it is the plain Python guard.
        let f = Fixture::new();
        let b = f.write("b.lock", &before_lock());
        let a = f.write("a.lock", &after_with_closure());
        let out = run_cli(
            &[
                "atl-trim",
                "--lock-guard",
                b.to_str().unwrap(),
                a.to_str().unwrap(),
            ],
            "",
        );
        assert_eq!(out.code, 1, "{}", text(&out.stderr));
    }
}

// ------------------------------------------------------------------------------------------
// Whole trims

/// Every file and symlink under `dir`: relative path -> content (a link as `-> target`).
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap().display().to_string();
            let kind = entry.file_type().unwrap();
            if kind.is_symlink() {
                let target = fs::read_link(&path).unwrap();
                out.insert(rel, format!("-> {}", target.display()).into_bytes());
            } else if kind.is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                walk(base, &path, out);
            } else {
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &dest).unwrap();
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

mod real_tree_test {
    use super::*;

    #[test]
    fn the_checked_in_atl_tree_is_a_fixed_point() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source = repo.join("atl");
        assert!(source.is_dir(), "{}", source.display());
        let f = Fixture::new();
        let copy = f.root.join("atl");
        copy_tree(&source, &copy);
        let out = run_cli(&["atl-trim", "--root", copy.to_str().unwrap()], "");
        assert_eq!(text(&out.stdout), "no changes\n", "{}", text(&out.stderr));
        assert_eq!((out.code, out.stderr.len()), (0, 0));
        assert_eq!(snapshot(&copy), snapshot(&source));
    }
}

/// A synthetic pristine tree for the real operations: for each operation in order, the text it
/// needs is added to the tree when the simulation does not already hold it (a file for `rm`, the
/// anchor for `sub`/`cut`, a block for `drop`). Returns the pristine tree and what the trim of it
/// reports.
fn seed(sim_root: &Path, seeds: &mut BTreeMap<String, String>, path: &str, add: &str) {
    let full = sim_root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&full).unwrap_or_default();
    text.push_str(add);
    fs::write(&full, text).unwrap();
    seeds.entry(path.to_string()).or_default().push_str(add);
}

fn synthetic_pristine(into: &Path) -> Vec<String> {
    let sim_dir = tempfile::tempdir().unwrap();
    let mut sim = Trim::new(sim_dir.path());
    let mut seeds: BTreeMap<String, String> = BTreeMap::new();
    for op in atl_trim::operations() {
        match &op {
            Op::Rm(paths) => {
                for path in *paths {
                    if !sim_dir.path().join(path).exists() {
                        seed(sim_dir.path(), &mut seeds, path, "x\n");
                    }
                }
            }
            Op::Sub { path, old, .. } | Op::Cut { path, old, .. } => {
                let have = fs::read_to_string(sim_dir.path().join(path)).unwrap_or_default();
                if !have.contains(old) {
                    seed(sim_dir.path(), &mut seeds, path, &format!("{old}\n"));
                }
            }
            Op::Drop { path, sig, .. } => {
                let have = fs::read_to_string(sim_dir.path().join(path)).unwrap_or_default();
                if !have.split('\n').any(|line| line == *sig) {
                    let indent = &sig[..sig.len() - sig.trim_start().len()];
                    let block = if sig.trim_end().ends_with([';', ',']) {
                        format!("{sig}\n")
                    } else {
                        format!("{sig}\n{indent}}}\n")
                    };
                    seed(sim_dir.path(), &mut seeds, path, &block);
                }
            }
        }
        sim.apply(&op)
            .unwrap_or_else(|e| panic!("synthetic tree: {e} at {op:?}"));
    }
    for (path, content) in &seeds {
        let full = into.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    sim.changed
}

mod synthetic_tree_test {
    use super::*;

    #[test]
    fn a_second_run_after_the_first_trim_prints_no_changes() {
        let f = Fixture::new();
        let expected = synthetic_pristine(&f.root);
        assert!(expected.len() > 100, "{}", expected.len());
        let root = f.root.to_str().unwrap();
        let first = run_cli(&["atl-trim", "--root", root], "");
        assert_eq!(first.code, 0, "{}", text(&first.stderr));
        assert_eq!(text(&first.stdout), format!("{}\n", expected.join("\n")));
        let after_first = snapshot(&f.root);
        let second = run_cli(&["atl-trim", "--root", root], "");
        assert_eq!(second.code, 0, "{}", text(&second.stderr));
        assert_eq!(text(&second.stdout), "no changes\n");
        assert_eq!(snapshot(&f.root), after_first);
    }

    #[test]
    fn a_hand_made_trim_is_idempotent_and_stops_on_a_changed_anchor() {
        let f = Fixture::new();
        f.write("a/x.txt", "x");
        f.write(
            "f.rs",
            "// head\nfn gone() {\n}\nlet v = 1;\nstep one;\nstep two;\n",
        );
        let ops = [
            Op::Rm(&["a"]),
            Op::Drop {
                path: "f.rs",
                sig: "fn gone() {",
                gone: None,
            },
            Op::Sub {
                path: "f.rs",
                old: "let v = 1;\n",
                new: "let v = 2;\n",
                done: None,
            },
            Op::Cut {
                path: "f.rs",
                old: "step one;\n",
                gone: "step one",
            },
            Op::Sub {
                path: "f.rs",
                old: "step two;\n",
                new: "step 2;\n",
                done: Some("step 2;"),
            },
        ];
        let mut first = f.trim();
        first.apply_all(&ops).unwrap();
        assert_eq!(
            first.changed,
            [
                "rm a",
                "drop f.rs fn gone() {",
                "edit f.rs",
                "edit f.rs",
                "edit f.rs"
            ]
        );
        assert_eq!(f.read("f.rs"), "let v = 2;\nstep 2;\n");
        let mut second = f.trim();
        second.apply_all(&ops).unwrap();
        assert!(second.changed.is_empty());
        // An upstream edit inside an anchored region stops the trim.
        f.write("f.rs", "// head\nlet v = 3;\nstep 2;\n");
        let mut third = f.trim();
        let message = err_text(third.apply_all(&ops));
        assert_eq!(message, "f.rs: anchor missing: 'let v = 1;'");
    }
}

mod run_in_test {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn relative_paths_are_taken_from_the_given_folder_and_messages_show_them_as_given() {
        let dir = tempfile::tempdir().unwrap();
        let out = atl_trim::run_in(dir.path(), &args(&["--root", "atl"]), &mut "".as_bytes());
        assert_eq!(out.code, 1);
        assert_eq!(text(&out.stderr), "atl-trim: atl: not a directory: ''\n");
        fs::create_dir(dir.path().join("atl")).unwrap();
        let out = atl_trim::run_in(dir.path(), &args(&["--root", "atl"]), &mut "".as_bytes());
        assert_eq!(out.code, 1);
        assert!(
            text(&out.stderr).starts_with("atl-trim: Cargo.toml: file missing: "),
            "{}",
            text(&out.stderr)
        );
        fs::write(dir.path().join("a.lock"), "").unwrap();
        fs::write(dir.path().join("b.lock"), "").unwrap();
        let out = atl_trim::run_in(
            dir.path(),
            &args(&["--lock-guard", "a.lock", "b.lock"]),
            &mut "".as_bytes(),
        );
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        assert!(out.stdout.is_empty());
    }
}
