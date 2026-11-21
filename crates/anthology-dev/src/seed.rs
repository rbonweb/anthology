//! `anthology-dev seed`: a scratch plugin tree whose locks pin a given binary, with that binary
//! installed (design §8.3, §14.2). Owner: RL1.
//!
//! `seed --bin PATH --plugins-dir DIR --config-dir DIR [--target TRIPLE] [--repo OWNER/NAME]`
//!
//! Evals and smokes run the plugins from a scratch copy of `plugins/` with a locally built
//! binary. `seed`:
//! 1. runs `PATH --version` (stdin closed, 10 s limit): it must exit 0 and print exactly
//!    `anthology <version> (<target>)`, the install-time probe of design §8.3;
//! 2. packs it as `anthology-<target>.tar.gz` (one member, `anthology`, mode 0755) and writes
//!    that as the offline seed `<ah>/bin/anthology-<target>.tar.gz` (design §8.4 step 6.1), where
//!    `<ah>` is `<config-dir>/anthology`;
//! 3. installs the "current" layout `<ah>/bin/<version>/`: `anthology`, then `.bin-sha256`,
//!    then `.sha256` last, assembled in a `.tmp-*` folder and renamed into place;
//! 4. rewrites `anthology.lock` in `DIR/anthology` and `DIR/anthology-guardrails` (those that
//!    exist; at least one must) to pin `<version>` and this target's tarball hash. The other
//!    targets keep the hashes of the lock already there when it pins the same version, else
//!    64 zeros.
//!
//! It refuses, before writing anything, a plugin folder that holds files tracked by git (so the
//! real `plugins/` and `port/staging/plugins/` are never rewritten) and a symlinked `<ah>/bin`
//! or `<ah>/bin/<version>`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_core::lock::{self as core_lock, ANTHOLOGY_TAG_PREFIX};
use anthology_core::platform::{self, Target};
use sha2::{Digest, Sha256};

use crate::Output;
use crate::lock::{DEFAULT_REPO, LOCK_FILE, PLUGINS, render, write_atomic};

/// How long the `--version` probe may run.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "usage: anthology-dev seed --bin PATH --plugins-dir DIR --config-dir DIR [--target TRIPLE] [--repo OWNER/NAME]";

/// `seed` options.
#[derive(Debug)]
pub struct SeedArgs {
    pub bin: PathBuf,
    pub plugins_dir: PathBuf,
    pub config_dir: PathBuf,
    pub target: Option<Target>,
    pub repo: String,
}

impl SeedArgs {
    pub fn parse(args: &[OsString]) -> Result<SeedArgs, String> {
        use lexopt::prelude::*;
        let (mut bin, mut plugins, mut config, mut target, mut repo) =
            (None, None, None, None, None);
        let mut parser = lexopt::Parser::from_args(args.iter().cloned());
        while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
            let value = |p: &mut lexopt::Parser| p.value().map_err(|e| e.to_string());
            match arg {
                Long("bin") => bin = Some(PathBuf::from(value(&mut parser)?)),
                Long("plugins-dir") => plugins = Some(PathBuf::from(value(&mut parser)?)),
                Long("config-dir") => config = Some(PathBuf::from(value(&mut parser)?)),
                Long("target") => {
                    let v = value(&mut parser)?.to_string_lossy().into_owned();
                    target =
                        Some(Target::from_triple(&v).ok_or_else(|| format!("unknown target {v}"))?);
                }
                Long("repo") => {
                    repo = Some(
                        value(&mut parser)?
                            .into_string()
                            .map_err(|_| "--repo is not UTF-8".to_string())?,
                    )
                }
                Value(v) => return Err(format!("unexpected argument: {}", v.to_string_lossy())),
                other => return Err(other.unexpected().to_string()),
            }
        }
        Ok(SeedArgs {
            bin: bin.ok_or("--bin is required")?,
            plugins_dir: plugins.ok_or("--plugins-dir is required")?,
            config_dir: config.ok_or("--config-dir is required")?,
            target,
            repo: repo.unwrap_or_else(|| DEFAULT_REPO.to_string()),
        })
    }
}

