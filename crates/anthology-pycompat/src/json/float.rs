//! Float text (F2): Python's `float.__repr__` and JavaScript's `Number.prototype.toString`.
//!
//! Both print the shortest digit string that reads back to the same `f64`, the closest one when
//! several are that short, and **the even one when two are equally close** (CPython's dtoa mode 0
//! rounds half to even; ECMA-262 Number::toString: "choose the one that is even"). They differ
//! only in layout. Rust's `{:e}` gives the shortest closest digits but breaks an exact tie
//! upwards, so [`shortest`] corrects ties.

/// `(digits, exponent)` of Rust's `{:e}`-style text: `d.ddd e X` → `("dddd", X)`, with trailing
/// zeros of the digits removed (`"0"` stays).
fn scientific(text: &str) -> (String, i32) {
    let (mantissa, exp) = text.split_once('e').unwrap_or((text, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let kept = digits.trim_end_matches('0');
    let digits = if kept.is_empty() { "0" } else { kept }.to_string();
    (digits, exp.parse().unwrap_or(0))
}

/// The shortest round-trip digits of a finite `f`: `(digits, decpt)`, meaning
/// `0.<digits> × 10^decpt`. Zero is `("0", 1)`. The sign is ignored.
fn shortest(f: f64) -> (String, i32) {
    let x = f.abs();
    let (digits, exp) = scientific(&format!("{x:e}"));
    let digits = tie_to_even(x, &digits, exp).unwrap_or(digits);
    (digits, exp + 1)
}

/// When `digits` (exponent `exp`, odd last digit) is one of two equally close shortest
/// candidates for `x`, the other, even one.
///
/// A tie means `x` is exactly the midpoint: `x`'s exact decimal expansion is the lower candidate's
/// digits followed by `5`. Shortest digits have at most 17 places, so the midpoint has at most 18:
/// the 18-digit rounding (`{:.17e}`, cheap) filters candidates, and the exact expansion
/// (`{:.800e}`: a double has at most 767 significant digits) confirms one. The other candidate
/// must also read back to `x`.
fn tie_to_even(x: f64, digits: &str, exp: i32) -> Option<String> {
    let last = *digits.as_bytes().last()?;
    if (last - b'0').is_multiple_of(2) {
        return None;
    }
    let head = &digits[..digits.len() - 1];
    let down = format!("{head}{}", char::from(last - 1));
    // `{:e}` rounds a tie up, so the even candidate is normally the lower one; the upper one is
    // checked too, for safety (never across a carry: a carry would make a shorter candidate).
    let mut options = vec![(down.clone(), format!("{down}5"))];
    if last != b'9' {
        options.push((
            format!("{head}{}", char::from(last + 1)),
            format!("{digits}5"),
        ));
    }
    for (other, midpoint) in options {
        if scientific(&format!("{x:.17e}")) != (midpoint.clone(), exp)
            || scientific(&format!("{x:.800e}")) != (midpoint, exp)
        {
            continue;
        }
        let places = other.len() as i32 - 1;
        let back: Option<f64> = format!("{other}e{}", exp - places).parse().ok();
        if back == Some(x) {
            return Some(other);
        }
    }
    None
}

fn exponent_form(digits: &str, decpt: i32, pad: usize) -> String {
    let mut out = String::with_capacity(digits.len() + 8);
    out.push_str(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits[1..]);
    }
    let e = decpt - 1;
    out.push('e');
    out.push(if e < 0 { '-' } else { '+' });
    out.push_str(&format!("{:0pad$}", e.unsigned_abs()));
    out
}

/// Python's `repr(f)` as `json.dumps` prints it: `NaN`, `Infinity`, `-Infinity`, else fixed
/// notation when `-4 < decpt <= 16` (always with a `.`), else `d[.ddd]e±XX`.
pub(crate) fn py_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let (digits, decpt) = shortest(f);
    let sign = if f.is_sign_negative() { "-" } else { "" };
    let n = digits.len() as i32;
    let body = if decpt <= -4 || decpt > 16 {
        exponent_form(&digits, decpt, 2)
    } else if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat(decpt.unsigned_abs() as usize))
    } else if decpt >= n {
        format!("{digits}{}.0", "0".repeat((decpt - n) as usize))
    } else {
        let (int, frac) = digits.split_at(decpt as usize);
        format!("{int}.{frac}")
    };
    format!("{sign}{body}")
}

