//! The `pycompat.str` difftest surface.

/// The surface name (plan §1.4).
pub const NAME: &str = "pycompat.str";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation: `None` until the surface is ported.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    None
}