/// Runs `anthology-dev seed …`; `args` are the arguments after `seed`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let parsed = match SeedArgs::parse(args) {
        Ok(a) => a,
        Err(e) => {
            return Output {
                stderr: format!("anthology-dev: seed: {e}\n{USAGE}\n").into_bytes(),
                code: 2,
                ..Output::default()
            };
        }
    };
    match seed(&parsed) {
        Ok(line) => Output {
            stdout: format!("{line}\n").into_bytes(),
            ..Output::default()
        },
        Err(e) => Output {
            stderr: format!("anthology-dev: seed: {e}\n").into_bytes(),
            code: 1,
            ..Output::default()
        },
    }
}

/// The version that `bin --version` reports for `target` (design §8.3 probe).
pub fn probe(bin: &Path, target: Target) -> Result<String, String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run {} --version: {e}", bin.display()))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    // The reader hands its bytes over a channel, so a grandchild that keeps stdout open cannot
    // hold the probe past its deadline.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.by_ref().take(4096).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let late = || {
        format!(
            "{} --version did not finish within {} s",
            bin.display(),
            PROBE_TIMEOUT.as_secs()
        )
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(late());
            }
            Err(e) => return Err(format!("{} --version: {e}", bin.display())),
        }
    };
    let out = rx
        .recv_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(200)),
        )
        .map_err(|_| late())?;
    if !status.success() {
        return Err(format!("{} --version failed ({status})", bin.display()));
    }
    let text = String::from_utf8_lossy(&out);
    let want_tail = format!(" ({})\n", target.triple());
    let version = text
        .strip_prefix("anthology ")
        .and_then(|r| r.strip_suffix(want_tail.as_str()))
        .filter(|v| !v.contains([' ', '\n']))
        .ok_or_else(|| {
            format!(
                "{} --version printed {text:?}, not \"anthology <version> ({})\"",
                bin.display(),
                target.triple()
            )
        })?;
    Ok(version.to_string())
}

/// `<64 hex>` of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A gzipped tar holding `anthology` (mode 0755, mtime 0, owner 0).
pub fn tarball(binary: &[u8]) -> Result<Vec<u8>, String> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    let mut header = tar::Header::new_ustar();
    header.set_size(binary.len() as u64);
    header.set_mode(0o755);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_entry_type(tar::EntryType::Regular);
    builder
        .append_data(&mut header, "anthology", binary)
        .map_err(|e| format!("cannot pack the binary: {e}"))?;
    let gz = builder
        .into_inner()
        .map_err(|e| format!("cannot pack the binary: {e}"))?;
    gz.finish()
        .map_err(|e| format!("cannot pack the binary: {e}"))
}

