//! Anthology sends no alert of its own: no sound, no desktop notification, no terminal bell, no
//! toast. A notification app of the user's choice works alongside it; the `/anthology-status` pane
//! stays.
//!
//! The behaviour of the mod is pinned in `plugins/anthology/tests/mods/alerts.test.ts`, and the
//! absence of a `Notification` hook in `cutover_hooks.rs`, `shims/statics.rs` and
//! `anthology-runtime/tests/atl_install.rs`. This file holds what only the file tree can show.

use std::path::Path;

use repo_tests::repo;

#[test]
fn the_notify_hook_script_is_gone() {
    assert!(
        !Path::new(&repo::path("plugins/anthology/hooks/notify.sh")).exists(),
        "hooks/notify.sh played a sound and rang the bell: it is removed"
    );
}

#[test]
fn no_mod_shows_a_toast_plays_a_sound_or_hooks_the_notification_event() {
    let dir = repo::path("plugins/anthology/hooks/mods");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("the mods folder") {
        let path = entry.expect("entry").path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ts" | "tsx")
        ) {
            continue;
        }
        seen += 1;
        let text = std::fs::read_to_string(&path).expect("a mod file");
        for line in text.lines() {
            let line = line.trim_start();
            if line.starts_with("//") || line.starts_with('*') || line.starts_with("/*") {
                continue;
            }
            for banned in ["ui.toast", "audio.play", "classic.Notification"] {
                assert!(
                    !line.contains(banned),
                    "{}: `{banned}`: Anthology sends no alerts: {line}",
                    path.display()
                );
            }
        }
    }
    assert!(seen >= 5, "mod files found: {seen}");
}

#[test]
fn the_shell_hooks_ring_no_bell_and_run_no_notifier() {
    let dir = repo::path("plugins/anthology/hooks");
    for entry in std::fs::read_dir(&dir).expect("the hooks folder") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("sh") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a hook script");
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            for banned in [
                "afplay",
                "notify-send",
                "osascript",
                "canberra-gtk-play",
                "paplay",
            ] {
                assert!(
                    !line.contains(banned),
                    "{}: `{banned}`: Anthology sends no alerts: {line}",
                    path.display()
                );
            }
            assert!(
                !line.contains("\\a"),
                "{}: no terminal bell: {line}",
                path.display()
            );
        }
    }
}
