//! The code-certain UI rules: nine tells that can be read straight off source code.
//!
//! The style rules read `view.style` and `view.classes` (regions of `view.code`), the others read
//! `view.code` or, for copy, `view.text`. Every pattern names a fixed value or shape, so a hit is
//! a tell and not a matter of taste.

use super::color::{Rgba, colors_in, is_blue_to_pink, is_violet, is_violet_family};
use crate::{Hit, Span, View, excerpt};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Custom properties resolve at most this many links deep, and never past this many bytes.
const MAX_VAR_DEPTH: usize = 6;
const MAX_RESOLVED: usize = 1 << 16;
/// A gradient call longer than this many bytes is not read: no real one is, and a long one that
/// holds other calls (nested to any depth) would make the work grow with the square of the text.
const MAX_CALL: usize = 4096;
/// `filter: blur(n)` is an orb from this many pixels.
const MIN_ORB_BLUR: u64 = 24;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("ui rule regex")
}

static GRADIENT_CALL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(?:linear|radial|conic)-gradient\("));
/// The first item of a gradient's arguments when it is no colour stop: an angle, a direction
/// (`to right`), a shape or size (`circle`, `ellipse`, `closest-side`), a position (`at 30% 20%`),
/// a conic start (`from 0deg`) or a colour space (`in oklch`).
static PRELUDE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:[-+.\d]|(?:to|from|in|at)\s|circle|ellipse|closest-|farthest-)"));
/// A colour hint between two stops: a bare length or percentage.
static HINT: LazyLock<Regex> = LazyLock::new(|| re(r"^[-+]?(?:\d+\.?\d*|\.\d+)(?:%|[A-Za-z]+)?$"));
static VAR_DECL: LazyLock<Regex> = LazyLock::new(|| re(r#"(--[A-Za-z0-9_-]+)\s*:\s*([^;{}"']*)"#));
static VAR_USE: LazyLock<Regex> =
    LazyLock::new(|| re(r"var\(\s*(--[A-Za-z0-9_-]+)\s*(?:,[^()]*)?\)"));
static VIOLET_STOP: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:from|via|to)-(?:indigo|violet|purple|fuchsia)-\d{2,3}\b"));
static GRADIENT_UTILITY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\bbg-(?:gradient|linear|radial)\S*"));

static BRACES: LazyLock<Regex> = LazyLock::new(|| re(r"\{[^{}]*\}"));
static CLIP_TEXT: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)((?:-webkit-)?background-clip\s*:\s*['"]?text\b)"#));
static CLIP_TEXT_JS: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)(backgroundClip\s*:\s*['"]text['"])"#));
static FILL_CLEAR: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?i)(?:^|[^-\w])((?:-webkit-text-fill-color|color)\s*:\s*['"]?transparent\b)"#)
});
static FILL_CLEAR_JS: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?i)(?:^|[^-\w])((?:WebkitTextFillColor|color)\s*:\s*['"]transparent['"])"#)
});
static CLIP_UTILITY: LazyLock<Regex> = LazyLock::new(|| re(r"\bbg-clip-text\b"));
static CLEAR_UTILITY: LazyLock<Regex> = LazyLock::new(|| re(r"\btext-transparent\b"));

static SHADCN_PRIMARY: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)--primary\s*:\s*(?:oklch\(\s*0\.205\s+0\s+0\s*\)|222\.2\s+47\.4%\s+11\.2%|240\s+5\.9%\s+10%|0\s+0%\s+9%)",
    )
});

static ROUND: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(border-radius\s*:\s*(?:50%|100%|9{3,}px))"));
static BLUR: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(?:^|[^-\w])((?:-webkit-)?filter\s*:\s*blur\(\s*(\d+)[^)]*\))"));
static ROUNDED_FULL: LazyLock<Regex> = LazyLock::new(|| re(r"\brounded-full\b"));
static BLUR_UTILITY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:^|[^\w-])blur-(?:2xl|3xl|\[\d{2,}px\])"));
static OUT_OF_FLOW: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b(?:absolute|fixed)\b|(?:^|[^\w-])-z-10\b"));

static EFFECT_LIBRARY: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"<(?:Spotlight|BackgroundBeams|BackgroundBeamsWithCollision|AnimatedBeam|ShimmerButton|NumberTicker|BorderBeam|",
        r"Meteors|SparklesCore|LampContainer|AuroraBackground|RetroGrid|OrbitingCircles|TextGenerateEffect|TypewriterEffect|HeroHighlight|",
        r"WavyBackground|InfiniteMovingCards|MagicCard|NeonGradientCard|ShineBorder|AnimatedGradientText|RainbowButton|FlipWords|",
        r"HoverBorderGradient|MovingBorder|GlowingEffect)\b|components/(?:magicui|aceternity)/"
    ))
});
static PLACEHOLDER_MEDIA: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(?:api\.dicebear\.com|i\.pravatar\.cc|ui-avatars\.com|robohash\.org|randomuser\.me/api/portraits|",
        r"placehold\.co|via\.placeholder\.com|picsum\.photos|source\.unsplash\.com/random)"
    ))
});
static PLACEHOLDER_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\blorem\s+ipsum\b|",
        r"\[(?:your\s+|company\s+|brand\s+|business\s+|product\s+)?(?:company|brand|business|product)\s+name\]|\[insert\b"
    ))
});
static FIVE_STARS: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"★{5}|(?:<Star(?:Icon)?\b[^>]*/>\s*){5}|",
        r"(?:\[\.\.\.Array\(5\)\]|Array\.from\(\{\s*length\s*:\s*5\s*\}\)|Array\(5\)\.fill\([^)]*\))[\s\S]{0,200}?(?:<Star|★)"
    ))
});
static GENERATOR: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r#"(?i)lovable-tagger|gpt-?engineer|\bv0\.dev\b|\bbolt\.new\b|"#,
        r#"name=["']generator["'][^>]*content=["'](?:v0|lovable|bolt)"#
    ))
});

