//! Lane BS helpers (plan §0.2): the shells under test, a sandbox per test, fake binaries and
//! release archives, PATH masking, and the shared TLS server the real downloaders talk to
//! (design §13.4; BS1 lead rulings T1 to T5).
//!
//! Capabilities come from the machine. `ANTHOLOGY_SHIMS_REQUIRE` (a comma list of
//! `dash,busybox,bash,macsh,net,curl,gnu-wget,busybox-wget,node,nonroot`) turns a missing one
//! into a failure; without it a missing one prints a skip line to stderr and the case passes.
//! The network cases run only with `ANTHOLOGY_SHIMS_NET=1`, where `github.com` resolves to
//! 127.0.0.1 and this process may bind port 443 (ci/jobs/shims-distros.sh).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use anthology_core::lock::{self, Lock};
use anthology_core::platform::{self, Target};
use sha2::{Digest, Sha256};

/// The repository root (this crate's manifest is two levels below it).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A fixture under `crates/anthology-testkit/fixtures/bs/`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/bs")
        .join(name)
}

// --- capabilities ---------------------------------------------------------------------------

/// Whether `cap` is in `ANTHOLOGY_SHIMS_REQUIRE`.
pub fn required(cap: &str) -> bool {
    std::env::var("ANTHOLOGY_SHIMS_REQUIRE")
        .unwrap_or_default()
        .split(',')
        .any(|c| c.trim() == cap)
}

/// `present`, or a skip line (and `false`) when `cap` is missing; a panic when it is required.
pub fn capability(cap: &str, present: bool, case: &str) -> bool {
    if !present {
        assert!(
            !required(cap),
            "{case}: {cap} is required here (ANTHOLOGY_SHIMS_REQUIRE) but missing"
        );
        eprintln!("skip: {case}: no {cap} here");
    }
    present
}

/// `name` on this process's PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The output of a short command, or `None` if it cannot run.
fn probe_output(argv: &[&str]) -> Option<String> {
    let out = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// The effective uid is 0.
pub fn is_root() -> bool {
    probe_output(&["id", "-u"]).is_some_and(|s| s.trim() == "0")
}

/// The network cases may run (`ANTHOLOGY_SHIMS_NET=1`).
pub fn net_enabled() -> bool {
    std::env::var("ANTHOLOGY_SHIMS_NET").is_ok_and(|v| v == "1")
}

/// The downloaders the bootstrap knows, in its order (design §8.4 step 6).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Downloader {
    Curl,
    GnuWget,
    BusyBoxWget,
    Node,
}

impl Downloader {
    pub const ALL: [Downloader; 4] = [
        Downloader::Curl,
        Downloader::GnuWget,
        Downloader::BusyBoxWget,
        Downloader::Node,
    ];

    /// The `ANTHOLOGY_SHIMS_REQUIRE` name.
    pub fn cap(self) -> &'static str {
        match self {
            Downloader::Curl => "curl",
            Downloader::GnuWget => "gnu-wget",
            Downloader::BusyBoxWget => "busybox-wget",
            Downloader::Node => "node",
        }
    }

    /// The program the bootstrap runs.
    pub fn program(self) -> &'static str {
        match self {
            Downloader::Curl => "curl",
            Downloader::GnuWget | Downloader::BusyBoxWget => "wget",
            Downloader::Node => "node",
        }
    }

    /// Whether this downloader is on PATH (a `wget` counts only as the kind it is).
    pub fn present(self) -> bool {
        if which(self.program()).is_none() {
            return false;
        }
        match self {
            Downloader::GnuWget => {
                probe_output(&["wget", "--version"]).is_some_and(|s| s.contains("GNU Wget"))
            }
            Downloader::BusyBoxWget => {
                !Downloader::GnuWget.present()
                    && probe_output(&["wget", "--help"]).is_some_and(|s| s.contains("BusyBox"))
            }
            _ => true,
        }
    }

    /// The programs to mask so the bootstrap picks this one (those before it in the order).
    pub fn mask_before(self) -> Vec<&'static str> {
        match self {
            Downloader::Curl => vec![],
            Downloader::GnuWget | Downloader::BusyBoxWget => vec!["curl"],
            Downloader::Node => vec!["curl", "wget"],
        }
    }
}

