//! Hidden text: bidi controls, zero-width and tag characters, whitespace pushes, homoglyph names
//! (Task 4).

use std::sync::OnceLock;

use regex::Regex;

use crate::{FileCtx, FileKind, Finding, Group, Lang, Severity};

/// The text of an excerpt in a finding, before the report escapes and cuts it.
const SEEN_CHARS: usize = 200;

/// A run of at least this many spaces or tabs, after code on the same line, is a push.
const PUSH_MIN_RUN: usize = 150;
/// The run must be followed by at least this many non-space characters.
const PUSH_MIN_TAIL: usize = 10;

// Helpers the payload and exfil rules share. The scan has its own `files::line_of`; these work on
// the text of one file, so the rules need nothing from the walk.

/// The 1-based line of byte offset `at`; a CRLF break counts once.
pub(super) fn line_at(text: &str, at: usize) -> usize {
    let end = at.min(text.len());
    1 + text.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}

/// The line that holds byte offset `at`, cut to `max` characters. A long line (a minified bundle)
/// starts a few characters before `at`, so the match is in view.
pub(super) fn excerpt(text: &str, at: usize, max: usize) -> String {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = text[start..end].trim_end_matches('\r');
    if line.chars().count() <= max {
        return line.to_string();
    }
    let mut from = at.saturating_sub(40).max(start);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    text[from..end]
        .trim_end_matches('\r')
        .chars()
        .take(max)
        .collect()
}

/// A pattern compiled once, on first use. The patterns are fixed in the source, so they compile.
pub(super) fn cached(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a fixed pattern compiles"))
}

/// The script set: the languages a repo runs as it is, with no build step in between.
pub(super) fn is_script(lang: Lang) -> bool {
    matches!(
        lang,
        Lang::Js | Lang::Py | Lang::Sh | Lang::Ps | Lang::Bat | Lang::Rb | Lang::Php | Lang::Pl
    )
}

/// A finding with this file's path, one hit and nothing else set.
pub(super) fn new_finding(f: &FileCtx, group: Group, rule: &str, severity: Severity) -> Finding {
    Finding {
        severity,
        group,
        rule: rule.to_string(),
        path: f.path.to_string(),
        line: 0,
        why: String::new(),
        seen: String::new(),
        action: String::new(),
        count: 1,
    }
}

/// The first offset and the number of hits of one rule in one file.
#[derive(Default)]
pub(super) struct Hits {
    pub(super) first: Option<usize>,
    pub(super) count: usize,
}

impl Hits {
    pub(super) fn add(&mut self, at: usize) {
        self.first.get_or_insert(at);
        self.count += 1;
    }
}

/// What a rule says about its hits.
pub(super) struct Rule<'a> {
    pub(super) group: Group,
    pub(super) id: &'a str,
    pub(super) severity: Severity,
    pub(super) why: &'a str,
    pub(super) action: &'a str,
}

/// One finding for `hits`, or none: the line and the excerpt are those of the first hit.
pub(super) fn report(f: &FileCtx, text: &str, hits: &Hits, rule: &Rule) -> Option<Finding> {
    let at = hits.first?;
    Some(Finding {
        line: line_at(text, at),
        why: rule.why.to_string(),
        seen: excerpt(text, at, SEEN_CHARS),
        action: rule.action.to_string(),
        count: hits.count,
        ..new_finding(f, rule.group, rule.id, rule.severity)
    })
}

const VIEW: &str =
    "View the file with a tool that shows control characters, and remove them before you trust it.";

pub(crate) fn check(f: &FileCtx, out: &mut Vec<Finding>) {
    homoglyph_name(f, out);
    let Some(text) = f.text else { return };
    if !text.is_ascii() {
        invisible_characters(f, text, out);
    }
    if matches!(f.kind, FileKind::Code(_)) && !f.path.ends_with(".min.js") {
        whitespace_push(f, text, out);
    }
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF)
}

/// The scripts whose writing needs a zero-width non-joiner: Arabic and the Indic scripts.
fn needs_non_joiner(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x06FF | 0x0900..=0x0DFF)
}