pub(crate) fn purple_gradient(view: &View) -> Vec<Hit> {
    const ID: &str = "ui.purple-gradient";
    let mut calls = Vec::new();
    for (base, css) in regions(&view.code, &view.style) {
        for (start, open, end) in gradient_calls(css) {
            calls.push((shift(base, start, end), &css[open + 1..end - 1]));
        }
    }
    let vars = if calls.iter().any(|(_, arg)| arg.contains("var(")) {
        custom_properties(&view.code)
    } else {
        HashMap::new()
    };
    let mut hits: Vec<Hit> = calls
        .into_iter()
        .filter(|(_, arg)| is_purple_stock(&stops(&resolve(arg, &vars))))
        .map(|(span, _)| hit(ID, &view.code, span, Vec::new()))
        .collect();
    hits.extend(class_hits(view, ID, |v| {
        VIOLET_STOP.is_match(v) && GRADIENT_UTILITY.is_match(v)
    }));
    hits
}

pub(crate) fn gradient_text(view: &View) -> Vec<Hit> {
    const ID: &str = "ui.gradient-text";
    let mut hits = Vec::new();
    for (base, body) in bodies(view) {
        let pair = declarations(base, body, &CLIP_TEXT, &FILL_CLEAR)
            .or_else(|| declarations(base, body, &CLIP_TEXT_JS, &FILL_CLEAR_JS));
        if let Some(parts) = pair {
            hits.push(hit(ID, &view.code, parts[0], parts.to_vec()));
        }
    }
    hits.extend(class_hits(view, ID, |v| {
        CLIP_UTILITY.is_match(v) && CLEAR_UTILITY.is_match(v)
    }));
    hits
}

pub(crate) fn shadcn_default(view: &View) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (base, css) in regions(&view.code, &view.style) {
        for m in SHADCN_PRIMARY.find_iter(css) {
            hits.push(hit(
                "ui.shadcn-default",
                &view.code,
                shift(base, m.start(), m.end()),
                Vec::new(),
            ));
        }
    }
    hits
}

pub(crate) fn glow_orb(view: &View) -> Vec<Hit> {
    const ID: &str = "ui.glow-orb";
    let mut hits = Vec::new();
    for (base, body) in bodies(view) {
        let round = ROUND.captures(body).and_then(|c| c.get(1));
        let blur = BLUR
            .captures_iter(body)
            .find(|c| c[2].parse::<u64>().unwrap_or(u64::MAX) >= MIN_ORB_BLUR)
            .and_then(|c| c.get(1));
        if let (Some(round), Some(blur)) = (round, blur) {
            let parts = ordered(
                shift(base, round.start(), round.end()),
                shift(base, blur.start(), blur.end()),
            );
            hits.push(hit(ID, &view.code, parts[0], parts.to_vec()));
        }
    }
    hits.extend(class_hits(view, ID, |v| {
        ROUNDED_FULL.is_match(v) && BLUR_UTILITY.is_match(v) && OUT_OF_FLOW.is_match(v)
    }));
    hits
}

pub(crate) fn effect_library(view: &View) -> Vec<Hit> {
    find_all(&EFFECT_LIBRARY, &view.code, "ui.effect-library")
}

pub(crate) fn placeholder_media(view: &View) -> Vec<Hit> {
    find_all(&PLACEHOLDER_MEDIA, &view.code, "ui.placeholder-media")
}

pub(crate) fn placeholder_text(view: &View) -> Vec<Hit> {
    find_all(&PLACEHOLDER_TEXT, &view.text, "ui.placeholder-text")
}

pub(crate) fn five_star_row(view: &View) -> Vec<Hit> {
    find_all(&FIVE_STARS, &view.code, "ui.five-star-row")
}

pub(crate) fn generator_signature(view: &View) -> Vec<Hit> {
    find_all(&GENERATOR, &view.code, "ui.generator-signature")
}

/// A hit on `span` of `doc`; its excerpt is the text of the span.
fn hit(rule: &'static str, doc: &str, span: Span, parts: Vec<Span>) -> Hit {
    Hit {
        rule,
        span,
        parts,
        excerpt: excerpt(doc, span),
    }
}