/// The shells every matrix row runs under (T1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Shell {
    Dash,
    BusyBox,
    BashPosix,
    MacSh,
}

impl Shell {
    pub const ALL: [Shell; 4] = [Shell::Dash, Shell::BusyBox, Shell::BashPosix, Shell::MacSh];

    /// The `ANTHOLOGY_SHIMS_REQUIRE` name.
    pub fn cap(self) -> &'static str {
        match self {
            Shell::Dash => "dash",
            Shell::BusyBox => "busybox",
            Shell::BashPosix => "bash",
            Shell::MacSh => "macsh",
        }
    }

    /// The argv prefix that runs a script, with an absolute program path; `None` if absent.
    pub fn argv(self) -> Option<Vec<OsString>> {
        match self {
            Shell::Dash => which("dash").map(|p| vec![p.into()]),
            Shell::BusyBox => which("busybox").map(|p| vec![p.into(), "sh".into()]),
            Shell::BashPosix => which("bash").map(|p| vec![p.into(), "--posix".into()]),
            Shell::MacSh => (cfg!(target_os = "macos") && Path::new("/bin/sh").exists())
                .then(|| vec!["/bin/sh".into()]),
        }
    }
}

/// The shells present here, after the REQUIRE check for each absent one.
pub fn shells(case: &str) -> Vec<Shell> {
    let found: Vec<Shell> = Shell::ALL
        .into_iter()
        .filter(|s| capability(s.cap(), s.argv().is_some(), case))
        .collect();
    assert!(!found.is_empty(), "{case}: no shell to test with");
    found
}

/// This machine's release target.
pub fn host_target() -> Target {
    let (s, m) = platform::uname();
    platform::detect_with((&s, &m)).expect("the tests run on a supported platform")
}

// --- archives and releases ------------------------------------------------------------------

/// The lowercase hex sha256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A tar member.
pub enum Member {
    File {
        name: String,
        mode: u32,
        data: Vec<u8>,
    },
    Symlink {
        name: String,
        target: String,
    },
    Dir {
        name: String,
    },
}

/// A `.tar.gz` of `members`.
pub fn tar_gz(members: &[Member]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(enc);
    for m in members {
        let mut h = tar::Header::new_ustar();
        h.set_mtime(1_700_000_000);
        h.set_uid(0);
        h.set_gid(0);
        match m {
            Member::File { name, mode, data } => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(*mode);
                h.set_size(data.len() as u64);
                h.set_cksum();
                b.append_data(&mut h, name, data.as_slice()).unwrap();
            }
            Member::Symlink { name, target } => {
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_mode(0o777);
                h.set_size(0);
                b.append_link(&mut h, name, target).unwrap();
            }
            Member::Dir { name } => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                h.set_size(0);
                h.set_cksum();
                b.append_data(&mut h, name, std::io::empty()).unwrap();
            }
        }
    }
    b.into_inner().unwrap().finish().unwrap()
}

/// The release asset for `binary`: `anthology` (0755) and `LICENSE`.
pub fn release_archive(binary: &[u8]) -> Vec<u8> {
    tar_gz(&[
        Member::File {
            name: "anthology".into(),
            mode: 0o755,
            data: binary.to_vec(),
        },
        Member::File {
            name: "LICENSE".into(),
            mode: 0o644,
            data: b"test license\n".to_vec(),
        },
    ])
}

static SERIAL: AtomicUsize = AtomicUsize::new(0);

/// A unique-per-process name for routes and repos.
pub fn unique(prefix: &str) -> String {
    format!(
        "{prefix}{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::SeqCst)
    )
}

