//! The text rules: stock vocabulary, stock phrases, chat residue, the denied-alternative contrast
//! and hype calls to action.
//!
//! Every rule matches on `view.text`, where masked bytes are `MASK` and so never join a phrase.
//! A hit's span is the match; its excerpt is read from `view.code`, which keeps the prose bytes.
//! Every pattern is case-insensitive, `['’]` stands for either apostrophe, and `\s+` sits between
//! words so a soft line wrap still matches.

use std::sync::LazyLock;

use regex::{Match, Regex};

use crate::{Hit, MASK, Span, View, excerpt};

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

/// A machine that lifts things, as the subject of `elevate` (the elevator elevates the car).
static ELEVATE_SUBJECT: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:elevators?|lifts?|hoists?|cranes?)\s+$"));

/// A merge that leaves no seam, as in merging two sorted lists.
static SEAMLESS_MERGE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^\s+(?:merges?|merging)\b"));

/// True when the stock word is used for what it literally means.
fn is_literal_use(text: &str, m: &Match<'_>) -> bool {
    let word = m.as_str().to_ascii_lowercase();
    let next = after(text, m.end(), AFTER);
    if word.starts_with("seamless") {
        SEAMLESS_BEFORE.is_match(next) || (word == "seamless" && SEAMLESS_MERGE.is_match(next))
    } else if word.starts_with("elevat") {
        ELEVATE_OBJECT.is_match(next) || ELEVATE_SUBJECT.is_match(before(text, m.start(), BEFORE))
    } else if word.starts_with("tapestr") {
        TAPESTRY_AFTER.is_match(next) || TAPESTRY_BEFORE.is_match(before(text, m.start(), BEFORE))
    } else if word.starts_with("groundbreaking") {
        GROUNDBREAKING_AFTER.is_match(next)
    } else {
        false
    }
}

/// How far the Title Case check reads on each side of a capitalized word.
const LINE_WINDOW: usize = 120;

/// A capital and then at least one lower-case letter: `Tapestry`, not `TAPESTRY` or `tapestry`.
fn is_capitalized(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(char::is_uppercase) && chars.any(char::is_lowercase)
}

/// True when nothing but markup and opening punctuation stands between byte `at` and the start
/// of the text, the start of a line or the end of a sentence.
fn starts_a_sentence(text: &str, at: usize) -> bool {
    let mask = char::from(MASK);
    for c in text[..at].chars().rev() {
        match c {
            '\n' | '.' | '!' | '?' | ':' => return true,
            ' ' | '\t' | '"' | '\'' | '\u{201c}' | '\u{2018}' | '(' | '[' | '*' | '_' | '#'
            | '>' | '|' | '+' | '-' => {}
            c if c == mask => {}
            _ => return false,
        }
    }
    true
}

/// The whole text of a link: `[Delve]`, or `[Delve` before a masked `](url)`.
fn is_link_text(text: &str, m: &Match<'_>) -> bool {
    let bytes = text.as_bytes();
    m.start() > 0
        && bytes[m.start() - 1] == b'['
        && matches!(bytes.get(m.end()), Some(&b']') | Some(&MASK))
}

/// A capitalized word or a version number right after the hit: `Unleash Edge`, `Tapestry 5`.
static NAME_AFTER: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[ \t]+(?:\p{Lu}|v?\d+(?:\.\d+)*(?:$|[\s,;:)!?]|\.(?:$|\s)))"));

