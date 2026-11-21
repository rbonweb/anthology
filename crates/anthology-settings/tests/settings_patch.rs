//! Port of `tests/test_settings_patch.py`: reversible, byte-stable edits of the user's
//! settings.json. Each test keeps its Python method name (ledger: port/ledger/test_settings_patch.tsv).

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_guard::data::{PREFIX_VARIANTS, PROFILE_ROOTS as GUARD_PROFILE_ROOTS};
use anthology_pycompat::json::{self, DumpOpts, Map, Value};
use anthology_settings::*;
use proptest::prelude::*;

const PROFILE_ROOTS: [&str; 2] = ["Library/Application Support/Google", ".config/chromium"];
const BASE: &str = r#"{"model": "opus", "permissions": {"allow": ["Bash(ls)"], "deny": ["Read(.env)"]}, "env": {"A": "1"}}"#;

fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/anthology")
}

fn proc_env() -> Env {
    Env::from_process()
}

fn ops() -> Vec<Op> {
    setup_ops_with(&plugin_root(), &proc_env(), Some(&PROFILE_ROOTS)).unwrap()
}

fn v(text: &str) -> Value {
    json::loads(text).unwrap()
}

/// The `subTest` rows of a Python test (`tests/data/test_settings_patch.jsonl`, extracted by
/// `port/extract/rows.py`): each row's `$msg`, in row order.
fn rows(py_id: &str) -> Vec<Value> {
    let text = include_str!("data/test_settings_patch.jsonl");
    let id = format!("test_settings_patch.{py_id}");
    let mut out: Vec<(i64, Value)> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row = v(line);
        if at(&row, &["py"]).as_str() == Some(id.as_str()) {
            let n = at(&row, &["row"]).as_i64().unwrap();
            out.push((n, at(&row, &["params", "$msg"]).clone()));
        }
    }
    out.sort_by_key(|(n, _)| *n);
    assert!(!out.is_empty(), "no rows for {py_id}");
    out.into_iter().map(|(_, m)| m).collect()
}

/// The fixtures a test's rows name, in row order.
fn fixture_rows(py_id: &str) -> Vec<(String, String)> {
    let all = fixtures();
    rows(py_id)
        .iter()
        .map(|name| {
            let name = name.as_str().unwrap();
            let text = all.iter().find(|(n, _)| *n == name).unwrap().1.clone();
            (name.to_string(), text)
        })
        .collect()
}

