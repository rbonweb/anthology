//! Every difftest surface (plan §1.4). Each surface lives in its own module; its lane fills it.

pub mod core_lock;
pub mod core_state;
pub mod dev_eval_baseline;
pub mod dev_memory_diff;
pub mod dev_release_edit;
pub mod dev_rtk_trim;
pub mod dev_smoke;
pub mod dev_startup_budget;
pub mod dev_upstream_watch;
pub mod dev_vendor;
pub mod dev_version_bump;
pub mod doctor;
pub mod guard_check;
pub mod guard_hook;
pub mod hook_session_start;
pub mod mem;
pub mod memory_portable;
pub mod memory_sync;
pub mod notes_parse;
pub mod privacy_findings;
pub mod pycompat_argparse;
pub mod pycompat_difflib;
pub mod pycompat_fnmatch;
pub mod pycompat_ipaddress;
pub mod pycompat_json;
pub mod pycompat_path;
pub mod pycompat_pyerr;
pub mod pycompat_quote;
pub mod pycompat_re;
pub mod pycompat_shlex;
pub mod pycompat_str;
pub mod rtk_install_reconcile;
pub mod runtime_guardrails_loc;
pub mod settings;
pub mod setup;
pub mod shell_analyze;
pub mod shell_segments;

/// Every surface name, in plan §1.4 order.
pub const ALL: &[&str] = &[
    pycompat_json::NAME,
    pycompat_str::NAME,
    pycompat_re::NAME,
    pycompat_difflib::NAME,
    pycompat_fnmatch::NAME,
    pycompat_quote::NAME,
    pycompat_path::NAME,
    pycompat_pyerr::NAME,
    pycompat_ipaddress::NAME,
    pycompat_shlex::NAME,
    pycompat_argparse::NAME,
    core_state::NAME,
    core_lock::NAME,
    shell_segments::NAME,
    shell_analyze::NAME,
    guard_check::NAME,
    guard_hook::NAME,
    settings::NAME,
    privacy_findings::NAME,
    notes_parse::NAME,
    memory_sync::NAME,
    mem::NAME,
    memory_portable::NAME,
    rtk_install_reconcile::NAME,
    doctor::NAME,
    hook_session_start::NAME,
    setup::NAME,
    dev_vendor::NAME,
    dev_version_bump::NAME,
    dev_release_edit::NAME,
    dev_memory_diff::NAME,
    dev_startup_budget::NAME,
    dev_eval_baseline::NAME,
    dev_upstream_watch::NAME,
    dev_rtk_trim::NAME,
    dev_smoke::NAME,
    // Added by RT1a (controller-approved registration, after plan §1.4's list).
    runtime_guardrails_loc::NAME,
];

/// A surface module's `surface()`.
type MakeSurface = fn() -> Option<Box<dyn crate::case::Surface>>;

/// Each surface's `surface()`, in [`ALL`] order (`None` until its lane ports it).
const IMPLS: &[MakeSurface] = &[
    pycompat_json::surface,
    pycompat_str::surface,
    pycompat_re::surface,
    pycompat_difflib::surface,
    pycompat_fnmatch::surface,
    pycompat_quote::surface,
    pycompat_path::surface,
    pycompat_pyerr::surface,
    pycompat_ipaddress::surface,
    pycompat_shlex::surface,
    pycompat_argparse::surface,
    core_state::surface,
    core_lock::surface,
    shell_segments::surface,
    shell_analyze::surface,
    guard_check::surface,
    guard_hook::surface,
    settings::surface,
    privacy_findings::surface,
    notes_parse::surface,
    memory_sync::surface,
    mem::surface,
    memory_portable::surface,
    rtk_install_reconcile::surface,
    doctor::surface,
    hook_session_start::surface,
    setup::surface,
    dev_vendor::surface,
    dev_version_bump::surface,
    dev_release_edit::surface,
    dev_memory_diff::surface,
    dev_startup_budget::surface,
    dev_eval_baseline::surface,
    dev_upstream_watch::surface,
    dev_rtk_trim::surface,
    dev_smoke::surface,
    runtime_guardrails_loc::surface,
];

/// Every surface name with its implementation, in [`ALL`] order.
pub fn registry() -> Vec<(&'static str, Option<Box<dyn crate::case::Surface>>)> {
    ALL.iter()
        .zip(IMPLS)
        .map(|(name, make)| (*name, make()))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_registry_follows_all_and_each_surface_has_its_name() {
        assert_eq!(super::IMPLS.len(), super::ALL.len());
        for (name, surface) in super::registry() {
            if let Some(s) = surface {
                assert_eq!(s.name(), name);
            }
        }
    }
}
