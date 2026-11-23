pub const ATL_DATA_DIR: &str = "atl";
pub const HISTORY_DB: &str = "history.db";
pub const RECALL_DB: &str = "recall.db";
pub const CONFIG_TOML: &str = "config.toml";
pub const FILTERS_TOML: &str = "filters.toml";
pub const PIPELINE_TOML: &str = "pipeline.toml";
pub const TRUSTED_FILTERS_JSON: &str = "trusted_filters.json";
pub const DEFAULT_HISTORY_DAYS: i64 = 90;

/// ATL-only subcommands that should never fall back to raw execution.
/// When adding a new ATL-only subcommand to `Commands`, add its clap name here.
pub const ATL_META_COMMANDS: &[&str] = &[
    "gain",
    "discover",
    "learn",
    "init",
    "config",
    "bypass",
    "proxy",
    "recall",
    "run",
    "hook",
    "hook-audit",
    "pipe",
    "cc-economics",
    "verify",
    "trust",
    "untrust",
    "session",
    "rewrite",
    "telemetry",
    "smart",
    "deps",
    "json",
    "llm-proxy",
    "pipeline",
    "methods",
    "tasks",
    "plugins",
    // `test` also passes through to the POSIX binary, but a flag clap rejects on
    // it is ATL's own: `atl test --shell` reports the error instead of running `test`.
    "test",
    "format",
    // `err` and `summary` name no real binary, and since they grew `--shell`
    // they have a flag to get wrong. Falling through would try to exec a
    // program called `err` instead of reporting the flag error (#4125 review).
    "err",
    "summary",
];
