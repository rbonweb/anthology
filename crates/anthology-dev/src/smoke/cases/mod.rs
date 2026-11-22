//! The smoke cases, one module each (EV1). Each `run(ctx)` is one `smoke_*` function of smoke.py,
//! or an EV1b smoke (`binary_missing`, `live_*`).

pub mod agent_memory_path;
pub mod atl_rewrite;
pub mod auto_memory_dir;
pub mod binary_missing;
pub mod cli_syntax;
pub mod context_cap;
pub mod eval_ceiling;
pub mod guard_timeout;
pub mod knowledge_read_writers;
pub mod live_guard;
pub mod live_mem;
pub mod live_seed;
pub mod live_session_start;
pub mod live_setup;
pub mod missing_interpreter;
pub mod model_check;
pub mod python_missing;
pub mod worktree_base;
pub mod writer_memory_commit;
