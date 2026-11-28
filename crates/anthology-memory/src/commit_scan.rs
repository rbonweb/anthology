//! The commit scan: what `anthology privacy hook` looks for in the staged diff and the message of a
//! `git commit`.
//!
//! It blocks only the owner's own literals, never every email address or IP address: the home
//! path and the user's handles, the git email, the machine and tailnet names, and the names in the
//! owner's banned-names notes. A finding is a kind and a location (a file and a line), never the
//! matched text, and the block message holds the same. The owner can let a value through with a
//! line in `.claude/privacy-allow.txt`.
//!
//! The matching is literals and the plain `regex` crate, ASCII case-insensitive: the line is
//! lowercased by ASCII rules (the byte offsets stay put) and compared with lowercased literals.
//! The word-boundary rules are written out by hand, as the plain engine has no look-around. It
//! uses the helpers of [`crate::privacy`] for the home literal, the machine names and the email
//! exemptions, so a commit and a repo-memory write treat the same values the same way.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::privacy::{exempt_email, home_literal, hostname_names};

/// What the owner's details are, for one scan.
#[derive(Clone, Debug, Default)]
pub struct ScanContext {
    /// `$HOME`, as set (the literal searched for is [`home_literal`] of it).
    pub home: Option<String>,
    /// The owner's handles: `$HOME`'s last folder, `$USER` and `$LOGNAME`, without the generic
    /// ones (the caller drops those).
    pub handles: Vec<String>,
    /// The addresses the owner commits with (`git config user.email`).
    pub emails: Vec<String>,
    /// The machine's host name.
    pub hostname: Option<String>,
    /// The names in the owner's banned-names notes ([`banned_names`]).
    pub banned: Vec<String>,
    /// Lines of the allow file: a value that one of these holds is let through. Empty lines and
    /// lines that start with `#` allow nothing.
    pub allow: Vec<String>,
}

/// What a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommitKind {
    HomePath,
    UserEmail,
    BannedName,
    MachineName,
}

impl CommitKind {
    /// The words the block message uses.
    pub fn label(self) -> &'static str {
        match self {
            CommitKind::HomePath => "home path",
            CommitKind::UserEmail => "user email",
            CommitKind::BannedName => "banned name",
            CommitKind::MachineName => "machine name",
        }
    }
}

/// One finding: its kind and where it is. Never the matched text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFinding {
    pub kind: CommitKind,
    /// The path of the file in the diff, or [`MESSAGE_FILE`] for the commit message.
    pub file: String,
    /// 1-based: the line in the new file, or in the message.
    pub line: usize,
}

/// The `file` of a finding in the commit message.
pub const MESSAGE_FILE: &str = "commit message";

/// The file name shown for an added line that follows no file header.
const UNKNOWN_FILE: &str = "(unknown file)";

/// How many places of one kind the block message names.
const SHOWN: usize = 3;

/// The longest a banned-names note is read.
const NOTE_MAX: u64 = 1 << 20;

/// The findings in the added lines of a `git diff -U0` (staged or not). A removed or context line
/// is not part of the commit, and the file header lines are not scanned. Line numbers come from
/// the `@@ -a,b +c,d @@` headers. One finding per kind and line, in the order of the diff.
pub fn scan_diff(diff: &str, ctx: &ScanContext) -> Vec<CommitFinding> {
    let matcher = Matcher::new(ctx);
    let mut out = Vec::new();
    let mut file = String::new();
    let (mut old_left, mut new_left, mut line_no) = (0usize, 0usize, 0usize);
    for raw in diff.split('\n') {
        // Inside a hunk, the header counts say which lines are its body: an added line that
        // reads `++ x` is `+++ x` in the diff, and is a line, not a file header.
        if old_left > 0 || new_left > 0 {
            match raw.as_bytes().first() {
                Some(b'+') => {
                    matcher.scan_line(&raw[1..], &file, line_no, &mut out);
                    line_no += 1;
                    new_left = new_left.saturating_sub(1);
                    continue;
                }
                Some(b'-') => {
                    old_left = old_left.saturating_sub(1);
                    continue;
                }
                Some(b' ') => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    line_no += 1;
                    continue;
                }
                // `\ No newline at end of file`
                Some(b'\\') => continue,
                // Not a body line: the hunk was shorter than its header said.
                _ => {
                    old_left = 0;
                    new_left = 0;
                }
            }
        }
        if raw.starts_with("diff --git ") {
            file.clear();
        } else if let Some(name) = raw.strip_prefix("+++ ") {
            file = new_name(name);
        } else if raw.starts_with("@@ ") {
            if let Some(((_, old), (start, new))) = hunk_counts(raw) {
                old_left = old;
                new_left = new;
                line_no = start;
            }
        } else if let Some(text) = raw.strip_prefix('+') {
            // An added line outside a counted hunk (a diff written by hand).
            matcher.scan_line(text, &file, line_no, &mut out);
            line_no += 1;
        }
    }
    out
}

