//! Python-semantics parity layer (design §12, plan GC-16).
//!
//! The JSON module is F2's; every other module is P1's (plan §3.1, §3.2).
#![forbid(unsafe_code)]

pub mod argparse;
pub mod difflib;
pub mod errno_tables;
pub mod fnmatch;
pub mod ipaddress;
pub mod json;
pub mod path;
pub mod pyerr;
pub mod quote;
pub mod re;
pub mod shlex;
pub mod str;
pub mod tables;
