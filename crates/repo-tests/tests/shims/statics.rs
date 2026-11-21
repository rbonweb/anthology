//! Static rules on the shipped shell files (design §8.1, §9.1, §9.2; R2, R3, R14).

use anthology_pycompat::json::{self, Value};
use repo_tests::repo;

const STAGING: &str = "port/staging/plugins";

/// lib.sh's NODE_GET, byte for byte (design §8.4 step 6.5).
const NODE_GET: &str = r#"const h=require("https"),f=require("fs"),[,u,o]=process.argv,M=67108864;let n=0;const q=c=>process.exit(c);setTimeout(()=>q(1),60000);function g(x){let p;try{p=new URL(x)}catch(e){return q(1)}if(p.protocol!=="https:")return q(1);h.get(p,{headers:{"user-agent":"anthology-bootstrap"}},r=>{const s=r.statusCode,l=r.headers.location;if(s>=300&&s<400&&l){r.resume();if(++n>5)return q(1);let t;try{t=new URL(l,p).href}catch(e){return q(1)}return g(t)}if(s!==200||Number(r.headers["content-length"]||0)>M){r.resume();return q(1)}let z=0;const w=f.createWriteStream(o);r.on("data",c=>{z+=c.length;if(z>M)q(1)});r.on("error",()=>q(1));r.on("aborted",()=>q(1));w.on("error",()=>q(1));w.on("close",()=>q(r.complete?0:1));r.pipe(w)}).on("error",()=>q(1))}g(u)"#;

fn staged(rel: &str) -> String {
    repo::read(&format!("{STAGING}/{rel}"))
}

/// The executable lines: no blank lines, no comment lines.
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect()
}

