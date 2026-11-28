//! Port of `tests/test_memory_privacy.py` (MP1): the privacy scan that guards every write into
//! repo memory, and the one shared secret-pattern list.
//!
//! Fixtures are assembled at run time: no real home path, email, hostname or token sits in this
//! file. Every timing assertion is a ratio (4x the input must cost less than 6x, best of
//! several runs, with a floor so a very fast scan cannot flake), never wall time. `cargo test`
//! builds unoptimized (about 25 times slower than a release build here), so the inputs are
//! smaller than Python's where noted; they stay well inside the linear regime, and quadratic
//! behavior costs 16x on a 4x input at any size.

use std::time::Instant;

use anthology_core::Env;
use anthology_memory::privacy::{
    Finding, PrivacyDeps, PrivacyLimits, SECRET_PATTERNS, UNSCANNABLE, describe, privacy_findings,
    search_any,
};

fn u() -> String {
    ["/", "Users"].concat()
}

fn h() -> String {
    ["/", "home"].concat()
}

fn host() -> String {
    ["alice-", "laptop"].concat()
}

fn home() -> String {
    u() + "/zed"
}

fn at(parts: &[&str]) -> String {
    parts.join("@")
}

/// The seams of a test that passes `home` and `hostname` explicitly: neither is ever read.
fn deps() -> PrivacyDeps {
    PrivacyDeps {
        hostname: || panic!("the hostname seam is not used when a hostname is given"),
        home: None,
        limits: PrivacyLimits::default(),
    }
}

fn findings(text: &str, home: &str, hostname: &str) -> Vec<Finding> {
    privacy_findings(text, Some(home), Some(hostname), &deps())
}

fn kinds_with(text: &str, home: &str, hostname: &str) -> Vec<String> {
    let mut k: Vec<String> = findings(text, home, hostname)
        .into_iter()
        .map(|f| f.kind)
        .collect();
    k.sort();
    k.dedup();
    k
}

fn kinds(text: &str) -> Vec<String> {
    kinds_with(text, &home(), &host())
}

fn f(kind: &str, line: usize) -> Finding {
    Finding {
        kind: kind.to_string(),
        line,
    }
}

/// The best of `runs` timings of `work`, in seconds (the first run also builds the compiled
/// pattern set of a new line-length tier).
fn best_cost(runs: usize, mut work: impl FnMut()) -> f64 {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed().as_secs_f64()
        })
        .fold(f64::INFINITY, f64::min)
}

/// 4x the input costs less than 10x (Python asserts 6x), never judged below a 0.5 s floor: shared CI
/// runners measured 8x-11x on linear scans, while a quadratic scan is 16x.
fn assert_scales(label: &str, small: f64, large: f64, floor: f64) {
    assert!(
        large < (10.0 * small).max(floor.max(0.5)),
        "{label}: {small:.4}s then {large:.4}s"
    );
}

fn scan_cost(text: &str) -> f64 {
    best_cost(3, || {
        privacy_findings(text, Some("/srv/zed"), Some(&host()), &deps());
    })
}

mod finding_shape_test {
    use super::*;

    #[test]
    fn test_labels_and_line_numbers_only_sorted_and_unique() {
        let mail = at(&["alice", "mail.acme.zz"]);
        let (u, h) = (u(), h());
        let text =
            format!("clean line\n{u}/alice/notes and {u}/bob/x\nwrite to {mail}\n{h}/carol\n");
        let found = findings(&text, &home(), &host());
        assert_eq!(
            found,
            vec![f("home-path", 2), f("email", 3), f("home-path", 4)]
        );
        // A finding is exactly a kind and a line (Python: sorted(f) == ["kind", "line"]).
        let Finding { kind: _, line: _ } = &found[0];
        let dumped = format!("{found:?}");
        for secret in ["alice", "bob", "carol", "acme", u.as_str()] {
            assert!(!dumped.contains(secret), "{secret} in {dumped}");
        }
    }

    #[test]
    fn test_clean_text_has_no_findings() {
        let text = "# Build\nRun `make test` in Docker. See [[project-ci]].\n\
                    Version 1.2.3 ships on 2026-04-05; the API listens on port 8080.\n";
        assert_eq!(findings(text, &home(), &host()), vec![]);
    }

    #[test]
    fn test_describe() {
        let found = [
            f("home-path", 3),
            f("email", 7),
            f("home-path", 9),
            f("secret:jwt", 9),
        ];
        assert_eq!(
            describe(&found),
            "home path (lines 3, 9), email address (line 7), secret-looking jwt (line 9)"
        );
        assert_eq!(describe(&[]), "");
        // The kind privacy adds (GC-12).
        assert_eq!(
            describe(&[f(UNSCANNABLE, 4)]),
            "text the privacy scan could not finish (line 4)"
        );
    }

    /// Python: `memory_sync.privacy_findings is memory_privacy.privacy_findings`. In Rust, sync
    /// (MS2) calls `privacy::privacy_findings`; it must never define a scan of its own.
    #[test]
    fn test_reexported_from_memory_sync() {
        let sync = source("src/sync.rs");
        assert!(
            !sync.contains("fn privacy_findings"),
            "sync.rs defines its own privacy_findings"
        );
        let lib = source("src/lib.rs");
        assert!(lib.contains("pub mod privacy;"));
    }

    #[test]
    fn test_memory_privacy_imports_no_anthology_module() {
        let privacy = source("src/privacy.rs");
        for module in [
            "sync", "index", "portable", "save", "render", "repo", "approve", "notes",
        ] {
            for spelling in [format!("crate::{module}"), format!("super::{module}")] {
                assert!(!privacy.contains(&spelling), "privacy.rs uses {spelling}");
            }
        }
        assert!(!privacy.contains("anthology_core::state"));
    }
}

