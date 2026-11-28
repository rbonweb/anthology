//! The quarantine fetch of a URL target (Task 6).
//!
//! A URL is fetched with `git init`, `git fetch --depth 1` and `git checkout` into a new folder:
//! no clone (so no submodule, no template hooks), no user or system git config, no `file:` or
//! `ext::` transport, no hooks, no LFS filters, no prompt. Nothing of the fetched tree runs.
//!
//! `fetch` reads three variables of the process environment (`PATH`, `HOME`, `SSH_AUTH_SOCK`) to
//! build the cleared environment it hands to git; nothing else here reads the environment.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::tools::Runner;

/// The oldest git with `GIT_CONFIG_GLOBAL`, which keeps the user's git config out of the fetch.
const GLOBAL_CONFIG_GIT: (u32, u32, u32) = (2, 32, 0);
/// Older git has CVE-2022-23521 and CVE-2022-41903 (memory errors on a crafted `.gitattributes`)
/// unless the distribution backported the fixes, so an older one is noted, never refused.
const SAFE_GIT: (u32, u32, u32) = (2, 39, 1);
const MAX_URL: usize = 2048;
const MAX_REV: usize = 200;
/// The longest piece (host, owner or repo) of a quarantine folder name.
const NAME_PART: usize = 64;
/// How much of git's stderr an error message keeps.
const STDERR_CAP: usize = 300;

/// The start of every git call: no hooks, no fsmonitor, no symlinks on disk, no `file:` or helper
/// transports, objects checked on fetch, no submodules and no LFS filters.
const GIT_PREFIX: [&str; 22] = [
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.symlinks=false",
    "-c",
    "protocol.file.allow=never",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "transfer.fsckObjects=true",
    "-c",
    "fetch.fsckObjects=true",
    "-c",
    "submodule.recurse=false",
    "-c",
    "filter.lfs.smudge=",
    "-c",
    "filter.lfs.process=",
    "-c",
    "filter.lfs.required=false",
];

/// What each of the six git steps is called in an error message.
const STEP_NAMES: [&str; 6] = [
    "version check",
    "init",
    "remote add",
    "fetch",
    "checkout",
    "ls-files",
];

/// What to fetch and where to put it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchPlan {
    pub url: String,
    pub rev: Option<String>,
    pub dest: PathBuf,
}

/// `s` if it is an `https://`, `ssh://` or `git@host:` URL without options, whitespace or
/// control characters.
pub fn validate_url(s: &str) -> Result<String, String> {
    let bad_form = || {
        format!(
            "{} is not a git URL: use https://, ssh:// or git@<host>:<path>",
            shown(s)
        )
    };
    if s.is_empty() {
        return Err("the URL is empty".to_string());
    }
    if s.len() > MAX_URL {
        return Err(format!("the URL is longer than {MAX_URL} bytes"));
    }
    if s.starts_with('-') {
        return Err(format!(
            "{} starts with '-', so git would read it as an option",
            shown(s)
        ));
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "{} has whitespace or a control character",
            shown(s)
        ));
    }
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("ssh://"))
    {
        let authority = rest.split('/').next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        if authority.starts_with('-') || host.is_empty() || host.starts_with('-') {
            return Err(bad_form());
        }
        Ok(s.to_string())
    } else if let Some(rest) = s.strip_prefix("git@") {
        let (host, path) = rest.split_once(':').ok_or_else(bad_form)?;
        let host_ok = !host.is_empty()
            && !host.starts_with('-')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !host_ok || path.is_empty() {
            return Err(bad_form());
        }
        Ok(s.to_string())
    } else {
        Err(bad_form())
    }
}

/// `s` if it is a commit hash, a branch or a tag name that cannot be read as an option.
pub fn validate_rev(s: &str) -> Result<String, String> {
    // A hash of 7 to 40 lowercase hex digits is a name of this form too.
    let first = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'/';
    let ok = !s.is_empty()
        && s.len() <= MAX_REV
        && first(s.as_bytes()[0])
        && s.bytes().all(|b| first(b) || b == b'-');
    if ok {
        Ok(s.to_string())
    } else {
        Err(format!(
            "{} is not a commit hash, branch or tag name (letters, digits, . _ / -; at most {MAX_REV} characters; no leading -)",
            shown(s)
        ))
    }
}