/// A word of three letters or more, for the Title Case check.
static TITLE_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\p{L}[\p{L}\p{N}'’-]{2,}"));

/// True when at least two thirds of the other words near the hit on its line are capitalized,
/// as in a heading (`The Seamless Way To Ship`): there a capital says nothing about a name.
fn on_a_title_case_line(text: &str, m: &Match<'_>) -> bool {
    let lead = before(text, m.start(), LINE_WINDOW);
    let lead = lead.rfind('\n').map_or(lead, |i| &lead[i + 1..]);
    let tail = after(text, m.end(), LINE_WINDOW);
    let tail = tail.find('\n').map_or(tail, |i| &tail[..i]);
    let (mut words, mut capitals) = (0, 0);
    for part in [lead, tail] {
        for word in TITLE_WORD.find_iter(part) {
            words += 1;
            if word.as_str().chars().next().is_some_and(char::is_uppercase) {
                capitals += 1;
            }
        }
    }
    words >= 2 && capitals * 3 >= words * 2
}

/// True when a capitalized stock word is a name (Apache Tapestry, the Unleash server, a link to
/// Delve): link text, a capital in mid-sentence, or a name or version right after it, outside a
/// Title Case line.
fn is_proper_noun(text: &str, m: &Match<'_>) -> bool {
    if !is_capitalized(m.as_str()) {
        return false;
    }
    if is_link_text(text, m) {
        return true;
    }
    let named =
        !starts_a_sentence(text, m.start()) || NAME_AFTER.is_match(after(text, m.end(), AFTER));
    named && !on_a_title_case_line(text, m)
}

pub(crate) fn ai_vocab(view: &View) -> Vec<Hit> {
    AI_VOCAB_WORDS
        .find_iter(&view.text)
        .filter(|m| !is_literal_use(&view.text, m) && !is_proper_noun(&view.text, m))
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

/// A second clause that states a fact rather than renaming the thing: optional adverbs, then a
/// past participle ("it is also read on reload", "it is set by the installer").
static FACTUAL_CLAUSE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)^\s+(?:(?:also|still|then|now|only|always|usually|often|never|already)\s+)*",
        r"(?:[a-z]*[a-df-z]ed|set|read|run|built|made|kept|done|shown|written|given|taken|known|",
        r"left|found|sent|put|held|bound|cut|shut|split|hidden|chosen|driven|broken|drawn|thrown|",
        r"seen|begun|bought|brought|caught|sold|told|paid|said|lost|meant|spent|stuck|spun)\b",
    ))
});

/// The forms whose second clause is a bare "it is" (1 to 3); forms 4 and 5 end on `about`.
const BARE_SECOND_CLAUSE: usize = 3;

pub(crate) fn not_x_but_y(view: &View) -> Vec<Hit> {
    let text = &view.text;
    let mut spans: Vec<Span> = CONTRASTS
        .iter()
        .enumerate()
        .flat_map(|(form, pattern)| {
            pattern
                .find_iter(text)
                .filter(move |m| {
                    form >= BARE_SECOND_CLAUSE
                        || !FACTUAL_CLAUSE.is_match(after(text, m.end(), AFTER + 20))
                })
                .map(|m| span_of(&m))
        })
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

/// How far back "to the next level" looks for the verb of its sentence.
const SENTENCE: usize = 200;

/// A step down a structure, right before "to the next level" (descends, goes down, recurses).
static DESCENT_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:descend(?:s|ed|ing)?|go(?:es|ing)?\s+down|mov(?:e|es|ed|ing)|recurs(?:e|es|ed|ing))\s+$",
    )
});

/// The verbs of the slogan: take, bring, elevate, push, kick or boost something to the next level.
static LIFT_VERB: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:take|takes|taking|took|bring|brings|bringing|brought|elevat(?:e|es|ed|ing)|push(?:es|ed|ing)?|kick(?:s|ed|ing)?|boost(?:s|ed|ing)?)\b",
    )
});

/// A version as what is said goodbye to: a release that goes away, not a pain point.
static VERSION_AFTER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s+(?:the\s+)?(?:v\d|version\s+\d|\d)"));

/// True when a hype phrase is used literally: a walk to the next level of a tree, or a goodbye
/// to an old version.
fn is_plain_hype(text: &str, m: &Match<'_>) -> bool {
    let phrase = m.as_str().to_ascii_lowercase();
    if phrase.starts_with("to") {
        let lead = before(text, m.start(), SENTENCE);
        let sentence = lead.rfind(['.', '!', '?']).map_or(lead, |i| &lead[i + 1..]);
        DESCENT_BEFORE.is_match(before(text, m.start(), BEFORE)) || !LIFT_VERB.is_match(sentence)
    } else if phrase.starts_with("say") {
        VERSION_AFTER.is_match(after(text, m.end(), AFTER))
    } else {
        false
    }
}

