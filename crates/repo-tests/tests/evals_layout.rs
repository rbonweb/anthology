//! Port of `tests/test_evals_layout.py`: static checks on the behaviour evals (case files, the
//! BrowserOS Neo mock, the runner's grants and the evals workflow).

use std::collections::{BTreeSet, HashMap};

use anthology_pycompat::json::{self, Int, Value};
use regex::Regex;
use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};

const NON_CASE_DIRS: [&str; 3] = ["mocks", "results", "smoke"];
const TOP_KEYS: [&str; 8] = [
    "schema_version",
    "name",
    "description",
    "tags",
    "plugins",
    "execution",
    "runs",
    "graders",
];
const EXECUTION_KEYS: [&str; 5] = [
    "prompt",
    "max_turns",
    "timeout_seconds",
    "model",
    "allowed_tools",
];
const REGEX_TARGETS: [&str; 3] = ["last_message", "trace", "mock_calls"];
const REGEX_MATCH: [&str; 2] = ["contains", "not_contains"];
const PLUGINS: [&str; 3] = [
    "../../plugins/anthology",
    "../../plugins/anthology-guardrails",
    "../../plugins/anthology-superpowers",
];
const MCP_PREFIX: &str = "mcp__browseros-neo__";
const REQUIRED_CASES: [&str; 11] = [
    "guard-g1",
    "guard-g4",
    "guard-g6",
    "guard-beats-atl",
    "hook-reach-subagent",
    "browser-browseros-only",
    "harness-sanity",
    "guide-present",
    "guide-main-only",
    "skills-hidden",
    "agent-fields",
];
// These two cannot pass inside `claude plugin eval` (no atl state, no plugin bin/ on the Bash
// PATH), so they are smokes.
const MOVED_TO_SMOKES: [&str; 2] = ["atl-rewrite", "agent-memory-path"];
// Bash is visible in this case only so that a host-browser launch attempt is recorded and denied.
const BASH_PROMPT_EXEMPT: [&str; 1] = ["browser-browseros-only"];
// This case holds Write (and Edit) only so that its `max: 0` checks are real.
const WRITE_PROMPT_EXEMPT: [&str; 1] = ["agent-fields"];
// The private-key marker is split so no scanner reads this file as a key.
const FORBIDDEN_LITERALS: [&str; 5] = [
    "password:",
    "password =",
    "AKIA",
    "ghp_",
    concat!("BEGIN", " PRIVATE KEY"),
];
const ABSOLUTE_PATHS: [&str; 3] = ["/Users/", "/home/", "~/"];
const KEY_GATE: &str = "steps.key.outputs.present == 'true'";
const GLOB_ANCHOR: [(&str, &str); 2] = [("tool", "Glob"), ("input_match", r"\*\.md")];

fn grader_keys(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "tool_used" => &["name", "type", "tool", "input_match", "min", "max"],
        "tool_order" => &["name", "type", "before", "after"],
        "regex" => &["name", "type", "target", "pattern", "match", "flags"],
        "file_exists" => &["name", "type", "path", "exists"],
        "llm" => &["name", "type", "criteria"],
        _ => return None,
    })
}

// ---- YAML helpers (PyYAML semantics where the Python relies on them) ----

type Cases = Vec<(String, Yaml)>;

/// Python truthiness of a loaded YAML value.
fn truthy(y: &Yaml) -> bool {
    match y {
        Yaml::Null | Yaml::BadValue => false,
        Yaml::Boolean(b) => *b,
        Yaml::Integer(i) => *i != 0,
        Yaml::Real(r) => r.parse::<f64>() != Ok(0.0),
        Yaml::String(s) => !s.is_empty(),
        Yaml::Array(a) => !a.is_empty(),
        Yaml::Hash(h) => !h.is_empty(),
        _ => true,
    }
}

/// `isinstance(v, int)`: a Python bool is an int.
fn int_of(y: &Yaml) -> Option<i64> {
    match y {
        Yaml::Integer(i) => Some(*i),
        Yaml::Boolean(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// `set(mapping)`; a key that is not a string is kept as its debug text so it fails a subset check.
fn key_set(y: &Yaml) -> BTreeSet<String> {
    match y.as_hash() {
        Some(h) => h
            .keys()
            .map(|k| match k {
                Yaml::String(s) => s.clone(),
                other => format!("{other:?}"),
            })
            .collect(),
        None => BTreeSet::new(),
    }
}

fn extras(have: &BTreeSet<String>, allowed: &[&str]) -> Vec<String> {
    have.iter()
        .filter(|k| !allowed.contains(&k.as_str()))
        .cloned()
        .collect()
}

fn set_of(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// `value == {k: v, ...}` for a mapping of strings (key order does not matter).
fn dict_eq(y: &Yaml, expected: &[(&str, &str)]) -> bool {
    match y.as_hash() {
        Some(h) => {
            h.len() == expected.len() && expected.iter().all(|(k, v)| y[*k].as_str() == Some(*v))
        }
        None => false,
    }
}

/// `value == [str, ...]`: a list of exactly these strings.
fn str_list_eq(y: &Yaml, expected: &[&str]) -> bool {
    let items = yaml::items(y);
    items.len() == expected.len()
        && items
            .iter()
            .zip(expected)
            .all(|(a, b)| a.as_str() == Some(*b))
}

fn str_list_has(y: &Yaml, needle: &str) -> bool {
    yaml::items(y).iter().any(|i| i.as_str() == Some(needle))
}

fn text_of(y: &Yaml) -> &str {
    y.as_str().unwrap_or("")
}

/// Whether any string (key or value) anywhere in `y` contains `needle`; `json.dumps` of the value
/// contains the needle exactly then (it never splits `secrets.`).
fn contains_text(y: &Yaml, needle: &str) -> bool {
    match y {
        Yaml::String(s) => s.contains(needle),
        Yaml::Array(a) => a.iter().any(|i| contains_text(i, needle)),
        Yaml::Hash(h) => h
            .iter()
            .any(|(k, v)| contains_text(k, needle) || contains_text(v, needle)),
        _ => false,
    }
}

fn ensure(ok: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(msg()) }
}

/// Runs `row` for every case and fails once with every failing row (subTest semantics).
fn each_case(cases: &Cases, mut row: impl FnMut(&str, &Yaml) -> Result<(), String>) {
    let fails: Vec<String> = cases
        .iter()
        .filter_map(|(n, c)| row(n, c).err().map(|e| format!("case={n}: {e}")))
        .collect();
    assert!(
        fails.is_empty(),
        "{} failing row(s):\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// Same for any list of labelled rows.
fn each_row<T>(rows: &[T], label: impl Fn(&T) -> String, row: impl Fn(&T) -> Result<(), String>) {
    let fails: Vec<String> = rows
        .iter()
        .filter_map(|r| row(r).err().map(|e| format!("{}: {e}", label(r))))
        .collect();
    assert!(
        fails.is_empty(),
        "{} failing row(s):\n{}",
        fails.len(),
        fails.join("\n")
    );
}

// ---- the case files ----

fn case_dirs() -> Vec<String> {
    repo::list_dir("evals")
        .into_iter()
        .filter(|n| {
            !NON_CASE_DIRS.contains(&n.as_str()) && repo::path(&format!("evals/{n}")).is_dir()
        })
        .collect()
}

fn all_cases() -> Cases {
    case_dirs()
        .into_iter()
        .map(|d| {
            let case = yaml::load_file(&format!("evals/{d}/case.yaml"));
            (d, case)
        })
        .collect()
}

fn case<'a>(cases: &'a Cases, name: &str) -> Result<&'a Yaml, String> {
    cases
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| c)
        .ok_or_else(|| format!("no case {name}"))
}

fn has_case(cases: &Cases, name: &str) -> bool {
    cases.iter().any(|(n, _)| n == name)
}

fn prompt<'a>(name: &str, case: &'a Yaml) -> &'a str {
    case["execution"]["prompt"]
        .as_str()
        .unwrap_or_else(|| panic!("{name}: execution.prompt is not a string"))
}