/// The pinned release a sandbox's lock names.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub repo: String,
    pub target: Target,
    pub archive: Vec<u8>,
}

impl Release {
    /// A release of `archive` for this machine, with a repo name unique to the test.
    pub fn new(version: &str, archive: Vec<u8>) -> Release {
        Release {
            version: version.into(),
            repo: format!("anthology-test/{}", unique("r")),
            target: host_target(),
            archive,
        }
    }

    pub fn tag(&self) -> String {
        format!("{}{}", lock::ANTHOLOGY_TAG_PREFIX, self.version)
    }

    pub fn sha256(&self) -> String {
        sha256_hex(&self.archive)
    }

    pub fn asset(&self) -> String {
        format!("anthology-{}.tar.gz", self.target.triple())
    }

    /// The URL path on github.com.
    pub fn url_path(&self) -> String {
        format!(
            "/{}/releases/download/{}/{}",
            self.repo,
            self.tag(),
            self.asset()
        )
    }

    pub fn url(&self) -> String {
        format!("https://github.com{}", self.url_path())
    }

    /// `anthology.lock` in the design §8.2 layout: this target's real hash, the others fixed.
    pub fn lock_text(&self) -> String {
        let sha256 = Target::ALL
            .into_iter()
            .map(|t| {
                let h = if t == self.target {
                    self.sha256()
                } else {
                    format!("{:064x}", t as u8)
                };
                (t, h)
            })
            .collect();
        lock::render_anthology(&Lock {
            version: self.version.clone(),
            tag: self.tag(),
            repo: self.repo.clone(),
            sha256,
        })
    }

    /// The probe's expected line.
    pub fn version_line(&self) -> String {
        format!("anthology {} ({})", self.version, self.target.triple())
    }
}

/// A fake `anthology` binary: a `#!/bin/sh` script answering `--version` with `version_line`,
/// `guard …` with `guard_body`, and `hook …` with `hook_body`; anything else prints its
/// arguments and the RTK variable (for the wrappers).
pub fn fake_binary(version_line: &str, guard_body: &str, hook_body: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\n\
         case ${{1:-}} in\n\
         --version) printf '%s\\n' '{version_line}'; exit 0 ;;\n\
         guard)\n{guard_body}\n;;\n\
         hook)\n{hook_body}\n;;\n\
         *) printf 'args:%s|rtk=%s\\n' \"$*\" \"${{RTK_SUPPRESS_HOOK_WARNING:-}}\"; exit 0 ;;\n\
         esac\n\
         exit 0\n"
    )
    .into_bytes()
}