/// The folder name for a fetch of `url` at `epoch`: `<host>-<owner>-<repo>-<epoch>`. Every part
/// is cut to `[A-Za-z0-9._-]`, so the name is one plain folder name whatever the URL says.
pub fn dest_name(url: &str, epoch: u64) -> String {
    let (host, path) = split_url(url);
    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let repo = segments
        .pop()
        .map(|s| s.strip_suffix(".git").unwrap_or(s))
        .map(clean_part)
        .unwrap_or_default();
    let mut owner: String = segments
        .into_iter()
        .map(clean_part)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    owner.truncate(NAME_PART);
    let host = clean_part(host);
    let or = |s: String, fallback: &str| {
        if s.is_empty() {
            fallback.to_string()
        } else {
            s
        }
    };
    format!(
        "{}-{}-{}-{epoch}",
        or(host, "host"),
        or(owner, "unknown"),
        or(repo, "repo")
    )
}

/// The git calls that fetch `p`, in order; `template` is an empty folder for `git init`. Each
/// list is the arguments after `git`.
pub fn git_steps(p: &FetchPlan, template: &Path) -> Vec<Vec<OsString>> {
    let os = |s: &str| OsString::from(s);
    let step = |rest: Vec<OsString>| -> Vec<OsString> {
        GIT_PREFIX.iter().map(|s| os(s)).chain(rest).collect()
    };
    let in_dest = |rest: &[&str]| -> Vec<OsString> {
        let mut v = vec![os("-C"), p.dest.as_os_str().to_owned()];
        v.extend(rest.iter().map(|s| os(s)));
        step(v)
    };
    let mut template_arg = os("--template=");
    template_arg.push(template);
    let rev = p.rev.as_deref().unwrap_or("HEAD");

    let mut remote_add = in_dest(&["remote", "add", "origin"]);
    remote_add.push(os(&p.url));
    let mut fetch_args = in_dest(&[
        "fetch",
        "-q",
        "--depth",
        "1",
        "--no-tags",
        "--recurse-submodules=no",
        "origin",
    ]);
    fetch_args.push(os(rev));

    vec![
        step(vec![os("--version")]),
        step(vec![
            os("init"),
            os("-q"),
            template_arg,
            p.dest.as_os_str().to_owned(),
        ]),
        remote_add,
        fetch_args,
        in_dest(&[
            "checkout",
            "-q",
            "--no-recurse-submodules",
            "-B",
            "check-repo",
            "FETCH_HEAD",
        ]),
        in_dest(&["ls-files", "-s", "-z"]),
    ]
}

/// The whole environment of a git call: `path` is `PATH`, `home` is `HOME`, `ssh` is
/// `SSH_AUTH_SOCK` when set. Nothing else of the user's environment reaches git.
pub fn git_env(path: &OsStr, home: &Path, ssh: Option<&OsStr>) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vec![
        ("PATH".into(), path.to_owned()),
        ("HOME".into(), home.as_os_str().to_owned()),
    ];
    if let Some(sock) = ssh {
        env.push(("SSH_AUTH_SOCK".into(), sock.to_owned()));
    }
    for (k, v) in [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_LFS_SKIP_SMUDGE", "1"),
        ("GIT_ALLOW_PROTOCOL", "https:ssh"),
        ("LC_ALL", "C"),
    ] {
        env.push((k.into(), v.into()));
    }
    env
}

/// Runs the steps of `p` through `r` within `timeout`, removes `p.dest` on failure and returns
/// the `(path, mode)` pairs of `git ls-files -s`.
///
/// `p.dest` must not exist: a folder that is already there is refused and never touched. The
/// folder is made with mode 0700. The modes are git's octal modes as numbers: `0o120000` is a
/// symlink, `0o160000` a submodule, `0o100755` an executable, `0o100644` a plain file.
pub fn fetch(
    p: &FetchPlan,
    r: &dyn Runner,
    timeout: Duration,
) -> Result<Vec<(String, u32)>, String> {
    fetch_with_notes(p, r, timeout).map(|done| done.modes)
}

/// What a fetch hands back: git's file modes, and the notes the report carries (an old git).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub modes: Vec<(String, u32)>,
    pub notes: Vec<String>,
}

/// The empty folder `git init --template` reads for the quarantine at `dest`: a hidden sibling,
/// `<parent>/.<name>.template`. `fetch` makes it and removes it; a caller that has to clean up
/// after an interrupt removes it too.
pub fn template_path(dest: &Path) -> Option<PathBuf> {
    let (parent, name) = (dest.parent()?, dest.file_name()?);
    let mut template_name = OsString::from(".");
    template_name.push(name);
    template_name.push(".template");
    Some(parent.join(template_name))
}

