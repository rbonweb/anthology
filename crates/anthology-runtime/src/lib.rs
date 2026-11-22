//! Anthology runtime (plan §3.6). Items are published per module; every entry point takes
//! `&Env` and a `Deps` (GC-18, GC-19).
#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod compat;
pub mod doctor;
pub mod fetch;
pub mod guardrails_loc;
pub mod rtk_install;
pub mod session_start;
pub mod setup;
