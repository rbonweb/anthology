//! Helpers shared by the `anthology-dev` integration tests (`mod support;`).
//!
//! This list is F4's. So that lanes never edit one shared file (plan §0.2), a lane puts its own
//! helper in `tests/support/<name>.rs` and includes it from its own test file with
//! `#[path = "support/<name>.rs"] mod <name>;`; `agents` can be included the same way.
#![allow(dead_code, reason = "each test binary uses only some helpers")]

pub mod agents;
