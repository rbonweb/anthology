//! `pycompat::str` against values printed by CPython 3.12 (the pinned image). The difftest
//! surface `pycompat.str` checks every code point; these pin the tricky cases in `cargo test`.

use anthology_pycompat::json::Int;
use anthology_pycompat::str::*;
use anthology_pycompat::tables;

#[test]
fn lower_follows_cpython_including_final_sigma() {
    let cases = [
        ("ΑΣ", "ας"),
        ("ΑΣ Α", "ας α"),
        ("Σ", "σ"),
        ("ΑΣ'", "ας'"),
        ("Α'Σ", "α'ς"),
        ("ΑΣ'Α", "ασ'α"),
        ("Σ\u{308}", "σ\u{308}"),
        ("Α\u{308}Σ", "α\u{308}ς"),
        ("ΣΑ", "σα"),
        ("İSTANBUL", "i\u{307}stanbul"),
        ("ΑΣ\u{345}", "ας\u{345}"),
        ("Abİ", "abi\u{307}"),
        ("ΑΣ.", "ας."),
        ("1Σ", "1σ"),
        ("\u{2b0}Σ", "\u{2b0}σ"),
        ("AΣ\u{2b0}", "aς\u{2b0}"),
        ("ΑΣΣ", "ασς"),
        ("ǅΣ", "ǆς"),
        ("\u{ab70}", "\u{ab70}"),
        ("ABC def", "abc def"),
        ("", ""),
    ];
    for (s, want) in cases {
        assert_eq!(lower(s), want, "{s:?}");
    }
}

#[test]
fn whitespace_split_and_strip_are_pythons() {
    assert_eq!(split_ws(" a  b\x1c c d  "), ["a", "b", "c", "d"]);
    assert_eq!(strip_ws(" a  b\x1c c d  "), "a  b\x1c c d");
    assert_eq!(split_ws("\x1f\x1e x \u{85}"), ["x"]);
    assert_eq!(strip_ws("\x1f\x1e x \u{85}"), "x");
    assert!(split_ws("").is_empty());
    assert!(split_ws("   ").is_empty());
    assert_eq!(strip_ws("   "), "");
    for c in [
        '\t', '\n', '\x0b', '\x0c', '\r', '\x1c', '\x1f', ' ', '\u{85}', '\u{a0}', '\u{3000}',
    ] {
        assert!(is_space(c), "{c:?}");
    }
    for c in ['\u{200b}', '\u{180e}', 'a', '\0'] {
        assert!(!is_space(c), "{c:?}");
    }
}

#[test]
fn digit_predicates_and_categories() {
    assert!(is_digit('²') && !is_decimal('²') && is_alnum('²') && !is_alpha('²'));
    assert!(is_decimal('٣') && is_digit('٣'));
    assert!(is_alpha('ǅ') && is_alnum('ǅ'));
    assert!(is_alnum('½') && !is_digit('½'));
    assert_eq!(decimal_value('٣'), Some(3));
    assert_eq!(decimal_value('9'), Some(9));
    assert_eq!(decimal_value('²'), None);
    assert_eq!(unicode_category('A'), "Lu");
    assert_eq!(unicode_category('\u{378}'), "Cn");
    assert_eq!(unicode_category('\u{e000}'), "Co");
    assert_eq!(unicode_category('\u{10ffff}'), "Cn");
    assert_eq!(unicode_category('\u{1f600}'), "So");
    assert_eq!(unicode_category('_'), "Pc");
    assert_eq!(tables::UNICODE_VERSION, "15.0.0");
}

#[test]
fn code_point_length_and_slices() {
    assert_eq!(char_len("aé😀"), 3);
    assert_eq!(char_slice("aé😀b", 1, 3), "é😀");
    assert_eq!(char_slice("aé😀b", 2, 99), "😀b");
    assert_eq!(char_slice("abc", 2, 1), "");
    assert_eq!(char_slice("abc", 5, 9), "");
}

#[test]
fn repr_is_cpythons() {
    let cases = [
        ("it's", "\"it's\""),
        ("a\"b", "'a\"b'"),
        ("both'\"", "'both\\'\"'"),
        ("\0\x7f\u{80}\u{a0}\u{ad}", "'\\x00\\x7f\\x80\\xa0\\xad'"),
        (
            "é\u{301}\u{200b}😀\u{e0001}\u{d7ff}",
            "'é\u{301}\\u200b😀\\U000e0001\\ud7ff'",
        ),
        ("\\\t\n\r", "'\\\\\\t\\n\\r'"),
    ];
    for (s, want) in cases {
        assert_eq!(repr(s), want, "{s:?}");
    }
}

#[test]
fn py_int_accepts_what_int_accepts() {
    let ok = [
        ("12", 12),
        (" -0 ", 0),
        ("+٣٤", 34),
        ("1_000", 1000),
        (" 5 ", 5),
        ("00", 0),
        ("١_٢", 12),
        ("-9223372036854775808", i64::MIN),
        ("\u{a0}7\u{3000}", 7),
    ];
    for (s, n) in ok {
        assert_eq!(py_int(s), Some(Int::Small(n)), "{s:?}");
    }
    assert_eq!(
        py_int("99999999999999999999"),
        Some(Int::Big("99999999999999999999".into()))
    );
    assert!(py_int(&"9".repeat(4300)).is_some());
    for s in ["1__0", "_1", "1_", "\x1c1", "-", "", "²", "1 2", "0x10"] {
        assert_eq!(py_int(s), None, "{s:?}");
    }
    assert_eq!(py_int(&"9".repeat(4301)), None);
}

#[test]
fn py_float_accepts_what_float_accepts() {
    let ok = [
        ("1.5", 1.5),
        (" 1. ", 1.0),
        (".5", 0.5),
        ("1e5", 100000.0),
        ("1E+05", 100000.0),
        ("1_000.000_1", 1000.0001),
        ("1e1_0", 1e10),
        ("١.٥", 1.5),
        (" 1.5\u{3000}", 1.5),
        ("1e400", f64::INFINITY),
        ("5e-324", 5e-324),
        ("1.e5", 1e5),
        ("-.5e-3", -0.0005),
        ("inf", f64::INFINITY),
        ("-Infinity", f64::NEG_INFINITY),
    ];
    for (s, v) in ok {
        assert_eq!(py_float(s), Some(v), "{s:?}");
    }
    let neg_zero = py_float("-0").unwrap();
    assert!(neg_zero == 0.0 && neg_zero.is_sign_negative());
    assert!(py_float("nAn").unwrap().is_nan());
    assert!(py_float("+nan").unwrap().is_nan());
    for s in [
        ".",
        "1e",
        "1_.5",
        "1._5",
        "\x1c1",
        "0x1p3",
        "in_f",
        "1.5.3",
        "e5",
        ".e5",
        "1_e5",
        "infinityx",
        "  ",
    ] {
        assert_eq!(py_float(s), None, "{s:?}");
    }
}