/// A synthesized 64-bit ELF header for the other architecture (x86_64 ↔ aarch64), 4 KiB.
pub fn other_arch_elf() -> Vec<u8> {
    let machine: u16 = if cfg!(target_arch = "x86_64") {
        183
    } else {
        62
    };
    let mut v = vec![0u8; 4096];
    v[..4].copy_from_slice(b"\x7fELF");
    v[4] = 2; // ELFCLASS64
    v[5] = 1; // little endian
    v[6] = 1; // EV_CURRENT
    v[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    v[18..20].copy_from_slice(&machine.to_le_bytes());
    v[20..24].copy_from_slice(&1u32.to_le_bytes());
    v[24..32].copy_from_slice(&0x400078u64.to_le_bytes()); // entry
    v[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
    v[52..54].copy_from_slice(&64u16.to_le_bytes()); // ehsize
    v[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
    v[56..58].copy_from_slice(&1u16.to_le_bytes()); // phnum
    v
}

/// Deterministic pseudo-random bytes (xorshift), never starting with `#!`.
pub fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut v: Vec<u8> = (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    if v.first() == Some(&b'#') {
        v[0] = 0x80;
    }
    v
}

// --- the sandbox ----------------------------------------------------------------------------

/// The result of one run.
#[derive(Debug)]
pub struct Run {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
    pub timed_out: bool,
}

impl Run {
    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// What to run.
pub struct Spec {
    pub argv: Vec<OsString>,
    pub stdin: Vec<u8>,
    pub env: Vec<(OsString, OsString)>,
    pub path: Option<OsString>,
    pub timeout: Duration,
}

/// A temporary HOME, CLAUDE_CONFIG_DIR and TMPDIR, and scratch copies of both staging plugins
/// with a lock for `release` (T2).
pub struct Sandbox {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub config: PathBuf,
    pub tmpdir: PathBuf,
    pub release: Release,
    extra_env: Vec<(OsString, OsString)>,
    masks: AtomicUsize,
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_tree(&entry.path(), &dest);
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

impl Sandbox {
    pub fn new(release: Release) -> Sandbox {
        let dir = tempfile::Builder::new()
            .prefix("anthology-bs-")
            .tempdir()
            .unwrap();
        // The real path, so messages that print <ah> match what the scripts see.
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let config = root.join("config");
        let tmpdir = root.join("tmp");
        for d in [&home, &config, &tmpdir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let sb = Sandbox {
            _dir: dir,
            root,
            home,
            config,
            tmpdir,
            release,
            extra_env: Vec::new(),
            masks: AtomicUsize::new(0),
        };
        for p in ["anthology", "anthology-guardrails"] {
            let src = crate::sandbox::staged("plugins").join(p);
            for sub in ["hooks", "bin"] {
                if src.join(sub).is_dir() {
                    copy_tree(&src.join(sub), &sb.plugin(p).join(sub));
                }
            }
        }
        sb.write_lock(&sb.release.lock_text());
        sb
    }

    /// The scratch plugin root (`anthology` or `anthology-guardrails`).
    pub fn plugin(&self, name: &str) -> PathBuf {
        self.root.join("plugins").join(name)
    }

    /// Writes `text` as both plugins' anthology.lock.
    pub fn write_lock(&self, text: &str) {
        for p in ["anthology", "anthology-guardrails"] {
            std::fs::write(self.plugin(p).join("anthology.lock"), text).unwrap();
        }
    }

    /// `<ah>` = `$CLAUDE_CONFIG_DIR/anthology`.
    pub fn ah(&self) -> PathBuf {
        self.config.join("anthology")
    }

    pub fn bindir(&self) -> PathBuf {
        self.ah().join("bin")
    }

    pub fn vdir(&self) -> PathBuf {
        self.bindir().join(&self.release.version)
    }

    pub fn bin(&self) -> PathBuf {
        self.vdir().join("anthology")
    }

    /// Adds an environment variable to every later run.
    pub fn set_env(&mut self, key: &str, value: impl Into<OsString>) {
        self.extra_env.push((key.into(), value.into()));
    }

    /// Places the release archive as the offline seed.
    pub fn seed(&self) {
        self.seed_bytes(&self.release.archive.clone());
    }

    pub fn seed_bytes(&self, bytes: &[u8]) {
        std::fs::create_dir_all(self.bindir()).unwrap();
        std::fs::write(self.bindir().join(self.release.asset()), bytes).unwrap();
    }

    /// An installed `<version>` holding `bytes` as the binary (mode `mode`), with markers.
    pub fn install(&self, bytes: &[u8], mode: u32) {
        std::fs::create_dir_all(self.vdir()).unwrap();
        std::fs::write(self.bin(), bytes).unwrap();
        std::fs::set_permissions(self.bin(), std::fs::Permissions::from_mode(mode)).unwrap();
        std::fs::write(
            self.vdir().join(".bin-sha256"),
            format!("{}\n", sha256_hex(bytes)),
        )
        .unwrap();
        std::fs::write(
            self.vdir().join(".sha256"),
            format!("{}\n", self.release.sha256()),
        )
        .unwrap();
    }

    /// The `.broken-*` entries in `<ah>/bin`, sorted.
    pub fn broken(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.bindir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(".broken-"))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// The names in `<ah>/bin`, sorted.
    pub fn bin_entries(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.bindir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// The base PATH: this process's.
    pub fn base_path() -> OsString {
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into())
    }

    /// A PATH of symlinks to every executable on the base PATH except `masked`, then `stubs`
    /// (name, script) first (T3: a stub only for tools that are not downloaders).
    pub fn path_with(&self, masked: &[&str], stubs: &[(&str, &str)]) -> OsString {
        let n = self.masks.fetch_add(1, Ordering::SeqCst);
        let dir = self.root.join(format!("path-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, script) in stubs {
            let p = dir.join(name);
            std::fs::write(&p, script).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let base = Self::base_path();
        for d in std::env::split_paths(&base) {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name();
                let s = name.to_string_lossy();
                if masked.iter().any(|m| *m == s) || dir.join(&name).symlink_metadata().is_ok() {
                    continue;
                }
                if is_executable(&e.path()) {
                    symlink(e.path(), dir.join(&name)).unwrap();
                }
            }
        }
        dir.into_os_string()
    }

    /// The environment of every run (T2): cleared, then PATH, HOME, CLAUDE_CONFIG_DIR,
    /// TMPDIR, LANG, the TLS trust variables in network mode, and a dead proxy otherwise, so
    /// nothing reaches the real github.com.
    fn base_env(&self, path: Option<OsString>) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), path.unwrap_or_else(Self::base_path)),
            ("HOME".into(), self.home.clone().into()),
            ("CLAUDE_CONFIG_DIR".into(), self.config.clone().into()),
            ("TMPDIR".into(), self.tmpdir.clone().into()),
            ("LANG".into(), "C".into()),
        ];
        if net_enabled() {
            let ca: OsString = fixture("ca.pem").into();
            env.push(("SSL_CERT_FILE".into(), ca.clone()));
            env.push(("NODE_EXTRA_CA_CERTS".into(), ca));
        } else {
            for k in ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"] {
                env.push((k.into(), "http://127.0.0.1:9".into()));
            }
        }
        env
    }

    /// Runs `script` (relative to the sandbox root, e.g. `plugins/anthology/hooks/x.sh`) under
    /// `shell` with `args`, feeding `stdin`.
    pub fn run(&self, shell: Shell, script: &str, args: &[&str], stdin: &[u8]) -> Run {
        self.run_spec(self.spec(shell, script, args, stdin))
    }

    /// The default spec for a run; adjust its fields, then call [`Sandbox::run_spec`].
    pub fn spec(&self, shell: Shell, script: &str, args: &[&str], stdin: &[u8]) -> Spec {
        let mut argv = shell.argv().expect("the shell is present");
        argv.push(self.root.join(script).into());
        argv.extend(args.iter().map(OsString::from));
        Spec {
            argv,
            stdin: stdin.to_vec(),
            env: Vec::new(),
            path: None,
            timeout: Duration::from_secs(120),
        }
    }

    pub fn run_spec(&self, spec: Spec) -> Run {
        let mut env = self.base_env(spec.path);
        env.extend(self.extra_env.iter().cloned());
        env.extend(spec.env);
        run_argv(&spec.argv, &env, &spec.stdin, spec.timeout, &self.root)
    }
}

/// Runs `argv` in its own process group with exactly `env`. On timeout the whole group is
/// killed. `elapsed` counts until the process exited **and** both pipes reached EOF (what a
/// hook runner waits for).
pub fn run_argv(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    stdin: &[u8],
    timeout: Duration,
    cwd: &Path,
) -> Run {
    let start = Instant::now();
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {argv:?}: {e}"));
    let pgid = child.id();
    let mut input = child.stdin.take().unwrap();
    let data = stdin.to_vec();
    std::thread::spawn(move || {
        let _ = input.write_all(&data);
    });
    let (tx, rx) = mpsc::channel::<(u8, Vec<u8>)>();
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        let _ = tx.send((1, v));
    });
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        let _ = tx2.send((2, v));
    });
    let deadline = start + timeout;
    let mut timed_out = false;
    while matches!(child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            timed_out = true;
            kill_group(pgid);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Reaps it (already exited, or killed above).
    let status = child.wait().expect("wait for the child");
    let (mut stdout, mut stderr) = (None, None);
    while stdout.is_none() || stderr.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.max(Duration::from_millis(1))) {
            Ok((1, v)) => stdout = Some(v),
            Ok((_, v)) => stderr = Some(v),
            Err(_) => {
                // A leftover process holds a pipe: that is a timeout too.
                timed_out = true;
                kill_group(pgid);
                if let Ok((k, v)) = rx.recv_timeout(Duration::from_secs(5)) {
                    if k == 1 {
                        stdout = Some(v);
                    } else {
                        stderr = Some(v);
                    }
                    continue;
                }
                break;
            }
        }
    }
    let elapsed = start.elapsed();
    // Nothing of the run may outlive it.
    kill_group(pgid);
    Run {
        code: status.code(),
        signal: status.signal(),
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
        elapsed,
        timed_out,
    }
}

