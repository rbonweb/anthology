//! Test helpers (dev only; plan §3.7). The core modules are F4's; each lane keeps its own
//! helpers in its own module (plan §0.2), named after the lane, so no lane edits this file.
//!
//! `sandbox`, `run`, `tree` and `rows` are re-exported at the crate root, where plan §3.7
//! publishes them.
#![forbid(unsafe_code)]

pub mod http;
pub mod mem;
pub mod release;
pub mod rows;
pub mod run;
pub mod sandbox;
pub mod session;
pub mod tree;

// Per-lane helper modules (plan §0.2).
pub mod bs;
pub mod co;
pub mod dv_a;
pub mod dv_b;
pub mod dv_v;
pub mod dv_w;
pub mod ev;
pub mod gd;
pub mod mi;
pub mod mp;
pub mod ms;
pub mod mt;
pub mod mx;
pub mod p;
pub mod rl;
pub mod rp;
pub mod rt_a;
pub mod rt_b;
pub mod rt_c;
pub mod sh;
pub mod ss;
pub mod st;
pub mod su;

#[allow(unused_imports)] // until the module has items
pub use rows::*;
#[allow(unused_imports)] // until the module has items
pub use run::*;
#[allow(unused_imports)] // until the module has items
pub use sandbox::*;
#[allow(unused_imports)] // until the module has items
pub use tree::*;
