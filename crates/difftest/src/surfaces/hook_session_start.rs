//! The `hook.session-start` difftest surface.

/// The surface name (plan §1.4).
pub const NAME: &str = "hook.session-start";

/// The registered implementation: `None` until the surface is ported.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    None
}
