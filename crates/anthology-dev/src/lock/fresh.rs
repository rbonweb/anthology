//! `lock check-fresh`: has the shipped set changed since the lock's tag (design §8.5, I-6)?
//!
//! The shipped graph is computed once, at `HEAD`, with
//! `cargo tree --locked -p anthology-cli -e normal,build --prefix none` for the four shipped
//! targets. Its workspace packages are the entries whose source is a path inside the
//! repository. The shipped set, compared between the tag's commit and `HEAD` through git
//! objects, is:
//!
//! 1. each workspace package's `src/` tree, `Cargo.toml` and build script (`build.rs` or its
//!    `build` path). `tests/`, `benches/` and `examples/` are not in it. A `[lib]` or `[[bin]]`
//!    root outside `src/` is an error, since its modules would sit beside it;
//! 2. every file those sources pull in from outside `src/`: `include_str!`, `include_bytes!`,
//!    `include!` and `#[path = "…"]` modules, followed transitively through the Rust ones. An
//!    include whose argument is neither a string literal nor `concat!(env!("OUT_DIR"), …)`,
//!    a non-literal `#[path]` and a `cfg_attr(…, path = …)` are errors;
//! 3. the files a build script declares with `cargo:rerun-if-changed=`; a build script that
//!    reads files without declaring any is an error;
//! 4. in `Cargo.lock`, the `[[package]]` blocks of every package name in the graph;
//! 5. `rust-toolchain.toml`, `rust-toolchain`, `.cargo/config.toml` and `.cargo/config`;
//! 6. the root `Cargo.toml`, all of it except what cannot change the binary
//!    ([`root_manifest_items`]): `[workspace]` `members`/`exclude`/`default-members`/`lints`/
//!    `metadata`, profiles other than `release`, and the `[workspace.dependencies]` entries of
//!    packages outside the graph. Comments, blank lines, key order and dotted-key spelling are
//!    ignored.
//!
//! Known limit: with explicit `--target`s, cargo evaluates the host-only edges (build
//! dependencies and proc-macros) for the machine that runs check-fresh (Linux in
//! `scripts/dev.sh`), so a build dependency only a macOS host would use is not named.
//!
//! The old commit is never resolved or built: a change to the graph itself shows up as a change
//! to a shipped `Cargo.toml` or to a `Cargo.lock` block of a package that is in the graph at
//! `HEAD`. Uncommitted changes under a shipped path are an error, since `HEAD` is what is
//! compared.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The four shipped targets, as `cargo tree --target` takes them.
pub const SHIPPED_TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

/// The package whose graph ships.
pub const ROOT_PACKAGE: &str = "anthology-cli";

/// The result of [`compare`].
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing in the shipped set changed; `commit` is the tag's short commit.
    Fresh { commit: String },
    /// `changed` names every changed item, sorted.
    Stale {
        commit: String,
        changed: Vec<String>,
    },
}

// ------------------------------------------------------------------------------------------
// git and cargo

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotepath=false"])
        .args(args)
        // Only reads: no index refresh, so a read-only git directory works (scripts/dev.sh
        // mounts a linked worktree's common git directory read-only).
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(root, args)?;
    String::from_utf8(out).map_err(|_| format!("git {}: output is not UTF-8", args.join(" ")))
}

/// The top level of the git work tree that holds `cwd`, canonicalized.
pub fn toplevel(cwd: &Path) -> Result<PathBuf, String> {
    let out = git_text(cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("not in a git work tree ({e})"))?;
    canonical(Path::new(out.trim_end_matches('\n')))
}

