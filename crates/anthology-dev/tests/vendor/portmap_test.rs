//! test_vendor.PortmapTest: the `portmap` entry field, the script rule applied to the pristine
//! and the shipped tree. Plus the record that the real port map patterns use no lookaround.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anthology_dev::vendor;
use anthology_pycompat::json::{self, Value};

use crate::fixture::{Fixture, MIT_TEXT, SHA_B, entry, have_git, jv, repo_root, s, set, write};

const PORTMAP: &str = "vendor/patches/taste.portmap.json";
const NEO: &str = "// bridge\nexport const x = 1;\n";
const PATCH_ADD_DROP: &str = concat!(
    "--- a/scripts/drop.mjs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-// drop\n-import fs from 'node:fs';\n",
    "--- /dev/null\n+++ b/scripts/neo.mjs\n@@ -0,0 +1,2 @@\n+// bridge\n+export const x = 1;\n",
);
const UPSTREAM: &[(&str, &str)] = &[
    ("LICENSE", MIT_TEXT),
    ("SKILL.md", "---\nname: taste\n---\nbody\n"),
    ("scripts/keep.mjs", "// keep\nimport fs from 'node:fs';\n"),
    ("scripts/port.mjs", "// port\nimport fs from 'node:fs';\n"),
    ("scripts/drop.mjs", "// drop\nimport fs from 'node:fs';\n"),
    ("assets/lib.js", "window.x = 1;\n"),
];

fn real_portmap() -> json::Map {
    let path = repo_root().join("vendor/patches/design-taste-frontend.portmap.json");
    jv(&fs::read_to_string(path).unwrap())
        .as_object()
        .unwrap()
        .clone()
}

struct Case {
    f: Fixture,
    rule: Value,
    chrome: Value,
}

fn setup() -> Case {
    let real = real_portmap();
    let c = Case {
        f: Fixture::new(),
        rule: real.get("script_rule").unwrap().clone(),
        chrome: real.get("chrome_regex").unwrap().clone(),
    };
    c.write_map();
    c
}

fn good(shipped: &Path) {
    fs::remove_file(shipped.join("scripts/drop.mjs")).unwrap();
    write(shipped, "scripts/neo.mjs", NEO, false);
}

