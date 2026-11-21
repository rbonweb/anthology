//! G5: browser MCP tools other than BrowserOS Neo (guard_rules.py `_g5`).

use anthology_pycompat::str as pystr;

/// `_BLOCKED_MCP`.
const BLOCKED_MCP: &[&str] = &[
    "playwright",
    "puppeteer",
    "chrome-devtools",
    "claude-in-chrome",
];

/// `_g5(tool_name)`: an `mcp__` tool whose name (lowercased, `_` read as `-`) holds a blocked
/// server's name.
pub fn g5(tool_name: &str) -> bool {
    if !tool_name.starts_with("mcp__") {
        return false;
    }
    let name = pystr::lower(tool_name).replace('_', "-");
    BLOCKED_MCP.iter().any(|word| name.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_mcp_tools_naming_a_blocked_server() {
        assert!(g5("mcp__playwright__browser_navigate"));
        assert!(g5("mcp__Chrome_DevTools__x"));
        assert!(g5("mcp__claude_in_chrome__x"));
        assert!(!g5("mcp__browseros-neo__navigate"));
        assert!(!g5("playwright"));
    }
}
