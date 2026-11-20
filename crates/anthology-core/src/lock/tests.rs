//! `lock`'s private rules; the CPython golden (`tests/golden.rs`) covers the messages.

use super::*;
use proptest::prelude::*;

const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn lock(version: &str, prefix: &str, repo: &str) -> Lock {
    Lock {
        version: version.into(),
        tag: format!("{prefix}{version}"),
        repo: repo.into(),
        sha256: Target::ALL.iter().map(|t| (*t, HEX.to_string())).collect(),
    }
}

#[test]
fn rtk_versions_follow_version_re_with_unicode_digits() {
    for ok in [
        "0.1.0",
        "10.20.30",
        "1.2.3-rc.1",
        "1.2.3--",
        "1.2.3-a-b.c-d",
        "\u{661}.\u{662}.\u{663}",
        "1.\u{ff11}.3",
    ] {
        assert!(rtk_version_ok(ok), "{ok:?}");
    }
    for bad in [
        "",
        "1",
        "1.2",
        "1.2.",
        "1.2.3.",
        "1.2.3-",
        "1.2.3-a.",
        "1.2.3-.a",
        "1.2.3-a..b",
        "v1.2.3",
        "1.2.3+b",
        "1.2.3 ",
        " 1.2.3",
        "1..2.3",
        "1.2.3-é",
        "\u{b2}.1.1",
        "1.2.3\n",
    ] {
        assert!(!rtk_version_ok(bad), "{bad:?}");
    }
}

#[test]
fn anthology_versions_follow_gc17() {
    for ok in ["0.1.0", "1.2.3-rc.1", "1.2.3-..", "1.2.3-A.b.9"] {
        assert!(anthology_version_ok(ok), "{ok:?}");
    }
    for bad in [
        "1.2.3-",
        "1.2.3-a-b",
        "\u{661}.1.1",
        "1.2",
        "1.2.3\n",
        "1.2.3_x",
    ] {
        assert!(!anthology_version_ok(bad), "{bad:?}");
    }
}

#[test]
fn repos_follow_repo_re_without_dot_parts() {
    for ok in ["a/b", "rbonweb/anthology", "A_b.c-d/x..y", "..a/b"] {
        assert!(repo_ok(ok), "{ok:?}");
    }
    for bad in [
        "", "a", "a/", "/b", "a/b/c", "./b", "a/..", "a b/c", "a/é", "a\\b/c",
    ] {
        assert!(!repo_ok(bad), "{bad:?}");
    }
}

#[test]
fn repr_matches_cpython() {
    for (s, want) in [
        ("plain", "'plain'"),
        ("a'b", "\"a'b\""),
        ("a\"b", "'a\"b'"),
        ("a'\"b", "'a\\'\"b'"),
        ("\\\t\n\r", "'\\\\\\t\\n\\r'"),
        ("\0\x1f\x7f", "'\\x00\\x1f\\x7f'"),
        ("\u{80}\u{a0}\u{ad}\u{e9}", "'\\x80\\xa0\\xad\u{e9}'"),
        ("\u{2028}\u{feff}\u{378}", "'\\u2028\\ufeff\\u0378'"),
        (
            "\u{1f600}\u{e0001}\u{10ffff}",
            "'\u{1f600}\\U000e0001\\U0010ffff'",
        ),
    ] {
        assert_eq!(py_repr_str(s), want, "{s:?}");
    }
    let v = json::loads(r#"[1, null, true, false, "x", {"a": [1.5, 1e16]}, 1e400, -1e400, 123456789012345678901234567890]"#)
        .expect("json");
    assert_eq!(
        py_repr(&v),
        "[1, None, True, False, 'x', {'a': [1.5, 1e+16]}, inf, -inf, 123456789012345678901234567890]"
    );
    assert_eq!(py_repr(&json::loads("NaN").expect("nan")), "nan");
    assert_eq!(py_repr(&json::loads("{}").expect("{}")), "{}");
    assert_eq!(py_repr(&json::loads("[]").expect("[]")), "[]");
}

#[test]
fn repr_of_a_deep_value_does_not_overflow() {
    let depth = 200_000;
    let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let v = json::loads(&text).expect("deep");
    assert_eq!(py_repr(&v), text);
}

#[test]
fn an_unknown_prefix_is_refused() {
    assert!(
        parse("{}", "rtk-v")
            .unwrap_err()
            .starts_with("unknown lock tag prefix 'rtk-v'")
    );
}

#[test]
fn the_anthology_layout_is_exact() {
    let l = lock("0.1.0", ANTHOLOGY_TAG_PREFIX, "rbonweb/anthology");
    let text = render_anthology(&l);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "{");
    assert_eq!(lines[1], "  \"version\": \"0.1.0\",");
    assert_eq!(lines[2], "  \"tag\": \"anthology-bin--v0.1.0\",");
    assert_eq!(lines[3], "  \"repo\": \"rbonweb/anthology\",");
    assert_eq!(lines[4], "  \"sha256\": {");
    assert_eq!(
        lines[5],
        format!("    \"aarch64-apple-darwin\": \"{HEX}\",")
    );
    assert_eq!(lines[6], format!("    \"x86_64-apple-darwin\": \"{HEX}\","));
    assert_eq!(
        lines[7],
        format!("    \"x86_64-unknown-linux-musl\": \"{HEX}\",")
    );
    assert_eq!(
        lines[8],
        format!("    \"aarch64-unknown-linux-musl\": \"{HEX}\"")
    );
    assert_eq!(lines[9], "  }");
    assert_eq!(lines[10], "}");
    assert_eq!(lines.len(), 11);
    assert!(text.ends_with("}\n"));
    // The same object in another layout is valid JSON but not the file bootstrap.sh reads.
    let compact = json::dumps(&json::loads(&text).expect("json"), &json::PY_DEFAULT);
    assert_eq!(
        parse(&compact, ANTHOLOGY_TAG_PREFIX),
        Err("anthology.lock: not in the layout anthology-dev lock write produces".into())
    );
    assert_eq!(
        parse(&compact, RTK_TAG_PREFIX).map(|l| l.version),
        Err("rtk.lock: tag must be rtk--v0.1.0".into())
    );
}

