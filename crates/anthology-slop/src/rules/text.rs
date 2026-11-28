//! The text rules: stock vocabulary, stock phrases, chat residue, the denied-alternative contrast
//! and hype calls to action.
//!
//! Every rule matches on `view.text`, where masked bytes are `MASK` and so never join a phrase.
//! A hit's span is the match; its excerpt is read from `view.code`, which keeps the prose bytes.
//! Every pattern is case-insensitive, `['’]` stands for either apostrophe, and `\s+` sits between
//! words so a soft line wrap still matches.

use std::sync::LazyLock;

use regex::{Match, Regex};

use crate::{Hit, Span, View, excerpt};

const AI_VOCAB: &str = "text.ai-vocab";
const STOCK_PHRASE: &str = "text.stock-phrase";
const CHAT_RESIDUE: &str = "text.chat-residue";
const NOT_X_BUT_Y: &str = "text.not-x-but-y";
const HYPE_CTA: &str = "text.hype-cta";

/// How many bytes after a stock word the literal exceptions read, and how many before.
const AFTER: usize = 40;
const BEFORE: usize = 30;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("text rule pattern")
}

/// `(?i)\b(?:a|b|...)\b` over the alternatives.
fn bounded(alternatives: &[&str]) -> Regex {
    re(&format!(r"(?i)\b(?:{})\b", alternatives.join("|")))
}

fn span_of(m: &Match<'_>) -> Span {
    Span {
        start: m.start(),
        end: m.end(),
    }
}

fn hit(rule: &'static str, view: &View, span: Span) -> Hit {
    Hit {
        rule,
        span,
        parts: Vec::new(),
        excerpt: excerpt(&view.code, span),
    }
}

/// One hit per match of `pattern` in the text.
fn matches(rule: &'static str, view: &View, pattern: &Regex) -> Vec<Hit> {
    pattern
        .find_iter(&view.text)
        .map(|m| hit(rule, view, span_of(&m)))
        .collect()
}

/// Up to `n` bytes of `text` after byte `at`, cut on a character boundary.
fn after(text: &str, at: usize, n: usize) -> &str {
    let mut end = at.saturating_add(n).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[at..end]
}

/// Up to `n` bytes of `text` before byte `at`, cut on a character boundary.
fn before(text: &str, at: usize, n: usize) -> &str {
    let mut start = at.saturating_sub(n);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..at]
}

static AI_VOCAB_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    bounded(&[
        "delv(?:e|es|ed|ing)",
        "tapestr(?:y|ies)",
        "seamless(?:ly)?",
        "elevat(?:e|es|ing)",
        "unleash(?:es|ed|ing)?",
        "supercharg(?:e|es|ing)",
        "turbocharg(?:e|es|ing)",
        "game-chang(?:er|ers|ing)",
        "groundbreaking",
        "unparalleled",
        "synerg(?:y|ies|istic)",
        "holistic(?:ally)?",
        "meticulous(?:ly)?",
        "plethora",
        "multifaceted",
        "ever-evolving",
    ])
});

/// The nouns that make `seamless` a literal word (a seamless pipe, a seamless texture).
static SEAMLESS_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^\s+(?:pipes?|tubes?|tubing|steel|knit|knitted|knitting|garments?|textures?|tiles?|tiling|tileable|patterns?|loops?|gutters?|flooring|stockings|socks|underwear)\b",
    )
});

/// What `elevate` takes when it is a computing term (elevate privileges).
static ELEVATE_OBJECT: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^\s+(?:the\s+|your\s+|their\s+|its\s+|a\s+)?(?:privileges?|permissions?|rights|access|process|shell|user|account|token|session|prompt)\b",
    )
});

static TAPESTRY_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^\s+(?:weaving|weavers?|looms?|museum|kits?|wool|yarn|workshops?)\b")
});

static TAPESTRY_BEFORE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:woven|medieval|antique|hand-woven|wool|silk)\s+$"));

static GROUNDBREAKING_AFTER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s+(?:ceremony|ceremonies)\b"));

