//! The JSX and TSX view (Task 3). Until then the document is all text.

use super::View;

pub(crate) fn view(doc: &str) -> View {
    View::plain(doc)
}
