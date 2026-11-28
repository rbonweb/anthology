//! A TOML detector, not a parser (Task 3): unknown syntax yields no entry, never an error.

/// One `key = value` line with the table header above it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kv {
    pub table: String,
    pub key: String,
    pub value: String,
    pub line: usize,
}

/// The entries of `text`. The stub finds none.
pub fn entries(_text: &str) -> Vec<Kv> {
    Vec::new()
}
