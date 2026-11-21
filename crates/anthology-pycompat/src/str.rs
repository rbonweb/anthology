//! `pycompat::str`: Python `str` semantics — case, whitespace, digits, `repr` (design §12,
//! plan §3.2, GC-16).
//!
//! Every predicate reads the tables `anthology-dev gen-unicode` generated from CPython 3.12
//! (`tables.rs`, Unicode 15.0), never Rust's own `char` methods, whose Unicode version differs.
//! Counting and slicing are in code points, as Python's are.

use crate::json::Int;
use crate::tables;

/// Is `c` in one of the sorted inclusive runs?
fn in_runs(runs: &[(u32, u32)], c: char) -> bool {
    let c = u32::from(c);
    runs.binary_search_by(|&(lo, hi)| {
        if hi < c {
            std::cmp::Ordering::Less
        } else if lo > c {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

/// `c.isspace()`: includes `\x1c`–`\x1f`, U+0085, U+00A0 and the Unicode spaces.
pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        return matches!(c, '\t'..='\r' | '\x1c'..=' ');
    }
    in_runs(tables::SPACE, c)
}

/// `c.isdigit()` (Numeric_Type Digit or Decimal: superscripts count, unlike `isdecimal`).
pub fn is_digit(c: char) -> bool {
    in_runs(tables::DIGIT, c)
}

/// `c.isdecimal()` (Unicode Nd): what `int()` and a str pattern's `\d` accept.
pub fn is_decimal(c: char) -> bool {
    in_runs(tables::DECIMAL, c)
}

/// `c.isalpha()` (categories Lu, Ll, Lt, Lm, Lo).
pub fn is_alpha(c: char) -> bool {
    in_runs(tables::ALPHA, c)
}

/// `c.isalnum()`: `isalpha`, `isdecimal`, `isdigit` or `isnumeric`.
pub fn is_alnum(c: char) -> bool {
    in_runs(tables::ALNUM, c)
}

/// `c.isprintable()`: what `repr` prints as is.
pub fn is_printable(c: char) -> bool {
    in_runs(tables::PRINTABLE, c)
}

/// What a str pattern's `\w` matches: `c.isalnum() or c == "_"`.
pub fn is_word(c: char) -> bool {
    in_runs(tables::WORD, c)
}

/// Case_Ignorable, as `str.lower` uses it for Final_Sigma.
pub fn is_case_ignorable(c: char) -> bool {
    in_runs(tables::CASE_IGNORABLE, c)
}

/// Cased, for a character that is not case-ignorable (the only ones Final_Sigma asks about;
/// a case-ignorable character answers `false`).
pub(crate) fn is_cased(c: char) -> bool {
    in_runs(tables::CASED, c)
}

/// `unicodedata.decimal(c, None)`.
pub fn decimal_value(c: char) -> Option<u32> {
    let cp = u32::from(c);
    let i = tables::DECIMAL_VALUE.partition_point(|&(_, hi, _)| hi < cp);
    match tables::DECIMAL_VALUE.get(i) {
        Some(&(lo, _, v)) if lo <= cp => Some(u32::from(v) + (cp - lo)),
        _ => None,
    }
}

/// `unicodedata.category(c)`; an unassigned code point is `"Cn"`.
pub fn unicode_category(c: char) -> &'static str {
    let cp = u32::from(c);
    let i = tables::CATEGORY.partition_point(|&(_, hi, _)| hi < cp);
    match tables::CATEGORY.get(i) {
        Some(&(lo, _, k)) if lo <= cp => tables::CATEGORY_NAMES[usize::from(k)],
        _ => "Cn",
    }
}

const CAPITAL_SIGMA: char = '\u{3a3}';

/// `s.lower()`: the full lowercase mapping (`'İ'.lower()` is two characters) and CPython's
/// Final_Sigma rule (`'ΑΣ'.lower() == 'ας'`, `'Σ'.lower() == 'σ'`).
pub fn lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    if !s.contains(CAPITAL_SIGMA) {
        for c in s.chars() {
            push_lower(&mut out, c);
        }
        return out;
    }
    let cs: Vec<char> = s.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        if c == CAPITAL_SIGMA {
            out.push(if final_sigma(&cs, i) {
                '\u{3c2}'
            } else {
                '\u{3c3}'
            });
        } else {
            push_lower(&mut out, c);
        }
    }
    out
}