/// The findings in a commit message, as lines of the file [`MESSAGE_FILE`].
pub fn scan_message(msg: &str, ctx: &ScanContext) -> Vec<CommitFinding> {
    let matcher = Matcher::new(ctx);
    let mut out = Vec::new();
    for (i, line) in msg.split('\n').enumerate() {
        matcher.scan_line(line, MESSAGE_FILE, i + 1, &mut out);
    }
    out
}

/// The names the owner banned, from `<anthology_home>/personal-memory/user-banned-names.md` and,
/// for one repo, `user-banned-names--<repo_name>.md`. The frontmatter is skipped; each `- name` or
/// `* name` line is a name, without its backticks and quotes; a name under 3 characters is
/// dropped. A missing or unreadable note adds nothing.
pub fn banned_names(anthology_home: &Path, repo_name: &str) -> Vec<String> {
    let dir = anthology_home.join("personal-memory");
    let mut names = read_names(&dir.join("user-banned-names.md"));
    // The repo name is a folder name; one that could name another folder reads no list.
    let plain = !repo_name.is_empty()
        && repo_name != "."
        && repo_name != ".."
        && !repo_name.contains(['/', '\\', '\0']);
    if plain {
        names.extend(read_names(
            &dir.join(format!("user-banned-names--{repo_name}.md")),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.clone()));
    names
}

fn read_names(path: &Path) -> Vec<String> {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    // A regular file only: reading a FIFO could block the hook.
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(NOTE_MAX).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines().peekable();
    if lines.peek().is_some_and(|l| l.trim_end() == "---") {
        lines.next();
        // The frontmatter ends at the next `---`; without one the file has no body.
        for line in lines.by_ref() {
            if line.trim_end() == "---" {
                break;
            }
        }
    }
    lines
        .filter_map(|line| {
            let rest = line
                .trim_start()
                .strip_prefix("- ")
                .or_else(|| line.trim_start().strip_prefix("* "))?;
            let name = rest
                .trim()
                .trim_matches(|c| c == '`' || c == '"' || c == '\'')
                .trim();
            (name.chars().count() >= 3).then(|| name.to_string())
        })
        .collect()
}

/// The message that blocks the commit: each kind with up to three places, then `and N more`.
/// Never a matched value.
pub fn block_message(f: &[CommitFinding]) -> String {
    let mut kinds: Vec<CommitKind> = Vec::new();
    for finding in f {
        if !kinds.contains(&finding.kind) {
            kinds.push(finding.kind);
        }
    }
    let parts: Vec<String> = kinds
        .iter()
        .map(|&kind| {
            let places: Vec<String> = f
                .iter()
                .filter(|x| x.kind == kind)
                .map(|x| format!("{}:{}", escape_controls(&x.file), x.line))
                .collect();
            let mut shown = places[..places.len().min(SHOWN)].join(", ");
            if places.len() > SHOWN {
                shown.push_str(&format!(", and {} more", places.len() - SHOWN));
            }
            format!("{} ({shown})", kind.label())
        })
        .collect();
    format!(
        "Blocked by anthology (privacy): this commit adds {}. Replace a home path with a repo-relative path, a name with a neutral one or the project's own, then stage and commit again. A value the owner wants public: ask the owner to add it as a line in .claude/privacy-allow.txt.",
        parts.join("; ")
    )
}

/// A file name is text from the repo: control characters are written as escapes, so a name cannot
/// reach a terminal as an escape sequence.
fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// The path in a `+++ ` line without its `b/` prefix: a C-quoted name is unquoted, and the tab git
/// adds after a name with a space is dropped. Empty for `/dev/null`.
fn new_name(name: &str) -> String {
    let name = name.strip_suffix('\t').unwrap_or(name);
    let name = match name
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(quoted) => unquote(quoted),
        None => name.to_string(),
    };
    if name == "/dev/null" {
        return String::new();
    }
    match name.strip_prefix("b/") {
        Some(rel) => rel.to_string(),
        None => name,
    }
}

