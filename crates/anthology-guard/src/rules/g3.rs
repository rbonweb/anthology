//! G3: Chrome profile paths (guard_rules.py `_g3`, and the tool-input part of `_check`).

use anthology_pycompat::json::Value;
use anthology_shell::Entry;

use super::{Limit, either};
use crate::paths::{Paths, code_strings, path_strings, search_roots};

/// `_PLAIN_TEXT_KEYS`: written text and the shell command, never read as paths.
const PLAIN_TEXT_KEYS: &[&str] = &[
    "content",
    "new_string",
    "new_source",
    "old_string",
    "edits",
    "command",
];

/// The G3 test on a tool's input: strings under path-like keys, the Glob/Grep search roots,
/// and code under code keys. Grep's `pattern` is a regex over file contents, not a path;
/// Glob's is a path glob.
pub(crate) fn g3_input(tool_name: &str, input: &Value, paths: &Paths<'_>) -> Result<bool, Limit> {
    let grep = tool_name == "Grep";
    let mut skip: Vec<&str> = PLAIN_TEXT_KEYS.to_vec();
    if grep {
        skip.push("pattern");
    }
    let mut words: Vec<String> = Vec::new();
    let mut found = Vec::new();
    path_strings(input, &skip, &mut found);
    words.extend(found.iter().map(|s| (*s).to_string()));
    let keys: &[&str] = if grep { &["glob"] } else { &["pattern", "glob"] };
    words.extend(search_roots(input, keys));
    either(
        paths.words_hit(&words, None, |p, c, cwd| p.is_profile_path(c, cwd, false)),
        || {
            let mut code = Vec::new();
            code_strings(input, &mut code);
            paths.code_hits_profile(&code, None)
        },
    )
}

/// `_g3(entry, home)`: a profile path among the words or redirection targets, or spelled
/// inside the code the entry runs.
pub(crate) fn g3(entry: &Entry, paths: &Paths<'_>) -> Result<bool, Limit> {
    let words: Vec<&str> = entry
        .raw
        .iter()
        .map(String::as_str)
        .chain(entry.redirs.iter().map(|r| r.target.as_str()))
        .collect();
    let cwd = entry.cwd.as_deref();
    either(
        paths.words_hit(&words, cwd, |p, c, cwd| p.is_profile_path(c, cwd, false)),
        || paths.code_hits_profile(&entry.code, cwd),
    )
}
