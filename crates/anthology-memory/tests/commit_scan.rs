//! The commit scan (`anthology_memory::commit_scan`): what `anthology privacy hook` looks for in
//! the staged diff and the message of a `git commit`. Only the owner's own literals block: the
//! home path and the handles, the git email, the machine and tailnet names and the banned names.
//!
//! Fixtures are assembled at run time: no real home path, email or host name sits in this file.

use anthology_memory::commit_scan::{
    CommitFinding, CommitKind, ScanContext, banned_names, block_message, scan_diff, scan_message,
};

fn users() -> String {
    ["/", "Users"].concat()
}

fn home_dir() -> String {
    ["/", "home"].concat()
}

/// The home folder of the test owner.
fn home() -> String {
    users() + "/zed"
}

fn at(parts: &[&str]) -> String {
    parts.join("@")
}

/// A diff of one file with one hunk of added lines, as `git diff -U0` prints it.
fn one_file(file: &str, first: usize, lines: &[&str]) -> String {
    let mut out = format!("diff --git a/{file} b/{file}\nindex 1111111..2222222 100644\n");
    out.push_str(&format!("--- a/{file}\n+++ b/{file}\n"));
    out.push_str(&format!(
        "@@ -{},0 +{},{} @@\n",
        first.saturating_sub(1),
        first,
        lines.len()
    ));
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn ctx() -> ScanContext {
    ScanContext {
        home: Some(home()),
        handles: vec!["zed".to_string()],
        ..ScanContext::default()
    }
}

fn found(kind: CommitKind, file: &str, line: usize) -> CommitFinding {
    CommitFinding {
        kind,
        file: file.to_string(),
        line,
    }
}

/// The lines of `file` a scan flagged with `kind`.
fn lines_of(findings: &[CommitFinding], kind: CommitKind) -> Vec<usize> {
    findings
        .iter()
        .filter(|f| f.kind == kind)
        .map(|f| f.line)
        .collect()
}

#[test]
fn home_literal_and_handle_forms_block() {
    let (u, h) = (users(), home_dir());
    // In order: the $HOME literal; /home/<handle>; \Users\<handle>; the dash form of a project
    // folder; ASCII case-insensitive; $HOME at the end of the line; a handle form with a quote
    // after it; the dash form of a dot folder.
    let lines = [
        format!("see {}/proj/notes.md", home()),
        format!("cat {h}/zed/.config/x"),
        "\\Users\\zed\\Desktop".to_string(),
        "cache -Users-zed-Desktop-proj".to_string(),
        format!("{u}/ZED/Desktop"),
        format!("export HOME={}", home()),
        format!("path = \"{u}/zed\""),
        "dir -home-zed--claude-worktrees".to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let got = scan_diff(&one_file("notes.md", 1, &refs), &ctx());
    assert_eq!(
        got,
        (1..=8)
            .map(|n| found(CommitKind::HomePath, "notes.md", n))
            .collect::<Vec<_>>()
    );
}

#[test]
fn other_homes_and_forge_urls_pass() {
    let u = users();
    let mut c = ctx();
    c.handles = vec!["h".to_string()];
    c.home = Some(u.clone() + "/h");
    let lines = [
        format!("WORKDIR {}/node/app", home_dir()),
        format!("open {u}/Shared/Library"),
        "git clone https://github.com/h/repo.git".to_string(),
        format!("{u}/hx/project"),
        format!("{u}/h2"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut diff = one_file("a.txt", 1, &refs);
    // A removed line is not part of the commit, whatever it holds.
    diff.push_str(&format!("@@ -9 +9 @@\n-cd {u}/h/work\n+cd $HOME/../x\n"));
    assert_eq!(scan_diff(&diff, &c), Vec::new());
}

#[test]
fn removed_and_context_lines_are_never_scanned() {
    let removed = format!(
        "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -3,2 +3,1 @@\n-cd {}/work\n context {}\n+fine\n",
        home(),
        home()
    );
    assert_eq!(scan_diff(&removed, &ctx()), Vec::new());
}

#[test]
fn emails() {
    let owner = at(&["owner", "mailbox.dev"]);
    let mut c = ctx();
    c.handles = vec!["h".to_string()];
    c.emails = vec![owner.clone()];
    // Lines 1 to 3 block: the configured address; a local part that is a handle; the configured
    // address in other case. The rest pass: somebody else's address; not an address; a reserved
    // domain; the forge user; GitHub's no-reply address.
    let lines = [
        format!("Author: {owner}"),
        format!("contact {}", at(&["h", "gmail.com"])),
        at(&["OWNER", "Mailbox.dev"]),
        format!("mail {}", at(&["support", "acme.io"])),
        at(&["noreply", "x"]),
        at(&["a", "example.com"]),
        format!("git clone {}:org/repo.git", at(&["git", "github.com"])),
        at(&["123+h", "users.noreply.github.com"]),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let got = scan_diff(&one_file("README.md", 1, &refs), &c);
    assert_eq!(lines_of(&got, CommitKind::UserEmail), [1, 2, 3]);
    assert_eq!(got.len(), 3, "{got:?}");
}

#[test]
fn an_exempt_address_passes_even_when_it_is_the_configured_one() {
    let mut c = ctx();
    c.emails = vec![at(&["12345+zed", "users.noreply.github.com"])];
    let line = format!("Author {}", at(&["12345+zed", "users.noreply.github.com"]));
    assert_eq!(scan_diff(&one_file("a", 1, &[&line]), &c), Vec::new());
}

#[test]
fn banned_names_read_global_and_per_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("personal-memory");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("user-banned-names.md"),
        "---\nname: Banned names\ntags:\n- frontmatter\ntype: user\n---\n\n- Acme Corp\n* `Globex`\n- \"Initech\"\n- ab\nplain text\n  - Hooli\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("user-banned-names--proj.md"),
        "- Umbrella\n- Initech\n",
    )
    .unwrap();

    let mut global = banned_names(tmp.path(), "other");
    global.sort();
    assert_eq!(global, ["Acme Corp", "Globex", "Hooli", "Initech"]);
    let mut per_repo = banned_names(tmp.path(), "proj");
    per_repo.sort();
    assert_eq!(
        per_repo,
        ["Acme Corp", "Globex", "Hooli", "Initech", "Umbrella"]
    );
    // a name that tries to leave the folder reads nothing of its own
    assert_eq!(banned_names(tmp.path(), "../x").len(), global.len());
    assert!(banned_names(&tmp.path().join("missing"), "proj").is_empty());

    let mut c = ctx();
    c.banned = per_repo;
    let lines = [
        "built for ACME CORP",      // 1
        "the initech-fork project", // 2: `-` is not a word character
        "umbrella.js",              // 3
        "acme corporation",         // 4: the name ends inside a longer word
        "Initechnology",            // 5: inside a word
        "(Hooli)",                  // 6
        "an acme",                  // 7: only the first word of a name
    ];
    let got = scan_diff(&one_file("a.md", 1, &lines), &c);
    assert_eq!(lines_of(&got, CommitKind::BannedName), [1, 2, 3, 6]);
}

#[test]
fn the_banned_name_of_a_commit_message_is_found() {
    let mut c = ctx();
    c.banned = vec!["Globex".to_string()];
    let got = scan_message("fix a bug\n\nported from globex\n", &c);
    assert_eq!(got, [found(CommitKind::BannedName, "commit message", 3)]);
}

#[test]
fn allow_list_drops_findings() {
    let path = format!("{}/proj/notes.md", home());
    let mut c = ctx();
    c.allow = vec![
        "# the public sample path".to_string(),
        String::new(),
        format!("  {}  ", path.to_uppercase()),
    ];
    let other = format!("{}/other/file", home());
    let lines = [
        format!("see {path}"),
        format!("see {other}"),
        format!("(see {path}.)"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let got = scan_diff(&one_file("a.md", 1, &refs), &c);
    // The allowed line names one path, not the home folder: another path under it still blocks,
    // and a path that ends a sentence is the same path.
    assert_eq!(got, [found(CommitKind::HomePath, "a.md", 2)]);
    // a comment line allows nothing
    let mut c = ctx();
    c.allow = vec![format!("# {}", home())];
    assert_eq!(scan_diff(&one_file("a.md", 1, &[&path]), &c).len(), 1);
}

#[test]
fn machine_name_and_tailnet() {
    let host = ["alice-", "laptop"].concat();
    let mut c = ctx();
    c.hostname = Some(host.clone());
    // Lines 1 to 3 block: the name; a tailnet host; the name in other case. The rest pass: a
    // generic name; a cluster name; the name inside a longer one; a host that only starts like a
    // tailnet one.
    let lines = [
        format!("ssh {host}"),
        "ping box.tail1234.ts.net".to_string(),
        format!("{}.local is up", host.to_uppercase()),
        "macbook-pro".to_string(),
        "svc.cluster.local".to_string(),
        format!("my-{host}-two"),
        "see foo.ts.network.example".to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let got = scan_diff(&one_file("ops.md", 1, &refs), &c);
    assert_eq!(lines_of(&got, CommitKind::MachineName), [1, 2, 3]);
    assert_eq!(got.len(), 3, "{got:?}");

    // a generic host name is nobody's own
    let mut c = ctx();
    c.hostname = Some("macbook-pro".to_string());
    assert_eq!(
        scan_diff(&one_file("a", 1, &["macbook-pro and ubuntu"]), &c),
        Vec::new()
    );
}

#[test]
fn line_numbers_follow_hunks() {
    let h = home();
    let diff = format!(
        "diff --git a/one.md b/one.md\nindex 1..2 100644\n--- a/one.md\n+++ b/one.md\n\
@@ -4,0 +5,2 @@\n+clean\n+at {h}\n\
@@ -10,2 +14,3 @@ fn x\n-old {h}\n-old\n+clean\n+also clean\n+and {h} again\n\
diff --git a/dir/two.md b/dir/two.md\nnew file mode 100644\n--- /dev/null\n+++ b/dir/two.md\n\
@@ -0,0 +1 @@\n+{h}\n\
diff --git a/gone.md b/gone.md\ndeleted file mode 100644\n--- a/gone.md\n+++ /dev/null\n\
@@ -1 +0,0 @@\n-{h}\n\
diff --git a/three.md b/three.md\n--- a/three.md\n+++ b/three.md\n\
@@ -7 +7 @@\n-x\n+{h}\n\\ No newline at end of file\n"
    );
    assert_eq!(
        scan_diff(&diff, &ctx()),
        [
            found(CommitKind::HomePath, "one.md", 6),
            found(CommitKind::HomePath, "one.md", 16),
            found(CommitKind::HomePath, "dir/two.md", 1),
            found(CommitKind::HomePath, "three.md", 7),
        ]
    );
}

#[test]
fn an_added_line_that_starts_with_two_pluses_is_a_line_not_a_header() {
    let line = format!("++ {}/x", home());
    let got = scan_diff(&one_file("a.txt", 3, &[&line]), &ctx());
    assert_eq!(got, [found(CommitKind::HomePath, "a.txt", 3)]);
}

#[test]
fn the_file_header_is_not_scanned() {
    let name = format!("{}/zed-notes.md", home().trim_start_matches('/'));
    let diff =
        format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -0,0 +1 @@\n+fine\n");
    assert_eq!(scan_diff(&diff, &ctx()), Vec::new());
}

#[test]
fn a_message_is_scanned_line_by_line() {
    let msg = format!(
        "fix: tidy\n\nran in {}/proj\nand on {}/zed\n",
        home(),
        home_dir()
    );
    assert_eq!(
        scan_message(&msg, &ctx()),
        [
            found(CommitKind::HomePath, "commit message", 3),
            found(CommitKind::HomePath, "commit message", 4),
        ]
    );
    assert_eq!(scan_message("fix: tidy\n", &ctx()), Vec::new());
}

#[test]
fn one_finding_per_kind_and_line() {
    let line = format!("{} and {}/zed", home(), home_dir());
    let got = scan_diff(&one_file("a", 1, &[&line]), &ctx());
    assert_eq!(got, [found(CommitKind::HomePath, "a", 1)]);
}

#[test]
fn block_message_never_holds_the_value() {
    let owner = at(&["owner", "mailbox.dev"]);
    let mut c = ctx();
    c.emails = vec![owner.clone()];
    c.banned = vec!["Globex".to_string()];
    let lines = [
        format!("one {}/a", home()),
        format!("two {}/b", home()),
        format!("three {}/c", home()),
        format!("four {}/d", home()),
        format!("five {}/e", home()),
        format!("by {owner} for globex"),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let got = scan_diff(&one_file("src/lib.rs", 1, &refs), &c);
    assert_eq!(got.len(), 7, "{got:?}");
    let msg = block_message(&got);
    assert_eq!(
        msg,
        "Blocked by anthology (privacy): this commit adds home path (src/lib.rs:1, src/lib.rs:2, src/lib.rs:3, and 2 more); user email (src/lib.rs:6); banned name (src/lib.rs:6). Replace a home path with a repo-relative path, a name with a neutral one or the project's own, then stage and commit again. A value the owner wants public: ask the owner to add it as a line in .claude/privacy-allow.txt."
    );
    for value in [home(), owner, "globex".to_string(), "Globex".to_string()] {
        assert!(!msg.contains(&value), "the message holds {value:?}");
    }
}

#[test]
fn block_message_escapes_control_characters_in_a_file_name() {
    let f = [found(CommitKind::HomePath, "a\u{1b}[31mb.md", 2)];
    let msg = block_message(&f);
    assert!(!msg.contains('\u{1b}'), "{msg:?}");
    assert!(msg.contains("(a\\u{1b}[31mb.md:2)"), "{msg}");
}

#[test]
fn a_context_with_nothing_to_look_for_finds_only_tailnet_names() {
    let c = ScanContext::default();
    let lines = [format!("{}/zed", users()), "box.tail1.ts.net".to_string()];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_eq!(
        scan_diff(&one_file("a", 1, &refs), &c),
        [found(CommitKind::MachineName, "a", 2)]
    );
}
