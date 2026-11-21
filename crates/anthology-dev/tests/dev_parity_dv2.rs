//! Cases whose Python answers were captured by running scripts/release_edit.py and
//! scripts/check_version_bump.py in the pinned Python 3.12 image. Not ledger targets.

use std::fs;

use anthology_dev::release_edit::{
    Failure, ReleaseError, changelog_move, check_dependents, plugin_version, set_plugin_version,
    set_source_plugin_version,
};
use anthology_dev::semver::{self, RangeError};
use anthology_dev::version_bump::verdict;
use anthology_pycompat::json::{self, Value};

#[test]
fn dependents_are_sorted_by_path_components() {
    let tmp = tempfile::tempdir().unwrap();
    for n in ["a", "a-b", "a.b", ".hid", "B", "\u{e9}"] {
        let d = tmp.path().join("plugins").join(n).join(".claude-plugin");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("plugin.json"),
            r#"{"dependencies": [{"name": "lib", "version": "~0.1"}]}"#,
        )
        .unwrap();
    }
    let got: Vec<String> = check_dependents(tmp.path(), "lib", "0.2.0")
        .unwrap()
        .iter()
        .map(|p| p.split(' ').next().unwrap().to_string())
        .collect();
    let want: Vec<String> = [".hid", "B", "a", "a-b", "a.b", "\u{e9}"]
        .iter()
        .map(|n| format!("plugins/{n}/.claude-plugin/plugin.json"))
        .collect();
    assert_eq!(got, want);
}

fn shown(r: Result<String, Failure>) -> String {
    match r {
        Err(Failure::Uncaught(u)) => format!("{}: {}", u.ty, u.msg),
        Err(Failure::Release(e)) => format!("ReleaseError: {e}"),
        Err(Failure::Os(m)) => format!("OSError: {m}"),
        Ok(text) => text,
    }
}

#[test]
fn set_source_version_follows_the_python_on_duplicate_keys() {
    let dup = |tail: &str| {
        format!(
            "{{\"sources\": [{{\"target\": \"t\", \"plugin_version\": \"1\"}}], \"sources\": {tail}}}"
        )
    };
    assert_eq!(
        shown(set_source_plugin_version(&dup("\"x\""), "t", "2")),
        "AttributeError: 'str' object has no attribute 'get'"
    );
    let one = |tail: &str| format!("{{\"sources\": [{{\"target\": \"t\"}}], \"sources\": {tail}}}");
    assert_eq!(
        shown(set_source_plugin_version(&one("5"), "t", "2")),
        "TypeError: object of type 'int' has no len()"
    );
    assert_eq!(
        shown(set_source_plugin_version(&one("null"), "t", "2")),
        "TypeError: object of type 'NoneType' has no len()"
    );
    assert_eq!(
        shown(set_source_plugin_version(&one("{\"k\": 1}"), "t", "2")),
        "AttributeError: 'str' object has no attribute 'get'"
    );
    for tail in ["{}", "\"\""] {
        let text = format!("{{\"sources\": [], \"sources\": {tail}}}");
        assert_eq!(
            shown(set_source_plugin_version(&text, "t", "2")),
            "ReleaseError: sources.json has no entry with target 't'"
        );
    }
    assert_eq!(
        shown(set_source_plugin_version(
            "{\"sources\": [{\"target\": \"t\", \"plugin_version\": 3}]}",
            "t",
            "2"
        )),
        "ReleaseError: plugin_version is not a string"
    );
    // The first duplicate key is the one edited.
    assert_eq!(
        shown(set_source_plugin_version(
            "{\"sources\": [{\"target\": \"t\", \"plugin_version\": \"1\", \"plugin_version\": 3}]}",
            "t",
            "2"
        )),
        "{\"sources\": [{\"target\": \"t\", \"plugin_version\": \"2\", \"plugin_version\": 3}]}"
    );
    for text in ["[1]", "{\"sources\": {\"a\": 1}}"] {
        assert_eq!(
            shown(set_source_plugin_version(text, "t", "2")),
            "ReleaseError: sources.json has no \"sources\" array"
        );
    }
}

#[test]
fn version_edits_follow_the_python() {
    let e = |r: Result<String, ReleaseError>| match r {
        Ok(s) => s,
        Err(e) => format!("ReleaseError: {e}"),
    };
    assert_eq!(
        e(set_plugin_version(
            "{\"version\": 3, \"version\": \"1.0\"}",
            "2"
        )),
        "ReleaseError: plugin.json \"version\" is not a string"
    );
    assert_eq!(
        e(set_plugin_version(
            "{\"a\": {\"version\": \"1\"}, \"version\": \"1\"}",
            "caf\u{e9}\u{1f600}"
        )),
        "{\"a\": {\"version\": \"1\"}, \"version\": \"caf\\u00e9\\ud83d\\ude00\"}"
    );
    for text in ["[]", "{\"version\": \"1\"} x"] {
        assert_eq!(
            e(set_plugin_version(text, "2")),
            "ReleaseError: plugin.json is not a valid JSON object"
        );
    }
    assert_eq!(
        e(plugin_version("\u{feff}{\"version\": \"1\"}")),
        "ReleaseError: plugin.json is not a valid JSON object"
    );
}

