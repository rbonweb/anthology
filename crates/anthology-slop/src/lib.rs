//! The slop gate (plan docs/superpowers/plans/2026-11-28-anti-slop-gate.md): masking, rules,
//! Edit replay and messages. The crate reads no environment; `anthology-cli` does.
//!
//! The interface here is frozen by Task 1 of the plan. `mask` builds a `View` of a document
//! (prose, code, style and class regions, waivers), each rule in `rules` reads the view, and
//! `check_with` keeps the hits that touch new text and applies the waivers.
#![forbid(unsafe_code)]

pub mod edit;
pub mod paths;
pub mod report;

mod mask;
mod rules;

pub use mask::{MASK, View, Waiver};
pub use rules::RULES;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use mask::common::{IndexScope, LineIndex, index_for, scoped_index};

/// The largest document checked; a larger one is allowed unchecked.
pub const MAX_DOC: usize = 1 << 20;

/// The file kinds the gate checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Markdown,
    Html,
    Jsx,
    Css,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Markdown => "markdown",
            Kind::Html => "html",
            Kind::Jsx => "jsx",
            Kind::Css => "css",
        }
    }
}

/// The kind of `path` from its extension (case-insensitive), or None for a file not checked.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => Some(Kind::Markdown),
        "html" | "htm" => Some(Kind::Html),
        "tsx" | "jsx" => Some(Kind::Jsx),
        "css" | "scss" => Some(Kind::Css),
        _ => None,
    }
}

/// Block: one hit refuses. Density: only the new text is counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Block,
    Density,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Block => "block",
            Severity::Density => "density",
        }
    }
}

/// One rule of "The v1 rules".
pub struct Rule {
    pub id: &'static str,
    pub severity: Severity,
    pub kinds: &'static [Kind],
    pub summary: &'static str,
    pub hint: &'static str,
    pub(crate) run: rules::Run,
}

/// The rule with this id.
pub fn rule(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// A byte range `[start, end)` of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn intersects(self, other: Span) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// What a rule fn found: `span` places it, `parts` (when not empty) are the pieces that must
/// touch new text, and `excerpt` is what the messages show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rule: &'static str,
    pub span: Span,
    pub parts: Vec<Span>,
    pub excerpt: String,
}

/// A hit kept by the engine. `waiver` is the reason of the waiver that covers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub line: usize,
    pub excerpt: String,
    pub waiver: Option<String>,
}

/// The findings of every rule for `kind` that touch `new`.
pub fn check(kind: Kind, doc: &str, new: &[Span]) -> Vec<Finding> {
    check_with(kind, doc, new, RULES)
}

/// `check` with the whole document as new text.
pub fn check_all(kind: Kind, doc: &str) -> Vec<Finding> {
    check(
        kind,
        doc,
        &[Span {
            start: 0,
            end: doc.len(),
        }],
    )
}

/// The waiver comments and region starts on lines touching `new`.
pub fn waivers_in(kind: Kind, doc: &str, new: &[Span]) -> Vec<Waiver> {
    let _scope = IndexScope::enter(doc);
    touching(doc, &mask::view(kind, doc).waivers, new)
}