/// `json.dumps(value, indent=<unit>, ensure_ascii=…)`.
fn dumps_indent(value: &Value, unit: &str, ensure_ascii: bool) -> String {
    let one = json::dumps(
        value,
        &DumpOpts {
            indent: Some(1),
            sort_keys: false,
            ensure_ascii,
            item_sep: ",",
            key_sep: ": ",
        },
    );
    one.split('\n')
        .map(|line| {
            let n = line.len() - line.trim_start_matches(' ').len();
            format!("{}{}", unit.repeat(n), &line[n..])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn dumps_sep(value: &Value, item_sep: &'static str, key_sep: &'static str) -> String {
    json::dumps(
        value,
        &DumpOpts {
            indent: None,
            sort_keys: false,
            ensure_ascii: true,
            item_sep,
            key_sep,
        },
    )
}

fn indent2(text: &str) -> String {
    dumps_indent(&v(text), "  ", true) + "\n"
}

fn make_plugin_root(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join("anthology");
    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    fs::write(
        root.join(".claude-plugin/plugin.json"),
        format!(r#"{{"name": "{name}"}}"#),
    )
    .unwrap();
    root
}

/// name -> settings text, the formats the property tests must keep byte-stable.
fn fixtures() -> Vec<(&'static str, String)> {
    let base = v(BASE);
    let base2 = dumps_indent(&base, "  ", true) + "\n";
    vec![
        ("two-space", base2.clone()),
        ("two-space-no-newline", dumps_indent(&base, "  ", true)),
        ("four-space", dumps_indent(&base, "    ", true) + "\n"),
        ("tab", dumps_indent(&base, "\t", true) + "\n"),
        ("compact", dumps_sep(&base, ",", ":") + "\n"),
        ("compact-spaces", dumps_sep(&base, ", ", ": ")),
        ("crlf", base2.replace('\n', "\r\n")),
        ("missing", String::new()),
        ("blank-newline", "\n".into()),
        ("empty-object", "{}".into()),
        ("empty-object-newline", "{}\n".into()),
        ("empty-object-spaced", "{\n}\n".into()),
        ("other-keys-only", indent2(r#"{"model": "x"}"#)),
        ("empty-permissions", indent2(r#"{"permissions": {}}"#)),
        (
            "empty-arrays",
            indent2(r#"{"permissions": {"allow": [], "deny": []}}"#),
        ),
        (
            "empty-arrays-compact",
            r#"{"permissions":{"allow":[],"deny":[]}}"#.into(),
        ),
        (
            "unicode",
            dumps_indent(
                &v(r#"{"note": "héllo ✓", "permissions": {"allow": ["Bash(echo é)"]}}"#),
                "  ",
                false,
            ) + "\n",
        ),
        ("empty-worktree", indent2(r#"{"worktree": {}}"#)),
        (
            "baseref-fresh",
            indent2(r#"{"worktree": {"baseRef": "fresh"}, "x": 1}"#),
        ),
        (
            "allow-one-item-compact",
            r#"{"permissions":{"allow":["Bash(ls)"]}}"#.into(),
        ),
        (
            "nested-values",
            indent2(r#"{"hooks": {"A": [{"x": [1, 2, {"y": null}]}]}, "z": 1.5e3}"#),
        ),
    ]
}

/// `data[k1][k2]…`.
fn at<'a>(data: &'a Value, keys: &[&str]) -> &'a Value {
    let mut cur = data;
    for k in keys {
        cur = cur.as_object().unwrap().get(k).unwrap();
    }
    cur
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn op_values(ops: &[Op], path: &[&str]) -> Vec<String> {
    ops.iter()
        .filter(|o| o.path() == path)
        .map(|o| o.value().as_str().unwrap().to_string())
        .collect()
}

// --- RoundTripTest -------------------------------------------------------------------------

#[test]
fn test_apply_then_revert_restores_the_original_bytes() {
    for (name, text) in
        fixture_rows("RoundTripTest.test_apply_then_revert_restores_the_original_bytes")
    {
        let planned = plan(&text, &ops()).unwrap();
        let (new, diff) = apply(&text, &planned.ops).unwrap();
        assert_ne!(new, text, "{name}");
        assert!(!diff.is_empty(), "{name}");
        let data = v(&new);
        assert!(strs(at(&data, &["permissions", "allow"])).contains(&"Bash(rtk *)".to_string()));
        assert_eq!(at(&data, &["worktree", "baseRef"]).as_str(), Some("head"));
        assert_eq!(revert(&new, &planned.stored).unwrap(), text, "{name}");
    }
}

#[test]
fn test_apply_twice_equals_apply_once() {
    for (name, text) in fixture_rows("RoundTripTest.test_apply_twice_equals_apply_once") {
        let (once, _) = apply(&text, &ops()).unwrap();
        let (twice, diff) = apply(&once, &ops()).unwrap();
        assert_eq!(twice, once, "{name}");
        assert_eq!(diff, "", "{name}");
        assert!(plan(&once, &ops()).unwrap().stored.is_empty(), "{name}");
    }
}

/// `difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes()` tags (CPython 3.12).
fn opcode_tags(a: &[char], b: &[char]) -> Vec<&'static str> {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (i, c) in b.iter().enumerate() {
        b2j.entry(*c).or_default().push(i);
    }
    let longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| {
        let (mut bi, mut bj, mut bs) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, c) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut next = HashMap::new();
            for &j in b2j.get(c).map(Vec::as_slice).unwrap_or(&[]) {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = j
                    .checked_sub(1)
                    .and_then(|p| j2len.get(&p))
                    .copied()
                    .unwrap_or(0)
                    + 1;
                next.insert(j, k);
                if k > bs {
                    (bi, bj, bs) = (i + 1 - k, j + 1 - k, k);
                }
            }
            j2len = next;
        }
        (bi, bj, bs)
    };
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest(alo, ahi, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();
    blocks.push((a.len(), b.len(), 0));
    let (mut i, mut j) = (0, 0);
    let mut tags = Vec::new();
    for (ai, bj, size) in blocks {
        if i < ai && j < bj {
            tags.push("replace");
        } else if i < ai {
            tags.push("delete");
        } else if j < bj {
            tags.push("insert");
        }
        (i, j) = (ai + size, bj + size);
        if size > 0 {
            tags.push("equal");
        }
    }
    tags
}

#[test]
fn test_everything_outside_the_edit_is_untouched() {
    // The rows leave out "missing", "blank-newline" and "baseref-fresh", as Python skips them.
    let rows = fixture_rows("RoundTripTest.test_everything_outside_the_edit_is_untouched");
    for skipped in ["missing", "blank-newline", "baseref-fresh"] {
        assert!(!rows.iter().any(|(n, _)| n == skipped));
    }
    for (name, text) in rows {
        let (new, _) = apply(&text, &ops()).unwrap();
        let a: Vec<char> = text.chars().collect();
        let b: Vec<char> = new.chars().collect();
        for tag in opcode_tags(&a, &b) {
            assert!(tag == "equal" || tag == "insert", "{name}: {tag}");
        }
    }
}

#[test]
fn test_inserted_lines_copy_the_files_style() {
    let base = v(BASE);
    let (two, _) = apply(&(dumps_indent(&base, "  ", true) + "\n"), &ops()).unwrap();
    assert!(two.contains("\n      \"Bash(ls)\",\n      \"Bash(rtk *)\""));
    let (four, _) = apply(&(dumps_indent(&base, "    ", true) + "\n"), &ops()).unwrap();
    assert!(four.contains("\n            \"Bash(ls)\",\n            \"Bash(rtk *)\""));
    let (tab, _) = apply(&(dumps_indent(&base, "\t", true) + "\n"), &ops()).unwrap();
    assert!(tab.contains("\n\t\t\t\"Bash(ls)\",\n\t\t\t\"Bash(rtk *)\""));
    let crlf_in = (dumps_indent(&base, "  ", true) + "\n").replace('\n', "\r\n");
    let (crlf, _) = apply(&crlf_in, &ops()).unwrap();
    // `assertNotRegex(crlf, r"(?<!\r)\n")`: every \n follows a \r.
    let b = crlf.as_bytes();
    assert!(
        b.iter()
            .enumerate()
            .all(|(i, &c)| c != b'\n' || (i > 0 && b[i - 1] == b'\r'))
    );
    let (compact, _) = apply(&dumps_sep(&base, ",", ":"), &ops()).unwrap();
    assert!(!compact.contains('\n'));
}

#[test]
fn test_missing_file_gets_a_formatted_document() {
    let (new, _) = apply("", &ops()).unwrap();
    assert!(new.ends_with("}\n"));
    assert!(new.contains("\n  \"permissions\": {"));
    assert!(
        json::loads(&new)
            .unwrap()
            .as_object()
            .unwrap()
            .get("worktree")
            == Some(&v(r#"{"baseRef": "head"}"#))
    );
}

// --- OpsTest -------------------------------------------------------------------------------

fn py_realpath_parent(p: &Path) -> String {
    let real = fs::canonicalize(p).unwrap();
    real.parent().unwrap().to_string_lossy().into_owned()
}

#[test]
fn test_setup_ops_contents() {
    let planned = plan("", &ops()).unwrap();
    let (text, _) = apply("", &planned.ops).unwrap();
    let data = v(&text);
    let allow = strs(at(&data, &["permissions", "allow"]));
    let deny = strs(at(&data, &["permissions", "deny"]));
    assert!(allow.contains(&"Bash(rtk *)".to_string()));
    assert!(allow.contains(&ALLOW_MEM_SAVE.to_string()));
    assert!(ALLOW_MEM_SAVE.starts_with("Bash(anthology-mem save"));
    for root in PROFILE_ROOTS {
        assert!(deny.contains(&format!("Read(~/{root})")));
        assert!(deny.contains(&format!("Read(~/{root}/**)")));
    }
    let parent = py_realpath_parent(&plugin_root());
    let rule = format!("Read(//{}/**)", parent.trim_start_matches('/'));
    assert!(allow.contains(&rule));
    assert_eq!(plugin_read_rule(&plugin_root()), rule);
    assert_eq!(at(&data, &["worktree", "baseRef"]).as_str(), Some("head"));
}

#[test]
fn test_default_profile_roots_come_from_the_guardrails_plugin() {
    // The guard's data (I-2) is the single source; port/difftest/oracle/data.py checks it
    // equals guard_rules.PROFILE_ROOTS and _PREFIX_VARIANTS.
    let deny = op_values(
        &setup_ops(&plugin_root(), &proc_env()).unwrap(),
        &["permissions", "deny"],
    );
    let mut expected = Vec::new();
    for root in GUARD_PROFILE_ROOTS {
        let vs: &[&str] = if root.starts_with(".config/") {
            &PREFIX_VARIANTS
        } else {
            &[""]
        };
        for var in vs {
            expected.push(format!("Read(~/{root}{var})"));
            expected.push(format!("Read(~/{root}{var}/**)"));
        }
    }
    assert_eq!(deny, expected);
    assert!(deny.contains(&format!("Read(~/{})", GUARD_PROFILE_ROOTS[0])));
}

#[test]
fn test_deny_rules_cover_the_guards_linux_channel_variants() {
    let deny = op_values(
        &setup_ops(&plugin_root(), &proc_env()).unwrap(),
        &["permissions", "deny"],
    );
    for rule in [
        "Read(~/.config/google-chrome-beta)",
        "Read(~/.config/google-chrome-unstable/**)",
        "Read(~/.config/chromium-dev)",
        "Read(~/.config/chromium-canary/**)",
    ] {
        assert!(deny.contains(&rule.to_string()), "{rule}");
    }
    assert!(!deny.contains(&"Read(~/Library/Application Support/Google-beta)".to_string()));
}

#[test]
fn test_injected_profile_roots_get_no_variants() {
    let ops = setup_ops_with(&plugin_root(), &proc_env(), Some(&[".config/x"])).unwrap();
    assert_eq!(
        op_values(&ops, &["permissions", "deny"]),
        vec!["Read(~/.config/x)", "Read(~/.config/x/**)"]
    );
}

#[test]
fn test_ops_are_json_serialisable() {
    let planned = plan(&dumps_sep(&v(BASE), ", ", ": "), &ops()).unwrap();
    let text = json::dumps(&Value::Array(planned.stored.clone()), &json::PY_DEFAULT);
    let back = json::loads(&text).unwrap();
    assert_eq!(back.as_array().unwrap(), &planned.stored);
    // And each record is still the op it came from.
    for (op, rec) in planned.ops.iter().zip(&planned.stored) {
        assert_eq!(Op::from_value(rec).as_ref(), Some(op));
    }
}

#[test]
fn test_users_own_rule_is_never_ours() {
    let text = indent2(r#"{"permissions": {"allow": ["Bash(rtk *)", "Bash(ls)"]}}"#);
    let planned = plan(&text, &ops()).unwrap();
    assert!(
        !planned
            .ops
            .iter()
            .any(|o| o.value().as_str() == Some("Bash(rtk *)"))
    );
    let (new, _) = apply(&text, &planned.ops).unwrap();
    let back = revert(&new, &planned.stored).unwrap();
    assert_eq!(back, text);
    assert!(strs(at(&v(&back), &["permissions", "allow"])).contains(&"Bash(rtk *)".to_string()));
}

fn base_ref_record(planned: &Planned) -> Value {
    planned
        .stored
        .iter()
        .find(|o| at(o, &["path"]) == &v(r#"["worktree", "baseRef"]"#))
        .unwrap()
        .clone()
}

#[test]
fn test_base_ref_previous_value_is_remembered_and_restored() {
    let text = indent2(r#"{"worktree": {"baseRef": "fresh"}}"#);
    let planned = plan(&text, &ops()).unwrap();
    assert_eq!(
        at(&base_ref_record(&planned), &["previous"]),
        &v(r#"{"present": true, "raw": "\"fresh\""}"#)
    );
    let (new, _) = apply(&text, &planned.ops).unwrap();
    assert_eq!(
        at(&v(&new), &["worktree", "baseRef"]).as_str(),
        Some("head")
    );
    let back = revert(&new, &planned.stored).unwrap();
    assert_eq!(
        at(&v(&back), &["worktree", "baseRef"]).as_str(),
        Some("fresh")
    );
}

#[test]
fn test_base_ref_changed_by_the_user_is_left_alone_on_revert() {
    let text = indent2(r#"{"worktree": {"baseRef": "fresh"}}"#);
    let planned = plan(&text, &ops()).unwrap();
    let (new, _) = apply(&text, &planned.ops).unwrap();
    let new = new.replace("\"head\"", "\"fresh-ish\"");
    let back = revert(&new, &planned.stored).unwrap();
    assert_eq!(
        at(&v(&back), &["worktree", "baseRef"]).as_str(),
        Some("fresh-ish")
    );
}

#[test]
fn test_user_edits_between_apply_and_revert_survive() {
    let text = indent2(BASE);
    let planned = plan(&text, &ops()).unwrap();
    let (new, _) = apply(&text, &planned.ops).unwrap();
    let mut data = v(&new);
    {
        let m = data.as_object_mut().unwrap();
        m.insert("model".into(), Value::Str("sonnet".into()));
        let perms = m.get_mut("permissions").unwrap().as_object_mut().unwrap();
        let allow = perms.get_mut("allow").unwrap().as_array_mut().unwrap();
        allow.push(Value::Str("Bash(make *)".into()));
        allow.insert(0, Value::Str("Bash(pwd)".into()));
        m.insert("extra".into(), v(r#"{"k": [1]}"#));
    }
    let edited = dumps_indent(&data, "  ", true) + "\n";
    let got = v(&revert(&edited, &planned.stored).unwrap());
    assert_eq!(at(&got, &["model"]).as_str(), Some("sonnet"));
    assert_eq!(
        strs(at(&got, &["permissions", "allow"])),
        vec!["Bash(pwd)", "Bash(ls)", "Bash(make *)"]
    );
    assert_eq!(strs(at(&got, &["permissions", "deny"])), vec!["Read(.env)"]);
    assert_eq!(at(&got, &["extra"]), &v(r#"{"k": [1]}"#));
    assert!(got.as_object().unwrap().get("worktree").is_none());
}

#[test]
fn test_created_containers_survive_when_the_user_added_to_them() {
    let planned = plan("", &ops()).unwrap();
    let (new, _) = apply("", &planned.ops).unwrap();
    let mut data = v(&new);
    data.as_object_mut()
        .unwrap()
        .get_mut("permissions")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("ask".into(), v(r#"["Bash(git push *)"]"#));
    let back = revert(&(dumps_indent(&data, "  ", true) + "\n"), &planned.stored).unwrap();
    assert_eq!(
        v(&back),
        v(r#"{"permissions": {"ask": ["Bash(git push *)"]}}"#)
    );
}

#[test]
fn test_revert_of_a_text_without_our_entries_changes_nothing() {
    let text = indent2(BASE);
    let planned = plan(&text, &ops()).unwrap();
    let (back, diff) = revert_with_diff(&text, &planned.stored).unwrap();
    assert_eq!(back, text);
    assert_eq!(diff, "");
}

// --- PluginRootGuardTest -------------------------------------------------------------------

struct Guard {
    _tmp: tempfile::TempDir,
    tmp: PathBuf,
    home: PathBuf,
    env: Env,
}

fn guard() -> Guard {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp = fs::canonicalize(tmp_dir.path()).unwrap();
    let home = tmp.join("home");
    fs::create_dir(&home).unwrap();
    let env = Env::default().with_os("HOME", &home);
    Guard {
        _tmp: tmp_dir,
        tmp,
        home,
        env,
    }
}

fn refuses(g: &Guard, root: &Path, fragment: &str) {
    let err = setup_ops_with(root, &g.env, Some(&PROFILE_ROOTS)).unwrap_err();
    assert!(err.contains(fragment), "{err:?} lacks {fragment:?}");
}

#[test]
fn test_a_proper_root_is_accepted() {
    let g = guard();
    let root = make_plugin_root(&g.tmp.join("plugins"), "anthology");
    let ops = setup_ops_with(&root, &g.env, Some(&PROFILE_ROOTS)).unwrap();
    let rule = plugin_read_rule(&root);
    assert!(
        ops.iter()
            .any(|o| o.value().as_str() == Some(rule.as_str()))
    );
}

#[test]
fn test_empty_relative_missing_and_non_folder_roots() {
    let g = guard();
    refuses(&g, Path::new(""), "absolute");
    refuses(&g, Path::new("plugins/anthology"), "absolute");
    refuses(&g, &g.tmp.join("missing/anthology"), "not a folder");
    fs::write(g.tmp.join("file"), "x").unwrap();
    refuses(&g, &g.tmp.join("file"), "not a folder");
    // The exact Python texts.
    assert_eq!(
        validate_plugin_root(Path::new("plugins/anthology"), &g.env).unwrap_err(),
        "the plugin root must be an absolute path (got 'plugins/anthology')"
    );
    assert_eq!(
        validate_plugin_root(&g.tmp.join("missing/anthology"), &g.env).unwrap_err(),
        format!(
            "the plugin root {}/missing/anthology is not a folder",
            g.tmp.display()
        )
    );
}

#[test]
fn test_root_without_an_anthology_manifest() {
    let g = guard();
    let bare = g.tmp.join("plugins/anthology");
    fs::create_dir_all(&bare).unwrap();
    refuses(&g, &bare, "not the anthology plugin");
    refuses(
        &g,
        &make_plugin_root(&g.tmp.join("other"), "something-else"),
        "not the anthology plugin",
    );
}

#[test]
fn test_filesystem_root_and_top_level_folders() {
    let g = guard();
    refuses(&g, Path::new("/"), "not");
    let root = make_plugin_root(&g.tmp, "anthology"); // the parent is an ancestor of HOME
    refuses(&g, &root, "home folder");
}

#[test]
fn test_parent_that_is_the_home_folder() {
    let g = guard();
    let root = make_plugin_root(&g.home, "anthology");
    refuses(&g, &root, "home folder");
}

#[test]
fn test_parent_that_is_an_ancestor_of_home() {
    let g = guard();
    let deep_home = g.tmp.join("a/b/home");
    fs::create_dir_all(&deep_home).unwrap();
    let root = make_plugin_root(&g.tmp.join("a"), "anthology");
    let env = Env::default().with_os("HOME", &deep_home);
    assert!(setup_ops_with(&root, &env, Some(&PROFILE_ROOTS)).is_err());
}

#[test]
fn test_glob_and_rule_metacharacters_in_the_path() {
    let g = guard();
    for ch in ["*", "?", "[", "]", "{", "}", "(", ")", "\\"] {
        let parent = g.tmp.join(format!("plug{ch}ins"));
        if fs::create_dir(&parent).is_err() {
            continue;
        }
        refuses(&g, &make_plugin_root(&parent, "anthology"), "pattern");
    }
}

#[test]
fn test_a_symlinked_root_is_judged_by_its_real_location() {
    let g = guard();
    let real = make_plugin_root(&g.home, "anthology"); // an unsafe place
    let link = g.tmp.join("safe/anthology");
    fs::create_dir(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    refuses(&g, &link, "home folder");
}

// --- PluginRuleStatusTest ------------------------------------------------------------------

struct Status {
    _tmp: tempfile::TempDir,
    env: Env,
    path: PathBuf,
    rule: String,
}

fn status_setup() -> Status {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp = fs::canonicalize(tmp_dir.path()).unwrap();
    fs::create_dir(tmp.join("c")).unwrap();
    let env = Env::default()
        .with_os("HOME", &tmp)
        .with_os("CLAUDE_CONFIG_DIR", tmp.join("c"));
    Status {
        path: tmp.join("c/settings.json"),
        _tmp: tmp_dir,
        env,
        rule: plugin_read_rule(&plugin_root()),
    }
}

impl Status {
    fn status(&self) -> &'static str {
        plugin_rule_status(&self.env, &plugin_root())
    }

    fn allow_json(&self, allow: &Value) -> String {
        let mut perms = Map::new();
        perms.insert("allow".into(), allow.clone());
        let mut top = Map::new();
        top.insert("permissions".into(), Value::Object(perms));
        json::dumps(&Value::Object(top), &json::PY_DEFAULT)
    }
}

#[test]
fn test_states() {
    let s = status_setup();
    assert_eq!(s.status(), "missing");
    fs::write(
        &s.path,
        s.allow_json(&Value::Array(vec![Value::Str(s.rule.clone())])),
    )
    .unwrap();
    assert_eq!(s.status(), "ok");
    fs::write(&s.path, s.allow_json(&v(r#"["Read(//old/**)"]"#))).unwrap();
    assert_eq!(s.status(), "missing");
    fs::write(&s.path, "{nope").unwrap();
    assert_eq!(s.status(), "unreadable");
}

#[test]
fn test_bom_is_stripped() {
    let s = status_setup();
    let text = format!(
        "\u{feff}{}",
        s.allow_json(&Value::Array(vec![Value::Str(s.rule.clone())]))
    );
    fs::write(&s.path, text).unwrap();
    assert_eq!(s.status(), "ok");
}

#[test]
fn test_allow_must_be_a_list_not_a_string() {
    let s = status_setup();
    fs::write(
        &s.path,
        s.allow_json(&Value::Str(format!("xx{}yy", s.rule))),
    )
    .unwrap();
    assert_eq!(s.status(), "missing");
}

#[test]
fn test_a_pathologically_deep_file_is_unreadable_not_a_crash() {
    let s = status_setup();
    fs::write(&s.path, "[".repeat(100_000)).unwrap();
    assert_eq!(s.status(), "unreadable");
}

// --- StyleTest -----------------------------------------------------------------------------

#[test]
fn test_inline_one_item_array_keeps_the_files_comma_space_style() {
    let text = r#"{"permissions": {"allow": ["Bash(ls)"]}}"#;
    let (new, _) = apply(text, &ops()).unwrap();
    assert!(new.contains(r#"["Bash(ls)", "Bash(rtk *)", "#));
    assert!(new.contains(r#"}, "worktree""#));
    let planned = plan(text, &ops()).unwrap();
    assert_eq!(revert(&new, &planned.stored).unwrap(), text);
}

#[test]
fn test_diff_has_no_context_lines() {
    let text =
        indent2(r#"{"env": {"SECRET_TOKEN": "hunter2"}, "permissions": {"allow": ["Bash(ls)"]}}"#);
    let (_, diff) = apply(&text, &ops()).unwrap();
    assert!(!diff.contains("hunter2"));
    assert!(!diff.contains("SECRET_TOKEN"));
    assert!(diff.contains("+      \"Bash(rtk *)\""));
}

// --- ReconcileTest -------------------------------------------------------------------------

#[test]
fn test_plugin_moved_replaces_only_the_stale_rule_we_added() {
    let text = indent2(r#"{"permissions": {"allow": ["Read(//somewhere/else/**)"]}}"#);
    let tmp = tempfile::tempdir().unwrap();
    let old_root = make_plugin_root(
        &fs::canonicalize(tmp.path()).unwrap().join("plugins"),
        "anthology",
    );
    let first_ops = setup_ops_with(&old_root, &proc_env(), Some(&PROFILE_ROOTS)).unwrap();
    let (t1, _, stored1) = reconcile(&text, &first_ops, &[]).unwrap();
    assert!(strs(at(&v(&t1), &["permissions", "allow"])).contains(&plugin_read_rule(&old_root)));
    let (t2, diff, stored2) = reconcile(&t1, &ops(), &stored1).unwrap();
    let allow = strs(at(&v(&t2), &["permissions", "allow"]));
    assert!(!allow.contains(&plugin_read_rule(&old_root)));
    assert!(allow.contains(&plugin_read_rule(&plugin_root())));
    assert!(allow.contains(&"Read(//somewhere/else/**)".to_string()));
    assert!(!diff.is_empty());
    assert_eq!(revert(&t2, &stored2).unwrap(), text);
}

#[test]
fn test_changed_mem_save_rule_replaces_only_the_one_we_added() {
    let text = indent2(BASE);
    let old_ops: Vec<Op> = ops()
        .into_iter()
        .map(|o| {
            if o.tag() == Some(TAG_MEM_SAVE) {
                o.with_value(Value::Str("Bash(anthology-mem save:*)".into()))
            } else {
                o
            }
        })
        .collect();
    let (t1, _, stored1) = reconcile(&text, &old_ops, &[]).unwrap();
    assert!(
        strs(at(&v(&t1), &["permissions", "allow"]))
            .contains(&"Bash(anthology-mem save:*)".to_string())
    );
    let (t2, _, stored2) = reconcile(&t1, &ops(), &stored1).unwrap();
    let allow = strs(at(&v(&t2), &["permissions", "allow"]));
    assert!(!allow.contains(&"Bash(anthology-mem save:*)".to_string()));
    assert!(allow.contains(&ALLOW_MEM_SAVE.to_string()));
    assert_eq!(revert(&t2, &stored2).unwrap(), text);
}

#[test]
fn test_rerun_keeps_the_original_ops_so_remove_still_works() {
    let text = indent2(BASE);
    let (t1, _, stored1) = reconcile(&text, &ops(), &[]).unwrap();
    let (t2, diff, stored2) = reconcile(&t1, &ops(), &stored1).unwrap();
    assert_eq!(t2, t1);
    assert_eq!(diff, "");
    assert_eq!(stored2, stored1);
    assert_eq!(revert(&t2, &stored2).unwrap(), text);
}

#[test]
fn test_rule_the_user_deleted_is_added_again_without_duplicating_the_record() {
    let text = indent2(BASE);
    let (t1, _, stored1) = reconcile(&text, &ops(), &[]).unwrap();
    let mut data = v(&t1);
    data.as_object_mut()
        .unwrap()
        .get_mut("permissions")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .get_mut("allow")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .retain(|x| x.as_str() != Some("Bash(rtk *)"));
    let (t2, _, stored2) =
        reconcile(&(dumps_indent(&data, "  ", true) + "\n"), &ops(), &stored1).unwrap();
    assert!(strs(at(&v(&t2), &["permissions", "allow"])).contains(&"Bash(rtk *)".to_string()));
    let keys: Vec<String> = stored2
        .iter()
        .map(|o| {
            format!(
                "{}|{}|{}",
                json::dumps(at(o, &["op"]), &json::PY_DEFAULT),
                json::dumps(at(o, &["path"]), &json::PY_DEFAULT),
                json::dumps(at(o, &["value"]), &json::PY_DEFAULT)
            )
        })
        .collect();
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(keys.len(), unique.len());
}

// --- SubagentModelOpTest -------------------------------------------------------------------

const SUBAGENT_KEY: &str = "CLAUDE_CODE_SUBAGENT_MODEL";

/// `SubagentModelOpTest.texts()`: name -> settings text for the opt-in `unset` op.
fn subagent_texts() -> Vec<(&'static str, String)> {
    let with_other = format!(
        r#"{{"model": "x", "env": {{"A": "1", "{SUBAGENT_KEY}": "SECRETMODEL"}}, "permissions": {{"allow": []}}}}"#
    );
    let alone = format!(r#"{{"env": {{"{SUBAGENT_KEY}": "SECRETMODEL"}}, "z": 1}}"#);
    let first_of_two = format!(r#"{{"env": {{"{SUBAGENT_KEY}": "SECRETMODEL", "B": "2"}}}}"#);
    vec![
        ("two-space", indent2(&with_other)),
        (
            "four-space-alone",
            dumps_indent(&v(&alone), "    ", true) + "\n",
        ),
        ("compact", dumps_sep(&v(&with_other), ",", ":")),
        ("crlf", indent2(&alone).replace('\n', "\r\n")),
        ("first-of-two", indent2(&first_of_two)),
    ]
}

fn subagent_text(name: &str) -> String {
    subagent_texts()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

/// The object without `key` (Python's `{k: v for k, v in data.items() if k != key}`).
fn without_key(data: &Value, key: &str) -> Map {
    let mut m = data.as_object().unwrap().clone();
    m.remove(key);
    m
}

// Same name as RoundTripTest's test above, so class-prefixed (ledger: one Rust name per id).
#[test]
fn subagent_model_op_test_apply_then_revert_restores_the_original_bytes() {
    let op = subagent_model_op();
    for (name, text) in subagent_texts() {
        let planned = plan(&text, std::slice::from_ref(&op)).unwrap();
        assert_eq!(planned.stored.len(), 1, "{name}");
        assert_eq!(
            at(&planned.stored[0], &["previous"]),
            &v(r#"{"present": true, "raw": "\"SECRETMODEL\""}"#),
            "{name}"
        );
        let (new, _) = apply(&text, &planned.ops).unwrap();
        let data = v(&new);
        assert!(
            !at(&data, &["env"])
                .as_object()
                .unwrap()
                .contains_key(SUBAGENT_KEY),
            "{name}"
        );
        assert_eq!(
            without_key(&data, "env"),
            without_key(&v(&text), "env"),
            "{name}"
        );
        assert!(
            plan(&new, std::slice::from_ref(&op))
                .unwrap()
                .stored
                .is_empty(),
            "{name}"
        );
        let back = revert(&new, &planned.stored).unwrap();
        if name == "first-of-two" {
            // put back as the env block's last key
            let restored = v(&back);
            assert_eq!(restored, v(&text), "{name}");
            let keys: Vec<&str> = at(&restored, &["env"])
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, _)| k)
                .collect();
            assert_eq!(keys, ["B", SUBAGENT_KEY], "{name}");
        } else {
            assert_eq!(back, text, "{name}");
        }
    }
}

#[test]
fn test_absent_or_blank_file_plans_nothing() {
    let op = subagent_model_op();
    for text in ["", "{}", r#"{"env": {"A": "1"}}"#, r#"{"model": "x"}"#] {
        assert!(
            plan(text, std::slice::from_ref(&op))
                .unwrap()
                .stored
                .is_empty(),
            "{text}"
        );
    }
}

#[test]
fn test_the_diff_shows_the_key_but_never_the_value() {
    let (_, diff) = apply(&subagent_text("two-space"), &[subagent_model_op()]).unwrap();
    assert!(diff.contains(r#"-    "CLAUDE_CODE_SUBAGENT_MODEL": "<redacted>""#));
    assert!(!diff.contains("SECRETMODEL"));
}

#[test]
fn test_a_value_set_again_since_is_left_alone_on_revert() {
    let text = subagent_text("two-space");
    let planned = plan(&text, &[subagent_model_op()]).unwrap();
    let (new, _) = apply(&text, &planned.ops).unwrap();
    let mut data = v(&new);
    data.as_object_mut()
        .unwrap()
        .get_mut("env")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(SUBAGENT_KEY.into(), Value::Str("mine-now".into()));
    let edited = dumps_indent(&data, "  ", true) + "\n";
    let (back, diff) = revert_with_diff(&edited, &planned.stored).unwrap();
    assert_eq!(back, edited);
    assert_eq!(diff, "");
}

#[test]
fn test_restored_when_the_env_block_went_away() {
    let text = subagent_text("two-space");
    let planned = plan(&text, &[subagent_model_op()]).unwrap();
    let (new, _) = apply(&text, &planned.ops).unwrap();
    let mut data = v(&new);
    data.as_object_mut().unwrap().remove("env");
    let (back, _) =
        revert_with_diff(&(dumps_indent(&data, "  ", true) + "\n"), &planned.stored).unwrap();
    assert_eq!(
        at(&v(&back), &["env"]),
        &v(&format!(r#"{{"{SUBAGENT_KEY}": "SECRETMODEL"}}"#))
    );
}

#[test]
fn test_reverted_together_with_the_setup_ops() {
    let text = subagent_text("two-space");
    let mut desired = ops();
    desired.push(subagent_model_op());
    let (new, _, stored) = reconcile(&text, &desired, &[]).unwrap();
    assert!(
        !at(&v(&new), &["env"])
            .as_object()
            .unwrap()
            .contains_key(SUBAGENT_KEY)
    );
    // a later run without the opt-in keeps the record, so remove still restores it
    let (again, _, stored2) = reconcile(&new, &ops(), &stored).unwrap();
    assert_eq!(again, new);
    assert_eq!(stored2, stored);
    assert_eq!(revert(&again, &stored2).unwrap(), text);
}

#[test]
fn test_a_key_set_twice_or_a_non_object_parent_plans_nothing() {
    let op = subagent_model_op();
    let twice = format!(r#"{{"env": {{"{SUBAGENT_KEY}": "a", "A": "1", "{SUBAGENT_KEY}": "b"}}}}"#);
    assert!(
        plan(&twice, std::slice::from_ref(&op))
            .unwrap()
            .stored
            .is_empty()
    );
    assert_eq!(
        apply(&twice, std::slice::from_ref(&op)).unwrap(),
        (twice.clone(), String::new())
    );
    for text in [r#"{"env": "x"}"#, r#"{"env": [1]}"#] {
        assert!(
            plan(text, std::slice::from_ref(&op))
                .unwrap()
                .stored
                .is_empty(),
            "{text}"
        );
    }
    assert!(plan("{nope", std::slice::from_ref(&op)).is_err());
}

#[test]
fn test_a_broken_record_is_skipped_on_revert() {
    let text = indent2(r#"{"env": {"A": "1"}}"#);
    let path = format!(r#"["env", "{SUBAGENT_KEY}"]"#);
    let records = [
        format!(r#"{{"op": "unset", "path": {path}}}"#),
        format!(r#"{{"op": "unset", "path": {path}, "previous": "x"}}"#),
        format!(r#"{{"op": "unset", "path": {path}, "previous": {{"present": true, "raw": 3}}}}"#),
        format!(
            r#"{{"op": "unset", "path": {path}, "previous": {{"present": true, "raw": "not json"}}}}"#
        ),
        format!(
            r#"{{"op": "unset", "path": {path}, "previous": {{"present": false, "raw": "\"m\""}}}}"#
        ),
        r#"{"op": "unset", "path": ["env", 1], "previous": {"present": true, "raw": "\"m\""}}"#
            .to_string(),
        r#"{"op": "unset", "path": [], "previous": {"present": true, "raw": "\"m\""}}"#.to_string(),
    ];
    for rec in records {
        let stored = [v(&rec)];
        assert_eq!(
            revert_with_diff(&text, &stored).unwrap(),
            (text.clone(), String::new()),
            "{rec}"
        );
    }
    assert!(
        plan(&text, &[Op::Unset { path: vec![] }])
            .unwrap()
            .stored
            .is_empty()
    );
}

#[test]
fn test_not_part_of_the_setup_ops_or_their_version() {
    assert!(!ops().iter().any(|o| o.kind() == "unset"));
    assert_eq!(
        subagent_model_op().to_value(),
        v(&format!(
            r#"{{"op": "unset", "path": ["env", "{SUBAGENT_KEY}"]}}"#
        ))
    );
}

// --- InvalidInputTest ----------------------------------------------------------------------

#[test]
fn test_unparseable_or_non_object_text_is_refused() {
    // Python raises ValueError (the class its callers catch); these are its texts.
    let expected = [
        "settings.json is not valid JSON: Expecting property name enclosed in double quotes: line 1 column 9 (char 8)",
        "settings.json is not valid JSON: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
        "settings.json is not a JSON object",
        "settings.json is not a JSON object",
        "settings.json is not a JSON object",
        "settings.json is not valid JSON: Extra data: line 1 column 10 (char 9)",
        "settings.json is not valid JSON: Expecting value: line 1 column 1 (char 0)",
    ];
    let stored: Vec<Value> = ops().iter().map(Op::to_value).collect();
    let rows = rows("InvalidInputTest.test_unparseable_or_non_object_text_is_refused");
    assert_eq!(rows.len(), expected.len());
    for (row, want) in rows.iter().zip(expected) {
        let text = row.as_str().unwrap();
        assert_eq!(apply(text, &ops()).unwrap_err(), want, "{text}");
        assert_eq!(revert(text, &stored).unwrap_err(), want, "{text}");
        assert_eq!(plan(text, &ops()).unwrap_err(), want, "{text}");
    }
}

#[test]
fn test_wrong_container_types_are_refused() {
    let expected = [
        "permissions is not an object",
        "permissions.allow is not a list",
        "worktree is not an object",
        "permissions.deny is not a list",
    ];
    let rows = rows("InvalidInputTest.test_wrong_container_types_are_refused");
    assert_eq!(rows.len(), expected.len());
    for (data, want) in rows.iter().zip(expected) {
        let text = dumps_sep(data, ", ", ": ");
        assert_eq!(apply(&text, &ops()).unwrap_err(), want, "{text}");
    }
}

#[test]
fn test_utf8_bom_is_kept() {
    let text = format!("\u{feff}{}", indent2(BASE));
    let planned = plan(&text, &ops()).unwrap();
    let (new, _) = apply(&text, &planned.ops).unwrap();
    assert!(new.starts_with("\u{feff}{"));
    assert_eq!(revert(&new, &planned.stored).unwrap(), text);
}

// --- DiffTest ------------------------------------------------------------------------------

#[test]
fn test_diff_is_a_unified_diff_of_the_change() {
    let (_, diff) = apply(&indent2(BASE), &ops()).unwrap();
    assert!(diff.contains("--- settings.json"));
    assert!(diff.contains("+++ settings.json"));
    assert!(diff.contains("+      \"Bash(rtk *)\""));
}

// --- RedactTest ----------------------------------------------------------------------------

const SECRETS: [&str; 6] = [
    "hunter2",
    "sk-live-123",
    "s3cr3t-cookie",
    "Bearer abc.def",
    "XYZSECRET",
    "opus",
];

/// The setup diff and the remove diff of `text`; both carry the change, no secret and the
/// expected lines.
fn check(text: &str, diff_lines: &[&str], undo_lines: &[&str]) -> (String, String) {
    let planned = plan(text, &ops()).unwrap();
    let (new, diff) = apply(text, &planned.ops).unwrap();
    let (back, undo) = revert_with_diff(&new, &planned.stored).unwrap();
    assert_eq!(back, text);
    for (shown, lines) in [(&diff, diff_lines), (&undo, undo_lines)] {
        assert!(shown.contains("Bash(rtk *)"));
        assert!(shown.contains("baseRef"));
        for secret in SECRETS {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        let got: Vec<&str> = shown.split('\n').collect();
        for line in lines {
            assert!(got.contains(line), "{line:?} not in {shown}");
        }
    }
    (diff, undo)
}

#[test]
fn test_minified_file() {
    let text = dumps_sep(
        &v(r#"{"env": {"API_TOKEN": "hunter2", "PLAIN": "sk-live-123"},
               "model": "opus",
               "permissions": {"allow": ["Bash(ls)"]},
               "mcp": {"headers": {"Authorization": "Bearer abc.def"}},
               "hooks": {"Stop": [{"command": "curl https://hooks.example/XYZSECRET"}]},
               "proxyUrl": "http://u:s3cr3t-cookie@proxy"}"#),
        ",",
        ":",
    );
    let (diff, undo) = check(&text, &[], &[]);
    for shown in [diff, undo] {
        for part in [
            r#""API_TOKEN":"<redacted>""#,
            r#""PLAIN":"<redacted>""#,
            r#""model":"<redacted>""#,
            r#""Authorization":"<redacted>""#,
            r#""command":"<redacted>""#,
            r#""proxyUrl":"<redacted>""#,
            r#""Bash(ls)","Bash(rtk *)""#,
        ] {
            assert!(shown.contains(part), "{part}");
        }
    }
}

#[test]
fn test_one_line_env_as_the_last_key() {
    let text = "{\n  \"model\": \"opus\",\n  \"permissions\": {\"allow\": [\"Bash(ls)\"]},\n  \"env\": {\"API_TOKEN\": \"hunter2\", \"PLAIN\": \"sk-live-123\", \"N\": 3}\n}\n";
    let env = r#"  "env": {"API_TOKEN": "<redacted>", "PLAIN": "<redacted>", "N": "<redacted>"}"#;
    check(
        text,
        &[&format!("-{env}"), &format!("+{env},")],
        &[&format!("-{env},"), &format!("+{env}")],
    );
}

#[test]
fn test_pretty_printed_file() {
    let text = indent2(
        r#"{"permissions": {"allow": ["Bash(ls)"]},
            "awsAuthRefresh": "login --password hunter2",
            "servers": [{"name": "x", "cookie": "s3cr3t-cookie", "env": {"K": ["sk-live-123"]}}],
            "env": {"SECRET": "hunter2"}}"#,
    );
    check(&text, &["+  },"], &["-  },"]);
    let red = redact(&text).unwrap();
    assert!(
        red.split('\n')
            .any(|l| l == r#"    "SECRET": "<redacted>""#)
    );
}

#[test]
fn test_pretty_printed_file_with_a_one_line_secret_as_the_last_key() {
    let text = indent2(
        r#"{"permissions": {"allow": ["Bash(ls)"]}, "apiKeyHelper": "/bin/echo sk-live-123"}"#,
    );
    check(
        &text,
        &[
            r#"-  "apiKeyHelper": "<redacted>""#,
            r#"+  "apiKeyHelper": "<redacted>","#,
        ],
        &[
            r#"-  "apiKeyHelper": "<redacted>","#,
            r#"+  "apiKeyHelper": "<redacted>""#,
        ],
    );
}

#[test]
fn test_redaction_keeps_keys_layout_and_line_count() {
    let text = indent2(
        r#"{"env": {"A": "1", "B": {"c": [1, "x"]}}, "model": "opus", "githubToken": "hunter2"}"#,
    );
    let out = redact(&text).unwrap();
    assert_eq!(out.lines().count(), text.lines().count());
    assert_eq!(
        v(&out),
        v(
            r#"{"env": {"A": "<redacted>", "B": {"c": ["<redacted>", "<redacted>"]}}, "model": "<redacted>", "githubToken": "<redacted>"}"#
        )
    );
    assert_eq!(redact("").unwrap(), "");
}

#[test]
fn test_diff_without_secrets_is_unchanged_by_redaction() {
    let text = indent2(r#"{"model": "opus", "permissions": {"allow": ["Bash(ls)"]}}"#);
    let (_, diff) = apply(&text, &plan(&text, &ops()).unwrap().ops).unwrap();
    // `difflib.unified_diff(text.splitlines(), new.splitlines(), …, n=0)` of the raw texts,
    // as CPython prints it for this change.
    let rule = plugin_read_rule(&plugin_root());
    let raw = [
        "--- settings.json",
        "+++ settings.json",
        "@@ -5 +5,10 @@",
        "-      \"Bash(ls)\"",
        "+      \"Bash(ls)\",",
        "+      \"Bash(rtk *)\",",
        "+      \"Bash(anthology-mem save *)\",",
        &format!("+      \"{rule}\""),
        "+    ],",
        "+    \"deny\": [",
        "+      \"Read(~/Library/Application Support/Google)\",",
        "+      \"Read(~/Library/Application Support/Google/**)\",",
        "+      \"Read(~/.config/chromium)\",",
        "+      \"Read(~/.config/chromium/**)\"",
        "@@ -6,0 +16,3 @@",
        "+  },",
        "+  \"worktree\": {",
        "+    \"baseRef\": \"head\"",
    ]
    .join("\n");
    assert_eq!(diff, raw);
    // The model value differs only outside the change, so redaction leaves the diff alone.
    assert!(!diff.contains("<redacted>"));
}

#[test]
fn test_shown_paths_stay_visible() {
    let text = dumps_indent(
        &v(
            r#"{"permissions": {"allow": ["Bash(ls)"], "deny": ["Read(x)"], "ask": ["y"]},
               "worktree": {"baseRef": "fresh", "other": "z"},
               "list": [{"permissions": {"allow": ["hidden"]}}]}"#,
        ),
        "  ",
        true,
    );
    assert_eq!(
        v(&redact(&text).unwrap()),
        v(
            r#"{"permissions": {"allow": ["Bash(ls)"], "deny": ["Read(x)"], "ask": ["<redacted>"]},
              "worktree": {"baseRef": "fresh", "other": "<redacted>"},
              "list": [{"permissions": {"allow": ["<redacted>"]}}]}"#
        )
    );
}

#[test]
fn test_every_setup_op_is_on_a_shown_path() {
    for op in setup_ops(&plugin_root(), &proc_env()).unwrap() {
        let path: Vec<&str> = op.path().iter().map(String::as_str).collect();
        assert!(SHOWN_PATHS.contains(&path.as_slice()), "{op:?}");
    }
}

// --- OpsVersionTest ------------------------------------------------------------------------

fn version(ops: &[Op]) -> String {
    ops_version(ops, &SettingsLimits::default())
}

#[test]
fn test_same_ops_same_version() {
    assert_eq!(version(&ops()), version(&ops()));
    assert!(version(&ops()).starts_with(&format!("{OPS_VERSION}-")));
}

#[test]
fn test_any_change_to_the_op_list_changes_it() {
    let base = version(&ops());
    let mut more = ops();
    more.push(Op::add(&["permissions", "allow"], "Bash(x *)"));
    let edited: Vec<Op> = ops()
        .into_iter()
        .map(|o| {
            if o.value().as_str() == Some("Bash(rtk *)") {
                o.with_value(Value::Str("Bash(rtk status *)".into()))
            } else {
                o
            }
        })
        .collect();
    let mut fewer = ops();
    fewer.pop();
    let mut roots_list: Vec<&str> = PROFILE_ROOTS.to_vec();
    roots_list.push(".config/new");
    let roots = setup_ops_with(&plugin_root(), &proc_env(), Some(&roots_list)).unwrap();
    for other in [more, edited, fewer, roots] {
        assert_ne!(version(&other), base);
    }
}

#[test]
fn test_bumping_ops_version_changes_it() {
    let base = version(&ops());
    let bumped = ops_version(
        &ops(),
        &SettingsLimits {
            ops_version: OPS_VERSION + 1,
        },
    );
    assert_ne!(bumped, base);
    assert!(bumped.starts_with(&format!("{}-", OPS_VERSION + 1)));
}

#[test]
fn test_the_plugin_location_is_not_part_of_it() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let env = Env::default().with("HOME", "/nonexistent-home");
    let one = setup_ops_with(
        &make_plugin_root(a.path(), "anthology"),
        &env,
        Some(&PROFILE_ROOTS),
    )
    .unwrap();
    let two = setup_ops_with(
        &make_plugin_root(b.path(), "anthology"),
        &env,
        Some(&PROFILE_ROOTS),
    )
    .unwrap();
    assert_ne!(one, two);
    assert_eq!(version(&one), version(&two));
}

#[test]
fn test_planned_undo_info_is_not_part_of_it() {
    let planned = plan("{}", &ops()).unwrap();
    // The stored records (with their undo info) name the same ops.
    let from_records: Vec<Op> = planned
        .stored
        .iter()
        .map(|r| Op::from_value(r).unwrap())
        .collect();
    assert_eq!(version(&from_records), version(&ops()));
    assert_eq!(version(&planned.ops), version(&ops()));
}

// --- FilesTest -----------------------------------------------------------------------------

fn files_tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let p = fs::canonicalize(t.path()).unwrap();
    (t, p)
}

#[test]
fn test_settings_path_honours_claude_config_dir_then_home() {
    assert_eq!(
        settings_path(
            &Env::default()
                .with("CLAUDE_CONFIG_DIR", "/c")
                .with("HOME", "/h")
        ),
        PathBuf::from("/c/settings.json")
    );
    assert_eq!(
        settings_path(&Env::default().with("HOME", "/h")),
        PathBuf::from("/h/.claude/settings.json")
    );
}

#[test]
fn test_read_text_keeps_crlf_and_missing_is_empty() {
    let (_t, tmp) = files_tmp();
    let path = tmp.join("settings.json");
    assert_eq!(read_text(&path).unwrap(), None);
    fs::write(&path, b"{\r\n  \"a\": 1\r\n}\r\n").unwrap();
    assert_eq!(
        read_text(&path).unwrap().as_deref(),
        Some("{\r\n  \"a\": 1\r\n}\r\n")
    );
}

#[test]
fn test_write_is_atomic_keeps_mode_and_bytes() {
    let (_t, tmp) = files_tmp();
    let path = tmp.join("settings.json");
    fs::write(&path, b"{}\r\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    write_text(&path, "{\r\n  \"a\": \"é\"\r\n}\r\n").unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        "{\r\n  \"a\": \"é\"\r\n}\r\n".as_bytes()
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    let names: Vec<_> = fs::read_dir(&tmp)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("settings.json")]);
}

#[test]
fn test_write_follows_a_symlink_to_its_target() {
    let (_t, tmp) = files_tmp();
    let target = tmp.join("dotfiles/settings.json");
    fs::create_dir(target.parent().unwrap()).unwrap();
    fs::write(&target, "{}").unwrap();
    let link = tmp.join("settings.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    write_text(&link, r#"{"a": 1}"#).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"a": 1}"#);
}

#[test]
fn test_write_creates_the_folder_for_a_new_file() {
    let (_t, tmp) = files_tmp();
    let path = tmp.join("cfg/settings.json");
    write_text(&path, "{}").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
}

// --- Properties ----------------------------------------------------------------------------

/// A random settings value (objects, arrays, strings and scalars), at most `depth` deep.
fn arb_value(depth: u32) -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1000i64..1000).prop_map(|i| Value::Int(json::Int::Small(i))),
        "[a-zA-Z0-9 é✓()*/.~-]{0,12}".prop_map(Value::Str),
    ];
    leaf.prop_recursive(depth, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(("[a-zA-Z]{1,8}", inner), 0..4).prop_map(|kv| {
                let mut m = Map::new();
                for (k, v) in kv {
                    m.insert(k, v);
                }
                Value::Object(m)
            }),
        ]
    })
}

/// Settings documents in the layouts the patcher must keep byte-stable.
fn arb_settings() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(
            (
                "permissions|worktree|env|model|hooks|[a-z]{1,6}",
                arb_value(3),
            ),
            0..5,
        ),
        0usize..6,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(kv, layout, crlf, bom)| {
            let mut m = Map::new();
            for (k, v) in kv {
                m.insert(k, v);
            }
            let doc = Value::Object(m);
            let mut text = match layout {
                0 => dumps_indent(&doc, "  ", false) + "\n",
                1 => dumps_indent(&doc, "    ", true),
                2 => dumps_indent(&doc, "\t", false) + "\n",
                3 => dumps_sep(&doc, ",", ":"),
                4 => dumps_sep(&doc, ", ", ": ") + "\n",
                _ => String::new(),
            };
            if crlf {
                text = text.replace('\n', "\r\n");
            }
            if bom {
                text.insert(0, '\u{feff}');
            }
            text
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, rng_seed: proptest::test_runner::RngSeed::Fixed(20261121), failure_persistence: None, ..ProptestConfig::default() })]

    /// Apply then revert restores the original bytes, for every text `plan` accepts.
    #[test]
    fn apply_then_revert_restores_the_bytes(text in arb_settings()) {
        let ops = ops();
        if let Ok(planned) = plan(&text, &ops) {
            let (new, _) = apply(&text, &planned.ops).unwrap();
            prop_assert_eq!(revert(&new, &planned.stored).unwrap(), text);
        }
    }
}

// --- Beyond the Python tests ---------------------------------------------------------------

/// A manifest Python's json cannot load (RecursionError, which setup does not catch) is
/// refused, never accepted; a merely deep one still loads.
#[test]
fn a_manifest_too_deep_for_python_is_refused() {
    let g = guard();
    let root = make_plugin_root(&g.tmp.join("plugins"), "anthology");
    let nested = |n: usize| {
        format!(
            r#"{{"name": "anthology", "x": {}{}}}"#,
            "[".repeat(n),
            "]".repeat(n)
        )
    };
    let manifest = root.join(".claude-plugin/plugin.json");
    fs::write(&manifest, nested(12_000)).unwrap();
    let err = setup_ops_with(&root, &g.env, Some(&PROFILE_ROOTS)).unwrap_err();
    assert!(err.starts_with("RecursionError: "), "{err}");
    fs::write(&manifest, nested(900)).unwrap();
    assert!(setup_ops_with(&root, &g.env, Some(&PROFILE_ROOTS)).is_ok());
}

/// `list(state.get("settings_ops") or [])`.
#[test]
fn stored_ops_take_pythons_list_of_the_state_value() {
    assert_eq!(stored_ops(None).unwrap(), Vec::<Value>::new());
    for falsy in ["null", "false", "0", "\"\"", "[]", "{}"] {
        assert_eq!(
            stored_ops(Some(&v(falsy))).unwrap(),
            Vec::<Value>::new(),
            "{falsy}"
        );
    }
    assert_eq!(
        stored_ops(Some(&v(r#"[{"op": "add"}, 1]"#))).unwrap(),
        vec![v(r#"{"op": "add"}"#), v("1")]
    );
    assert_eq!(
        stored_ops(Some(&v(r#""ab""#))).unwrap(),
        vec![v(r#""a""#), v(r#""b""#)]
    );
    assert_eq!(
        stored_ops(Some(&v(r#"{"k": 1}"#))).unwrap(),
        vec![v(r#""k""#)]
    );
    for bad in ["1", "true", "2.5"] {
        assert!(
            stored_ops(Some(&v(bad)))
                .unwrap_err()
                .starts_with("TypeError: "),
            "{bad}"
        );
    }
}
