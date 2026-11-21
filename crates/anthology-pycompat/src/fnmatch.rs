//! `pycompat::fnmatch`: `fnmatch.fnmatchcase` of CPython 3.12 (design §12).
//!
//! [`translate`] is `fnmatch.translate` step for step (bracket rules, `!`, a leading `]`, empty
//! ranges removed, `\` and `-` escaped, set operations escaped), except that each `*` becomes
//! `.*`: 3.12 writes an interior `*fixed` as the atomic `(?>.*?fixed)`, which accepts exactly the
//! same names (each fixed piece is a run of one-character items, so its earliest occurrence is
//! always a valid choice), and the `regex` crate needs no atomic groups to be linear. The result
//! is a Python pattern, compiled by [`crate::re`], so its classes mean what they mean in Python.

use crate::re;

/// A compiled shell pattern.
#[derive(Clone, Debug)]
pub struct Pattern(re::Regex);

impl Pattern {
    /// `fnmatch.fnmatchcase(name, pat)` for this pattern.
    pub fn matches(&self, name: &str) -> bool {
        // The translation never needs fancy-regex, so this cannot hit a backtrack limit.
        self.0.is_match(name).unwrap_or(false)
    }
}

/// Compiles `pat`; `None` only if Python's `re` would refuse the translation too.
pub fn compile(pat: &str) -> Option<Pattern> {
    // `fnmatchcase` uses `re.match`, which is anchored at the start.
    re::compile(&format!("\\A{}", translate(pat)), false)
        .ok()
        .map(Pattern)
}

/// `fnmatch.fnmatchcase(name, pat)`: case-sensitive, `*` and `?` match `/` too. It compiles
/// `pat` on every call (Python caches it): to match many names against one pattern, use
/// [`compile`] once and [`Pattern::matches`].
pub fn fnmatchcase(name: &str, pat: &str) -> bool {
    compile(pat).is_some_and(|p| p.matches(name))
}

/// Python's `re.escape` (3.7+: only the special characters).
fn escape(c: char) -> String {
    if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// `fnmatch.translate(pat)`, with `.*` for every `*` (module docs).
pub fn translate(pat: &str) -> String {
    let p: Vec<char> = pat.chars().collect();
    let n = p.len();
    let mut res = String::new();
    let mut last_star = false;
    let mut i = 0;
    while i < n {
        let c = p[i];
        i += 1;
        if c == '*' {
            if !last_star {
                res.push_str(".*");
                last_star = true;
            }
            continue;
        }
        last_star = false;
        if c == '?' {
            res.push('.');
        } else if c == '[' {
            let mut j = i;
            if j < n && p[j] == '!' {
                j += 1;
            }
            if j < n && p[j] == ']' {
                j += 1;
            }
            while j < n && p[j] != ']' {
                j += 1;
            }
            if j >= n {
                res.push_str("\\[");
                continue;
            }
            let span: String = p[i..j].iter().collect();
            let mut stuff = if !span.contains('-') {
                span.replace('\\', "\\\\")
            } else {
                let mut chunks: Vec<String> = Vec::new();
                let mut k = if p[i] == '!' { i + 2 } else { i + 1 };
                loop {
                    match (k..j).find(|&x| p[x] == '-') {
                        None => break,
                        Some(found) => {
                            chunks.push(p[i..found].iter().collect());
                            i = found + 1;
                            k = found + 3;
                        }
                    }
                }
                let chunk: String = p[i..j].iter().collect();
                if !chunk.is_empty() {
                    chunks.push(chunk);
                } else if let Some(last) = chunks.last_mut() {
                    last.push('-');
                }
                // Remove empty ranges -- invalid in RE.
                let mut k = chunks.len().saturating_sub(1);
                while k > 0 {
                    let prev_last = chunks[k - 1].chars().last();
                    let first = chunks[k].chars().next();
                    if let (Some(a), Some(b)) = (prev_last, first)
                        && a > b
                    {
                        let mut merged: Vec<char> = chunks[k - 1].chars().collect();
                        merged.pop();
                        merged.extend(chunks[k].chars().skip(1));
                        chunks[k - 1] = merged.into_iter().collect();
                        chunks.remove(k);
                    }
                    k -= 1;
                }
                chunks
                    .iter()
                    .map(|s| s.replace('\\', "\\\\").replace('-', "\\-"))
                    .collect::<Vec<_>>()
                    .join("-")
            };
            // Escape set operations (&&, ~~ and ||).
            stuff = stuff
                .chars()
                .map(|c| {
                    if "&~|".contains(c) {
                        format!("\\{c}")
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            i = j + 1;
            if stuff.is_empty() {
                // Empty range: never match (3.12 writes `(?!)`).
                res.push_str("[^\\x00-\\U0010ffff]");
            } else if stuff == "!" {
                res.push('.');
            } else {
                if let Some(rest) = stuff.strip_prefix('!') {
                    stuff = format!("^{rest}");
                } else if stuff.starts_with(['^', '[']) {
                    stuff = format!("\\{stuff}");
                }
                res.push('[');
                res.push_str(&stuff);
                res.push(']');
            }
        } else {
            res.push_str(&escape(c));
        }
    }
    format!("(?s:{res})\\Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_classes() {
        assert!(fnmatchcase("a/b.txt", "*.txt"));
        assert!(fnmatchcase("a", "[!b]"));
        assert!(!fnmatchcase("b", "[!b]"));
        assert!(fnmatchcase("]", "[]]"));
        assert!(fnmatchcase("-", "[a-]"));
        assert!(!fnmatchcase("x", "[z-a]"));
        assert!(fnmatchcase("[", "["));
        assert!(fnmatchcase("a\nb", "a?b"));
        assert!(!fnmatchcase("A", "a"));
    }
}