/// One hit per match of `re` in `doc`.
fn find_all(re: &Regex, doc: &str, rule: &'static str) -> Vec<Hit> {
    re.find_iter(doc)
        .map(|m| hit(rule, doc, shift(0, m.start(), m.end()), Vec::new()))
        .collect()
}

fn shift(base: usize, start: usize, end: usize) -> Span {
    Span {
        start: base + start,
        end: base + end,
    }
}

/// The two spans in document order.
fn ordered(a: Span, b: Span) -> [Span; 2] {
    if b.start < a.start { [b, a] } else { [a, b] }
}

/// The text of each span of `doc` with the span's start; a span that is not a range of `doc` gives
/// nothing.
fn regions<'a>(doc: &'a str, spans: &'a [Span]) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    spans
        .iter()
        .filter_map(|s| doc.get(s.start..s.end).map(|text| (s.start, text)))
}

/// One hit per class span whose value passes `test`; the hit spans the whole value.
fn class_hits(view: &View, rule: &'static str, test: impl Fn(&str) -> bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (base, value) in regions(&view.code, &view.classes) {
        if test(value) {
            hits.push(hit(
                rule,
                &view.code,
                shift(base, 0, value.len()),
                Vec::new(),
            ));
        }
    }
    hits
}

/// The declaration lists of the style spans: each innermost `{...}` pair, or a whole span that
/// holds no braces (a `style` attribute).
fn bodies(view: &View) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for (base, css) in regions(&view.code, &view.style) {
        if !css.contains(['{', '}']) {
            out.push((base, css));
            continue;
        }
        for m in BRACES.find_iter(css) {
            out.push((base + m.start() + 1, &css[m.start() + 1..m.end() - 1]));
        }
    }
    out
}

/// The spans of the first capture group of `first` and `second` in `body`, in document order,
/// when both are there.
fn declarations(base: usize, body: &str, first: &Regex, second: &Regex) -> Option<[Span; 2]> {
    let a = first.captures(body)?.get(1)?;
    let b = second.captures(body)?.get(1)?;
    Some(ordered(
        shift(base, a.start(), a.end()),
        shift(base, b.start(), b.end()),
    ))
}

/// Each gradient call in `css` as `(start, open, end)`: where its name starts, its `(`, and the byte
/// after the `)` that closes it. A call is dropped when a `;`, `{` or `}` comes before its `)`, when
/// the text ends first, or when it is longer than `MAX_CALL`.
///
/// One pass over `css` pairs every `(` with its `)` on a stack, so the work is linear however many
/// calls are left open or nested. The old search from each call to the end of the text was quadratic.
fn gradient_calls(css: &str) -> Vec<(usize, usize, usize)> {
    let names: Vec<(usize, usize)> = GRADIENT_CALL
        .find_iter(css)
        .map(|m| (m.start(), m.end() - 1))
        .collect();
    let mut ends: Vec<Option<usize>> = vec![None; names.len()];
    // Open parentheses not yet closed, with the index of the gradient call each one starts.
    let mut open: Vec<(usize, Option<usize>)> = Vec::new();
    let mut next = 0;
    for (i, b) in css.bytes().enumerate() {
        match b {
            b'(' => {
                let call = (names.get(next).map(|n| n.1) == Some(i)).then_some(next);
                next += usize::from(call.is_some());
                open.push((i, call));
            }
            b')' => {
                if let Some((from, Some(call))) = open.pop()
                    && i + 1 - from <= MAX_CALL
                {
                    ends[call] = Some(i + 1);
                }
            }
            b';' | b'{' | b'}' => open.clear(),
            _ => {}
        }
    }
    names
        .into_iter()
        .zip(ends)
        .filter_map(|((start, open), end)| end.map(|end| (start, open, end)))
        .collect()
}

/// The colour stops of a gradient's arguments, each as the colours it holds: the items between
/// top-level commas, without the leading angle, shape or position and without colour hints (a bare
/// length or percentage).
fn stops(args: &str) -> Vec<Vec<Rgba>> {
    let mut items = top_level_items(args);
    if items.first().is_some_and(|first| PRELUDE.is_match(first)) {
        items.remove(0);
    }
    items
        .into_iter()
        .filter(|item| !item.is_empty() && !HINT.is_match(item))
        .map(colors_in)
        .collect()
}

/// `args` cut at the commas outside parentheses, each part trimmed.
fn top_level_items(args: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let (mut depth, mut from) = (0usize, 0usize);
    for (i, b) in args.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                items.push(args[from..i].trim());
                from = i + 1;
            }
            _ => {}
        }
    }
    items.push(args[from..].trim());
    items
}

/// The stock purple gradient: two or three stops with a violet among them, and either every stop
/// violet, indigo, purple or pink-violet, or (two stops) the other end a saturated blue or pink.
/// A stop with several colours (a var declared for a light and a dark theme) counts when any one
/// of them does. A longer list is a legend, a hue slider or a brand palette, not the tell.
fn is_purple_stock(stops: &[Vec<Rgba>]) -> bool {
    if !(2..=3).contains(&stops.len()) {
        return false;
    }
    let every = |is: fn(Rgba) -> bool| stops.iter().all(|s| s.iter().copied().any(is));
    let violet = stops.iter().any(|s| s.iter().copied().any(is_violet));
    violet
        && if stops.len() == 2 {
            every(is_blue_to_pink)
        } else {
            every(is_violet_family)
        }
}