/// True when the stock word is used for what it literally means.
fn is_literal_use(text: &str, m: &Match<'_>) -> bool {
    let word = m.as_str().to_ascii_lowercase();
    let next = after(text, m.end(), AFTER);
    if word.starts_with("seamless") {
        SEAMLESS_BEFORE.is_match(next)
    } else if word.starts_with("elevat") {
        ELEVATE_OBJECT.is_match(next)
    } else if word.starts_with("tapestr") {
        TAPESTRY_AFTER.is_match(next) || TAPESTRY_BEFORE.is_match(before(text, m.start(), BEFORE))
    } else if word.starts_with("groundbreaking") {
        GROUNDBREAKING_AFTER.is_match(next)
    } else {
        false
    }
}

pub(crate) fn ai_vocab(view: &View) -> Vec<Hit> {
    AI_VOCAB_WORDS
        .find_iter(&view.text)
        .filter(|m| !is_literal_use(&view.text, m))
        .map(|m| hit(AI_VOCAB, view, span_of(&m)))
        .collect()
}

static STOCK_PHRASES: LazyLock<Regex> = LazyLock::new(|| {
    bounded(&[
        r"in\s+today['’]s\s+(?:fast-paced|digital\s+age|digital\s+world|rapidly\s+evolving|ever-changing)",
        r"in\s+(?:an|the)\s+ever-evolving",
        r"embark(?:s|ed|ing)?\s+on\s+(?:a|an|the|this|our|your)\s+(?:journey|quest|mission|adventure)",
        r"let['’]s\s+dive\s+(?:in|into|deeper)",
        r"without\s+further\s+ado",
        r"gone\s+are\s+the\s+days",
        r"imagine\s+a\s+world\s+where",
        r"look\s+no\s+further",
        r"in\s+the\s+realm\s+of",
        r"(?:a|stands\s+as\s+a)\s+testament\s+to",
        r"navigat(?:e|ing)\s+the\s+complexities",
        r"unlock(?:s|ing)?\s+the\s+(?:full\s+)?(?:power|potential)",
        r"harness(?:es|ing)?\s+the\s+power\s+of",
        r"it['’]s\s+(?:important|worth)\s+(?:to\s+note|noting)\s+that",
    ])
});

pub(crate) fn stock_phrase(view: &View) -> Vec<Hit> {
    matches(STOCK_PHRASE, view, &STOCK_PHRASES)
}

/// The chat wordings, then a sentence-opening `Certainly!`, `Absolutely!` or `Sure!` (named group
/// `c`, so the hit sits on the word and not on the full stop before it).
static CHAT_PHRASES: LazyLock<Regex> = LazyLock::new(|| {
    let words = [
        r"(?:i\s+)?hope\s+(?:this|that)\s+helps",
        r"hopefully\s+(?:this|that)\s+helps",
        r"i\s+hope\s+(?:this|that)\s+(?:was|is)\s+helpful",
        r"i\s+hope\s+you\s+(?:find|found)\s+this\s+(?:helpful|useful)",
        r"let\s+me\s+know\s+if\s+(?:you\s+have\s+any|you\s+need\s+any|you\s+need\s+anything|there['’]s\s+anything)",
        r"i['’]d\s+be\s+(?:happy|glad)\s+to\s+help",
        r"i['’]m\s+(?:happy|glad|here)\s+to\s+help",
        r"great\s+question",
        r"you['’]re\s+absolutely\s+right",
        r"as\s+an\s+ai(?:\s+language\s+model|\s+assistant)?,?\s+i",
        r"here['’]s\s+(?:a|the)\s+(?:revised|rewritten|updated|polished|improved)\s+(?:version|draft)",
    ];
    re(&format!(
        r"(?i)\b(?:{})\b|(?m:^|[.!?]\s+)(?P<c>(?:certainly|absolutely|sure)!)",
        words.join("|")
    ))
});

pub(crate) fn chat_residue(view: &View) -> Vec<Hit> {
    CHAT_PHRASES
        .captures_iter(&view.text)
        .filter_map(|caps| caps.name("c").or_else(|| caps.get(0)))
        .map(|m| hit(CHAT_RESIDUE, view, span_of(&m)))
        .collect()
}