fn allowed_tools(case: &Yaml) -> Vec<String> {
    yaml::strings(&case["execution"]["allowed_tools"])
}

/// Every regex a grader carries.
fn grader_regexes(g: &Yaml) -> Vec<&Yaml> {
    let mut found = Vec::new();
    if yaml::has(g, "input_match") {
        found.push(&g["input_match"]);
    }
    if yaml::has(g, "pattern") {
        found.push(&g["pattern"]);
    }
    for side in ["before", "after"] {
        if g[side].as_hash().is_some() && yaml::has(&g[side], "input_match") {
            found.push(&g[side]["input_match"]);
        }
    }
    found
}

/// The tools a grader names (`None` for a dict side without a string `tool`).
fn grader_tools(g: &Yaml) -> Vec<Option<&str>> {
    let mut tools = Vec::new();
    if yaml::has(g, "tool") {
        tools.push(g["tool"].as_str());
    }
    for side in ["before", "after"] {
        let value = &g[side];
        if value.as_str().is_some() {
            tools.push(value.as_str());
        } else if value.as_hash().is_some() {
            tools.push(value["tool"].as_str());
        }
    }
    tools
}

fn slug_of(r: &str) -> String {
    match r.strip_prefix("ds/") {
        Some(rest) => format!("ds-{rest}"),
        None => r.to_string(),
    }
}

fn path_regex_of(r: &str) -> String {
    match r.strip_prefix("ds/") {
        Some(rest) => format!(r"knowledge/design-skills/skills/{rest}/SKILL\.md"),
        None => format!(r"knowledge/{r}/SKILL\.md"),
    }
}

/// The `--allow-tools` grants listed in scripts/run-evals.sh, one per line.
fn run_script_grants() -> Vec<String> {
    let text = repo::read("scripts/run-evals.sh");
    let m = Regex::new(r"(?m)^GRANTS='([^']*)'")
        .unwrap()
        .captures(&text)
        .expect("scripts/run-evals.sh has no GRANTS='...' block");
    m[1].lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Startup reading per agent (caveman first): the roster's always reads.
fn expected_startup() -> Vec<(&'static str, Vec<String>)> {
    repo::expected_roster()
        .into_iter()
        .map(|a| {
            let mut refs = vec!["caveman".to_string()];
            refs.extend(a.knowledge.iter().map(|k| (*k).to_string()));
            (a.name, refs)
        })
        .collect()
}

/// Every Startup path per agent: the always reads, then the conditional ones.
fn expected_startup_paths() -> Vec<(&'static str, Vec<String>)> {
    repo::expected_roster()
        .into_iter()
        .map(|a| {
            let mut refs = vec!["caveman".to_string()];
            refs.extend(
                a.knowledge
                    .iter()
                    .chain(&a.conditional)
                    .map(|k| (*k).to_string()),
            );
            (a.name, refs)
        })
        .collect()
}

fn preload_agents() -> BTreeSet<&'static str> {
    repo::expected_roster()
        .into_iter()
        .filter(|a| !a.preloads.is_empty())
        .map(|a| a.name)
        .collect()
}

// `plugin eval` refuses worktree isolation, so a worktree agent never runs as a knowledge-read
// case; the knowledge-read-writers smoke checks those instead.
fn worktree_agents() -> BTreeSet<&'static str> {
    repo::expected_roster()
        .into_iter()
        .filter(|a| a.worktree)
        .map(|a| a.name)
        .collect()
}

fn knowledge_agents() -> Vec<&'static str> {
    let wt = worktree_agents();
    let mut v: Vec<&'static str> = repo::expected_roster()
        .into_iter()
        .map(|a| a.name)
        .filter(|n| !wt.contains(n))
        .collect();
    v.sort_unstable();
    v
}

/// The first `# ` heading of a superpowers skill, without the marker.
fn skill_heading(skill: &str) -> Result<String, String> {
    let text = repo::read(&repo::superpowers_skill_path(skill));
    let body = if text.starts_with("---\n") {
        text.split_once("\n---\n")
            .ok_or_else(|| format!("{skill}: no end of frontmatter"))?
            .1
    } else {
        &text
    };
    let m = Regex::new(r"(?m)^# (.+)$")
        .unwrap()
        .captures(body)
        .ok_or_else(|| format!("{skill}: no heading"))?;
    Ok(m[1].trim().to_string())
}