/// `fetch`, with the notes. An old git is a note, never a refusal.
pub fn fetch_with_notes(
    p: &FetchPlan,
    r: &dyn Runner,
    timeout: Duration,
) -> Result<Fetched, String> {
    let deadline = Instant::now() + timeout;
    validate_url(&p.url)?;
    if let Some(rev) = &p.rev {
        validate_rev(rev)?;
    }
    // Absolute, so `-C <dest>` means the same folder whatever the working folder of git is.
    let dest = std::path::absolute(&p.dest)
        .map_err(|e| format!("cannot resolve {}: {e}", p.dest.display()))?;
    if dest.symlink_metadata().is_ok() {
        return Err(format!(
            "{} already exists; the quarantine folder must be new",
            dest.display()
        ));
    }
    let git = r.find("git").ok_or_else(|| {
        "git was not found on PATH; fetching a URL needs git (2.39.1 or newer is best)".to_string()
    })?;
    let (Some(parent), Some(template)) = (dest.parent(), template_path(&dest)) else {
        return Err(format!("{} is not a usable folder name", dest.display()));
    };
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    // An empty folder next to the quarantine: `git init --template` copies nothing from it, so
    // no hook or config of a system template reaches the new repo.
    std::fs::create_dir(&template)
        .map_err(|e| format!("cannot create {}: {e}", template.display()))?;
    if let Err(e) = make_private_dir(&dest) {
        let _ = std::fs::remove_dir_all(&template);
        return Err(format!("cannot create {}: {e}", dest.display()));
    }

    let plan = FetchPlan {
        url: p.url.clone(),
        rev: p.rev.clone(),
        dest: dest.clone(),
    };
    let result = run_steps(r, &git, &plan, &template, parent, deadline, timeout);
    let _ = std::fs::remove_dir_all(&template);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    result
}

/// The six steps, each within what is left of the one deadline.
fn run_steps(
    r: &dyn Runner,
    git: &Path,
    plan: &FetchPlan,
    template: &Path,
    cwd: &Path,
    deadline: Instant,
    total: Duration,
) -> Result<Fetched, String> {
    let path = child_path(std::env::var_os("PATH"));
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| template.to_path_buf());
    let ssh = std::env::var_os("SSH_AUTH_SOCK");
    let env = git_env(&path, &home, ssh.as_deref());

    let mut listing = Vec::new();
    let mut notes = Vec::new();
    for (i, args) in git_steps(plan, template).iter().enumerate() {
        let name = STEP_NAMES[i];
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| format!("the fetch timed out (limit {total:?}) before git {name}"))?;
        let (code, stdout, stderr) = r.run(git, args, cwd, &env, left).map_err(|e| {
            if e.kind() == io::ErrorKind::TimedOut {
                format!("git {name} timed out (the whole fetch is limited to {total:?})")
            } else {
                format!("git {name} did not run: {e}")
            }
        })?;
        if code != 0 {
            return Err(format!(
                "git {name} failed (exit {code}): {}",
                last_line(&stderr)
            ));
        }
        match i {
            0 => notes.extend(version_note(&String::from_utf8_lossy(&stdout))),
            // The repo `git init` just made gets the protections before anything is fetched
            // into it, and stays safe for the `git -C <dest> status` the report suggests.
            1 => harden_config(&plan.dest)?,
            5 => listing = stdout,
            _ => {}
        }
    }
    Ok(Fetched {
        modes: parse_ls_files(&listing)?,
        notes,
    })
}

/// The protections appended to the quarantine's own `.git/config`: no symlinks written to disk,
/// no hooks and no fsmonitor command run by a later git call in the folder.
const QUARANTINE_CONFIG: &str = "[core]\n\tsymlinks = false\n\thooksPath = /dev/null\n\tfsmonitor = false\n";

/// Appends `QUARANTINE_CONFIG` to `<dest>/.git/config`, which `git init` has just made.
fn harden_config(dest: &Path) -> Result<(), String> {
    use std::io::Write;
    let config = dest.join(".git").join("config");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&config)
        .and_then(|mut f| f.write_all(QUARANTINE_CONFIG.as_bytes()))
        .map_err(|e| format!("cannot protect the quarantine's {}: {e}", config.display()))
}

/// The note for what `git --version` printed, if the git is older than `SAFE_GIT` or its version
/// cannot be read.
fn version_note(out: &str) -> Option<String> {
    let floor = format!("{}.{}.{}", SAFE_GIT.0, SAFE_GIT.1, SAFE_GIT.2);
    let Some(v) = parse_version(out) else {
        return Some(format!(
            "cannot read the git version from {}, so it was not checked against {floor}",
            shown(out.trim())
        ));
    };
    if v >= SAFE_GIT {
        return None;
    }
    let mut note = format!(
        "git {}.{}.{} is older than {floor}, which fixes CVE-2022-23521 and CVE-2022-41903 (memory errors on a crafted .gitattributes); your distribution may have backported them, otherwise update git",
        v.0, v.1, v.2
    );
    if v < GLOBAL_CONFIG_GIT {
        note.push_str(
            "; it also ignores GIT_CONFIG_GLOBAL, so your own git config applied to this fetch",
        );
    }
    Some(note)
}