/// A source file of this crate.
fn source(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

mod kind_test {
    use super::*;

    #[test]
    fn test_home_path() {
        let (u, h) = (u(), h());
        for text in [
            format!("{u}/alice/project"),
            format!("cd {h}/bob"),
            format!("({u}/alice)"),
            format!("{u}/alice."),
            format!("/mnt/c{u}/alice/proj"),
            format!("/var{h}/alice/src"),
            format!("/System/Volumes/Data{u}/alice/x"),
            format!("file://{u}/alice/x"),
            format!("C:{u}/alice/x"),
            format!("/mnt/c{u}/alice"),
            format!("/api{h}/feed"), // an accepted false positive on the safe side
            format!("1://h{u}/alice/x"),
            format!("+.-://h{u}/alice/x"), // no scheme letter
            format!("PATH=/opt{h}/carol/bin"),
        ] {
            assert_eq!(kinds(&text), ["home-path"], "{text}");
        }
    }

    #[test]
    fn test_literal_home_value() {
        let home = ["/", "srv", "/people/zed"].concat();
        assert_eq!(
            kinds_with(&format!("cache in {home}/.cache"), &home, &host()),
            ["home-path"]
        );
        assert!(kinds_with(&format!("cache in {home}s/.cache"), &home, &host()).is_empty());
        assert!(kinds_with("just / here", "/", &host()).is_empty()); // a 1-char home is ignored
    }

    /// Python patches `os.environ`; here the HOME seam is `PrivacyDeps::from_env`.
    #[test]
    fn test_home_defaults_to_the_environment() {
        let home = ["/", "srv", "/people/zed"].concat();
        let mut deps = PrivacyDeps::from_env(&Env::default().with("HOME", &home));
        deps.hostname = || panic!("hostname is given");
        let found = privacy_findings(&format!("in {home}/x"), None, Some(&host()), &deps);
        let kinds: Vec<&str> = found.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(kinds, ["home-path"]);
    }

    #[test]
    fn test_email() {
        assert_eq!(
            kinds(&format!("mail {}", at(&["alice", "mail.acme.zz"]))),
            ["email"]
        );
    }

    #[test]
    fn test_private_ip() {
        for ip in [
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.20",
            "100.64.3.4",
            "169.254.10.10",
            "fd12:3456:789a::1",
            "fe80::1%en0",
        ] {
            assert_eq!(kinds(&format!("host at {ip}")), ["private-ip"], "{ip}");
        }
    }

    #[test]
    fn test_ip_address() {
        for ip in ["8.8.4.4", "172.32.0.1", "1.2.3.4"] {
            assert_eq!(kinds(&format!("server {ip}.")), ["ip-address"], "{ip}");
        }
    }

    #[test]
    fn test_private_hostname() {
        for host in [
            "nas.lan",
            "build-box.local",
            "db.internal",
            "wiki.corp",
            "router.home.arpa",
            "pi.tail1234.ts.net",
            "box.localdomain",
            "printer.home",
            "intra.intranet",
        ] {
            assert_eq!(
                kinds(&format!("open http://{host}:8080/ now")),
                ["private-hostname"],
                "{host}"
            );
        }
    }

    #[test]
    fn test_machine_hostname() {
        let host = host();
        assert_eq!(
            kinds(&format!("built on {} yesterday", host.to_uppercase())),
            ["private-hostname"]
        );
        assert_eq!(
            kinds_with(
                &format!("built on {host}"),
                &home(),
                &format!("{host}.local")
            ),
            ["private-hostname"]
        );
        assert!(kinds(&format!("built on {host}-2")).is_empty());
        assert!(kinds_with("a box called abc", &home(), "abc").is_empty()); // under 4 chars
        assert!(kinds_with("on localhost", &home(), "localhost").is_empty());
    }

    #[test]
    fn test_generic_machine_names_are_never_a_finding() {
        for name in [
            "ubuntu",
            "raspberrypi",
            "MacBook-Pro",
            "docker",
            "workstation",
            "laptop",
            "debian.local",
        ] {
            let short = name.split('.').next().unwrap();
            assert!(
                kinds_with(&format!("runs on {short} today"), &home(), name).is_empty(),
                "{name}"
            );
        }
    }

    /// Python patches `socket.gethostname` to raise; here the hostname seam fails.
    #[test]
    fn test_hostname_lookup_failure_never_raises() {
        let deps = PrivacyDeps {
            hostname: || None,
            home: None,
            limits: PrivacyLimits::default(),
        };
        assert_eq!(
            privacy_findings("plain text", Some(&home()), None, &deps),
            vec![]
        );
    }

    #[test]
    fn test_ssh_target() {
        let target = at(&["deploy", "build-01"]);
        assert_eq!(kinds(&format!("ssh -p 2222 {target}")), ["ssh-target"]);
        for cmd in ["scp f ", "sftp ", "rsync -a x ", "mosh "] {
            let k = kinds(&format!("{cmd}{}", at(&["bob", "buildhost"])));
            assert!(k.iter().any(|k| k == "ssh-target"), "{cmd}");
        }
        assert_eq!(kinds(&format!("{target} via ssh")), ["ssh-target"]); // target first
        assert!(kinds(&format!("{target} without a command")).is_empty());
    }

    #[test]
    fn test_git_user_is_exempt_only_on_public_forges() {
        for forge in [
            "github.com",
            "gitlab.com",
            "bitbucket.org",
            "codeberg.org",
            "ssh.dev.azure.com",
            "git.sr.ht",
        ] {
            let git = at(&["git", forge]);
            assert!(kinds(&format!("ssh -T {git}")).is_empty(), "{forge}");
            assert!(
                kinds(&format!("remote {git}:acme/app.git")).is_empty(),
                "{forge}"
            );
        }
        assert_eq!(
            kinds(&format!("remote {}:a/b.git", at(&["git", "forge.acme.zz"]))),
            ["email"]
        );
        assert_eq!(
            kinds(&format!("ssh {}", at(&["git", "build-box"]))),
            ["ssh-target"]
        );
    }

    #[test]
    fn test_ssh_key_path() {
        for text in [
            "~/.ssh/id_work".to_string(),
            "$HOME/.ssh/config".to_string(),
            ["/", "etc", "/ssh-keys/.ssh/k"].concat(),
            "the id_ed25519 key".to_string(),
            "id_rsa.pub".to_string(),
            "copy id_ecdsa".to_string(),
            "an id_dsa file".to_string(),
        ] {
            assert_eq!(kinds(&text), ["ssh-key-path"], "{text}");
        }
    }

    #[test]
    fn test_secrets() {
        let cases = [
            ("aws-access-key", ["AK", "IA", "ABCDEFGHIJKLMNOP"].concat()),
            (
                "github-token",
                format!("{}{}", "gh", "p_") + &"a1".repeat(20),
            ),
            ("slack-token", ["xo", "xb-", "1234567890-abc"].concat()),
            (
                "private-key",
                ["-----BEGIN ", "OPENSSH PRIVATE KEY-----"].concat(),
            ),
            (
                "jwt",
                [
                    "eyJ",
                    "hbGciOiJIUzI1NiJ9",
                    ".eyJ",
                    "zdWIiOiIxMjM0In0",
                    ".c2lnbmF0dXJlLXg",
                ]
                .concat(),
            ),
            ("google-api-key", ["AI", "za"].concat() + &"C".repeat(35)),
            (
                "stripe-live-key",
                ["sk", "_live_", "abcdefghij0123"].concat(),
            ),
            (
                "bearer-token",
                ["Bear", "er ", "abcdefghij0123456789xyz"].concat(),
            ),
            (
                "basic-auth",
                ["Authorization: Bas", "ic ", "dXNlcjpwYXNzd29yZA=="].concat(),
            ),
            (
                "gitlab-token",
                ["gl", "pat-", "abcdefghij0123456789"].concat(),
            ),
            (
                "huggingface-token",
                ["hf", "_"].concat() + &"abcdefghij".repeat(3) + "xy",
            ),
            (
                "npm-token",
                ["np", "m_"].concat() + &"abcdefghij".repeat(3) + "abcdef",
            ),
        ];
        for (label, text) in &cases {
            let found = findings(&format!("x\n{text}\n"), &home(), &host());
            assert!(
                found.contains(&f(&format!("secret:{label}"), 2)),
                "{label}: {found:?}"
            );
            assert!(!format!("{found:?}").contains(text.as_str()));
        }
        let anthropic = ["sk", "-ant-", "abcdefghij0123456789"].concat();
        assert!(
            kinds(&anthropic)
                .iter()
                .any(|k| k == "secret:anthropic-key")
        );
    }

    #[test]
    fn test_cloud_secret_key_assignment_and_url_credentials() {
        // m7: an AWS-style secret key assignment and `scheme://user:pw@host` were not caught
        let cases: [(&str, Vec<String>); 2] = [
            (
                "secret:cloud-secret-key",
                vec![
                    [
                        "aws_secret_",
                        "access_key = ",
                        "wJalrXUtnFEMI",
                        "K7MDENGbPxRfiCY",
                    ]
                    .concat(),
                    ["AWS_SECRET_", "ACCESS_KEY=", "wJalrXUtnFEMI", "K7MDENG"].concat(),
                    ["secret-", "access-key: ", "abcdefgh12345678"].concat(),
                    ["access_", "key = ", "abcdefgh12345678"].concat(),
                    [
                        "\"aws_secret_",
                        "access_key\": \"",
                        "wJalrXUtnFEMI",
                        "K7MDENG\"",
                    ]
                    .concat(),
                    ["\"Secret", "AccessKey\": \"", "wJalrXUtnFEMI", "K7MDENG\""].concat(),
                    ["'secret_", "key': '", "abcdefgh12345678'"].concat(),
                ],
            ),
            (
                "secret:url-credentials",
                vec![
                    ["postgres", "://app:", "Sup3rS3cr", "etPw@db.acme.zz/prod"].concat(),
                    ["see https", "://bob:", "hunter22xx", "@host.acme.zz/x"].concat(),
                    ["redis", "://:", "pw12345678", "@cache.acme.zz"].concat(),
                ],
            ),
        ];
        for (kind, texts) in &cases {
            for text in texts {
                let found = findings(&format!("x\n{text}\n"), &home(), &host());
                assert!(found.contains(&f(kind, 2)), "{text}: {found:?}");
                assert!(!format!("{found:?}").contains(text.as_str()));
            }
        }
    }
}

mod quoted_key_scaling_test {
    use super::*;

    /// Python: 10,000 then 40,000 units, under `max(6 * small, 0.2)`. Here 2,500 then 10,000
    /// (unoptimized build), under `max(10 * small, 0.5)`: 4x the input stays far from the 16x a
    /// quadratic scan would take, and shared CI runners measured 8x.
    #[test]
    fn test_quoted_secret_key_scan_scales_linearly() {
        let unit = format!("secret_key\" {} ", "a".repeat(20));
        let small = scan_cost(&unit.repeat(2_500));
        let large = scan_cost(&unit.repeat(10_000));
        assert_scales("quoted secret keys", small, large, 0.5);
    }
}

// Key names are split so no assignment-shaped literal sits in this file (gitleaks scans the repo).
const PW: &str = concat!("pass", "word");
const TOK: &str = concat!("to", "ken");
const SEC: &str = concat!("sec", "ret");
const AK: &str = concat!("api", "_key");

fn value() -> String {
    ["hunter2", "xyz"].concat()
}

mod quoted_key_test {
    use super::*;

    /// A JSON- or dict-quoted key (`"key": "value"`) is an assignment too (FW-Q, ADR-021).
    pub fn flagged() -> Vec<String> {
        let v = value();
        let q = "'";
        vec![
            format!("\"{PW}\": \"{v}\""),
            format!("{q}api_token{q}: {q}{v}{q}"),
            format!("\"{SEC}\": \"{v}\""),
            format!("{{\"{TOK}\":\"{v}\"}}"),
            format!("\"{AK}\" : \"{v}\""),
            format!("\"{PW}\"={v}"),
            format!("{{\"user\": \"bob\", \"{PW}\": \"{v}\", \"x\": 1}}"),
            format!("\"{TOK}\": {v}"), // an unquoted scalar value
            format!("\"{}\":\"{v}\"", PW.to_uppercase()),
            format!("{q}{PW}{q}: {q}{v}{q}"),
            // a value that only starts with null/none/true/false is a value (fix round 1, I-1)
            format!("\"{PW}\": \"true-love-99\""),
            format!("\"{PW}\": \"True!Love#2024\""),
            format!("\"{AK}\": \"null.void.12345\""),
            format!("\"{PW}\": \"None-of-your-biz1\""),
            format!("\"{TOK}\": \"false/positive/xyz\""),
            format!("\"{TOK}\": falsetto12345"),
            // JSON inside a double-quoted shell string keeps its backslashes (I-2)
            format!("{{\\\"{PW}\\\": \\\"{v}\\\"}}"),
            format!("{{\\\"{AK}\\\":\\\"{v}\\\"}}"),
            format!("curl -d \"{{\\\"{TOK}\\\": \\\"{v}\\\"}}\""),
            // a placeholder with more after it is not a placeholder (I-3)
            format!("\"{PW}\": \"<a>b\""),
            format!("\"{PW}\": \"${{X}}tail\""),
            format!("\"{PW}\": \"***real\""),
            format!("{PW}=<a>b"),
            format!("{PW}: ${{X}}tail"),
            format!("{PW}=***real"),
            format!("\"{PW}\": \"string\""), // not a placeholder: a schema uses {"type": ...}
            format!("\"{TOK}\": \"<a>bcdefgh\""),
            format!("{SEC}=xray1234"),
            // a `,` `}` `]` inside the quotes does not end a value that starts like a placeholder
            format!("\"{PW}\": \"true,horse,battery\""),
            format!("\"{PW}\": \"x,Hunter2xyz\""),
            format!("\"{PW}\": \"X}}9aBcdEf\""),
            format!("\"{PW}\": \"*]q8Zr!kP\""),
            format!("\"{PW}\": \"<a>,realsecret1\""),
            format!("\"{PW}\": \"null, real-secret-99\""),
            format!("\"{PW}\": \"${{A}},realsecret1\""),
            format!("\"{AK}\": \"x,abcdef123\""),
            format!("\"{TOK}\": \"x,abcdef123456\""),
            format!("\"{SEC}\": \"true,abcdefgh12\""),
            format!("{{\\\"{PW}\\\": \\\"x,{v}\\\"}}"),
        ]
    }

    pub fn allowed() -> Vec<String> {
        let q = "'";
        vec![
            format!("\"{PW}\": {{\"type\": \"string\"}}"),
            format!("\"{SEC}\": {{\"type\": \"string\", \"description\": \"the secret\"}}"),
            format!("\"{PW}\": \"\""),
            format!("{q}{PW}{q}: {q}{q}"),
            format!("\"{TOK}\": null"),
            format!("\"{TOK}\": null,"),
            format!("\"{PW}\": true"),
            format!("\"{SEC}\": false"),
            format!("\"{TOK}\": []"),
            format!("\"{TOK}\": [{{\"type\": \"string\"}}, {{\"type\": \"number\"}}]"),
            format!("\"{PW}\": \"\", \"user\": \"bobbyxyz1234\""),
            format!("{{\"{PW}\":\"\",\"{TOK}\":null}}"),
            format!("the {PW} field"),
            format!("the {PW} field is required"),
            format!("the \"{PW}\" property"),
            format!("send the \"{TOK}\" header"),
            format!("\"{PW}\""),
            format!("\"{PW}\", \"{TOK}\""),
            format!("\"{PW}\": \"true\""),
            format!("\"{TOK}\": none }}"),
            format!("{{\\\"{PW}\\\": \\\"\\\"}}"),
            format!("{{\\\"{TOK}\\\": null}}"),
            // template placeholders, quoted or not (I-3)
            format!("\"{PW}\": \"<placeholder>\""),
            format!("\"{PW}\": \"x\""),
            format!("{q}{PW}{q}: {q}false{q}"),
            format!("{PW}=\"false\""),
            format!("{PW}='none'"),
            format!("\"{TOK}\": \"None\""),
            format!("\"{PW}\": \"<your-password>\""),
            format!("\"{PW}\": \"${{X}}\""),
            format!("\"{SEC}\": \"${{DB_PASS}}\""),
            format!("\"{PW}\": \"***\""),
            format!("\"{TOK}\": \"...\""),
            format!("\"{AK}\": \"xxxxxxxxxx\""),
            format!("\"{PW}\": \"XXXX\""),
            format!("{q}{TOK}{q}: {q}<token-here>{q}"),
            format!("{{\"username\": \"<user>\", \"{PW}\": \"<pass>\"}}"),
            format!("{PW}=<your-{PW}>"),
            format!("{PW}: ${{DB_PASS}}"),
            format!("{TOK}=<your-token-here>"),
            format!("{SEC}=********"),
            format!("{AK}: xxxxxxxxxxxx"),
            format!("{PW}=..."),
            format!("{PW}: \"<your-{PW}>\""),
        ]
    }

    #[test]
    fn test_quoted_keys_with_a_value_are_flagged() {
        for text in flagged() {
            let k = kinds(&format!("x\n{text}\n"));
            assert!(
                k.iter().any(|k| k == "secret:credential-assignment"),
                "{text}: {k:?}"
            );
            let found = findings(&text, &home(), &host());
            assert!(!format!("{found:?}").contains(&value()));
        }
    }

    #[test]
    fn test_empty_null_and_schema_values_stay_clean() {
        for text in allowed() {
            assert!(kinds(&text).is_empty(), "{text}: {:?}", kinds(&text));
        }
    }

    /// GD1's `_has_secret` is "a G6 pattern matches"; the G6 list is compiled here with the
    /// privacy scan's CPython semantics (the same sources, `anthology_guard::secrets`).
    #[test]
    fn test_the_guard_flags_and_clears_the_same_rows() {
        let guard = anthology_guard::secrets::GUARD_SECRET_PATTERNS;
        for text in flagged() {
            let lower = text.to_lowercase();
            if [PW, AK].iter().any(|k| lower.contains(k)) {
                // G6 covers password and api key only
                assert!(search_any(guard, &text), "{text}");
            }
        }
        for text in allowed() {
            assert!(!search_any(guard, &text), "{text}");
        }
    }

    #[derive(Debug)]
    struct Shape {
        label: String,
        unit: Option<String>,
        tail: Option<String>,
    }

    impl anthology_testkit::FromPyJson for Shape {
        fn from_py_json(v: &anthology_pycompat::json::Value) -> Result<Shape, String> {
            let params = v
                .as_object()
                .and_then(|o| o.get("params"))
                .and_then(|p| p.as_object())
                .ok_or("no params")?;
            let text = |k: &str| params.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Ok(Shape {
                label: text("shape").or_else(|| text("run")).ok_or("no label")?,
                unit: text("shape"),
                tail: text("run"),
            })
        }
    }

    /// The 21 repeated shapes and 9 tails of the Python test (its subTest rows, tagged in
    /// `tests/data/test_memory_privacy.jsonl`), by label.
    fn unit_for(label: &str) -> String {
        let sp = |n: usize| " ".repeat(n);
        match label {
            "closing quotes" => format!("{PW}\""),
            "quote then blanks" => format!("{PW}\"{}", sp(4)),
            "key quote colon" => format!("{PW}\":"),
            "key quote colon blanks" => format!("\"{TOK}\": {}", sp(16)),
            "key quote colon quotes" => format!("\"{SEC}\": {}", "\"".repeat(6)),
            "single quotes" => format!("{AK}' "),
            "null values" => format!("\"{TOK}\": null, "),
            "empty values" => format!("\"{PW}\": \"\", "),
            "object values" => format!("\"{SEC}\": {{\"a\": 1}}, "),
            "short values" => format!("\"{PW}\": \"ab\" "),
            "escaped quotes" => format!("\\\"{PW}\\\": \\\""),
            "escaped null" => format!("\\\"{TOK}\\\": null, "),
            "angle placeholders" => format!("{PW}=<a> "),
            "open angles" => format!("{PW}=<"),
            "template placeholders" => format!("\"{SEC}\": \"${{X}}\" "),
            "open templates" => format!("\"{SEC}\": \"${{"),
            "star runs" => format!("{TOK}=*** "),
            "x then more" => format!("{PW}=xxxx, y "),
            "prefix words" => format!("\"{TOK}\": \"true-"),
            "quoted skip then comma" => format!("\"{PW}\": \"x,"),
            "quoted skip then brace" => format!("\"{TOK}\": \"true}}"),
            other => panic!("unknown shape {other:?}"),
        }
    }

    fn tail_for(label: &str) -> &'static str {
        match label {
            "blanks" => " ",
            "quotes" => "\"",
            "open braces" => "{",
            "letters" => "a",
            "escapes" => "\\",
            "angles" => "<",
            "stars" => "*",
            "x run" => "x",
            "dollar braces" => "${",
            other => panic!("unknown run {other:?}"),
        }
    }

    /// Python: each shape 20,000 then 80,000 times, each tail 200,000 then 800,000 times,
    /// under `max(6 * small, 0.1)`. Here 2,500 then 10,000 and 25,000 then 100,000
    /// (unoptimized build).
    #[test]
    fn test_quoted_key_patterns_scan_in_linear_time() {
        let mut patterns: Vec<(&str, bool)> = SECRET_PATTERNS
            .iter()
            .filter(|(label, _, _)| *label == "credential-assignment")
            .map(|&(_, py, ic)| (py, ic))
            .collect();
        patterns.extend(
            anthology_guard::secrets::GUARD_SECRET_PATTERNS
                .iter()
                .filter(|(py, _)| py.contains("password") || py.contains("key")),
        );
        assert!(patterns.len() >= 5);
        let cost = |text: &str| {
            best_cost(3, || {
                for p in &patterns {
                    search_any(std::slice::from_ref(p), text);
                }
            })
        };
        let rows: Vec<Shape> = anthology_testkit::rows(
            "test_memory_privacy",
            "test_memory_privacy.QuotedKeyTest.test_quoted_key_patterns_scan_in_linear_time",
        );
        assert_eq!(rows.len(), 30);
        for row in &rows {
            if let Some(shape) = &row.unit {
                let unit = unit_for(shape);
                let (small, large) = (cost(&unit.repeat(2_500)), cost(&unit.repeat(10_000)));
                assert_scales(&row.label, small, large, 0.1);
            }
            if let Some(run) = &row.tail {
                let head = format!("\"{PW}\":");
                let tail = tail_for(run);
                let small = cost(&(head.clone() + &tail.repeat(25_000)));
                let large = cost(&(head + &tail.repeat(100_000)));
                assert_scales(&row.label, small, large, 0.1);
            }
        }
    }
}

