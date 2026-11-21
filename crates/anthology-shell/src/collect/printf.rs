//! The text bash's `printf` builtin writes (SH-FW re-review C-4a), so a format filled by arguments and fed to a
//! shell is read as the command it becomes. Escapes (`\n`, `\x20`, the `\...` inside a `%b` argument) are left
//! as written; the caller decodes them afterwards.
//!
//! A superset reading where exact bash output does not matter for the guard: numeric conversions print their
//! argument as written, `%q` prints it unquoted, `%b` ignores a precision (it would cut the decoded text, not the
//! raw one), `%(fmt)T` prints `fmt`, and a bad conversion is copied and the rest still printed (bash stops).

use crate::lexer::{TooComplex, Work};

/// bash's `SKIP1` flags and `LENMODS` length modifiers (printf.def).
const FLAGS: &[u8] = b"#'-+ 0";
const LENMODS: &[u8] = b"hjlLtz";

/// The raw text `printf format args...` writes. Every output byte and every pass over the format is charged to
/// the Rust-only budget.
pub(super) fn printf_fill(
    format: &str,
    args: &[String],
    work: &mut Work,
) -> Result<String, TooComplex> {
    let mut out = String::new();
    let mut next = 0;
    loop {
        work.charge_extra(format.len().max(1))?;
        let start = next;
        fill_once(format, args, &mut next, &mut out, work)?;
        // bash reuses the format while arguments remain, but only after a pass that used one.
        if next == start || next >= args.len() {
            return Ok(out);
        }
    }
}

/// One pass over the format, taking arguments from `args[*next..]`.
fn fill_once(
    format: &str,
    args: &[String],
    next: &mut usize,
    out: &mut String,
    work: &mut Work,
) -> Result<(), TooComplex> {
    let bytes = format.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // An escape is one unit (bash decodes it in place), so `\%` never starts a conversion.
                let len = 1 + format[i + 1..].chars().next().map_or(0, char::len_utf8);
                emit(out, &format[i..i + len], work)?;
                i += len;
            }
            b'%' => i = conversion(format, i, args, next, out, work)?,
            _ => {
                let end = bytes[i..]
                    .iter()
                    .position(|&b| b == b'\\' || b == b'%')
                    .map_or(bytes.len(), |p| i + p);
                emit(out, &format[i..end], work)?;
                i = end;
            }
        }
    }
    Ok(())
}

/// The conversion starting at the `%` at `start`; returns the index after it.
fn conversion(
    format: &str,
    start: usize,
    args: &[String],
    next: &mut usize,
    out: &mut String,
    work: &mut Work,
) -> Result<usize, TooComplex> {
    let bytes = format.as_bytes();
    let mut j = start + 1;
    if bytes.get(j) == Some(&b'%') {
        emit(out, "%", work)?;
        return Ok(j + 1);
    }
    let mut left = false;
    while let Some(&b) = bytes.get(j).filter(|b| FLAGS.contains(b)) {
        left |= b == b'-';
        j += 1;
    }
    let width = if bytes.get(j) == Some(&b'*') {
        j += 1;
        let w = int_arg(take(args, next));
        left |= w < 0;
        usize::try_from(w.unsigned_abs()).unwrap_or(usize::MAX)
    } else {
        digits(bytes, &mut j).unwrap_or(0)
    };
    let mut precision = None;
    if bytes.get(j) == Some(&b'.') {
        j += 1;
        precision = if bytes.get(j) == Some(&b'*') {
            j += 1;
            // A negative precision is as if none was given (C).
            usize::try_from(int_arg(take(args, next))).ok()
        } else {
            Some(digits(bytes, &mut j).unwrap_or(0))
        };
    }
    while bytes.get(j).is_some_and(|b| LENMODS.contains(b)) {
        j += 1;
    }
    let Some(conv) = format[j..].chars().next() else {
        // A trailing `%` (or `%5`, ...): no conversion character, copied as written.
        emit(out, &format[start..], work)?;
        return Ok(format.len());
    };
    let after = j + conv.len_utf8();
    let text: &str = match conv {
        's' | 'q' => cut(take(args, next).unwrap_or(""), precision),
        'b' => take(args, next).unwrap_or(""),
        'c' => {
            let arg = take(args, next).unwrap_or("");
            &arg[..arg.chars().next().map_or(0, char::len_utf8)]
        }
        'd' | 'i' | 'u' | 'o' | 'x' | 'X' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'a' | 'A' => {
            take(args, next).unwrap_or("0")
        }
        '(' => match format[after..].find(')') {
            Some(p) if bytes.get(after + p + 1) == Some(&b'T') => {
                take(args, next);
                pad(out, &format[after..after + p], width, left, work)?;
                return Ok(after + p + 2);
            }
            _ => {
                emit(out, &format[start..after], work)?;
                return Ok(after);
            }
        },
        _ => {
            // An unknown conversion: copied as written, and the rest is still printed (bash stops here).
            emit(out, &format[start..after], work)?;
            return Ok(after);
        }
    };
    pad(out, text, width, left, work)?;
    Ok(after)
}

/// The next argument, if one is left.
fn take<'a>(args: &'a [String], next: &mut usize) -> Option<&'a str> {
    let arg = args.get(*next)?;
    *next += 1;
    Some(arg)
}

