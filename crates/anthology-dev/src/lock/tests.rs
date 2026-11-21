//! Unit tests of the pure parts of `lock` and `lock::fresh`.

use std::collections::BTreeSet;

use super::fresh::*;
use super::*;

fn hex(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn checksums() -> String {
    format!(
        "{}  anthology-aarch64-apple-darwin.tar.gz\n{}  anthology-aarch64-unknown-linux-musl.tar.gz\n\
         {}  anthology-x86_64-apple-darwin.tar.gz\n{}  anthology-x86_64-unknown-linux-musl.tar.gz\n",
        hex('a'),
        hex('d'),
        hex('b'),
        hex('c')
    )
}

#[test]
fn checksums_parse_the_sorted_sha256sum_output() {
    let d = parse_checksums(&checksums()).unwrap();
    assert_eq!(d[&Target::AppleArm64], hex('a'));
    assert_eq!(d[&Target::AppleX86_64], hex('b'));
    assert_eq!(d[&Target::LinuxX86_64Musl], hex('c'));
    assert_eq!(d[&Target::LinuxArm64Musl], hex('d'));
    // The final newline is optional.
    assert!(parse_checksums(checksums().trim_end()).is_ok());
}

#[test]
fn checksums_refuse_anything_else() {
    let good = checksums();
    let cases = [
        good.replacen("  ", " ", 1),                  // one space
        good.replacen("  ", " *", 1),                 // binary-mode marker
        good.replacen(&hex('a'), &hex('A'), 1),       // upper case
        good.replacen(&hex('a'), &hex('a')[..63], 1), // 63 digits
        good.replacen("aarch64-apple-darwin", "x86_64-pc-windows", 1), // unknown target
        good.replacen("anthology-aarch64", "rtk-aarch64", 1), // other asset
        good.replacen(".tar.gz\n", ".tgz\n", 1),
        format!("{good}\n"), // blank line
        format!("{good}{}  anthology-x86_64-apple-darwin.tar.gz\n", hex('e')), // duplicate
        good.lines().take(3).map(|l| format!("{l}\n")).collect(), // missing target
        String::new(),
    ];
    for case in cases {
        assert!(parse_checksums(&case).is_err(), "{case:?}");
    }
}

#[test]
fn render_is_the_design_layout_and_refuses_bad_fields() {
    let digests = parse_checksums(&checksums()).unwrap();
    let text = render("anthology-bin--v0.1.0", DEFAULT_REPO, digests.clone()).unwrap();
    let want = format!(
        "{{\n  \"version\": \"0.1.0\",\n  \"tag\": \"anthology-bin--v0.1.0\",\n  \"repo\": \"rbonweb/anthology\",\n  \"sha256\": {{\n    \"aarch64-apple-darwin\": \"{}\",\n    \"x86_64-apple-darwin\": \"{}\",\n    \"x86_64-unknown-linux-musl\": \"{}\",\n    \"aarch64-unknown-linux-musl\": \"{}\"\n  }}\n}}\n",
        hex('a'),
        hex('b'),
        hex('c'),
        hex('d')
    );
    assert_eq!(text, want);
    for (tag, repo) in [
        ("anthology--v0.1.0", DEFAULT_REPO),
        ("rtk--v0.1.0", DEFAULT_REPO),
        ("anthology-bin--v0.1", DEFAULT_REPO),
        ("anthology-bin--v0.1.0-", DEFAULT_REPO),
        ("anthology-bin--v0.1.0-rc_1", DEFAULT_REPO),
        ("anthology-bin--v0.1.0", "rbonweb"),
        ("anthology-bin--v0.1.0", "rbonweb/.."),
        ("anthology-bin--v0.1.0", "a/b/c"),
    ] {
        assert!(render(tag, repo, digests.clone()).is_err(), "{tag} {repo}");
    }
    assert!(render("anthology-bin--v1.2.3-rc.1", "o/r", digests).is_ok());
}

#[test]
fn tree_lines_parse() {
    let p = parse_tree_line("anthology-core v0.1.0 (/w/crates/anthology-core)").unwrap();
    assert_eq!(
        (p.name.as_str(), p.version.as_str(), p.source.as_deref()),
        ("anthology-core", "0.1.0", Some("/w/crates/anthology-core"))
    );
    let p = parse_tree_line("anthology-core v0.1.0 (/w/crates/anthology-core) (*)").unwrap();
    assert_eq!(p.source.as_deref(), Some("/w/crates/anthology-core"));
    let p = parse_tree_line("regex v1.13.2").unwrap();
    assert_eq!((p.name.as_str(), p.source), ("regex", None));
    let p = parse_tree_line("regex v1.13.2 (*)").unwrap();
    assert_eq!(p.source, None);
    let p = parse_tree_line("serde_derive v1.0.0 (proc-macro)").unwrap();
    assert_eq!(p.source, None);
    let p = parse_tree_line("x v1.0.0 (/a b/x) (proc-macro) (*)").unwrap();
    assert_eq!(p.source.as_deref(), Some("/a b/x"));
    assert!(parse_tree_line("[build-dependencies]").is_none());
    assert!(parse_tree_line("nothing").is_none());
}

#[test]
fn normalize_resolves_dots() {
    assert_eq!(normalize("a/b/../c/./d").as_deref(), Some("a/c/d"));
    assert_eq!(normalize("a/../..").as_deref(), None);
    assert_eq!(
        normalize("crates/x/src/../../../data/f").as_deref(),
        Some("data/f")
    );
}

#[test]
fn includes_are_found_and_unfollowable_ones_refused() {
    let src = r#"
const A: &str = include_str!("../data/a.txt");
const B: &[u8] = include_bytes!( "b.bin" );
// include_str!(not_a_literal) in a comment
/// include!(also a comment)
const C: &str = include_str!(concat!(env!("OUT_DIR"), "/gen.rs"));
include!(r"raw.rs");
fn my_include_str() {}
"#;
    assert_eq!(includes(src).unwrap(), ["../data/a.txt", "b.bin", "raw.rs"]);
    assert!(includes("const X: &str = include_str!(PATH);").is_err());
    assert!(includes("const X: &str = include_str!(concat!(\"a\", \"b\"));").is_err());
    assert!(includes("const X: &str = include_str!(\"a\\\\b\");").is_err());
}

#[test]
fn cargo_lock_blocks_by_name() {
    let lock = "# This file is automatically @generated by Cargo.\nversion = 4\n\n\
[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n\
[[package]]\nname = \"b\"\nversion = \"2.0.0\"\nsource = \"registry+x\"\nchecksum = \"00\"\n\n\
[[package]]\nname = \"b\"\nversion = \"1.0.0\"\n\n[metadata]\nx = 1\n";
    let blocks = lock_blocks(lock);
    assert_eq!(blocks["a"], ["name = \"a\"\nversion = \"1.0.0\""]);
    assert_eq!(blocks["b"].len(), 2);
    assert!(blocks["b"][0].contains("1.0.0"));
    assert!(!blocks.contains_key("x"));
}

#[test]
fn toml_tables_and_entries() {
    let text = r#"
[workspace]
members = [
  "crates/*", # all
]

[workspace.dependencies]
# a comment
regex = "=1.13.2"  # pinned
ren = { package = "real-name", version = "1" }
multi = { version = "1",
  features = ["x"] }
s = "a # not a comment"

[workspace.dependencies.sub]
version = "2"

[profile.release]
lto = "thin"

[[bin]]
path = "tools/main.rs"
"#;
    let tables = parse_tables(text);
    let deps = tables
        .iter()
        .find(|t| t.header == "workspace.dependencies")
        .unwrap();
    let keys: Vec<&str> = deps.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["regex", "ren", "multi", "s"]);
    assert_eq!(deps.entries[0].text, "regex = \"=1.13.2\"");
    assert_eq!(
        deps.entries[2].text,
        "multi = { version = \"1\",\nfeatures = [\"x\"] }"
    );
    assert_eq!(deps.entries[3].text, "s = \"a # not a comment\"");
    let ws = tables.iter().find(|t| t.header == "workspace").unwrap();
    assert_eq!(ws.entries.len(), 1);
    assert!(tables.iter().any(|t| t.header == "bin" && t.array));
    assert_eq!(
        manifest_targets(text),
        [(TargetKind::Crate, "tools/main.rs".to_string())]
    );
    assert_eq!(
        manifest_targets("[package]\nname = \"x\"\nbuild = \"gen/b.rs\"\n"),
        [(TargetKind::Build, "gen/b.rs".to_string())]
    );
}