/// The 1-based line of byte `at`. One lookup scans the document up to `at`; the engine looks
/// lines up in a `LineIndex` instead.
pub fn line_of(doc: &str, at: usize) -> usize {
    if let Some(index) = scoped_index(doc) {
        return index.line_of(at);
    }
    let bytes = doc.as_bytes();
    bytes[..at.min(bytes.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// `span` of `doc` with whitespace collapsed and MASK bytes dropped; over 60 characters, the
/// first 57 and "...".
pub fn excerpt(doc: &str, span: Span) -> String {
    let bytes = doc.as_bytes();
    let end = span.end.min(bytes.len());
    let start = span.start.min(end);
    let raw = String::from_utf8_lossy(&bytes[start..end]);
    let kept: String = raw.chars().filter(|&c| c != char::from(MASK)).collect();
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 60 {
        let head: String = collapsed.chars().take(57).collect();
        format!("{head}...")
    } else {
        collapsed
    }
}

pub(crate) fn check_with(kind: Kind, doc: &str, new: &[Span], rules: &[Rule]) -> Vec<Finding> {
    let _scope = IndexScope::enter(doc);
    let view = mask::view(kind, doc);
    findings(kind, doc, &view, new, rules)
}

/// The new text as sorted, disjoint spans, so "does this span touch new text" is a binary
/// search and not a scan of every new span. Empty spans hold no text and touch nothing.
struct NewText(Vec<Span>);

impl NewText {
    fn new(new: &[Span]) -> NewText {
        let mut spans: Vec<Span> = new.iter().copied().filter(|s| s.start < s.end).collect();
        spans.sort();
        let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
        for span in spans {
            match merged.last_mut() {
                Some(last) if span.start < last.end => last.end = last.end.max(span.end),
                _ => merged.push(span),
            }
        }
        NewText(merged)
    }

    /// Whether `span` intersects any new span (`Span::intersects`).
    fn touches(&self, span: Span) -> bool {
        let first = self.0.partition_point(|n| n.end <= span.start);
        self.0.get(first).is_some_and(|n| n.start < span.end)
    }
}

/// The engine after masking, so tests can hand it a view with waivers.
fn findings(kind: Kind, doc: &str, view: &View, new: &[Span], rules: &[Rule]) -> Vec<Finding> {
    let index = index_for(doc);
    let new_text = NewText::new(new);
    let new_waivers = touching_new(&index, &view.waivers, &new_text);
    // The first valid waiver of each rule on a line of new text (for density rules), and the
    // first valid waiver of each rule on each line (for block rules), in document order.
    let mut density_waivers: HashMap<&'static str, &Waiver> = HashMap::new();
    for w in new_waivers.iter().filter(|w| w.valid) {
        if let Some(id) = w.rule {
            density_waivers.entry(id).or_insert(w);
        }
    }
    let mut line_waivers: BTreeMap<(&'static str, usize), &Waiver> = BTreeMap::new();
    for w in view.waivers.iter().filter(|w| w.valid) {
        if let Some(id) = w.rule {
            line_waivers.entry((id, w.line)).or_insert(w);
        }
    }
    let mut out = Vec::new();
    for rule in rules.iter().filter(|r| r.kinds.contains(&kind)) {
        let (hits, density) = match rule.run {
            rules::Run::Text(f) => (f(view), false),
            rules::Run::Density(f) => (f(view, new), true),
        };
        for hit in hits {
            let parts = if hit.parts.is_empty() {
                std::slice::from_ref(&hit.span)
            } else {
                &hit.parts[..]
            };
            if !density && !parts.iter().any(|p| new_text.touches(*p)) {
                continue;
            }
            let line = index.line_of(hit.span.start);
            let waiver = if density {
                density_waivers.get(hit.rule).copied()
            } else {
                // A waiver on any line the hit spans covers it, so one on the last line of a
                // wrapped phrase counts.
                let last = index.line_of(hit.span.end.saturating_sub(1).max(hit.span.start));
                line_waivers
                    .range((hit.rule, line)..=(hit.rule, last))
                    .next()
                    .map(|(_, w)| *w)
            };
            out.push(Finding {
                rule: hit.rule,
                line,
                excerpt: hit.excerpt,
                waiver: waiver.map(|w| w.reason.clone()),
            });
        }
    }
    out.sort_by(|a, b| (a.line, a.rule).cmp(&(b.line, b.rule)));
    out.dedup_by(|b, a| (a.line, a.rule) == (b.line, b.rule));
    out
}

/// The waivers whose line touches one of `new`.
fn touching(doc: &str, waivers: &[Waiver], new: &[Span]) -> Vec<Waiver> {
    touching_new(&index_for(doc), waivers, &NewText::new(new))
}

fn touching_new(index: &LineIndex, waivers: &[Waiver], new: &NewText) -> Vec<Waiver> {
    waivers
        .iter()
        .filter(|w| new.touches(index.line_span(w.line)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rules::Run;

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    fn hit(rule: &'static str, span: Span, parts: Vec<Span>) -> Hit {
        Hit {
            rule,
            span,
            parts,
            excerpt: format!("{}-{}", span.start, span.end),
        }
    }

    fn test_rule(id: &'static str, severity: Severity, run: Run) -> Rule {
        Rule {
            id,
            severity,
            kinds: &[Kind::Markdown],
            summary: "test",
            hint: "test",
            run,
        }
    }

    fn finding(rule: &'static str, line: usize, excerpt: &str, waiver: Option<&str>) -> Finding {
        Finding {
            rule,
            line,
            excerpt: excerpt.to_string(),
            waiver: waiver.map(str::to_string),
        }
    }

    // "word\nxxxx\nword tail": hits on line 1 [0, 4) and line 3 [10, 14).
    const TWO_LINES: &str = "word\nxxxx\nword tail";

    fn two_hits(_: &View) -> Vec<Hit> {
        vec![
            hit("text.ai-vocab", sp(0, 4), vec![]),
            hit("text.ai-vocab", sp(10, 14), vec![]),
        ]
    }

    fn parted(_: &View) -> Vec<Hit> {
        vec![hit("text.ai-vocab", sp(0, 4), vec![sp(0, 4), sp(20, 24)])]
    }

    fn first_word(_: &View) -> Vec<Hit> {
        vec![hit("text.ai-vocab", sp(0, 1), vec![])]
    }

    fn dense(_: &View, _: &[Span]) -> Vec<Hit> {
        vec![hit("text.em-dash-density", sp(0, 4), vec![])]
    }

    fn unordered(_: &View) -> Vec<Hit> {
        vec![
            hit("text.ai-vocab", sp(10, 14), vec![]),
            hit("text.stock-phrase", sp(0, 4), vec![]),
            hit("text.ai-vocab", sp(1, 3), vec![]),
            hit("text.ai-vocab", sp(0, 4), vec![]),
        ]
    }

    #[test]
    fn rules_are_the_sixteen_v1_rules() {
        let ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            [
                "text.ai-vocab",
                "text.stock-phrase",
                "text.chat-residue",
                "text.not-x-but-y",
                "text.hype-cta",
                "text.em-dash-density",
                "text.triad-density",
                "ui.purple-gradient",
                "ui.gradient-text",
                "ui.shadcn-default",
                "ui.glow-orb",
                "ui.effect-library",
                "ui.placeholder-media",
                "ui.placeholder-text",
                "ui.five-star-row",
                "ui.generator-signature"
            ]
        );
        assert_eq!(
            RULES
                .iter()
                .filter(|r| r.severity == Severity::Density)
                .count(),
            2
        );
        assert!(
            RULES
                .iter()
                .all(|r| !r.kinds.is_empty() && !r.hint.is_empty())
        );
    }

    #[test]
    fn density_rules_run_as_density_and_the_rest_as_text() {
        for r in RULES {
            let density = matches!(r.run, Run::Density(_));
            assert_eq!(density, r.severity == Severity::Density, "{}", r.id);
        }
    }

    #[test]
    fn rule_finds_a_known_id_only() {
        assert_eq!(rule("ui.glow-orb").map(|r| r.id), Some("ui.glow-orb"));
        assert!(rule("ui.no-such").is_none());
    }

    #[test]
    fn kind_of_maps_extensions() {
        for (path, want) in [
            ("a.md", Some(Kind::Markdown)),
            ("a.MDX", Some(Kind::Markdown)),
            ("a.markdown", Some(Kind::Markdown)),
            ("docs/a.Md", Some(Kind::Markdown)),
            ("a.html", Some(Kind::Html)),
            ("a.htm", Some(Kind::Html)),
            ("a.tsx", Some(Kind::Jsx)),
            ("a.jsx", Some(Kind::Jsx)),
            ("a.css", Some(Kind::Css)),
            ("a.scss", Some(Kind::Css)),
            ("a.ts", None),
            ("a.rs", None),
            ("a.txt", None),
            ("md", None),
        ] {
            assert_eq!(kind_of(Path::new(path)), want, "{path}");
        }
    }

    #[test]
    fn kind_and_severity_names() {
        let names: Vec<&str> = [Kind::Markdown, Kind::Html, Kind::Jsx, Kind::Css]
            .iter()
            .map(|k| k.name())
            .collect();
        assert_eq!(names, ["markdown", "html", "jsx", "css"]);
        assert_eq!(Severity::Block.name(), "block");
        assert_eq!(Severity::Density.name(), "density");
    }

    #[test]
    fn spans_intersect_only_when_they_overlap() {
        assert!(sp(0, 4).intersects(sp(3, 5)));
        assert!(sp(3, 5).intersects(sp(0, 4)));
        assert!(!sp(0, 4).intersects(sp(4, 6)));
        assert!(!sp(4, 6).intersects(sp(0, 4)));
    }

    #[test]
    fn check_with_keeps_block_hits_that_touch_new_text() {
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(two_hits),
        )];
        let got = check_with(Kind::Markdown, TWO_LINES, &[sp(9, 12)], &rules);
        assert_eq!(got, [finding("text.ai-vocab", 3, "10-14", None)]);
    }

    #[test]
    fn a_rule_runs_only_for_its_kinds() {
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(two_hits),
        )];
        assert!(check_with(Kind::Css, TWO_LINES, &[sp(0, 19)], &rules).is_empty());
    }

    #[test]
    fn a_hit_counts_when_any_part_touches_new_text() {
        let doc = "x".repeat(30);
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(parted),
        )];
        let got = check_with(Kind::Markdown, &doc, &[sp(21, 22)], &rules);
        assert_eq!(got, [finding("text.ai-vocab", 1, "0-4", None)]);
        // the span itself does not count once parts are given
        let got = check_with(Kind::Markdown, &doc, &[sp(5, 6)], &rules);
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn density_hits_are_always_kept() {
        let rules = [test_rule(
            "text.em-dash-density",
            Severity::Density,
            Run::Density(dense),
        )];
        let got = check_with(Kind::Markdown, TWO_LINES, &[sp(15, 19)], &rules);
        assert_eq!(got, [finding("text.em-dash-density", 1, "0-4", None)]);
    }

    fn with_comment_waivers(doc: &str) -> View {
        let start = doc.find("<!--").unwrap();
        let end = doc.find("-->").unwrap() + 3;
        View {
            waivers: mask::common::comment_waivers(doc, sp(start, end)),
            ..View::plain(doc)
        }
    }

    #[test]
    fn a_valid_line_waiver_waives_and_keeps_the_reason() {
        let doc = "x <!-- slop-ok: text.ai-vocab named by the user -->";
        let view = with_comment_waivers(doc);
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(first_word),
        )];
        let got = findings(Kind::Markdown, doc, &view, &[sp(0, doc.len())], &rules);
        assert_eq!(
            got,
            [finding(
                "text.ai-vocab",
                1,
                "0-1",
                Some("named by the user")
            )]
        );
    }

    #[test]
    fn a_line_waiver_covers_only_its_own_line_and_rule() {
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(first_word),
        )];
        let doc = "x\n<!-- slop-ok: text.ai-vocab named by the user -->";
        let view = with_comment_waivers(doc);
        let got = findings(Kind::Markdown, doc, &view, &[sp(0, doc.len())], &rules);
        assert_eq!(got, [finding("text.ai-vocab", 1, "0-1", None)]);
        let doc = "x <!-- slop-ok: text.hype-cta named by the user -->";
        let view = with_comment_waivers(doc);
        let got = findings(Kind::Markdown, doc, &view, &[sp(0, doc.len())], &rules);
        assert_eq!(got, [finding("text.ai-vocab", 1, "0-1", None)]);
    }

    #[test]
    fn a_waiver_without_a_reason_or_with_an_unknown_rule_does_not_waive() {
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(first_word),
        )];
        for doc in [
            "x <!-- slop-ok: text.ai-vocab -->",
            "x <!-- slop-ok: text.ai-vocab short -->",
            "x <!-- slop-ok: text.no-such named by the user -->",
        ] {
            let view = with_comment_waivers(doc);
            let got = findings(Kind::Markdown, doc, &view, &[sp(0, doc.len())], &rules);
            assert_eq!(got, [finding("text.ai-vocab", 1, "0-1", None)], "{doc}");
        }
    }

    #[test]
    fn a_density_hit_is_waived_by_a_waiver_on_a_line_of_new_text() {
        let rules = [test_rule(
            "text.em-dash-density",
            Severity::Density,
            Run::Density(dense),
        )];
        let doc = "one\ntwo\n<!-- slop-ok: text.em-dash-density quoted from the brief -->";
        let view = with_comment_waivers(doc);
        let got = findings(Kind::Markdown, doc, &view, &[sp(8, doc.len())], &rules);
        assert_eq!(
            got,
            [finding(
                "text.em-dash-density",
                1,
                "0-4",
                Some("quoted from the brief")
            )]
        );
        // the waiver line is not new text: no waiver
        let got = findings(Kind::Markdown, doc, &view, &[sp(4, 7)], &rules);
        assert_eq!(got, [finding("text.em-dash-density", 1, "0-4", None)]);
    }

    #[test]
    fn findings_are_sorted_and_deduplicated() {
        let rules = [test_rule(
            "text.ai-vocab",
            Severity::Block,
            Run::Text(unordered),
        )];
        let got = check_with(Kind::Markdown, TWO_LINES, &[sp(0, 19)], &rules);
        assert_eq!(
            got,
            [
                finding("text.ai-vocab", 1, "1-3", None),
                finding("text.stock-phrase", 1, "0-4", None),
                finding("text.ai-vocab", 3, "10-14", None),
            ]
        );
    }

    #[test]
    fn waivers_on_lines_touching_new_text_are_returned() {
        let doc = "a <!-- slop-ok: text.ai-vocab named by the user -->\nb\nc";
        let waivers = with_comment_waivers(doc).waivers;
        assert_eq!(waivers.len(), 1);
        assert_eq!(touching(doc, &waivers, &[sp(0, 1)]), waivers);
        assert!(touching(doc, &waivers, &[sp(52, 53)]).is_empty());
        assert!(touching(doc, &waivers, &[]).is_empty());
    }

    #[test]
    fn line_of_counts_from_one() {
        assert_eq!(line_of("a\nb\nc", 0), 1);
        assert_eq!(line_of("a\nb\nc", 4), 3);
        assert_eq!(line_of("a\nb\nc", 1), 1);
        assert_eq!(line_of("a\nb\nc", 2), 2);
        assert_eq!(line_of("a\nb\nc", 99), 3);
    }

    fn two_line_hit(_: &View) -> Vec<Hit> {
        vec![hit("text.stock-phrase", sp(0, 8), vec![])]
    }

    #[test]
    fn a_waiver_on_any_line_a_hit_spans_waives_it() {
        let rules = [test_rule(
            "text.stock-phrase",
            Severity::Block,
            Run::Text(two_line_hit),
        )];
        // the hit [0, 8) runs over lines 1 and 2; the waiver sits on line 2
        let doc = "ab\ncd ef <!-- slop-ok: text.stock-phrase quoting the brief -->";
        let view = with_comment_waivers(doc);
        let got = findings(Kind::Markdown, doc, &view, &[sp(0, doc.len())], &rules);
        assert_eq!(
            got,
            [finding(
                "text.stock-phrase",
                1,
                "0-8",
                Some("quoting the brief")
            )]
        );
        // a waiver one line past the hit does not count
        let doc = "ab\ncd ef\n<!-- slop-ok: text.stock-phrase quoting the brief -->";
        let view = with_comment_waivers(doc);
        let got = findings(Kind::Markdown, doc, &view, &[sp(0, doc.len())], &rules);
        assert_eq!(got, [finding("text.stock-phrase", 1, "0-8", None)]);
    }

    #[test]
    fn new_text_matches_span_intersects_for_unsorted_overlapping_spans() {
        let new = [
            sp(20, 30),
            sp(0, 5),
            sp(3, 8),
            sp(8, 10),
            sp(12, 12),
            sp(40, 41),
        ];
        let text = NewText::new(&new);
        for start in 0..45 {
            for end in start..46 {
                let probe = sp(start, end);
                let want = new.iter().any(|n| n.start < n.end && probe.intersects(*n));
                assert_eq!(text.touches(probe), want, "{probe:?}");
            }
        }
    }

    #[test]
    fn a_waiver_on_the_last_line_of_a_wrapped_phrase_waives_it() {
        let doc = "Gone are\nthe days <!-- slop-ok: text.stock-phrase quoted from the brief -->\n";
        let got = check_all(Kind::Markdown, doc);
        assert_eq!(
            got,
            [finding(
                "text.stock-phrase",
                1,
                "Gone are the days",
                Some("quoted from the brief")
            )]
        );
        // with no waiver the same phrase is still a finding
        let got = check_all(Kind::Markdown, "Gone are\nthe days\n");
        assert_eq!(
            got,
            [finding("text.stock-phrase", 1, "Gone are the days", None)]
        );
    }

    /// The engine must scale with the size of the document, not with hits times size.
    const TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

    #[test]
    fn a_megabyte_with_170k_hits_is_checked_in_time() {
        let doc = "delve\n".repeat(170_000);
        assert!(doc.len() < MAX_DOC);
        let start = std::time::Instant::now();
        let got = check_all(Kind::Markdown, &doc);
        let took = start.elapsed();
        assert_eq!(got.len(), 170_000);
        assert_eq!(got[169_999].line, 170_000);
        assert!(took < TIME_LIMIT, "took {took:?}");
    }

    #[test]
    fn eighteen_thousand_waiver_lines_are_checked_in_time() {
        let line = "delve <!-- slop-ok: text.ai-vocab named by the user -->\n";
        let doc = line.repeat(18_000);
        assert!(doc.len() < MAX_DOC);
        let start = std::time::Instant::now();
        let got = check_all(Kind::Markdown, &doc);
        let waivers = waivers_in(Kind::Markdown, &doc, &[sp(0, doc.len())]);
        let took = start.elapsed();
        assert_eq!(got.len(), 18_000);
        assert!(
            got.iter()
                .all(|f| f.waiver.as_deref() == Some("named by the user"))
        );
        assert_eq!(waivers.len(), 18_000);
        assert!(took < TIME_LIMIT, "took {took:?}");
    }

    #[test]
    fn many_new_spans_are_checked_in_time() {
        // every other line is new: 85k new spans against 170k hits
        let doc = "delve\n".repeat(170_000);
        let new: Vec<Span> = (0..170_000)
            .step_by(2)
            .map(|i| sp(i * 6, i * 6 + 6))
            .collect();
        let start = std::time::Instant::now();
        let got = check(Kind::Markdown, &doc, &new);
        let took = start.elapsed();
        assert_eq!(got.len(), 85_000);
        assert!(took < TIME_LIMIT, "took {took:?}");
    }

    #[test]
    fn excerpt_collapses_whitespace_and_caps_at_sixty() {
        let doc = "  a \n\t b\u{1}c  d ";
        assert_eq!(excerpt(doc, sp(0, doc.len())), "a bc d");
        assert_eq!(excerpt("hello world", sp(6, 11)), "world");
        let long = "x".repeat(61);
        assert_eq!(excerpt(&long, sp(0, 61)), format!("{}...", "x".repeat(57)));
        let sixty = "x".repeat(60);
        assert_eq!(excerpt(&sixty, sp(0, 60)), sixty);
        let wide = "é".repeat(70);
        assert_eq!(
            excerpt(&wide, sp(0, wide.len())),
            format!("{}...", "é".repeat(57))
        );
    }
}
