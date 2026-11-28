//! File kinds, decoding and line numbers (Task 2). The stubs return empty values.

use crate::FileKind;

/// The kind of the file at `path`, from its name and its first bytes `head`.
pub fn classify(_path: &str, _head: &[u8]) -> FileKind {
    FileKind::Data
}

/// `b` as text: UTF-8 (optional BOM) or UTF-16 with a BOM; `None` for anything else.
pub fn decode(_b: &[u8]) -> Option<String> {
    None
}

/// The 1-based line of byte offset `at` in `t`; CRLF counts as one line break.
pub fn line_of(_t: &str, _at: usize) -> usize {
    0
}

/// The name of the executable or archive format `b` starts with, if any.
pub fn magic(_b: &[u8]) -> Option<&'static str> {
    None
}