fn canonical(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The text of `path` at `rev`, or `None` when it does not exist there.
fn show(root: &Path, rev: &str, path: &str) -> Option<String> {
    let spec = format!("{rev}:{path}");
    git(root, &["cat-file", "-e", &spec]).ok()?;
    git(root, &["show", &spec])
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// A package in `cargo tree` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphPkg {
    pub name: String,
    pub version: String,
    /// The source in parentheses (a path for path packages), if any.
    pub source: Option<String>,
}

/// One line of `cargo tree --prefix none`: `name vX.Y.Z[ (source)][ (proc-macro)][ (*)]`.
pub fn parse_tree_line(line: &str) -> Option<GraphPkg> {
    let line = line.trim_end();
    let (name, rest) = line.split_once(' ')?;
    let (version, mut rest) = match rest.split_once(' ') {
        Some((v, r)) => (v, r),
        None => (rest, ""),
    };
    let version = version.strip_prefix('v')?;
    if name.is_empty() || version.is_empty() {
        return None;
    }
    loop {
        if let Some(r) = rest
            .strip_suffix(" (*)")
            .or_else(|| rest.strip_suffix("(*)"))
        {
            rest = r.trim_end();
        } else if let Some(r) = rest
            .strip_suffix(" (proc-macro)")
            .or_else(|| rest.strip_suffix("(proc-macro)"))
        {
            rest = r.trim_end();
        } else {
            break;
        }
    }
    let source = if rest.is_empty() {
        None
    } else {
        let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.to_string())
    };
    Some(GraphPkg {
        name: name.to_string(),
        version: version.to_string(),
        source,
    })
}

/// The shipped graph at the work tree `root` (module docs).
pub fn shipped_graph(root: &Path, cargo: &OsStr) -> Result<Vec<GraphPkg>, String> {
    let manifest = root.join("Cargo.toml");
    let mut cmd = Command::new(cargo);
    cmd.arg("tree")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", ROOT_PACKAGE, "-e", "normal,build", "--prefix", "none"]);
    for t in SHIPPED_TARGETS {
        cmd.args(["--target", t]);
    }
    let out = cmd
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run cargo tree: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pkgs: Vec<GraphPkg> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let pkg =
            parse_tree_line(line).ok_or_else(|| format!("cargo tree: cannot read {line:?}"))?;
        if !pkgs.contains(&pkg) {
            pkgs.push(pkg);
        }
    }
    if !pkgs.iter().any(|p| p.name == ROOT_PACKAGE) {
        return Err(format!("cargo tree printed no {ROOT_PACKAGE}"));
    }
    Ok(pkgs)
}

// ------------------------------------------------------------------------------------------
// The shipped set at HEAD

/// The shipped set's paths and package names (module docs).
#[derive(Debug, Default)]
pub struct ShippedSet {
    /// Repository-relative files and folders compared as files.
    pub paths: BTreeSet<String>,
    /// Every package name in the graph.
    pub names: BTreeSet<String>,
}

/// `p` relative to `root`, `/`-separated; an error when it is outside.
fn relative(root: &Path, p: &Path) -> Result<String, String> {
    let rel = p
        .strip_prefix(root)
        .map_err(|_| format!("{} is outside the repository", p.display()))?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Ok(parts.join("/"))
}

/// Resolves `.` and `..` in a `/`-separated relative path; `None` when it climbs above the root.
pub fn normalize(rel: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    Some(out.join("/"))
}

impl ShippedSet {
    /// The set for `graph`, reading the work tree `root` (which equals `HEAD` on these paths).
    pub fn at_head(root: &Path, graph: &[GraphPkg]) -> Result<ShippedSet, String> {
        let mut set = ShippedSet::default();
        for p in [
            "rust-toolchain.toml",
            "rust-toolchain",
            ".cargo/config.toml",
            ".cargo/config",
        ] {
            set.paths.insert(p.to_string());
        }
        for pkg in graph {
            set.names.insert(pkg.name.clone());
            let Some(source) = &pkg.source else { continue };
            if !source.starts_with('/') {
                continue; // a registry or git source: compared through Cargo.lock
            }
            let dir = canonical(Path::new(source))?;
            let rel = relative(root, &dir).map_err(|e| {
                format!(
                    "{} is a path package outside the repository ({e})",
                    pkg.name
                )
            })?;
            set.add_package(root, &rel)?;
        }
        Ok(set)
    }