#[test]
fn changelog_move_follows_the_python_on_line_endings_and_layout() {
    let m =
        |text: &str, version: &str, date: &str| changelog_move(text, "p", version, date).unwrap();
    assert_eq!(
        m(
            "## p\r\n\r\n### [Unreleased]\r\n\r\n- x\r\n",
            "1.0.0",
            "2026-01-01"
        ),
        "## p\r\n\r\n### [Unreleased]\r\n\n### [1.0.0] - 2026-01-01\n\n- x\r\n"
    );
    assert_eq!(
        m("## p\n\n### [Unreleased]\n\n- x", "1.0.0", "2026-01-01"),
        "## p\n\n### [Unreleased]\n\n### [1.0.0] - 2026-01-01\n\n- x\n"
    );
    assert_eq!(
        m(
            "## p\n\n### [Unreleased]\n\n- x\n\n### [0.9.0]\n- y\n\n\n",
            "1.0.0",
            "2026-01-01"
        ),
        "## p\n\n### [Unreleased]\n\n### [1.0.0] - 2026-01-01\n\n- x\n\n### [0.9.0]\n- y\n\n\n"
    );
    assert_eq!(
        m(
            "## p\n### [Unreleased]\n- x\n#### sub\n- z\n### [0.9.0]\n",
            "1",
            "d"
        ),
        "## p\n### [Unreleased]\n\n### [1] - d\n\n- x\n#### sub\n- z\n\n### [0.9.0]\n"
    );
    // A line separator other than \n ends a line too (str.splitlines).
    assert_eq!(
        m(
            "## p\n\u{b}\n### [Unreleased]\u{c}- x\u{85}",
            "1.0.0",
            "2026-01-01"
        ),
        "## p\n\u{b}\n### [Unreleased]\u{c}\n### [1.0.0] - 2026-01-01\n\n- x\u{85}\n"
    );
}

#[test]
fn ranges_follow_the_python_on_big_numbers_and_builds() {
    let big = "99999999999999999999";
    let rng = |r: &str, v: &str| semver::range_accepts(&Value::Str(r.into()), v);
    assert_eq!(rng(&format!("^{big}.1"), &format!("{big}.9.9")), Ok(true));
    assert_eq!(
        rng(&format!("~{big}.1.0"), &format!("{big}.2.0")),
        Ok(false)
    );
    assert_eq!(
        rng(&format!("^0.0.{big}"), "0.0.100000000000000000000"),
        Ok(false)
    );
    assert_eq!(rng(&format!("^0.0.{big}"), &format!("0.0.{big}")), Ok(true));
    assert_eq!(rng(">=1.2.3-rc.1", "1.2.3-rc.2"), Ok(true));
    assert_eq!(rng(">=1.2.3-rc.1", "1.2.4"), Ok(true));
    assert_eq!(rng("1.2.3", "1.2.3+x"), Ok(true));
    assert_eq!(
        rng("1.2.3+x", "1.2.3"),
        Err(RangeError::Unsupported(
            "can't check range '1.2.3+x'".into()
        ))
    );
    assert_eq!(rng("~1.2.3", "1.2.3-rc.1"), Ok(false));
    let bad = || RangeError::BadVersion("'x' is not a valid semver version".into());
    assert_eq!(rng("~1.2", "x"), Err(bad()));
    assert_eq!(
        rng("~", "1.2.3"),
        Err(RangeError::Unsupported("can't check range '~'".into()))
    );
    // The version is parsed before the range form is judged.
    assert_eq!(rng("*", "x"), Err(bad()));
    let v = |text: &str| json::loads(text).unwrap();
    assert_eq!(
        semver::range_accepts(&v("5"), "1.2.3"),
        Err(RangeError::Unsupported("can't check range 5".into()))
    );
    assert_eq!(
        semver::range_accepts(&v("[\"a\"]"), "1.2.3"),
        Err(RangeError::Unsupported("can't check range ['a']".into()))
    );
}

#[test]
fn verdict_messages_embed_the_python_repr() {
    let v = |text: &str| json::loads(text).unwrap();
    assert_eq!(
        verdict(
            "p",
            &v("{\"a\": [1, 2.5, null, true]}"),
            &v("\"1.0.0\""),
            true,
            true
        ),
        (
            true,
            "plugins/p 1.0.0 (base version {'a': [1, 2.5, None, True]} was invalid)".into()
        )
    );
    assert_eq!(
        verdict(
            "p",
            &v("\"1.0.0\""),
            &v("{\"a\": [1, 2.5, null, true, \"it's\"]}"),
            true,
            true
        ),
        (
            false,
            "plugins/p version {'a': [1, 2.5, None, True, \"it's\"]} is not valid semver".into()
        )
    );
    assert_eq!(
        verdict("p", &v("\"bad\\n\""), &v("\"1.0.0\""), true, true),
        (
            true,
            "plugins/p 1.0.0 (base version 'bad\\n' was invalid)".into()
        )
    );
    assert_eq!(
        semver::compare("1.0", "x\n").unwrap_err().to_string(),
        "not valid semver: '1.0' / 'x\\n'"
    );
}