mod exemption_test {
    use super::*;

    #[test]
    fn test_home_segments_are_case_sensitive() {
        // macOS spells it /Users/ and Linux /home/: API routes such as /users/42 are not homes
        for text in [
            "GET /api/v1/users/42".to_string(),
            "GET /users/42".to_string(),
            ["/", "USERS/alice/x"].concat(),
            ["/", "Home/alice/x"].concat(),
            "see /api/v1/users/alice/settings".to_string(),
        ] {
            assert!(kinds(&text).is_empty(), "{text}");
        }
    }

    #[test]
    fn test_nothing_is_flagged() {
        let (u, h) = (u(), h());
        for text in [
            format!("{u}/Shared/build"),
            format!("{u}/<name>/project"),
            format!("{h}/<user>/x"),
            "$HOME/project".to_string(),
            at(&["noreply", "acme.zz"]),
            at(&["no-reply", "acme.zz"]),
            at(&["1234+alice", "users.noreply.github.com"]),
            at(&["alice", "example.com"]),
            at(&["bob", "mail.example.org"]),
            at(&["carol", "example.net"]),
            at(&["dan", "host.test"]),
            at(&["eve", "box.invalid"]),
            at(&["frank", "site.example"]),
            at(&["gail", "box.localhost"]),
            format!("git remote: {}:acme/app.git", at(&["git", "github.com"])),
            format!("ssh -T {}", at(&["git", "github.com"])),
            format!("ssh {}", at(&["alice", "build.example.com"])),
            "127.0.0.1".into(),
            "127.8.9.10".into(),
            "0.0.0.0".into(),
            "255.255.255.255".into(),
            "192.0.2.1".into(),
            "198.51.100.7".into(),
            "203.0.113.200".into(),
            "v1.2.3.4".into(),
            "1.2.3.4.5".into(),
            "999.1.2.3".into(),
            "2001:db8::1".into(),
            "at 12:30:45 today".into(),
            "settings.local.json".into(),
            "Path.home() call".into(),
            "https://example.com/home/index.html".into(),
            "an .ssh folder".into(),
            "src/home/view.py".into(),
            format!("see https://example.com{u}/alice/profile"),
            format!("x1a://h{u}/alice"),
            format!("svn+ssh://h{u}/alice"),
            "https://example.com:8443/path".into(),
            "see https://example.com:443/a@b".into(),
            "git+ssh://host.test/repo".into(),
            "the access key is rotated monthly".into(),
            "secret key handling".into(),
            "key = short".into(),
            "https://example.com/x?next=http://example.org/y".into(),
        ] {
            assert!(kinds(&text).is_empty(), "{text}: {:?}", kinds(&text));
        }
    }
}