/// SIGKILL to the process group, through /bin/sh's builtin `kill` (slim images have no kill
/// binary; `kill -9 -<pgid>` is the spelling dash, BusyBox and bash all accept).
fn kill_group(pgid: u32) {
    let _ = Command::new("/bin/sh")
        .args(["-c", &format!("kill -9 -{pgid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// --- the TLS server (T4, T5) ----------------------------------------------------------------

/// How the server answers one registered path.
#[derive(Clone, Debug)]
pub enum Mode {
    /// 200 with the body at once.
    Normal(Vec<u8>),
    /// 200 with the body in ten pieces over about `millis` ms.
    Slow(Vec<u8>, u64),
    /// 200 with a 1 MB Content-Length and one byte every 200 ms (it never ends in a budget).
    Never,
    /// 404.
    NotFound,
    /// 302 to the same path over plain http.
    RedirectHttp,
    /// 302 to another path on this server.
    Redirect(String),
    /// Closes the connection right after the TLS handshake (an offline server).
    Drop,
    /// 200 with a Content-Length over 64 MiB, then the body as fast as it goes.
    Oversize,
}

/// The process-wide server on 127.0.0.1:443.
pub struct Server {
    routes: Mutex<HashMap<String, Mode>>,
    hits: Mutex<HashMap<String, usize>>,
}

static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

/// The shared server, started on first use (network mode only).
pub fn server() -> Arc<Server> {
    SERVER
        .get_or_init(|| {
            let s = Arc::new(Server {
                routes: Mutex::new(HashMap::new()),
                hits: Mutex::new(HashMap::new()),
            });
            // macOS lets a non-root user bind port 443 only on the wildcard address, so its CI job
            // sets ANTHOLOGY_SHIMS_BIND=0.0.0.0:443; github.com still resolves to 127.0.0.1.
            let addr = std::env::var("ANTHOLOGY_SHIMS_BIND")
                .unwrap_or_else(|_| "127.0.0.1:443".to_string());
            let listener = TcpListener::bind(&addr)
                .unwrap_or_else(|e| panic!("bind {addr} (ANTHOLOGY_SHIMS_NET needs it): {e}"));
            let config = Arc::new(tls_config());
            let srv = s.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    let Ok(tcp) = conn else { continue };
                    let (srv, config) = (srv.clone(), config.clone());
                    std::thread::spawn(move || srv.serve(tcp, config));
                }
            });
            s
        })
        .clone()
}

impl Server {
    /// Routes `path` (e.g. [`Release::url_path`]) to `mode`.
    pub fn register(&self, path: &str, mode: Mode) {
        self.routes.lock().unwrap().insert(path.into(), mode);
    }

    /// How many requests reached `path`.
    pub fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }

    fn serve(&self, tcp: TcpStream, config: Arc<rustls::ServerConfig>) {
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
        let Ok(conn) = rustls::ServerConnection::new(config) else {
            return;
        };
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
            match tls.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
            if req.len() > 65536 {
                return;
            }
        }
        let line = String::from_utf8_lossy(&req);
        let path = line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .split('?')
            .next()
            .unwrap_or("/")
            .to_string();
        *self.hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
        let mode = self
            .routes
            .lock()
            .unwrap()
            .get(&path)
            .cloned()
            .unwrap_or(Mode::NotFound);
        let head = |status: &str, len: usize, extra: &str| {
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {len}\r\n{extra}Connection: close\r\n\r\n"
            )
        };
        let _ = match mode {
            Mode::Normal(body) => tls
                .write_all(head("200 OK", body.len(), "").as_bytes())
                .and_then(|()| tls.write_all(&body)),
            Mode::Slow(body, millis) => {
                let r = tls.write_all(head("200 OK", body.len(), "").as_bytes());
                let piece = body.len().div_ceil(10).max(1);
                let mut r2 = r;
                for c in body.chunks(piece) {
                    if r2.is_err() {
                        break;
                    }
                    let _ = tls.flush();
                    std::thread::sleep(Duration::from_millis(millis / 10));
                    r2 = tls.write_all(c);
                }
                r2
            }
            Mode::Never => {
                // Under the 64 MiB cap, so only the budget can stop it.
                let mut r = tls.write_all(head("200 OK", 1_000_000, "").as_bytes());
                while r.is_ok() {
                    r = tls.write_all(b"x").and_then(|()| tls.flush());
                    std::thread::sleep(Duration::from_millis(200));
                }
                r
            }
            Mode::NotFound => tls.write_all(head("404 Not Found", 0, "").as_bytes()),
            Mode::RedirectHttp => tls.write_all(
                head(
                    "302 Found",
                    0,
                    &format!("Location: http://github.com{path}\r\n"),
                )
                .as_bytes(),
            ),
            Mode::Redirect(to) => tls.write_all(
                head(
                    "302 Found",
                    0,
                    &format!("Location: https://github.com{to}\r\n"),
                )
                .as_bytes(),
            ),
            Mode::Drop => Ok(()),
            Mode::Oversize => {
                let len = 70 * 1024 * 1024;
                let mut r = tls.write_all(head("200 OK", len, "").as_bytes());
                let chunk = vec![0u8; 65536];
                let mut sent = 0;
                while r.is_ok() && sent < len {
                    r = tls.write_all(&chunk);
                    sent += chunk.len();
                }
                r
            }
        };
        let _ = tls.flush();
        tls.conn.send_close_notify();
        let _ = tls.flush();
    }
}