impl Case {
    fn write_map(&self) {
        let mut data = json::Map::new();
        data.insert("script_rule".into(), self.rule.clone());
        data.insert("chrome_regex".into(), self.chrome.clone());
        data.insert(
            "scripts".into(),
            jv(r#"{
                "scripts/keep.mjs": {"action": "keep", "chrome": false, "why": "k"},
                "scripts/port.mjs": {"action": "port", "how": "trim", "chrome": true, "why": "p", "browseros": "b"},
                "scripts/drop.mjs": {"action": "drop", "chrome": true, "why": "d", "browseros": "b"}
            }"#),
        );
        data.insert(
            "added".into(),
            jv(r#"{"scripts/neo.mjs": {"chrome": false, "why": "n"}}"#),
        );
        write(
            &self.f.root,
            PORTMAP,
            json::dumps(&Value::Object(data), &json::INDENT2) + "\n",
            false,
        );
    }

    fn entry(&self, files: &[(&str, &str)], patch: bool) -> Value {
        let transform = if patch {
            r#"[{"patch": "vendor/patches/taste.patch"}]"#
        } else {
            "[]"
        };
        self.f.add_upstream(
            "taste",
            files,
            &[("transform", jv(transform)), ("portmap", s(PORTMAP))],
        )
    }

    fn data(&self) -> json::Map {
        jv(&fs::read_to_string(self.f.root.join(PORTMAP)).unwrap())
            .as_object()
            .unwrap()
            .clone()
    }

    fn trees(&self, shipped_edit: impl Fn(&Path)) -> (PathBuf, PathBuf) {
        let (pristine, shipped) = (self.f.tmp.join("p"), self.f.tmp.join("s"));
        for base in [&pristine, &shipped] {
            let _ = fs::remove_dir_all(base);
            for (rel, text) in UPSTREAM {
                write(base, rel, text, false);
            }
        }
        shipped_edit(&shipped);
        (pristine, shipped)
    }

    fn problems(&self, shipped_edit: impl Fn(&Path)) -> Vec<String> {
        let (pristine, shipped) = self.trees(shipped_edit);
        vendor::portmap_problems(&self.data(), &pristine, &shipped).unwrap()
    }

    fn build(&self) -> vendor::Result<vendor::BuildStats> {
        self.f.build("taste")
    }
}

fn any(found: &[String], a: &str, b: &str) -> bool {
    found.iter().any(|p| p.contains(a) && p.contains(b))
}

#[test]
fn test_the_rule_finds_extensions_shebangs_and_node_js_only() {
    let c = setup();
    let tree = c.f.tmp.join("t");
    for (rel, text) in [
        ("a.mjs", ""),
        ("B.MJS", ""),
        ("c.py", ""),
        ("d.sh", ""),
        ("e.ts", ""),
        ("f.txt", "#!/bin/sh\n"),
        ("g.txt", "# !\n"),
        ("h.md", "# title\n"),
        ("browser.js", "window.x = 1;\n"),
        ("node1.js", "const fs = require('fs');\n"),
        ("node2.js", "import fs from 'node:fs';\n"),
        ("node3.js", "process.exit(1)\n"),
        ("node4.js", "const m = await import('node:fs');\n"),
        ("node5.js", "import { dirname } from 'path';\n"),
        ("page.js", "const required = 1; // from 'react'\n"),
        ("sub/x.cjs", ""),
    ] {
        write(&tree, rel, text, false);
    }
    write(&tree, "bin.dat", b"\xff\xfe\x00 process.exit(1)", false);
    let want: BTreeSet<String> = [
        "a.mjs",
        "B.MJS",
        "c.py",
        "d.sh",
        "e.ts",
        "f.txt",
        "node1.js",
        "node2.js",
        "node3.js",
        "node4.js",
        "node5.js",
        "sub/x.cjs",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        vendor::portmap_scripts(&tree, c.rule.as_object().unwrap()).unwrap(),
        want
    );
}

#[test]
fn test_a_clean_shipped_tree_has_no_problems() {
    let c = setup();
    assert_eq!(c.problems(good), Vec::<String>::new());
}

#[test]
fn test_an_upstream_script_missing_from_the_map_is_a_problem() {
    let c = setup();
    let (pristine, shipped) = c.trees(good);
    write(
        &pristine,
        "scripts/new.mjs",
        "import fs from 'node:fs';\n",
        false,
    );
    write(
        &shipped,
        "scripts/new.mjs",
        "import fs from 'node:fs';\n",
        false,
    );
    let found = vendor::portmap_problems(&c.data(), &pristine, &shipped).unwrap();
    assert!(
        any(&found, "scripts/new.mjs", "not in the port map"),
        "{found:?}"
    );
}

#[test]
fn test_a_mapped_script_that_is_not_upstream_is_a_problem() {
    let c = setup();
    let (pristine, shipped) = c.trees(good);
    fs::remove_file(pristine.join("scripts/keep.mjs")).unwrap();
    let found = vendor::portmap_problems(&c.data(), &pristine, &shipped).unwrap();
    assert!(any(&found, "scripts/keep.mjs", "not upstream"), "{found:?}");
}

#[test]
fn test_a_shipped_drop_script_is_a_problem() {
    let c = setup();
    let found = c.problems(|s| {
        write(s, "scripts/neo.mjs", NEO, false);
    });
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("scripts/drop.mjs"));
    assert!(found[0].contains("drop"));
}