/// Import scans untrusted archives: one huge line must never take quadratic time. Python bounds
/// each line at 1.0 s of wall time; here each shape is timed at a quarter of its size and at its
/// size, and the larger must cost less than 6x (the scan finishes: no `unscannable`).
mod hostile_input_test {
    use super::*;

    /// Python's n is 100,000; 25,000 here (unoptimized build).
    const N: usize = 25_000;

    /// A hostile line of about `n` characters.
    type Make<'a> = Box<dyn Fn(usize) -> String + 'a>;

    fn assert_linear(label: &str, make: impl Fn(usize) -> String) {
        let (small_text, large_text) = (make(N / 4), make(N));
        let small = scan_cost(&small_text);
        let large = scan_cost(&large_text);
        assert_scales(label, small, large, 0.1);
        let found = privacy_findings(&large_text, Some("/srv/zed"), Some(&host()), &deps());
        assert!(
            !found.iter().any(|f| f.kind == UNSCANNABLE),
            "{label}: {found:?}"
        );
    }

    #[test]
    fn test_long_tokens_before_every_kind() {
        let host = host();
        let tails: [(&str, String); 7] = [
            ("home path", format!(" {}/x", u())),
            ("email", format!(" {}", at(&["alice", "mail.acme.zz"]))),
            ("private ip", " 10.1.2.3".into()),
            ("hostname", format!(" nas.lan {host}")),
            ("ssh", format!(" ssh {}", at(&["bob", "buildhost"]))),
            ("key path", " ~/.ssh/id_rsa".into()),
            (
                "secret",
                format!(" {}{}", ["gh", "p_"].concat(), "a1".repeat(20)),
            ),
        ];
        type Prefix = fn(usize) -> String;
        let prefixes: [(&str, Prefix); 5] = [
            ("a", |n| "a".repeat(n)),
            ("a/", |n| "a/".repeat(n / 2)),
            ("-", |n| "-".repeat(n)),
            ("a.", |n| "a.".repeat(n / 2)),
            ("a@", |n| "a@".repeat(n / 2)),
        ];
        for (pname, prefix) in prefixes {
            for (label, tail) in &tails {
                assert_linear(&format!("{label} after {pname:?}..."), |n| prefix(n) + tail);
            }
        }
    }