fn is_variation_selector(c: char) -> bool {
    matches!(c as u32, 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
}

/// A zero-width joiner glues the emoji of a family or a profession; a joiner anywhere else hides.
fn joins_emoji(text: &str, at: usize) -> bool {
    let before = text[..at].chars().rev().find(|c| *c != '\u{FE0F}');
    let after = text[at + '\u{200D}'.len_utf8()..].chars().next();
    before.is_some_and(is_emoji) && after.is_some_and(is_emoji)
}

/// A non-joiner is part of Persian and Indic spelling when a letter of that script is next to it.
fn spells_a_word(text: &str, at: usize) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + '\u{200C}'.len_utf8()..].chars().next();
    before.is_some_and(needs_non_joiner) || after.is_some_and(needs_non_joiner)
}

/// The offset after a flag sequence (a black flag, tag letters, a cancel tag) whose tags start at
/// `from`; `None` when the tags do not end in the cancel tag.
fn flag_sequence_end(text: &str, from: usize) -> Option<usize> {
    let mut letters = 0;
    for (i, c) in text[from..].char_indices() {
        match c as u32 {
            0xE0020..=0xE007E => letters += 1,
            0xE007F if letters > 0 => return Some(from + i + c.len_utf8()),
            _ => return None,
        }
    }
    None
}

fn invisible_characters(f: &FileCtx, text: &str, out: &mut Vec<Finding>) {
    let (mut bidi, mut zero, mut tag, mut variation) = (
        Hits::default(),
        Hits::default(),
        Hits::default(),
        Hits::default(),
    );
    let mut run = 0;
    let mut skip_to = 0;
    for (i, c) in text.char_indices() {
        if i < skip_to {
            continue;
        }
        let u = c as u32;
        if u < 0x200B {
            run = 0;
            continue;
        }
        match u {
            0x202A..=0x202E | 0x2066..=0x2069 => bidi.add(i),
            0x200B | 0x2060 => zero.add(i),
            0xFEFF if i > 0 => zero.add(i),
            0x200D if !joins_emoji(text, i) => zero.add(i),
            0x200C if !spells_a_word(text, i) => zero.add(i),
            0x1F3F4 => {
                if let Some(end) = flag_sequence_end(text, i + c.len_utf8()) {
                    skip_to = end;
                }
            }
            0xE0000..=0xE007F => tag.add(i),
            _ => {}
        }
        if is_variation_selector(c) {
            run += 1;
            if run == 4 {
                variation.add(i);
            }
        } else {
            run = 0;
        }
    }
    let agent = f.kind == FileKind::Agent;
    let code = matches!(f.kind, FileKind::Code(_));
    let bidi_level = if code || agent {
        Severity::High
    } else {
        Severity::Medium
    };
    let zero_level = if agent {
        Severity::High
    } else if code {
        Severity::Medium
    } else {
        Severity::Low
    };
    let tag_level = if agent {
        Severity::Critical
    } else {
        Severity::High
    };
    let rule = |id, severity, why| Rule {
        group: Group::Hidden,
        id,
        severity,
        why,
        action: VIEW,
    };
    let found = [
        report(
            f,
            text,
            &bidi,
            &rule(
                "hidden.bidi",
                bidi_level,
                "Bidirectional control characters change how text is ordered on screen, so what you read is not what a program sees.",
            ),
        ),
        report(
            f,
            text,
            &zero,
            &rule(
                "hidden.zero-width",
                zero_level,
                "Zero-width characters are invisible: they hide text from a reader and split words to get past a scan.",
            ),
        ),
        report(
            f,
            text,
            &tag,
            &rule(
                "hidden.tag-chars",
                tag_level,
                "Unicode tag characters are invisible to a reader, but a language model reads them as text: a hidden instruction.",
            ),
        ),
        report(
            f,
            text,
            &variation,
            &rule(
                "hidden.variation-run",
                Severity::High,
                "A run of variation selectors is invisible and can carry hidden data or code.",
            ),
        ),
    ];
    out.extend(found.into_iter().flatten());
}