    fn add_package(&mut self, root: &Path, pkg: &str) -> Result<(), String> {
        let join = |p: &str| {
            if pkg.is_empty() {
                p.to_string()
            } else {
                format!("{pkg}/{p}")
            }
        };
        let src = join("src");
        let in_src = |p: &str| p.starts_with(&format!("{src}/"));
        self.paths.insert(src.clone());
        self.paths.insert(join("Cargo.toml"));
        let manifest = std::fs::read_to_string(root.join(join("Cargo.toml")))
            .map_err(|e| format!("{}: {e}", join("Cargo.toml")))?;

        // The Rust files to scan, and the build scripts among them.
        let mut todo: Vec<String> = Vec::new();
        let mut build_scripts: Vec<String> = Vec::new();
        let mut has_build_key = false;
        for (kind, target) in manifest_targets(&manifest) {
            let rel = normalize(&join(&target))
                .ok_or_else(|| format!("{pkg}/Cargo.toml: {target} is outside the repository"))?;
            match kind {
                TargetKind::Build => {
                    has_build_key = true;
                    self.paths.insert(rel.clone());
                    build_scripts.push(rel.clone());
                }
                TargetKind::Crate if !in_src(&rel) => {
                    // Its `mod` files would be found next to it, outside the compared tree.
                    return Err(format!(
                        "{pkg}/Cargo.toml: the crate root {target} is outside src/; check-fresh \
                         cannot follow its modules"
                    ));
                }
                TargetKind::Crate => {}
            }
            todo.push(rel);
        }
        if !has_build_key {
            let rel = join("build.rs");
            self.paths.insert(rel.clone());
            build_scripts.push(rel.clone());
            todo.push(rel);
        }
        let mut files = Vec::new();
        collect_rs(&root.join(&src), &mut files)?;
        for f in files {
            todo.push(relative(root, &f)?);
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(file) = todo.pop() {
            if !seen.insert(file.clone()) {
                continue;
            }
            let text = match std::fs::read_to_string(root.join(&file)) {
                Ok(t) => t,
                // Only a default build.rs may be absent.
                Err(_) if build_scripts.contains(&file) && !has_build_key => continue,
                Err(e) => return Err(format!("{file}: {e}")),
            };
            let dir = file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            for r in references(&text).map_err(|e| format!("{file}: {e}"))? {
                let target = if r.path.starts_with('/') {
                    relative(root, Path::new(&r.path))?
                } else {
                    normalize(&format!("{dir}/{}", r.path))
                        .ok_or_else(|| format!("{file}: {:?} is outside the repository", r.path))?
                };
                if !in_src(&target) {
                    self.paths.insert(target.clone());
                }
                if r.rust {
                    if !root.join(&target).is_file() {
                        return Err(format!(
                            "{file}: {:?} does not name a file check-fresh can follow",
                            r.path
                        ));
                    }
                    todo.push(target);
                }
            }
            if build_scripts.contains(&file) {
                let inputs = build_inputs(&text).map_err(|e| format!("{file}: {e}"))?;
                for input in inputs {
                    let target = normalize(&join(&input)).ok_or_else(|| {
                        format!("{file}: rerun-if-changed {input:?} is outside the repository")
                    })?;
                    self.paths.insert(target);
                }
            }
        }
        Ok(())
    }
}

/// What a manifest target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// `[package] build`.
    Build,
    /// `[lib] path` or a `[[bin]] path`.
    Crate,
}

/// Every `.rs` file under `dir` (none when it does not exist).
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            collect_rs(&path, out)?;
        } else if path.extension() == Some(OsStr::new("rs")) {
            out.push(path);
        }
    }
    Ok(())
}

/// The `build` path of `[package]` and the `path` of `[lib]` and every `[[bin]]`, as written.
pub fn manifest_targets(manifest: &str) -> Vec<(TargetKind, String)> {
    let mut out = Vec::new();
    for table in parse_tables(manifest) {
        let (key, kind) = match table.header.as_str() {
            "package" => ("build", TargetKind::Build),
            "lib" | "bin" => ("path", TargetKind::Crate),
            _ => continue,
        };
        for e in &table.entries {
            if e.key == key
                && let Some(v) = string_value(&e.text)
            {
                out.push((kind, v));
            }
        }
    }
    out
}

/// The basic or literal string after `=` in an entry, if that is all the value is.
fn string_value(entry: &str) -> Option<String> {
    let value = entry.split_once('=')?.1.trim();
    let inner = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
    (!inner.contains(['"', '\'', '\\'])).then(|| inner.to_string())
}

/// A file a Rust source pulls in: `rust` when it is compiled as Rust (`include!`, `#[path]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub path: String,
    pub rust: bool,
}

