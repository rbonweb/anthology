//! JSON with comments (Task 3). The stub refuses everything.

use anthology_pycompat::json::Value;

/// Parses `text` as JSON after stripping `//` and `/* */` comments outside strings and trailing
/// commas.
pub fn parse(_text: &str) -> Result<Value, String> {
    Err("not implemented yet".to_string())
}