/// The repository this anthology-dev was built from (its plugin trees are never seeded).
fn own_repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An error unless `dir` is certainly a scratch copy: not one of this repository's plugin
/// trees, and either outside any git work tree or holding no tracked file. Every git failure
/// other than "not a git repository" refuses (fail closed), as does a missing git.
fn refuse_tracked(dir: &Path) -> Result<(), String> {
    let refuse = |why: String| {
        Err(format!(
            "{}: {why}; seed only a scratch copy of the plugins",
            dir.display()
        ))
    };
    let canonical = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Ok(repo) = std::fs::canonicalize(own_repository()) {
        for own in ["plugins", "port/staging/plugins"] {
            if canonical.starts_with(repo.join(own)) {
                return refuse(format!("it is this repository's {own}"));
            }
        }
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .output()
    };
    let inside = match git(&["rev-parse", "--is-inside-work-tree"]) {
        Ok(o) => o,
        Err(e) => {
            return refuse(format!(
                "cannot run git to check that it is untracked ({e})"
            ));
        }
    };
    if !inside.status.success() {
        let err = String::from_utf8_lossy(&inside.stderr);
        if err.contains("not a git repository") {
            return Ok(());
        }
        return refuse(format!(
            "git cannot tell whether it is tracked ({})",
            err.trim()
        ));
    }
    if String::from_utf8_lossy(&inside.stdout).trim() != "true" {
        return refuse("it is inside a git directory".into());
    }
    match git(&["ls-files", "-z", "--", "."]) {
        Ok(o) if o.status.success() && o.stdout.is_empty() => Ok(()),
        Ok(o) if o.status.success() => refuse("it holds files tracked by git".into()),
        Ok(o) => refuse(format!(
            "git ls-files failed ({})",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => refuse(format!("cannot run git ls-files ({e})")),
    }
}

/// An error when `p` is a symlink.
fn refuse_symlink(p: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_type().is_symlink() => Err(format!(
            "{} is a symlink; refusing to write through it",
            p.display()
        )),
        _ => Ok(()),
    }
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Does the work of `seed`; returns its stdout line.
pub fn seed(a: &SeedArgs) -> Result<String, String> {
    let target = match a.target {
        Some(t) => t,
        None => {
            let (s, m) = platform::uname();
            platform::detect_with((&s, &m))?
        }
    };
    let plugin_dirs: Vec<PathBuf> = PLUGINS
        .iter()
        .map(|p| a.plugins_dir.join(p))
        .filter(|p| p.is_dir())
        .collect();
    if plugin_dirs.is_empty() {
        return Err(format!(
            "{} has neither {} nor {}",
            a.plugins_dir.display(),
            PLUGINS[0],
            PLUGINS[1]
        ));
    }
    for dir in &plugin_dirs {
        refuse_tracked(dir)?;
    }

    let version = probe(&a.bin, target)?;
    let tag = format!("{ANTHOLOGY_TAG_PREFIX}{version}");
    let binary = std::fs::read(&a.bin).map_err(|e| format!("{}: {e}", a.bin.display()))?;
    let archive = tarball(&binary)?;
    let archive_sha = sha256_hex(&archive);

    // The lock first (it validates the version); written after the install.
    let mut digests: BTreeMap<Target, String> =
        Target::ALL.iter().map(|t| (*t, "0".repeat(64))).collect();
    for dir in &plugin_dirs {
        if let Ok(text) = std::fs::read_to_string(dir.join(LOCK_FILE))
            && let Ok(old) = core_lock::parse(&text, ANTHOLOGY_TAG_PREFIX)
            && old.version == version
        {
            digests = old.sha256;
            break;
        }
    }
    digests.insert(target, archive_sha.clone());
    let lock = render(&tag, &a.repo, digests)?;

    let ah = a.config_dir.join("anthology");
    let bin_dir = ah.join("bin");
    refuse_symlink(&ah)?;
    refuse_symlink(&bin_dir)?;
    std::fs::create_dir_all(&bin_dir).map_err(|e| format!("{}: {e}", bin_dir.display()))?;
    let version_dir = bin_dir.join(&version);
    refuse_symlink(&version_dir)?;

    write_atomic(
        &bin_dir.join(format!("anthology-{}.tar.gz", target.triple())),
        &archive,
    )?;

    let staging = bin_dir.join(format!(".tmp-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let installed = (|| {
        write_file(&staging.join("anthology"), &binary, 0o755)?;
        write_file(
            &staging.join(".bin-sha256"),
            format!("{}\n", sha256_hex(&binary)).as_bytes(),
            0o644,
        )?;
        write_file(
            &staging.join(".sha256"),
            format!("{archive_sha}\n").as_bytes(),
            0o644,
        )?;
        if version_dir.exists() {
            let old = bin_dir.join(format!(".tmp-seed-old-{}", std::process::id()));
            std::fs::rename(&version_dir, &old)
                .map_err(|e| format!("{}: {e}", version_dir.display()))?;
            let _ = std::fs::remove_dir_all(&old);
        }
        std::fs::rename(&staging, &version_dir)
            .map_err(|e| format!("{}: {e}", version_dir.display()))
    })();
    if let Err(e) = installed {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    for dir in &plugin_dirs {
        write_atomic(&dir.join(LOCK_FILE), lock.as_bytes())?;
    }
    Ok(format!(
        "seeded anthology {version} ({}) into {}",
        target.triple(),
        version_dir.display()
    ))
}