/// JavaScript's `Number::toString(f)` for a finite `f` (ECMA-262 §6.1.6.1.20); `-0` is `0`.
pub(crate) fn js_number(f: f64) -> String {
    if f == 0.0 {
        return "0".into();
    }
    let (digits, n) = shortest(f);
    let sign = if f < 0.0 { "-" } else { "" };
    let k = digits.len() as i32;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(n as usize);
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(n.unsigned_abs() as usize))
    } else {
        exponent_form(&digits, n, 1)
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_repr_layout() {
        for (f, text) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1234567890123456.0, "1234567890123456.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (5e-324, "5e-324"),
            (1.5e-7, "1.5e-07"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (123.456, "123.456"),
            (f64::NAN, "NaN"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(py_repr(f), text, "{f:e}");
        }
    }

    #[test]
    fn javascript_layout() {
        for (f, text) in [
            (-0.0, "0"),
            (1.0, "1"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (123.456, "123.456"),
            (-2.5, "-2.5"),
            (5e-324, "5e-324"),
            (9007199254740993.0, "9007199254740992"),
        ] {
            assert_eq!(js_number(f), text, "{f:e}");
        }
    }

    /// `(even, odd)`: exact ties between two shortest candidates, found by a 200k-double sweep
    /// against CPython 3.12 and node. Both print `even`; Rust's `{:e}` alone gives `odd`.
    const TIES: [(&str, &str); 52] = [
        ("-1005362108961699.2", "-1005362108961699.3"),
        ("-1008907968779213.2", "-1008907968779213.3"),
        ("-1034243636303790.2", "-1034243636303790.3"),
        ("-1052568991201244.2", "-1052568991201244.3"),
        ("-108868734838530.12", "-108868734838530.13"),
        ("-1165756394280552.2", "-1165756394280552.3"),
        ("-1321860269472500.2", "-1321860269472500.3"),
        ("-1425502010969177.2", "-1425502010969177.3"),
        ("-1564071795740695.2", "-1564071795740695.3"),
        ("-1588876272191213.2", "-1588876272191213.3"),
        ("-1736261076126565.2", "-1736261076126565.3"),
        ("-1781452600791121.2", "-1781452600791121.3"),
        ("-18985994893425.562", "-18985994893425.563"),
        ("-1935706430279870.2", "-1935706430279870.3"),
        ("-2032606824207703.2", "-2032606824207703.3"),
        ("-34144067629171.062", "-34144067629171.063"),
        ("-604887918568506.2", "-604887918568506.3"),
        ("-621653617420272.2", "-621653617420272.3"),
        ("-625666732998560.2", "-625666732998560.3"),
        ("-855662849482537.2", "-855662849482537.3"),
        ("-875432608492266.2", "-875432608492266.3"),
        ("-962939897839142.2", "-962939897839142.3"),
        ("1000000000000000.2", "1000000000000000.3"),
        ("1059438285926254.2", "1059438285926254.3"),
        ("1077004770123625.2", "1077004770123625.3"),
        ("1108521345863294.2", "1108521345863294.3"),
        ("11400367614230.312", "11400367614230.313"),
        ("1181532493496023.2", "1181532493496023.3"),
        ("1317344093802259.2", "1317344093802259.3"),
        ("1371010358211522.2", "1371010358211522.3"),
        ("1452635759377422.2", "1452635759377422.3"),
        ("1474018355666710.2", "1474018355666710.3"),
        ("165106605853604.62", "165106605853604.63"),
        ("1653398604280269.2", "1653398604280269.3"),
        ("1685094889599744.2", "1685094889599744.3"),
        ("1791217536786859.2", "1791217536786859.3"),
        ("1804816367024172.2", "1804816367024172.3"),
        ("1867659689466436.2", "1867659689466436.3"),
        ("2071676411253124.2", "2071676411253124.3"),
        ("214698485734851.12", "214698485734851.13"),
        ("2189850932385181.2", "2189850932385181.3"),
        ("222507385850720.12", "222507385850720.13"),
        ("223692119297042.62", "223692119297042.63"),
        ("26363981746409.312", "26363981746409.313"),
        ("586722369255960.2", "586722369255960.3"),
        ("626352750482887.2", "626352750482887.3"),
        ("660390690203280.2", "660390690203280.3"),
        ("704671096823540.2", "704671096823540.3"),
        ("709412953119813.2", "709412953119813.3"),
        ("756716708459839.2", "756716708459839.3"),
        ("838275818987062.2", "838275818987062.3"),
        ("90339556295875.12", "90339556295875.13"),
    ];

    #[test]
    fn ties_round_half_to_even_in_both_layouts() {
        for (even, odd) in TIES {
            let f: f64 = even.parse().unwrap();
            assert_eq!(odd.parse::<f64>().unwrap(), f, "{odd} is the same double");
            assert_eq!(py_repr(f), even);
            assert_eq!(js_number(f), even);
        }
        for (text, want) in [
            ("1000000000000000.25", "1000000000000000.2"),
            ("1000000000000000.75", "1000000000000000.8"),
            ("90339556295875.125", "90339556295875.12"),
            ("0.5", "0.5"),
            ("2.5", "2.5"),
            ("0.125", "0.125"),
        ] {
            let f: f64 = text.parse().unwrap();
            assert_eq!(
                (py_repr(f).as_str(), js_number(f).as_str()),
                (want, want),
                "{text}"
            );
        }
        assert_eq!(
            (py_repr(1e23).as_str(), js_number(1e23).as_str()),
            ("1e+23", "1e+23")
        );
    }
}