/// `PATH` for a child: the absolute entries only (a relative one, such as an empty entry or `.`,
/// would let a program in the working folder run), or the system folders when none is left.
fn child_path(raw: Option<OsString>) -> OsString {
    raw.map(|p| absolute_path_entries(&p))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| OsString::from("/usr/bin:/bin"))
}

/// The absolute entries of the `PATH` text `path`, in order, joined again.
pub fn absolute_path_entries(path: &OsStr) -> OsString {
    let kept: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|p| p.is_absolute())
        .collect();
    std::env::join_paths(kept).unwrap_or_default()
}

/// Reads the git version out of `git --version` output: `git version 2.39.5 (Apple Git-154)`.
/// A missing patch level reads as 0.
fn parse_version(out: &str) -> Option<(u32, u32, u32)> {
    let v = out.trim().strip_prefix("git version ")?;
    let digits = v.split(|c: char| !c.is_ascii_digit() && c != '.').next()?;
    let mut parts = digits.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// The `(path, mode)` pairs of `git ls-files -s -z`: records `<mode> <hash> <stage>\t<path>`
/// ended by NUL.
fn parse_ls_files(raw: &[u8]) -> Result<Vec<(String, u32)>, String> {
    let bad = || "cannot read the file list git printed".to_string();
    let mut out = Vec::new();
    for rec in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = rec.iter().position(|b| *b == b'\t').ok_or_else(bad)?;
        let head = std::str::from_utf8(&rec[..tab]).map_err(|_| bad())?;
        let mode: u32 = head
            .split_ascii_whitespace()
            .next()
            .and_then(|m| u32::from_str_radix(m, 8).ok())
            .ok_or_else(bad)?;
        out.push((String::from_utf8_lossy(&rec[tab + 1..]).into_owned(), mode));
    }
    Ok(out)
}

/// A new folder only its owner can enter.
fn make_private_dir(path: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}

/// The host and the path of a URL of the three accepted forms; empty strings for anything else.
fn split_url(url: &str) -> (&str, &str) {
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("ssh://"))
    {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or("");
        // `host:port`, but keep an IPv6 literal whole.
        let host = if host.starts_with('[') {
            host
        } else {
            host.split(':').next().unwrap_or("")
        };
        (host, path)
    } else if let Some(rest) = url.strip_prefix("git@") {
        rest.split_once(':').unwrap_or(("", ""))
    } else {
        ("", "")
    }
}

/// `s` cut to `[A-Za-z0-9._-]`, without leading dots, at most `NAME_PART` characters.
fn clean_part(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out = out.trim_start_matches('.').to_string();
    out.truncate(NAME_PART);
    out
}

/// The last non-empty line of git's stderr, safe to print.
fn last_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no error text");
    line.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(STDERR_CAP)
        .collect()
}

