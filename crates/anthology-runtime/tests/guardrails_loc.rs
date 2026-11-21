//! Port of `tests/test_guardrails_loc.py` (RT1a): the guardrails folder is the one the hook
//! really runs (P2/B2).
//!
//! A `directory` marketplace loads its plugins in place; `installed_plugins.json` still names an
//! install-time snapshot in the plugin cache that never runs. A GitHub marketplace runs the
//! cache. Fixtures use the staging layout: a plugin folder "has the guard" when it holds
//! `hooks/guard.sh` (F-6), so `copy_guardrails` writes that file instead of copying the Python
//! plugin (its `guard_rules.py` markers only served `load_rules`, which F-7 removed).
//!
//! Each test keeps the Python method name. The `SelfTestFollowsTest` rows are RT3's, and the
//! `load_rules` rows are `dropped:superseded:F-7` (port/ledger/test_guardrails_loc.tsv).

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_runtime::guardrails_loc::{self, GuardLoc, Problem, REPO_READ_LIMIT};
use anthology_testkit::rt_a::{RealTemp, write};

const DISABLE: &str = r#"{"enabledPlugins": {"anthology-guardrails@anthology": false}}"#;
const FLAG: &str = r#""enabledPlugins":{"anthology-guardrails@anthology":false}"#;
const BIG: usize = 3 << 20;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;

/// The real anthology plugin folder (`PLUGIN` in the Python tests).
fn plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/anthology")
}

fn loc(root: Option<&Path>, problem: Option<Problem>) -> GuardLoc {
    GuardLoc {
        root: root.map(Path::to_path_buf),
        problem,
    }
}

/// `LiveRootCase` (with `MemCase`'s temporary HOME and config).
struct Case {
    _tmp: RealTemp,
    tmp: PathBuf,
    config: PathBuf,
    cache: PathBuf,
    market: PathBuf,
    live: PathBuf,
}

impl Case {
    fn new() -> Case {
        let t = RealTemp::new();
        let tmp = t.path.clone();
        let config = tmp.join("home").join(".claude");
        fs::create_dir_all(tmp.join("home")).unwrap();
        fs::create_dir_all(tmp.join("data")).unwrap();
        let cache = config.join("plugins/cache/anthology/anthology-guardrails/0.1.0");
        let market = tmp.join("market");
        let live = market.join("plugins/anthology-guardrails");
        Case {
            _tmp: t,
            tmp,
            config,
            cache,
            market,
            live,
        }
    }

    fn env(&self) -> Env {
        Env::default().with_os("CLAUDE_CONFIG_DIR", &self.config)
    }

    /// A guardrails plugin folder: its `hooks/guard.sh` (F-6).
    fn copy_guardrails(&self, dest: &Path) -> PathBuf {
        write(&dest.join("hooks/guard.sh"), "#!/bin/sh\nexit 2\n");
        dest.to_path_buf()
    }

