//! Edit replay (Task 6): the document after a Write, Edit or MultiEdit, and its new spans.
//! Until then nothing replays.

use crate::Span;

/// The tool input of a Write, Edit or MultiEdit. A MultiEdit edit is `(old, new, replace_all)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEdit {
    Write {
        content: String,
    },
    Edit {
        old: String,
        new: String,
        replace_all: bool,
    },
    MultiEdit {
        edits: Vec<(String, String, bool)>,
    },
}

impl ToolEdit {
    /// The tool name: "Write", "Edit" or "MultiEdit".
    pub fn tool(&self) -> &'static str {
        match self {
            ToolEdit::Write { .. } => "Write",
            ToolEdit::Edit { .. } => "Edit",
            ToolEdit::MultiEdit { .. } => "MultiEdit",
        }
    }
}

/// The document after `edit` on `before` (None: the file is missing) and its new spans, or None
/// when the edit would not apply.
pub fn apply(_before: Option<&str>, _edit: &ToolEdit) -> Option<(String, Vec<Span>)> {
    None
}

/// The spans of the lines of `after` that are not lines of `before`.
pub fn added_lines(_before: &str, _after: &str) -> Vec<Span> {
    Vec::new()
}

/// The spans of 1-based inclusive line ranges of `doc`.
pub fn line_spans(_doc: &str, _ranges: &[(usize, usize)]) -> Vec<Span> {
    Vec::new()
}
