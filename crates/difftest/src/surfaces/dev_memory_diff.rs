//! The `dev.memory-diff` difftest surface.

/// The surface name (plan §1.4).
pub const NAME: &str = "dev.memory-diff";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation (DV2): the shared engine in [`super::dev_dv2`].
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    Some(Box::new(super::dev_dv2::Dv2(
        super::dev_dv2::Tool::MemoryDiff,
    )))
}