/// Code after a long run of blanks on a line that already has code: off screen in most editors.
fn whitespace_push(f: &FileCtx, text: &str, out: &mut Vec<Finding>) {
    let b = text.as_bytes();
    // The first push as (start of the run, end of the run) and how many there are.
    let mut first: Option<(usize, usize)> = None;
    let mut count = 0;
    let mut ink = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => ink = false,
            b' ' | b'\t' => {
                let start = i;
                while i < b.len() && matches!(b[i], b' ' | b'\t') {
                    i += 1;
                }
                let tail = text[i..]
                    .chars()
                    .take(PUSH_MIN_TAIL)
                    .take_while(|c| !c.is_whitespace())
                    .count();
                if ink && i - start >= PUSH_MIN_RUN && tail == PUSH_MIN_TAIL {
                    first.get_or_insert((start, i));
                    count += 1;
                }
                continue;
            }
            b'\r' => {}
            _ => ink = true,
        }
        i += 1;
    }
    let Some((start, end)) = first else { return };
    // What a reader sees, then what the push hides.
    let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let before: Vec<char> = text[line_start..start].chars().rev().take(40).collect();
    let before: String = before.into_iter().rev().collect();
    let after: String = text[end..]
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    out.push(Finding {
        line: line_at(text, start),
        why: "A long run of blanks pushes code past the visible edge of the editor, so a reader sees only the harmless start of the line.".to_string(),
        seen: format!("{before}[{} blanks]{after}", end - start),
        action: "Open the file with line wrapping on and read the whole line before you trust it.".to_string(),
        count,
        ..new_finding(f, Group::Hidden, "hidden.whitespace-push", Severity::High)
    });
}

fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || (matches!(c as u32, 0x00C0..=0x024F) && c.is_alphabetic())
}

fn is_greek_or_cyrillic(c: char) -> bool {
    matches!(c as u32, 0x0370..=0x052F) && c.is_alphabetic()
}

/// A word of the name (a run of letters) with Latin and Cyrillic or Greek letters together. A
/// Russian name with a Latin extension, `notes.md`, is two words and is not a mix.
fn mixes_scripts(component: &str) -> bool {
    component
        .split(|c: char| !c.is_alphabetic())
        .any(|word| word.chars().any(is_latin) && word.chars().any(is_greek_or_cyrillic))
}

/// The Latin letter a Cyrillic or Greek letter is drawn like, if any.
fn look_alike(c: char) -> Option<char> {
    Some(match c {
        // Cyrillic, lower case: a e o p c y x i j s d h l q w v.
        '\u{0430}' => 'a',
        '\u{0435}' => 'e',
        '\u{043E}' => 'o',
        '\u{0440}' => 'p',
        '\u{0441}' => 'c',
        '\u{0443}' => 'y',
        '\u{0445}' => 'x',
        '\u{0456}' => 'i',
        '\u{0458}' => 'j',
        '\u{0455}' => 's',
        '\u{0501}' => 'd',
        '\u{04BB}' => 'h',
        '\u{04CF}' => 'l',
        '\u{051B}' => 'q',
        '\u{051D}' => 'w',
        '\u{0475}' => 'v',
        // Cyrillic, upper case.
        '\u{0410}' => 'A',
        '\u{0412}' => 'B',
        '\u{0415}' => 'E',
        '\u{041A}' => 'K',
        '\u{041C}' => 'M',
        '\u{041D}' => 'H',
        '\u{041E}' => 'O',
        '\u{0420}' => 'P',
        '\u{0421}' => 'C',
        '\u{0422}' => 'T',
        '\u{0425}' => 'X',
        '\u{0406}' => 'I',
        '\u{0408}' => 'J',
        '\u{0405}' => 'S',
        // Greek, lower case.
        '\u{03B1}' => 'a',
        '\u{03B9}' => 'i',
        '\u{03BA}' => 'k',
        '\u{03BD}' => 'v',
        '\u{03BF}' => 'o',
        '\u{03C1}' => 'p',
        '\u{03C5}' => 'u',
        '\u{03C7}' => 'x',
        // Greek, upper case.
        '\u{0391}' => 'A',
        '\u{0392}' => 'B',
        '\u{0395}' => 'E',
        '\u{0396}' => 'Z',
        '\u{0397}' => 'H',
        '\u{0399}' => 'I',
        '\u{039A}' => 'K',
        '\u{039C}' => 'M',
        '\u{039D}' => 'N',
        '\u{039F}' => 'O',
        '\u{03A1}' => 'P',
        '\u{03A4}' => 'T',
        '\u{03A5}' => 'Y',
        '\u{03A7}' => 'X',
        _ => return None,
    })
}

const HOMOGLYPH: &str = "hidden.homoglyph-name";