/// The paths of `include_str!`, `include_bytes!` and `include!` and of `#[path]` modules in
/// Rust source `text` ([`references`], paths only).
pub fn includes(text: &str) -> Result<Vec<String>, String> {
    Ok(references(text)?.into_iter().map(|r| r.path).collect())
}

/// The string literal at the start of `arg` (`"…"`, `r"…"`, `r#"…"#`), if it is one.
fn literal_at(arg: &str, n: usize) -> Result<Option<String>, String> {
    let (body, close, plain) = if let Some(b) = arg.strip_prefix('"') {
        (b, "\"", true)
    } else if let Some(b) = arg.strip_prefix("r\"") {
        (b, "\"", false)
    } else if let Some(b) = arg.strip_prefix("r#\"") {
        (b, "\"#", false)
    } else {
        return Ok(None);
    };
    let end = body
        .find(close)
        .ok_or_else(|| format!("line {n}: unterminated path"))?;
    let path = &body[..end];
    if plain && path.contains('\\') {
        return Err(format!("line {n}: escaped path {path:?}"));
    }
    Ok(Some(path.to_string()))
}

/// Every file Rust source `text` pulls in: `include_str!`, `include_bytes!` and `include!`
/// arguments and `#[path = "…"]` modules, in that line order. A `concat!(env!("OUT_DIR"), …)`
/// include is skipped (the build script's output; its inputs are the build script's, see
/// [`build_inputs`]); any other non-literal include or `#[path]`, and any
/// `cfg_attr(…, path = …)`, is an error. Lines that start with `//` are comments.
pub fn references(text: &str) -> Result<Vec<Ref>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if line.trim_start().starts_with("//") {
            continue;
        }
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("cfg_attr(") && compact.contains("path=") {
            return Err(format!(
                "line {n}: a cfg_attr path attribute; the shipped set cannot follow it"
            ));
        }
        let mut search = compact.as_str();
        while let Some(at) = search.find("#[path=") {
            let arg = &search[at + "#[path=".len()..];
            match literal_at(arg, n)? {
                Some(path) => out.push(Ref { path, rust: true }),
                None => {
                    return Err(format!(
                        "line {n}: a #[path] whose value is not a string literal"
                    ));
                }
            }
            search = &search[at + 1..];
        }
        let mut rest = line;
        while let Some((macro_len, start)) = find_include(rest) {
            let rust = rest[start..].starts_with("include!");
            let after = rest[start + macro_len..].trim_start();
            let Some(arg) = after
                .strip_prefix('(')
                .or_else(|| after.strip_prefix('['))
                .or_else(|| after.strip_prefix('{'))
            else {
                return Err(format!("line {n}: cannot read the include"));
            };
            let arg = arg.trim_start();
            match literal_at(arg, n)? {
                Some(path) => out.push(Ref { path, rust }),
                None => {
                    let compact: String = arg.chars().filter(|c| !c.is_whitespace()).collect();
                    if !compact.starts_with("concat!(env!(\"OUT_DIR\")") {
                        return Err(format!(
                            "line {n}: an include whose path is not a string literal; the \
                             shipped set cannot follow it"
                        ));
                    }
                }
            }
            rest = &rest[start + macro_len..];
        }
    }
    Ok(out)
}

/// The files a build script declares with `cargo:rerun-if-changed=` (relative to the package),
/// except itself. A build script that reads files (`fs::`, `File::`, `read_to_string`,
/// `read_dir`) without declaring any is an error: check-fresh could not see its inputs.
pub fn build_inputs(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("rerun-if-changed=") {
        let tail = &rest[at + "rerun-if-changed=".len()..];
        let end = tail.find(['"', '\n']).unwrap_or(tail.len());
        let path = &tail[..end];
        if path.is_empty() || path.contains(['{', '}', '\\']) {
            return Err(format!(
                "rerun-if-changed={path:?} is not a literal path; the shipped set cannot follow it"
            ));
        }
        if path != "build.rs" {
            out.push(path.to_string());
        }
        rest = &tail[end..];
    }
    let reads = ["fs::", "File::", "read_to_string", "read_dir"]
        .iter()
        .any(|p| text.contains(p));
    if reads && out.is_empty() {
        return Err(
            "the build script reads files but declares none with rerun-if-changed; the shipped \
             set cannot follow them"
                .into(),
        );
    }
    Ok(out)
}