/// The five forms of "it is not X, it is Y".
static CONTRASTS: LazyLock<[Regex; 5]> = LazyLock::new(|| {
    [
        r"(?i)\b(?:it|this|that)(?:['’]s|\s+is|\s+was)\s+not\s+(?:just\s+|merely\s+|simply\s+|only\s+|really\s+)?(?:about|a|an|the|your|our)\s+[^;,—–:.!?\n]{1,80}[;,—–:]\s*(?:it|this|that)(?:['’]s|\s+is)\b",
        r"(?i)\b(?:it|this|that)\s+(?:isn['’]t|wasn['’]t)\s+(?:just\s+|merely\s+|simply\s+|only\s+|really\s+)?(?:about|a|an|the|your|our)\s+[^;,—–:.!?\n]{1,80}[;,—–:]\s*(?:it|this|that)(?:['’]s|\s+is)\b",
        r"(?i)\b(?:isn['’]t|is\s+not|aren['’]t|are\s+not)\s+(?:just|merely|simply|only)\s+[^.!?\n]{1,60}[.;,—–:]\s+(?:it|they)(?:['’]s|['’]re|\s+is|\s+are)\b",
        r"(?i)\b(?:it|this)(?:['’]s\s+not|\s+is\s+not|\s+isn['’]t)\s+(?:just\s+|really\s+)?about\s+[^.!?\n]{1,80}\.\s+it(?:['’]s|\s+is)\s+about\b",
        r"(?i)\b(?:it['’]s|this\s+is)\s+(?:more\s+than|less\s+about)\s+[^;,—–:.!?\n]{1,60}(?:[;,—–:]\s*it['’]s|\s+and\s+more\s+about)\b",
    ]
    .map(re)
});

pub(crate) fn not_x_but_y(view: &View) -> Vec<Hit> {
    let mut spans: Vec<Span> = CONTRASTS
        .iter()
        .flat_map(|pattern| pattern.find_iter(&view.text).map(|m| span_of(&m)))
        .collect();
    spans.sort();
    // Two forms can match the same sentence: keep the earlier match.
    let mut kept: Vec<Span> = Vec::new();
    for span in spans {
        if kept.last().is_none_or(|last| !last.intersects(span)) {
            kept.push(span);
        }
    }
    kept.into_iter()
        .map(|span| hit(NOT_X_BUT_Y, view, span))
        .collect()
}

static HYPE_PHRASES: LazyLock<Regex> = LazyLock::new(|| {
    bounded(&[
        r"ready\s+to\s+(?:transform|supercharge|elevate|level\s+up|take\s+your)",
        r"to\s+the\s+next\s+level",
        r"join\s+(?:thousands|millions|hundreds)\s+of",
        r"transform\s+(?:the\s+way\s+you|your\s+[a-z]+\s+(?:experience|workflow|business))",
        r"revolutioni[sz]e\s+(?:the\s+way|your|how)",
        r"the\s+future\s+of\s+[a-z]+\s+is\s+(?:here|now)",
        r"say\s+goodbye\s+to",
        r"at\s+your\s+fingertips",
        r"in\s+just\s+a\s+few\s+clicks",
        r"we['’]ve\s+got\s+you\s+covered",
        r"the\s+possibilities\s+are\s+endless",
        r"(?:start|begin)\s+your\s+journey",
        r"don['’]t\s+miss\s+out",
    ])
});