#[test]
fn root_manifest_items_select_the_shipped_parts() {
    let text = r#"
[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
regex = "=1.13.2"
proptest = "=1.12.0"
ren = { package = "real-name", version = "1" }

[workspace.dependencies.sub]
version = "2"

[profile.release]
lto = "thin"

[profile.release.package.foo]
opt-level = 3

[profile.dev]
opt-level = 0

[patch.crates-io]
regex = { path = "x" }
"#;
    let names: BTreeSet<String> = ["regex", "real-name", "sub"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let items = root_manifest_items(text, &names);
    let labels: Vec<&str> = items.keys().map(String::as_str).collect();
    assert_eq!(
        labels,
        [
            "Cargo.toml [patch.crates-io]",
            "Cargo.toml [profile.release.package.foo]",
            "Cargo.toml [profile.release]",
            "Cargo.toml [workspace.dependencies] regex",
            "Cargo.toml [workspace.dependencies] ren",
            "Cargo.toml [workspace.dependencies] sub",
            "Cargo.toml [workspace.package] edition",
            "Cargo.toml [workspace.package] version",
        ]
    );
    // Comments and blank lines do not count.
    let commented = text.replace("lto = \"thin\"", "# speed\nlto = \"thin\" # thin\n");
    assert_eq!(root_manifest_items(&commented, &names), items);
}

#[test]
fn the_probe_option_is_a_usage_error() {
    for sub in ["write", "check", "check-fresh"] {
        let out = run(
            &[sub.into(), "--f1-registration-probe".into()],
            &mut std::io::empty(),
        );
        assert_eq!(out.code, 2, "{sub}");
    }
}

#[test]
fn path_modules_and_rust_includes_are_references() {
    let src = r#"
#[path = "../../shared/x.rs"]
mod x;
#[ path = "y.rs" ] mod y;
include!("gen.rs");
const T: &str = include_str!("t.txt");
// #[path = "commented.rs"]
"#;
    assert_eq!(
        references(src).unwrap(),
        [
            Ref {
                path: "../../shared/x.rs".into(),
                rust: true
            },
            Ref {
                path: "y.rs".into(),
                rust: true
            },
            Ref {
                path: "gen.rs".into(),
                rust: true
            },
            Ref {
                path: "t.txt".into(),
                rust: false
            },
        ]
    );
    assert!(references("#[cfg_attr(unix, path = \"u.rs\")]\nmod m;").is_err());
    assert!(references("#[path = concat!(\"a\", \"b\")]\nmod m;").is_err());
}

#[test]
fn build_inputs_are_declared_or_refused() {
    let ok = r#"fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../../data/gen.txt");
    let _ = std::fs::read_to_string("../../data/gen.txt");
}"#;
    assert_eq!(build_inputs(ok).unwrap(), ["../../data/gen.txt"]);
    assert_eq!(
        build_inputs("fn main() { println!(\"cargo:rerun-if-changed=build.rs\"); }").unwrap(),
        Vec::<String>::new()
    );
    assert!(build_inputs("fn main() { let _ = std::fs::read(\"x\"); }").is_err());
    assert!(build_inputs("fn main() { println!(\"cargo:rerun-if-changed={p}\"); }").is_err());
}

#[test]
fn dotted_keys_and_headers() {
    assert_eq!(dotted("a.b"), ["a", "b"]);
    assert_eq!(
        dotted(" profile . \"release\" .'x.y'"),
        ["profile", "release", "x.y"]
    );
    let t = parse_tables("[profile.\"release\".package.foo]\nopt-level = 0\n");
    assert_eq!(t[1].header, "profile.release.package.foo");
}

#[test]
fn root_manifest_items_compare_everything_but_the_denylist() {
    let names: BTreeSet<String> = ["anthology-core"].iter().map(|s| s.to_string()).collect();
    let base = "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n\n[workspace.lints.rust]\nx = \"warn\"\n\n[profile.dev]\nopt-level = 0\n";
    let items = root_manifest_items(base, &names);
    assert_eq!(
        items.keys().collect::<Vec<_>>(),
        ["Cargo.toml [workspace]"],
        "{items:?}"
    );
    assert_eq!(
        items["Cargo.toml [workspace]"],
        "workspace.resolver = \"3\""
    );
    // The denylisted parts do not count; anything else does.
    let same = base
        .replace("crates/*", "crates/*\", \"x")
        .replace("x = \"warn\"", "x = \"deny\"")
        .replace("opt-level = 0", "opt-level = 1");
    assert_eq!(root_manifest_items(&same, &names), items);
    assert_ne!(
        root_manifest_items(&base.replace("\"3\"", "\"2\""), &names),
        items
    );
    let top = format!("cargo-features = [\"x\"]\n{base}");
    assert!(
        root_manifest_items(&top, &names).contains_key("Cargo.toml (top level) cargo-features")
    );
    // Quoted headers and dotted keys are the same items as plain tables.
    let plain = root_manifest_items("[profile.release]\nlto = \"thin\"\n", &names);
    let quoted = root_manifest_items("[profile.\"release\"]\nlto = \"thin\"\n", &names);
    let dotted_form = root_manifest_items("[profile]\nrelease.lto = \"thin\"\n", &names);
    assert_eq!(plain, quoted);
    assert_eq!(plain, dotted_form);
    assert!(plain.contains_key("Cargo.toml [profile.release]"));
    assert!(root_manifest_items("[profile]\ndev.lto = \"thin\"\n", &names).is_empty());
}

#[test]
fn renamed_workspace_dependencies_are_found_by_their_package_key() {
    let names: BTreeSet<String> = ["anthology-core"].iter().map(|s| s.to_string()).collect();
    let text = "[workspace.dependencies]\nx = { path = \"crates/package-x\", package = \"anthology-core\" }\nunused = { path = \"crates/package-y\" }\n\n[workspace.dependencies.z]\npath = \"crates/z\"\npackage = \"anthology-core\"\n";
    let items = root_manifest_items(text, &names);
    assert_eq!(
        items.keys().collect::<Vec<_>>(),
        [
            "Cargo.toml [workspace.dependencies] x",
            "Cargo.toml [workspace.dependencies] z"
        ]
    );
}

#[test]
fn module_items_are_told_apart() {
    assert_eq!(outline_mod("pub mod y;\n"), Some(1));
    assert_eq!(
        outline_mod("// mod y;\nfn f() {}\n#[cfg(test)] pub(crate) mod t ;\n"),
        Some(3)
    );
    assert_eq!(
        outline_mod("mod inline {\n}\nfn module() {}\nlet modx = 1;\n"),
        None
    );
    assert_eq!(inline_mod("mod inline {\n}\n"), Some(1));
    assert_eq!(inline_mod("pub mod y;\n"), None);
    assert_eq!(
        outline_mod("mod split\n{\n}\n"),
        Some(1),
        "unclear counts as both"
    );
    assert_eq!(inline_mod("mod split\n{\n}\n"), Some(1));
}

#[test]
fn absolute_build_inputs_and_inline_workspace_dependencies() {
    assert!(build_inputs("fn main() { println!(\"cargo:rerun-if-changed=/etc/x\"); }").is_err());
    let names: BTreeSet<String> = BTreeSet::new();
    let items = root_manifest_items("[workspace]\ndependencies = { a = \"1\" }\n", &names);
    assert!(
        items.contains_key("Cargo.toml [workspace.dependencies]"),
        "{items:?}"
    );
}