/// The inside of a C-quoted git path (`na\303\257ve`) as text.
fn unquote(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        if b != b'\\' || i >= bytes.len() {
            out.push(b);
            continue;
        }
        let esc = bytes[i];
        i += 1;
        out.push(match esc {
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'0'..=b'3' if bytes.len() - i >= 2 => {
                let digits = &bytes[i..i + 2];
                if digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                    i += 2;
                    (esc - b'0') * 64 + (digits[0] - b'0') * 8 + (digits[1] - b'0')
                } else {
                    esc
                }
            }
            other => other,
        });
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `((old start, old count), (new start, new count))` of `@@ -a[,b] +c[,d] @@`; a missing count
/// is 1.
fn hunk_counts(line: &str) -> Option<((usize, usize), (usize, usize))> {
    let mut parts = line.split(' ');
    parts.next()?;
    let old = side(parts.next()?.strip_prefix('-')?)?;
    let new = side(parts.next()?.strip_prefix('+')?)?;
    Some((old, new))
}

fn side(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

// --- matching -------------------------------------------------------------------------------

/// A character of a word: a letter, a digit or `_`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A word character or `-`: what must not touch a host name or a home literal.
fn is_name_char(c: char) -> bool {
    is_word(c) || c == '-'
}

fn char_before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

fn char_after(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

/// The end of the path that runs on from `end`: up to a space, a quote, a bracket or the like,
/// without the punctuation that closes a sentence.
fn path_end(text: &str, end: usize) -> usize {
    let stop = |c: char| c.is_whitespace() || "\"'`<>()[]{}|;,*?!^".contains(c);
    let run = text[end..].find(stop).map_or(text.len(), |k| end + k);
    text[..run].trim_end_matches(['.', ':']).len().max(end)
}

/// The byte offset of every occurrence of `needle` in `text`, overlapping ones too.
fn occurrences<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    let mut from = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() {
            return None;
        }
        let at = from + text.get(from..)?.find(needle)?;
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
        Some(at)
    })
}

/// `[a-z0-9._%+-]+@domain`, on a lowercased line. The plain engine has no look-behind: a match
/// starts at the first character of its run, and [`Matcher::email`] looks at what follows.
fn email_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"([a-z0-9._%+-]+)@([a-z0-9-]+(?:\.[a-z0-9-]+)*\.[a-z]{2,})")
            .expect("the email pattern compiles")
    })
}

/// A host on a tailnet, on a lowercased line.
fn tailnet_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[a-z0-9-]+(?:\.[a-z0-9-]+)*\.ts\.net").expect("the tailnet pattern compiles")
    })
}

/// A context made ready for matching: every literal lowercased once.
struct Matcher {
    /// The lowercased `$HOME` literal.
    home: Option<String>,
    /// `(form, strict)`: a lowercased handle form, and whether the character after it must not
    /// continue a name. The dash forms end with the dash that ends the handle.
    handle_forms: Vec<(String, bool)>,
    handles: Vec<String>,
    emails: Vec<String>,
    hosts: Vec<String>,
    banned: Vec<String>,
    allow: Vec<String>,
}

impl Matcher {
    fn new(ctx: &ScanContext) -> Matcher {
        let lower = |s: &str| s.to_ascii_lowercase();
        let handles: Vec<String> = ctx
            .handles
            .iter()
            .map(|h| lower(h.trim()))
            .filter(|h| !h.is_empty())
            .collect();
        let mut handle_forms = Vec::new();
        for h in &handles {
            for form in [
                format!("/users/{h}"),
                format!("/home/{h}"),
                format!("\\users\\{h}"),
            ] {
                handle_forms.push((form, true));
            }
            for form in [format!("-users-{h}-"), format!("-home-{h}-")] {
                handle_forms.push((form, false));
            }
        }
        Matcher {
            home: ctx
                .home
                .as_deref()
                .and_then(home_literal)
                .map(|h| lower(&h)),
            handle_forms,
            handles,
            emails: ctx
                .emails
                .iter()
                .map(|e| lower(e.trim()))
                .filter(|e| !e.is_empty())
                .collect(),
            hosts: ctx
                .hostname
                .as_deref()
                .map(hostname_names)
                .unwrap_or_default()
                .iter()
                .map(|h| lower(h))
                .collect(),
            banned: ctx
                .banned
                .iter()
                .map(|b| lower(b.trim()))
                .filter(|b| !b.is_empty())
                .collect(),
            allow: ctx
                .allow
                .iter()
                .map(|a| lower(a.trim()))
                .filter(|a| !a.is_empty() && !a.starts_with('#'))
                .collect(),
        }
    }