/// The knowledge references of an agent file's `Startup` section, in order, as `x` / `ds/x`.
fn startup_refs(text: &str) -> Option<Vec<String>> {
    let head = Regex::new(r"(?m)^(#+)[ \t]*Startup\b.*$")
        .unwrap()
        .captures(text)?;
    let whole = head.get(0).unwrap();
    let rest = &text[whole.end()..];
    let next = Regex::new(&format!(r"(?m)^#{{1,{}}}[ \t]+\S", head[1].len())).unwrap();
    let section = match next.find(rest) {
        Some(n) => &rest[..n.start()],
        None => rest,
    };
    let prefix = "design-skills/skills/";
    let refs = Regex::new(r"knowledge/([A-Za-z0-9_./-]+?)/SKILL\.md")
        .unwrap()
        .captures_iter(section)
        .map(|c| match c[1].strip_prefix(prefix) {
            Some(rest) => format!("ds/{rest}"),
            None => c[1].to_string(),
        })
        .collect();
    Some(refs)
}

mod case_layout_tests {
    use super::*;

    #[test]
    fn test_there_are_cases() {
        assert!(!all_cases().is_empty());
    }

    #[test]
    fn test_every_case_directory_has_a_case_file() {
        for d in case_dirs() {
            assert!(repo::path(&format!("evals/{d}/case.yaml")).is_file(), "{d}");
        }
    }

    // rows:computed one row per case directory, read from the repo
    #[test]
    fn test_names_and_schema() {
        let name_re = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap();
        each_case(&all_cases(), |dirname, case| {
            ensure(name_re.is_match(dirname), || {
                format!("{dirname} is not a kebab-case name")
            })?;
            ensure(case["name"].as_str() == Some(dirname), || {
                format!("name is {:?}", case["name"])
            })?;
            ensure(case["schema_version"].as_str() == Some("1.0"), || {
                format!("schema_version is {:?}", case["schema_version"])
            })?;
            ensure(int_of(&case["runs"]) == Some(1), || {
                format!("runs is {:?}", case["runs"])
            })?;
            ensure(case["description"].as_str().is_some(), || {
                "description is not a string".into()
            })?;
            ensure(truthy(&case["description"]), || {
                "description is empty".into()
            })
        });
    }

    // rows:computed one row per case directory, read from the repo
    #[test]
    fn test_keys_are_the_allowed_ones() {
        each_case(&all_cases(), |_, case| {
            let keys = key_set(case);
            let extra = extras(&keys, &TOP_KEYS);
            ensure(extra.is_empty(), || format!("unknown top keys {extra:?}"))?;
            ensure(
                ["schema_version", "name", "execution", "graders"]
                    .iter()
                    .all(|k| keys.contains(*k)),
                || format!("a required top key is missing from {keys:?}"),
            )?;
            let extra = extras(&key_set(&case["execution"]), &EXECUTION_KEYS);
            ensure(extra.is_empty(), || {
                format!("unknown execution keys {extra:?}")
            })
        });
    }

    // rows:computed one row per case directory, read from the repo
    #[test]
    fn test_execution_values() {
        each_case(&all_cases(), |_, case| {
            let ex = &case["execution"];
            ensure(ex["prompt"].as_str().is_some(), || {
                "prompt is not a string".into()
            })?;
            ensure(!text_of(&ex["prompt"]).trim().is_empty(), || {
                "prompt is blank".into()
            })?;
            ensure(
                ["opus", "sonnet", "haiku"]
                    .iter()
                    .any(|m| ex["model"].as_str() == Some(*m)),
                || format!("model is {:?}", ex["model"]),
            )?;
            ensure(int_of(&ex["max_turns"]).is_some(), || {
                "max_turns is not an int".into()
            })?;
            ensure(int_of(&ex["timeout_seconds"]).is_some(), || {
                "timeout_seconds is not an int".into()
            })?;
            ensure(ex["allowed_tools"].as_vec().is_some(), || {
                "allowed_tools is not a list".into()
            })?;
            ensure(truthy(&case["tags"]), || "tags are empty".into())?;
            ensure(
                yaml::items(&case["tags"])
                    .iter()
                    .all(|t| t.as_str().is_some()),
                || "a tag is not a string".into(),
            )
        });
    }

    // rows:computed one row per case directory, read from the repo
    #[test]
    fn test_plugins_are_the_three_in_order_and_resolve() {
        each_case(&all_cases(), |dirname, case| {
            ensure(str_list_eq(&case["plugins"], &PLUGINS), || {
                format!("plugins are {:?}", case["plugins"])
            })?;
            for rel in yaml::strings(&case["plugins"]) {
                let raw = repo::path(&format!("evals/{dirname}/{rel}"));
                let resolved = raw.canonicalize().unwrap_or(raw);
                let manifest = resolved.join(".claude-plugin").join("plugin.json");
                ensure(manifest.is_file(), || {
                    format!("no manifest at {}", manifest.display())
                })?;
            }
            Ok(())
        });
    }

    // U9: a plugin must never carry its evals (they would ship to users).
    #[test]
    fn test_no_evals_directory_inside_a_plugin() {
        for plugin in repo::list_dir("plugins") {
            assert!(
                !repo::path(&format!("plugins/{plugin}/evals")).exists(),
                "{plugin}"
            );
        }
    }

    // rows:computed one row per case directory, read from the repo
    #[test]
    fn test_graders() {
        each_case(&all_cases(), |_, case| {
            ensure(truthy(&case["graders"]), || "no graders".into())?;
            let graders = yaml::items(&case["graders"]);
            let names: Vec<&str> = graders
                .iter()
                .map(|g| g["name"].as_str().ok_or("a grader has no name"))
                .collect::<Result<_, _>>()?;
            let unique: BTreeSet<&&str> = names.iter().collect();
            ensure(unique.len() == names.len(), || {
                "grader names must be unique".into()
            })?;
            for g in graders {
                let name = text_of(&g["name"]);
                let kind = text_of(&g["type"]);
                let Some(allowed) = grader_keys(kind) else {
                    return Err(format!("{name}: unknown type {:?}", g["type"]));
                };
                let extra = extras(&key_set(g), allowed);
                ensure(extra.is_empty(), || {
                    format!("{name}: unknown keys {extra:?}")
                })?;
                for pattern in grader_regexes(g) {
                    let Some(p) = pattern.as_str() else {
                        return Err(format!("{name}: a pattern is not a string"));
                    };
                    // The data is compiled as given. A pattern Python's `re` accepts but the
                    // regex crate cannot (lookaround, backreference) fails here, loudly.
                    Regex::new(p).map_err(|e| {
                        format!(
                            "{name}: pattern {p:?} does not compile with the regex crate \
                             (Python's re may accept it): {e}"
                        )
                    })?;
                }
                check_grader_shape(g)?;
            }
            Ok(())
        });
    }

