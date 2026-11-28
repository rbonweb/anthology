//! Masking: a `View` of a document per kind. Every string of a view has the document's length,
//! and masked bytes are `MASK` (newlines kept), so an offset means the same byte everywhere.

// The shared helpers are filled in Task 1 and called by the kind views of Tasks 2 and 3.
#[allow(dead_code)]
pub(crate) mod common;
mod css;
mod html;
mod jsx;
mod markdown;

use crate::{Kind, Span};

/// The byte a masked byte becomes.
pub const MASK: u8 = 0x01;

/// What the rules read.
/// - `text`: the doc with every non-text byte set to MASK (newlines kept);
/// - `code`: the doc with comments and ignore regions set to MASK;
/// - `style`: the CSS regions of `code`;
/// - `classes`: the class/className values in `code`;
/// - `dash_skip`: ranges whose dashes the density rule skips;
/// - `waivers`: the waiver comments and ignore-region starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub text: String,
    pub code: String,
    pub style: Vec<Span>,
    pub classes: Vec<Span>,
    pub dash_skip: Vec<Span>,
    pub waivers: Vec<Waiver>,
}

impl View {
    /// `text` = `code` = `doc`, `style` = the whole doc, nothing else.
    pub fn plain(doc: &str) -> View {
        View {
            text: doc.to_string(),
            code: doc.to_string(),
            style: vec![Span {
                start: 0,
                end: doc.len(),
            }],
            classes: Vec::new(),
            dash_skip: Vec::new(),
            waivers: Vec::new(),
        }
    }
}

/// A `slop-ok` comment (`rule` Some) or an ignore-region start (`rule` None).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiver {
    pub line: usize,
    pub rule: Option<&'static str>,
    pub reason: String,
    pub valid: bool,
}

pub(crate) fn view(kind: Kind, doc: &str) -> View {
    match kind {
        Kind::Markdown => markdown::view(doc),
        Kind::Html => html::view(doc),
        Kind::Jsx => jsx::view(doc),
        Kind::Css => css::view(doc),
    }
}
