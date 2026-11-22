//! `pycompat::quote`: `urllib.parse.quote` and `unquote` of CPython 3.12 (design §12).

/// `urllib.parse.quote(s, safe=safe)`: UTF-8, then every byte outside `A-Za-z0-9_.-~` and the
/// ASCII characters of `safe` becomes `%XX` (upper-case hex). Non-ASCII characters in `safe`
/// are ignored, as CPython's `safe.encode("ascii", "ignore")` drops them.
pub fn quote(s: &str, safe: &str) -> String {
    let safe: Vec<u8> = safe.bytes().filter(u8::is_ascii).collect();
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || safe.contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `urllib.parse.unquote(s)`: each run of ASCII characters has its `%XX` escapes (either
/// case) turned into bytes, which are decoded as UTF-8 with `errors="replace"`; runs of other
/// characters are kept as they are, and a `%` not followed by two hex digits stays.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let n = rest
            .char_indices()
            .find(|(_, c)| !c.is_ascii())
            .map_or(rest.len(), |(i, _)| i);
        if n > 0 {
            out.push_str(&String::from_utf8_lossy(&unquote_to_bytes(&rest[..n])));
            rest = &rest[n..];
        } else {
            let m = rest
                .char_indices()
                .find(|(_, c)| c.is_ascii())
                .map_or(rest.len(), |(i, _)| i);
            out.push_str(&rest[..m]);
            rest = &rest[m..];
        }
    }
    out
}

/// `urllib.parse.unquote_to_bytes` of an ASCII string.
fn unquote_to_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() {
            out.push(hex(b[i + 1]).unwrap_or(0) * 16 + hex(b[i + 2]).unwrap_or(0));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn hex(c: u8) -> Option<u8> {
    char::from(c).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_and_unquote() {
        assert_eq!(quote("a b/c~é", "/"), "a%20b/c~%C3%A9");
        assert_eq!(quote("a b/c", ""), "a%20b%2Fc");
        assert_eq!(quote("a:b", ":é"), "a:b");
        assert_eq!(unquote("a%20b%2fc%zz%"), "a b/c%zz%");
        assert_eq!(unquote("%C3%A9%e9x"), "é\u{fffd}x");
        assert_eq!(unquote("100%"), "100%");
    }
}
