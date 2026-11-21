//! The `privacy.findings` difftest surface (MP1; oracle `port/difftest/oracle/privacy.py`).
//!
//! Cases:
//! - `{"op": "findings", "text", "home", "hostname"}`: `privacy_findings` with an explicit home
//!   and hostname. Both sides print one `<line>\t<kind>` line per finding, then
//!   `describe: <text>`. Python never reports `unscannable`, so a Rust one (a backtrack limit
//!   hit, I-9) is a diff: zero unexplained diffs also proves that no case hits the limit.
//! - `{"op": "patterns"}` and `{"op": "constants"}`: `SECRET_PATTERNS`, then every other
//!   pattern, the two dynamic templates and the exemption tables, so the Rust copy cannot drift
//!   from the Python.
//! - `{"op": "classes"}`: the generated `\w`/`\s`/`\d`, lowercase and extra-case tables,
//!   against the oracle's own `re`.
//! - `{"op": "fold", "chars"}`: for every character CPython's IGNORECASE tables touch, which of
//!   them its hostname literal matches (Rust compiles the literal it writes; Python
//!   `re.compile(re.escape(c), re.I)`).
//!
//! Seeds are the inputs of `tests/test_memory_privacy.py` and their neighbours. Generated cases
//! splice fragments of every kind (and near misses) with boundary characters (marks, `No`
//! digits, `Pc`, joiners, İ ı ſ K, the `\s` oddities, Unicode digits, code points new after
//! Unicode 15.0), random text and hostile repeats of up to about 50 KB per line, with host
//! names of cased letters from CPython's case tables and the home and host values respelled.
//! Fixture secrets and paths are assembled at run time.

use anthology_memory::privacy::{self, PrivacyDeps, PrivacyLimits};
use std::sync::OnceLock;

use anthology_pycompat::json::{Int, Map, Value};