/// Decodes standard base64 (the PEM body); whitespace is skipped.
fn base64_decode(text: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// The DER of a one-certificate PEM file.
pub fn pem_der(text: &str) -> Vec<u8> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    base64_decode(&body)
}

fn hex_decode(text: &str) -> Vec<u8> {
    let t = text.trim();
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).unwrap())
        .collect()
}

/// The github.com server config: the committed certificate and the hex PKCS#8 key (T5).
fn tls_config() -> rustls::ServerConfig {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let cert = pem_der(&std::fs::read_to_string(fixture("server.pem")).unwrap());
    let key = hex_decode(&std::fs::read_to_string(fixture("server-key.pkcs8.hex")).unwrap());
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_a_known_value() {
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ="), b"hello world");
        assert_eq!(base64_decode("YQ==\n"), b"a");
    }

    #[test]
    fn the_fixture_certificate_decodes() {
        let der = pem_der(&std::fs::read_to_string(fixture("server.pem")).unwrap());
        assert_eq!(der[0], 0x30, "a DER SEQUENCE");
        let _ = tls_config();
    }

    #[test]
    fn the_lock_text_parses() {
        let r = Release::new("0.1.0-rc.1", release_archive(b"x"));
        let l = lock::parse(&r.lock_text(), lock::ANTHOLOGY_TAG_PREFIX).unwrap();
        assert_eq!(l.sha256[&r.target], r.sha256());
    }
}