    /// Scans one line of text; the findings go on `out`.
    fn scan_line(&self, text: &str, file: &str, line: usize, out: &mut Vec<CommitFinding>) {
        if text.is_empty() {
            return;
        }
        let lower = text.to_ascii_lowercase();
        let found = [
            (CommitKind::HomePath, self.home_path(&lower)),
            (CommitKind::UserEmail, self.email(&lower)),
            (CommitKind::BannedName, self.banned_name(&lower)),
            (CommitKind::MachineName, self.machine_name(&lower)),
        ];
        for (kind, hit) in found {
            if hit {
                out.push(CommitFinding {
                    kind,
                    file: if file.is_empty() {
                        UNKNOWN_FILE.to_string()
                    } else {
                        file.to_string()
                    },
                    line,
                });
            }
        }
    }

    /// True unless an allowed line holds the matched text.
    fn kept(&self, matched: &str) -> bool {
        !self.allow.iter().any(|a| a.contains(matched))
    }

    /// `$HOME` (not followed by a name character) and the handle forms. For the allow file the
    /// matched text is the whole path the match starts, so a line that allows one path does not
    /// allow every other path under the same home.
    fn home_path(&self, line: &str) -> bool {
        if let Some(home) = &self.home {
            for at in occurrences(line, home) {
                let end = at + home.len();
                if !char_after(line, end).is_some_and(is_name_char)
                    && self.kept(&line[at..path_end(line, end)])
                {
                    return true;
                }
            }
        }
        for (form, strict) in &self.handle_forms {
            for at in occurrences(line, form) {
                let end = at + form.len();
                // A handle form ends at a character that cannot continue a file name.
                let ends = !strict
                    || !char_after(line, end)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
                if ends && self.kept(&line[at..path_end(line, end)]) {
                    return true;
                }
            }
        }
        false
    }

    /// An address that is the owner's: a configured one, or one whose local part is a handle.
    /// The reserved, no-reply and forge addresses are nobody's.
    fn email(&self, line: &str) -> bool {
        if self.emails.is_empty() && self.handles.is_empty() {
            return false;
        }
        for caps in email_regex().captures_iter(line) {
            let (Some(whole), Some(local), Some(domain)) = (caps.get(0), caps.get(1), caps.get(2))
            else {
                continue;
            };
            let before = char_before(line, whole.start());
            let after = char_after(line, whole.end());
            if before.is_some_and(|c| is_word(c) || ".%+-".contains(c))
                || after.is_some_and(is_name_char)
            {
                continue;
            }
            if exempt_email(local.as_str(), domain.as_str()) {
                continue;
            }
            let owner = self.emails.iter().any(|e| e == whole.as_str())
                || self.handles.iter().any(|h| h == local.as_str());
            if owner && self.kept(whole.as_str()) {
                return true;
            }
        }
        false
    }

    /// A banned name, as a whole word at each end that is a word character.
    fn banned_name(&self, line: &str) -> bool {
        for name in &self.banned {
            let first_word = name.chars().next().is_some_and(is_word);
            let last_word = name.chars().next_back().is_some_and(is_word);
            for at in occurrences(line, name) {
                let end = at + name.len();
                let before_ok = !first_word || !char_before(line, at).is_some_and(is_word);
                let after_ok = !last_word || !char_after(line, end).is_some_and(is_word);
                if before_ok && after_ok && self.kept(&line[at..end]) {
                    return true;
                }
            }
        }
        false
    }

    /// The machine's name as a word, and any host on a tailnet.
    fn machine_name(&self, line: &str) -> bool {
        for name in &self.hosts {
            for at in occurrences(line, name) {
                let end = at + name.len();
                let touches = char_before(line, at).is_some_and(is_name_char)
                    || char_after(line, end).is_some_and(is_name_char);
                if !touches && self.kept(&line[at..end]) {
                    return true;
                }
            }
        }
        if line.contains(".ts.net") {
            for m in tailnet_regex().find_iter(line) {
                let touches = char_before(line, m.start())
                    .is_some_and(|c| is_name_char(c) || c == '.')
                    || char_after(line, m.end()).is_some_and(is_name_char);
                if !touches && self.kept(m.as_str()) {
                    return true;
                }
            }
        }
        false
    }
}