/// CPython's `handle_capital_sigma`: `\p{cased}\p{case-ignorable}*Σ!(\p{case-ignorable}*\p{cased})`.
fn final_sigma(cs: &[char], i: usize) -> bool {
    let before = cs[..i].iter().rev().find(|&&c| !is_case_ignorable(c));
    if !before.is_some_and(|&c| is_cased(c)) {
        return false;
    }
    match cs[i + 1..].iter().find(|&&c| !is_case_ignorable(c)) {
        None => true,
        Some(&c) => !is_cased(c),
    }
}

fn push_lower(out: &mut String, c: char) {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return;
    }
    let cp = u32::from(c);
    if let Ok(i) = tables::LOWER.binary_search_by_key(&cp, |&(k, _)| k) {
        // The generator only writes valid scalar values.
        out.push(char::from_u32(tables::LOWER[i].1).unwrap_or(c));
    } else if let Ok(i) = tables::LOWER_MULTI.binary_search_by_key(&cp, |&(k, _)| k) {
        out.push_str(tables::LOWER_MULTI[i].1);
    } else {
        out.push(c);
    }
}

/// `s.split()` (no separator): runs of whitespace separate, and there are no empty parts.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// `s.strip()` (no argument).
pub fn strip_ws(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `len(s)`: code points.
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `s[a:b]` in code points, clamped as Python clamps (`a >= b` gives `""`).
pub fn char_slice(s: &str, a: usize, b: usize) -> &str {
    if a >= b {
        return "";
    }
    let start = byte_at(s, a);
    let end = byte_at(s, b);
    &s[start..end]
}

/// The byte offset of code point `n`, or `s.len()` past the end.
fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// `repr(s)`: CPython's quote choice and escapes (`\\`, `\t`, `\n`, `\r`, `\xhh`, `\uhhhh`,
/// `\Uhhhhhhhh` for what `isprintable` rejects).
pub fn repr(s: &str) -> String {
    repr_points(&s.chars().map(u32::from).collect::<Vec<_>>())
}

/// `repr(os.fsdecode(b))` on a POSIX system: `b` decoded as UTF-8 with `surrogateescape`, so
/// each byte that is not part of valid UTF-8 is a lone surrogate U+DC80..U+DCFF, which `repr`
/// writes `\udcXX`.
pub fn repr_surrogateescape(b: &[u8]) -> String {
    let mut points = Vec::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        points.extend(chunk.valid().chars().map(u32::from));
        points.extend(chunk.invalid().iter().map(|&x| 0xDC00 + u32::from(x)));
    }
    repr_points(&points)
}

