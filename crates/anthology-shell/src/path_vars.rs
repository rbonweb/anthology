//! Carve-out (d) (T4, ADR-038): a whole double-quoted `"$NAME"` word read as the file the variable
//! names, for the names a caller vouches for (`PathVars`). The guard passes the host variables the
//! user listed; atl passes none, so its analysis never changes.
//!
//! `analyze_with_vars` reads each `"$NAME"` word, in the program or an argument position, as the
//! canonical path (`PathValue::canonical`), and reads the value as written as another reading of the
//! same command. The read-through stands only when the whole command is plain (`path_vars_allowed`);
//! otherwise the command is analyzed as it is without variables.

use std::collections::BTreeMap;

use crate::analyze::Entry;

/// The variables a caller vouches for: their values are what the shell will put in place.
pub trait PathVars {
    /// `name`'s value when a `"$name"` word may be read as the file it names, or `None` to leave the
    /// word as it is.
    fn resolve(&self, name: &str) -> Option<PathValue>;
}

/// A host variable's value: an absolute path to a regular file.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PathValue {
    /// The value as written.
    pub value: String,
    /// The value with every symlink followed: what the rules read.
    pub canonical: String,
    /// The file may be run as a program; a word in the program position is read only then.
    pub executable: bool,
}

/// Carve-out (a)'s directory prefix names (`"$HOME/…"`): a caller does not offer them as host
/// variables, since carve-out (a) reads them in its own way.
pub const DIRECTORY_PREFIX_NAMES: &[&str] = crate::lexer::PREFIX_NAMES;

/// Carve-out (d)'s plain command: the whole text keeps carve-out (a)'s allowlist, with a `"$NAME"`
/// whole word of a read name (`used`) as the only expansion, no read name anywhere else, and nothing
/// that sources a file or runs `eval`.
///
/// - Every character outside those words is one of `[A-Za-z0-9_./:=,+@%-]`, a blank, a quote or one of
///   `;&|<>`, with no `<<` (here-string, here-doc) and no `<` before a digit or `-` (zsh's `<->` and
///   `<1-9>` patterns, which `typeset -m` matches names with).
/// - Every `$` is in double quotes opened right before it, at the start of a word (the start of the text,
///   a blank or one of `;&|` before the quote, outside every quote) that is not a redirection target,
///   and is followed by a read NAME and the closing quote at the end of a word (the end of the text, a
///   blank or one of `;&|<>`). So no `$` inside single quotes, in another word, or in braces.
/// - No read NAME appears in the text outside those words, or in any word of the analysis (`entries`,
///   quotes removed, so `HO''ST` counts): a plain substring match, so it over-blocks. An alternate
///   reading holds the words of its entry, or the values as written.
///
/// The text check and the word check overlap: a `"$NAME"` form that is not a whole word leaves
/// `$NAME` in a word of the analysis too, unless it is a redirection target. Each stands on its own.
pub(crate) fn path_vars_allowed(
    command: &str,
    entries: &[Entry],
    used: &BTreeMap<String, PathValue>,
) -> bool {
    const WORD: &[u8] = b"_./:=,+@%-";
    const SYNTAX: &[u8] = b" \t\n\"';&|<>";
    const BEFORE: &[u8] = b" \t\n;&|";
    const AFTER: &[u8] = b" \t\n;&|<>";
    let b = command.as_bytes();
    let mut covered = vec![false; b.len()];
    let (mut single, mut double) = (false, false);
    let mut k = 0;
    while k < b.len() {
        let c = b[k];
        if c == b'$' {
            // a double quote is never open inside single quotes (the toggles below)
            let opened = double && k > 0 && b[k - 1] == b'"';
            let at_start = opened && (k == 1 || BEFORE.contains(&b[k - 2]));
            let end = (k + 1..b.len())
                .find(|&j| !(b[j].is_ascii_alphanumeric() || b[j] == b'_'))
                .unwrap_or(b.len());
            let closed =
                b.get(end) == Some(&b'"') && b.get(end + 1).is_none_or(|c| AFTER.contains(c));
            if !(at_start
                && closed
                && !redirected(b, k - 1)
                && used.contains_key(&command[k + 1..end]))
            {
                return false;
            }
            covered[k - 1..=end].iter_mut().for_each(|c| *c = true);
            double = false;
            k = end + 1;
            continue;
        }
        if !(c.is_ascii_alphanumeric() || WORD.contains(&c) || SYNTAX.contains(&c))
            || command[k..].starts_with("<<")
            || (c == b'<'
                && b.get(k + 1)
                    .is_some_and(|n| n.is_ascii_digit() || *n == b'-'))
        {
            return false;
        }
        match c {
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            _ => {}
        }
        k += 1;
    }
    // `builtin source` and `command .` are peeled to `source` and `.` before they become entries
    let runs_text = entries
        .iter()
        .any(|e| matches!(e.prog.as_str(), "source" | "." | "eval"));
    !runs_text
        && used.keys().all(|name| {
            !command
                .match_indices(name.as_str())
                .any(|(i, _)| !covered[i])
                && !entries
                    .iter()
                    .flat_map(|e| &e.raw)
                    .any(|w| w.contains(name.as_str()))
        })
}

/// The double quote at `quote` opens a redirection target: the text before it, past blanks, ends in
/// `<` or `>`, or in `&` or `|` right after one of them (`>&`, `>|`, `<&`).
fn redirected(b: &[u8], quote: usize) -> bool {
    let Some(p) = b[..quote].iter().rposition(|c| !b" \t".contains(c)) else {
        return false;
    };
    matches!(b[p], b'<' | b'>')
        || (matches!(b[p], b'&' | b'|') && p > 0 && matches!(b[p - 1], b'<' | b'>'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text check alone, with no words of an analysis to fall back on.
    fn text_allowed(command: &str) -> bool {
        let value = PathValue {
            value: "/opt/hosttool/bin/tool".into(),
            canonical: "/opt/hosttool/bin/tool".into(),
            executable: true,
        };
        let used = BTreeMap::from([("HOST_TOOL".to_string(), value)]);
        path_vars_allowed(command, &[], &used)
    }

    #[test]
    fn a_whole_double_quoted_word_ends_at_its_closing_quote() {
        for command in [
            r#""$HOST_TOOL" x"#,
            r#""$HOST_TOOL""#,
            r#"a;"$HOST_TOOL"|b"#,
            r#""$HOST_TOOL">f"#,
        ] {
            assert!(text_allowed(command), "{command}");
        }
        for command in [
            // not closed right after the name, though a word end follows
            r#""$HOST_TOOL  x""#,
            r#""$HOST_TOOL;x""#,
            // closed, but the word goes on
            r#""$HOST_TOOL"x"#,
            r#""$HOST_TOOL"'x'"#,
            r#""$HOST_TOOL""x""#,
        ] {
            assert!(!text_allowed(command), "{command}");
        }
    }
}