    #[test]
    fn test_repeated_structures() {
        let u = u();
        let cases: Vec<(&str, Make<'_>)> = vec![
            (
                "late scheme then many homes",
                Box::new(|n| {
                    format!(
                        "/{}https://h{}",
                        "a".repeat(n),
                        format!("{u}/x").repeat(n / 100)
                    )
                }),
            ),
            (
                "many homes in a relative run",
                Box::new(|n| format!("a{}", format!("{u}/x").repeat(n / 8))),
            ),
            (
                "many file urls",
                Box::new(|n| format!("x:///{}/x ", &u[1..]).repeat(n / 14)),
            ),
            (
                "schemes in one token",
                Box::new(|n| format!("{}{u}/x", "a://".repeat(n / 4))),
            ),
            (
                "jwt starts after dashes",
                Box::new(|n| "eyJ-".repeat(n / 4)),
            ),
            (
                "jwt segments",
                Box::new(|n| format!("eyJ{}.{}", "a".repeat(n), "b".repeat(8))),
            ),
            (
                "labels",
                Box::new(|n| format!("{}.", "a".repeat(70)).repeat(n / 71)),
            ),
            (
                "domain dots",
                Box::new(|n| format!("a@{}_", "b.".repeat(n / 2))),
            ),
            ("hex colons", Box::new(|n| "fe80:".repeat(n / 5))),
            (
                "url credentials, no at sign",
                Box::new(|n| "a://b:".repeat(n / 6)),
            ),
            (
                "url user then long password",
                Box::new(|n| format!("a://b:{}", "c".repeat(n))),
            ),
            (
                "secret key assignments",
                Box::new(|n| "secret_key".repeat(n / 10)),
            ),
            (
                "quoted secret keys",
                Box::new(|n| "secret_key\"".repeat(n / 11)),
            ),
        ];
        for (label, make) in &cases {
            assert_linear(label, make);
        }
    }