#[test]
fn the_layout_check_is_public_for_callers_that_decode_themselves() {
    let l = lock("0.1.0", ANTHOLOGY_TAG_PREFIX, "rbonweb/anthology");
    let text = render_anthology(&l);
    let value = json::loads(&text).expect("json");
    let decoded = validate(&value, ANTHOLOGY_TAG_PREFIX).expect("valid");
    assert_eq!(check_anthology_layout(&text, &decoded), Ok(()));
    assert_eq!(check_anthology_layout(text.trim_end(), &decoded), Ok(()));
    for other in [
        format!("{text}\n"),
        text.replace('\n', "\r\n"),
        format!(" {text}"),
    ] {
        assert_eq!(
            check_anthology_layout(&other, &decoded),
            Err("anthology.lock: not in the layout anthology-dev lock write produces".into())
        );
    }
}

fn charset(chars: &'static str, len: std::ops::Range<usize>) -> impl Strategy<Value = String> {
    let chars: Vec<char> = chars.chars().collect();
    prop::collection::vec(prop::sample::select(chars), len).prop_map(|v| v.into_iter().collect())
}

fn repo_part() -> impl Strategy<Value = String> {
    charset(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-",
        1..12,
    )
    .prop_filter("no . or .. part", |p| p != "." && p != "..")
}

fn arb_lock(prefix: &'static str) -> impl Strategy<Value = Lock> {
    let pre = charset(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.",
        1..10,
    );
    (
        (0u32..100_000, 0u32..1000, 0u32..1000),
        prop::option::of(pre),
        repo_part(),
        repo_part(),
        prop::collection::vec(prop::array::uniform32(any::<u8>()), 4),
    )
        .prop_map(move |((a, b, c), pre, owner, name, digests)| {
            let version = match pre {
                Some(p) => format!("{a}.{b}.{c}-{p}"),
                None => format!("{a}.{b}.{c}"),
            };
            Lock {
                tag: format!("{prefix}{version}"),
                version,
                repo: format!("{owner}/{name}"),
                sha256: Target::ALL
                    .iter()
                    .zip(digests)
                    .map(|(t, d)| (*t, d.iter().map(|b| format!("{b:02x}")).collect()))
                    .collect(),
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn render_anthology_round_trips(l in arb_lock(ANTHOLOGY_TAG_PREFIX)) {
        let text = render_anthology(&l);
        prop_assert_eq!(parse(&text, ANTHOLOGY_TAG_PREFIX), Ok(l.clone()));
        prop_assert_eq!(parse(text.trim_end_matches('\n'), ANTHOLOGY_TAG_PREFIX), Ok(l));
    }

    #[test]
    fn an_rtk_lock_in_the_same_layout_parses(l in arb_lock(RTK_TAG_PREFIX)) {
        prop_assume!(rtk_version_ok(&l.version));
        prop_assert_eq!(parse(&render_anthology(&l), RTK_TAG_PREFIX), Ok(l));
    }
}