/// `s` quoted and cut for an error message.
fn shown(s: &str) -> String {
    let cut: String = s.chars().take(80).collect();
    let more = if cut.len() < s.len() { "..." } else { "" };
    format!("{cut:?}{more}")
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::process::Stdio;

    use super::*;
    use crate::tools::fake::{Call, Fake};

    /// What every git call starts with, in order.
    const PREFIX: [&str; 22] = [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.symlinks=false",
        "-c",
        "protocol.file.allow=never",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "transfer.fsckObjects=true",
        "-c",
        "fetch.fsckObjects=true",
        "-c",
        "submodule.recurse=false",
        "-c",
        "filter.lfs.smudge=",
        "-c",
        "filter.lfs.process=",
        "-c",
        "filter.lfs.required=false",
    ];

    fn with_prefix(rest: &[&str]) -> Vec<String> {
        PREFIX
            .iter()
            .chain(rest.iter())
            .map(|s| s.to_string())
            .collect()
    }

    fn words(step: &[OsString]) -> Vec<String> {
        step.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn pairs(env: &[(OsString, OsString)]) -> Vec<(String, String)> {
        env.iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    type Reply = io::Result<(i32, Vec<u8>, Vec<u8>)>;

    fn out(code: i32, stdout: &str, stderr: &str) -> Reply {
        Ok((code, stdout.as_bytes().to_vec(), stderr.as_bytes().to_vec()))
    }

    /// What `git init` leaves: the repo's own `.git/config`, which `fetch` then appends to.
    fn fake_init(c: &Call) {
        let dest = PathBuf::from(c.args.last().expect("init names the folder"));
        std::fs::create_dir_all(dest.join(".git")).unwrap();
        std::fs::write(dest.join(".git").join("config"), "[core]\n\tbare = false\n").unwrap();
    }

    /// A git that succeeds at every step.
    fn git_ok(c: &Call) -> Reply {
        git_version(c, "git version 2.43.0\n")
    }

    /// `git_ok`, with `version` as what `git --version` prints.
    fn git_version(c: &Call, version: &str) -> Reply {
        let w = c.words();
        let has = |s: &str| w.iter().any(|a| a == s);
        if has("init") {
            fake_init(c);
            out(0, "", "")
        } else if has("--version") {
            out(0, version, "")
        } else if has("ls-files") {
            out(
                0,
                "100644 aaaa 0\tREADME.md\x00120000 bbbb 0\tlink\x00160000 cccc 0\tvendor/sub\x00\
                 100755 dddd 0\tdir with space/run.sh\x00",
                "",
            )
        } else {
            out(0, "", "")
        }
    }

    fn plan(dest: &Path) -> FetchPlan {
        FetchPlan {
            url: "https://example.invalid/o/r.git".into(),
            rev: None,
            dest: dest.to_path_buf(),
        }
    }

    fn entries(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    #[test]
    fn git_steps_are_exact() {
        let mut p = plan(Path::new("/q/dest"));
        p.rev = Some("v1.2".into());
        let got: Vec<Vec<String>> = git_steps(&p, Path::new("/q/tpl"))
            .iter()
            .map(|s| words(s))
            .collect();
        assert_eq!(
            got,
            vec![
                with_prefix(&["--version"]),
                with_prefix(&["init", "-q", "--template=/q/tpl", "/q/dest"]),
                with_prefix(&[
                    "-C",
                    "/q/dest",
                    "remote",
                    "add",
                    "origin",
                    "https://example.invalid/o/r.git"
                ]),
                with_prefix(&[
                    "-C",
                    "/q/dest",
                    "fetch",
                    "-q",
                    "--depth",
                    "1",
                    "--no-tags",
                    "--recurse-submodules=no",
                    "origin",
                    "v1.2"
                ]),
                with_prefix(&[
                    "-C",
                    "/q/dest",
                    "checkout",
                    "-q",
                    "--no-recurse-submodules",
                    "-B",
                    "check-repo",
                    "FETCH_HEAD"
                ]),
                with_prefix(&["-C", "/q/dest", "ls-files", "-s", "-z"]),
            ]
        );
    }

    #[test]
    fn git_steps_fetch_head_without_a_rev() {
        let steps = git_steps(&plan(Path::new("/q/dest")), Path::new("/q/tpl"));
        assert_eq!(words(&steps[3]).last().map(String::as_str), Some("HEAD"));
    }

    #[test]
    fn git_env_drops_user_config() {
        let env = git_env(
            OsStr::new("/usr/bin:/bin"),
            Path::new("/home/u"),
            Some(OsStr::new("/run/agent.sock")),
        );
        let want = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            pairs(&env),
            want(&[
                ("PATH", "/usr/bin:/bin"),
                ("HOME", "/home/u"),
                ("SSH_AUTH_SOCK", "/run/agent.sock"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_TERMINAL_PROMPT", "0"),
                ("GIT_LFS_SKIP_SMUDGE", "1"),
                ("GIT_ALLOW_PROTOCOL", "https:ssh"),
                ("LC_ALL", "C"),
            ])
        );
        let bare = git_env(OsStr::new("/usr/bin"), Path::new("/home/u"), None);
        assert!(
            pairs(&bare).iter().all(|(k, _)| k != "SSH_AUTH_SOCK"),
            "{bare:?}"
        );
        assert_eq!(bare.len(), 8);
    }

    #[test]
    fn rejects_option_like_urls_and_revs() {
        for ok in [
            "https://github.com/o/r",
            "https://github.com/o/r.git",
            "ssh://git@github.com/o/r.git",
            "ssh://git@host.example:2222/o/r",
            "git@github.com:o/r.git",
            "https://[::1]/o/r",
        ] {
            assert_eq!(validate_url(ok), Ok(ok.to_string()), "{ok}");
        }
        for bad in [
            "",
            "-oProxyCommand=x",
            "--upload-pack=x",
            "https://host/o r",
            "https://host/o\tr",
            "https://host/o\nr",
            "https://host/o\u{7}r",
            "https://host/o\u{85}r",
            "https://host/o\u{a0}r",
            "file:///etc/passwd",
            "file://localhost/x",
            "ext::sh%20-c%20x",
            "ext::sh -c x",
            "http://host/o/r",
            "git://host/o/r",
            "ftp://host/o/r",
            "HTTPS://host/o/r",
            "/tmp/repo",
            "./repo",
            "../repo",
            "repo",
            "https://",
            "ssh://",
            "https:///o/r",
            "ssh://-oProxyCommand=x/o/r",
            "ssh://git@-oProxyCommand=x/o/r",
            "git@:o/r",
            "git@host:",
            "git@host",
            "git@ho st:o/r",
            "git@host_x:o/r",
        ] {
            assert!(validate_url(bad).is_err(), "{bad:?} must be refused");
        }
        let long = format!("https://host/{}", "a".repeat(3000));
        assert!(validate_url(&long).is_err());

        let longest = "a".repeat(200);
        let too_long = "a".repeat(201);
        for ok in [
            "main",
            "v1.2.3",
            "feature/x-y",
            "refs/tags/v1",
            "0123456",
            "0123456789abcdef0123456789abcdef01234567",
            "HEAD",
            longest.as_str(),
        ] {
            assert_eq!(validate_rev(ok), Ok(ok.to_string()), "{ok}");
        }
        for bad in [
            "",
            "-",
            "-x",
            "--upload-pack=x",
            "a b",
            "main;ls",
            "ma$in",
            "main\n",
            "feat:x",
            "HEAD~1",
            "@{u}",
            "a\\b",
            too_long.as_str(),
        ] {
            assert!(validate_rev(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn dest_name_is_host_owner_repo_epoch() {
        assert_eq!(
            dest_name("https://github.com/rbonweb/anthology.git", 1790000000),
            "github.com-rbonweb-anthology-1790000000"
        );
        assert_eq!(dest_name("git@github.com:o/r.git", 7), "github.com-o-r-7");
        assert_eq!(
            dest_name("ssh://git@host.example:2222/grp/sub/repo/", 7),
            "host.example-grp-sub-repo-7"
        );
    }

    #[test]
    fn dest_name_is_one_safe_folder_name() {
        let huge = format!(
            "https://{}/{}/{}",
            "h".repeat(300),
            "o".repeat(300),
            "r".repeat(300)
        );
        for url in [
            "https://host/../../etc",
            "https://host/o/..",
            "https://host/a b/c\u{202e}d",
            "https://host/",
            "https://host",
            "git@host:../../x",
            huge.as_str(),
        ] {
            let name = dest_name(url, 42);
            assert!(name.ends_with("-42"), "{url}: {name}");
            assert!(!name.starts_with('.'), "{url}: {name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                "{url}: {name}"
            );
            assert!(name.len() <= 3 * 64 + 3 + 20, "{url}: {name}");
        }
    }

    #[test]
    fn git_version_text_forms() {
        assert_eq!(parse_version("git version 2.43.0\n"), Some((2, 43, 0)));
        assert_eq!(
            parse_version("git version 2.39.5 (Apple Git-154)\n"),
            Some((2, 39, 5))
        );
        assert_eq!(
            parse_version("git version 2.45.1.windows.1"),
            Some((2, 45, 1))
        );
        assert_eq!(parse_version("git version 10.0.1"), Some((10, 0, 1)));
        // No patch level reads as 0.
        assert_eq!(parse_version("git version 2.39"), Some((2, 39, 0)));
        assert_eq!(parse_version("not git"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn child_path_keeps_only_absolute_entries() {
        let raw = OsString::from(":.:rel/bin:/usr/bin:../x:/bin:");
        assert_eq!(child_path(Some(raw)), OsString::from("/usr/bin:/bin"));
        // Nothing absolute left, or no PATH at all: the plain system folders.
        assert_eq!(
            child_path(Some(OsString::from(".:bin"))),
            OsString::from("/usr/bin:/bin")
        );
        assert_eq!(child_path(None), OsString::from("/usr/bin:/bin"));
        assert_eq!(
            absolute_path_entries(OsStr::new("a:/b:c")),
            OsString::from("/b")
        );
        assert_eq!(absolute_path_entries(OsStr::new("")), OsString::new());
    }

    #[test]
    fn template_path_is_a_hidden_sibling() {
        assert_eq!(
            template_path(Path::new("/q/dest")),
            Some(PathBuf::from("/q/.dest.template"))
        );
        assert_eq!(template_path(Path::new("/")), None);
    }

    #[test]
    fn the_quarantine_config_is_hardened_before_git_fetches_or_checks_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let r = Fake::new(&["git"], move |c| {
            let w = c.words();
            if w.iter().any(|a| a == "fetch" || a == "checkout") {
                let at = w.iter().position(|a| a == "-C").unwrap();
                let cfg = std::fs::read_to_string(Path::new(&w[at + 1]).join(".git/config"));
                log.lock().unwrap().push(cfg.unwrap());
            }
            git_ok(c)
        });
        fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "fetch and checkout both ran");
        for cfg in seen.iter() {
            // git init's own lines stay; the three protections are added to [core].
            assert!(cfg.starts_with("[core]\n\tbare = false\n"), "{cfg}");
            for want in [
                "symlinks = false",
                "hooksPath = /dev/null",
                "fsmonitor = false",
            ] {
                assert!(cfg.contains(want), "{want} missing in {cfg}");
            }
        }
    }

    #[test]
    fn an_init_that_leaves_no_config_is_an_error_and_removes_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], |c| {
            if c.words().iter().any(|a| a == "init") {
                out(0, "", "")
            } else {
                git_ok(c)
            }
        });
        let err = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap_err();
        assert!(err.contains(".git"), "{err}");
        assert!(!dest.exists());
        assert_eq!(entries(tmp.path()), 0);
        assert_eq!(r.calls().len(), 2, "nothing runs after the failed hardening");
    }

    #[test]
    fn fetch_returns_the_git_modes_and_runs_every_step_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("q").join("dest");
        let r = Fake::new(&["git"], git_ok);
        let modes = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap();
        // Git prints the modes in octal: 0o120000 is a symlink, 0o160000 a submodule.
        assert_eq!(
            modes,
            vec![
                ("README.md".to_string(), 0o100644),
                ("link".to_string(), 0o120000),
                ("vendor/sub".to_string(), 0o160000),
                ("dir with space/run.sh".to_string(), 0o100755),
            ]
        );
        let calls = r.calls();
        assert_eq!(calls.len(), 6);
        let abs = std::path::absolute(&dest).unwrap();
        let abs = abs.to_str().unwrap();
        assert_eq!(
            calls[2].words(),
            with_prefix(&[
                "-C",
                abs,
                "remote",
                "add",
                "origin",
                "https://example.invalid/o/r.git"
            ])
        );
        assert_eq!(
            calls[4].words(),
            with_prefix(&[
                "-C",
                abs,
                "checkout",
                "-q",
                "--no-recurse-submodules",
                "-B",
                "check-repo",
                "FETCH_HEAD"
            ])
        );
        for c in &calls {
            assert_eq!(c.prog, Path::new("/fake/bin/git"));
            let env = pairs(&c.env);
            for want in [
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_TERMINAL_PROMPT", "0"),
                ("GIT_ALLOW_PROTOCOL", "https:ssh"),
            ] {
                assert!(
                    env.contains(&(want.0.to_string(), want.1.to_string())),
                    "{want:?} missing in {env:?}"
                );
            }
        }
        // The empty template folder is gone; only the quarantine is left.
        assert_eq!(entries(dest.parent().unwrap()), 1);
        assert!(dest.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn failed_fetch_removes_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], |c| {
            if c.words().iter().any(|a| a == "fetch") {
                out(
                    128,
                    "",
                    "fatal: unable to access 'https://example.invalid/o/r.git/': Could not resolve host: example.invalid\n",
                )
            } else {
                git_ok(c)
            }
        });
        let err = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap_err();
        assert!(err.contains("fetch"), "{err}");
        assert!(err.contains("Could not resolve host"), "{err}");
        assert!(!dest.exists());
        assert_eq!(entries(tmp.path()), 0, "no template folder is left either");
        // The run stopped at the failed step.
        assert_eq!(r.calls().len(), 4);
    }

    #[test]
    fn a_stuck_git_is_a_timeout_and_removes_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], |c| {
            if c.words().iter().any(|a| a == "fetch") {
                Err(io::Error::new(io::ErrorKind::TimedOut, "killed"))
            } else {
                git_ok(c)
            }
        });
        let err = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(!dest.exists());
        assert_eq!(entries(tmp.path()), 0);
    }

    #[test]
    fn one_deadline_spans_all_steps() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], |c| {
            std::thread::sleep(Duration::from_millis(60));
            git_ok(c)
        });
        let err = fetch(&plan(&dest), &r, Duration::from_millis(150)).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(!dest.exists());
        let calls = r.calls();
        assert!(calls.len() < 6, "{} calls", calls.len());
        assert!(calls[0].timeout <= Duration::from_millis(150));
        for pair in calls.windows(2) {
            assert!(pair[1].timeout < pair[0].timeout, "the budget shrinks");
        }
    }

    /// What `fetch_with_notes` says for `git --version` printing `version`.
    fn notes_for(version: &'static str) -> Vec<String> {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], move |c| git_version(c, version));
        let done = fetch_with_notes(&plan(&dest), &r, Duration::from_secs(120))
            .unwrap_or_else(|e| panic!("{version:?} must not be refused: {e}"));
        assert_eq!(r.calls().len(), 6, "every step ran for {version:?}");
        assert!(dest.is_dir());
        done.notes
    }

    #[test]
    fn old_git_is_noted_and_never_refused() {
        // Below 2.39.1: the two CVEs; below 2.32 also no GIT_CONFIG_GLOBAL.
        let old = notes_for("git version 2.39.0\n");
        assert_eq!(old.len(), 1, "{old:?}");
        assert!(old[0].contains("2.39.0"), "{old:?}");
        assert!(old[0].contains("2.39.1"), "{old:?}");
        assert!(old[0].contains("CVE-2022-23521"), "{old:?}");
        assert!(!old[0].contains("GIT_CONFIG_GLOBAL"), "{old:?}");
        let older = notes_for("git version 2.31.1\n");
        assert_eq!(older.len(), 1, "{older:?}");
        assert!(older[0].contains("2.31.1"), "{older:?}");
        assert!(older[0].contains("GIT_CONFIG_GLOBAL"), "{older:?}");
        let ancient = notes_for("git version 1.8.3.1\n");
        assert!(ancient[0].contains("GIT_CONFIG_GLOBAL"), "{ancient:?}");
        // Distribution builds name their own suffix; the number still counts.
        assert_eq!(notes_for("git version 2.38.5 (Apple Git-153)\n").len(), 1);
    }

    #[test]
    fn a_current_git_has_no_note() {
        for v in [
            "git version 2.39.1\n",
            "git version 2.39.5 (Apple Git-154)\n",
            "git version 2.43.0\n",
            "git version 3.0.0\n",
        ] {
            assert!(notes_for(v).is_empty(), "{v}");
        }
    }

    #[test]
    fn an_unreadable_git_version_is_noted_not_refused() {
        let notes = notes_for("something else entirely\n");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("2.39.1"), "{notes:?}");
        // The text is quoted and cut, never pasted raw.
        let notes = notes_for("git version \u{202e}9\n");
        assert!(!notes[0].contains('\u{202e}'), "{notes:?}");
    }

    #[test]
    fn fetch_drops_the_notes_and_keeps_the_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], |c| git_version(c, "git version 2.30.0\n"));
        let modes = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap();
        assert_eq!(modes.len(), 4);
    }

    #[test]
    fn a_missing_git_is_an_error_that_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&[], |_| unreachable!("git is not found"));
        let err = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap_err();
        assert!(err.contains("git"), "{err}");
        assert_eq!(entries(tmp.path()), 0);
    }

    #[test]
    fn an_existing_dest_is_refused_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("keep.txt"), "mine").unwrap();
        let r = Fake::new(&["git"], git_ok);
        let err = fetch(&plan(&dest), &r, Duration::from_secs(120)).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dest.join("keep.txt")).unwrap(),
            "mine"
        );
        assert!(r.calls().is_empty());
    }

    #[test]
    fn a_hostile_url_or_rev_never_reaches_git() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let r = Fake::new(&["git"], git_ok);
        let mut p = plan(&dest);
        p.url = "--upload-pack=touch x".into();
        assert!(fetch(&p, &r, Duration::from_secs(120)).is_err());
        let mut p = plan(&dest);
        p.rev = Some("--upload-pack=x".into());
        assert!(fetch(&p, &r, Duration::from_secs(120)).is_err());
        assert!(r.calls().is_empty());
        assert_eq!(entries(tmp.path()), 0);
    }

    /// Starts git for real, with the environment `fetch` gives it and nothing else.
    struct RealGit;

    impl Runner for RealGit {
        fn find(&self, name: &str) -> Option<PathBuf> {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|d| d.join(name))
                .find(|c| c.is_file())
        }

        fn run(
            &self,
            prog: &Path,
            args: &[OsString],
            cwd: &Path,
            env: &[(OsString, OsString)],
            _timeout: Duration,
        ) -> Reply {
            let o = std::process::Command::new(prog)
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null())
                .output()?;
            Ok((o.status.code().unwrap_or(-1), o.stdout, o.stderr))
        }
    }

    #[test]
    fn real_git_takes_the_argv_and_a_failed_fetch_leaves_nothing() {
        if RealGit.find("git").is_none() {
            eprintln!("skipped: git is not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest");
        let err = fetch(&plan(&dest), &RealGit, Duration::from_secs(60)).unwrap_err();
        // Past the version check, init and remote add, git reached the network and failed
        // there; a bad flag would end earlier with git's usage text.
        assert!(err.contains("fetch"), "{err}");
        for usage in ["usage:", "unknown option", "unrecognized"] {
            assert!(!err.contains(usage), "{err}");
        }
        assert!(!dest.exists());
        assert_eq!(entries(tmp.path()), 0);
    }
}