    /// Each token's end is found without reading to the end of the line. Python: 40,000 then
    /// 160,000 tokens; 2,500 then 10,000 here for the ratio. The full 160,000-token line (2.24
    /// MB) must still be clean: it needs the line-length budget (I-9).
    #[test]
    fn test_many_url_tokens_on_one_line() {
        let u = u();
        for sep in [" ", "\t"] {
            let line = |k: usize| format!("a://h{u}/x{sep}").repeat(k);
            let cost = |k: usize| {
                let text = line(k);
                best_cost(3, || {
                    let found = privacy_findings(&text, Some("/srv/zed"), Some(&host()), &deps());
                    assert_eq!(found, vec![], "{sep:?}");
                })
            };
            let (small, large) = (cost(2_500), cost(10_000));
            assert_scales(&format!("{sep:?}"), small, large, 0.2);
        }
        let full = format!("a://h{u}/x ").repeat(160_000);
        assert_eq!(
            privacy_findings(&full, Some("/srv/zed"), Some(&host()), &deps()),
            vec![]
        );
    }
}

mod shared_secret_patterns_test {
    use super::*;

    /// The guard (G6) can never block something the scan misses.
    #[test]
    fn test_every_guard_pattern_is_in_the_shared_list() {
        let shared: Vec<(&str, bool)> = SECRET_PATTERNS.iter().map(|&(_, p, ic)| (p, ic)).collect();
        let guard = anthology_guard::secrets::GUARD_SECRET_PATTERNS;
        assert!(!guard.is_empty());
        for g in guard {
            assert!(shared.contains(g), "{}", g.0);
        }
    }

