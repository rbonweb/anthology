//! ATL is a fork of rtk by RTK AI Labs (Apache-2.0). What the license asks of a modified copy,
//! and how the fork names itself: the LICENSE text kept, a NOTICE that credits upstream and
//! ships in every release archive, a README that says whose fork this is, a manifest that
//! points at this repository, and a DISCLAIMER that no longer describes the trimmed parts.

use regex::Regex;
use repo_tests::repo;
use repo_tests::yaml;

fn matches(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(text)
}

/// The license text is upstream's, word for word: only its copyright line names RTK AI Labs.
#[test]
fn the_license_keeps_the_upstream_copyright() {
    let text = repo::read("atl/LICENSE");
    assert!(text.contains("Apache License"));
    assert!(text.contains("Version 2.0, January 2004"));
    assert!(text.contains("Copyright 2026 RTK AI Labs"));
}

/// Apache-2.0 section 4(d): a modified version carries a NOTICE that credits where it came
/// from. Upstream ships none, so the fork's is the whole file.
#[test]
fn the_notice_credits_the_upstream_project() {
    let text = repo::read("atl/NOTICE");
    for line in [
        "ATL (Anthology Token Layer)",
        "This product is a modified version of rtk (https://github.com/rtk-ai/rtk),",
        "Copyright 2026 RTK AI Labs, licensed under the Apache License, Version 2.0.",
        "It was renamed and trimmed by Anthology.",
    ] {
        assert!(text.contains(line), "{line}");
    }
}

/// Each release archive holds the binary, LICENSE and NOTICE, named `atl-<triple>.tar.gz`.
#[test]
fn each_release_archive_holds_the_binary_the_license_and_the_notice() {
    let wf = yaml::load_file(".github/workflows/atl-release.yml");
    let package = yaml::items(&wf["jobs"]["build"]["steps"])
        .iter()
        .find(|s| s["name"].as_str() == Some("Package"))
        .expect("the Package step");
    let script = package["run"].as_str().unwrap_or("");
    assert!(
        script.contains(r#"cp "target/$TARGET/release/atl" LICENSE NOTICE "$stage/""#),
        "{script}"
    );
    assert!(
        script.contains(r#"tar -czf "dist/atl-$TARGET.tar.gz" -C "$stage" atl LICENSE NOTICE"#),
        "{script}"
    );
}

#[test]
fn the_readme_says_whose_fork_this_is() {
    let text = repo::read("atl/README.md");
    assert!(text.starts_with("# ATL (Anthology Token Layer)"));
    assert!(text.contains("a fork of rtk by RTK AI Labs"));
    assert!(text.contains("https://github.com/rtk-ai/rtk"));
    assert!(text.contains("## Fork and credit"));
}

#[test]
fn the_disclaimer_no_longer_names_the_trimmed_license() {
    let text = repo::read("atl/DISCLAIMER.md");
    let section = text
        .split("## Precompiled Binaries")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("a Precompiled Binaries section");
    assert!(section.contains("Apache License 2.0"));
    assert!(section.contains("NOTICE"));
    assert!(!section.contains("LICENSE-ENGINE"), "{section}");
    assert!(!section.contains("links the"), "{section}");
    assert!(!text.contains("engine/LICENSE"));
    assert!(!text.contains("proxy/LICENSE"));
}

#[test]
fn the_manifests_name_atl_and_point_at_this_repository() {
    let text = repo::read("atl/Cargo.toml");
    assert!(matches(r#"(?m)^name = "atl"$"#, &text));
    assert!(matches(
        r#"(?m)^version = "1\.2\.1-anthology\.[0-9]+"$"#,
        &text
    ));
    assert!(matches(r#"(?m)^authors = \["RTK AI Labs"\]$"#, &text));
    assert!(matches(
        r#"(?m)^description = ".*a fork of rtk by RTK AI Labs"#,
        &text
    ));
    for key in ["homepage", "repository"] {
        assert!(
            text.contains(&format!("{key} = \"https://github.com/rbonweb/anthology\"")),
            "{key}"
        );
    }
    assert!(matches(r#"(?m)^atl-text = \{ path = "text" \}$"#, &text));
    let text_crate = repo::read("atl/text/Cargo.toml");
    assert!(matches(r#"(?m)^name = "atl-text"$"#, &text_crate));
    assert!(matches(r#"(?m)^authors = \["RTK AI Labs"\]$"#, &text_crate));
}

/// The project folder of an install is called after ATL, and its old name is gone from the tree.
#[test]
fn the_crate_folder_is_atl_and_the_old_folder_is_gone() {
    assert!(repo::path("atl/Cargo.toml").is_file());
    assert!(repo::path("atl/.atl/filters.toml").is_file());
    assert!(!repo::path("rtk").exists()); // legacy
}