/// `repr` over code points, lone surrogates included.
fn repr_points(points: &[u32]) -> String {
    let has = |c: char| points.contains(&u32::from(c));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(points.len() + 2);
    out.push(quote);
    for &n in points {
        match char::from_u32(n) {
            Some('\\') => out.push_str("\\\\"),
            Some('\t') => out.push_str("\\t"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            Some(c) if c == quote => {
                out.push('\\');
                out.push(c);
            }
            Some(c @ ' '..='~') => out.push(c),
            Some(c) if !c.is_ascii() && is_printable(c) => out.push(c),
            _ => {
                if n < 0x100 {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n < 0x1_0000 {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// CPython's `_PyUnicode_TransformDecimalAndSpaceToASCII`, then the ASCII-space strip of
/// `PyLong_FromString`/`float()`: ASCII stays, other whitespace becomes `' '`, other decimal
/// digits become their ASCII digit, anything else `'?'` (never valid in a number).
fn ascii_number_text(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c
            } else if is_space(c) {
                ' '
            } else if let Some(d) = decimal_value(c) {
                char::from(b'0' + d as u8)
            } else {
                '?'
            }
        })
        .collect();
    // Py_ISSPACE: only space, \t, \n, \v, \f and \r (not \x1c-\x1f).
    t.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'))
        .to_string()
}

/// Digits with single underscores between them (`_Py_string_to_number_with_underscores` and
/// `PyLong_FromString` base 10): the digits without the underscores, or `None`.
fn digits_with_underscores(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() || !b[b.len() - 1].is_ascii_digit() {
        return None;
    }
    let mut out = String::with_capacity(b.len());
    let mut prev_underscore = false;
    for &c in b {
        match c {
            b'0'..=b'9' => {
                out.push(char::from(c));
                prev_underscore = false;
            }
            b'_' if !prev_underscore => prev_underscore = true,
            _ => return None,
        }
    }
    Some(out)
}

/// Python's default limit on the digits of an `int()` from text (`sys.int_info.default_max_str_digits`).
pub const INT_MAX_STR_DIGITS: usize = 4300;

/// `int(s)` for a str, base 10: `None` where Python raises `ValueError`. Unicode whitespace
/// around it and Unicode decimal digits are accepted; so are single underscores between digits
/// and leading zeros. More than 4,300 digits is a `ValueError`, as in Python.
pub fn py_int(s: &str) -> Option<Int> {
    let t = ascii_number_text(s);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, &t[..]),
    };
    let digits = digits_with_underscores(body)?;
    if digits.len() > INT_MAX_STR_DIGITS {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(Int::Small(0));
    }
    let text = if neg {
        format!("-{digits}")
    } else {
        digits.to_string()
    };
    Some(match text.parse::<i64>() {
        Ok(n) => Int::Small(n),
        Err(_) => Int::Big(text),
    })
}

/// `float(s)` for a str: `None` where Python raises `ValueError`. Accepts what CPython does:
/// surrounding (Unicode) whitespace, Unicode decimal digits, a sign, `inf`/`infinity`/`nan` in
/// any case, `1.`, `.5`, exponents, and single underscores between digits.
pub fn py_float(s: &str) -> Option<f64> {
    let t = ascii_number_text(s);
    let (sign, body) = match t.as_bytes().first() {
        Some(b'-') => ("-", &t[1..]),
        Some(b'+') => ("", &t[1..]),
        _ => ("", &t[..]),
    };
    let low = body.to_ascii_lowercase();
    if matches!(low.as_str(), "inf" | "infinity" | "nan") {
        let v = if low == "nan" {
            f64::NAN
        } else {
            f64::INFINITY
        };
        return Some(if sign == "-" { -v } else { v });
    }
    // mantissa: digits [. [digits]] | . digits; then [e [sign] digits]. Underscores only
    // between two digits.
    let (mant, exp) = match low.find('e') {
        Some(i) => (&low[..i], Some(&low[i + 1..])),
        None => (&low[..], None),
    };
    let (int_part, frac_part) = match mant.find('.') {
        Some(i) => (&mant[..i], Some(&mant[i + 1..])),
        None => (mant, None),
    };
    let part = |p: &str| -> Option<String> {
        if p.is_empty() {
            Some(String::new())
        } else {
            digits_with_underscores(p)
        }
    };
    let ip = part(int_part)?;
    let fp = match frac_part {
        Some(f) => part(f)?,
        None => String::new(),
    };
    if ip.is_empty() && fp.is_empty() {
        return None;
    }
    let mut clean = format!("{sign}{ip}");
    if frac_part.is_some() {
        clean.push('.');
        clean.push_str(&fp);
    }
    if let Some(e) = exp {
        let (esign, edigits) = match e.as_bytes().first() {
            Some(b'-') => ("-", &e[1..]),
            Some(b'+') => ("", &e[1..]),
            _ => ("", e),
        };
        let ed = digits_with_underscores(edigits)?;
        clean.push('e');
        clean.push_str(esign);
        clean.push_str(&ed);
    }
    // A plain decimal float now; Rust's parse is correctly rounded, like CPython's dtoa.
    // Rust needs a digit before a bare "." exponent ("1.e5" and ".5" both parse).
    clean.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_runs_finds_the_edges() {
        let runs = [(2, 4), (8, 8)];
        let hit: Vec<u32> = (0..10)
            .filter(|&n| in_runs(&runs, char::from_u32(n).unwrap()))
            .collect();
        assert_eq!(hit, [2, 3, 4, 8]);
        assert!(!in_runs(&[], 'a'));
    }

    #[test]
    fn byte_at_counts_code_points() {
        assert_eq!(byte_at("aé b", 0), 0);
        assert_eq!(byte_at("aé b", 2), 3);
        assert_eq!(byte_at("aé b", 9), 5);
    }
}
