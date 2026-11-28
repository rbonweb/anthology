//! The gate's messages: the refusal the agent reads, the lines `slop check` prints, and the
//! rules table.

use crate::{Finding, Kind, Waiver};
use std::fmt::Write;

/// The most findings a block message lists.
pub const MAX_SHOWN: usize = 20;

/// What a refusal tells the agent to do, after the list of tells, in the comment syntax of `kind`.
/// The checker reads `<!-- -->` in Markdown and HTML, `{/* */}` in JSX and `/* */` in CSS.
fn advice(kind: Kind) -> String {
    let (specimen, start, end, waiver) = match kind {
        Kind::Markdown | Kind::Html => (
            "in a code span or between",
            "<!-- slop-ignore-start -->",
            "<!-- slop-ignore-end -->",
            "<!-- slop-ok: <rule> <reason> -->",
        ),
        Kind::Jsx => (
            "between",
            "{/* slop-ignore-start */}",
            "{/* slop-ignore-end */}",
            "{/* slop-ok: <rule> <reason> */}",
        ),
        Kind::Css => (
            "between",
            "/* slop-ignore-start */",
            "/* slop-ignore-end */",
            "/* slop-ok: <rule> <reason> */",
        ),
    };
    format!(
        "Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen {specimen} {start} and {end}. Add {waiver} only for wording the user asked for word for word; the lead is told about each one.\n"
    )
}

/// The stderr message of a refused edit to a file of `kind`. `findings` holds only unwaived
/// findings; at most `MAX_SHOWN` are listed, then the count of the rest. The closing advice shows
/// the waiver and ignore-region syntax of `kind`. The message ends with a newline.
pub fn hook_message(tool: &str, shown: &str, kind: Kind, findings: &[Finding]) -> String {
    let mut out = format!(
        "anthology slop gate: the {tool} of {shown} was not applied. Its new text has these AI tells:\n"
    );
    for f in findings.iter().take(MAX_SHOWN) {
        let _ = write!(out, "  line {}: {} \"{}\"", f.line, f.rule, f.excerpt);
        if let Some(rule) = crate::rule(f.rule) {
            let _ = write!(out, ": {}", rule.hint);
        }
        out.push('\n');
    }
    if findings.len() > MAX_SHOWN {
        let _ = writeln!(out, "  and {} more", findings.len() - MAX_SHOWN);
    }
    out.push_str(&advice(kind));
    out
}

/// `{shown}:{line}: {rule}: "{excerpt}"`.
pub fn check_line(shown: &str, f: &Finding) -> String {
    format!("{shown}:{}: {}: \"{}\"", f.line, f.rule, f.excerpt)
}

/// `{shown}:{line}: slop-ok {rule}: {reason}` or `{shown}:{line}: slop-ignore region`.
pub fn waiver_line(shown: &str, w: &Waiver) -> String {
    match w.rule {
        Some(rule) => format!("{shown}:{}: slop-ok {rule}: {}", w.line, w.reason),
        None => format!("{shown}:{}: slop-ignore region", w.line),
    }
}