pub(crate) fn hype_cta(view: &View) -> Vec<Hit> {
    matches(HYPE_CTA, view, &HYPE_PHRASES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MASK, Span};

    type Rule = fn(&View) -> Vec<Hit>;

    /// The excerpts a rule finds in `doc`, read through `View::plain`.
    fn found(rule: Rule, doc: &str) -> Vec<String> {
        rule(&View::plain(doc))
            .into_iter()
            .map(|h| h.excerpt)
            .collect()
    }

    fn assert_each_flagged(rule: Rule, cases: &[(&str, &str)]) {
        let mut wrong = Vec::new();
        for (doc, want) in cases {
            let got = found(rule, doc);
            if got != [*want] {
                wrong.push(format!("{doc:?}: wanted [{want:?}], got {got:?}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    fn assert_none_flagged(rule: Rule, docs: &[&str]) {
        let mut wrong = Vec::new();
        for doc in docs {
            let got = found(rule, doc);
            if !got.is_empty() {
                wrong.push(format!("{doc:?}: {got:?}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn ai_vocab_flags_every_word_of_the_list() {
        assert_each_flagged(
            ai_vocab,
            &[
                ("We delve into caching.", "delve"),
                ("She delves into it.", "delves"),
                ("They delved deep.", "delved"),
                ("Delving further helps.", "Delving"),
                ("A rich tapestry of options.", "tapestry"),
                ("The tapestries of the web.", "tapestries"),
                ("A seamless login.", "seamless"),
                ("It works seamlessly.", "seamlessly"),
                ("Elevate your morning routine.", "Elevate"),
                ("This elevates the team.", "elevates"),
                ("Elevating results daily.", "Elevating"),
                ("Unleash the cache.", "Unleash"),
                ("It unleashes speed.", "unleashes"),
                ("It unleashed speed.", "unleashed"),
                ("Unleashing speed.", "Unleashing"),
                ("Supercharge your builds.", "Supercharge"),
                ("It supercharges builds.", "supercharges"),
                ("Supercharging builds.", "Supercharging"),
                ("Turbocharge your builds.", "Turbocharge"),
                ("It turbocharges builds.", "turbocharges"),
                ("Turbocharging builds.", "Turbocharging"),
                ("A game-changer for teams.", "game-changer"),
                ("Two game-changers arrived.", "game-changers"),
                ("A game-changing idea.", "game-changing"),
                ("Groundbreaking research.", "Groundbreaking"),
                ("Unparalleled speed.", "Unparalleled"),
                ("A real synergy.", "synergy"),
                ("Many synergies.", "synergies"),
                ("A synergistic effect.", "synergistic"),
                ("A holistic view.", "holistic"),
                ("Viewed holistically.", "holistically"),
                ("A meticulous review.", "meticulous"),
                ("Reviewed meticulously.", "meticulously"),
                ("A plethora of options.", "plethora"),
                ("A multifaceted problem.", "multifaceted"),
                ("An ever-evolving field.", "ever-evolving"),
            ],
        );
    }

    #[test]
    fn ai_vocab_is_case_insensitive_and_word_bounded() {
        assert_each_flagged(ai_vocab, &[("DELVE into it.", "DELVE")]);
        assert_none_flagged(
            ai_vocab,
            &[
                "The delver left.",
                "Seamlessness is rare.",
                "The elevated deck.",
                "The tapestryx file.",
                "Undelve.",
            ],
        );
    }

    #[test]
    fn ai_vocab_finds_each_occurrence() {
        assert_eq!(
            found(ai_vocab, "We delve, then delve again."),
            ["delve", "delve"]
        );
    }

    #[test]
    fn literal_uses_are_not_flagged() {
        // the literal exceptions of the plan, each by its own list
        assert_none_flagged(
            ai_vocab,
            &[
                "To elevate privileges, the installer calls sudo once.",
                "The mill rolls seamless steel pipe in three diameters.",
                "Use a seamless texture so the floor tiles repeat without a visible edge.",
                "The museum restored a medieval tapestry from the abbey.",
                "The groundbreaking ceremony starts at nine.",
                "Seamless pipes.",
                "Seamless tubes and seamless tubing.",
                "Seamless knit socks.",
                "Seamless knitted garments.",
                "Seamless knitting.",
                "A seamless garment.",
                "Seamless tiles, tiling and tileable patterns.",
                "Seamless pattern loops.",
                "Seamless gutters and seamless flooring.",
                "Seamless stockings, seamless socks, seamless underwear.",
                "It joins seamlessly steel pipes.",
                "Elevate the privileges.",
                "Elevate your access.",
                "Elevate their permissions.",
                "Elevate its rights.",
                "Elevate a shell.",
                "Elevate the process.",
                "Elevating the user.",
                "Elevates the account.",
                "Elevate the token.",
                "Elevate the session.",
                "Elevate the prompt.",
                "Elevate access to root.",
                "Tapestry weaving classes run on Friday.",
                "Tapestry weavers meet monthly.",
                "Tapestry looms for sale.",
                "The tapestry museum opens at ten.",
                "A tapestry kit with a needle.",
                "Tapestry wool by the skein.",
                "Tapestry yarn in six colours.",
                "Tapestry workshops are full.",
                "A woven tapestry hangs here.",
                "An antique tapestry hangs here.",
                "A hand-woven tapestry hangs here.",
                "A wool tapestry hangs here.",
                "A silk tapestry hangs here.",
                "The tapestries museum.",
                "Groundbreaking ceremonies run long.",
                "A medieval\ntapestry.",
                "Elevate\nthe privileges.",
            ],
        );
        // the neighbours that are not on a list stay flagged
        assert_each_flagged(
            ai_vocab,
            &[
                ("Elevate your game.", "Elevate"),
                ("A seamless experience.", "seamless"),
                ("It weaves a tapestry of ideas.", "tapestry"),
                ("The groundbreaking research.", "groundbreaking"),
                ("Seamless integration with your tools.", "Seamless"),
            ],
        );
        assert_none_flagged(
            stock_phrase,
            &["It's not installed by default; it's added by setup."],
        );
        assert_none_flagged(
            not_x_but_y,
            &["It's not installed by default; it's added by setup."],
        );
        assert_none_flagged(chat_residue, &["Feel free to open an issue."]);
        assert_none_flagged(hype_cta, &["Feel free to open an issue."]);
    }

    #[test]
    fn ai_vocab_windows_do_not_split_a_character() {
        // multi-byte characters around the match must not panic when the windows are cut
        let doc = format!("{} tapestry {}", "é".repeat(40), "é".repeat(40));
        assert_eq!(found(ai_vocab, &doc), ["tapestry"]);
        let doc = format!("{} seamless {}", "é".repeat(40), "é".repeat(40));
        assert_eq!(found(ai_vocab, &doc), ["seamless"]);
    }

    #[test]
    fn stock_phrase_flags_each_alternative() {
        assert_each_flagged(
            stock_phrase,
            &[
                (
                    "In today's fast-paced world, teams ship.",
                    "In today's fast-paced",
                ),
                ("In today's digital age we ship.", "In today's digital age"),
                (
                    "In today's digital world we ship.",
                    "In today's digital world",
                ),
                (
                    "In today's rapidly evolving market.",
                    "In today's rapidly evolving",
                ),
                (
                    "In today's ever-changing market.",
                    "In today's ever-changing",
                ),
                ("In an ever-evolving market.", "In an ever-evolving"),
                ("In the ever-evolving market.", "In the ever-evolving"),
                ("We embark on a journey to ship.", "embark on a journey"),
                ("She embarked on the quest.", "embarked on the quest"),
                ("Embarking on our adventure.", "Embarking on our adventure"),
                ("They embarks on this mission.", "embarks on this mission"),
                ("Let's dive in.", "Let's dive in"),
                ("Let's dive into the code.", "Let's dive into"),
                ("Let's dive deeper.", "Let's dive deeper"),
                ("Without further ado, the release.", "Without further ado"),
                ("Gone are the days of manual deploys.", "Gone are the days"),
                (
                    "Imagine a world where builds never fail.",
                    "Imagine a world where",
                ),
                ("Look no further.", "Look no further"),
                ("In the realm of caching.", "In the realm of"),
                ("This is a testament to the team.", "a testament to"),
                (
                    "It stands as a testament to the team.",
                    "stands as a testament to",
                ),
                (
                    "Navigating the complexities of caching.",
                    "Navigating the complexities",
                ),
                (
                    "We navigate the complexities of caching.",
                    "navigate the complexities",
                ),
                ("Unlock the power of caching.", "Unlock the power"),
                (
                    "Unlock the full potential of caching.",
                    "Unlock the full potential",
                ),
                ("It unlocks the potential.", "unlocks the potential"),
                ("Unlocking the full power.", "Unlocking the full power"),
                ("Harness the power of caching.", "Harness the power of"),
                (
                    "It harnesses the power of caching.",
                    "harnesses the power of",
                ),
                (
                    "Harnessing the power of caching.",
                    "Harnessing the power of",
                ),
                (
                    "It's important to note that caches expire.",
                    "It's important to note that",
                ),
                (
                    "It's worth noting that caches expire.",
                    "It's worth noting that",
                ),
            ],
        );
    }

    #[test]
    fn stock_phrase_ignores_the_plain_uses() {
        assert_none_flagged(
            stock_phrase,
            &[
                "In today's meeting we shipped the fix.",
                "Unlock the door with the key.",
                "Unlock the file before you write.",
                "Navigate the menu to settings.",
                "Harness the horses.",
                "In the realm lives a king.",
                "It's important to back up first.",
                "Let's dive bar night.",
                "Gone are the others.",
                "Imagine a world.",
                "The embark button starts the trip.",
                "A testament was filed.",
            ],
        );
    }

    #[test]
    fn chat_residue_flags_each_alternative() {
        assert_each_flagged(
            chat_residue,
            &[
                ("I hope this helps.", "I hope this helps"),
                ("Hope that helps!", "Hope that helps"),
                ("Hopefully this helps.", "Hopefully this helps"),
                ("Hopefully that helps.", "Hopefully that helps"),
                ("I hope this was helpful.", "I hope this was helpful"),
                ("I hope that is helpful.", "I hope that is helpful"),
                (
                    "I hope you find this helpful.",
                    "I hope you find this helpful",
                ),
                (
                    "I hope you found this useful.",
                    "I hope you found this useful",
                ),
                (
                    "Let me know if you have any questions.",
                    "Let me know if you have any",
                ),
                (
                    "Let me know if you need any help.",
                    "Let me know if you need any",
                ),
                (
                    "Let me know if you need anything else.",
                    "Let me know if you need anything",
                ),
                (
                    "Let me know if there's anything to change.",
                    "Let me know if there's anything",
                ),
                ("I'd be happy to help.", "I'd be happy to help"),
                ("I'd be glad to help.", "I'd be glad to help"),
                ("I'm happy to help.", "I'm happy to help"),
                ("I'm glad to help.", "I'm glad to help"),
                ("I'm here to help.", "I'm here to help"),
                ("Great question!", "Great question"),
                ("You're absolutely right.", "You're absolutely right"),
                (
                    "As an AI language model, I cannot browse.",
                    "As an AI language model, I",
                ),
                ("As an AI assistant I cannot.", "As an AI assistant I"),
                ("As an AI, I cannot.", "As an AI, I"),
                ("As an AI I cannot.", "As an AI I"),
                (
                    "Here's a revised version of the text.",
                    "Here's a revised version",
                ),
                (
                    "Here's the updated draft below.",
                    "Here's the updated draft",
                ),
                (
                    "Here's a polished version below.",
                    "Here's a polished version",
                ),
                ("Here's the rewritten draft.", "Here's the rewritten draft"),
                ("Certainly! Here is the file.", "Certainly!"),
                ("Done. Absolutely! Here it is.", "Absolutely!"),
                ("Is it ready? Sure! Here it is.", "Sure!"),
            ],
        );
    }

    #[test]
    fn chat_residue_ignores_the_plain_uses() {
        assert_none_flagged(
            chat_residue,
            &[
                "Certainly the binary is faster than the shell version.",
                "Of course the hook still runs when the binary is missing; it allows the edit.",
                "Feel free to open an issue when the guard blocks a command you need.",
                "I am sure! of nothing",
                "The answer is certainly! no",
                "Let me know about the plan.",
                "A great questionnaire.",
                "Hopefully the build passes.",
                "I hope the build passes.",
                "As an AIDE, I",
                "Here's the plan for the week.",
                "This is sure!",
            ],
        );
    }

    #[test]
    fn a_sentence_opening_chat_word_is_found_on_its_own_line() {
        let doc = "Done.\nCertainly! Here it is.";
        let hits = chat_residue(&View::plain(doc));
        assert_eq!(hits.len(), 1);
        let at = doc.find("Certainly").unwrap();
        assert_eq!(
            hits[0].span,
            Span {
                start: at,
                end: at + "Certainly!".len()
            }
        );
        // at the start of the document and of a line
        assert_eq!(found(chat_residue, "Sure! Here."), ["Sure!"]);
        assert_eq!(found(chat_residue, "x\nSure! Here."), ["Sure!"]);
        assert_eq!(
            found(chat_residue, "Done.\n\nAbsolutely! Here."),
            ["Absolutely!"]
        );
    }

    #[test]
    fn not_x_but_y_flags_each_of_the_five_forms() {
        assert_each_flagged(
            not_x_but_y,
            &[
                ("It's not a tool, it's a platform.", "It's not a tool, it's"),
                (
                    "This is not just a cache; it is a memory layer.",
                    "This is not just a cache; it is",
                ),
                (
                    "That was not merely the code: this is the people.",
                    "That was not merely the code: this is",
                ),
                (
                    "It's not a tool \u{2014} it's a platform.",
                    "It's not a tool \u{2014} it's",
                ),
                (
                    "It's not a tool \u{2013} it's a platform.",
                    "It's not a tool \u{2013} it's",
                ),
                (
                    "This isn't about speed; it's about trust.",
                    "This isn't about speed; it's",
                ),
                (
                    "That wasn't just a bug, it's a feature.",
                    "That wasn't just a bug, it's",
                ),
                (
                    "The cache isn't just a store. It's a layer.",
                    "isn't just a store. It's",
                ),
                (
                    "These are not merely files, they're a record.",
                    "are not merely files, they're",
                ),
                (
                    "Builds aren't only slow; they are costly.",
                    "aren't only slow; they are",
                ),
                (
                    "This is not about speed. It is about trust.",
                    "This is not about speed. It is about",
                ),
                (
                    "It isn't really about speed. It's about trust.",
                    "It isn't really about speed. It's about",
                ),
                (
                    "It's more than a tool; it's a platform.",
                    "It's more than a tool; it's",
                ),
                (
                    "This is more than a tool, it's a platform.",
                    "This is more than a tool, it's",
                ),
                (
                    "It's less about speed and more about trust.",
                    "It's less about speed and more about",
                ),
            ],
        );
    }

    #[test]
    fn curly_apostrophes_match() {
        let hits = found(
            not_x_but_y,
            "It\u{2019}s not a tool, it\u{2019}s a platform.",
        );
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(
            found(stock_phrase, "In today\u{2019}s fast-paced world."),
            ["In today\u{2019}s fast-paced"]
        );
        assert_eq!(
            found(stock_phrase, "Let\u{2019}s dive in."),
            ["Let\u{2019}s dive in"]
        );
        assert_eq!(
            found(chat_residue, "I\u{2019}d be happy to help."),
            ["I\u{2019}d be happy to help"]
        );
        assert_eq!(
            found(hype_cta, "We\u{2019}ve got you covered."),
            ["We\u{2019}ve got you covered"]
        );
        assert_eq!(
            found(hype_cta, "Don\u{2019}t miss out."),
            ["Don\u{2019}t miss out"]
        );
    }

    #[test]
    fn phrases_match_across_a_line_wrap() {
        assert_eq!(
            found(chat_residue, "I hope this\nhelps."),
            ["I hope this helps"]
        );
        assert_eq!(
            found(stock_phrase, "Without\nfurther   ado, go."),
            ["Without further ado"]
        );
        assert_eq!(
            found(hype_cta, "Join thousands\nof teams."),
            ["Join thousands of"]
        );
        let wrapped = found(not_x_but_y, "It's not\na tool, it's a platform.");
        assert_eq!(wrapped.len(), 1, "{wrapped:?}");
        let wrapped = found(not_x_but_y, "It's not a tool,\nit's a platform.");
        assert_eq!(wrapped.len(), 1, "{wrapped:?}");
        assert_eq!(found(ai_vocab, "We delve\ninto it."), ["delve"]);
    }

    #[test]
    fn not_x_but_y_ignores_the_plain_uses() {
        assert_none_flagged(
            not_x_but_y,
            &[
                "It's not installed by default; it's added by setup.",
                "This is not a bug.",
                "It isn't a bug; use the flag.",
                "It's not a tool.",
                "The cache is not just fast.",
                "It's more than 3 GB.",
                "It is not about speed.",
                "It is a tool, and it is a platform.",
                "Is this a tool? It is a platform.",
                "It's not a bug. We fixed the flag.",
            ],
        );
    }

    #[test]
    fn not_x_but_y_reports_one_hit_for_overlapping_forms() {
        // forms 1 and 3 both match here: the earlier one is kept
        let hits = found(
            not_x_but_y,
            "This is not just a cache; it is a memory layer.",
        );
        assert_eq!(hits, ["This is not just a cache; it is"]);
        // two separate sentences stay two hits
        let hits = found(
            not_x_but_y,
            "It's not a tool, it's a platform. It's not a bug, it's a feature.",
        );
        assert_eq!(hits.len(), 2, "{hits:?}");
    }

    #[test]
    fn hype_cta_flags_each_alternative() {
        assert_each_flagged(
            hype_cta,
            &[
                ("Ready to transform your team?", "Ready to transform"),
                ("Ready to supercharge your team?", "Ready to supercharge"),
                ("Ready to elevate your team?", "Ready to elevate"),
                ("Ready to level up your team?", "Ready to level up"),
                ("Ready to take your team further?", "Ready to take your"),
                ("Take it to the next level.", "to the next level"),
                ("Join thousands of teams.", "Join thousands of"),
                ("Join millions of users.", "Join millions of"),
                ("Join hundreds of teams.", "Join hundreds of"),
                ("Transform the way you work.", "Transform the way you"),
                (
                    "Transform your team workflow today.",
                    "Transform your team workflow",
                ),
                (
                    "Transform your sales experience today.",
                    "Transform your sales experience",
                ),
                (
                    "Transform your small business today.",
                    "Transform your small business",
                ),
                ("Revolutionise the way you work.", "Revolutionise the way"),
                ("Revolutionize your work.", "Revolutionize your"),
                ("It will revolutionize how teams ship.", "revolutionize how"),
                ("The future of work is here.", "The future of work is here"),
                ("The future of ship is now.", "The future of ship is now"),
                ("Say goodbye to slow builds.", "Say goodbye to"),
                ("Every tool at your fingertips.", "at your fingertips"),
                ("Deploy in just a few clicks.", "in just a few clicks"),
                ("We've got you covered.", "We've got you covered"),
                (
                    "The possibilities are endless.",
                    "The possibilities are endless",
                ),
                ("Start your journey today.", "Start your journey"),
                ("Begin your journey today.", "Begin your journey"),
                ("Don't miss out.", "Don't miss out"),
            ],
        );
    }

    #[test]
    fn hype_cta_ignores_the_plain_uses() {
        assert_none_flagged(
            hype_cta,
            &[
                "Join the mailing list.",
                "Transform the input into JSON.",
                "Transform your data with the map step.",
                "Say hello to the team.",
                "The future of the project is open.",
                "Ready to run the tests?",
                "Revolution in the cache.",
                "Join thousands.",
                "Take your time.",
            ],
        );
    }

    #[test]
    fn masked_bytes_never_join_a_phrase() {
        let mask = char::from(MASK);
        let doc = format!("hope {mask}{mask}{mask} helps");
        assert!(found(chat_residue, &doc).is_empty());
        assert!(found(chat_residue, &format!("I hope this{mask}helps")).is_empty());
        assert!(found(ai_vocab, &format!("de{mask}lve")).is_empty());
        assert!(found(ai_vocab, &format!("{mask}delve{mask}")).len() == 1);
        assert!(found(stock_phrase, &format!("Without {mask}{mask} ado")).is_empty());
        assert!(found(hype_cta, &format!("Say {mask} to")).is_empty());
    }

    #[test]
    fn hits_carry_their_rule_span_and_the_code_excerpt() {
        let doc = "We delve into caching.";
        let hits = ai_vocab(&View::plain(doc));
        assert_eq!(
            hits,
            [Hit {
                rule: "text.ai-vocab",
                span: Span { start: 3, end: 8 },
                parts: Vec::new(),
                excerpt: "delve".to_string(),
            }]
        );
        // the excerpt reads `code`, the match reads `text`
        let view = View {
            code: doc.to_uppercase(),
            ..View::plain(doc)
        };
        assert_eq!(ai_vocab(&view)[0].excerpt, "DELVE");
        for (rule, doc, id) in [
            (
                stock_phrase as Rule,
                "Look no further.",
                "text.stock-phrase",
            ),
            (chat_residue as Rule, "Great question!", "text.chat-residue"),
            (
                not_x_but_y as Rule,
                "It's not a tool, it's a platform.",
                "text.not-x-but-y",
            ),
            (hype_cta as Rule, "Say goodbye to bugs.", "text.hype-cta"),
        ] {
            let hits = rule(&View::plain(doc));
            assert_eq!(hits.len(), 1, "{id}");
            assert_eq!(hits[0].rule, id);
            assert!(hits[0].parts.is_empty(), "{id}");
        }
    }

    #[test]
    fn rules_find_nothing_in_an_empty_view() {
        for rule in [ai_vocab, stock_phrase, chat_residue, not_x_but_y, hype_cta] {
            assert!(rule(&View::plain("")).is_empty());
        }
    }
}
