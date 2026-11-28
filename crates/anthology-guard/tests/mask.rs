//! `anthology_guard::mask`: the secret masker behind `anthology guard mask`. Fixture secrets are
//! built at run time (`concat`) so no whole secret sits in this file for a scanner to find.

use std::path::{Path, PathBuf};

use anthology_guard::mask::{MASK_LABELS, Masked, marker, mask_secrets};
use anthology_guard::secrets::GUARD_SECRET_PATTERNS;

fn aws() -> String {
    ["AKIA", "IOSFODNN7", "EXAMPLE"].concat()
}

fn ghp() -> String {
    ["ghp", "_", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8"].concat()
}

fn github_pat() -> String {
    ["github", "_pat_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"].concat()
}

fn sk() -> String {
    ["sk", "-", "Ab3dEfGh1jKlMnOpQr5tUv"].concat()
}

fn slack() -> String {
    ["xox", "b-", "1234567890-abcdefghij"].concat()
}

fn begin() -> String {
    ["-----BEGIN ", "PRIVATE KEY-----"].concat()
}

fn end() -> String {
    ["-----END ", "PRIVATE KEY-----"].concat()
}

const BODY: &str = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7";

fn unchanged(text: &str) {
    let m = mask_secrets(text);
    assert_eq!(m.count, 0, "masked something in {text:?}: {:?}", m.text);
    assert_eq!(m.text, text);
}

/// The text with exactly `secret` replaced by the marker for `label`.
fn masked_once(text: &str, secret: &str, label: &str) {
    let m = mask_secrets(text);
    assert_eq!(m.count, 1, "{text:?} -> {:?}", m.text);
    assert_eq!(m.text, text.replacen(secret, &marker(label), 1));
}

#[test]
fn masks_each_label_and_keeps_the_rest() {
    let aws = aws();
    masked_once(&format!("export KEY_ID={aws}\n"), &aws, "aws-access-key");
    let ghp = ghp();
    masked_once(&format!("token: {ghp} (read)\n"), &ghp, "github-token");
    let pat = github_pat();
    masked_once(
        &format!("remote https://{pat}@host/x\n"),
        &pat,
        "github-token",
    );
    let sk = sk();
    masked_once(&format!("key is {sk}.\n"), &sk, "api-key");
    let slack = slack();
    masked_once(&format!("SLACK={slack} next\n"), &slack, "slack-token");

    // A private key: the header lines stay, the body is one marker line.
    let key = format!("{}\n{BODY}\n{BODY}\n{}\n", begin(), end());
    let m = mask_secrets(&key);
    assert_eq!(m.count, 1);
    assert_eq!(
        m.text,
        format!("{}\n{}\n{}\n", begin(), marker("private-key"), end())
    );

    // Credential values keep their quotes and everything around them.
    let m = mask_secrets("const c = { password: \"s3cr3tV4lue\", user: 'bob' }\n");
    assert_eq!(m.count, 1);
    assert_eq!(
        m.text,
        format!(
            "const c = {{ password: \"{}\", user: 'bob' }}\n",
            marker("credential")
        )
    );
    let value = ["q9w8", "e7r6", "t5y4"].concat();
    let m = mask_secrets(&format!("api_key = '{value}'\n"));
    assert_eq!(m.count, 1);
    assert_eq!(m.text, format!("api_key = '{}'\n", marker("credential")));
    let m = mask_secrets(&format!("\"apiKey\": \"{value}\"\n"));
    assert_eq!(m.count, 1);
    assert_eq!(
        m.text,
        format!("\"apiKey\": \"{}\"\n", marker("credential"))
    );
}

#[test]
fn a_secret_inside_a_longer_blob_or_with_too_little_after_it_stays() {
    let aws = aws();
    // Rule 0: a base64 neighbor on either side means it is no key.
    unchanged(&format!("x{aws}\n"));
    unchanged(&format!("{aws}y\n"));
    unchanged(&format!("a/{aws}\n"));
    unchanged(&format!("{aws}==\n"));
    // Rule 1: under 30 characters after the underscore.
    unchanged("ghp_abcdefghijklmnopqrstuvwxyz\n");
    // Rule 2: under 30 after `github_pat_`.
    unchanged(&format!("{}\n", ["github", "_pat_", "short"].concat()));
    // Rule 3: needs a digit and a capital.
    unchanged("sk-abcdefghijklmnopqrstuvwx\n");
    unchanged("sk-ABCDEFGHIJKLMNOPQRSTUVWX\n");
    // Rule 4: needs a digit and 10 characters.
    unchanged(&format!("{}\n", ["xox", "b-", "abcdefghijklmnop"].concat()));
    unchanged(&format!("{}\n", ["xox", "b-", "123"].concat()));
}

fn pat(tail: &str) -> String {
    ["github", "_pat_", tail].concat()
}

#[test]
fn a_long_identifier_that_starts_like_a_github_pat_stays() {
    // Names, not tokens: no digit, or too short.
    unchanged(&format!("{}\n", pat("example_variable_name_here")));
    unchanged(&format!(
        "let x = {}_value;\n",
        pat("example_variable_name_here")
    ));
    unchanged(&format!(
        "{}\n",
        pat("token_value_here_and_more_text_after")
    ));
    unchanged(&format!("{}\n", pat(&"AbCdEfGhIj".repeat(3))));
    // A digit, but under 30 characters.
    unchanged(&format!("{}\n", pat(&"A1b2C3d4E5".repeat(3)[..29])));
}

#[test]
fn a_real_shaped_github_pat_is_still_masked() {
    // 11 characters, an underscore and 59 more: the shape GitHub issues.
    let tail = [
        "11AbCdEfG0",
        "x",
        "_",
        "aB3dEfGhIj",
        "KlMnOpQrSt",
        "Uv1wXyZ234",
        "5678AbCdEf",
        "GhIjKlMnOp",
        "QrStUvWxY",
    ]
    .concat();
    assert!(tail.len() >= 70, "{}", tail.len());
    masked_once(
        &format!("token {}\n", pat(&tail)),
        &pat(&tail),
        "github-token",
    );
    // The shortest tail that counts: 30 characters with a digit.
    let edge = format!("A1{}", "b".repeat(28));
    assert_eq!(edge.len(), 30);
    masked_once(&format!("{}\n", pat(&edge)), &pat(&edge), "github-token");
}

#[test]
fn keeps_lockfiles_shas_and_code_unchanged() {
    let sri = format!("  \"integrity\": \"sha512-{}==\",\n", "A".repeat(86));
    let checksum = format!("checksum = \"{}\"\n", "0123456789abcdef".repeat(4));
    let lines = [
        sri.as_str(),
        checksum.as_str(),
        "9f8e7d6 fix(auth): password: reset flow\n",
        "3c4b5a2 chore: rotate api_key = old\n",
        "password = os.environ[\"DB_PASSWORD\"]\n",
        "api_key: ${API_KEY}\n",
        "\"password\": \"Password\"\n",
        ".sk-loading-spinner-wrapper-large {\n",
        "ghp_short\n",
        "let pem = \"-----BEGIN PRIVATE KEY-----\\n\" + body;\n",
        "password = config.db.password\n",
        "  password: userPassword1,\n",
    ];
    for line in lines {
        unchanged(line);
    }
    // All of them in one text, as a command prints them.
    unchanged(&lines.concat());
}

#[test]
fn sri_tokens_are_never_masked() {
    // A url-safe digest can hold `-sk-` and more; the whole token is skipped.
    let inner = format!("X-{}", sk());
    unchanged(&format!("\"integrity\": \"sha512-{inner}\"\n"));
    unchanged(&format!("sha256-{inner}\n"));
    unchanged(&format!("sha1-{inner} sha384-{inner}\n"));
    // Without the prefix the same text is a key.
    assert_eq!(mask_secrets(&format!("shaX-{inner}\n")).count, 1);
}

#[test]
fn masks_env_and_yaml_values() {
    let m = mask_secrets("DB_PASSWORD=hunter2xyz\n");
    assert_eq!(m.count, 1);
    assert_eq!(m.text, format!("DB_PASSWORD={}\n", marker("credential")));
    let m = mask_secrets("export API_KEY=hunter2xyz\n");
    assert_eq!(m.text, format!("export API_KEY={}\n", marker("credential")));
    let m = mask_secrets("password: hunter2xyz\n");
    assert_eq!(m.count, 1);
    assert_eq!(m.text, format!("password: {}\n", marker("credential")));
    let m = mask_secrets("  db_password: hunter2xyz\n");
    assert_eq!(m.text, format!("  db_password: {}\n", marker("credential")));
    // Code that assigns a variable keeps it.
    unchanged("password = some_var2\n");
    unchanged("    password = hunter2xyz\n");
    unchanged("password == hunter2xyz\n");
}

#[test]
fn a_value_is_masked_only_when_it_looks_like_a_secret() {
    // Too short, spaced, an expression or a placeholder-shaped text.
    unchanged("password = \"abc123\"\n");
    unchanged("password: \"two words 12345\"\n");
    unchanged("password = get_password(user)\n");
    unchanged("password = \"%s1234567\"\n");
    unchanged("password = <your-password-1>\n");
    unchanged("password: Foo::Bar::Baz9\n");
    unchanged("password = a->b->c1\n");
    // A long value of letters, or letters with a symbol, is a secret.
    let m = mask_secrets("password = \"abcdefghijklmnopqrstuvwxyz\"\n");
    assert_eq!(m.count, 1);
    let m = mask_secrets("password = \"correct-horse!\"\n");
    assert_eq!(m.count, 1);
    // An unquoted value ends at a comma or a closing bracket.
    let m = mask_secrets("{ password: hunter2xyz, user: bob }\n");
    assert_eq!(
        m.count, 0,
        "an identifier-shaped value off an env line stays"
    );
    let m = mask_secrets("{ api_key: \"hunter2-xyz\", user: bob }\n");
    assert_eq!(
        m.text,
        format!("{{ api_key: \"{}\", user: bob }}\n", marker("credential"))
    );
}

#[test]
fn private_key_body_is_one_marker_line() {
    let key = format!("{}\n{BODY}\n{BODY}\n{BODY}\n{}\nafter\n", begin(), end());
    let m = mask_secrets(&key);
    assert_eq!(m.count, 1);
    assert_eq!(
        m.text,
        format!("{}\n{}\n{}\nafter\n", begin(), marker("private-key"), end())
    );
    // No END line: unchanged.
    unchanged(&format!("{}\n{BODY}\n{BODY}\n", begin()));
    // A line that is not base64 inside the body: unchanged.
    unchanged(&format!(
        "{}\n{BODY}\nnot base64 at all\n{}\n",
        begin(),
        end()
    ));
    // Nothing between the headers: nothing to mask.
    unchanged(&format!("{}\n{}\n", begin(), end()));
    // The END line is more than 200 lines away.
    let far = format!(
        "{}\n{}{}\n",
        begin(),
        format!("{BODY}\n").repeat(201),
        end()
    );
    unchanged(&far);
    let near = format!(
        "{}\n{}{}\n",
        begin(),
        format!("{BODY}\n").repeat(199),
        end()
    );
    assert_eq!(mask_secrets(&near).count, 1);
}

#[test]
fn private_key_lines_with_a_prefix_or_headers_are_masked() {
    // grep -n and cat -n prefixes, and the PEM headers of an encrypted key.
    let grep = format!(
        "key.pem:1:{}\nkey.pem:2:Proc-Type: 4,ENCRYPTED\nkey.pem:3:DEK-Info: AES-128-CBC,0123\nkey.pem:4:\nkey.pem:5:{BODY}\nkey.pem:6:{}\n",
        begin(),
        end()
    );
    let m = mask_secrets(&grep);
    assert_eq!(m.count, 1);
    assert_eq!(
        m.text,
        format!(
            "key.pem:1:{}\n{}\nkey.pem:6:{}\n",
            begin(),
            marker("private-key"),
            end()
        )
    );
    let cat = format!("     1\t{}\n     2\t{BODY}\n     3\t{}\n", begin(), end());
    let m = mask_secrets(&cat);
    assert_eq!(m.count, 1);
    assert_eq!(
        m.text,
        format!(
            "     1\t{}\n{}\n     3\t{}\n",
            begin(),
            marker("private-key"),
            end()
        )
    );
}

#[test]
fn line_endings_and_a_missing_final_newline_survive() {
    let aws = aws();
    let m = mask_secrets(&format!("a\r\nid {aws}\r\nb"));
    assert_eq!(m.text, format!("a\r\nid {}\r\nb", marker("aws-access-key")));
    let m = mask_secrets(&aws);
    assert_eq!(m.text, marker("aws-access-key"));
    unchanged("");
    unchanged("\n\n");
}

#[test]
fn overlapping_hits_make_one_marker() {
    // A credential whose value is also a key: one marker, count 1.
    let m = mask_secrets(&format!("api_key={}\n", sk()));
    assert_eq!(m.count, 1);
    assert!(!m.text.contains("Ab3dEf"), "{}", m.text);
}

#[test]
fn labels_cover_every_pattern() {
    assert_eq!(MASK_LABELS.len(), GUARD_SECRET_PATTERNS.len());
    assert_eq!(
        MASK_LABELS,
        [
            "aws-access-key",
            "github-token",
            "github-token",
            "api-key",
            "slack-token",
            "private-key",
            "credential",
            "credential"
        ]
    );
    assert_eq!(marker("credential"), "[masked by anthology: credential]");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The owner reads these files through Bash all day: no line of them may be masked.
#[test]
fn repo_sources_mask_nothing() {
    let root = repo_root();
    let mut files: Vec<PathBuf> = ["Cargo.lock", "fuzz/Cargo.lock", "atl/Cargo.lock"]
        .iter()
        .map(|p| root.join(p))
        .collect();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates")
        .flatten()
    {
        rust_sources(&entry.path().join("src"), &mut files);
    }
    assert!(files.len() > 50, "found only {} files", files.len());
    for file in files {
        // A checkout without a file (a sparse copy) has nothing to check there.
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Masked { count, text: out } = mask_secrets(&text);
        if count != 0 {
            let first = text
                .lines()
                .zip(out.lines())
                .position(|(a, b)| a != b)
                .map_or(0, |i| i + 1);
            panic!(
                "{} masked {count} time(s), first at line {first}",
                file.display()
            );
        }
        assert_eq!(out, text, "{}", file.display());
    }
}
