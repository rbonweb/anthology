//! Port of `tests/test_rtk_claude_only.py`: the rtk subtree is the Apache-2.0 edition, Claude Code
//! only, with no telemetry. An allowlist: an upstream sync that adds an agent, a dependency or a
//! re-added file fails here, and the answer is to extend the trim, not this list.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use regex::Regex;
use repo_tests::repo;
use repo_tests::yaml;

// Build output and OS debris that are never tracked: skipped by the tree allowlists.
const UNTRACKED_DEBRIS: [&str; 2] = ["target", ".DS_Store"];
const TOP_LEVEL: [&str; 22] = [
    ".cargo",
    ".git-blame-ignore-revs",
    ".gitattributes",
    ".gitignore",
    ".rtk",
    "CHANGELOG.md",
    "CLAUDE.md",
    "CONTRIBUTING.md",
    "Cargo.lock",
    "Cargo.toml",
    "DISCLAIMER.md",
    "LICENSE",
    "README.md",
    "SECURITY.md",
    "build.rs",
    "docs",
    "hooks",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "src",
    "tests",
    "text",
];
const HOOKS_ENTRIES: [&str; 5] = [
    "README.md",
    "claude",
    "rtk-awareness-full.md",
    "rtk-awareness-high.md",
    "rtk-awareness.md",
];
const TRIM_SH: &str = "scripts/rtk-trim.sh";
const NET_TEST: &str = "scripts/rtk-net-test.sh";
const SYNC: &str = "scripts/sync-rtk.sh";

const INIT_MODULES: [&str; 3] = ["agents_md.rs", "claude.rs", "mod.rs"];

fn cargo_deps_expected() -> BTreeMap<String, Vec<String>> {
    let table: [(&str, &[&str]); 6] = [
        (
            "dependencies",
            &[
                "anyhow",
                "base16ct",
                "chrono",
                "clap",
                "codepage",
                "colored",
                "crossterm",
                "dirs",
                "encoding_rs",
                "flate2",
                "ignore",
                "oem_cp",
                "quick-xml",
                "ratatui",
                "regex",
                "rtk-text",
                "rusqlite",
                "serde",
                "serde_json",
                "sha2",
                "tempfile",
                "toml",
                "toml_edit",
                "ureq",
                "walkdir",
                "which",
            ],
        ),
        ("target.'cfg(unix)'.dependencies", &["libc"]),
        ("target.'cfg(windows)'.dependencies", &["windows-sys"]),
        (
            "target.'cfg(target_env = \"musl\")'.dependencies",
            &["mimalloc"],
        ),
        ("build-dependencies", &["toml"]),
        ("dev-dependencies", &["temp-env"]),
    ];
    deps_map(&table)
}

fn deps_map(table: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    table
        .iter()
        .map(|(k, v)| {
            (
                (*k).to_string(),
                v.iter().map(|s| (*s).to_string()).collect(),
            )
        })
        .collect()
}

const CHECK_CFG: &str = "unexpected_cfgs = { level = \"deny\", \
     check-cfg = ['cfg(feature, values(\"engine\", \"example-plugins\"))'] }";

const FORK_TESTS: [&str; 8] = [
    "fork_1_auto_allow_hook.rs",
    "fork_2_read_minimal_default.rs",
    "fork_3_node_filter.rs",
    "fork_4_python_filter.rs",
    "fork_5_cd_chain_rewrite.rs",
    "fork_6_biome_lint.rs",
    "fork_7_opencode_run.rs",
    "fork_9_awareness_exact.rs",
];

const OTHER_INIT: [&str; 12] = [
    "antigravity",
    "codex",
    "copilot",
    "cursor",
    "droid",
    "gemini",
    "hermes",
    "instructions_agents",
    "opencode",
    "pi",
    "trae",
    "vibe",
];
const OTHER_HOOKS: [&str; 11] = [
    "antigravity",
    "cline",
    "codex",
    "copilot",
    "cursor",
    "hermes",
    "kilocode",
    "opencode",
    "pi",
    "vibe",
    "windsurf",
];
const REMOVED_TESTS: [&str; 13] = [
    "agent_coverage_doc_test",
    "antigravity_hook_test",
    "copilot_repo_hook_test",
    "copilot_selfheal_test",
    "fork_8_upstream_headers",
    "hook_detection_multi_agent_test",
    "init_agent_cursor_test",
    "init_codex_uninstall_test",
    "init_opencode_test",
    "omp_init_test",
    "trae_hook_test",
    "hook_config_io_test",
    "init_uninstall_exit_code_test",
];

