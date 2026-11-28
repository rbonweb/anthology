//! The Markdown view (Task 2). Until then the document is all text.

use super::View;

pub(crate) fn view(doc: &str) -> View {
    View::plain(doc)
}
