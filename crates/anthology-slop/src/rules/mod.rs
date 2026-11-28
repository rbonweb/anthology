//! The v1 rules: the 17 rules of the plan's rule table, in that order.

// The colour helpers are frozen in Task 1 and used by the UI rules of Task 5.
#[allow(dead_code)]
mod color;
mod density;
mod text;
mod ui;

use crate::Kind::{Css, Html, Jsx, Markdown};
use crate::Severity::{Block, Density};
use crate::{Hit, Kind, Rule, Span, View};

/// How a rule runs: a text rule reads the view, a density rule also gets the new spans.
pub(crate) enum Run {
    Text(fn(&View) -> Vec<Hit>),
    Density(fn(&View, &[Span]) -> Vec<Hit>),
}

const PROSE: &[Kind] = &[Markdown, Html, Jsx];
const PAGES: &[Kind] = &[Markdown, Html];
const STYLED: &[Kind] = &[Css, Html, Jsx];

/// The 17 rules of "The v1 rules", in that order.
pub static RULES: &[Rule] = &[
    Rule {
        id: "text.ai-vocab",
        severity: Block,
        kinds: PROSE,
        summary: "stock AI vocabulary",
        hint: "say the concrete thing the word stands for: the step, the number, the part",
        run: Run::Text(text::ai_vocab),
    },
    Rule {
        id: "text.stock-phrase",
        severity: Block,
        kinds: PROSE,
        summary: "stock opener or cliche",
        hint: "cut the lead-in and start with the point",
        run: Run::Text(text::stock_phrase),
    },
    Rule {
        id: "text.chat-residue",
        severity: Block,
        kinds: PROSE,
        summary: "chat-assistant wording in a file",
        hint: "delete it: chat wording does not belong in a file",
        run: Run::Text(text::chat_residue),
    },
    Rule {
        id: "text.not-x-but-y",
        severity: Block,
        kinds: PROSE,
        summary: "denied-alternative contrast",
        hint: "state the positive claim once, without the denied alternative",
        run: Run::Text(text::not_x_but_y),
    },
    Rule {
        id: "text.hype-cta",
        severity: Block,
        kinds: PROSE,
        summary: "hype call to action",
        hint: "say what happens when the reader acts, with one fact",
        run: Run::Text(text::hype_cta),
    },
    Rule {
        id: "text.em-dash-density",
        severity: Density,
        kinds: PAGES,
        summary: "em dashes: 3 or more, over 1 per 80 words",
        hint: "use commas, colons, parentheses or full stops for most of these dashes",
        run: Run::Density(density::em_dash),
    },
    Rule {
        id: "text.triad-density",
        severity: Density,
        kinds: PAGES,
        summary: "lists of three: 3 or more, over 1 per 150 words",
        hint: "keep the lists the content needs; vary or cut the rest",
        run: Run::Density(density::triad),
    },
    Rule {
        id: "ui.purple-gradient",
        severity: Block,
        kinds: STYLED,
        summary: "purple or indigo gradient (C1)",
        hint: "pick a gradient from the brand palette, or use a flat colour",
        run: Run::Text(ui::purple_gradient),
    },
    Rule {
        id: "ui.gradient-text",
        severity: Block,
        kinds: STYLED,
        summary: "gradient-filled text (C6)",
        hint: "use a solid text colour; put emphasis in size or weight",
        run: Run::Text(ui::gradient_text),
    },
    Rule {
        id: "ui.shadcn-default",
        severity: Block,
        kinds: &[Css, Html],
        summary: "untouched shadcn/ui base theme (K1)",
        hint: "set the theme tokens to the product's own palette",
        run: Run::Text(ui::shadcn_default),
    },
    Rule {
        id: "ui.glow-orb",
        severity: Block,
        kinds: STYLED,
        summary: "decorative blurred orb (C11)",
        hint: "remove the orb, or replace it with an image or shape that carries meaning",
        run: Run::Text(ui::glow_orb),
    },
    Rule {
        id: "ui.effect-library",
        severity: Block,
        kinds: &[Jsx],
        summary: "stock effect-library component (K10)",
        hint: "build the effect the design needs, or drop it",
        run: Run::Text(ui::effect_library),
    },
    Rule {
        id: "ui.placeholder-media",
        severity: Block,
        kinds: STYLED,
        summary: "placeholder avatar or image service (IM4)",
        hint: "use the product's real media or a local asset",
        run: Run::Text(ui::placeholder_media),
    },
    Rule {
        id: "ui.placeholder-text",
        severity: Block,
        kinds: &[Html, Jsx],
        summary: "placeholder copy",
        hint: "write the real copy for this screen",
        run: Run::Text(ui::placeholder_text),
    },
    Rule {
        id: "ui.dummy-contact",
        severity: Block,
        kinds: &[Html, Jsx],
        summary: "dummy phone number or chat link",
        hint: "use the user's real contact, or leave the channel out",
        run: Run::Text(ui::dummy_contact),
    },
    Rule {
        id: "ui.five-star-row",
        severity: Block,
        kinds: &[Html, Jsx],
        summary: "five-star row (IM5)",
        hint: "show a real rating with its source, or no stars",
        run: Run::Text(ui::five_star_row),
    },
    Rule {
        id: "ui.generator-signature",
        severity: Block,
        kinds: &[Html, Jsx],
        summary: "generator signature left in (F1)",
        hint: "remove the generator tag or package",
        run: Run::Text(ui::generator_signature),
    },
];
