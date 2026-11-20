//! Is a `ported` row's Rust test really compiled and run? (design §13.3: "a missing or `#[ignore]`
//! Rust target").
//!
//! Two layers:
//! - **Binding:** the `cargo test --workspace --locked -- --list` output, plain and with
//!   `--all-features` (`ci/jobs/ledger.sh` captures both into the fresh directory). A target must
//!   appear in one of them. This is what cargo compiles: a `cfg` that is false, a file no `mod`
//!   reaches, an `examples/` file or a `#[test]` inside a fn body is simply absent.
//! - **Static** (no build, the same on every OS): the test exists in its file, is not
//!   `#[ignore]`d and is behind no `cfg` other than `test`/`feature`, and the file is reached by
//!   `mod` declarations from a test target root, none of them gated. This catches what one Linux
//!   listing cannot see, such as a `#[cfg(target_os = "macos")]` that hides a test on macOS only.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::rust_scan::{self, Scan};

/// The listing files in the fresh-extraction directory: `-- --list` and `-- --list --ignored`
/// (which lists only the ignored tests), each plain and with `--all-features`.
pub const LIST_PLAIN: &str = "rust-tests-plain.txt";
pub const LIST_ALL: &str = "rust-tests-all.txt";
pub const LIST_IGNORED_PLAIN: &str = "rust-tests-ignored-plain.txt";
pub const LIST_IGNORED_ALL: &str = "rust-tests-ignored-all.txt";

/// What cargo compiles (`listed`) and what of that it would skip as ignored (`ignored`).
#[derive(Debug, Default)]
pub struct Listings {
    pub listed: Listing,
    pub ignored: Listing,
}

/// `cargo test -- --list` entries: (package, with `-` as `_`, or `None` when the output does not
/// say; the target root relative to the package, such as `tests/ledger.rs`; the test path).
#[derive(Debug, Default)]
pub struct Listing {
    entries: BTreeSet<(Option<String>, String, String)>,
}

fn normalize(package: &str) -> String {
    package.replace('-', "_")
}