/// `exit` used as a command word in `line` (not `process.exit`, not inside a word).
fn exit_words(line: &str) -> Vec<String> {
    // The argument stops at a quote too, so a trap string's `…; exit 2'` reads as `exit 2`; a
    // quoted argument such as `exit "$rc"` leaves a bare `exit`, which the callers reject.
    let re = regex::Regex::new(r#"(^|[\s;&|(){}])exit(\s+[^\s;&|)'"]+)?"#).unwrap();
    re.captures_iter(line)
        .map(|c| {
            c.get(0)
                .unwrap()
                .as_str()
                .trim_start_matches([' ', ';', '&', '|', '(', ')', '{', '}', '\t'])
                .to_string()
        })
        .collect()
}

#[test]
fn lib_and_bootstrap_are_byte_identical_in_both_plugins() {
    for f in ["hooks/lib.sh", "hooks/bootstrap.sh"] {
        let a = std::fs::read(repo::path(&format!("{STAGING}/anthology/{f}"))).unwrap();
        let g = std::fs::read(repo::path(&format!("{STAGING}/anthology-guardrails/{f}"))).unwrap();
        assert!(a == g, "{f} differs between the plugins");
    }
}

#[test]
fn lib_sh_exits_only_inside_the_watchdog() {
    let lib = staged("anthology/hooks/lib.sh");
    let allowed = ["        trap 'kill \"$_w_s\" 2>/dev/null; exit 0' TERM"];
    let mut seen = 0;
    for line in code_lines(&lib) {
        if line.starts_with("ANTH_NODE_GET=") {
            continue;
        }
        if !exit_words(line).is_empty() {
            assert!(
                allowed.contains(&line),
                "lib.sh has an exit outside the watchdog: {line}"
            );
            seen += 1;
        }
    }
    assert_eq!(seen, 1, "the watchdog's exit");
    // And the watchdog's subshell is where that line is.
    let i = lib.find(allowed[0]).unwrap();
    assert!(
        lib[..i].ends_with("        _w_s=\n"),
        "the exit sits in the watchdog subshell"
    );
}

/// guard.sh sources lib.sh into its own shell, so nothing in lib.sh may end, replace or
/// re-route that shell: no top-level command at all (only function definitions and `ANTH_`
/// assignments), and `exec`, `trap` and `set -e` only where the watchdog's background subshell
/// uses them.
#[test]
fn lib_sh_cannot_end_or_replace_the_sourcing_shell() {
    let lib = staged("anthology/hooks/lib.sh");
    let def = regex::Regex::new(r"^[a-z_][a-z0-9_]*\(\) \{$").unwrap();
    let assign = regex::Regex::new(r"^ANTH_[A-Z0-9_]+=").unwrap();
    let mut prev = "";
    for line in lib.lines() {
        let top = !line.is_empty() && !line.starts_with([' ', '\t', '#']);
        if top {
            assert!(
                def.is_match(line)
                    || line == "}"
                    || assign.is_match(line)
                    || (line == "'" && prev == "ANTH_NL='"),
                "lib.sh: a top-level command runs in the sourcing shell: {line}"
            );
        }
        prev = line;
    }
    let word = |w: &str| regex::Regex::new(&format!(r"(^|[\s;&|(){{}}]){w}(\s|$)")).unwrap();
    let (exec, trap) = (word("exec"), word("trap"));
    let mut seen = (0, 0);
    for line in code_lines(&lib) {
        if line.starts_with("ANTH_NODE_GET=") {
            continue;
        }
        if exec.is_match(line) {
            assert_eq!(
                line, "        exec </dev/null >/dev/null 2>&1",
                "lib.sh exec"
            );
            seen.0 += 1;
        }
        if trap.is_match(line) {
            assert_eq!(
                line, "        trap 'kill \"$_w_s\" 2>/dev/null; exit 0' TERM",
                "lib.sh trap"
            );
            seen.1 += 1;
        }
        assert!(
            !line.contains("set -e") && !line.contains("errexit"),
            "lib.sh: no set -e: {line}"
        );
    }
    assert_eq!(seen, (1, 1), "the watchdog's exec and trap");
    // Both sit in the watchdog's `( … ) &` subshell, before its sleep.
    let start = lib
        .find("    (\n        exec </dev/null >/dev/null 2>&1\n")
        .expect("watchdog");
    let rest = &lib[start..];
    assert!(rest.find("trap ").unwrap() < rest.find("    ) &").unwrap());
}

#[test]
fn guard_sh_has_exactly_two_allow_sites_and_falls_through_to_a_block() {
    let text = staged("anthology-guardrails/hooks/guard.sh");
    let lines = code_lines(&text);
    let mut zeros = Vec::new();
    for line in &lines {
        for w in exit_words(line) {
            match w.as_str() {
                "exit 0" => zeros.push(line.trim().to_string()),
                "exit 2" => {}
                other => panic!("guard.sh: `{other}` in `{line}`: only exit 0 or exit 2"),
            }
        }
        let t = line.trim();
        assert!(
            !t.starts_with("exec ") && !t.contains(" exec ") && t != "exec",
            "guard.sh must not exec: {line}"
        );
        assert!(
            !t.starts_with("set -e") && !t.contains("set -e"),
            "no set -e: {line}"
        );
        assert!(
            !t.contains("exit $") && !t.contains("exit \"$"),
            "no exit $var: {line}"
        );
    }
    assert_eq!(zeros.len(), 2, "exit 0 sites: {zeros:?}");
    // The allow sites: the token (exact, with one newline) and the read-only allowlist.
    assert!(
        text.contains(
            "if [ \"$ANTH_RC\" = 0 ] && [ \"$ANTH_ERR\" = \"anthology-guard:allow:v1$anth_nl\" ]; then"
        ),
        "the token comparison"
    );
    assert!(
        text.contains("if anth_readonly \"$anth_input\"; then"),
        "the allowlist"
    );
    assert!(text.contains("anth_nl='\n'\n"), "anth_nl is one newline");
    let n = lines.len();
    assert_eq!(lines[n - 1], "exit 2", "the last statement");
    assert_eq!(
        lines[n - 2],
        format!("printf '%s\\n' '{}' >&2", super::common::FALLBACK),
        "the fallback block"
    );
}

#[test]
fn the_node_downloader_is_the_fixed_constant() {
    let lib = staged("anthology/hooks/lib.sh");
    let line = lib
        .lines()
        .find(|l| l.starts_with("ANTH_NODE_GET='"))
        .expect("ANTH_NODE_GET line");
    let value = line
        .strip_prefix("ANTH_NODE_GET='")
        .and_then(|v| v.strip_suffix('\''))
        .unwrap();
    assert_eq!(value, NODE_GET);
    assert!(
        !NODE_GET.contains('\''),
        "no single quote inside the sh literal"
    );
}

fn hooks(plugin: &str) -> Value {
    json::loads(&staged(&format!("{plugin}/hooks/hooks.json"))).unwrap()
}

#[test]
fn guardrails_hooks_json_is_design_9_1() {
    let want = json::loads(
        r#"{"hooks": {
          "SessionStart": [{"hooks": [{"type": "command", "command": "sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/bootstrap.sh", "session", "--plugin", "guardrails"], "timeout": 60}]}],
          "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/guard.sh"], "timeout": 60, "onFailure": "block"}]}]
        }}"#,
    )
    .unwrap();
    assert_eq!(hooks("anthology-guardrails"), want);
}

