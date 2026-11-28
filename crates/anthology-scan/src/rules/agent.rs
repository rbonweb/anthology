//! Agent configuration: Claude Code settings, MCP servers, skills, instruction files (Task 10).

use crate::{FileCtx, Finding, Surface};

pub(crate) fn surfaces(_f: &FileCtx, _out: &mut Vec<Finding>) -> Vec<Surface> {
    Vec::new()
}

/// Whether `path` is an agent instruction file or lives in an instruction folder.
pub(crate) fn is_instruction(_path: &str) -> bool {
    false
}