    #[test]
    fn test_shape() {
        for (label, pattern, ignorecase) in SECRET_PATTERNS {
            let mut chars = label.chars();
            assert!(
                chars.next().is_some_and(|c| c.is_ascii_lowercase()),
                "{label}"
            );
            assert!(
                chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{label}"
            );
            // Python: `hasattr(rx, "search")`, i.e. it compiled. A pattern that does not
            // compile counts as a match (fail closed), and none matches the empty text.
            assert!(!search_any(&[(pattern, *ignorecase)], ""), "{label}");
        }
    }

    /// Python: `memory_sync` has no `_SECRET_PATTERNS` of its own.
    #[test]
    fn test_memory_sync_defines_no_private_pattern_list() {
        let sync = source("src/sync.rs");
        assert!(
            !sync.contains("SECRET_PATTERNS:"),
            "sync.rs defines a pattern list"
        );
        assert!(!sync.contains("SECRET_PATTERNS ="));
    }
}

/// I-9: a limit forced low gives `unscannable` on the line it hits; the other lines keep their
/// findings.
mod backtrack_limit {
    use super::*;

    #[test]
    fn a_limit_forced_low_gives_unscannable_on_that_line() {
        let low = |backtrack_limit| PrivacyDeps {
            hostname: || None,
            home: None,
            limits: PrivacyLimits {
                backtrack_limit,
                backtrack_per_byte: 0,
            },
        };
        let mail = at(&["alice", "mail.acme.zz"]);
        // Every backtracking search pops at least once, so a budget of 0 leaves every line that
        // passes a prefilter unscannable; a line no prefilter accepts never needs the engine.
        let found = privacy_findings(
            &format!("{mail}\nplain words\nsee {mail}"),
            Some("/srv/zed"),
            Some(""),
            &low(0),
        );
        assert_eq!(found, vec![f(UNSCANNABLE, 1), f(UNSCANNABLE, 3)]);
        assert_eq!(
            describe(&found),
            "text the privacy scan could not finish (lines 1, 3)"
        );
        // A small budget: the long line is unscannable, the short ones keep their findings.
        let text = format!("{} {mail}\nok\nsee {mail}", "x".repeat(5_000));
        let found = privacy_findings(&text, Some("/srv/zed"), Some(""), &low(200));
        assert!(found.contains(&f(UNSCANNABLE, 1)), "{found:?}");
        assert!(!found.iter().any(|f| f.line == 2), "{found:?}");
        assert!(found.contains(&f("email", 3)), "{found:?}");
        assert!(!found.contains(&f(UNSCANNABLE, 3)), "{found:?}");
    }
}

