//! The content rules, one module per group. Each `check` adds findings for one file; `autorun`
//! and `agent` also return the auto-run surfaces the file holds. The stubs find nothing.

pub(crate) mod agent;
pub(crate) mod autorun;
pub(crate) mod deps;
pub(crate) mod exfil;
pub(crate) mod hidden;
pub(crate) mod payload;
pub(crate) mod scanner;