fn homoglyph_name(f: &FileCtx, out: &mut Vec<Finding>) {
    let Some(name) = f.path.split('/').find(|c| mixes_scripts(c)) else {
        return;
    };
    out.push(Finding {
        why: format!(
            "The name \"{name}\" mixes Latin letters with look-alike Cyrillic or Greek letters, so it can pose as another file."
        ),
        seen: f.path.to_string(),
        action: "Compare the name with the file it may imitate before you trust it.".to_string(),
        ..new_finding(f, Group::Hidden, HOMOGLYPH, Severity::Medium)
    });
}

/// Raises a homoglyph finding to HIGH when the Latin-only twin of its name is in the scan, in the
/// same folder. The orchestrator calls this once, with every path of the tree and the findings.
pub(crate) fn raise_homoglyph_twins(paths: &[&str], findings: &mut [Finding]) {
    for finding in findings.iter_mut().filter(|x| x.rule == HOMOGLYPH) {
        let parts: Vec<&str> = finding.path.split('/').collect();
        let Some(last) = parts.iter().rposition(|c| mixes_scripts(c)) else {
            continue;
        };
        let own = parts[..=last].join("/");
        let twin = parts[..=last]
            .iter()
            .map(|c| {
                if mixes_scripts(c) {
                    c.chars().map(|x| look_alike(x).unwrap_or(x)).collect()
                } else {
                    (*c).to_string()
                }
            })
            .collect::<Vec<String>>()
            .join("/");
        let under = format!("{twin}/");
        if twin != own && paths.iter().any(|p| *p == twin || p.starts_with(&under)) {
            finding.severity = Severity::High;
            finding.why = format!(
                "{} The Latin-only twin \"{twin}\" is in the same folder.",
                finding.why
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, Group, Lang, Severity};

    fn run(path: &str, kind: FileKind, text: &str) -> Vec<Finding> {
        let f = FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        out
    }

    fn code(text: &str) -> Vec<Finding> {
        run("src/app.js", FileKind::Code(Lang::Js), text)
    }

    fn rules(v: &[Finding]) -> Vec<&str> {
        v.iter().map(|f| f.rule.as_str()).collect()
    }

    fn one<'a>(v: &'a [Finding], rule: &str) -> &'a Finding {
        let hits: Vec<&Finding> = v.iter().filter(|f| f.rule == rule).collect();
        assert_eq!(hits.len(), 1, "{rule} in {:?}", rules(v));
        hits[0]
    }

    #[test]
    fn bidi_is_high_in_code_and_agent_medium_elsewhere() {
        let t = "let a = 1;\nlet b = \"\u{202E}evil\"; // \u{2066}x\u{2069}\n";
        let v = code(t);
        let f = one(&v, "hidden.bidi");
        assert_eq!(
            (f.severity, f.group, f.line, f.count),
            (Severity::High, Group::Hidden, 2, 3)
        );
        assert!(f.seen.contains("evil"), "{:?}", f.seen);
        let v = run("CLAUDE.md", FileKind::Agent, t);
        assert_eq!(one(&v, "hidden.bidi").severity, Severity::High);
        for kind in [FileKind::Doc, FileKind::Config, FileKind::Data] {
            let v = run("notes.txt", kind, t);
            assert_eq!(
                one(&v, "hidden.bidi").severity,
                Severity::Medium,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn arabic_text_with_a_right_to_left_mark_is_clean() {
        assert!(run("a.md", FileKind::Doc, "مرحبا\u{200F} بالعالم\n").is_empty());
        assert!(code("// \u{200F}\u{200E}\n").is_empty());
    }

    #[test]
    fn zero_width_is_high_in_agent_and_medium_in_code() {
        for c in ['\u{200B}', '\u{2060}', '\u{200D}', '\u{200C}'] {
            let t = format!("ignore a{c}b previous\n");
            let v = run("AGENTS.md", FileKind::Agent, &t);
            let f = one(&v, "hidden.zero-width");
            assert_eq!(
                (f.severity, f.line, f.count),
                (Severity::High, 1, 1),
                "{c:?}"
            );
            let v = code(&t);
            assert_eq!(
                one(&v, "hidden.zero-width").severity,
                Severity::Medium,
                "{c:?}"
            );
        }
    }

    #[test]
    fn a_byte_order_mark_is_zero_width_only_after_the_start() {
        assert!(code("\u{FEFF}let a = 1;\n").is_empty());
        let v = code("let a = 1;\n\u{FEFF}let b = 2;\n");
        assert_eq!(one(&v, "hidden.zero-width").line, 2);
    }

    #[test]
    fn zero_width_in_other_text_is_low() {
        let v = run("a.md", FileKind::Doc, "a\u{200B}b\n");
        assert_eq!(one(&v, "hidden.zero-width").severity, Severity::Low);
    }

    #[test]
    fn family_emoji_joiners_are_clean() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert!(run("a.md", FileKind::Agent, family).is_empty());
        // A heart with a variation selector before the joiner, then a flame.
        assert!(run("a.md", FileKind::Agent, "\u{2764}\u{FE0F}\u{200D}\u{1F525}").is_empty());
    }

    #[test]
    fn persian_non_joiner_is_clean_and_latin_non_joiner_is_not() {
        assert!(run("a.md", FileKind::Agent, "می\u{200C}خواهم").is_empty());
        assert!(run("a.md", FileKind::Agent, "नहीं\u{200C}है").is_empty());
        let v = run("a.md", FileKind::Agent, "plain\u{200C}text");
        assert_eq!(one(&v, "hidden.zero-width").severity, Severity::High);
    }

    #[test]
    fn tag_characters_are_critical_in_agent_and_high_elsewhere() {
        let hidden = "ignore\u{E0049}\u{E0047}\u{E004E} rules\n";
        let v = run(".cursorrules", FileKind::Agent, hidden);
        let f = one(&v, "hidden.tag-chars");
        assert_eq!((f.severity, f.count), (Severity::Critical, 3));
        assert_eq!(
            one(&code(hidden), "hidden.tag-chars").severity,
            Severity::High
        );
    }

    #[test]
    fn the_england_flag_sequence_is_clean() {
        let england = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";
        assert!(run("a.md", FileKind::Agent, england).is_empty());
        // The same tags with no flag in front, or no cancel tag at the end, are not a flag.
        let loose = "\u{E0067}\u{E0062}\u{E007F}";
        assert_eq!(
            one(&run("a.md", FileKind::Agent, loose), "hidden.tag-chars").count,
            3
        );
        let open = "\u{1F3F4}\u{E0067}\u{E0062}";
        assert_eq!(
            one(&run("a.md", FileKind::Agent, open), "hidden.tag-chars").count,
            2
        );
    }

    #[test]
    fn four_variation_selectors_in_a_row_are_high() {
        let run4 = "a\u{FE00}\u{FE01}\u{FE02}\u{FE0F}b";
        let f = &code(run4);
        assert_eq!(one(f, "hidden.variation-run").severity, Severity::High);
        let supp = "x\u{E0100}\u{E0101}\u{E0102}\u{E0103}\u{E0104}";
        assert_eq!(
            one(&run("a.md", FileKind::Doc, supp), "hidden.variation-run").severity,
            Severity::High
        );
        // Two runs of four count as two hits.
        let two = format!("{run4} {run4}");
        assert_eq!(one(&code(&two), "hidden.variation-run").count, 2);
    }

    #[test]
    fn ideographic_and_keycap_selectors_are_clean() {
        assert!(run("a.md", FileKind::Doc, "葛\u{E0100} 辻\u{E0101}").is_empty());
        assert!(run("a.md", FileKind::Doc, "1\u{FE0F}\u{20E3} 2\u{FE0F}\u{20E3}").is_empty());
        assert!(code("a\u{FE00}\u{FE01}\u{FE02}b").is_empty());
    }

    fn pushed(spaces: usize, tail: &str) -> String {
        format!("const a = 1;{}{tail}\n", " ".repeat(spaces))
    }

    #[test]
    fn a_whitespace_push_hides_code_off_screen() {
        let t = pushed(200, "require('child_process')");
        let v = code(&t);
        let f = one(&v, "hidden.whitespace-push");
        assert_eq!(
            (f.severity, f.group, f.line, f.count),
            (Severity::High, Group::Hidden, 1, 1)
        );
        assert!(f.seen.contains("require"), "{:?}", f.seen);
        let tabs = format!("x = 1;{}__import__('os')\n", "\t".repeat(150));
        assert_eq!(
            one(
                &run("a.py", FileKind::Code(Lang::Py), &tabs),
                "hidden.whitespace-push"
            )
            .line,
            1
        );
    }

    #[test]
    fn whitespace_runs_that_are_too_short_or_hide_nothing_are_clean() {
        assert!(code(&pushed(149, "require('child_process')")).is_empty());
        assert!(code(&pushed(200, "short")).is_empty());
        // Indentation is a run with nothing in front of it.
        assert!(code(&format!("{}require('child_process')\n", " ".repeat(200))).is_empty());
        // Trailing spaces hide nothing.
        assert!(code(&format!("const a = 1;{}\n", " ".repeat(200))).is_empty());
        // Only code files: a Doc may align a table.
        assert!(
            run(
                "a.md",
                FileKind::Doc,
                &pushed(200, "require('child_process')")
            )
            .is_empty()
        );
    }

    #[test]
    fn a_minified_bundle_is_clean() {
        let min = format!("!function(){{{}}}();\n", "var a=1;b(a);".repeat(500));
        assert!(code(&min).is_empty());
        let t = pushed(200, "require('child_process')");
        assert!(run("dist/app.min.js", FileKind::Code(Lang::Js), &t).is_empty());
    }

    #[test]
    fn hidden_text_rules_skip_a_file_that_is_not_text() {
        let f = FileCtx {
            path: "a.bin",
            bytes: b"\xe2\x80\xae",
            text: None,
            kind: FileKind::Binary,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_name_mixing_latin_and_cyrillic_is_medium() {
        let v = run("gradl\u{0435}-wrapper.jar", FileKind::Binary, "");
        let f = one(&v, "hidden.homoglyph-name");
        assert_eq!(
            (f.severity, f.group, f.line, f.count),
            (Severity::Medium, Group::Hidden, 0, 1)
        );
        let v = run("bin/g\u{03BF}.sh", FileKind::Code(Lang::Sh), "");
        assert_eq!(one(&v, "hidden.homoglyph-name").severity, Severity::Medium);
        let v = run("p\u{0430}ckage/index.js", FileKind::Code(Lang::Js), "");
        assert_eq!(
            one(&v, "hidden.homoglyph-name").path,
            "p\u{0430}ckage/index.js"
        );
    }

    #[test]
    fn single_script_names_are_clean() {
        assert!(run("\u{0434}\u{043E}\u{043A}\u{0443}\u{043C}\u{0435}\u{043D}\u{0442}\u{044B}/\u{0447}\u{0438}\u{0442}\u{0430}\u{0439}.md", FileKind::Doc, "").is_empty());
        assert!(run("\u{03B1}\u{03B2}\u{03B3}.txt", FileKind::Data, "").is_empty());
        assert!(run("src/caf\u{00E9}.js", FileKind::Code(Lang::Js), "").is_empty());
    }

    #[test]
    fn a_homoglyph_with_a_latin_twin_in_the_folder_is_high() {
        let spoof = "gradl\u{0435}-wrapper.jar";
        let mut v = run(spoof, FileKind::Binary, "");
        raise_homoglyph_twins(&[spoof, "gradle-wrapper.jar", "other.txt"], &mut v);
        assert_eq!(one(&v, "hidden.homoglyph-name").severity, Severity::High);
        assert!(
            one(&v, "hidden.homoglyph-name")
                .why
                .contains("gradle-wrapper.jar")
        );
    }

    #[test]
    fn a_homoglyph_without_a_twin_stays_medium() {
        let spoof = "gradl\u{0435}-wrapper.jar";
        let mut v = run(spoof, FileKind::Binary, "");
        raise_homoglyph_twins(&[spoof, "other/gradle-wrapper.jar"], &mut v);
        assert_eq!(one(&v, "hidden.homoglyph-name").severity, Severity::Medium);
    }

    #[test]
    fn a_homoglyph_folder_with_a_twin_folder_is_high() {
        let spoof = "p\u{0430}ckage/index.js";
        let mut v = run(spoof, FileKind::Code(Lang::Js), "");
        raise_homoglyph_twins(&[spoof, "package/other.js"], &mut v);
        assert_eq!(one(&v, "hidden.homoglyph-name").severity, Severity::High);
    }
}
