//! Making untrusted text safe to print and to hand to a model (Task 7). The stubs return empty
//! text.

/// `b` as text with invisible and control characters shown as `\u{XXXX}`, invalid UTF-8 as
/// `\x{HH}`, cut at `max_chars` with a trailing `…`.
pub fn escape(_b: &[u8], _max_chars: usize) -> String {
    String::new()
}

/// `s` with every secret it holds replaced by `<secret:{label}>`.
pub fn mask(_s: &str) -> String {
    String::new()
}
