//! Shared runtime pieces (plan §3.3). `roster` is F1's; the rest is F3's.
//!
//! `unsafe` is denied everywhere except `sys` (GC-18). `env`'s `pub` items are re-exported at
//! the crate root, where plan §3.3 publishes `Env`.
//!
//! A duplicate `pub` name across these modules fails clippy here (`ambiguous_glob_reexports`,
//! denied by `-D warnings`): rename it in your module, or make it `pub(crate)`.
#![deny(unsafe_code)]

pub mod clock;
pub mod env;
pub mod fsx;
pub mod git;
pub mod lock;
pub mod paths;
pub mod platform;
pub mod repos;
pub mod roster;
pub mod state;
#[allow(unsafe_code)]
pub mod sys;

#[allow(unused_imports)] // until the module has items
pub use env::*;
