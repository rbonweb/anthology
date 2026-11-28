//! The density rules (Task 4), counted over the new text only. Until then each finds nothing.

use crate::{Hit, Span, View};

pub(crate) fn em_dash(_view: &View, _new: &[Span]) -> Vec<Hit> {
    Vec::new()
}

pub(crate) fn triad(_view: &View, _new: &[Span]) -> Vec<Hit> {
    Vec::new()
}