use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "privacy.findings";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`): a
/// library surface with no platform dependence, so yes.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(PrivacyFindings))
}

struct PrivacyFindings;

impl Surface for PrivacyFindings {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut fold = Map::new();
        fold.insert("op".into(), Value::Str("fold".into()));
        let chars = privacy::case_candidates()
            .into_iter()
            .map(|c| Value::Int(Int::Small(i64::from(c))))
            .collect();
        fold.insert("chars".into(), Value::Array(chars));
        let mut out = vec![
            Case::new("seed:patterns", op("patterns")),
            Case::new("seed:constants", op("constants")),
            Case::new("seed:classes", op("classes")),
            Case::new("seed:fold", Value::Object(fold)),
        ];
        let home = format!("{}/zed", u());
        let host = host();
        for (i, text) in seed_texts().into_iter().enumerate() {
            out.push(Case::new(
                format!("seed:{}", i + 1),
                findings_case(&text, &home, &host),
            ));
        }
        for (i, (text, home, host)) in seed_dynamic().into_iter().enumerate() {
            out.push(Case::new(
                format!("seed:dyn:{}", i + 1),
                findings_case(&text, &home, &host),
            ));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("privacy", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

fn op(name: &str) -> Value {
    let mut m = Map::new();
    m.insert("op".into(), Value::Str(name.into()));
    Value::Object(m)
}

fn findings_case(text: &str, home: &str, hostname: &str) -> Value {
    let mut m = Map::new();
    m.insert("op".into(), Value::Str("findings".into()));
    m.insert("text".into(), Value::Str(text.into()));
    m.insert("home".into(), Value::Str(home.into()));
    m.insert("hostname".into(), Value::Str(hostname.into()));
    Value::Object(m)
}

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.as_object()
        .and_then(|o| o.get(k))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("privacy.findings: no {k:?}"))
}

fn run(case: &Value) -> Observed {
    match field(case, "op") {
        "findings" => {
            let deps = PrivacyDeps {
                hostname: || panic!("privacy.findings: the hostname is explicit"),
                home: None,
                limits: PrivacyLimits::default(),
            };
            let found = privacy::privacy_findings(
                field(case, "text"),
                Some(field(case, "home")),
                Some(field(case, "hostname")),
                &deps,
            );
            let mut out: String = found
                .iter()
                .map(|f| format!("{}\t{}\n", f.line, f.kind))
                .collect();
            out.push_str(&format!("describe: {}\n", privacy::describe(&found)));
            Observed::text(0, &out, "")
        }
        "patterns" => {
            let out: String = privacy::SECRET_PATTERNS
                .iter()
                .map(|(label, py, ic)| format!("{label}\t{}\t{py}\n", u8::from(*ic)))
                .collect();
            Observed::text(0, &out, "")
        }
        "constants" => Observed::text(0, &privacy::python_constants(), ""),
        "classes" => {
            use privacy::pyclass::{D, DIGIT_ZEROS, EXTRA_CASES, LOWER, S, W};
            let ranges = |name: &str, r: &[(u32, u32)]| {
                let parts: Vec<String> = r.iter().map(|(a, b)| format!("{a:x}-{b:x}")).collect();
                format!("{name} {}\n", parts.join(" "))
            };
            let zeros: Vec<String> = DIGIT_ZEROS.iter().map(|z| format!("{z:x}")).collect();
            let lower: Vec<String> = LOWER.iter().map(|(a, b)| format!("{a:x}:{b:x}")).collect();
            let extra: Vec<String> = EXTRA_CASES
                .iter()
                .map(|(k, vs)| {
                    let vs: Vec<String> = vs.iter().map(|v| format!("{v:x}")).collect();
                    format!("{k:x}:{}", vs.join(","))
                })
                .collect();
            let out = ranges("w", W)
                + &ranges("s", S)
                + &ranges("d", D)
                + &format!("zeros {}\n", zeros.join(" "))
                + &format!("lower {}\n", lower.join(" "))
                + &format!("extra {}\n", extra.join(" "));
            Observed::text(0, &out, "")
        }
        "fold" => {
            let chars: Vec<u32> = case
                .as_object()
                .and_then(|o| o.get("chars"))
                .and_then(Value::as_array)
                .expect("privacy.findings: fold needs chars")
                .iter()
                .map(|v| match v {
                    Value::Int(Int::Small(n)) => u32::try_from(*n).expect("a code point"),
                    other => panic!("privacy.findings: not a code point: {other:?}"),
                })
                .collect();
            Observed::text(0, &privacy::ignorecase_fold(&chars), "")
        }
        other => panic!("privacy.findings: unknown op {other:?}"),
    }
}

fn u() -> String {
    ["/", "Users"].concat()
}

fn h() -> String {
    ["/", "home"].concat()
}

fn host() -> String {
    ["alice-", "laptop"].concat()
}

fn at(a: &str, b: &str) -> String {
    [a, b].join("@")
}

const PW: &str = concat!("pass", "word");
const TOK: &str = concat!("to", "ken");
const SEC: &str = concat!("sec", "ret");
const AK: &str = concat!("api", "_key");

fn value() -> String {
    ["hunter2", "xyz"].concat()
}

/// Fragments of every kind, near misses and exemptions (the Python tests' inputs and more).
fn fragments() -> Vec<String> {
    let (u, h, host, v) = (u(), h(), host(), value());
    let mut f: Vec<String> = vec![
        // home paths
        format!("{u}/alice/project"),
        format!("cd {h}/bob"),
        format!("({u}/alice)"),
        format!("{u}/alice."),
        format!("/mnt/c{u}/alice/proj"),
        format!("/var{h}/alice/src"),
        format!("file://{u}/alice/x"),
        format!("file:///{}/x", &u[1..]),
        format!("x:///{}/x", &u[1..]),
        format!("C:{u}/alice/x"),
        format!("/api{h}/feed"),
        format!("1://h{u}/alice/x"),
        format!("+.-://h{u}/alice/x"),
        format!("PATH=/opt{h}/carol/bin"),
        format!("{u}/Shared/build"),
        format!("{u}/<name>/project"),
        format!("{u}/..."),
        format!("{u}/.."),
        format!("see https://example.com{u}/alice/profile"),
        format!("x1a://h{u}/alice"),
        format!("svn+ssh://h{u}/alice"),
        format!("a://h{u}/x"),
        "src/home/view.py".into(),
        "GET /users/42".into(),
        ["/", "USERS/alice/x"].concat(),
        "https://example.com/home/index.html".into(),
        "$HOME/project".into(),
        // emails
        at("alice", "mail.acme.zz"),
        at("noreply", "acme.zz"),
        at("No-Reply", "acme.zz"),
        at("1234+alice", "users.noreply.github.com"),
        at("alice", "example.com"),
        at("bob", "mail.example.org"),
        at("dan", "host.test"),
        at("eve", "box.invalid"),
        at("gail", "box.localhost"),
        at("git", "github.com"),
        at("GIT", "GitLab.com."),
        at("git", "forge.acme.zz"),
        at("a.b%c+d-e", "x-y.acme.zz"),
        at("alice", "acme.z9"),
        at("alice", "acme.zz-"),
        at("alice", "acme.zz.."),
        // IPv4
        "10.1.2.3".into(),
        "172.16.0.9".into(),
        "172.31.255.1".into(),
        "192.168.1.20".into(),
        "100.64.3.4".into(),
        "169.254.10.10".into(),
        "8.8.4.4".into(),
        "172.32.0.1".into(),
        "1.2.3.4".into(),
        "127.0.0.1".into(),
        "0.0.0.0".into(),
        "255.255.255.255".into(),
        "192.0.2.1".into(),
        "198.51.100.7".into(),
        "203.0.113.200".into(),
        "v1.2.3.4".into(),
        "1.2.3.4.5".into(),
        "999.1.2.3".into(),
        "1.2.3.4567".into(),
        "01.002.3.4".into(),
        "\u{661}\u{660}.\u{661}.\u{662}.\u{663}".into(),
        "\u{967}\u{966}.0.0.1".into(),
        "8.8.8.\u{ff18}".into(),
        "1\u{1d7ce}.1.1.1".into(),
        // IPv6
        "fd12:3456:789a::1".into(),
        "fe80::1%en0".into(),
        "fe80::1%".into(),
        "FE80::abcd".into(),
        "fc00::".into(),
        "fbff::1".into(),
        "febf::1".into(),
        "fec0::1".into(),
        "2001:db8::1".into(),
        "fd00:1:2:3:4:5:6:7".into(),
        "fd00:1:2:3:4:5:6".into(),
        "fd00::1::2".into(),
        "fd00:::1".into(),
        "::fd00:1".into(),
        "fd00:1:2:3:4:5:6:7:8".into(),
        "fd001::1".into(),
        "fe80:".into(),
        "at 12:30:45 today".into(),
        // hostnames
        "nas.lan".into(),
        "build-box.local".into(),
        "db.internal".into(),
        "wiki.corp".into(),
        "router.home.arpa".into(),
        "pi.tail1234.ts.net".into(),
        "box.localdomain".into(),
        "printer.home".into(),
        "intra.intranet".into(),
        "NAS.LAN".into(),
        "nas.lan.".into(),
        "nas.lan-x".into(),
        "nas.lan(".into(),
        "nas.lan.x".into(),
        "nas.lan.-".into(),
        "settings.local.json".into(),
        "Path.home() call".into(),
        "x.INTERNAL".into(),
        "x.\u{130}nternal".into(),
        "x.\u{131}ntranet".into(),
        "x.\u{212a}".into(),
        "-a.lan".into(),
        host.clone(),
        host.to_uppercase(),
        format!("{host}-2"),
        format!("{host}.local"),
        // ssh
        format!("ssh -p 2222 {}", at("deploy", "build-01")),
        format!("scp f {}", at("bob", "buildhost")),
        format!("sftp {}", at("bob", "buildhost")),
        format!("rsync -a x {}", at("bob", "buildhost")),
        format!("mosh {}", at("bob", "buildhost")),
        format!("{} via ssh", at("deploy", "build-01")),
        format!("ssh -T {}", at("git", "github.com")),
        format!("ssh {}", at("alice", "build.example.com")),
        "git+ssh://host.test/repo".into(),
        "ssh-keygen".into(),
        "openssh".into(),
        at("deploy", "build-01"),
        // key paths
        "~/.ssh/id_work".into(),
        "$HOME/.ssh/config".into(),
        "the id_ed25519 key".into(),
        "id_rsa.pub".into(),
        "id_rsa-x".into(),
        "my_id_rsa".into(),
        "an .ssh folder".into(),
        "/.ssh/".into(),
        // secrets
        ["AK", "IA", "ABCDEFGHIJKLMNOP"].concat(),
        ["AS", "IA", "ABCDEFGHIJKLMNOP"].concat(),
        ["gh", "p_"].concat() + &"a1".repeat(20),
        ["gh", "o_x"].concat(),
        ["github", "_pat_", "abcdefghij0123456789"].concat(),
        ["sk", "-", "abcdefghij0123456"].concat(),
        ["sk", "-ant-", "abcdefghij0123456789"].concat(),
        ["xo", "xb-", "1234567890-abc"].concat(),
        ["-----BEGIN ", "OPENSSH PRIVATE KEY-----"].concat(),
        [
            "eyJ",
            "hbGciOiJIUzI1NiJ9",
            ".eyJ",
            "zdWIiOiIxMjM0In0",
            ".c2lnbmF0dXJlLXg",
        ]
        .concat(),
        ["-eyJ", "aaaaaaaa.bbbbbbbb.cccccccc"].concat(),
        ["AI", "za"].concat() + &"C".repeat(35),
        ["sk", "_live_", "abcdefghij0123"].concat(),
        ["rk", "_live_", "abcdefghij0123"].concat(),
        ["Bear", "er ", "abcdefghij0123456789xyz"].concat(),
        ["Authorization: Bas", "ic ", "dXNlcjpwYXNzd29yZA=="].concat(),
        ["AUTHORIZATION:bas", "ic   ", "dXNlcjpw"].concat(),
        ["gl", "pat-", "abcdefghij0123456789"].concat(),
        ["hf", "_"].concat() + &"abcdefghij".repeat(3) + "xy",
        ["np", "m_"].concat() + &"abcdefghij".repeat(3) + "abcdef",
        [
            "aws_secret_",
            "access_key = ",
            "wJalrXUtnFEMI",
            "K7MDENGbPxRfiCY",
        ]
        .concat(),
        ["\"Secret", "AccessKey\": \"", "wJalrXUtnFEMI", "K7MDENG\""].concat(),
        ["'secret_", "key': '", "abcdefgh12345678'"].concat(),
        ["postgres", "://app:", "Sup3rS3cr", "etPw@db.acme.zz/prod"].concat(),
        ["redis", "://:", "pw12345678", "@cache.acme.zz"].concat(),
        "https://example.com:443/a@b".into(),
        "the access key is rotated monthly".into(),
        "secret key handling".into(),
        "key = short".into(),
        // credential assignments (ADR-021), flagged and allowed shapes
        format!("\"{PW}\": \"{v}\""),
        format!("'api_token': '{v}'"),
        format!("{{\"{TOK}\":\"{v}\"}}"),
        format!("\"{AK}\" : \"{v}\""),
        format!("\"{PW}\"={v}"),
        format!("\"{TOK}\": {v}"),
        format!("\"{}\":\"{v}\"", PW.to_uppercase()),
        format!("\"{PW}\": \"true-love-99\""),
        format!("\"{TOK}\": falsetto12345"),
        format!("{{\\\"{PW}\\\": \\\"{v}\\\"}}"),
        format!("curl -d \"{{\\\"{TOK}\\\": \\\"{v}\\\"}}\""),
        format!("\"{PW}\": \"<a>b\""),
        format!("\"{PW}\": \"${{X}}tail\""),
        format!("{PW}=***real"),
        format!("\"{PW}\": \"true,horse,battery\""),
        format!("\"{PW}\": \"X}}9aBcdEf\""),
        format!("\"{PW}\": \"null, real-secret-99\""),
        format!("\"{PW}\": {{\"type\": \"string\"}}"),
        format!("\"{PW}\": \"\""),
        format!("\"{TOK}\": null,"),
        format!("\"{TOK}\": NULL"),
        format!("\"{PW}\": \"fal\u{17f}e\""),
        format!("\"{TOK}\": []"),
        format!("the {PW} field"),
        format!("\"{PW}\", \"{TOK}\""),
        format!("\"{TOK}\": none }}"),
        format!("{{\\\"{TOK}\\\": null}}"),
        format!("\"{PW}\": \"<placeholder>\""),
        format!("{PW}='none'"),
        format!("\"{SEC}\": \"${{DB_PASS}}\""),
        format!("\"{AK}\": \"xxxxxxxxxx\""),
        format!("{SEC}=********"),
        format!("{AK}: xxxxxxxxxxxx"),
        format!("{PW}=..."),
        format!("{PW}: \"<your-{PW}>\""),
        format!("{SEC}=xray1234"),
        format!("{SEC}={v}\r"),
        format!("{PW}:\u{a0}{v}"),
        format!("{PW}:\u{1c}{v}"),
        format!("{PW}:\u{2028}{v}"),
        format!("PA\u{17f}\u{17f}WORD={v}"),
        format!("ap\u{130}_\u{212a}ey={v}"),
        format!("ap\u{131}-key: {v}"),
        format!("passwd = {v}"),
        format!("{TOK}\t=\t{v}"),
        format!("{PW}=\"{v}"),
    ];
    // Doubled and escaped quote forms.
    f.push(format!("\\\"{PW}\\\":\\\"x,{v}\\\""));
    f
}

/// Characters next to which Python's and Rust's `\w`, `\b`, `\s`, `\d` or case folding differ.
const BOUNDARY: &[char] = &[
    '\u{301}',  // combining acute (Mn: \w in Rust, not in Python)
    '\u{903}',  // Mc
    '\u{20dd}', // Me
    '\u{b2}',   // superscript two (No: \w in Python, not in Rust)
    '\u{bd}',   // vulgar half (No)
    '\u{2460}', // circled one (No)
    '\u{203f}', // undertie (Pc)
    '\u{2054}', // inverted undertie (Pc)
    '\u{200c}', // ZWNJ
    '\u{200d}', // ZWJ
    '\u{130}',  // İ
    '\u{131}',  // ı
    '\u{17f}',  // ſ
    '\u{212a}', // Kelvin sign
    '\u{1c}',
    '\u{1f}',
    '\u{85}',
    '\u{a0}',
    '\u{2028}',
    '\u{3000}',
    '\u{200b}', // zero-width space (not \s)
    '\u{661}',  // Arabic-Indic one (Nd)
    '\u{ff11}', // fullwidth one (Nd)
    '\u{1d7cf}',
    '\u{2ffc}',  // new in Unicode 15.1
    '\u{31e4}',  // new in Unicode 16
    '\u{10d40}', // Garay digit zero (Nd in Unicode 16 only)
    '\u{1e5f1}', // Ol Onal digit (Unicode 16)
    '\u{a7cb}',  // Latin letter new in Unicode 16
    '\u{4e00}',
    'é',
    'Σ',
    '_',
    '-',
    '.',
    '@',
    '/',
    ':',
    '%',
    '+',
    '"',
    '\'',
    '\\',
    '=',
    '(',
    ',',
    '}',
    ']',
    '{',
    '[',
    '<',
    '>',
    '$',
    '*',
    'x',
    '\t',
    '\r',
    ' ',
];

/// Hostile units, repeated up to about 50 KB.
fn hostile() -> Vec<String> {
    let mut units: Vec<String> = HOSTILE.iter().map(|s| (*s).to_string()).collect();
    units.push(format!("{}/x", u()));
    units.push(format!("a://h{}/x ", u()));
    units
}

const HOSTILE: &[&str] = &[
    "a",
    "a/",
    "-",
    "a.",
    "a@",
    "eyJ-",
    "fe80:",
    "a://b:",
    "secret_key",
    "secret_key\"",
    "1.",
    "a://",
    "\"password\":",
    "\"token\": null, ",
    "\\\"password\\\": \\\"",
    "${",
    "<",
    "x",
    "\"",
    " ",
    "b.",
    "1",
    "::",
    "%en0",
    "id_",
    "ssh ",
];

fn pick_home(rng: &mut Rng) -> String {
    let choices = [
        "/srv/zed".to_string(),
        format!("{}/zed", u()),
        String::new(),
        "/".into(),
        "//".into(),
        "/x".into(),
        format!("{}/zed/", h()),
        "/opt/\u{fc}n\u{ef}".into(),
        "/srv/a.b".into(),
        "/srv/(x)".into(),
        "C:\\Users\\zed".into(),
    ];
    if rng.chance(0.1) {
        return format!("/{}", rng.string(10));
    }
    rng.pick(&choices).clone()
}

fn pick_host(rng: &mut Rng) -> String {
    let host = host();
    let choices = [
        host.clone(),
        host.to_uppercase(),
        format!("{host}.local"),
        format!("{host}.LOCAL."),
        format!(" {host}\n"),
        format!("{host}.corp.acme.zz"),
        "debian.local".into(),
        "ubuntu".into(),
        "MacBook-Pro".into(),
        "wor\u{212a}station".into(),
        "abc".into(),
        "abcd".into(),
        String::new(),
        ".local".into(),
        "box.".into(),
        "\u{130}stanbul-box".into(),
        "\u{dc}n\u{ef}-box".into(),
        "k\u{131}wi-box".into(),
        "\u{131}stanbul".into(),
        "\u{17f}erver-01".into(),
        "\u{1e9e}tra\u{df}e-box".into(),
        "\u{3a3}\u{3bf}\u{3c6}\u{3af}\u{3b1}-box".into(),
        "\u{3c2}\u{3d1}\u{3f0}\u{3f1}-box".into(),
        "\u{1c5}ungla-box".into(),
        "\u{13a0}\u{13a1}\u{13a2}-box".into(),
        "nas.lan".into(),
        "a.b".into(),
        "x-y".into(),
    ];
    let k = rng.f64();
    if k < 0.08 {
        return rng.ascii_string(12);
    }
    if k < 0.16 {
        // Cased letters of every kind CPython's IGNORECASE tables touch.
        let cased = cased_letters();
        let n = rng.index(6) + 4;
        let name: String = (0..n).map(|_| *rng.pick(cased)).collect();
        return format!("{name}-box");
    }
    rng.pick(&choices).clone()
}

fn cased_letters() -> &'static [char] {
    static CASED: OnceLock<Vec<char>> = OnceLock::new();
    CASED.get_or_init(|| {
        privacy::case_candidates()
            .into_iter()
            .filter_map(char::from_u32)
            .collect()
    })
}

/// `v` in a random spelling: upper, lower, the other case of some letters, or with İ ı ſ K
/// written as the ASCII letters they fold to (and back).
fn respell(rng: &mut Rng, v: &str) -> String {
    match rng.index(5) {
        0 => v.to_uppercase(),
        1 => v.to_lowercase(),
        2 => v
            .chars()
            .map(|c| match c {
                '\u{131}' | '\u{130}' => 'i',
                '\u{17f}' => 's',
                '\u{212a}' => 'k',
                'i' => '\u{130}',
                's' => '\u{17f}',
                'k' => '\u{212a}',
                other => other,
            })
            .collect(),
        3 => v
            .chars()
            .flat_map(|c| {
                let flip: Vec<char> = if rng.chance(0.5) {
                    c.to_uppercase().collect()
                } else {
                    c.to_lowercase().collect()
                };
                flip
            })
            .collect(),
        _ => v.to_string(),
    }
}

fn piece(rng: &mut Rng, frags: &[String], home: &str, host: &str) -> String {
    let k = rng.f64();
    if k < 0.45 {
        let mut f = rng.pick(frags).clone();
        if rng.chance(0.15) {
            mutate(rng, &mut f);
        }
        f
    } else if k < 0.65 {
        rng.pick(BOUNDARY).to_string()
    } else if k < 0.75 {
        rng.ascii_string(10)
    } else if k < 0.82 {
        rng.string(6)
    } else if k < 0.88 {
        // The dynamic patterns' values, in other cases and next to other characters.
        let v = if rng.chance(0.5) { home } else { host };
        let v = v.trim().trim_end_matches('.');
        // Sometimes the short name of an FQDN, which the scan also looks for.
        let short = rng.chance(0.3);
        let v = if short {
            v.split('.').next().unwrap_or(v)
        } else {
            v
        };
        respell(rng, v)
    } else if k < 0.94 {
        let units = hostile();
        let unit = rng.pick(&units).clone();
        let max = (50_000 / unit.len().max(1)).max(1);
        let n = if rng.chance(0.7) {
            rng.index(64) + 1
        } else {
            rng.index(max) + 1
        };
        unit.repeat(n)
    } else {
        rng.pick(&[" ", "\t", "\n", ", ", "; ", " (", ") "])
            .to_string()
    }
}

/// A small edit: a character dropped, doubled or swapped for a boundary character.
fn mutate(rng: &mut Rng, s: &mut String) {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return;
    }
    let i = rng.index(chars.len());
    let mut out: Vec<char> = chars.clone();
    match rng.index(4) {
        0 => {
            out.remove(i);
        }
        1 => out.insert(i, chars[i]),
        2 => out[i] = *rng.pick(BOUNDARY),
        _ => out.insert(i, *rng.pick(BOUNDARY)),
    }
    *s = out.into_iter().collect();
}

fn gen_case(rng: &mut Rng) -> Value {
    let frags = fragments();
    let home = pick_home(rng);
    let host = pick_host(rng);
    let n = rng.index(10) + 1;
    let text: String = (0..n)
        .map(|_| piece(rng, &frags, &home, &host))
        .collect::<Vec<_>>()
        .join(if rng.chance(0.5) { " " } else { "" });
    findings_case(&text, &home, &host)
}

/// The inputs of the Python tests, one case each, and each fragment alone.
fn seed_texts() -> Vec<String> {
    let mut out = fragments();
    let mail = at("alice", "mail.acme.zz");
    let (u, h) = (u(), h());
    out.push(format!(
        "clean line\n{u}/alice/notes and {u}/bob/x\nwrite to {mail}\n{h}/carol\n"
    ));
    out.push(
        "# Build\nRun `make test` in Docker. See [[project-ci]].\n\
         Version 1.2.3 ships on 2026-04-05; the API listens on port 8080.\n"
            .into(),
    );
    for f in fragments() {
        out.push(format!("x\n{f}\n"));
        out.push(format!("\u{301}{f}\u{301}"));
        out.push(format!("\u{b2}{f}\u{b2}"));
    }
    // Hostile shapes at a moderate size (each well under the default budget).
    for unit in hostile() {
        out.push(unit.repeat(20_000 / unit.len().max(1)));
    }
    out
}

/// Cases for the dynamic patterns: (text, home, hostname).
fn seed_dynamic() -> Vec<(String, String, String)> {
    let host = host();
    let home = ["/", "srv", "/people/zed"].concat();
    vec![
        (
            format!("cache in {home}/.cache"),
            home.clone(),
            host.clone(),
        ),
        (
            format!("cache in {home}s/.cache"),
            home.clone(),
            host.clone(),
        ),
        (format!("in {home}-x"), home.clone(), host.clone()),
        (format!("in {home}\u{301}"), home.clone(), host.clone()),
        ("just / here".into(), "/".into(), host.clone()),
        (
            format!("built on {host}"),
            home.clone(),
            format!("{host}.local"),
        ),
        (
            format!("built on {}", host.to_uppercase()),
            home.clone(),
            host.clone(),
        ),
        (format!("built on {host}-2"), home.clone(), host.clone()),
        (
            format!("built on {host}\u{301}"),
            home.clone(),
            host.clone(),
        ),
        ("a box called abc".into(), home.clone(), "abc".into()),
        ("on localhost".into(), home.clone(), "localhost".into()),
        (
            "runs on workstation".into(),
            home.clone(),
            "wor\u{212a}station".into(),
        ),
        (
            "runs on debian today".into(),
            home.clone(),
            "debian.local".into(),
        ),
        (
            "on \u{130}STANBUL-BOX now".into(),
            home.clone(),
            "istanbul-box".into(),
        ),
        (
            "on istanbul-box now".into(),
            home.clone(),
            "\u{130}stanbul-box".into(),
        ),
        (
            "on \u{dc}N\u{cf}-BOX now".into(),
            home.clone(),
            "\u{fc}n\u{ef}-box".into(),
        ),
        ("on corp7 now".into(), home.clone(), "corp7.acme.zz".into()),
        // ı folds with i, I and İ in CPython (review I-1).
        (
            "on kiwi-box now".into(),
            home.clone(),
            "k\u{131}wi-box".into(),
        ),
        (
            "on KIWI-BOX now".into(),
            home.clone(),
            "k\u{131}wi-box".into(),
        ),
        (
            "on k\u{130}wi-box".into(),
            home.clone(),
            "k\u{131}wi-box".into(),
        ),
        ("Istanbul".into(), home.clone(), "\u{131}stanbul".into()),
        (
            "on server-01".into(),
            home.clone(),
            "\u{17f}erver-01".into(),
        ),
        (
            "on \u{3c3}\u{3bf}\u{3c6}\u{3b9}\u{3b1}-box".into(),
            home.clone(),
            "\u{3a3}\u{3bf}\u{3c6}\u{3af}\u{3b1}-box".into(),
        ),
        (
            "on stra\u{df}e-box".into(),
            home.clone(),
            "\u{1e9e}tra\u{df}e-box".into(),
        ),
        ("plain text".into(), home, String::new()),
    ]
}
