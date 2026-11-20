//! The settings.json patcher (plan §3.6, ST1).
//!
//! Every module's `pub` items are re-exported at the crate root, where plan §3.6 publishes them.
//! Keep `pub` names unique across modules and use `pub(crate)` for helpers.
#![forbid(unsafe_code)]

pub mod diff;
pub mod ops;
pub mod plan;
pub mod reconcile;
pub mod render;
pub mod revert;
pub mod scanner;
pub mod validate;
pub mod write;

#[allow(unused_imports)] // until the module has items
pub use diff::*;
#[allow(unused_imports)] // until the module has items
pub use ops::*;
#[allow(unused_imports)] // until the module has items
pub use plan::*;
#[allow(unused_imports)] // until the module has items
pub use reconcile::*;
#[allow(unused_imports)] // until the module has items
pub use render::*;
#[allow(unused_imports)] // until the module has items
pub use revert::*;
#[allow(unused_imports)] // until the module has items
pub use scanner::*;
#[allow(unused_imports)] // until the module has items
pub use validate::*;
#[allow(unused_imports)] // until the module has items
pub use write::*;