    fn installed_text(&self, install_path: Option<&Path>, extra: &str, pad: usize) -> String {
        let install = install_path.unwrap_or(&self.cache).display().to_string();
        let mut plugins = format!(
            r#""anthology-guardrails@anthology": [{{"scope": "user", "installPath": "{install}", "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}}]"#
        );
        if !extra.is_empty() {
            plugins = format!("{plugins}, {extra}");
        }
        let pad = if pad > 0 {
            format!(r#", "pad": "{}""#, "A".repeat(pad))
        } else {
            String::new()
        };
        format!(r#"{{"version": 2, "plugins": {{{plugins}}}{pad}}}"#)
    }

    fn installed(&self) {
        self.installed_with(None, "");
    }

    fn installed_with(&self, install_path: Option<&Path>, extra: &str) {
        write(
            &self.config.join("plugins/installed_plugins.json"),
            self.installed_text(install_path, extra, 0),
        );
    }

    fn known_text(&self, source: &str, location: Option<&Path>, pad: usize) -> String {
        let location = location.unwrap_or(&self.market).display().to_string();
        let pad = if pad > 0 {
            format!(r#", "pad": "{}""#, "A".repeat(pad))
        } else {
            String::new()
        };
        format!(
            r#"{{"anthology": {{"source": {source}, "installLocation": "{location}", "lastUpdated": "x"}}{pad}}}"#
        )
    }

    fn known(&self, source: &str, location: Option<&Path>) {
        write(
            &self.config.join("plugins/known_marketplaces.json"),
            self.known_text(source, location, 0),
        );
    }

    fn directory_known(&self) {
        self.known(
            &format!(
                r#"{{"source": "directory", "path": "{}"}}"#,
                self.market.display()
            ),
            None,
        );
    }

    /// `manifest(source)`; `source` is JSON text.
    fn manifest_json(&self, source: &str) {
        write(
            &self.market.join(".claude-plugin/marketplace.json"),
            format!(
                r#"{{"name": "anthology", "plugins": [{{"name": "anthology-guardrails", "source": {source}}}]}}"#
            ),
        );
    }

    fn manifest(&self) {
        self.manifest_json(r#""./plugins/anthology-guardrails""#);
    }

    fn inspect(&self) -> GuardLoc {
        self.inspect_from(&plugin())
    }

    fn inspect_from(&self, plugin_root: &Path) -> GuardLoc {
        guardrails_loc::inspect(plugin_root, &self.env(), &self.tmp)
    }

    fn disabled(&self) -> bool {
        guardrails_loc::disabled(&self.env(), &self.tmp)
    }

    /// The cache copy and the in-place marketplace copy both exist.
    fn diverging(&self) {
        self.copy_guardrails(&self.cache);
        self.copy_guardrails(&self.live);
        self.manifest();
        self.directory_known();
        self.installed();
    }

    fn project_settings(&self, name: &str) -> PathBuf {
        self.tmp.join(".claude").join(name)
    }

    fn settings_bytes(&self, raw: &[u8], name: &str) {
        write(&self.project_settings(name), raw);
    }

    /// `FallbackTest.sibling_case`.
    fn sibling_case(&self) -> (PathBuf, PathBuf) {
        let base = self.tmp.join("plugins-dir");
        let root = base.join("anthology");
        fs::create_dir_all(&root).unwrap();
        let sibling = self.copy_guardrails(&base.join("anthology-guardrails"));
        self.copy_guardrails(&self.cache);
        self.installed();
        (root, sibling)
    }

    /// `EscapingSourceTest.assert_not_accepted` (`source` is JSON text).
    fn assert_not_accepted(&self, source: &str) {
        if !self.cache.exists() {
            self.copy_guardrails(&self.cache);
        }
        self.directory_known();
        self.manifest_json(source);
        self.installed();
        assert_eq!(
            self.inspect_from(&self.tmp.join("lone/anthology")),
            loc(None, Some(Problem::LiveMissing)),
            "{source}"
        );
    }

    /// `CacheSnapshotGoneTest.installed`: anthology itself is installed too.
    fn installed_with_anthology(&self) {
        let anthology = self.tmp.join("anthology").display().to_string();
        self.installed_with(
            None,
            &format!(
                r#""anthology@anthology": [{{"scope": "user", "installPath": "{anthology}", "version": "0.1.0"}}]"#
            ),
        );
    }

    /// `CacheSnapshotGoneTest.live_only`.
    fn live_only(&self, cache_dir: bool) {
        self.copy_guardrails(&self.live);
        if cache_dir {
            fs::create_dir_all(self.cache.join("hooks")).unwrap(); // the snapshot has no guard
        }
        self.manifest();
        self.directory_known();
        self.installed_with_anthology();
    }

    fn put(&self, place: &str, value: &str) {
        let path = match place {
            "user" => self.config.join("settings.json"),
            "repo" => self.project_settings("settings.json"),
            "local" => self.project_settings("settings.local.json"),
            other => panic!("{other}"),
        };
        write(
            &path,
            format!(r#"{{"enabledPlugins": {{"anthology-guardrails@anthology": {value}}}}}"#),
        );
    }
}

fn utf16(text: &str, le: bool) -> Vec<u8> {
    let mut out: Vec<u8> = if le {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for u in text.encode_utf16() {
        out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
    }
    out
}

fn disable_with_pad(pad: usize) -> String {
    format!(
        r#"{{"enabledPlugins": {{"anthology-guardrails@anthology": false}}, "pad": "{}"}}"#,
        "A".repeat(pad)
    )
}

fn pad_only(pad: usize) -> String {
    format!(r#"{{"pad": "{}"}}"#, "A".repeat(pad))
}

/// A FIFO at `path`, opened read-write and non-blocking with `data` written into it; the
/// returned file keeps it open.
fn fifo_with(path: &Path, data: &[u8]) -> fs::File {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(path)
        .unwrap();
    std::io::Write::write_all(&mut f, data).unwrap();
    f
}

// --- ResolutionTest

#[test]
fn test_directory_marketplace_resolves_to_the_marketplace_copy() {
    let c = Case::new();
    c.diverging();
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_github_marketplace_uses_install_path() {
    let c = Case::new();
    c.diverging();
    c.known(
        r#"{"source": "github", "repo": "owner/anthology", "ref": "build"}"#,
        Some(&c.config.join("plugins/marketplaces/anthology")),
    );
    assert_eq!(c.inspect(), loc(Some(&c.cache), None));
}

#[test]
fn test_other_source_types_use_install_path() {
    let c = Case::new();
    c.diverging();
    for source in [
        r#"{"source": "git", "url": "https://example.invalid/x.git"}"#,
        r#"{"source": "url", "url": "https://example.invalid/m.json"}"#,
    ] {
        c.known(source, None);
        assert_eq!(c.inspect(), loc(Some(&c.cache), None), "{source}");
    }
}

#[test]
fn test_directory_marketplace_with_a_symlinked_install_location() {
    let c = Case::new();
    c.diverging();
    let link = c.tmp.join("link");
    symlink(&c.market, &link).unwrap();
    c.known(
        &format!(r#"{{"source": "directory", "path": "{}"}}"#, link.display()),
        Some(&link),
    );
    let found = c.inspect();
    assert_eq!(found.problem, None);
    assert_eq!(
        fs::canonicalize(found.root.unwrap()).unwrap(),
        fs::canonicalize(&c.live).unwrap()
    );
}

// --- FallbackTest

#[test]
fn test_missing_known_marketplaces_outside_the_cache_uses_the_sibling() {
    let c = Case::new();
    let (root, sibling) = c.sibling_case();
    assert_eq!(c.inspect_from(&root), loc(Some(&sibling), None));
}

#[test]
fn test_garbage_known_marketplaces_outside_the_cache_uses_the_sibling() {
    let c = Case::new();
    let (root, sibling) = c.sibling_case();
    for text in [
        "{not json",
        "[]",
        r#"{"anthology": 3}"#,
        r#"{"anthology": {"source": "directory"}}"#,
        "",
    ] {
        write(&c.config.join("plugins/known_marketplaces.json"), text);
        assert_eq!(c.inspect_from(&root), loc(Some(&sibling), None), "{text}");
    }
}

#[test]
fn test_directory_marketplace_with_an_unusable_manifest_is_a_problem_not_the_sibling() {
    let c = Case::new();
    let (root, _sibling) = c.sibling_case();
    c.directory_known();
    // a directory marketplace never runs the cache copy: an unusable manifest is a problem
    let missing = loc(None, Some(Problem::LiveMissing));
    assert_eq!(c.inspect_from(&root), missing); // no marketplace.json
    let manifest = c.market.join(".claude-plugin/marketplace.json");
    write(&manifest, "{broken");
    assert_eq!(c.inspect_from(&root), missing);
    write(&manifest, r#"{"plugins": "x"}"#);
    assert_eq!(c.inspect_from(&root), missing);
    write(&manifest, r#"{"plugins": []}"#); // not listed
    assert_eq!(c.inspect_from(&root), missing);
}

#[test]
fn test_plugin_root_inside_the_cache_uses_install_path() {
    let c = Case::new();
    c.copy_guardrails(&c.cache);
    let in_cache = c.config.join("plugins/cache/anthology/anthology/0.1.0");
    fs::create_dir_all(&in_cache).unwrap();
    // a sibling-looking folder exists next to it, but a cached plugin is not loaded in place
    c.copy_guardrails(&in_cache.parent().unwrap().join("anthology-guardrails"));
    c.installed();
    assert_eq!(c.inspect_from(&in_cache), loc(Some(&c.cache), None));
}

#[test]
fn test_outside_the_cache_without_a_sibling_uses_install_path() {
    let c = Case::new();
    c.copy_guardrails(&c.cache);
    c.installed();
    let lone = c.tmp.join("lone/anthology");
    fs::create_dir_all(&lone).unwrap();
    assert_eq!(c.inspect_from(&lone), loc(Some(&c.cache), None));
}

#[test]
fn test_dev_session_without_registry_entries_uses_the_sibling() {
    let c = Case::new();
    let base = c.tmp.join("dev");
    fs::create_dir_all(base.join("anthology")).unwrap();
    let sibling = c.copy_guardrails(&base.join("anthology-guardrails"));
    assert_eq!(
        c.inspect_from(&base.join("anthology")),
        loc(Some(&sibling), None)
    );
}

#[test]
fn test_problems_keep_their_meaning() {
    let c = Case::new();
    assert_eq!(
        c.inspect_from(&c.tmp.join("nowhere/anthology")),
        loc(None, Some(Problem::NotFound))
    );
    write(
        &c.config.join("plugins/installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"anthology@anthology": [{{"scope": "user", "installPath": "{}"}}]}}}}"#,
            plugin().display()
        ),
    );
    assert_eq!(c.inspect(), loc(None, Some(Problem::NotInstalled)));
    c.copy_guardrails(&c.cache);
    c.installed();
    write(&c.config.join("settings.json"), DISABLE);
    assert_eq!(
        c.inspect_from(&c.tmp.join("lone/anthology")),
        loc(Some(&c.cache), Some(Problem::Disabled))
    );
}

// --- EscapingSourceTest

#[test]
fn test_parent_escape_is_refused() {
    let c = Case::new();
    // a real guard sits at each escape target, so only the containment check can refuse it
    c.copy_guardrails(&c.tmp.join("outside"));
    c.assert_not_accepted(r#""../outside""#);
    c.assert_not_accepted(r#""./plugins/../../outside""#);
    c.assert_not_accepted(r#""../../etc""#);
}

#[test]
fn test_absolute_source_is_refused() {
    let c = Case::new();
    let outside = c.copy_guardrails(&c.tmp.join("abs-outside"));
    c.assert_not_accepted(&format!(r#""{}""#, outside.display()));
}

#[test]
fn test_symlink_escape_is_refused() {
    let c = Case::new();
    let outside = c.copy_guardrails(&c.tmp.join("sym-outside"));
    fs::create_dir_all(c.market.join("plugins")).unwrap();
    symlink(&outside, c.market.join("plugins/anthology-guardrails")).unwrap();
    c.assert_not_accepted(r#""./plugins/anthology-guardrails""#);
}

#[test]
fn test_symlink_escape_is_not_undone_by_the_sibling_fallback() {
    let c = Case::new();
    // the in-place layout: <market>/plugins/anthology next to an escaping guardrails link
    let outside = c.copy_guardrails(&c.tmp.join("sym-outside"));
    let plugins = c.market.join("plugins");
    fs::create_dir_all(plugins.join("anthology")).unwrap();
    symlink(&outside, plugins.join("anthology-guardrails")).unwrap();
    c.copy_guardrails(&c.cache);
    c.directory_known();
    c.manifest();
    c.installed();
    assert_eq!(
        c.inspect_from(&plugins.join("anthology")),
        loc(None, Some(Problem::LiveMissing))
    );
    fs::remove_file(c.config.join("plugins/known_marketplaces.json")).unwrap(); // the plain fallback
    assert_eq!(
        c.inspect_from(&plugins.join("anthology")),
        loc(Some(&c.cache), None)
    );
}

#[test]
fn test_sibling_link_to_the_folder_itself_is_refused() {
    let c = Case::new();
    // <plugins>/anthology-guardrails -> <plugins>, which itself has the guard
    let plugins = c.tmp.join("dev");
    fs::create_dir_all(plugins.join("anthology")).unwrap();
    c.copy_guardrails(&plugins);
    symlink(&plugins, plugins.join("anthology-guardrails")).unwrap();
    assert_eq!(
        c.inspect_from(&plugins.join("anthology")),
        loc(None, Some(Problem::NotFound))
    );
}

#[test]
fn test_dev_session_does_not_follow_an_escaping_sibling_link() {
    let c = Case::new();
    let outside = c.copy_guardrails(&c.tmp.join("sym-outside"));
    fs::create_dir_all(c.tmp.join("dev/anthology")).unwrap();
    symlink(&outside, c.tmp.join("dev/anthology-guardrails")).unwrap();
    assert_eq!(
        c.inspect_from(&c.tmp.join("dev/anthology")),
        loc(None, Some(Problem::NotFound))
    );
}

#[test]
fn test_source_of_the_wrong_type_is_refused() {
    let c = Case::new();
    for source in [
        r#"{"source": "github", "repo": "a/b"}"#,
        "5",
        r#""""#,
        "null",
    ] {
        c.assert_not_accepted(source);
    }
}

// --- LiveMissingTest: a listed in-place folder without the guard is what the hook runs

#[test]
fn test_live_folder_without_guard_py_is_live_missing_not_the_cache() {
    let c = Case::new();
    c.copy_guardrails(&c.cache);
    fs::create_dir_all(c.live.join("hooks")).unwrap();
    c.manifest();
    c.directory_known();
    c.installed();
    assert_eq!(c.inspect(), loc(None, Some(Problem::LiveMissing)));
    assert_eq!(
        c.inspect_from(&c.market.join("plugins/anthology")),
        loc(None, Some(Problem::LiveMissing))
    );
}

#[test]
fn test_live_folder_that_does_not_exist_is_live_missing() {
    let c = Case::new();
    c.copy_guardrails(&c.cache);
    c.manifest();
    c.directory_known();
    c.installed();
    assert_eq!(c.inspect(), loc(None, Some(Problem::LiveMissing)));
}

#[test]
fn test_disabled_takes_priority_over_live_missing() {
    let c = Case::new();
    c.copy_guardrails(&c.cache);
    fs::create_dir_all(c.live.join("hooks")).unwrap();
    c.manifest();
    c.directory_known();
    c.installed();
    write(&c.config.join("settings.json"), DISABLE);
    assert_eq!(c.inspect(), loc(None, Some(Problem::Disabled)));
}

#[test]
fn test_unlisted_plugin_and_missing_manifest_are_live_missing() {
    let c = Case::new();
    c.copy_guardrails(&c.cache);
    c.directory_known();
    c.installed();
    assert_eq!(c.inspect(), loc(None, Some(Problem::LiveMissing))); // no marketplace.json
    write(
        &c.market.join(".claude-plugin/marketplace.json"),
        r#"{"name": "anthology", "plugins": [{"name": "other", "source": "./plugins/other"}]}"#,
    );
    assert_eq!(c.inspect(), loc(None, Some(Problem::LiveMissing)));
}

#[test]
fn test_live_folder_with_guard_py_is_unaffected() {
    let c = Case::new();
    c.diverging();
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

// --- CacheSnapshotGoneTest (m13): a directory marketplace runs the live folder

#[test]
fn test_missing_cache_folder_still_resolves_to_the_live_folder() {
    let c = Case::new();
    c.live_only(false);
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_cache_folder_without_guard_py_still_resolves_to_the_live_folder() {
    let c = Case::new();
    c.live_only(true);
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_neither_folder_has_guard_py_is_not_installed() {
    let c = Case::new();
    fs::create_dir_all(c.live.join("hooks")).unwrap();
    c.manifest();
    c.directory_known();
    c.installed_with_anthology();
    assert_eq!(c.inspect(), loc(None, Some(Problem::NotInstalled)));
}

#[test]
fn test_cache_gone_with_a_non_directory_marketplace_is_not_installed() {
    let c = Case::new();
    c.copy_guardrails(&c.live);
    c.manifest();
    c.known(r#"{"source": "github", "repo": "owner/anthology"}"#, None);
    c.installed_with_anthology();
    assert_eq!(c.inspect(), loc(None, Some(Problem::NotInstalled)));
}

#[test]
fn test_cache_gone_and_live_source_escaping_is_not_installed_not_the_cache() {
    let c = Case::new();
    c.copy_guardrails(&c.tmp.join("outside/anthology-guardrails"));
    c.manifest_json(r#""../outside/anthology-guardrails""#);
    c.directory_known();
    c.installed_with_anthology();
    assert_eq!(c.inspect(), loc(None, Some(Problem::NotInstalled)));
}

#[test]
fn test_disabled_still_wins_when_the_cache_is_gone() {
    let c = Case::new();
    c.live_only(false);
    write(&c.config.join("settings.json"), DISABLE);
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_project_scoped_entry_for_another_project_is_still_skipped() {
    let c = Case::new();
    c.live_only(false);
    write(
        &c.config.join("plugins/installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"anthology@anthology": [{{"scope": "user", "installPath": "{}"}}], "anthology-guardrails@anthology": [{{"scope": "project", "projectPath": "{}", "installPath": "{}", "lastUpdated": "2026-01-01T00:00:00Z"}}]}}}}"#,
            c.tmp.join("anthology").display(),
            c.tmp.join("elsewhere").display(),
            c.cache.display()
        ),
    );
    assert_eq!(c.inspect(), loc(None, Some(Problem::NotInstalled)));
}

// --- BoundedReadTest (m15): working-folder settings are read only when regular and at most
// 2 MiB; one that exists but is refused counts as DISABLED (fail closed)

#[test]
fn test_limit_is_claude_codes_own() {
    assert_eq!(REPO_READ_LIMIT, 2 * 1024 * 1024);
}

#[test]
fn test_oversized_project_settings_count_as_disabled() {
    let c = Case::new();
    c.diverging();
    write(
        &c.project_settings("settings.json"),
        disable_with_pad(REPO_READ_LIMIT as usize + 10),
    );
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_oversized_project_settings_without_the_flag_also_count_as_disabled() {
    let c = Case::new();
    c.diverging();
    write(&c.project_settings("settings.local.json"), pad_only(BIG));
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_one_and_a_half_mib_that_disables_the_guard_is_honoured() {
    let c = Case::new();
    c.diverging();
    write(
        &c.project_settings("settings.json"),
        disable_with_pad(3 << 19),
    );
    let size = fs::metadata(c.project_settings("settings.json"))
        .unwrap()
        .len();
    assert!(1 << 20 < size && size < REPO_READ_LIMIT, "{size}");
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_one_and_a_half_mib_without_the_flag_is_read_and_active() {
    let c = Case::new();
    c.diverging();
    write(&c.project_settings("settings.json"), pad_only(3 << 19));
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_file_exactly_at_the_limit_is_read_one_byte_over_is_refused() {
    let c = Case::new();
    c.diverging();
    let base = pad_only(0).len();
    let at_limit = pad_only(REPO_READ_LIMIT as usize - base);
    assert_eq!(at_limit.len() as u64, REPO_READ_LIMIT);
    write(&c.project_settings("settings.json"), &at_limit);
    assert_eq!(c.inspect(), loc(Some(&c.live), None)); // read, and it does not disable
    write(
        &c.project_settings("settings.json"),
        at_limit.replace(r#""pad": ""#, r#""pad": "A"#),
    );
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled))); // refused
}

#[test]
fn test_small_project_settings_still_disable_and_plain_ones_do_not() {
    let c = Case::new();
    c.diverging();
    write(&c.project_settings("settings.json"), DISABLE);
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
    write(
        &c.project_settings("settings.json"),
        r#"{"enabledPlugins": {"other@x": false}}"#,
    );
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_absent_and_empty_repo_settings_are_not_refusals() {
    let c = Case::new();
    c.diverging();
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
    write(&c.project_settings("settings.json"), ""); // Claude Code reads an empty file as {}
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
    write(&c.project_settings("settings.json"), " \n");
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_a_repo_settings_file_that_does_not_parse_counts_as_disabled() {
    let c = Case::new();
    c.diverging();
    for name in ["settings.json", "settings.local.json"] {
        for text in ["{not json", "[1, 2", r#"{"a": }"#, "\x00"] {
            write(&c.project_settings(name), text);
            assert_eq!(
                c.inspect(),
                loc(Some(&c.live), Some(Problem::Disabled)),
                "{name} {text:?}"
            );
        }
        fs::remove_file(c.project_settings(name)).unwrap();
    }
}

#[test]
fn test_a_malformed_users_own_settings_file_is_ignored() {
    let c = Case::new();
    c.diverging();
    write(&c.config.join("settings.json"), "{not json");
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_a_non_regular_file_counts_as_disabled_even_when_it_reads_as_nothing() {
    // /dev/null reads as empty, so only the regular-file check can refuse it
    let c = Case::new();
    c.diverging();
    fs::create_dir_all(c.tmp.join(".claude")).unwrap();
    symlink("/dev/null", c.project_settings("settings.json")).unwrap();
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_a_fifo_with_data_counts_as_disabled_without_blocking() {
    let c = Case::new();
    c.diverging();
    let _keep = fifo_with(&c.project_settings("settings.json"), b"{}");
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_an_unreadable_repo_settings_file_counts_as_disabled() {
    let c = Case::new();
    c.diverging();
    let path = write(&c.project_settings("settings.json"), "{}");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    // the suite runs as a normal user (scripts/rust-test.sh, CI): root would read it anyway
    assert!(fs::read(&path).is_err(), "the file must be unreadable here");
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn test_device_symlink_is_not_read_and_counts_as_disabled() {
    let c = Case::new();
    c.diverging();
    fs::create_dir_all(c.tmp.join(".claude")).unwrap();
    symlink("/dev/zero", c.project_settings("settings.json")).unwrap();
    let start = Instant::now();
    assert_eq!(
        c.inspect().problem,
        Some(Problem::Disabled),
        "a device is refused"
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "a device is never read"
    );
}

// --- DecodeLikeClaudeCodeTest (I-1b): settings are decoded leniently before JSON.parse

#[test]
fn test_utf16le_with_a_bom_is_decoded() {
    let c = Case::new();
    c.settings_bytes(&utf16(&format!("{{{FLAG}}}"), true), "settings.json");
    assert!(c.disabled());
    c.settings_bytes(&utf16(r#"{"other": 1}"#, true), "settings.json");
    assert!(!c.disabled()); // decoded and read, not refused
}

#[test]
fn test_utf16be_with_a_bom_is_decoded() {
    let c = Case::new();
    c.settings_bytes(&utf16(&format!("{{{FLAG}}}"), false), "settings.json");
    assert!(c.disabled());
    c.settings_bytes(&utf16(r#"{"other": 1}"#, false), "settings.json");
    assert!(!c.disabled()); // decoded and read, not refused
}

#[test]
fn test_utf8_bom_is_stripped() {
    let c = Case::new();
    c.settings_bytes(
        &[b"\xef\xbb\xbf".as_slice(), format!("{{{FLAG}}}").as_bytes()].concat(),
        "settings.json",
    );
    assert!(c.disabled());
}

#[test]
fn test_an_invalid_byte_in_an_unrelated_value_does_not_hide_the_flag() {
    let c = Case::new();
    c.settings_bytes(
        &[
            format!(r#"{{{FLAG},"env":{{"X":"a"#).as_bytes(),
            b"\xff\"}}".as_slice(),
        ]
        .concat(),
        "settings.json",
    );
    assert!(c.disabled());
    c.settings_bytes(b"{\"env\":{\"X\":\"a\xff\"}}", "settings.json");
    assert!(!c.disabled()); // replaced and parsed: no flag, nothing refused
}

#[test]
fn test_a_deeply_nested_value_counts_as_disabled() {
    let c = Case::new();
    let deep = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
    c.settings_bytes(
        format!(r#"{{{FLAG},"zz":{deep}}}"#).as_bytes(),
        "settings.json",
    );
    assert!(c.disabled());
    c.settings_bytes(
        format!(r#"{{"zz":{deep}}}"#).as_bytes(),
        "settings.local.json",
    );
    assert!(c.disabled());
    // the nesting alone refuses it (Python's RecursionError), not the flag next door
    fs::remove_file(c.project_settings("settings.json")).unwrap();
    assert!(c.disabled());
}

#[test]
fn test_the_same_through_inspect() {
    let c = Case::new();
    c.diverging();
    c.settings_bytes(&utf16(&format!("{{{FLAG}}}"), true), "settings.json");
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_a_utf8_bom_is_stripped_in_every_file() {
    // only the BOM strip separates these from "invalid": a user file would be ignored, a repo
    // one refused
    let c = Case::new();
    let bom = b"\xef\xbb\xbf".as_slice();
    write(
        &c.config.join("settings.json"),
        [bom, format!("{{{FLAG}}}").as_bytes()].concat(),
    );
    assert!(c.disabled());
    fs::remove_file(c.config.join("settings.json")).unwrap();
    c.settings_bytes(
        &[bom, br#"{"other": 1}"#.as_slice()].concat(),
        "settings.json",
    );
    assert!(!c.disabled());
}

#[test]
fn test_the_users_own_files_are_decoded_the_same_way() {
    let c = Case::new();
    c.diverging();
    write(
        &c.config.join("settings.json"),
        utf16(&format!("{{{FLAG}}}"), true),
    );
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

// --- EnabledValueTest (I-2): only the literal `true` turns a plugin on

#[test]
fn test_a_non_true_entry_in_the_repo_settings_beats_a_user_true() {
    let c = Case::new();
    c.diverging();
    c.put("user", "true");
    for value in [
        "[]",
        r#"["0.1.0"]"#,
        r#""yes""#,
        r#""true""#,
        "null",
        "0",
        "1",
        "{}",
        r#"{"a": 1}"#,
        "false",
    ] {
        for place in ["repo", "local"] {
            c.put(place, value);
            assert!(c.disabled(), "{place} {value}");
            assert_eq!(
                c.inspect(),
                loc(Some(&c.live), Some(Problem::Disabled)),
                "{place} {value}"
            );
            let name = if place == "repo" {
                "settings.json"
            } else {
                "settings.local.json"
            };
            fs::remove_file(c.project_settings(name)).unwrap();
        }
    }
}

#[test]
fn test_a_non_true_entry_in_the_user_settings_disables_too() {
    let c = Case::new();
    c.diverging();
    for value in ["[]", r#"["0.1.0"]"#, r#""yes""#, "null", "false"] {
        c.put("user", value);
        assert!(c.disabled(), "{value}");
    }
}

#[test]
fn test_true_and_an_absent_entry_are_active() {
    let c = Case::new();
    c.diverging();
    assert!(!c.disabled());
    for place in ["user", "repo", "local"] {
        c.put(place, "true");
    }
    assert!(!c.disabled());
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
    write(
        &c.project_settings("settings.json"),
        r#"{"enabledPlugins": {"other@x": []}}"#,
    );
    write(
        &c.project_settings("settings.local.json"),
        r#"{"enabledPlugins": {}}"#,
    );
    assert!(!c.disabled());
}

#[test]
fn test_only_the_guardrails_plugin_key_matches() {
    // a similar-looking plugin name must not count: the match is the exact prefix
    let c = Case::new();
    c.diverging();
    for key in [
        "anthology-guardrails-extra@x",
        "my-anthology-guardrails@x",
        "anthology-guardrails",
        "ANTHOLOGY-GUARDRAILS@anthology",
    ] {
        write(
            &c.project_settings("settings.json"),
            format!(r#"{{"enabledPlugins": {{"{key}": false}}}}"#),
        );
        assert!(!c.disabled(), "{key}");
    }
}

#[test]
fn test_another_marketplace_entry_with_a_list_counts_the_same() {
    let c = Case::new();
    c.diverging();
    write(
        &c.project_settings("settings.json"),
        r#"{"enabledPlugins": {"anthology-guardrails@other": ["1"]}}"#,
    );
    assert!(c.disabled());
}

// --- UsersOwnFilesAreUncappedTest: the user's own files are read at any size

#[test]
fn test_a_huge_user_settings_file_that_disables_the_guard_is_honoured() {
    let c = Case::new();
    c.diverging();
    write(&c.config.join("settings.json"), disable_with_pad(BIG));
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_a_huge_user_settings_file_without_the_flag_does_not_disable() {
    let c = Case::new();
    c.diverging();
    write(&c.config.join("settings.json"), pad_only(BIG));
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_a_huge_installed_plugins_file_is_read() {
    let c = Case::new();
    c.diverging();
    write(
        &c.config.join("plugins/installed_plugins.json"),
        c.installed_text(None, "", BIG),
    );
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_a_huge_known_marketplaces_file_is_read() {
    let c = Case::new();
    c.diverging();
    let source = format!(
        r#"{{"source": "directory", "path": "{}"}}"#,
        c.market.display()
    );
    write(
        &c.config.join("plugins/known_marketplaces.json"),
        c.known_text(&source, None, BIG),
    );
    assert_eq!(c.inspect(), loc(Some(&c.live), None));
}

#[test]
fn test_a_non_regular_user_settings_file_fails_closed() {
    let c = Case::new();
    c.diverging();
    symlink("/dev/null", c.config.join("settings.json")).unwrap();
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
}

#[test]
fn test_the_live_marketplace_json_is_repo_controlled_and_capped() {
    let c = Case::new();
    c.diverging();
    write(
        &c.market.join(".claude-plugin/marketplace.json"),
        format!(
            r#"{{"name": "anthology", "pad": "{}", "plugins": [{{"name": "anthology-guardrails", "source": "./plugins/anthology-guardrails"}}]}}"#,
            "A".repeat(REPO_READ_LIMIT as usize + 10)
        ),
    );
    assert_eq!(c.inspect(), loc(None, Some(Problem::LiveMissing)));
}

// --- Beyond the Python tests (RT1a review): bounds the port must keep. Not ledger targets.

/// `market/plugins/s0 -> s1 -> … -> s<n-1> -> real-chain`, with the guard in `real-chain`, as
/// the directory marketplace's source `./plugins/s0`.
fn chained(c: &Case, n: usize) {
    let plugins = c.market.join("plugins");
    c.copy_guardrails(&plugins.join("real-chain"));
    for i in 0..n {
        let to = if i + 1 == n {
            "real-chain".to_string()
        } else {
            format!("s{}", i + 1)
        };
        symlink(to, plugins.join(format!("s{i}"))).unwrap();
    }
    c.copy_guardrails(&c.cache);
    c.manifest_json(r#""./plugins/s0""#);
    c.directory_known();
    c.installed();
}

#[test]
fn a_short_symlink_chain_in_a_directory_source_resolves() {
    let c = Case::new();
    chained(&c, 5);
    assert_eq!(
        c.inspect(),
        loc(Some(&c.market.join("plugins/real-chain")), None)
    );
}

#[test]
fn a_long_symlink_chain_in_a_directory_source_is_live_missing() {
    // Python's realpath recursion fails (about 990 links; caught: live_missing). Rust refuses
    // past REALPATH_MAX_DEPTH, and its stack stays bounded.
    let c = Case::new();
    chained(&c, 1100);
    assert_eq!(c.inspect(), loc(None, Some(Problem::LiveMissing)));
}

#[test]
fn a_very_long_symlink_chain_does_not_overflow_the_stack() {
    let c = Case::new();
    chained(&c, 20_000);
    let (env, tmp) = (c.env(), c.tmp.clone());
    // a small stack: the bound, not the stack size, ends the walk
    let got = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || guardrails_loc::inspect(&plugin(), &env, &tmp))
        .unwrap()
        .join()
        .expect("inspect overflowed its stack");
    assert_eq!(got, loc(None, Some(Problem::LiveMissing)));
}

fn nested(n: usize) -> String {
    format!("{}{}", "[".repeat(n), "]".repeat(n))
}

#[test]
fn a_deep_installed_plugins_file_is_invalid_past_pythons_limit() {
    // CPython 3.12's json.loads fails at 9,998 nested containers; _load reads that as invalid
    for (depth, want) in [(9_996, true), (9_997, false)] {
        let c = Case::new();
        c.copy_guardrails(&c.cache);
        let text = c.installed_text(None, "", 0);
        let text = format!(r#"{}, "pad": {}}}"#, &text[..text.len() - 1], nested(depth));
        write(&c.config.join("plugins/installed_plugins.json"), text);
        let found = c.inspect_from(&c.tmp.join("lone/anthology"));
        let expect = if want {
            loc(Some(&c.cache), None)
        } else {
            loc(None, Some(Problem::NotFound))
        };
        assert_eq!(found, expect, "pad depth {depth} (+1 for the top object)");
    }
}

#[test]
fn a_deep_known_marketplaces_file_is_invalid_past_pythons_limit() {
    // past the limit it is unreadable: the sibling fallback, not the directory marketplace
    let c = Case::new();
    let (root, sibling) = c.sibling_case();
    c.directory_known();
    let text = format!(
        r#"{{"anthology": {{"source": {{"source": "directory", "path": "{0}"}}, "installLocation": "{0}"}}, "pad": {1}}}"#,
        c.market.display(),
        nested(9_997)
    );
    write(&c.config.join("plugins/known_marketplaces.json"), text);
    assert_eq!(c.inspect_from(&root), loc(Some(&sibling), None));
}

#[test]
fn a_deep_user_settings_file_is_ignored_past_pythons_limit() {
    let c = Case::new();
    c.diverging();
    let flag = |depth: usize| {
        format!(
            r#"{{"enabledPlugins": {{"anthology-guardrails@anthology": false}}, "pad": {}}}"#,
            nested(depth)
        )
    };
    write(&c.config.join("settings.json"), flag(9_996));
    assert_eq!(c.inspect(), loc(Some(&c.live), Some(Problem::Disabled)));
    write(&c.config.join("settings.json"), flag(9_997));
    assert_eq!(c.inspect(), loc(Some(&c.live), None)); // malformed, as in Python: ignored
}

#[test]
fn a_settings_file_that_is_a_symlink_loop_counts_as_disabled() {
    for place in [
        ".claude/settings.json",
        ".claude/settings.local.json",
        "user",
    ] {
        let c = Case::new();
        c.diverging();
        let path = if place == "user" {
            c.config.join("settings.json")
        } else {
            c.tmp.join(place)
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(&path, &path).unwrap();
        assert_eq!(
            c.inspect(),
            loc(Some(&c.live), Some(Problem::Disabled)),
            "{place}"
        );
    }
}

#[test]
fn a_repeated_key_takes_its_last_value_like_python() {
    let c = Case::new();
    c.diverging();
    let k = r#""anthology-guardrails@anthology""#;
    for (text, disabled) in [
        (
            format!(r#"{{"enabledPlugins": {{{k}: false, {k}: true}}}}"#),
            false,
        ),
        (
            format!(r#"{{"enabledPlugins": {{{k}: true, {k}: false}}}}"#),
            true,
        ),
        (
            format!(r#"{{"enabledPlugins": {{{k}: false}}, "enabledPlugins": {{}}}}"#),
            false,
        ),
        (
            format!(r#"{{"enabledPlugins": {{}}, "enabledPlugins": {{{k}: []}}}}"#),
            true,
        ),
    ] {
        for name in ["settings.json", "settings.local.json"] {
            write(&c.project_settings(name), &text);
            assert_eq!(c.disabled(), disabled, "{name} {text}");
            fs::remove_file(c.project_settings(name)).unwrap();
        }
        write(&c.config.join("settings.json"), &text);
        assert_eq!(c.disabled(), disabled, "user {text}");
        fs::remove_file(c.config.join("settings.json")).unwrap();
    }
}