#[test]
fn anthology_hooks_json_runs_session_start_sh_and_keeps_rtk_hook() {
    let want = json::loads(
        r#"{"hooks": {
          "SessionStart": [{"hooks": [{"type": "command", "command": "sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"], "timeout": 60}]}],
          "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/bin/rtk-hook"], "timeout": 60, "onFailure": "block"}]}]
        }}"#,
    )
    .unwrap();
    assert_eq!(hooks("anthology"), want);
    // The rtk entry is the live one, unchanged.
    let live = json::loads(&repo::read("plugins/anthology/hooks/hooks.json")).unwrap();
    let pre = |v: &Value| {
        let hooks = v.as_object().unwrap().get("hooks").unwrap();
        hooks
            .as_object()
            .unwrap()
            .get("PreToolUse")
            .unwrap()
            .clone()
    };
    assert_eq!(pre(&hooks("anthology")), pre(&live));
}

#[test]
fn every_shim_is_posix_sh_with_a_shebang() {
    for rel in [
        "anthology/hooks/bootstrap.sh",
        "anthology/hooks/session-start.sh",
        "anthology/bin/anthology",
        "anthology/bin/anthology-mem",
        "anthology-guardrails/hooks/guard.sh",
        "anthology-guardrails/hooks/bootstrap.sh",
    ] {
        let text = staged(rel);
        assert!(text.starts_with("#!/bin/sh\n"), "{rel}: #!/bin/sh");
        let code = code_lines(&text).join("\n");
        for bash_only in [
            "[[",
            "local ",
            "function ",
            "$'",
            "<<<",
            "pipefail",
            "BASH_SOURCE",
        ] {
            assert!(!code.contains(bash_only), "{rel}: {bash_only}");
        }
        let mode = std::fs::metadata(repo::path(&format!("{STAGING}/{rel}")))
            .unwrap()
            .permissions();
        use std::os::unix::fs::PermissionsExt;
        assert!(mode.mode() & 0o111 != 0, "{rel} is executable");
    }
    let lib = staged("anthology/hooks/lib.sh");
    let code: Vec<&str> = code_lines(&lib)
        .into_iter()
        .filter(|l| !l.starts_with("ANTH_NODE_GET="))
        .collect();
    let code = code.join("\n");
    for bash_only in [
        "[[",
        "local ",
        "function ",
        "$'",
        "<<<",
        "pipefail",
        "BASH_SOURCE",
    ] {
        assert!(!code.contains(bash_only), "lib.sh: {bash_only}");
    }
}

#[test]
fn the_wrappers_differ_only_in_name_and_the_mem_argument() {
    let a = staged("anthology/bin/anthology");
    let m = staged("anthology/bin/anthology-mem");
    assert!(a.contains("export RTK_SUPPRESS_HOOK_WARNING"));
    let norm = |s: &str| {
        code_lines(s)
            .into_iter()
            .map(|l| {
                l.replace("anth_name=anthology-mem", "anth_name=anthology")
                    .replace("\"$ANTH_BIN\" mem ", "\"$ANTH_BIN\" ")
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(norm(&a), norm(&m));
    assert_eq!(m.matches("exec \"$ANTH_BIN\" mem \"$@\"").count(), 2);
}