fn removed_paths() -> Vec<String> {
    let mut v: Vec<String> = [
        // BUSL-1.1 crates and what only they serve
        "engine",
        "plugin",
        "proxy",
        "examples",
        "src/example_plugins.rs",
        "src/core/call_context.rs",
        "src/core/classify.rs",
        "src/core/pipeline_cmd.rs",
        "src/core/pipeline_config.rs",
        "src/core/pipeline_log.rs",
        "src/core/pipeline_remote.rs",
        "src/core/plugins_cmd.rs",
        "src/core/task_cmd.rs",
        "src/hooks/posthook.rs",
        "src/hooks/session_start.rs",
        "src/hooks/hook_cmd",
        // telemetry
        "src/core/telemetry.rs",
        "src/core/telemetry_cmd.rs",
        "docs/TELEMETRY.md",
        "docs/guide/resources/telemetry.md",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    // other agents
    v.extend(OTHER_INIT.iter().map(|m| format!("src/hooks/init/{m}.rs")));
    v.extend(OTHER_HOOKS.iter().map(|h| format!("hooks/{h}")));
    // packaging, translations, upstream tooling
    v.extend(
        [
            "README_es.md",
            "README_fr.md",
            "README_ja.md",
            "README_ko.md",
            "README_pt.md",
            "README_zh.md",
            "Formula",
            "install.sh",
            "INSTALL.md",
            ".release-please-manifest.json",
            "release-please-config.json",
            ".claude",
            ".github",
            ".semgrep.yml",
            "openclaw",
            "scripts",
            // docs of removed features
            "docs/guide/pipelines",
            "docs/guide/proxy",
            "docs/guide/resources/agent-coverage.md",
            "docs/guide/resources/editions.md",
            "docs/guide/getting-started/supported-agents.md",
        ]
        .iter()
        .map(|s| (*s).to_string()),
    );
    // tests of removed code
    v.extend(REMOVED_TESTS.iter().map(|t| format!("tests/{t}.rs")));
    v
}

// ---- the stdlib Cargo.toml line scan (Python 3.9 has no tomllib) ----

const DEP_SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
const KEY_PART: &str = r#"(?:"[^"]*"|'[^']*'|[A-Za-z0-9_-]+)"#;

/// The parts of a dotted TOML key or table name, raw (quotes kept), split outside quotes.
fn dotted(name: &str) -> Vec<String> {
    let (mut parts, mut cur, mut quote) = (Vec::new(), String::new(), None::<char>);
    for ch in name.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if ch == q {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
            cur.push(ch);
        } else if ch == '.' {
            parts.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

fn unquote(part: &str) -> String {
    let b = part.as_bytes();
    if b.len() > 1 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        part[1..part.len() - 1].to_string()
    } else {
        part.to_string()
    }
}

/// Net `[`/`{` nesting a line opens, outside strings and comments.
fn depth_change(line: &str) -> i32 {
    let (mut depth, mut quote) = (0, None::<char>);
    for ch in line.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '#' {
            break;
        } else if ch == '[' || ch == '{' {
            depth += 1;
        } else if ch == ']' || ch == '}' {
            depth -= 1;
        }
    }
    depth
}

/// `{section: sorted dependency names}` for every dependency section of a Cargo.toml: a line scan
/// that tracks bracket nesting, so only top-level lines count. Dependencies are the keys of a
/// `[*dependencies]` table (indented, quoted or dotted keys: the first part is the name) and the
/// `foo` of a `[*dependencies.foo]` sub-table, filed under its parent section.
fn cargo_deps(path: &Path) -> BTreeMap<String, Vec<String>> {
    let header_re = Regex::new(r"^\s*\[\[?(.+?)\]\]?\s*(#.*)?$").unwrap();
    let key_re = Regex::new(&format!(r"^\s*({KEY_PART})\s*(?:\.\s*{KEY_PART}\s*)*=")).unwrap();
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut section, mut depth) = (None::<String>, 0);
    for line in text.lines() {
        if depth == 0 {
            if let Some(header) = header_re.captures(line) {
                let parts = dotted(header[1].trim());
                section = None;
                let last = parts.last().unwrap().as_str();
                if DEP_SECTIONS.contains(&last) {
                    section = Some(parts.join("."));
                } else if parts.len() > 1 && DEP_SECTIONS.contains(&parts[parts.len() - 2].as_str())
                {
                    out.entry(parts[..parts.len() - 1].join("."))
                        .or_default()
                        .push(unquote(last));
                }
                continue;
            }
            if let Some(sec) = &section
                && let Some(key) = key_re.captures(line)
            {
                out.entry(sec.clone()).or_default().push(unquote(&key[1]));
            }
        }
        depth += depth_change(line);
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// The non-blank, non-comment lines of the TOML section `[name]`.
fn section_lines(rel: &str, name: &str) -> Vec<String> {
    let section = Regex::new(r"^\[([^\]]+)\]\s*(#.*)?$").unwrap();
    let (mut lines, mut inside) = (Vec::new(), false);
    for line in repo::read(rel).lines() {
        if let Some(m) = section.captures(line) {
            inside = m[1].trim() == name;
            continue;
        }
        if inside && !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            lines.push(line.to_string());
        }
    }
    lines
}

fn rtk(rel: &str) -> String {
    format!("rtk/{rel}")
}

fn matches(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(text)
}

fn is_executable(rel: &str) -> bool {
    std::fs::metadata(repo::path(rel)).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// `p.name for p in directory.glob(pattern)` for the patterns `<prefix>*<suffix>`, sorted.
fn glob_names(rel: &str, prefix: &str, suffix: &str) -> Vec<String> {
    repo::list_dir(rel)
        .into_iter()
        .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
        .collect()
}

/// `any(files for _, _, files in os.walk(dir))`: a file somewhere below. A symlink to a
/// directory is not walked (as `os.walk` with the default `followlinks=False`).
fn walk_has_files(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in read.flatten() {
        let p = e.path();
        if !p.is_dir() {
            return true;
        }
        if e.file_type().is_ok_and(|t| !t.is_symlink()) && walk_has_files(&p) {
            return true;
        }
    }
    false
}

/// The entries of `rel` git would see: no build output or OS debris, no empty dirs.
fn entries(rel: &str) -> Vec<String> {
    repo::list_dir(rel)
        .into_iter()
        .filter(|n| {
            let p = repo::path(&format!("{rel}/{n}"));
            !UNTRACKED_DEBRIS.contains(&n.as_str()) && (!p.is_dir() || walk_has_files(&p))
        })
        .collect()
}

mod init_modules_test {
    use super::*;

    #[test]
    fn test_only_claude_and_its_helpers() {
        let init = rtk("src/hooks/init");
        assert_eq!(glob_names(&init, "", ".rs"), INIT_MODULES);
        let dirs: Vec<String> = repo::list_dir(&init)
            .into_iter()
            .filter(|n| repo::path(&format!("{init}/{n}")).is_dir())
            .collect();
        assert!(dirs.is_empty(), "{dirs:?}");
    }
}

/// `cargo_deps` sees a dependency in every form a manifest can declare it.
mod cargo_parser_test {
    use super::*;

    fn deps(text: &str) -> BTreeMap<String, Vec<String>> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cargo.toml");
        std::fs::write(&path, text).unwrap();
        cargo_deps(&path)
    }

    #[test]
    fn test_plain_dotted_and_multi_line_values() {
        assert_eq!(
            deps(concat!(
                "[dependencies]\nanyhow = \"1\"\nrtk-text = { path = \"text\" }\n",
                "serde.workspace = true\n",
                "windows-sys = { version = \"0.61\", features = [\n    \"A\",\n    \"B\",\n] }\n"
            )),
            deps_map(&[(
                "dependencies",
                &["anyhow", "rtk-text", "serde", "windows-sys"]
            )])
        );
    }

    #[test]
    fn test_sub_table_headers() {
        assert_eq!(
            deps(concat!(
                "[dependencies]\nanyhow = \"1\"\n\n[dependencies.foo]\nversion = \"1\"\n",
                "features = [\"x\"]\n",
                "[target.'cfg(unix)'.dependencies.bar]\nversion = \"2\"\n",
                "[dev-dependencies.\"baz\"]\nversion = \"3\"\n"
            )),
            deps_map(&[
                ("dependencies", &["anyhow", "foo"]),
                ("target.'cfg(unix)'.dependencies", &["bar"]),
                ("dev-dependencies", &["baz"]),
            ])
        );
    }

    #[test]
    fn test_indented_and_quoted_keys() {
        assert_eq!(
            deps(concat!(
                "[build-dependencies]\n  indented = \"1\"\n\"quoted\" = \"1\"\n",
                "'single'.version = \"1\"\n",
                "  [ target.'cfg(windows)'.dependencies ]\nwin = \"1\"\n"
            )),
            deps_map(&[
                ("build-dependencies", &["indented", "quoted", "single"]),
                ("target.'cfg(windows)'.dependencies", &["win"]),
            ])
        );
    }

    #[test]
    fn test_other_tables_and_nested_arrays_are_not_dependencies() {
        assert_eq!(
            deps(concat!(
                "[package]\nname = \"rtk\"\n[package.metadata.deb]\nassets = [\n",
                "    [\"target/release/rtk\", \"usr/bin/\", \"755\"]\n]\n[[bin]]\nname = \"x\"\n",
                "[dependencies]\nok = \"1\"\n[features]\nfoo = [\"dep:ok\"]\n"
            )),
            deps_map(&[("dependencies", &["ok"])])
        );
    }
}

mod cargo_dependencies_test {
    use super::*;

    #[test]
    fn test_cli_dependencies_per_section() {
        assert_eq!(
            cargo_deps(&repo::path(&rtk("Cargo.toml"))),
            cargo_deps_expected()
        );
    }

    #[test]
    fn test_text_crate_dependencies() {
        assert_eq!(
            cargo_deps(&repo::path(&rtk("text/Cargo.toml"))),
            deps_map(&[("dependencies", &["regex"])])
        );
    }

    #[test]
    fn test_workspace_is_the_cli_and_the_text_crate() {
        assert_eq!(
            section_lines(&rtk("Cargo.toml"), "workspace"),
            [r#"members = [".", "text"]"#]
        );
    }

    #[test]
    fn test_no_features() {
        assert!(section_lines(&rtk("Cargo.toml"), "features").is_empty());
    }

    #[test]
    fn test_only_two_manifests() {
        fn walk(dir: &Path, rel: &str, found: &mut Vec<String>) {
            let mut files = Vec::new();
            let mut dirs = Vec::new();
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let p = e.path();
                if p.is_dir() {
                    // the top level prunes build output and the git directory
                    let pruned = rel.is_empty() && (name == "target" || name == ".git");
                    if !pruned && e.file_type().is_ok_and(|t| !t.is_symlink()) {
                        dirs.push((name, p));
                    }
                } else {
                    files.push(name);
                }
            }
            if files.iter().any(|f| f == "Cargo.toml") {
                found.push(if rel.is_empty() {
                    "Cargo.toml".to_string()
                } else {
                    format!("{rel}/Cargo.toml")
                });
            }
            for (name, p) in dirs {
                let sub = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                walk(&p, &sub, found);
            }
        }
        let mut found = Vec::new();
        walk(&repo::path("rtk"), "", &mut found);
        found.sort();
        assert_eq!(found, ["Cargo.toml", "text/Cargo.toml"]);
    }

    #[test]
    fn test_dead_engine_cfg_is_declared_and_dead_code_allowed() {
        let lints = section_lines(&rtk("Cargo.toml"), "lints.rust");
        assert!(lints.iter().any(|l| l == CHECK_CFG), "{lints:?}");
        assert!(
            lints.iter().any(|l| l == r#"dead_code = "allow""#),
            "{lints:?}"
        );
        assert!(
            lints.iter().any(|l| l == r#"warnings = "deny""#),
            "{lints:?}"
        );
    }

    #[test]
    fn test_no_busl_license_in_the_tree() {
        for rel in ["rtk/Cargo.toml", "rtk/text/Cargo.toml"] {
            assert!(
                !matches(r#"(?m)^license\s*=\s*"BUSL"#, &repo::read(rel)),
                "{rel}"
            );
        }
    }
}

mod no_telemetry_test {
    use super::*;

    #[test]
    fn test_no_telemetry_sources() {
        fn walk(dir: &Path, found: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if e.file_name().to_string_lossy().starts_with("telemetry") {
                    found.push(p.display().to_string());
                }
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    walk(&p, found);
                }
            }
        }
        let mut found = Vec::new();
        walk(&repo::path("rtk/src"), &mut found);
        found.sort();
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn test_no_telemetry_module_command_or_ping() {
        assert!(!matches(
            r"\bmod telemetry",
            &repo::read("rtk/src/core/mod.rs")
        ));
        let lib = repo::read("rtk/src/lib.rs");
        assert!(!lib.contains("Telemetry {"));
        assert!(!lib.contains("telemetry_cmd"));
        assert!(!lib.contains("maybe_ping"));
        assert!(!repo::read("rtk/src/hooks/init/mod.rs").contains("prompt_telemetry_consent"));
    }
}

/// A new host directory or top-level file (an upstream addition) fails here.
mod tree_allowlist_test {
    use super::*;

    #[test]
    fn test_top_level_entries() {
        assert_eq!(entries("rtk"), TOP_LEVEL);
    }

    #[test]
    fn test_hooks_entries() {
        assert_eq!(entries("rtk/hooks"), HOOKS_ENTRIES);
    }
}

mod removed_paths_test {
    use super::*;

    #[test]
    fn test_none_of_the_removed_paths_exist() {
        // `os.path.lexists`: a broken symlink counts as present.
        let present: Vec<String> = removed_paths()
            .into_iter()
            .filter(|p| std::fs::symlink_metadata(repo::path(&rtk(p))).is_ok())
            .collect();
        assert!(present.is_empty(), "{present:?}");
    }
}

mod claude_only_cli_test {
    use super::*;

    fn lib() -> String {
        repo::read("rtk/src/lib.rs")
    }

    #[test]
    fn test_hook_subcommands_are_claude_and_check() {
        let lib = lib();
        let m = Regex::new(r"(?ms)^(?:pub )?enum HookCommands \{\n(.*?)^\}")
            .unwrap()
            .captures(&lib)
            .expect("enum HookCommands not found in rtk/src/lib.rs");
        let variants: BTreeSet<String> = Regex::new(r"(?m)^    ([A-Z]\w*)\b")
            .unwrap()
            .captures_iter(&m[1])
            .map(|c| c[1].to_string())
            .collect();
        let want: BTreeSet<String> = ["Claude", "Check"].map(String::from).into();
        assert_eq!(variants, want);
    }

    #[test]
    fn test_no_agent_target() {
        let lib = lib();
        assert!(!matches(r"\benum AgentTarget\b", &lib));
        assert!(!lib.contains("AgentTarget"));
    }

    #[test]
    fn test_fork_tests_are_the_eight_kept() {
        assert_eq!(glob_names("rtk/tests", "fork_", ".rs"), FORK_TESTS);
    }
}

mod trim_scripts_test {
    use super::*;

    const IMAGE_FROM_RUST_DOCKERFILE: &str =
        r#"awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile""#;

    #[test]
    fn test_trim_entry_is_executable_strict_and_runs_the_python_trim() {
        let text = repo::read(TRIM_SH);
        assert!(is_executable(TRIM_SH));
        assert!(text.starts_with("#!"));
        assert!(matches(r"(?m)^set -eu$", &text));
        // omitted: the script names scripts/rtk_trim.py and that file exists (pins the Python
        // trim) -> CO2 replacement: cutover_ci.rs asserts rtk-trim.sh runs the trim through
        // scripts/dev.sh and scripts/rtk_trim.py is gone
    }

    #[test]
    fn test_trim_prunes_the_lockfile_offline_in_the_pinned_image() {
        let text = repo::read(TRIM_SH);
        assert!(text.contains(IMAGE_FROM_RUST_DOCKERFILE));
        assert!(text.contains("cargo metadata"));
        assert!(text.contains("--offline"));
        assert!(text.contains("--format-version 1"));
        assert!(text.contains("anthology-cargo:/usr/local/cargo/registry"));
        assert!(text.contains("--lock-guard"));
        assert!(!text.contains("generate-lockfile"));
        assert!(!matches(r"rust:[0-9.]+-slim", &text));
    }

    #[test]
    fn test_sync_runs_the_trim_unconditionally() {
        let text = repo::read(SYNC);
        assert!(text.contains("if bash scripts/rtk-trim.sh; then"));
        assert!(!text.contains("trim skipped"));
        assert!(!text.contains("if [ -f scripts/rtk-trim.sh ]"));
    }

    #[test]
    fn test_sync_strips_the_conflict_comments_from_the_merge_message() {
        assert!(repo::read(SYNC).contains("git commit -q --no-edit --cleanup=strip"));
    }

    #[test]
    fn test_network_test_runs_the_binary_with_the_network_off() {
        let text = repo::read(NET_TEST);
        assert!(is_executable(NET_TEST));
        assert!(matches(r"(?m)^set -eu$", &text));
        assert!(text.contains("--network none"));
        assert!(text.contains("cargo build --release --locked"));
        assert!(text.contains(IMAGE_FROM_RUST_DOCKERFILE));
        assert!(!matches(r"rust:[0-9.]+-slim", &text));
    }

    #[test]
    fn test_ci_runs_the_network_test_after_the_rtk_tests() {
        let ci = yaml::load_file(".github/workflows/ci.yml");
        let runs: Vec<&str> = yaml::items(&ci["jobs"]["rtk-test"]["steps"])
            .iter()
            .map(|s| s["run"].as_str().unwrap_or(""))
            .collect();
        let position = |want: &str| {
            runs.iter()
                .position(|r| *r == want)
                .unwrap_or_else(|| panic!("no rtk-test step runs {want}: {runs:?}"))
        };
        assert!(position("scripts/rtk-test.sh") < position("scripts/rtk-net-test.sh"));
    }
}