/// A `*` width or precision argument: bash reads an integer; anything else (or none) is 0.
fn int_arg(arg: Option<&str>) -> i64 {
    let arg = arg.unwrap_or("").trim_start();
    let (negative, digits) = match arg.as_bytes().first() {
        Some(b'-') => (true, &arg[1..]),
        Some(b'+') => (false, &arg[1..]),
        _ => (false, arg),
    };
    let value = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            n.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    if negative { -value } else { value }
}

/// A run of decimal digits at `*j` (saturating), advancing past it; `None` when there is none.
fn digits(bytes: &[u8], j: &mut usize) -> Option<usize> {
    let begin = *j;
    let mut n: usize = 0;
    while let Some(&d) = bytes.get(*j).filter(|b| b.is_ascii_digit()) {
        n = n.saturating_mul(10).saturating_add(usize::from(d - b'0'));
        *j += 1;
    }
    (*j > begin).then_some(n)
}

/// `text` cut to `precision` characters.
fn cut(text: &str, precision: Option<usize>) -> &str {
    match precision.and_then(|p| text.char_indices().nth(p)) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// `text` padded with spaces to `width` characters, on the left unless `left`.
fn pad(
    out: &mut String,
    text: &str,
    width: usize,
    left: bool,
    work: &mut Work,
) -> Result<(), TooComplex> {
    // Counting stops at `width`, so the scan is bounded by the padding it decides.
    let shown = text.chars().take(width).count();
    let fill = width - shown;
    work.charge_extra(fill)?;
    if !left {
        out.extend(std::iter::repeat_n(' ', fill));
    }
    emit(out, text, work)?;
    if left {
        out.extend(std::iter::repeat_n(' ', fill));
    }
    Ok(())
}

fn emit(out: &mut String, text: &str, work: &mut Work) -> Result<(), TooComplex> {
    work.charge_extra(text.len())?;
    out.push_str(text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Reading;

    fn fill_with(budget: usize, format: &str, args: &[&str]) -> Result<String, TooComplex> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let mut work = Work::new(usize::MAX, budget, Reading::Bash5);
        printf_fill(format, &args, &mut work)
    }

    fn fill(format: &str, args: &[&str]) -> String {
        fill_with(1 << 20, format, args).unwrap()
    }

    #[test]
    fn payloads_show_the_command() {
        assert!(fill(r"npx %s install\n", &["playwright"]).contains("npx playwright install"));
        assert!(fill("npx%splaywright install", &[" "]).contains("npx playwright install"));
        assert!(fill("%c%s", &["n", "px playwright install"]).contains("npx playwright install"));
        assert!(
            fill("%ls %(x)T", &["npx playwright install", "0"])
                .contains("npx playwright install x")
        );
    }

    #[test]
    fn format_reused_while_args_remain() {
        assert_eq!(fill(r"%s-%s\n", &["a", "b", "c"]), r"a-b\nc-\n");
    }

    #[test]
    fn width_precision_and_star() {
        assert_eq!(fill("%5s|", &["ab"]), "   ab|");
        assert_eq!(fill("%-3s|", &["a"]), "a  |");
        assert_eq!(fill("%.2s|", &["abc"]), "ab|");
        assert_eq!(fill("%*s|", &["4", "a"]), "   a|");
        assert_eq!(fill("%*s|", &["-3", "a"]), "a  |");
        assert_eq!(fill("%.*s|", &["1", "xyz"]), "x|");
        assert_eq!(fill("%.1s|", &["\u{e9}t\u{e9}"]), "\u{e9}|");
    }

    #[test]
    fn percent_char_and_numbers() {
        assert_eq!(fill("100%%", &[]), "100%");
        assert_eq!(fill("[%c]", &[""]), "[]");
        assert_eq!(fill("[%c]", &[]), "[]");
        assert_eq!(fill("%d %x", &["42"]), "42 0");
        assert_eq!(fill("%b", &[r"\x6epx"]), r"\x6epx");
        assert_eq!(fill(r"\%s", &["a"]), r"\%s");
    }

    #[test]
    fn unknown_and_trailing_percent_copied() {
        assert_eq!(fill("a%yb %s", &["c"]), "a%yb c");
        assert_eq!(fill("a%z", &[]), "a%z");
        assert_eq!(fill("ab%", &[]), "ab%");
        assert_eq!(fill("ab%5", &[]), "ab%5");
    }

    #[test]
    fn no_conversions_printed_once() {
        assert_eq!(fill("hi ", &["a", "b", "c"]), "hi ");
    }

    #[test]
    fn big_input_stays_linear() {
        let args: Vec<String> = (0..20_000).map(|n| n.to_string()).collect();
        let total: usize = args.iter().map(String::len).sum();
        let mut work = Work::new(usize::MAX, 8 * total, Reading::Bash5);
        let out = printf_fill("%s ", &args, &mut work).unwrap();
        assert_eq!(out.len(), total + args.len());
        assert_eq!(
            fill_with(1 << 16, "%999999999999999999999s", &["a"]),
            Err(TooComplex)
        );
        assert_eq!(
            fill_with(1 << 16, "%*s", &["99999999999", "a"]),
            Err(TooComplex)
        );
        assert_eq!(
            fill_with(1 << 10, "%s", &[&"x".repeat(1 << 11)]),
            Err(TooComplex)
        );
    }
}
