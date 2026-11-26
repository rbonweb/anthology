//! The anthology binary: is it installed, and the step that installs it through the guardrails
//! plugin's own `hooks/bootstrap.sh`.

use std::path::Path;

/// True when `<ah>/bin/<version>/anthology` exists for some version. Hidden folders
/// (`.tmp-*`, `.fetch-*`, `.broken-*`) are the bootstrap's own scratch space.
pub fn installed(ah: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(ah.join("bin")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        !entry.file_name().to_string_lossy().starts_with('.')
            && std::fs::metadata(entry.path().join("anthology")).is_ok_and(|m| m.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_version_folder_with_the_binary_counts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!installed(dir.path()), "no bin folder at all");
        fs::create_dir_all(dir.path().join("bin/0.2.5")).unwrap();
        assert!(!installed(dir.path()), "an empty version folder");
        fs::write(dir.path().join("bin/0.2.5/anthology"), "x").unwrap();
        assert!(installed(dir.path()));
    }

    #[test]
    fn scratch_folders_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bin/.tmp-abc")).unwrap();
        fs::write(dir.path().join("bin/.tmp-abc/anthology"), "x").unwrap();
        fs::create_dir_all(dir.path().join("bin/.broken-0.2.5-1-2")).unwrap();
        fs::write(dir.path().join("bin/.broken-0.2.5-1-2/anthology"), "x").unwrap();
        assert!(!installed(dir.path()));
    }
}
