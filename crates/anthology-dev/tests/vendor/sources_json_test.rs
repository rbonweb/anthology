//! test_vendor.SourcesJsonTest.

use std::collections::BTreeSet;

use anthology_dev::vendor;
use anthology_pycompat::json::Value;

use crate::fixture::{SHA_A, del, entry, jv, repo_root, s, stripped_knowledge, validate, with};

fn pins() -> Vec<Value> {
    vendor::load_sources(&repo_root()).unwrap()
}

fn get<'a>(pins: &'a [Value], name: &str) -> &'a anthology_pycompat::json::Map {
    pins.iter()
        .filter_map(Value::as_object)
        .find(|e| e.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("no pin {name}"))
}

fn field<'a>(e: &'a anthology_pycompat::json::Map, k: &str) -> Option<&'a str> {
    e.get(k).and_then(Value::as_str)
}

#[test]
fn test_real_sources_validate() {
    let entries = pins();
    assert_eq!(vendor::validate_sources(&entries), Vec::<String>::new());
    let names: BTreeSet<String> = entries
        .iter()
        .filter_map(|e| e.as_object()?.get("name")?.as_str().map(str::to_string))
        .collect();
    let mut want: BTreeSet<String> = [
        "superpowers",
        "design-skills",
        "frontend-design",
        "web-design-guidelines",
        "ui-taste",
        "caveman",
        "design-taste-frontend",
        "karpathy-guidelines",
        "website-clone",
        "humanizer",
        "avoid-ai-design",
        "atl",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    want.extend(stripped_knowledge().into_keys());
    assert_eq!(names, want);
}

#[test]
fn test_real_pins_match_the_plan() {
    let p = pins();
    let sp = get(&p, "superpowers");
    assert_eq!(
        field(sp, "commit"),
        Some("22f0b463e7a5e579904dd6211a520321035526f7")
    );
    assert_eq!(field(sp, "ref"), Some("v7.0.2"));
    // The pin follows the shipped plugin's own version (release.sh bumps both together).
    let manifest = std::fs::read_to_string(
        repo_root().join("plugins/anthology-superpowers/.claude-plugin/plugin.json"),
    )
    .unwrap();
    let manifest = anthology_pycompat::json::loads(&manifest).unwrap();
    let shipped = manifest
        .as_object()
        .and_then(|m| m.get("version"))
        .and_then(Value::as_str);
    assert!(shipped.is_some(), "plugin.json has a version");
    assert_eq!(field(sp, "plugin_version"), shipped);
    assert_eq!(field(sp, "upstream_version"), Some("7.0.2"));
    assert_eq!(
        field(get(&p, "design-skills"), "commit"),
        Some("7e25e66e87f64f9d3dea22198f9e29937e7c48f5")
    );
    let fd = get(&p, "frontend-design");
    assert_eq!(
        field(fd, "commit"),
        Some("e4bb5d960d87126ff3b3a8af1a104b7db67560ba")
    );
    assert_eq!(field(fd, "path"), Some("skills/frontend-design"));
    let wd = get(&p, "web-design-guidelines");
    assert_eq!(
        field(wd, "commit"),
        Some("1a2c79d4e6c4a2283dc2c614b6538cbe1ed5ca62")
    );
    assert_eq!(field(wd, "path"), Some("skills/web-design-guidelines"));
    assert!(wd.get("license_file").unwrap().is_null());
    assert_eq!(field(wd, "license_note"), Some("README"));
}

#[test]
fn test_the_22_knowledge_entries() {
    let p = pins();
    let known = stripped_knowledge();
    assert_eq!(known.len(), 22);
    for (name, (slug, path)) in &known {
        let e = get(&p, name);
        assert_eq!(&vendor::source_slug(e), slug, "{name}");
        assert_eq!(field(e, "path"), Some(path.as_str()), "{name}");
        assert_eq!(
            field(e, "target"),
            Some(format!("plugins/anthology/knowledge/{name}").as_str()),
            "{name}"
        );
        let license = if slug.starts_with("anthropics") {
            "Apache-2.0"
        } else {
            "MIT"
        };
        assert_eq!(field(e, "license"), Some(license), "{name}");
        assert_eq!(field(e, "license_file"), Some("LICENSE"), "{name}");
        let transform = e.get("transform").unwrap().as_array().unwrap();
        assert_eq!(
            transform[0],
            jv(r#"{"name": "strip-connectors"}"#),
            "{name}"
        );
        let mut expected = vec![jv(r#"{"name": "strip-connectors"}"#)];
        if name == "code-review-checklist" {
            expected.push(jv(
                r#"{"name": "rename-skill", "old": "code-review", "new": "code-review-checklist"}"#,
            ));
        }
        assert_eq!(transform, &expected, "{name}");
    }
    let commits = |slug: &str, key: &str| -> BTreeSet<String> {
        p.iter()
            .filter_map(Value::as_object)
            .filter(|e| vendor::source_slug(e) == slug)
            .filter_map(|e| field(e, key).map(str::to_string))
            .collect()
    };
    assert_eq!(
        commits("anthropics-knowledge-work-plugins", "commit"),
        BTreeSet::from(["0e0fcacddcc892eb92a1b7873ff9d68159c47571".to_string()])
    );
    assert_eq!(
        commits("uxkin-agent", "commit"),
        BTreeSet::from(["07e8f5f61421c7268fddc1511878fb09945da854".to_string()])
    );
    assert_eq!(
        commits("uxkin-agent", "ref"),
        BTreeSet::from(["website-design".to_string()])
    );
}

#[test]
fn test_valid_entry_has_no_errors() {
    assert_eq!(validate(&[entry("x", &[])]), Vec::<String>::new());
}

#[test]
fn test_checksum_instead_of_commit_is_accepted() {
    let mut e = entry("x", &[("checksum", s(&"c".repeat(64)))]);
    del(&mut e, "commit");
    assert_eq!(validate(&[e]), Vec::<String>::new());
}

#[test]
fn test_commit_must_be_40_hex() {
    for bad in [
        "abc123".to_string(),
        "g".repeat(40),
        "A".repeat(40),
        "a".repeat(39),
        String::new(),
    ] {
        let errors = validate(&[entry("x", &[("commit", s(&bad))])]);
        assert!(errors.iter().any(|m| m.contains("commit")), "{bad}");
    }
}

#[test]
fn test_commit_or_checksum_required() {
    let mut e = entry("x", &[]);
    del(&mut e, "commit");
    assert!(!validate(&[e]).is_empty());
}

#[test]
fn test_license_restricted_to_mit_and_apache() {
    for bad in [
        s("GPL-3.0-only"),
        s("CC-BY-4.0"),
        s(""),
        Value::Null,
        s("NOASSERTION"),
    ] {
        let errors = validate(&[entry("x", &[("license", bad.clone())])]);
        assert!(errors.iter().any(|m| m.contains("license")), "{bad:?}");
    }
    assert_eq!(
        validate(&[entry("x", &[("license", s("Apache-2.0"))])]),
        Vec::<String>::new()
    );
}

#[test]
fn test_required_fields() {
    for field in ["name", "path", "ref", "license", "target", "transform"] {
        let mut e = entry("x", &[]);
        del(&mut e, field);
        assert!(!validate(&[e]).is_empty(), "{field}");
    }
}

#[test]
fn test_duplicate_names_and_targets_rejected() {
    assert!(
        !validate(&[
            entry("x", &[]),
            entry("x", &[("target", s("plugins/anthology/knowledge/y"))])
        ])
        .is_empty()
    );
    assert!(
        !validate(&[
            entry("x", &[]),
            entry("y", &[("target", s("plugins/anthology/knowledge/x"))])
        ])
        .is_empty()
    );
}

#[test]
fn test_target_must_stay_inside_the_repo() {
    for bad in ["/etc", "../x", "plugins/../../x", ""] {
        assert!(
            !validate(&[entry("x", &[("target", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_target_cannot_name_the_root_or_a_critical_directory() {
    // build/sync rmtree the target, so these would delete the checkout, .git or vendor/
    for bad in [
        ".",
        "./",
        "plugins/.",
        "./plugins/x",
        "plugins/./x",
        "plugins//x",
        "plugins/x/",
        ".git",
        "vendor",
        "scripts",
        "tests",
        "plugins",
        "plugins/",
        ".github",
        "pluginsx/y",
        "other/plugins/x",
    ] {
        assert!(
            !validate(&[entry("x", &[("target", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_target_is_on_the_allowlist() {
    for bad in [
        "plugins/anthology",
        "plugins/anthology/",
        "plugins/anthology/knowledge",
        "plugins/anthology/knowledge/a/b",
        "plugins/anthology/skills/x",
        "plugins/anthology/.claude-plugin",
        "plugins/anthology-guardrails",
        "plugins/anthology/knowledge/.git",
        "plugins/anthology/knowledge/Upper",
        "plugins/anthology-superpowers/skills",
        "plugins/x",
        "plugins/anthology/knowledgex/a",
    ] {
        assert!(
            !validate(&[entry("x", &[("target", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
    for good in [
        "plugins/anthology-superpowers",
        "plugins/anthology/knowledge/x",
        "plugins/anthology/knowledge/code-review-checklist",
    ] {
        assert_eq!(
            validate(&[entry("x", &[("target", s(good))])]),
            Vec::<String>::new(),
            "{good}"
        );
    }
}

#[test]
fn test_subtree_target_is_still_a_clean_relative_path() {
    let e = jv(r#"{"name": "atl", "managed_by": "subtree", "license": "Apache-2.0"}"#);
    for bad in [".", "../x", "/x", "a/./b"] {
        assert!(
            !validate(&[with(&e, &[("target", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
    assert_eq!(
        validate(&[with(&e, &[("target", s("atl"))])]),
        Vec::<String>::new()
    );
}

#[test]
fn test_path_must_stay_inside_the_upstream_repo() {
    for bad in ["/etc", "../x", "a/../../x"] {
        assert!(
            !validate(&[entry("x", &[("path", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_repo_must_be_https() {
    for bad in [
        "git://github.com/a/b",
        "file:///tmp/x",
        "http://github.com/a/b",
        "-oProxy=x",
    ] {
        assert!(
            !validate(&[entry("x", &[("repo", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_unknown_transform_rejected() {
    assert!(!validate(&[entry("x", &[("transform", jv(r#"["nonsense"]"#))])]).is_empty());
    assert!(
        !validate(&[entry(
            "x",
            &[("transform", jv(r#"[{"name": "nonsense"}]"#))]
        )])
        .is_empty()
    );
}

#[test]
fn test_rename_plugin_needs_plugin_version() {
    let t = jv(r#"[{"name": "rename-plugin", "old": "a", "new": "b"}]"#);
    assert!(!validate(&[entry("x", &[("transform", t.clone())])]).is_empty());
    assert_eq!(
        validate(&[entry(
            "x",
            &[("transform", t), ("plugin_version", s("0.1.0"))]
        )]),
        Vec::<String>::new()
    );
}

#[test]
fn test_strip_connectors_and_rename_skill_are_known_transforms() {
    assert_eq!(
        validate(&[entry("x", &[("transform", jv(r#"["strip-connectors"]"#))])]),
        Vec::<String>::new()
    );
    let t = jv(r#"[{"name": "rename-skill", "old": "a", "new": "x"}]"#);
    assert_eq!(
        validate(&[entry("x", &[("transform", t)])]),
        Vec::<String>::new()
    );
    for bad in [
        r#"{"name": "rename-skill", "old": "a"}"#,
        r#"{"name": "rename-skill", "new": "b"}"#,
        r#"{"name": "rename-skill", "old": "a", "new": "../b"}"#,
        r#"{"name": "rename-skill", "old": "", "new": "b"}"#,
    ] {
        let t = jv(&format!("[{bad}]"));
        assert!(
            !validate(&[entry("x", &[("transform", t)])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_missing_license_file_requires_a_note() {
    assert!(!validate(&[entry("x", &[("license_file", Value::Null)])]).is_empty());
    assert_eq!(
        validate(&[entry(
            "x",
            &[("license_file", Value::Null), ("license_note", s("README"))]
        )]),
        Vec::<String>::new()
    );
}

#[test]
fn test_same_repo_entries_must_agree_on_the_license_file() {
    let a = entry(
        "a",
        &[
            ("repo", s("https://github.com/o/r")),
            ("license_file", s("LICENSE")),
        ],
    );
    let b = entry(
        "b",
        &[
            ("repo", s("https://github.com/o/r")),
            ("license_file", s("OTHER")),
        ],
    );
    assert!(!validate(&[a, b]).is_empty());
}

#[test]
fn test_subtree_entries_are_not_checked_for_pins() {
    let e =
        jv(r#"{"name": "atl", "managed_by": "subtree", "license": "Apache-2.0", "target": "atl"}"#);
    assert_eq!(validate(&[e]), Vec::<String>::new());
}

#[test]
fn test_slug() {
    let slug = |repo: &str| {
        let e = entry("x", &[("repo", s(repo))]);
        vendor::source_slug(e.as_object().unwrap())
    };
    assert_eq!(
        slug("https://github.com/obra/superpowers"),
        "obra-superpowers"
    );
    assert_eq!(slug("https://github.com/o/r.git/"), "o-r");
}

// ---- parity details beyond the Python tests (vendor.py semantics; no ledger rows)

#[test]
fn python_dollar_accepts_one_trailing_newline() {
    // `NAME.match("x\n")`, `SHA1.match(...)`: Python's `$` matches before a final newline.
    assert_eq!(
        validate(&[entry(
            "x\n",
            &[("target", s("plugins/anthology/knowledge/x"))]
        )]),
        Vec::<String>::new()
    );
    let e = entry("x", &[("commit", s(&format!("{SHA_A}\n")))]);
    assert_eq!(validate(&[e]), Vec::<String>::new());
    let e = entry("x", &[("commit", s(&format!("{SHA_A}\n\n")))]);
    assert_eq!(
        validate(&[e]),
        vec!["x: commit must be 40 lowercase hex characters".to_string()]
    );
}

#[test]
fn messages_and_order_match_python() {
    // python3 -c 'import vendor; print(vendor.validate_sources([...]))' on the same input.
    let entries = [
        jv(
            r#"{"license": 5, "target": "x", "transform": [1, {"patch": "/abs"}, {"name": null}, {"name": "rename-plugin", "old": ""}, {"name": "rename-skill", "old": "A", "new": "b"}], "repo": "https://h/o/r", "license_extra": ["a/LICENSE", "b/NOTICE", "c/NOTICE"], "portmap": "vendor/x.json"}"#,
        ),
        jv("5"),
        jv(
            r#"{"name": 7, "license": "MIT", "target": "plugins/anthology-superpowers", "path": "", "ref": "m", "transform": [], "url": "https://e.t/a/../", "repo": "", "commit": null, "checksum": "C"}"#,
        ),
    ];
    let got = validate(&entries);
    let want = [
        "entry 0: name must be lowercase letters, digits and dashes",
        "entry 0: license 5 is not in ['Apache-2.0', 'MIT']; the owner must decide",
        "entry 0: target must be plugins/anthology-superpowers or plugins/anthology/knowledge/<name> (build deletes and recreates it)",
        "entry 0: missing path",
        "entry 0: missing ref",
        "entry 0: needs a 40-hex commit or a checksum",
        "entry 0: missing license_file (null needs a license_note)",
        "entry 0: license_extra 'a/LICENSE' clashes with another shipped license file named LICENSE",
        "entry 0: license_extra 'c/NOTICE' clashes with another shipped license file named NOTICE",
        "entry 0: bad transform 1",
        "entry 0: bad patch path '/abs'",
        "entry 0: unknown transform None",
        "entry 0: rename-plugin needs old and new",
        "entry 0: rename-plugin needs plugin_version",
        "entry 0: rename-skill needs old and new skill names",
        "entry 0: portmap must be a .json file under vendor/patches/",
        "entry 1: not an object",
        "7: name must be lowercase letters, digits and dashes",
        "7: checksum must be 64 lowercase hex characters",
        "7: url must be an https URL ending in a slash",
        "7: a url source needs a license_file inside the snapshot",
    ];
    assert_eq!(got, want.iter().map(|s| s.to_string()).collect::<Vec<_>>());
}
