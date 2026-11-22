//! The structural shell parser shared by the anthology guard and rtk (design §7, plan §3.4).
//!
//! Every module's `pub` items are re-exported at the crate root, where plan §3.4 publishes them,
//! so a lane fills its module without editing this file. Keep `pub` names unique across modules
//! and use `pub(crate)` for helpers.
//!
//! A duplicate `pub` name across these modules fails clippy here (`ambiguous_glob_reexports`,
//! denied by `-D warnings`): rename it in your module, or make it `pub(crate)`.
#![forbid(unsafe_code)]

pub mod analyze;
pub mod collect;
pub mod heredoc;
pub mod lenient;
pub mod lexer;
pub mod limits;
pub mod opaque;
pub mod prefixes;
pub mod readonly;
pub mod rtk_forms;
pub mod shell_args;

#[allow(unused_imports)] // until the module has items
pub use analyze::*;
#[allow(unused_imports)] // until the module has items
pub use collect::*;
#[allow(unused_imports)] // until the module has items
pub use heredoc::*;
#[allow(unused_imports)] // until the module has items
pub use lenient::*;
#[allow(unused_imports)] // until the module has items
pub use lexer::*;
#[allow(unused_imports)] // until the module has items
pub use limits::*;
#[allow(unused_imports)] // until the module has items
pub use opaque::*;
#[allow(unused_imports)] // until the module has items
pub use prefixes::*;
#[allow(unused_imports)] // until the module has items
pub use readonly::*;
#[allow(unused_imports)] // until the module has items
pub use rtk_forms::*;
#[allow(unused_imports)] // until the module has items
pub use shell_args::*;
