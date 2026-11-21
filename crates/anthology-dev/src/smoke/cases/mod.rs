//! The smoke cases, one module each (EV1). Each `run(ctx)` is one `smoke_*` function of smoke.py.

pub mod agent_memory_path;
pub mod auto_memory_dir;
pub mod cli_syntax;
pub mod context_cap;
pub mod eval_ceiling;
pub mod guard_timeout;
pub mod knowledge_read_writers;
pub mod missing_interpreter;
pub mod model_check;
pub mod python_missing;
pub mod rtk_rewrite;
pub mod worktree_base;
pub mod writer_memory_commit;