impl Listing {
    /// Parses one `cargo test … -- --list 2>&1` output (several may be added).
    pub fn add(&mut self, text: &str) {
        let mut current: Option<(Option<String>, String)> = None;
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("Running ") {
                // `Running unittests src/lib.rs (/target/debug/build/<package>/<hash>/out/<bin>)`
                // or `Running tests/x.rs (/target/debug/deps/x-<hash>)`.
                let rest = rest.strip_prefix("unittests ").unwrap_or(rest);
                let Some((src, binary)) = rest.split_once(" (") else {
                    current = None;
                    continue;
                };
                let binary = binary.trim_end_matches(')');
                let package = binary
                    .split_once("/build/")
                    .and_then(|(_, after)| after.split('/').next())
                    .map(normalize)
                    .or_else(|| {
                        // The older layout: deps/<crate>-<hash> names the package only for a
                        // library or binary's own unit tests.
                        let file = binary.rsplit('/').next()?;
                        let stem = file.rsplit_once('-')?.0;
                        src.starts_with("src/").then(|| normalize(stem))
                    });
                current = Some((package, src.to_string()));
            } else if trimmed.starts_with("Doc-tests ") {
                current = None;
            } else if let Some((package, src)) = &current
                && let Some(name) = line.strip_suffix(": test")
                && !name.contains(' ')
            {
                self.entries
                    .insert((package.clone(), src.clone(), name.to_string()));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn contains(&self, package: &str, src: &str, name: &str) -> bool {
        let key = |p: Option<String>| (p, src.to_string(), name.to_string());
        self.entries.contains(&key(Some(normalize(package)))) || self.entries.contains(&key(None))
    }
}

/// Scanned files, by path (`Err` when unreadable).
pub type ScanCache = HashMap<PathBuf, Result<Scan, String>>;

fn scanned<'a>(cache: &'a mut ScanCache, path: &Path) -> Option<&'a Scan> {
    cache
        .entry(path.to_path_buf())
        .or_insert_with(|| {
            fs::read_to_string(path)
                .map(|t| rust_scan::scan(&t))
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .ok()
}

/// `[package] name` of `crates/<dir>/Cargo.toml`.
fn package_name(root: &Path, dir: &str) -> Option<String> {
    let text = fs::read_to_string(root.join("crates").join(dir).join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

/// A target file inside one crate: its test-target roots (relative to the crate) with the module
/// path from each root to the file.
fn roots_of(rel: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let parts: Vec<&str> = rel.split('/').collect();
    let module_path = |files: &[&str]| -> Vec<String> {
        let mut segs: Vec<String> = files
            .iter()
            .map(|p| p.trim_end_matches(".rs").to_string())
            .collect();
        if segs.last().map(String::as_str) == Some("mod") {
            segs.pop();
        }
        segs
    };
    let itself = || Ok(vec![(rel.to_string(), Vec::new())]);
    match parts.as_slice() {
        ["src", "lib.rs"] | ["src", "main.rs"] => itself(),
        ["src", "bin", _] | ["src", "bin", _, "main.rs"] => itself(),
        ["src", "bin", b, rest @ ..] => {
            Ok(vec![(format!("src/bin/{b}/main.rs"), module_path(rest))])
        }
        ["src", rest @ ..] => {
            let segs = module_path(rest);
            Ok(vec![
                ("src/lib.rs".to_string(), segs.clone()),
                ("src/main.rs".to_string(), segs),
            ])
        }
        ["tests", _] | ["tests", _, "main.rs"] => itself(),
        ["tests", t, rest @ ..] => Ok(vec![(format!("tests/{t}/main.rs"), module_path(rest))]),
        _ => Err(format!(
            "is not in a test target (src/, src/bin/ or tests/ of a crate; {rel})"
        )),
    }
}

/// The file of module `segs` under target root `root_rel` (both relative to `crate_dir`).
fn module_file(crate_dir: &Path, root_rel: &str, segs: &[String]) -> Option<PathBuf> {
    if segs.is_empty() {
        let p = crate_dir.join(root_rel);
        return p.is_file().then_some(p);
    }
    let base = crate_dir.join(Path::new(root_rel).parent()?);
    let joined = segs.join("/");
    [
        base.join(format!("{joined}.rs")),
        base.join(&joined).join("mod.rs"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Follows the `mod` declarations from `root_rel` to module `segs`. `Err` names the break.
fn reach(
    crate_dir: &Path,
    root_rel: &str,
    segs: &[String],
    cache: &mut ScanCache,
) -> Result<(), String> {
    let root_file = crate_dir.join(root_rel);
    if !root_file.is_file() {
        return Err(format!("no target root {root_rel}"));
    }
    let mut i = segs.len();
    while i > 0 {
        // The declaration of segs[i-1] lives in the nearest file for segs[..j] (j < i); the
        // modules segs[j..i-1] are inline in it.
        let mut found = None;
        for j in (0..i).rev() {
            if let Some(file) = module_file(crate_dir, root_rel, &segs[..j]) {
                found = Some((j, file));
                break;
            }
        }
        let Some((j, file)) = found else {
            return Err(format!("no file declares module {}", segs[..i].join("::")));
        };
        let Some(scan) = scanned(cache, &file) else {
            return Err(format!("cannot read {}", file.display()));
        };
        let want = &segs[j..i];
        let Some(decl) = scan.mods.iter().find(|d| d.path == want) else {
            return Err(format!(
                "no `mod {};` in {} reaches it",
                want.join("::"),
                file.strip_prefix(crate_dir).unwrap_or(&file).display()
            ));
        };
        if decl.has_path {
            return Err(format!(
                "`mod {}` uses #[path], which the ledger does not follow",
                want.join("::")
            ));
        }
        if decl.gated || scan.file_gated {
            return Err(format!(
                "`mod {}` is behind a cfg other than test or feature",
                want.join("::")
            ));
        }
        i = j;
    }
    match scanned(cache, &root_file) {
        Some(s) if s.file_gated => Err(format!("{root_rel} is behind a cfg")),
        _ => Ok(()),
    }
}

/// Checks a `ported` row's `rust_test` (`crates/<dir>/<file.rs>::<mod>::…::<fn>`, named like the
/// Python method `method`). `listings` are the binding `--list` gate (absent with `--no-fresh`).
pub fn check(
    root: &Path,
    target: &str,
    method: &str,
    listings: Option<&Listings>,
    cache: &mut ScanCache,
) -> Option<String> {
    if target.is_empty() {
        return Some("is ported but names no Rust test".into());
    }
    let Some((file, path)) = target.split_once(".rs::") else {
        return Some(format!(
            "rust_test {target:?} is not `<file.rs>::<mod>::…::<fn>`"
        ));
    };
    let file = format!("{file}.rs");
    let relative = Path::new(&file);
    let safe = relative.is_relative()
        && relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    let mut parts = file.splitn(3, '/');
    let (Some("crates"), Some(dir), Some(rel)) = (parts.next(), parts.next(), parts.next()) else {
        return Some(format!(
            "rust_test file {file:?} is not a path under crates/<crate>/"
        ));
    };
    if !safe {
        return Some(format!(
            "rust_test file {file:?} is not a path under crates/<crate>/"
        ));
    }
    let segments: Vec<String> = path.split("::").map(str::to_string).collect();
    if segments.iter().any(|s| !super::is_identifier(s)) {
        return Some(format!(
            "rust_test {target:?} has an empty or invalid path segment"
        ));
    }
    if segments.last().map(String::as_str) != Some(method) {
        return Some(format!(
            "rust_test {target:?} must keep the Python method name {method} (plan §0.1.5)"
        ));
    }

    // Static: the test in its file.
    let full = root.join(relative);
    let Some(scan) = scanned(cache, &full) else {
        return Some(format!("Rust test file {file} is missing"));
    };
    match scan.tests.iter().find(|t| t.path == segments) {
        None => {
            return Some(format!(
                "Rust test {target} is missing (no #[test] fn there)"
            ));
        }
        Some(t) if t.ignored => return Some(format!("Rust test {target} is #[ignore]d")),
        Some(t) if t.gated => {
            return Some(format!(
                "Rust test {target} is behind a cfg other than test or feature (it must run on \
                 Linux and macOS)"
            ));
        }
        Some(t) => {
            if let Some(m) = &t.in_macro
                && !rust_scan::TRUSTED_TEST_MACROS.contains(&m.as_str())
            {
                return Some(format!(
                    "Rust test {target} is inside `{m}!`, whose expansion the ledger cannot check \
                     (trusted test macros: {})",
                    rust_scan::TRUSTED_TEST_MACROS.join(", ")
                ));
            }
        }
    }

    // Static: the file is reached from a test target root.
    let roots = match roots_of(rel) {
        Ok(r) => r,
        Err(e) => return Some(format!("Rust test file {file} {e}")),
    };
    let crate_dir = root.join("crates").join(dir);
    let mut reached = Vec::new();
    let mut breaks = Vec::new();
    for (root_rel, segs) in &roots {
        match reach(&crate_dir, root_rel, segs, cache) {
            Ok(()) => reached.push((root_rel.clone(), segs.clone())),
            Err(e) => breaks.push(format!("from {root_rel}: {e}")),
        }
    }
    if reached.is_empty() {
        return Some(format!(
            "Rust test file {file} is not compiled into a test target ({})",
            breaks.join("; ")
        ));
    }

    // Binding: cargo listed it.
    if let Some(listings) = listings {
        let Some(package) = package_name(root, dir) else {
            return Some(format!("crates/{dir}/Cargo.toml has no [package] name"));
        };
        let names: Vec<(String, String)> = reached
            .iter()
            .map(|(root_rel, segs)| {
                let name: Vec<&str> = segs
                    .iter()
                    .chain(segments.iter())
                    .map(String::as_str)
                    .collect();
                (root_rel.clone(), name.join("::"))
            })
            .collect();
        let found_in = |l: &Listing| names.iter().any(|(r, n)| l.contains(&package, r, n));
        if found_in(&listings.ignored) {
            return Some(format!(
                "Rust test {target} is ignored: `cargo test -- --list --ignored` lists it"
            ));
        }
        if !found_in(&listings.listed) {
            return Some(format!(
                "Rust test {target} is not in the `cargo test -- --list` output (plain or \
                 --all-features): cargo does not compile or run it"
            ));
        }
    }
    None
}

/// `crates/<dir>/…` → `<dir>`.
pub fn crate_dir_of(target: &str) -> Option<&str> {
    let rest = target.strip_prefix("crates/")?;
    let dir = rest.split('/').next()?;
    (!dir.is_empty()).then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_listing_layouts() {
        let mut listing = Listing::default();
        listing.add(
            "   Compiling x v0.1.0\n\
             \x20    Running unittests src/lib.rs (/target/debug/build/anthology-dev/0a/out/anthology_dev-0a)\n\
             ledger::tests::module_args: test\n\
             \n\
             1 test, 0 benchmarks\n\
             \x20    Running tests/ledger.rs (/target/debug/build/anthology-dev/5d/out/ledger-5d)\n\
             a_complete_ledger_passes: test\n\
             \x20    Running tests/old.rs (/target/debug/deps/old-1234)\n\
             legacy: test\n\
             \x20  Doc-tests anthology_dev\n\
             src/lib.rs - x (line 3): test\n",
        );
        assert!(listing.contains("anthology-dev", "src/lib.rs", "ledger::tests::module_args"));
        assert!(listing.contains(
            "anthology-dev",
            "tests/ledger.rs",
            "a_complete_ledger_passes"
        ));
        assert!(!listing.contains("other", "tests/ledger.rs", "a_complete_ledger_passes"));
        assert!(listing.contains("any-package", "tests/old.rs", "legacy"));
        assert!(!listing.contains("anthology-dev", "src/lib.rs", "x"));
    }

    #[test]
    fn target_roots() {
        let r = |rel| {
            roots_of(rel).map(|v| {
                v.into_iter()
                    .map(|(a, b)| (a, b.join("::")))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            r("tests/a.rs"),
            Ok(vec![("tests/a.rs".into(), String::new())])
        );
        assert_eq!(
            r("tests/a/b/mod.rs"),
            Ok(vec![("tests/a/main.rs".into(), "b".into())])
        );
        assert_eq!(
            r("src/x/y.rs"),
            Ok(vec![
                ("src/lib.rs".into(), "x::y".into()),
                ("src/main.rs".into(), "x::y".into())
            ])
        );
        assert_eq!(
            r("src/bin/t.rs"),
            Ok(vec![("src/bin/t.rs".into(), String::new())])
        );
        assert!(r("examples/e.rs").is_err());
        assert!(r("benches/b.rs").is_err());
    }
}