/// Every `--name: value` declaration in `code`. A name declared twice (a light and a dark theme)
/// keeps both values.
fn custom_properties(code: &str) -> HashMap<&str, Vec<&str>> {
    let mut vars: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in VAR_DECL.captures_iter(code) {
        if let (Some(name), Some(value)) = (c.get(1), c.get(2)) {
            vars.entry(name.as_str())
                .or_default()
                .push(value.as_str().trim());
        }
    }
    vars
}

/// `arg` with each `var(--x)` replaced by every value declared for `--x`, up to `MAX_VAR_DEPTH`
/// links deep. A variable with no declaration stays, so its fallback colour is still read.
fn resolve(arg: &str, vars: &HashMap<&str, Vec<&str>>) -> String {
    let mut text = arg.to_string();
    for _ in 0..MAX_VAR_DEPTH {
        if vars.is_empty() || text.len() > MAX_RESOLVED || !text.contains("var(") {
            break;
        }
        let next = VAR_USE
            .replace_all(&text, |c: &regex::Captures| match vars.get(&c[1]) {
                Some(values) => values.join(" "),
                None => c[0].to_string(),
            })
            .into_owned();
        if next == text {
            break;
        }
        text = next;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MASK, Span};

    /// A view whose style span is the whole document, as the CSS kind builds it.
    fn css(doc: &str) -> View {
        View::plain(doc)
    }

    /// A view with no style span and one class span: the first occurrence of `value` in `doc`.
    fn classes(doc: &str, value: &str) -> View {
        let start = doc.find(value).expect("class value in doc");
        View {
            style: Vec::new(),
            classes: vec![Span {
                start,
                end: start + value.len(),
            }],
            ..View::plain(doc)
        }
    }

    /// A view whose `text` is all masked, as for source that holds no prose.
    fn code_only(doc: &str) -> View {
        let masked: String = doc
            .chars()
            .map(|c| if c == '\n' { c } else { char::from(MASK) })
            .collect();
        View {
            text: masked,
            ..View::plain(doc)
        }
    }

    fn seen<'a>(doc: &'a str, hits: &[Hit]) -> Vec<&'a str> {
        hits.iter()
            .map(|h| &doc[h.span.start..h.span.end])
            .collect()
    }

    fn part_texts<'a>(doc: &'a str, hit: &Hit) -> Vec<&'a str> {
        hit.parts.iter().map(|p| &doc[p.start..p.end]).collect()
    }

    #[test]
    fn a_violet_gradient_is_flagged_and_a_blue_one_is_not() {
        let doc = ".hero { background: linear-gradient(135deg, #6366f1, #a855f7); }";
        let hits = purple_gradient(&css(doc));
        assert_eq!(
            seen(doc, &hits),
            ["linear-gradient(135deg, #6366f1, #a855f7)"]
        );
        assert_eq!(hits[0].rule, "ui.purple-gradient");
        assert_eq!(hits[0].excerpt, "linear-gradient(135deg, #6366f1, #a855f7)");
        assert!(hits[0].parts.is_empty());
        let blue = ".hero { background: linear-gradient(#3b82f6, #0ea5e9); }";
        assert!(purple_gradient(&css(blue)).is_empty());
    }

    #[test]
    fn every_gradient_kind_and_colour_form_counts() {
        for doc in [
            "a { background: radial-gradient(circle, hsl(262 83% 58%), hsl(330 80% 60%)); }",
            "a { background: conic-gradient(from 0deg, rgba(124, 58, 237, 0.4), #ec4899); }",
            "a { background: repeating-linear-gradient(45deg, #3b82f6, #8b5cf6 10px); }",
            "a { background: LINEAR-GRADIENT(to right, #6366F1, #A855F7); }",
        ] {
            assert_eq!(purple_gradient(&css(doc)).len(), 1, "{doc}");
        }
    }

    #[test]
    fn a_var_is_resolved_against_its_declaration() {
        let doc = ":root { --brand: #7c3aed; }\n.a { background: linear-gradient(var(--brand), #3b82f6); }";
        let hits = purple_gradient(&css(doc));
        assert_eq!(seen(doc, &hits), ["linear-gradient(var(--brand), #3b82f6)"]);
        let chained = ":root { --a: var(--b); --b: var(--c); --c: #7c3aed; }\n.a { background: linear-gradient(var(--a), #3b82f6); }";
        assert_eq!(purple_gradient(&css(chained)).len(), 1);
        let plain = ":root { --brand: #3b82f6; }\n.a { background: linear-gradient(var(--brand), #0ea5e9); }";
        assert!(purple_gradient(&css(plain)).is_empty());
        let list = ":root { --stops: #6366f1, #a855f7; }\n.a { background: linear-gradient(90deg, var(--stops)); }";
        assert_eq!(
            purple_gradient(&css(list)).len(),
            1,
            "a var may hold both stops"
        );
    }

    #[test]
    fn var_resolution_stops_at_depth_six_and_on_cycles() {
        let deep = ":root { --a: var(--b); --b: var(--c); --c: var(--d); --d: var(--e); --e: var(--f); --f: var(--g); --g: #7c3aed; }\n.a { background: linear-gradient(var(--a), #3b82f6); }";
        assert!(
            purple_gradient(&css(deep)).is_empty(),
            "seven links are past the depth"
        );
        let cycle = ":root { --a: var(--a) var(--a); }\n.a { background: linear-gradient(var(--a), #3b82f6); }";
        assert!(purple_gradient(&css(cycle)).is_empty());
        let missing = ".a { background: linear-gradient(var(--nowhere), #3b82f6); }";
        assert!(purple_gradient(&css(missing)).is_empty());
        let fallback = ".a { background: linear-gradient(var(--nowhere, #6366f1), #3b82f6); }";
        assert_eq!(purple_gradient(&css(fallback)).len(), 1);
    }

    #[test]
    fn a_gradient_call_spans_only_its_own_parentheses() {
        let doc = "a { background: linear-gradient(#fff, #eee), linear-gradient(rgba(99, 102, 241, 1), #a855f7); }";
        let hits = purple_gradient(&css(doc));
        assert_eq!(
            seen(doc, &hits),
            ["linear-gradient(rgba(99, 102, 241, 1), #a855f7)"]
        );
        let open = "a { background: linear-gradient(#6366f1, #a855f7 ; }";
        assert!(
            purple_gradient(&css(open)).is_empty(),
            "an unclosed call is no call"
        );
    }

    #[test]
    fn a_viridis_legend_is_not_a_purple_gradient() {
        let doc = ".legend { background: linear-gradient(90deg,#440154,#3b528b,#21918c,#5ec962,#fde725); }";
        assert!(purple_gradient(&css(doc)).is_empty());
        let short = ".legend { background: linear-gradient(90deg, #440154, #21918c, #fde725); }";
        assert!(purple_gradient(&css(short)).is_empty());
    }

    #[test]
    fn a_hue_slider_is_not_a_purple_gradient() {
        let doc = ".hue { background: linear-gradient(to right, hsl(0 100% 50%), hsl(60 100% 50%), hsl(120 100% 50%), hsl(180 100% 50%), hsl(240 100% 50%), hsl(300 100% 50%), hsl(360 100% 50%)); }";
        assert!(purple_gradient(&css(doc)).is_empty());
    }

    #[test]
    fn a_brand_gradient_with_discord_blurple_is_not_a_purple_gradient() {
        for stops in ["#5865f2, #57f287", "#5865f2, #23272a", "#5865f2, #ffffff"] {
            let doc = format!(".brand {{ background: linear-gradient(135deg, {stops}); }}");
            assert!(purple_gradient(&css(&doc)).is_empty(), "{stops}");
        }
        let four =
            ".brand { background: linear-gradient(90deg, #5865f2, #7289da, #99aab5, #eb459e); }";
        assert!(purple_gradient(&css(four)).is_empty());
    }

    #[test]
    fn four_stops_are_never_a_purple_gradient() {
        let doc = ".a { background: linear-gradient(90deg, #6366f1, #7c3aed, #a855f7, #d946ef); }";
        assert!(purple_gradient(&css(doc)).is_empty());
    }

    #[test]
    fn the_classic_violet_pairs_and_triples_are_still_flagged() {
        for doc in [
            ".a { background: linear-gradient(135deg, #6366f1, #a855f7); }",
            ".a { background: linear-gradient(135deg, #6366f1 0%, #a855f7 100%); }",
            ".a { background: linear-gradient(to right, #6366f1, #8b5cf6, #d946ef); }",
            ".a { background: linear-gradient(135deg, #6366f1, #3b82f6); }",
            ".a { background: linear-gradient(135deg, #8b5cf6, #ec4899); }",
            ".a { background: radial-gradient(circle at 30% 20%, #a855f7, #6366f1); }",
            ".a { background: linear-gradient(0.25turn, #6366f1, 40%, #a855f7); }",
            ".a { background: linear-gradient(in oklch to right, #6366f1, #a855f7); }",
        ] {
            assert_eq!(purple_gradient(&css(doc)).len(), 1, "{doc}");
        }
    }

    #[test]
    fn a_violet_next_to_green_or_a_neutral_is_not_the_classic_pair() {
        for doc in [
            ".a { background: linear-gradient(135deg, #6366f1, #22c55e); }",
            ".a { background: linear-gradient(135deg, #6366f1, #f97316); }",
            ".a { background: linear-gradient(135deg, #6366f1, white); }",
            ".a { background: linear-gradient(135deg, #6366f1, transparent); }",
            ".a { background: linear-gradient(135deg, #6366f1, #a855f7, #22c55e); }",
            ".a { background: linear-gradient(#6366f1); }",
        ] {
            assert!(purple_gradient(&css(doc)).is_empty(), "{doc}");
        }
    }

    #[test]
    fn unclosed_gradient_calls_are_scanned_once() {
        let doc = "linear-gradient(".repeat((1 << 20) / "linear-gradient(".len());
        let start = std::time::Instant::now();
        assert!(purple_gradient(&css(&doc)).is_empty());
        let took = start.elapsed();
        assert!(took.as_secs() < 10, "1 MiB of unclosed calls took {took:?}");
    }

    #[test]
    fn deeply_nested_gradient_calls_stay_cheap() {
        let depth = (1 << 20) / ("linear-gradient(".len() + 1);
        let doc = format!(
            "{}#6366f1, #a855f7{}",
            "linear-gradient(".repeat(depth),
            ")".repeat(depth)
        );
        let start = std::time::Instant::now();
        let hits = purple_gradient(&css(&doc));
        let took = start.elapsed();
        assert!(took.as_secs() < 10, "1 MiB of nested calls took {took:?}");
        assert_eq!(
            hits.len(),
            1,
            "only the innermost call is a gradient of two stops"
        );
    }

    #[test]
    fn a_gradient_outside_a_style_span_is_not_read() {
        let doc = "<p>linear-gradient(#6366f1, #a855f7)</p>";
        let view = View {
            style: Vec::new(),
            ..View::plain(doc)
        };
        assert!(purple_gradient(&view).is_empty());
    }

    #[test]
    fn tailwind_violet_gradient_classes_are_flagged() {
        let doc = r#"<div class="p-4 bg-gradient-to-r from-indigo-500 to-purple-600">"#;
        let value = "p-4 bg-gradient-to-r from-indigo-500 to-purple-600";
        let hits = purple_gradient(&classes(doc, value));
        assert_eq!(seen(doc, &hits), [value]);
        let via = classes(
            "bg-linear-to-br from-blue-500 via-violet-400 to-cyan-500",
            "bg-linear-to-br from-blue-500 via-violet-400 to-cyan-500",
        );
        assert_eq!(purple_gradient(&via).len(), 1);
        let from_only = classes("from-indigo-500 p-2", "from-indigo-500 p-2");
        assert!(purple_gradient(&from_only).is_empty());
        let blue = classes(
            "bg-gradient-to-r from-blue-500 to-cyan-500",
            "bg-gradient-to-r from-blue-500 to-cyan-500",
        );
        assert!(purple_gradient(&blue).is_empty());
    }

    #[test]
    fn gradient_text_in_kebab_camel_and_tailwind_forms_is_flagged() {
        let kebab = ".title { background: linear-gradient(red, blue); background-clip: text; -webkit-text-fill-color: transparent; }";
        let hits = gradient_text(&css(kebab));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule, "ui.gradient-text");
        assert_eq!(
            part_texts(kebab, &hits[0]),
            [
                "background-clip: text",
                "-webkit-text-fill-color: transparent"
            ]
        );
        assert_eq!(seen(kebab, &hits), ["background-clip: text"]);

        let prefixed = ".t { -webkit-background-clip: text; color: transparent; }";
        assert_eq!(gradient_text(&css(prefixed)).len(), 1);

        let camel = "const s = <h1 style={{ backgroundClip: 'text', WebkitTextFillColor: 'transparent' }}>Hi</h1>;";
        let hits = gradient_text(&css(camel));
        assert_eq!(
            part_texts(camel, &hits[0]),
            [
                "backgroundClip: 'text'",
                "WebkitTextFillColor: 'transparent'"
            ]
        );

        let attr = r#"<h1 style="background-clip:text;color:transparent">Hi</h1>"#;
        let value = "background-clip:text;color:transparent";
        let start = attr.find(value).unwrap();
        let view = View {
            style: vec![Span {
                start,
                end: start + value.len(),
            }],
            ..View::plain(attr)
        };
        assert_eq!(
            gradient_text(&view).len(),
            1,
            "a style attribute has no braces"
        );

        let tw = r#"<h1 class="text-4xl bg-clip-text text-transparent">"#;
        let value = "text-4xl bg-clip-text text-transparent";
        assert_eq!(seen(tw, &gradient_text(&classes(tw, value))), [value]);
    }

    #[test]
    fn clip_alone_or_a_background_colour_is_not_gradient_text() {
        assert!(gradient_text(&css(".t { background-clip: text; }")).is_empty());
        assert!(
            gradient_text(&css(
                ".t { background-clip: text; background-color: transparent; }"
            ))
            .is_empty()
        );
        assert!(gradient_text(&css(".t { -webkit-text-fill-color: transparent; }")).is_empty());
        assert!(
            gradient_text(&css(
                ".a { background-clip: text; } .b { color: transparent; }"
            ))
            .is_empty()
        );
        assert!(
            gradient_text(&css(
                ".t { background-clip: border-box; color: transparent; }"
            ))
            .is_empty()
        );
        let clip_only = classes("bg-clip-text text-black", "bg-clip-text text-black");
        assert!(gradient_text(&clip_only).is_empty());
    }

    #[test]
    fn the_untouched_shadcn_primary_is_flagged() {
        for value in [
            "oklch(0.205 0 0)",
            "222.2 47.4% 11.2%",
            "240 5.9% 10%",
            "0 0% 9%",
        ] {
            let doc = format!(":root {{ --background: 0 0% 100%; --primary: {value}; }}");
            let hits = shadcn_default(&css(&doc));
            assert_eq!(
                seen(&doc, &hits),
                [format!("--primary: {value}")],
                "{value}"
            );
            assert_eq!(hits[0].rule, "ui.shadcn-default");
        }
    }

    #[test]
    fn a_themed_primary_is_not_the_shadcn_default() {
        for doc in [
            ":root { --primary: oklch(0.45 0.2 30); }",
            ":root { --primary: 0 0% 90%; }",
            ":root { --primary: 240 5.9% 10.5%; }",
            ":root { --sidebar-primary: oklch(0.205 0 0); }",
            ":root { --primary-foreground: 0 0% 9%; }",
        ] {
            assert!(shadcn_default(&css(doc)).is_empty(), "{doc}");
        }
    }

    #[test]
    fn a_big_blurred_round_box_is_a_glow_orb() {
        let doc = ".orb { position: absolute; border-radius: 50%; filter: blur(96px); }";
        let hits = glow_orb(&css(doc));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule, "ui.glow-orb");
        assert_eq!(
            part_texts(doc, &hits[0]),
            ["border-radius: 50%", "filter: blur(96px)"]
        );
        assert_eq!(seen(doc, &hits), ["border-radius: 50%"]);
        for doc in [
            ".o { border-radius: 100%; -webkit-filter: blur(24px); }",
            ".o { border-radius: 9999px; filter: blur( 120px ); }",
        ] {
            assert_eq!(glow_orb(&css(doc)).len(), 1, "{doc}");
        }
    }

    #[test]
    fn a_small_blur_a_backdrop_blur_or_a_square_box_is_not_an_orb() {
        for doc in [
            ".o { border-radius: 50%; filter: blur(8px); }",
            ".o { border-radius: 50%; filter: blur(23px); }",
            ".o { border-radius: 50%; backdrop-filter: blur(96px); }",
            ".o { border-radius: 8px; filter: blur(96px); }",
            ".o { filter: blur(96px); }",
            ".a { border-radius: 50%; } .b { filter: blur(96px); }",
        ] {
            assert!(glow_orb(&css(doc)).is_empty(), "{doc}");
        }
    }

    #[test]
    fn tailwind_orb_classes_are_flagged() {
        for value in [
            "absolute -top-20 rounded-full bg-sky-400 blur-3xl",
            "fixed rounded-full blur-2xl",
            "rounded-full blur-[120px] -z-10",
            "md:blur-3xl rounded-full absolute",
        ] {
            let doc = format!(r#"<div class="{value}">"#);
            assert_eq!(
                seen(&doc, &glow_orb(&classes(&doc, value))),
                [value],
                "{value}"
            );
        }
        for value in [
            "rounded-full blur-3xl",
            "absolute rounded-full backdrop-blur-3xl",
            "absolute rounded-full blur-sm",
            "absolute blur-3xl rounded-lg",
            "absolute rounded-full blur-[8px]",
        ] {
            assert!(glow_orb(&classes(value, value)).is_empty(), "{value}");
        }
    }

    #[test]
    fn a_stock_effect_component_is_flagged() {
        let doc = "export const B = () => <ShimmerButton>Go</ShimmerButton>;";
        let hits = effect_library(&View::plain(doc));
        assert_eq!(seen(doc, &hits), ["<ShimmerButton"]);
        assert_eq!(hits[0].rule, "ui.effect-library");
        let import = r#"import { Marquee } from "@/components/magicui/marquee";"#;
        assert_eq!(
            seen(import, &effect_library(&View::plain(import))),
            ["components/magicui/"]
        );
        let aceternity = r#"import { X } from "@/components/aceternity/x";"#;
        assert_eq!(effect_library(&View::plain(aceternity)).len(), 1);
        for name in [
            "Spotlight",
            "BackgroundBeams",
            "Meteors",
            "GlowingEffect",
            "FlipWords",
        ] {
            let doc = format!("<{name} className=\"a\" />");
            assert_eq!(effect_library(&View::plain(&doc)).len(), 1, "{name}");
        }
    }

    #[test]
    fn an_ordinary_component_or_a_masked_comment_is_not_an_effect_library() {
        for doc in [
            "<Marquee>x</Marquee>",
            "<ShimmerButtonGroup />",
            "<Spotlights />",
            "<spotlight>",
            "components/ui/button",
        ] {
            assert!(effect_library(&View::plain(doc)).is_empty(), "{doc}");
        }
        let doc = "// <ShimmerButton>";
        let view = View {
            code: doc.chars().map(|_| char::from(MASK)).collect(),
            ..View::plain(doc)
        };
        assert!(effect_library(&view).is_empty());
    }

    #[test]
    fn placeholder_image_services_are_flagged() {
        let doc = r#"<img src="https://i.pravatar.cc/150?img=3" alt="Ana">"#;
        let hits = placeholder_media(&View::plain(doc));
        assert_eq!(seen(doc, &hits), ["i.pravatar.cc"]);
        assert_eq!(hits[0].rule, "ui.placeholder-media");
        for host in [
            "api.dicebear.com/7.x/avataaars/svg",
            "ui-avatars.com/api/?name=A",
            "robohash.org/x",
            "randomuser.me/api/portraits/men/1.jpg",
            "placehold.co/600x400",
            "via.placeholder.com/150",
            "picsum.photos/200",
            "source.unsplash.com/random/800x600",
        ] {
            let doc = format!("background: url(https://{host});");
            assert_eq!(placeholder_media(&View::plain(&doc)).len(), 1, "{host}");
        }
    }

    #[test]
    fn a_real_image_host_is_not_a_placeholder() {
        for doc in [
            r#"<img src="https://example.com/a.png">"#,
            "images.unsplash.com/photo-1",
            "randomuser.me/about",
            "spicsum.photos",
        ] {
            assert!(placeholder_media(&View::plain(doc)).is_empty(), "{doc}");
        }
    }

    #[test]
    fn placeholder_copy_is_flagged_only_in_text() {
        let doc = "<p>Lorem ipsum dolor sit amet.</p>";
        let hits = placeholder_text(&View::plain(doc));
        assert_eq!(seen(doc, &hits), ["Lorem ipsum"]);
        assert_eq!(hits[0].rule, "ui.placeholder-text");
        assert_eq!(hits[0].excerpt, "Lorem ipsum");
        for doc in [
            "<h1>[Company Name]</h1>",
            "<p>Welcome to [your brand name].</p>",
            "<p>[insert testimonial]</p>",
            "<p>LOREM\nIPSUM</p>",
        ] {
            assert_eq!(placeholder_text(&View::plain(doc)).len(), 1, "{doc}");
        }
        assert!(placeholder_text(&code_only("const s = 'Lorem ipsum';")).is_empty());
        assert!(
            placeholder_text(&View::plain(
                "<p>The names are listed in the company brief.</p>"
            ))
            .is_empty()
        );
    }

    #[test]
    fn a_row_of_five_stars_is_flagged() {
        let doc = "<span>★★★★★</span>";
        let hits = five_star_row(&View::plain(doc));
        assert_eq!(seen(doc, &hits), ["★★★★★"]);
        assert_eq!(hits[0].rule, "ui.five-star-row");
        let icons = "<div><Star /><Star /><StarIcon className=\"a\" /><Star /><Star /></div>";
        assert_eq!(five_star_row(&View::plain(icons)).len(), 1);
        for doc in [
            "{[...Array(5)].map((_, i) => <Star key={i} />)}",
            "{Array.from({ length: 5 }).map((_, i) => <StarIcon key={i} />)}",
            "{Array(5).fill(0).map((_, i) => <span key={i}>★</span>)}",
        ] {
            assert_eq!(five_star_row(&View::plain(doc)).len(), 1, "{doc}");
        }
    }

    #[test]
    fn a_partial_star_row_is_not_flagged() {
        for doc in [
            "<span>★★★★☆</span>",
            "<div><Star /><Star /><Star /><Star /></div>",
            "{[...Array(4)].map((_, i) => <Star key={i} />)}",
            "{[...Array(5)].map((_, i) => <Dot key={i} />)}",
        ] {
            assert!(five_star_row(&View::plain(doc)).is_empty(), "{doc}");
        }
    }

    #[test]
    fn a_generator_signature_is_flagged() {
        let doc = r#"import { componentTagger } from "lovable-tagger";"#;
        let hits = generator_signature(&View::plain(doc));
        assert_eq!(seen(doc, &hits), ["lovable-tagger"]);
        assert_eq!(hits[0].rule, "ui.generator-signature");
        for doc in [
            r#"<meta name="generator" content="v0">"#,
            r#"<meta name='generator' content='Lovable'>"#,
            r#"<script src="https://cdn.gptengineer.app/x.js"></script>"#,
            r#"<a href="https://v0.dev/t/abc">made with v0.dev</a>"#,
            r#"<a href="https://bolt.new/~/x">"#,
            r#""gpt-engineer": "1.0""#,
        ] {
            assert!(!generator_signature(&View::plain(doc)).is_empty(), "{doc}");
        }
        for doc in [
            r#"<meta name="generator" content="Hugo 0.120">"#,
            "version v0.1 of the dev kit",
            "the bolt news",
        ] {
            assert!(generator_signature(&View::plain(doc)).is_empty(), "{doc}");
        }
    }

    #[test]
    fn the_engine_runs_the_ui_rules_on_a_stylesheet() {
        let doc = ".hero { background: linear-gradient(135deg, #6366f1, #a855f7); }\n.ok { color: #111; }";
        let found = crate::check_all(crate::Kind::Css, doc);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].rule, found[0].line), ("ui.purple-gradient", 1));
        assert!(crate::check_all(crate::Kind::Css, ".a { color: #111; }").is_empty());
    }
}