#[test]
fn test_a_modified_keep_script_is_a_problem() {
    let c = setup();
    let found = c.problems(|s| {
        good(s);
        write(
            s,
            "scripts/keep.mjs",
            "// keep\nimport fs from 'node:fs';\n// edited\n",
            false,
        );
    });
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("scripts/keep.mjs"));
    assert!(found[0].contains("byte-identical"));
}

#[test]
fn test_a_missing_keep_or_port_script_is_a_problem() {
    let c = setup();
    for rel in ["scripts/keep.mjs", "scripts/port.mjs"] {
        let found = c.problems(|s| {
            good(s);
            fs::remove_file(s.join(rel)).unwrap();
        });
        assert!(any(&found, rel, "not shipped"), "{rel} {found:?}");
    }
}

#[test]
fn test_a_ported_script_may_differ() {
    let c = setup();
    let found = c.problems(|s| {
        good(s);
        write(
            s,
            "scripts/port.mjs",
            "// ported\nimport fs from 'node:fs';\n",
            false,
        );
    });
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn test_the_added_script_must_ship_and_must_not_be_upstream() {
    let c = setup();
    let found = c.problems(|s| fs::remove_file(s.join("scripts/drop.mjs")).unwrap());
    assert!(any(&found, "scripts/neo.mjs", "not shipped"), "{found:?}");
    let (pristine, shipped) = c.trees(good);
    write(
        &pristine,
        "scripts/neo.mjs",
        "import fs from 'node:fs';\n",
        false,
    );
    let found = vendor::portmap_problems(&c.data(), &pristine, &shipped).unwrap();
    assert!(any(&found, "scripts/neo.mjs", "upstream"), "{found:?}");
}

#[test]
fn test_an_unlisted_shipped_script_is_a_problem() {
    let c = setup();
    let found = c
        .problems(|s| {
            good(s);
            write(s, "scripts/extra.py", "print(1)\n", false);
            write(s, "assets/extra.js", "const fs = require('fs');\n", false);
        })
        .join("\n");
    assert!(found.contains("scripts/extra.py"));
    assert!(found.contains("assets/extra.js"));
}

#[test]
fn test_a_chrome_string_in_a_file_or_a_path_is_a_problem() {
    let c = setup();
    let cases: [(&str, &[u8]); 4] = [
        ("notes/a.md", b"launch with --Remote-Debugging-Port=0"),
        ("notes/b.md", b"uses Playwright"),
        ("notes/c.bin", b"\x00\x01 google chrome \x02"),
        ("notes/puppeteer.svg", b"<svg/>"),
    ];
    for (rel, text) in cases {
        let found = c.problems(|s| {
            good(s);
            write(s, rel, text, false);
        });
        assert_eq!(found.len(), 1, "{rel} {found:?}");
        assert!(found[0].contains(rel));
    }
}

#[test]
fn test_headless_chrome_alone_is_not_a_chrome_launch() {
    let c = setup();
    let found = c.problems(|s| {
        good(s);
        write(s, "a.md", "HeadlessChrome/ UA", false);
    });
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn test_build_refuses_an_unmapped_upstream_script() {
    let c = setup();
    let mut files = UPSTREAM.to_vec();
    files.push(("scripts/new.mjs", "import fs from 'node:fs';\n"));
    let e = c.entry(&files, false);
    c.f.set_sources(std::slice::from_ref(&e));
    let err = c.build().unwrap_err();
    assert!(err.0.contains("scripts/new.mjs"), "{}", err.0);
    assert!(!c.f.target("taste", "").exists());
}

#[test]
fn test_build_refuses_a_shipped_drop_script() {
    let c = setup();
    let e = c.entry(UPSTREAM, false);
    c.f.set_sources(&[e]);
    let err = c.build().unwrap_err();
    assert!(err.0.contains("scripts/drop.mjs"), "{}", err.0);
}

#[test]
fn test_a_missing_portmap_file_is_an_error() {
    let c = setup();
    let e = c.entry(UPSTREAM, false);
    fs::remove_file(c.f.root.join(PORTMAP)).unwrap();
    c.f.set_sources(&[e]);
    let err = c.build().unwrap_err();
    assert!(err.0.contains("taste.portmap.json"), "{}", err.0);
}

#[test]
fn test_the_portmap_path_must_be_a_safe_json_file_under_vendor_patches() {
    for bad in [
        s("../x.json"),
        s("/etc/x.json"),
        s("vendor/other/x.json"),
        s("vendor/patches/x.txt"),
        s("vendor/patches/"),
        s("vendor/patches/../x.json"),
        jv("3"),
    ] {
        let errors = vendor::validate_sources(&[entry("p", &[("portmap", bad.clone())])]);
        assert!(
            errors.iter().any(|m| m.contains("portmap")),
            "{bad:?} {errors:?}"
        );
    }
    let ok = entry("p", &[("portmap", s("vendor/patches/p.portmap.json"))]);
    assert_eq!(vendor::validate_sources(&[ok]), Vec::<String>::new());
}

#[test]
fn test_check_is_clean_with_a_good_patch_and_lists_every_problem_without_one() {
    if !have_git() {
        return;
    }
    let c = setup();
    let mut e = c.entry(UPSTREAM, true);
    write(
        &c.f.root,
        "vendor/patches/taste.patch",
        PATCH_ADD_DROP,
        false,
    );
    c.f.build_all(std::slice::from_ref(&e));
    assert_eq!(c.f.check(), Vec::<String>::new());
    set(&mut e, "transform", jv("[]"));
    c.f.set_sources(&[e]);
    let found = c.f.check();
    assert!(
        found.iter().any(|p| p.contains("scripts/drop.mjs")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|p| p.contains("scripts/neo.mjs")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .filter(|p| p.starts_with("taste: portmap: "))
            .count()
            >= 2,
        "{found:?}"
    );
}

#[test]
fn test_sync_reports_portmap_problems_without_failing_the_rebuild() {
    if !have_git() {
        return;
    }
    let c = setup();
    let e = c.entry(UPSTREAM, true);
    write(
        &c.f.root,
        "vendor/patches/taste.patch",
        PATCH_ADD_DROP,
        false,
    );
    c.f.build_all(&[e]);
    write(
        &c.f.upstream("taste"),
        "scripts/new.mjs",
        "import fs from 'node:fs';\n",
        false,
    );
    let report =
        c.f.sync("taste", "v2", &|_, _| Ok(SHA_B.to_string()))
            .unwrap();
    assert!(
        report.portmap.iter().any(|p| p.contains("scripts/new.mjs")),
        "{:?}",
        report.portmap
    );
}

/// Plan GC-16: `js_node_regex` goes through the Python → fancy-regex translation, and
/// `chrome_regex` through the ASCII bytes mode. Neither uses a lookaround today; this test
/// records that, so a lookaround added upstream is noticed (it would still compile and run
/// through fancy-regex, but deserves a parity check against Python first).
#[test]
fn test_portmap_patterns_have_no_lookaround() {
    let dir = repo_root().join("vendor/patches");
    let mut seen = 0;
    for e in fs::read_dir(&dir).unwrap() {
        let path = e.unwrap().path();
        if !path.to_string_lossy().ends_with(".portmap.json") {
            continue;
        }
        let map = jv(&fs::read_to_string(&path).unwrap());
        let map = map.as_object().unwrap();
        let rule = map.get("script_rule").unwrap().as_object().unwrap();
        for pattern in [
            rule.get("js_node_regex").unwrap().as_str().unwrap(),
            map.get("chrome_regex").unwrap().as_str().unwrap(),
        ] {
            for look in ["(?=", "(?!", "(?<=", "(?<!"] {
                assert!(!pattern.contains(look), "{}: {pattern}", path.display());
            }
        }
        seen += 1;
    }
    assert!(seen >= 1, "no port map under {}", dir.display());
}
