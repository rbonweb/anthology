//! Scanner suppressions: files and comments that steer secret and code scanners (Task 10).
//!
//! A repo can tell a scanner to look away: an ignore file, a rule override, a `nosemgrep` comment.
//! None of it is proof of bad intent, so each is LOW; it is a place to look. The scan itself never
//! reads these files to configure a tool (the tool runner passes its own config).

use super::agent::{clip, name_of};
use crate::files::line_of;
use crate::{FileCtx, Finding, Group, Severity};

/// File names (at any depth) that configure or silence a scanner.
const CONFIG_NAMES: &[&str] = &[
    ".gitleaks.toml",
    ".gitleaksignore",
    ".semgrepignore",
    "zizmor.yml",
    "osv-scanner.toml",
    ".trufflehogignore",
];

/// Inline markers that silence a scanner on one line. Split so this source does not carry them.
const INLINE_MARKERS: [&str; 2] = [concat!("gitleaks", ":allow"), concat!("no", "semgrep")];

/// Whether `path` is a scanner config or ignore file.
fn is_scanner_config(path: &str) -> bool {
    // ASCII case is ignored: macOS and Windows open `.GitLeaks.toml` as `.gitleaks.toml`.
    let path = path.to_ascii_lowercase();
    let name = name_of(&path);
    CONFIG_NAMES.contains(&name)
        || name.starts_with(".guarddog")
        || path == ".atl/filters.toml"
        || path.ends_with("/.atl/filters.toml")
}

pub(crate) fn check(f: &FileCtx, out: &mut Vec<Finding>) {
    if is_scanner_config(f.path) {
        out.push(Finding {
            severity: Severity::Low,
            group: Group::Scanner,
            rule: "scanner.config-file".to_string(),
            path: f.path.to_string(),
            line: 0,
            why: "This file configures or silences a scanner, so a scan that reads it may miss things.".to_string(),
            seen: name_of(f.path).to_string(),
            action: "Read it to see what it hides; this scan ignores it.".to_string(),
            count: 1,
        });
        return;
    }
    let Some(text) = f.text else {
        return;
    };
    let mut first: Option<usize> = None;
    let mut count = 0;
    for marker in INLINE_MARKERS {
        for (at, _) in text.match_indices(marker) {
            count += 1;
            first = Some(first.map_or(at, |p| p.min(at)));
        }
    }
    let Some(at) = first else {
        return;
    };
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    out.push(Finding {
        severity: Severity::Low,
        group: Group::Scanner,
        rule: "scanner.inline-suppress".to_string(),
        path: f.path.to_string(),
        line: line_of(text, at),
        why: "A comment tells a scanner to skip a line, so that line was not checked for secrets or bugs.".to_string(),
        seen: clip(text[line_start..line_end].trim()),
        action: "Read the line the comment silences.".to_string(),
        count,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, Group, Severity};

    fn run_kind(path: &str, text: &str, kind: FileKind) -> Vec<Finding> {
        let f = FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        out
    }

    fn run(path: &str, text: &str) -> Vec<Finding> {
        run_kind(path, text, FileKind::Config)
    }

    /// A suppression marker, split so this source does not carry one.
    const GITLEAKS_ALLOW: &str = concat!("gitleaks", ":allow");
    const NO_SEMGREP: &str = concat!("no", "semgrep");

    #[test]
    fn scanner_config_files_are_low() {
        for path in [
            ".gitleaks.toml",
            ".gitleaksignore",
            ".semgrepignore",
            "zizmor.yml",
            ".github/zizmor.yml",
            "osv-scanner.toml",
            ".guarddog.yml",
            ".guarddogignore",
            ".trufflehogignore",
            ".atl/filters.toml",
            "sub/dir/.gitleaks.toml",
            "pkg/.atl/filters.toml",
        ] {
            let out = run(path, "x = 1\n");
            assert_eq!(out.len(), 1, "{path}");
            assert_eq!(out[0].rule, "scanner.config-file", "{path}");
            assert_eq!(out[0].severity, Severity::Low, "{path}");
            assert_eq!(out[0].group, Group::Scanner, "{path}");
            assert_eq!(out[0].path, path);
            assert_eq!(out[0].line, 0, "{path}");
            assert_eq!(out[0].count, 1, "{path}");
        }
    }

    #[test]
    fn case_changes_do_not_hide_scanner_config() {
        for path in [
            ".GitLeaks.toml",
            ".GITLEAKSIGNORE",
            "Zizmor.YML",
            ".github/ZIZMOR.yml",
            "OSV-Scanner.toml",
            ".GuardDog.yml",
            ".ATL/Filters.TOML",
            "pkg/.Atl/filters.toml",
        ] {
            let out = run(path, "x = 1\n");
            assert_eq!(out.len(), 1, "{path}");
            assert_eq!(out[0].rule, "scanner.config-file", "{path}");
            assert_eq!(out[0].path, path);
        }
        assert!(run("Docs/Filters.toml", "x = 1\n").is_empty());
    }

    #[test]
    fn similar_names_are_not_scanner_config() {
        for path in [
            "gitleaks.md",
            ".gitleaks.toml.bak",
            "docs/gitleaks.toml",
            "filters.toml",
            ".atl/other.toml",
            "zizmor.yaml",
            "my.guarddog",
            "src/osv-scanner.rs",
            ".semgrepignore.md",
            "README.md",
        ] {
            assert!(run(path, "x = 1\n").is_empty(), "{path}");
        }
    }

    #[test]
    fn inline_suppressions_are_low_with_a_count_and_the_first_line() {
        let text = format!(
            "let a = 1;\nlet key = \"x\"; // {GITLEAKS_ALLOW}\nlet b = 2;\n# {NO_SEMGREP}: rule\n// {GITLEAKS_ALLOW}\n"
        );
        let out = run_kind("src/app.js", &text, FileKind::Code(crate::Lang::Js));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "scanner.inline-suppress");
        assert_eq!(out[0].severity, Severity::Low);
        assert_eq!(out[0].group, Group::Scanner);
        assert_eq!(out[0].line, 2);
        assert_eq!(out[0].count, 3);
        assert_eq!(out[0].seen, format!("let key = \"x\"; // {GITLEAKS_ALLOW}"));
    }

    #[test]
    fn each_inline_marker_alone_is_found() {
        for marker in [GITLEAKS_ALLOW, NO_SEMGREP] {
            let out = run("notes.txt", &format!("a\nb {marker}\n"));
            assert_eq!(out.len(), 1, "{marker}");
            assert_eq!(out[0].line, 2);
        }
    }

    #[test]
    fn a_scanner_config_that_also_has_a_marker_is_one_finding() {
        let out = run(".gitleaks.toml", &format!("# {GITLEAKS_ALLOW}\n"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "scanner.config-file");
    }

    #[test]
    fn a_file_without_a_marker_is_clean() {
        let text = "fn main() { println!(\"gitleaks and semgrep are tools\"); }\n";
        assert!(run_kind("src/main.rs", text, FileKind::Code(crate::Lang::Rs)).is_empty());
    }

    #[test]
    fn a_file_that_is_not_text_gives_nothing_for_markers() {
        let f = FileCtx {
            path: "blob.bin",
            bytes: GITLEAKS_ALLOW.as_bytes(),
            text: None,
            kind: FileKind::Binary,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_config_file_that_is_not_text_is_still_reported() {
        let f = FileCtx {
            path: ".gitleaks.toml",
            bytes: &[0xff, 0xfe, 0x00],
            text: None,
            kind: FileKind::Binary,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        assert_eq!(out.len(), 1);
    }
}