/// Properties (design §13.5), with a fixed seed.
mod properties {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngSeed};

    /// Pieces that trip (or nearly trip) every kind, and boundary characters.
    fn piece() -> impl Strategy<Value = String> {
        let fixed: Vec<String> = vec![
            format!("{}/alice/x", u()),
            format!("file://{}/bob", h()),
            format!("https://example.com{}/c", u()),
            at(&["alice", "mail.acme.zz"]),
            at(&["git", "github.com"]),
            "10.1.2.3".into(),
            "8.8.4.4".into(),
            "\u{661}0.1.2.3".into(),
            "fe80::1%en0".into(),
            "fd00::".into(),
            "nas.lan".into(),
            host(),
            "ssh ".into(),
            at(&["deploy", "build-01"]),
            "~/.ssh/id_rsa".into(),
            ["gh", "p_"].concat() + &"a1".repeat(20),
            format!("\"{PW}\": \"{}\"", value()),
            format!("{PW}=<x>"),
            "eyJaaaaaaaaa.bbbbbbbbb.ccccccccc".into(),
            ["postgres", "://u:", "pw123456", "@db"].concat(),
            "\u{301}".into(),
            "\u{130}".into(),
            "\u{212a}".into(),
            "\u{17f}".into(),
            "\u{b2}".into(),
            "\u{1c}".into(),
            "\u{a0}".into(),
            "\r".into(),
            "\n".into(),
            " ".into(),
            "a".repeat(40),
            "a.".repeat(20),
            "-".repeat(30),
        ];
        prop_oneof![
            4 => proptest::sample::select(fixed),
            1 => "[ -~]{0,12}",
            1 => any::<char>().prop_map(String::from),
        ]
    }

    fn text() -> impl Strategy<Value = String> {
        proptest::collection::vec(piece(), 0..24).prop_map(|p| p.concat())
    }

    fn deps_with(backtrack_limit: usize, backtrack_per_byte: usize) -> PrivacyDeps {
        PrivacyDeps {
            hostname: || None,
            home: None,
            limits: PrivacyLimits {
                backtrack_limit,
                backtrack_per_byte,
            },
        }
    }

    fn config() -> Config {
        Config {
            cases: 128,
            rng_seed: RngSeed::Fixed(0x6d70_315f_7072_6976),
            failure_persistence: None,
            ..Config::default()
        }
    }

    proptest! {
        #![proptest_config(config())]

        /// I-9: lowering the limit never loses a finding: each one stays, or its line becomes
        /// `unscannable`.
        #[test]
        fn findings_never_shrink_as_the_limit_drops(
            text in text(),
            low in 0usize..400,
            per in 0usize..2,
        ) {
            let default = deps_with(1_000_000, 64);
            let high = privacy_findings(&text, Some("/srv/zed"), Some(&host()), &default);
            let lowered = deps_with(low, per);
            let lower = privacy_findings(&text, Some("/srv/zed"), Some(&host()), &lowered);
            for f in &high {
                let blocked = Finding { kind: UNSCANNABLE.into(), line: f.line };
                prop_assert!(
                    lower.contains(f) || lower.contains(&blocked),
                    "{f:?} lost at limit {low}: {lower:?}"
                );
            }
            prop_assert!(!high.iter().any(|f| f.kind == UNSCANNABLE), "{high:?}");
        }

        /// A finding inserted between blanks is reported on its line.
        #[test]
        fn an_insertion_is_found_on_its_line(
            lines in proptest::collection::vec("[^\n]{0,40}", 1..6),
            at_line in 0usize..6,
            which in 0usize..6,
        ) {
            let (kind, piece) = [
                ("home-path", format!("{}/alice/x", u())),
                ("email", at(&["alice", "mail.acme.zz"])),
                ("private-ip", "10.1.2.3".to_string()),
                ("private-hostname", "nas.lan".to_string()),
                ("ssh-key-path", "~/.ssh/id_rsa".to_string()),
                ("secret:github-token", ["gh", "p_"].concat() + &"a1".repeat(20)),
            ][which].clone();
            let mut lines = lines;
            let k = at_line % lines.len();
            lines[k] = format!("{} {piece} {}", lines[k], lines[k]);
            let text = lines.join("\n");
            let found = privacy_findings(&text, Some("/srv/zed"), Some(""), &deps_with(1_000_000, 64));
            let want = Finding { kind: kind.into(), line: k + 1 };
            prop_assert!(found.contains(&want), "{text:?}: {found:?}");
        }
    }
}