/// One `{id}\t{severity}\t{kinds}\t{summary}` line per rule, kinds joined with commas. Every line,
/// the last too, ends with a newline, so a caller prints the table as it is.
pub fn rules_table() -> String {
    let mut out = String::new();
    for rule in crate::RULES {
        let kinds: Vec<&str> = rule.kinds.iter().map(|k| k.name()).collect();
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}",
            rule.id,
            rule.severity.name(),
            kinds.join(","),
            rule.summary
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;

    fn finding(rule: &'static str, line: usize, excerpt: &str) -> Finding {
        Finding {
            rule,
            line,
            excerpt: excerpt.to_string(),
            waiver: None,
        }
    }

    fn waiver(line: usize, rule: Option<&'static str>, reason: &str) -> Waiver {
        Waiver {
            line,
            rule,
            reason: reason.to_string(),
            valid: true,
        }
    }

    #[test]
    fn hook_message_is_the_exact_copy() {
        let got = hook_message(
            "Write",
            "docs/a.md",
            Kind::Markdown,
            &[
                finding("text.ai-vocab", 3, "delve"),
                finding("text.hype-cta", 7, "to the next level"),
            ],
        );
        let want = [
            "anthology slop gate: the Write of docs/a.md was not applied. Its new text has these AI tells:",
            "  line 3: text.ai-vocab \"delve\": say the concrete thing the word stands for: the step, the number, the part",
            "  line 7: text.hype-cta \"to the next level\": say what happens when the reader acts, with one fact",
            "Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen in a code span or between <!-- slop-ignore-start --> and <!-- slop-ignore-end -->. Add <!-- slop-ok: <rule> <reason> --> only for wording the user asked for word for word; the lead is told about each one.",
            "",
        ]
        .join("\n");
        assert_eq!(got, want);
    }

    #[test]
    fn hook_message_names_the_tool() {
        let got = hook_message(
            "MultiEdit",
            "src/a.html",
            Kind::Html,
            &[finding("ui.glow-orb", 1, "x")],
        );
        assert!(
            got.starts_with("anthology slop gate: the MultiEdit of src/a.html was not applied. Its new text has these AI tells:\n  line 1: ui.glow-orb \"x\": remove the orb"),
            "{got}"
        );
    }

    /// The closing paragraph of a message for `kind`.
    fn advice_for(kind: Kind) -> String {
        let got = hook_message("Edit", "a", kind, &[finding("text.ai-vocab", 1, "delve")]);
        got.lines().last().unwrap().to_string()
    }

    #[test]
    fn markdown_and_html_show_the_html_comment_syntax() {
        let want = "Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen in a code span or between <!-- slop-ignore-start --> and <!-- slop-ignore-end -->. Add <!-- slop-ok: <rule> <reason> --> only for wording the user asked for word for word; the lead is told about each one.";
        assert_eq!(advice_for(Kind::Markdown), want);
        assert_eq!(advice_for(Kind::Html), want);
    }

    #[test]
    fn jsx_shows_the_jsx_comment_syntax() {
        assert_eq!(
            advice_for(Kind::Jsx),
            "Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen between {/* slop-ignore-start */} and {/* slop-ignore-end */}. Add {/* slop-ok: <rule> <reason> */} only for wording the user asked for word for word; the lead is told about each one."
        );
    }

    #[test]
    fn css_shows_the_block_comment_syntax() {
        assert_eq!(
            advice_for(Kind::Css),
            "Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen between /* slop-ignore-start */ and /* slop-ignore-end */. Add /* slop-ok: <rule> <reason> */ only for wording the user asked for word for word; the lead is told about each one."
        );
    }

    #[test]
    fn the_advice_names_no_other_kinds_comment() {
        for kind in [Kind::Jsx, Kind::Css] {
            assert!(!advice_for(kind).contains("<!--"), "{kind:?}");
        }
        for kind in [Kind::Markdown, Kind::Html] {
            assert!(!advice_for(kind).contains("/*"), "{kind:?}");
        }
    }

    #[test]
    fn every_waiver_the_message_shows_is_read_by_the_checker() {
        // the syntax in the advice must be one the engine accepts, for the kind it is shown for
        let cases = [
            (
                Kind::Markdown,
                "A <!-- slop-ok: text.ai-vocab the user named it --> delve here.",
            ),
            (
                Kind::Html,
                "<p>delve <!-- slop-ok: text.ai-vocab the user named it --></p>",
            ),
            (
                Kind::Jsx,
                "const a = <p>delve {/* slop-ok: text.ai-vocab the user named it */}</p>;",
            ),
            (
                Kind::Css,
                "a { top: 0 } /* slop-ok: ui.purple-gradient the user named it */",
            ),
        ];
        for (kind, doc) in cases {
            let waivers = crate::waivers_in(
                kind,
                doc,
                &[Span {
                    start: 0,
                    end: doc.len(),
                }],
            );
            assert_eq!(waivers.len(), 1, "{kind:?}: {waivers:?}");
            assert!(waivers[0].valid, "{kind:?}");
        }
    }

    #[test]
    fn hook_message_caps_at_twenty_and_counts_the_rest() {
        let many = |n: usize| -> Vec<Finding> {
            (1..=n)
                .map(|i| finding("text.ai-vocab", i, "delve"))
                .collect()
        };
        let lines_of = |msg: &str| -> Vec<String> {
            msg.lines()
                .filter(|l| l.starts_with("  line ") || l.starts_with("  and "))
                .map(str::to_string)
                .collect()
        };
        assert_eq!(MAX_SHOWN, 20);

        let got = lines_of(&hook_message("Edit", "a.md", Kind::Markdown, &many(23)));
        assert_eq!(got.len(), 21, "{got:?}");
        assert!(got[19].starts_with("  line 20: "), "{}", got[19]);
        assert_eq!(got[20], "  and 3 more");

        let got = lines_of(&hook_message("Edit", "a.md", Kind::Markdown, &many(21)));
        assert_eq!(got.len(), 21);
        assert_eq!(got[20], "  and 1 more");

        let got = lines_of(&hook_message("Edit", "a.md", Kind::Markdown, &many(20)));
        assert_eq!(got.len(), 20, "{got:?}");
        assert!(got.iter().all(|l| l.starts_with("  line ")));
    }

    #[test]
    fn hook_message_ends_with_one_newline_after_the_closing_paragraph() {
        let got = hook_message(
            "Edit",
            "a.md",
            Kind::Markdown,
            &[finding("text.ai-vocab", 1, "delve")],
        );
        assert!(got.ends_with("the lead is told about each one.\n"), "{got}");
        assert!(!got.ends_with("\n\n"));
    }

    #[test]
    fn an_unknown_rule_shows_no_hint() {
        let got = hook_message(
            "Edit",
            "a.md",
            Kind::Markdown,
            &[finding("text.no-such", 4, "x")],
        );
        assert!(got.contains("\n  line 4: text.no-such \"x\"\n"), "{got}");
    }

    #[test]
    fn check_and_waiver_line_formats() {
        assert_eq!(
            check_line(
                "docs/a.md",
                &finding("text.stock-phrase", 12, "let's dive in")
            ),
            "docs/a.md:12: text.stock-phrase: \"let's dive in\""
        );
        assert_eq!(
            waiver_line(
                "docs/a.md",
                &waiver(5, Some("text.ai-vocab"), "named by the user")
            ),
            "docs/a.md:5: slop-ok text.ai-vocab: named by the user"
        );
        assert_eq!(
            waiver_line("docs/a.md", &waiver(9, None, "specimen")),
            "docs/a.md:9: slop-ignore region"
        );
    }

    #[test]
    fn rules_table_has_one_tab_separated_line_per_rule() {
        let table = rules_table();
        assert!(table.ends_with('\n'));
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), crate::RULES.len());
        for (line, rule) in lines.iter().zip(crate::RULES) {
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(fields.len(), 4, "{line}");
            assert_eq!(fields[0], rule.id);
            assert_eq!(fields[1], rule.severity.name());
            assert_eq!(fields[3], rule.summary);
        }
    }

    #[test]
    fn rules_table_is_the_plans_table() {
        let want = "\
text.ai-vocab\tblock\tmarkdown,html,jsx\tstock AI vocabulary
text.stock-phrase\tblock\tmarkdown,html,jsx\tstock opener or cliche
text.chat-residue\tblock\tmarkdown,html,jsx\tchat-assistant wording in a file
text.not-x-but-y\tblock\tmarkdown,html,jsx\tdenied-alternative contrast
text.hype-cta\tblock\tmarkdown,html,jsx\thype call to action
text.em-dash-density\tdensity\tmarkdown,html\tem dashes: 3 or more, over 1 per 80 words
text.triad-density\tdensity\tmarkdown,html\tlists of three: 3 or more, over 1 per 150 words
ui.purple-gradient\tblock\tcss,html,jsx\tpurple or indigo gradient (C1)
ui.gradient-text\tblock\tcss,html,jsx\tgradient-filled text (C6)
ui.shadcn-default\tblock\tcss,html\tuntouched shadcn/ui base theme (K1)
ui.glow-orb\tblock\tcss,html,jsx\tdecorative blurred orb (C11)
ui.effect-library\tblock\tjsx\tstock effect-library component (K10)
ui.placeholder-media\tblock\tcss,html,jsx\tplaceholder avatar or image service (IM4)
ui.placeholder-text\tblock\thtml,jsx\tplaceholder copy
ui.dummy-contact\tblock\thtml,jsx\tdummy phone number or chat link
ui.five-star-row\tblock\thtml,jsx\tfive-star row (IM5)
ui.generator-signature\tblock\thtml,jsx\tgenerator signature left in (F1)
";
        assert_eq!(rules_table(), want);
    }
}