pub(crate) fn hype_cta(view: &View) -> Vec<Hit> {
    HYPE_PHRASES
        .find_iter(&view.text)
        .filter(|m| !is_plain_hype(&view.text, m))
        .map(|m| hit(HYPE_CTA, view, span_of(&m)))
        .collect()
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

    /// Each finding of a Markdown doc as `rule excerpt`, read through the real mask.
    fn markdown_findings(doc: &str) -> Vec<String> {
        crate::check_all(crate::Kind::Markdown, doc)
            .into_iter()
            .map(|f| format!("{} {}", f.rule, f.excerpt))
            .collect()
    }

    #[test]
    fn review_proper_nouns_and_technical_phrases_pass() {
        // the allow examples of the pre-release review, item 3, as a Markdown file reads them
        let mut wrong = Vec::new();
        for doc in [
            "[Delve](https://github.com/go-delve/delve) is the Go debugger",
            "Apache Tapestry",
            "the Unleash server",
            "descends to the next level of the tree",
            "is not only read at start. It is also read on reload",
            "This is not the default, it is set by the installer",
            "Say goodbye to the v1 config: it is removed in 2.0",
        ] {
            let got = markdown_findings(doc);
            if !got.is_empty() {
                wrong.push(format!("{doc:?}: {got:?}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn chat_residue_in_a_file_stays_a_tell() {
        // lead ruling: the closing line of a chat answer is slop when it lands in a file
        assert_eq!(
            markdown_findings("Thanks for reading. Let me know if you have any questions."),
            ["text.chat-residue Let me know if you have any"]
        );
    }

    #[test]
    fn ai_vocab_skips_proper_nouns() {
        assert_none_flagged(
            ai_vocab,
            &[
                "Apache Tapestry",
                "the Unleash server",
                "Start the Unleash server before the SDK connects.",
                "Unleash Edge proxies the API for the clients.",
                "Tapestry 5 renders each page from a plain Java class.",
                "Tapestry v5.4 renders each page.",
                "MediaPipe Holistic tracks the body and the face in one pass.",
                "The Synergy client shares one keyboard between two computers.",
                "[Delve] is the Go debugger.",
                "Step through the handler with [Delve](/dlv) and set a breakpoint.",
            ],
        );
    }

    #[test]
    fn ai_vocab_still_flags_capitalized_tells() {
        assert_each_flagged(
            ai_vocab,
            &[
                // a Title Case line is a heading, not a name
                ("Elevate Your Workflow", "Elevate"),
                ("Unleash Your Potential", "Unleash"),
                ("The Seamless Way To Ship Faster", "Seamless"),
                // the start of a sentence, a list item, a heading, bold text
                ("Done. Seamless sync is on.", "Seamless"),
                ("- Seamless sync across devices", "Seamless"),
                ("## Seamless sync", "Seamless"),
                ("**Seamless** sync for teams.", "Seamless"),
                // shouting is not a name, and link text in lower case is prose
                ("We DELVE into caching.", "DELVE"),
                ("Learn about [seamless sync](/docs).", "seamless"),
            ],
        );
    }

    #[test]
    fn ai_vocab_skips_technical_literal_uses() {
        assert_none_flagged(
            ai_vocab,
            &[
                "The elevator elevates the car to the top floor in eight seconds.",
                "A seamless merge of two sorted lists runs in linear time.",
            ],
        );
        assert_each_flagged(
            ai_vocab,
            &[
                ("The new logo elevates the car brand.", "elevates"),
                ("A seamless handover for your team.", "seamless"),
                ("Your tools seamlessly merge.", "seamlessly"),
            ],
        );
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
    fn not_x_but_y_skips_a_factual_second_clause() {
        assert_none_flagged(
            not_x_but_y,
            &[
                "The config is not only read at start. It is also read on reload.",
                "This is not the default, it is set by the installer.",
                "That's not a typo, it is spelled that way in the spec.",
                "It's not an error, it's logged as a warning.",
                "The lock file isn't only read by the CLI; it is also checked by CI.",
            ],
        );
    }

    #[test]
    fn not_x_but_y_still_flags_a_reframing() {
        assert_each_flagged(
            not_x_but_y,
            &[
                (
                    "It's not a tool, it's also a platform.",
                    "It's not a tool, it's",
                ),
                (
                    "It's not about speed, it's trust.",
                    "It's not about speed, it's",
                ),
                (
                    "The cache isn't just a store. It's still a layer.",
                    "isn't just a store. It's",
                ),
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
    fn hype_cta_skips_tree_levels_and_versions() {
        assert_none_flagged(
            hype_cta,
            &[
                "The walker descends to the next level of the tree.",
                "The cursor moves to the next level of the outline when you press Tab.",
                "The parser takes one token and recurses to the next level of the grammar.",
                "Press Tab to go down to the next level.",
                // no slogan verb in the sentence; the one before it does not count
                "Indent the item to the next level of the list.",
                "Take a backup. Indent the item to the next level of the list.",
                "Say goodbye to the v1 config: it is removed in 2.0.",
                "Say goodbye to version 1 of the API.",
            ],
        );
    }

    #[test]
    fn hype_cta_still_flags_the_marketing_uses() {
        assert_each_flagged(
            hype_cta,
            &[
                ("Bring your docs to the next level.", "to the next level"),
                ("We take testing to the next level.", "to the next level"),
                ("Elevate your team to the next level.", "to the next level"),
                ("Push your builds to the next level.", "to the next level"),
                (
                    "Say goodbye to the hassle of manual deploys.",
                    "Say goodbye to",
                ),
                ("Say goodbye to the old way of shipping.", "Say goodbye to"),
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