    fn check_grader_shape(g: &Yaml) -> Result<(), String> {
        let name = text_of(&g["name"]);
        match text_of(&g["type"]) {
            "tool_used" => {
                ensure(g["tool"].as_str().is_some(), || {
                    format!("{name}: tool is not a string")
                })?;
                for key in ["min", "max"] {
                    if yaml::has(g, key) {
                        ensure(int_of(&g[key]).is_some(), || {
                            format!("{name}: {key} is not an int")
                        })?;
                    }
                }
                // The harness defaults `min` to 1 (`min ?? 1`): a grader with only `max: 0`
                // reports "called 0x (expected 1..0)" and can never pass.
                if yaml::has(g, "max") && int_of(&g["max"]) == Some(0) {
                    ensure(yaml::has(g, "min") && int_of(&g["min"]) == Some(0), || {
                        format!("{name}: max: 0 needs min: 0")
                    })?;
                }
            }
            "tool_order" => {
                for side in ["before", "after"] {
                    let value = &g[side];
                    ensure(!value.is_badvalue(), || format!("{name}: no {side}"))?;
                    if value.as_hash().is_some() {
                        let extra = extras(&key_set(value), &["tool", "input_match"]);
                        ensure(extra.is_empty() && yaml::has(value, "tool"), || {
                            format!("{name}: bad {side} {value:?}")
                        })?;
                    } else {
                        ensure(value.as_str().is_some(), || {
                            format!("{name}: {side} is not a string")
                        })?;
                    }
                }
            }
            "regex" => {
                let target = &g["target"];
                ensure(!target.is_badvalue(), || format!("{name}: no target"))?;
                if target.as_hash().is_some() {
                    ensure(key_set(target) == set_of(&["source", "path"]), || {
                        format!("{name}: bad target keys")
                    })?;
                    ensure(target["source"].as_str() == Some("file"), || {
                        format!("{name}: target source is not file")
                    })?;
                    ensure(target["path"].as_str().is_some(), || {
                        format!("{name}: target path is not a string")
                    })?;
                } else {
                    ensure(
                        REGEX_TARGETS.iter().any(|t| target.as_str() == Some(*t)),
                        || format!("{name}: bad target {target:?}"),
                    )?;
                }
                if yaml::has(g, "flags") {
                    let ok = g["flags"]
                        .as_str()
                        .is_some_and(|f| Regex::new(r"^[dgimsuvy]+$").unwrap().is_match(f));
                    ensure(ok, || format!("{name}: bad flags {:?}", g["flags"]))?;
                }
                let matching = if yaml::has(g, "match") {
                    g["match"].as_str()
                } else {
                    Some("contains")
                };
                ensure(REGEX_MATCH.iter().any(|m| matching == Some(*m)), || {
                    format!("{name}: bad match {:?}", g["match"])
                })?;
                ensure(g["pattern"].as_str().is_some(), || {
                    format!("{name}: pattern is not a string")
                })?;
            }
            "file_exists" => {
                ensure(g["path"].as_str().is_some(), || {
                    format!("{name}: path is not a string")
                })?;
                ensure(matches!(g["exists"], Yaml::Boolean(_)), || {
                    format!("{name}: exists is not a bool")
                })?;
            }
            "llm" => {
                ensure(!text_of(&g["criteria"]).trim().is_empty(), || {
                    format!("{name}: criteria is blank")
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    #[test]
    fn test_prompts_have_no_absolute_paths() {
        for (name, case) in all_cases() {
            for bad in ABSOLUTE_PATHS {
                assert!(!prompt(&name, &case).contains(bad), "{name}: {bad}");
            }
        }
    }

    #[test]
    fn test_no_secret_looking_literals() {
        let sk = Regex::new(r"\bsk-").unwrap();
        for d in case_dirs() {
            let text = repo::read(&format!("evals/{d}/case.yaml"));
            for bad in FORBIDDEN_LITERALS {
                assert!(!text.contains(bad), "{d}: {bad}");
            }
            assert!(!sk.is_match(&text), "{d}");
        }
    }

    #[test]
    fn test_gitignore_excludes_the_results() {
        let text = repo::read(".gitignore");
        assert!(
            text.lines().any(|l| l.trim() == "evals/results/"),
            ".gitignore has no evals/results/ line"
        );
    }
}

mod roster_case_tests {
    use super::*;

    #[test]
    fn test_a_routing_case_per_agent_and_a_knowledge_case_per_non_worktree_agent() {
        let cases = all_cases();
        let startup = expected_startup();
        let wt = worktree_agents();
        for (agent, _) in &startup {
            assert!(has_case(&cases, &format!("route-{agent}")), "{agent}");
            if wt.contains(agent) {
                assert!(
                    !has_case(&cases, &format!("knowledge-{agent}")),
                    "plugin eval cannot run a worktree agent: {agent}"
                );
            } else {
                assert!(has_case(&cases, &format!("knowledge-{agent}")), "{agent}");
            }
        }
        assert_eq!(startup.len(), 12);
        let mut found: Vec<&str> = cases
            .iter()
            .filter_map(|(n, _)| n.strip_prefix("knowledge-"))
            .collect();
        found.sort_unstable();
        assert_eq!(found, knowledge_agents());
        assert!(!wt.is_empty());
    }

    // rows:computed one row per roster agent, read from the repo
    #[test]
    fn test_routing_cases_dispatch_the_agent() {
        let cases = all_cases();
        each_row(
            &expected_startup(),
            |(a, _)| format!("agent={a}"),
            |(agent, _)| {
                let c = case(&cases, &format!("route-{agent}"))?;
                ensure(str_list_has(&c["tags"], "routing"), || {
                    "tags lack routing".into()
                })?;
                let wanted = format!("\"anthology:{agent}\"");
                ensure(
                    yaml::items(&c["graders"]).iter().any(|g| {
                        dict_eq(
                            g,
                            &[
                                ("name", "dispatched"),
                                ("type", "tool_used"),
                                ("tool", "Agent"),
                                ("input_match", &wanted),
                            ],
                        )
                    }),
                    || format!("no `dispatched` grader for {wanted}"),
                )
            },
        );
    }

    // rows:computed one row per non-worktree roster agent, read from the repo
    #[test]
    fn test_knowledge_cases_read_the_startup_files_in_order() {
        let cases = all_cases();
        let startup = expected_startup();
        let preload = preload_agents();
        let rows: Vec<&(&str, Vec<String>)> = knowledge_agents()
            .iter()
            .map(|a| startup.iter().find(|(n, _)| n == a).unwrap())
            .collect();
        each_row(
            &rows,
            |r| format!("agent={}", r.0),
            |r| {
                let (agent, refs) = (r.0, &r.1);
                let c = case(&cases, &format!("knowledge-{agent}"))?;
                let list = yaml::items(&c["graders"]);
                let mut graders: HashMap<&str, &Yaml> = HashMap::new();
                for g in list {
                    if let Some(n) = g["name"].as_str() {
                        graders.insert(n, g);
                    }
                }
                let get = |n: &str| -> Result<&Yaml, String> {
                    graders
                        .get(n)
                        .copied()
                        .ok_or_else(|| format!("no grader {n}"))
                };
                let starting = |p: &str| -> Vec<&str> {
                    list.iter()
                        .filter_map(|g| g["name"].as_str())
                        .filter(|n| n.starts_with(p))
                        .collect()
                };
                let want_reads: Vec<String> = refs
                    .iter()
                    .map(|r| format!("read-{}", slug_of(r)))
                    .collect();
                ensure(starting("read-") == want_reads, || {
                    format!(
                        "read graders are {:?}, not {want_reads:?}",
                        starting("read-")
                    )
                })?;
                for r in refs {
                    let (slug, pattern) = (slug_of(r), path_regex_of(r));
                    let read = get(&format!("read-{slug}"))?;
                    let order = get(&format!("order-{slug}"))?;
                    ensure(
                        read["type"].as_str() == Some("tool_used")
                            && read["tool"].as_str() == Some("Read")
                            && read["input_match"].as_str() == Some(&pattern),
                        || format!("read-{slug} is not a Read of {pattern}"),
                    )?;
                    ensure(order["type"].as_str() == Some("tool_order"), || {
                        format!("order-{slug} is not a tool_order")
                    })?;
                    ensure(
                        dict_eq(
                            &order["before"],
                            &[("tool", "Read"), ("input_match", &pattern)],
                        ),
                        || format!("order-{slug} before is {:?}", order["before"]),
                    )?;
                    // the first Glob of the *.md listing, not any discovery Glob before it
                    ensure(dict_eq(&order["after"], &GLOB_ANCHOR), || {
                        format!("order-{slug} after is {:?}", order["after"])
                    })?;
                }
                let orders = graders.keys().filter(|n| n.starts_with("order-")).count();
                ensure(orders == refs.len(), || {
                    format!("{orders} order graders for {} files", refs.len())
                })?;
                // consecutive Startup files are read in the documented order
                let want_inorder: Vec<String> = refs
                    .windows(2)
                    .map(|w| format!("inorder-{}-{}", slug_of(&w[0]), slug_of(&w[1])))
                    .collect();
                ensure(starting("inorder-") == want_inorder, || {
                    format!("inorder graders are {:?}", starting("inorder-"))
                })?;
                for w in refs.windows(2) {
                    let name = format!("inorder-{}-{}", slug_of(&w[0]), slug_of(&w[1]));
                    let g = get(&name)?;
                    ensure(g["type"].as_str() == Some("tool_order"), || {
                        format!("{name} is not a tool_order")
                    })?;
                    ensure(
                        dict_eq(
                            &g["before"],
                            &[("tool", "Read"), ("input_match", &path_regex_of(&w[0]))],
                        ),
                        || format!("{name} before is {:?}", g["before"]),
                    )?;
                    ensure(
                        dict_eq(
                            &g["after"],
                            &[("tool", "Read"), ("input_match", &path_regex_of(&w[1]))],
                        ),
                        || format!("{name} after is {:?}", g["after"]),
                    )?;
                }
                let first = get("main-dispatches-first")?;
                ensure(
                    first["before"].as_str() == Some("Agent")
                        && dict_eq(&first["after"], &GLOB_ANCHOR),
                    || "main-dispatches-first is not Agent before the Glob anchor".into(),
                )?;
                let tools = &c["execution"]["allowed_tools"];
                if preload.contains(agent) {
                    // Skill is granted, so "no Skill call" means something, and the agent must
                    // quote the first heading of each preloaded skill (U1: the preload arrived).
                    ensure(str_list_has(tools, "Skill"), || {
                        "Skill is not granted".into()
                    })?;
                    let pni = get("preload-not-invoked")?;
                    ensure(
                        pni["tool"].as_str() == Some("Skill")
                            && int_of(&pni["min"]) == Some(0)
                            && int_of(&pni["max"]) == Some(0),
                        || "preload-not-invoked is not Skill min 0 max 0".into(),
                    )?;
                    let quoted = get("preload-quoted")?;
                    ensure(
                        quoted["type"].as_str() == Some("regex")
                            && quoted["target"].as_str() == Some("last_message"),
                        || "preload-quoted is not a last_message regex".into(),
                    )?;
                    for skill in &repo::roster_agent(agent).preloads {
                        let heading = skill_heading(skill)?;
                        ensure(text_of(&quoted["pattern"]).contains(&heading), || {
                            format!("preload-quoted lacks the heading of {skill}: {heading}")
                        })?;
                    }
                } else {
                    ensure(!str_list_has(tools, "Skill"), || {
                        "Skill is granted to a non-preload agent".into()
                    })?;
                    ensure(!graders.contains_key("preload-not-invoked"), || {
                        "preload-not-invoked on a non-preload agent".into()
                    })?;
                }
                Ok(())
            },
        );
    }

    // rows:computed one row per roster agent, read from the repo
    #[test]
    fn test_agent_files_agree_with_the_cases() {
        // Runs once the agents exist (stream A); until then each agent is skipped.
        each_row(
            &expected_startup_paths(),
            |(a, _)| format!("agent={a}"),
            |(agent, refs)| {
                let rel = format!("plugins/anthology/agents/{agent}.md");
                if !repo::path(&rel).is_file() {
                    eprintln!("skipped: {rel} does not exist yet");
                    return Ok(());
                }
                let got = startup_refs(&repo::read(&rel));
                ensure(got.as_ref() == Some(refs), || {
                    format!("{rel}: startup refs are {got:?}, not {refs:?}")
                })
            },
        );
    }
}

mod startup_parser_tests {
    use super::*;

    #[test]
    fn test_parses_plain_and_design_skills_references() {
        let text = "# Agent\n\n## Startup\n\n1. Read `${CLAUDE_PLUGIN_ROOT}/knowledge/caveman/SKILL.md`\n\
                    2. Read knowledge/design-skills/skills/ux-research/SKILL.md\n\n## Other\nknowledge/x/SKILL.md\n";
        assert_eq!(
            startup_refs(text),
            Some(vec!["caveman".to_string(), "ds/ux-research".to_string()])
        );
    }

    #[test]
    fn test_no_startup_section() {
        assert_eq!(startup_refs("# Agent\n"), None);
    }
}

mod required_case_tests {
    use super::*;

    #[test]
    fn test_required_cases_exist() {
        let cases = all_cases();
        for name in REQUIRED_CASES {
            assert!(has_case(&cases, name), "{name}");
        }
    }

    #[test]
    fn test_moved_cases_are_smokes_not_cases() {
        let cases = all_cases();
        for name in MOVED_TO_SMOKES {
            assert!(!has_case(&cases, name), "{name}");
            assert!(!repo::path(&format!("evals/{name}")).exists(), "{name}");
        }
        // omitted: smoke.py registers `"<name>": (smoke_` for each moved case (pins
        // evals/smoke/smoke.py) -> CO2 replacement: cutover_ci.rs asserts the Rust smoke runner
        // registers atl-rewrite and agent-memory-path
    }

    #[test]
    fn test_subagent_bash_cases_check_the_agent_ran_first() {
        // The main session also holds Bash, so the Bash call must come after the Agent dispatch.
        let cases = all_cases();
        {
            let (name, command) = ("hook-reach-subagent", "agent-browser");
            let c = case(&cases, name).unwrap();
            let order: Vec<&Yaml> = yaml::items(&c["graders"])
                .iter()
                .filter(|g| g["type"].as_str() == Some("tool_order"))
                .collect();
            assert_eq!(order.len(), 1, "{name}");
            assert_eq!(order[0]["before"].as_str(), Some("Agent"), "{name}");
            assert!(
                dict_eq(
                    &order[0]["after"],
                    &[("tool", "Bash"), ("input_match", command)]
                ),
                "{name}: {:?}",
                order[0]["after"]
            );
        }
    }

    #[test]
    fn test_the_guard_cases_say_what_they_cover() {
        let cases = all_cases();
        let desc = case(&cases, "guard-beats-atl").unwrap()["description"]
            .as_str()
            .unwrap_or("");
        assert!(desc.contains("atl-rewrite smoke"), "{desc}");
        let g6 = prompt("guard-g6", case(&cases, "guard-g6").unwrap());
        assert!(
            g6.contains("rule G6"),
            "the model must not refuse the probe write"
        );
        assert!(g6.contains("Do not refuse"));
        let main_only = prompt("guide-main-only", case(&cases, "guide-main-only").unwrap());
        assert!(main_only.contains("Step 1 word"));
    }

    #[test]
    fn test_skills_hidden_has_a_canary() {
        let cases = all_cases();
        let c = case(&cases, "skills-hidden").unwrap();
        assert!(
            yaml::items(&c["graders"])
                .iter()
                .any(|g| g["pattern"].as_str() == Some(r#""skills":\["#)),
            "no grader pattern is the skills canary"
        );
    }

    #[test]
    fn test_guard_cases_name_their_guard() {
        let cases = all_cases();
        for (name, guard) in [
            ("guard-g1", "G1"),
            ("guard-g4", "G4"),
            ("guard-g6", "G6"),
            ("guard-beats-atl", "G1"),
            ("hook-reach-subagent", "G4"),
        ] {
            let c = case(&cases, name).unwrap();
            let patterns: Vec<&str> = yaml::items(&c["graders"])
                .iter()
                .filter(|g| g["name"].as_str() == Some("blocked"))
                .map(|g| {
                    g["pattern"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{name}: blocked has no pattern"))
                })
                .collect();
            assert_eq!(
                patterns,
                vec![format!(r"anthology-guardrails \({guard}\)")],
                "{name}"
            );
        }
    }
}

mod mock_tests {
    use super::*;

    const MOCKS: &str = "evals/mocks/browseros-neo";

    fn tools_json() -> Value {
        let text = repo::read(&format!("{MOCKS}/_tools.json"));
        json::loads(&text).unwrap_or_else(|e| panic!("_tools.json: {}", e.py_str(&text)))
    }

    fn jtruthy(v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(Int::Small(i)) => *i != 0,
            Value::Int(Int::Big(_)) => true,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(m) => !m.is_empty(),
        }
    }

    fn tool_list(data: &Value) -> &[Value] {
        data.as_object()
            .and_then(|m| m.get("tools"))
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("_tools.json has no tools array"))
    }

    fn tool_names() -> BTreeSet<String> {
        let data = tools_json();
        tool_list(&data)
            .iter()
            .map(|t| {
                t.as_object()
                    .and_then(|m| m.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("a tool has no name"))
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn test_tools_json_is_well_formed() {
        let data = tools_json();
        let top: BTreeSet<&str> = data.as_object().unwrap().iter().map(|(k, _)| k).collect();
        assert_eq!(top, BTreeSet::from(["tools"]));
        for tool in tool_list(&data) {
            let obj = tool.as_object().unwrap_or_else(|| panic!("{tool:?}"));
            let get = |k: &str| obj.get(k).unwrap_or_else(|| panic!("{tool:?}: no {k}"));
            assert!(
                jtruthy(get("name")) && jtruthy(get("description")),
                "{tool:?}"
            );
            let schema_type = get("inputSchema")
                .as_object()
                .and_then(|m| m.get("type"))
                .and_then(Value::as_str);
            assert_eq!(schema_type, Some("object"), "{:?}", get("name"));
        }
    }

    #[test]
    fn test_every_mocked_tool_has_a_responder_and_the_reverse() {
        let responders: BTreeSet<String> = repo::list_dir(MOCKS)
            .iter()
            .filter_map(|n| n.strip_suffix(".md").map(str::to_string))
            .collect();
        assert_eq!(tool_names(), responders);
    }

    #[test]
    fn test_every_tool_a_grader_names_is_mocked() {
        let tools = tool_names();
        for (name, case) in all_cases() {
            for g in yaml::items(&case["graders"]) {
                for tool in grader_tools(g).into_iter().flatten() {
                    if let Some(short) = tool.strip_prefix(MCP_PREFIX) {
                        assert!(tools.contains(short), "{name}: {tool}");
                        assert!(
                            repo::path(&format!("{MOCKS}/{short}.md")).is_file(),
                            "{name}: {tool}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_the_cases_that_use_the_mock_grant_tool_search() {
        let cases = all_cases();
        for name in ["browser-browseros-only", "harness-sanity"] {
            let c = case(&cases, name).unwrap();
            assert!(
                str_list_has(&c["execution"]["allowed_tools"], "ToolSearch"),
                "{name}"
            );
        }
    }
}

mod grant_tests {
    use super::*;

    fn bash_grants(grants: &[String]) -> Vec<&str> {
        grants
            .iter()
            .filter_map(|g| g.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')))
            .collect()
    }

    #[test]
    fn test_grants_are_read() {
        let grants = run_script_grants();
        assert!(!bash_grants(&grants).is_empty());
        let shape = Regex::new(r"^(Bash|Write|Edit|NotebookEdit)\(.+\)$").unwrap();
        for grant in &grants {
            assert!(shape.is_match(grant), "{grant}");
        }
    }

    #[test]
    fn test_every_literal_bash_grant_is_used_by_a_prompt() {
        let grants = run_script_grants();
        let cases = all_cases();
        let prompts: Vec<&str> = cases.iter().map(|(n, c)| prompt(n, c)).collect();
        for command in bash_grants(&grants) {
            if command.contains('*') {
                // a wildcard grant is not a literal command a prompt can name
                continue;
            }
            assert!(
                prompts.iter().any(|p| p.contains(command)),
                "no case prompt runs: {command}"
            );
        }
    }

    // rows:computed one row per case that grants Bash and is not exempt, read from the repo
    #[test]
    fn test_every_bash_case_prompt_names_a_granted_command() {
        let grants = run_script_grants();
        let literal: Vec<&str> = bash_grants(&grants)
            .into_iter()
            .filter(|c| !c.contains('*'))
            .collect();
        let cases: Cases = all_cases()
            .into_iter()
            .filter(|(n, c)| {
                str_list_has(&c["execution"]["allowed_tools"], "Bash")
                    && !BASH_PROMPT_EXEMPT.contains(&n.as_str())
            })
            .collect();
        each_case(&cases, |name, case| {
            let p = prompt(name, case);
            ensure(literal.iter().any(|c| p.contains(c)), || {
                format!("the prompt names no granted command: {p}")
            })
        });
    }

    // A case may list `Bash(cmd)` / `Write(path)` itself, but only for a command the runner
    // grants. Today no case lists one, so this fails the moment one appears without a grant.
    #[test]
    fn test_pattern_entries_in_allowed_tools_equal_a_grant() {
        let grants = run_script_grants();
        let mut stray = Vec::new();
        for (name, case) in all_cases() {
            for tool in allowed_tools(&case) {
                if tool.contains('(') && !grants.contains(&tool) {
                    stray.push((name.clone(), tool));
                }
            }
        }
        assert!(stray.is_empty(), "{stray:?}");
    }

    // A `Write(...)`, `Edit(...)` or `NotebookEdit(...)` grant only matters for a case that
    // lists the tool.
    #[test]
    fn test_every_write_style_grant_is_held_by_a_case() {
        let held: BTreeSet<String> = all_cases()
            .iter()
            .flat_map(|(_, c)| allowed_tools(c))
            .collect();
        for grant in run_script_grants() {
            let tool = grant.split('(').next().unwrap();
            if tool != "Bash" {
                assert!(held.contains(tool), "{grant}");
            }
        }
    }

    #[test]
    fn test_the_grants_name_no_command_of_a_moved_case() {
        let text = run_script_grants().join("\n");
        for bad in ["atlrepo", "anthology-mem"] {
            assert!(!text.contains(bad), "{bad}");
        }
    }

    #[test]
    fn test_the_runner_keeps_npx_offline() {
        let text = repo::read("scripts/run-evals.sh");
        assert!(
            Regex::new(r"(?m)^export NPM_CONFIG_OFFLINE=true$")
                .unwrap()
                .is_match(&text)
        );
    }

    // The evals run `claude -p`, an SDK-style session: without host mode "full" the session
    // start would send no guide, and the cases that read it would test nothing.
    #[test]
    fn evals_run_in_full_host_mode() {
        let text = repo::read("scripts/run-evals.sh");
        assert!(
            Regex::new(r"(?m)^export ANTHOLOGY_HOST_MODE=full$")
                .unwrap()
                .is_match(&text)
        );
    }

    // A gated tool needs the case entry AND an operator grant; the grant is path-limited.
    #[test]
    fn test_plain_write_tools_have_a_narrow_operator_grant() {
        let grants = run_script_grants();
        for (name, case) in all_cases() {
            for tool in allowed_tools(&case) {
                if ["Write", "Edit", "NotebookEdit"].contains(&tool.as_str()) {
                    let prefix = format!("{tool}(.claude/memory/");
                    assert!(
                        grants.iter().any(|g| g.starts_with(&prefix)),
                        "{name}: {tool}"
                    );
                }
            }
        }
    }

    // agent-fields asserts `max: 0` on these tools; that only means something when the main
    // session has them.
    #[test]
    fn test_the_tool_field_cases_hold_the_tools_they_check() {
        let cases = all_cases();
        let c = case(&cases, "agent-fields").unwrap();
        for tool in ["Edit", "Write", "NotebookEdit"] {
            assert!(
                str_list_has(&c["execution"]["allowed_tools"], tool),
                "{tool}"
            );
        }
    }

    #[test]
    fn test_every_write_case_prompt_names_a_granted_path() {
        let grants = run_script_grants();
        let prefixes: Vec<&str> = grants
            .iter()
            .filter_map(|g| g.strip_prefix("Write("))
            .map(|g| g.strip_suffix(')').unwrap_or(g).trim_end_matches('*'))
            .collect();
        assert!(!prefixes.is_empty());
        for (name, case) in all_cases() {
            if str_list_has(&case["execution"]["allowed_tools"], "Write")
                && !WRITE_PROMPT_EXEMPT.contains(&name.as_str())
            {
                let p = prompt(&name, &case);
                assert!(prefixes.iter().any(|x| p.contains(x)), "{name}");
            }
        }
    }
}

mod workflow_tests {
    use super::*;

    fn workflow() -> Yaml {
        yaml::load_file(".github/workflows/evals.yml")
    }

    // `on:` is the string key "on" under YAML 1.2 (PyYAML read it as True).
    #[test]
    fn test_triggers_are_schedule_and_dispatch_only() {
        let wf = workflow();
        let triggers = &wf["on"];
        assert_eq!(
            key_set(triggers),
            set_of(&["schedule", "workflow_dispatch"])
        );
        assert_eq!(yaml::items(&triggers["schedule"]).len(), 1);
        let inputs = &triggers["workflow_dispatch"]["inputs"];
        assert_eq!(key_set(inputs), set_of(&["max_cost_usd"]));
        assert_eq!(inputs["max_cost_usd"]["default"].as_str(), Some("20"));
    }

    #[test]
    fn test_permissions() {
        let wf = workflow();
        assert!(wf["permissions"].as_hash().is_some_and(|h| h.is_empty()));
        assert!(dict_eq(
            &wf["jobs"]["evals"]["permissions"],
            &[("contents", "read")]
        ));
    }

    #[test]
    fn test_one_job_guarded_by_repository_with_a_timeout() {
        let wf = workflow();
        let job = &wf["jobs"]["evals"];
        assert_eq!(yaml::keys(&wf["jobs"]), vec!["evals".to_string()]);
        // `keys` skips non-string keys; Python's `list(jobs)` would not.
        assert_eq!(wf["jobs"].as_hash().map(|h| h.len()), Some(1));
        assert_eq!(
            job["if"].as_str(),
            Some("github.repository == 'rbonweb/anthology'")
        );
        assert_eq!(job["runs-on"].as_str(), Some("ubuntu-latest"));
        // the evals (up to 120 min of cases) plus the smokes
        assert_eq!(job["timeout-minutes"].as_i64(), Some(180));
        let conc = &wf["concurrency"];
        assert_eq!(key_set(conc), set_of(&["group", "cancel-in-progress"]));
        assert_eq!(conc["group"].as_str(), Some("evals"));
        assert_eq!(conc["cancel-in-progress"].as_bool(), Some(false));
    }

    #[test]
    fn test_secret_is_read_only_through_env() {
        let wf = workflow();
        let steps = yaml::items(&wf["jobs"]["evals"]["steps"]);
        for step in steps {
            for field in ["run", "uses", "with", "if", "name"] {
                assert!(
                    !contains_text(&step[field], "secrets."),
                    "secret outside env: {step:?}"
                );
            }
        }
        let key = &steps[0];
        assert_eq!(key["id"].as_str(), Some("key"));
        assert!(dict_eq(
            &key["env"],
            &[("ANTHROPIC_API_KEY", "${{ secrets.ANTHROPIC_API_KEY }}")]
        ));
        let run = text_of(&key["run"]);
        assert!(run.contains("present=true"));
        assert!(run.contains("present=false"));
        assert!(
            run.contains("::notice::ANTHROPIC_API_KEY is not set; the behaviour evals are skipped")
        );
    }

    #[test]
    fn test_every_other_step_is_gated_on_the_key() {
        let wf = workflow();
        for step in &yaml::items(&wf["jobs"]["evals"]["steps"])[1..] {
            assert!(text_of(&step["if"]).contains(KEY_GATE), "{step:?}");
        }
    }

    #[test]
    fn test_installs_the_pinned_claude_code_and_runs_the_script() {
        let wf = workflow();
        let steps = yaml::items(&wf["jobs"]["evals"]["steps"]);
        let runs: Vec<&str> = steps.iter().map(|s| text_of(&s["run"])).collect();
        assert!(
            runs.iter()
                .any(|r| r.contains("npm ci --prefix ci/claude-code"))
        );
        assert!(runs.iter().any(|r| r.contains("ci/jobs/evals.sh --ci")));
        let run_step = steps
            .iter()
            .find(|s| text_of(&s["run"]).contains("ci/jobs/evals.sh --ci"))
            .unwrap();
        let env = &run_step["env"];
        assert_eq!(
            env["CLAUDE_BIN"].as_str(),
            Some("ci/claude-code/node_modules/.bin/claude")
        );
        assert_eq!(env["DISABLE_AUTOUPDATER"].as_str(), Some("1"));
        // a regressed guard must not let a granted npx fetch packages
        assert_eq!(env["NPM_CONFIG_OFFLINE"].as_str(), Some("true"));
        // `plugin eval` needs the credential in its children (scrubbing fails every case "Not
        // logged in") and hides it from their tools itself; the smokes build their own
        // environment.
        assert_eq!(env["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"].as_str(), Some("0"));
        assert!(text_of(&env["MAX_COST_USD"]).contains("inputs.max_cost_usd"));
        assert!(text_of(&run_step["run"]).contains(r#"--max-cost-usd "$MAX_COST_USD""#));
    }

    #[test]
    fn test_results_are_uploaded_even_after_a_failure() {
        let wf = workflow();
        let upload = yaml::items(&wf["jobs"]["evals"]["steps"])
            .iter()
            .find(|s| text_of(&s["uses"]).starts_with("actions/upload-artifact@"))
            .expect("no upload-artifact step");
        assert_eq!(
            upload["if"].as_str(),
            Some(format!("always() && {KEY_GATE}").as_str())
        );
        assert_eq!(upload["with"]["path"].as_str(), Some("evals/results/"));
        assert_eq!(upload["with"]["retention-days"].as_i64(), Some(30));
        assert_eq!(upload["with"]["if-no-files-found"].as_str(), Some("ignore"));
    }
}
