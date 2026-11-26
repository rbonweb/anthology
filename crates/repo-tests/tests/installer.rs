//! The one-command installer (`crates/anthology-installer`): the places where it has to agree
//! with files elsewhere in the repository.

use std::process::Command;

use repo_tests::repo;

/// The command the installer runs for the guard binary is the one `anth_fix_cmd` builds in each
/// plugin's `lib.sh`, which is the only command the guard lets through while the binary is
/// missing.
#[test]
fn the_bootstrap_command_matches_what_anth_fix_cmd_builds() {
    for plugin in ["anthology", "anthology-guardrails"] {
        let lib = repo::path(&format!("plugins/{plugin}/hooks/lib.sh"));
        let hooks = "/opt/plugin/hooks";
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#". "$1" && anth_fix_cmd "$2" && printf '%s' "$ANTH_FIX""#)
            .arg("sh")
            .arg(&lib)
            .arg(hooks)
            .output()
            .expect("run sh");
        assert!(
            out.status.success(),
            "{plugin}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            anthology_installer::binary::bootstrap_command(hooks),
            "{plugin}: lib.sh and the installer disagree"
        );
    }
}