/// The next `include!`, `include_str!` or `include_bytes!` in `s` that is not part of a longer
/// identifier: `(length of the macro name with "!", byte offset)`.
fn find_include(s: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(i) = s[from..].find("include") {
        let at = from + i;
        let before_ok = s[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let tail = &s[at..];
        let len = ["include_bytes!", "include_str!", "include!"]
            .iter()
            .find(|m| tail.starts_with(**m))
            .map(|m| m.len());
        if before_ok && let Some(len) = len {
            return Some((len, at));
        }
        from = at + "include".len();
    }
    None
}

// ------------------------------------------------------------------------------------------
// Cargo.lock and Cargo.toml

/// The `[[package]]` blocks of a `Cargo.lock`, by package name; each name's blocks are sorted.
pub fn lock_blocks(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<Vec<&str>> = None;
    let flush = |block: Vec<&str>, out: &mut BTreeMap<String, Vec<String>>| {
        let name = block.iter().find_map(|l| {
            l.strip_prefix("name = \"")
                .and_then(|r| r.strip_suffix('"'))
                .map(str::to_string)
        });
        if let Some(name) = name {
            let mut lines = block;
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            out.entry(name).or_default().push(lines.join("\n"));
        }
    };
    for line in text.lines() {
        if line.starts_with('[') {
            if let Some(block) = current.take() {
                flush(block, &mut out);
            }
            if line.trim_end() == "[[package]]" {
                current = Some(Vec::new());
            }
        } else if let Some(block) = current.as_mut() {
            block.push(line.trim_end());
        }
    }
    if let Some(block) = current.take() {
        flush(block, &mut out);
    }
    for blocks in out.values_mut() {
        blocks.sort();
    }
    out
}

/// One `key = value` of a TOML table: `key` is the first segment of the (possibly dotted) key,
/// `text` the whole entry with comments removed and lines trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// Every segment of the key.
    pub path: Vec<String>,
    /// The value, comments removed, continuation lines joined with newlines.
    pub value: String,
    pub text: String,
}

/// A TOML table: `header` without brackets or spaces (`profile.release`); `array` for `[[…]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub header: String,
    pub array: bool,
    pub entries: Vec<Entry>,
}

