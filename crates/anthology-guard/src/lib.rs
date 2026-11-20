//! The anthology-guardrails rules (design §9.5, plan §3.5, GC-6).
//!
//! `data` and `secrets` are F1's; the rest is GD's. The `pub` items of `check`, `hook`, `paths`
//! and `rules` are re-exported at the crate root, where plan §3.5 publishes them. Keep `pub`
//! names unique across those modules and use `pub(crate)` for helpers.
//!
//! A duplicate `pub` name across these modules fails clippy here (`ambiguous_glob_reexports`,
//! denied by `-D warnings`): rename it in your module, or make it `pub(crate)`.
#![forbid(unsafe_code)]

pub mod check;
pub mod data;
pub mod hook;
pub mod paths;
pub mod rules;
pub mod secrets;

#[allow(unused_imports)] // until the module has items
pub use check::*;
#[allow(unused_imports)] // until the module has items
pub use hook::*;
#[allow(unused_imports)] // until the module has items
pub use paths::*;
#[allow(unused_imports)] // until the module has items
pub use rules::*;
