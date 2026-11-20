//! Float text (F2): Python's `float.__repr__` and JavaScript's `Number.prototype.toString`.
//!
//! Both print the shortest digit string that reads back to the same `f64` (the closest one when
//! several are that short), which is what Rust's `{:e}` gives; they differ only in layout.

/// The shortest round-trip digits of a finite `f`: `(digits, decpt)`, meaning
/// `0.<digits> × 10^decpt`. Zero is `("0", 1)`. The sign is ignored.
fn shortest(f: f64) -> (String, i32) {
    let text = format!("{:e}", f.abs());
    let (mantissa, exp) = text.split_once('e').unwrap_or((&text, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exp: i32 = exp.parse().unwrap_or(0);
    (digits, exp + 1)
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
}