/// Scans one line: `(bracket depth change, the line without its comment, multi-line string
/// state after it)`. `ml` is the open multi-line string delimiter, if any.
fn scan(line: &str, ml: &mut Option<&'static str>) -> (i32, String) {
    let mut depth = 0;
    let mut out = String::new();
    let mut chars = line.char_indices().peekable();
    let mut in_str: Option<char> = None;
    while let Some((i, c)) = chars.next() {
        if let Some(delim) = *ml {
            out.push(c);
            if line[i..].starts_with(delim) {
                out.push_str(&delim[1..]);
                for _ in 0..delim.len() - 1 {
                    chars.next();
                }
                *ml = None;
            } else if c == '\\'
                && delim == "\"\"\""
                && let Some((_, n)) = chars.next()
            {
                out.push(n);
            }
            continue;
        }
        if let Some(q) = in_str {
            out.push(c);
            if c == '\\' && q == '"' {
                if let Some((_, n)) = chars.next() {
                    out.push(n);
                }
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '#' => break,
            '"' | '\'' => {
                let triple = if c == '"' { "\"\"\"" } else { "'''" };
                if line[i..].starts_with(triple) {
                    out.push_str(triple);
                    chars.next();
                    chars.next();
                    *ml = Some(triple);
                } else {
                    out.push(c);
                    in_str = Some(c);
                }
            }
            '[' | '{' => {
                depth += 1;
                out.push(c);
            }
            ']' | '}' => {
                depth -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    (depth, out.trim().to_string())
}

/// The segments of a dotted TOML key or table header: `a . "b.c" . 'd'` → `a`, `b.c`, `d`.
pub fn dotted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '.' => out.push(std::mem::take(&mut cur)),
            None if c.is_whitespace() => {}
            None => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// `key = value` split at the first `=` outside quotes.
fn split_key(clean: &str) -> (&str, &str) {
    let mut quote: Option<char> = None;
    for (i, c) in clean.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '=' => return (&clean[..i], clean[i + 1..].trim()),
            None => {}
        }
    }
    (clean, "")
}

/// The tables of a TOML document, in order. Entries before the first header belong to a table
/// with an empty header. Comments and blank lines are dropped.
pub fn parse_tables(text: &str) -> Vec<Table> {
    let mut tables = vec![Table {
        header: String::new(),
        array: false,
        entries: Vec::new(),
    }];
    let mut depth = 0i32;
    let mut ml: Option<&'static str> = None;
    for line in text.lines() {
        let continuing = depth > 0 || ml.is_some();
        let (d, clean) = scan(line, &mut ml);
        if continuing {
            depth += d;
            if let Some(e) = tables.last_mut().and_then(|t| t.entries.last_mut())
                && !clean.is_empty()
            {
                e.text.push('\n');
                e.text.push_str(&clean);
                e.value.push('\n');
                e.value.push_str(&clean);
            }
            continue;
        }
        if clean.is_empty() {
            continue;
        }
        if clean.starts_with('[') {
            let array = clean.starts_with("[[");
            let inner = clean.trim_start_matches('[').trim_end_matches(']').trim();
            tables.push(Table {
                header: dotted(inner).join("."),
                array,
                entries: Vec::new(),
            });
            continue;
        }
        depth += d;
        let (key, value) = split_key(&clean);
        let path = dotted(key);
        let entry = Entry {
            key: path.first().cloned().unwrap_or_default(),
            path,
            value: value.to_string(),
            text: clean,
        };
        if let Some(t) = tables.last_mut() {
            t.entries.push(entry);
        }
    }
    tables
}

/// The `package = "…"` key of an inline dependency table, if any.
fn renamed_package(value: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = value[from..].find("package") {
        let at = from + i;
        from = at + "package".len();
        let before_ok = value[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c == '{' || c == ',' || c.is_whitespace());
        let Some(rest) = value[from..].trim_start().strip_prefix('=') else {
            continue;
        };
        if !before_ok {
            continue;
        }
        let rest = rest.trim_start().strip_prefix('"')?;
        return Some(rest[..rest.find('"')?].to_string());
    }
    None
}

/// The compared items of a root `Cargo.toml` (module docs, item 5): label → text. Everything is
/// compared except what cannot change the shipped binary: `[workspace]` `members`, `exclude`,
/// `default-members`, `lints` and `metadata`; profiles other than `release`; and the
/// `[workspace.dependencies]` entries of packages outside the graph. Each item's text is its
/// lines as `dotted.key = value`, sorted, so the table and dotted-key spellings and the order
/// of keys do not matter.
pub fn root_manifest_items(text: &str, names: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut items: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut renames: BTreeMap<String, String> = BTreeMap::new();
    for table in parse_tables(text) {
        let head: Vec<String> = if table.header.is_empty() {
            Vec::new()
        } else {
            dotted(&table.header)
        };
        for e in &table.entries {
            let mut path = head.clone();
            path.extend(e.path.iter().cloned());
            let line = format!("{} = {}", path.join("."), e.value);
            let seg = |i: usize| path.get(i).map(String::as_str);
            match (seg(0), seg(1)) {
                (
                    Some("workspace"),
                    Some("members" | "exclude" | "default-members" | "lints" | "metadata"),
                ) => continue,
                (Some("profile"), Some(p)) if p != "release" => continue,
                (Some("workspace"), Some("dependencies")) => {
                    let Some(dep) = seg(2) else { continue };
                    match &path[3..] {
                        [] => {
                            if let Some(p) = renamed_package(&e.value) {
                                renames.insert(dep.to_string(), p);
                            }
                        }
                        [k] if k == "package" => {
                            if let Some(p) = string_value(&format!("x = {}", e.value)) {
                                renames.insert(dep.to_string(), p);
                            }
                        }
                        _ => {}
                    }
                    deps.entry(dep.to_string()).or_default().push(line);
                    continue;
                }
                _ => {}
            }
            let label = match path.as_slice() {
                [w, p, key, ..] if w == "workspace" && p == "package" => {
                    format!("Cargo.toml [workspace.package] {key}")
                }
                [key] => format!("Cargo.toml (top level) {key}"),
                more => format!("Cargo.toml [{}]", more[..more.len() - 1].join(".")),
            };
            items.entry(label).or_default().push(line);
        }
    }
    for (dep, lines) in deps {
        let used = names.contains(&dep) || renames.get(&dep).is_some_and(|p| names.contains(p));
        if used {
            items
                .entry(format!("Cargo.toml [workspace.dependencies] {dep}"))
                .or_default()
                .extend(lines);
        }
    }
    items
        .into_iter()
        .map(|(label, mut lines)| {
            lines.sort();
            (label, lines.join("\n"))
        })
        .collect()
}

/// Items present on one side or with different texts.
fn changed_items<'a>(
    old: &'a BTreeMap<String, String>,
    new: &'a BTreeMap<String, String>,
) -> impl Iterator<Item = String> + 'a {
    old.keys()
        .chain(new.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| old.get(*k) != new.get(*k))
        .cloned()
}

