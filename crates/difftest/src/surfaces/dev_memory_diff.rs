//! The `dev.memory-diff` difftest surface.

/// The surface name (plan §1.4).
pub const NAME: &str = "dev.memory-diff";

/// The registered implementation: `None` until the surface is ported.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    None
}
