//! G6: secrets written into committed memory (guard_rules.py `_g6_file_tool`, `_g6_command`
//! and their helpers). The patterns are `crate::secrets::GUARD_SECRET_PATTERNS`, compiled
//! through `pycompat::re` (GC-6, GC-16) under the check's backtrack limit.

use anthology_pycompat::json::Value;
use anthology_shell::Entry;

use super::{Limit, any_of};
use crate::paths::Paths;
use crate::secrets::GUARD_SECRET_PATTERNS;

/// `_CONTENT_KEYS`.
const CONTENT_KEYS: &[&str] = &["content", "new_string", "new_source"];
/// `_PATH_KEYS`.
const PATH_KEYS: &[&str] = &["file_path", "path", "notebook_path"];
/// `_FILE_WRITERS`.
const FILE_WRITERS: &[&str] = &["tee", "cp", "mv", "install", "rsync", "ln", "dd"];
/// `_IN_PLACE_EDITORS`.
const IN_PLACE_EDITORS: &[&str] = &["sed", "perl"];

/// One compiled G6 pattern.
pub(crate) enum Secret {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl Secret {
    fn search(&self, text: &str) -> Result<bool, Limit> {
        match self {
            Secret::Plain(r) => Ok(r.is_match(text)),
            Secret::Fancy(r) => r.is_match(text).map_err(|_| Limit),
        }
    }
}

/// The G6 patterns, compiled with `limit` (`None` for one that cannot be built: it then counts
/// as a limit, so the check fails closed).
pub(crate) fn compile_secrets(limit: usize) -> Vec<Option<Secret>> {
    GUARD_SECRET_PATTERNS
        .iter()
        .map(|(py, ignorecase)| {
            let flags = anthology_pycompat::re::Flags {
                ignorecase: *ignorecase,
                ..Default::default()
            };
            let t = anthology_pycompat::re::translate_with(py, flags).ok()?;
            if t.fancy {
                fancy_regex::RegexBuilder::new(&t.pattern)
                    .backtrack_limit(limit)
                    .build()
                    .ok()
                    .map(Secret::Fancy)
            } else {
                regex::Regex::new(&t.pattern).ok().map(Secret::Plain)
            }
        })
        .collect()
}

/// `_has_secret(text)`.
pub(crate) fn has_secret(text: &str, secrets: &[Option<Secret>]) -> Result<bool, Limit> {
    any_of(secrets, |s| s.as_ref().ok_or(Limit)?.search(text))
}

/// `_written_strings(tool_input)`: the text a file tool writes.
fn written_strings(input: &Value) -> Vec<&str> {
    let Some(map) = input.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<&str> = CONTENT_KEYS
        .iter()
        .filter_map(|k| map.get(k).and_then(Value::as_str))
        .collect();
    if let Some(Value::Array(edits)) = map.get("edits") {
        for edit in edits {
            if let Some(s) = edit
                .as_object()
                .and_then(|e| e.get("new_string"))
                .and_then(Value::as_str)
            {
                out.push(s);
            }
        }
    }
    out
}

/// `_g6_file_tool(tool_input, home)`: a file tool writing a secret into a memory path.
pub(crate) fn g6_file_tool(
    input: &Value,
    paths: &Paths<'_>,
    secrets: &[Option<Secret>],
) -> Result<bool, Limit> {
    let Some(map) = input.as_object() else {
        return Ok(false);
    };
    let targets: Vec<&str> = PATH_KEYS
        .iter()
        .filter_map(|k| map.get(k).and_then(Value::as_str))
        .collect();
    if !paths.words_hit(&targets, None, |p, c, cwd| p.is_memory_path(c, cwd))? {
        return Ok(false);
    }
    any_of(written_strings(input), |text| has_secret(text, secrets))
}

/// `_IN_PLACE_FLAG.match(arg)`: `-[A-Za-z]*i|--in-place`, anchored at the start: the run of
/// ASCII letters after a `-` holds an `i`, or the argument starts with `--in-place`.
fn in_place_flag(arg: &str) -> bool {
    if arg.starts_with("--in-place") {
        return true;
    }
    arg.strip_prefix('-').is_some_and(|rest| {
        rest.chars()
            .take_while(char::is_ascii_alphabetic)
            .any(|c| c == 'i')
    })
}

/// `_entry_writes_memory(entry, home)`: an output redirection or a file writer whose target is
/// a memory path.
fn entry_writes_memory(entry: &Entry, paths: &Paths<'_>) -> Result<bool, Limit> {
    let cwd = entry.cwd.as_deref();
    let redirected = any_of(&entry.redirs, |r| {
        if r.op.starts_with("&>") || r.op.starts_with('>') {
            paths.words_hit(&[r.target.as_str()], cwd, |p, c, cwd| p.is_memory_path(c, cwd))
        } else {
            Ok(false)
        }
    });
    if redirected == Ok(true) {
        return Ok(true);
    }
    let prog = entry.prog.as_str();
    let writes = if FILE_WRITERS.contains(&prog)
        || (IN_PLACE_EDITORS.contains(&prog) && entry.args.iter().any(|a| in_place_flag(a)))
    {
        paths.words_hit(&entry.args, cwd, |p, c, cwd| p.is_memory_path(c, cwd))
    } else {
        Ok(false)
    };
    super::either(redirected, || writes)
}

/// `_g6_command(command, entries, home)`: some entry writes into memory, and the command text
/// holds a secret.
pub(crate) fn g6_command(
    command: &str,
    entries: &[Entry],
    paths: &Paths<'_>,
    secrets: &[Option<Secret>],
) -> Result<bool, Limit> {
    match any_of(entries, |e| entry_writes_memory(e, paths)) {
        Ok(false) => Ok(false),
        Ok(true) => has_secret(command, secrets),
        Err(Limit) => match has_secret(command, secrets) {
            Ok(false) => Ok(false),
            _ => Err(Limit),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_place_flags() {
        for a in ["-i", "-pi", "-Ei.bak", "-xyzib", "--in-place", "--in-place=.bak"] {
            assert!(in_place_flag(a), "{a}");
        }
        for a in ["-n", "-ab9i", "--inplace", "i", "-", "--i"] {
            assert!(!in_place_flag(a), "{a}");
        }
    }

    #[test]
    fn every_secret_pattern_compiles() {
        let secrets = compile_secrets(1_000_000);
        assert_eq!(secrets.len(), GUARD_SECRET_PATTERNS.len());
        assert!(secrets.iter().all(Option::is_some));
        let token = ["gh", "p_", "abc123"].concat();
        assert_eq!(has_secret(&token, &secrets), Ok(true));
        assert_eq!(has_secret("no secret here", &secrets), Ok(false));
    }
}