// ------------------------------------------------------------------------------------------
// The comparison

fn literal(paths: &BTreeSet<String>) -> Vec<String> {
    paths.iter().map(|p| format!(":(literal){p}")).collect()
}

/// Compares the shipped set between the commit of `tag` and `HEAD` in the work tree `root`.
pub fn compare(root: &Path, tag: &str, cargo: &OsStr) -> Result<Verdict, String> {
    let tag_ref = format!("refs/tags/{tag}^{{commit}}");
    let tag_commit = git_text(root, &["rev-parse", "--verify", "--quiet", &tag_ref])
        .map_err(|_| {
            format!(
                "the lock's tag {tag} is not in this repository; fetch it (git fetch --tags) \
                 and run again"
            )
        })?
        .trim()
        .to_string();
    let head = git_text(root, &["rev-parse", "--verify", "HEAD^{commit}"])?
        .trim()
        .to_string();
    let short = git_text(root, &["rev-parse", "--short", &tag_commit])?
        .trim()
        .to_string();

    let graph = shipped_graph(root, cargo)?;
    let set = ShippedSet::at_head(root, &graph)?;

    let mut specs = set.paths.clone();
    specs.insert("Cargo.toml".into());
    specs.insert("Cargo.lock".into());
    let mut status_args = vec!["status", "--porcelain=v1", "--untracked-files=all", "--"];
    let spec_args = literal(&specs);
    status_args.extend(spec_args.iter().map(String::as_str));
    let dirty = git_text(root, &status_args)?;
    if !dirty.trim().is_empty() {
        return Err(format!(
            "uncommitted changes in the shipped set (HEAD is what is compared):\n{}",
            dirty.trim_end()
        ));
    }

    if tag_commit == head {
        return Ok(Verdict::Fresh { commit: short });
    }

    let mut changed: BTreeSet<String> = BTreeSet::new();
    let path_specs = literal(&set.paths);
    let mut diff_args = vec![
        "diff",
        "--name-only",
        "--no-renames",
        "--no-ext-diff",
        tag_commit.as_str(),
        head.as_str(),
        "--",
    ];
    diff_args.extend(path_specs.iter().map(String::as_str));
    for line in git_text(root, &diff_args)?.lines() {
        if !line.is_empty() {
            changed.insert(line.to_string());
        }
    }

    let old_lock = lock_blocks(&show(root, &tag_commit, "Cargo.lock").unwrap_or_default());
    let new_lock = lock_blocks(&show(root, &head, "Cargo.lock").unwrap_or_default());
    for name in &set.names {
        if old_lock.get(name) != new_lock.get(name) {
            changed.insert(format!("Cargo.lock: {name}"));
        }
    }

    let old_toml = root_manifest_items(
        &show(root, &tag_commit, "Cargo.toml").unwrap_or_default(),
        &set.names,
    );
    let new_toml = root_manifest_items(
        &show(root, &head, "Cargo.toml").unwrap_or_default(),
        &set.names,
    );
    changed.extend(changed_items(&old_toml, &new_toml));

    if changed.is_empty() {
        Ok(Verdict::Fresh { commit: short })
    } else {
        Ok(Verdict::Stale {
            commit: short,
            changed: changed.into_iter().collect(),
        })
    }
}
