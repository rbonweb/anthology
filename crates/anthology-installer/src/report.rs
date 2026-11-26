//! What the installer prints: one `ok|skip|fail <step>: <detail>` line per step, or with `--json`
//! one document at the end.

use std::io::Write;

use anthology_pycompat::json::{self, Int, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Skip,
    Fail,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Skip => "skip",
            Status::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub status: Status,
    pub step: String,
    pub detail: String,
}

/// Text from a program, a file or the network is untrusted: one control character (an escape
/// sequence) could rewrite what the terminal shows. Every control character becomes a space,
/// runs of whitespace collapse, and the text is cut after `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let joined = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    match joined.char_indices().nth(max) {
        Some((cut, _)) => format!("{}...", &joined[..cut]),
        None => joined,
    }
}

/// Like [`clean`] for a block of lines (a diff): newlines and tabs stay, other control
/// characters (and a carriage return) become `?`.
pub fn clean_block(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '?'
            } else {
                c
            }
        })
        .collect()
}

/// Longest detail of a step line, in characters.
pub const DETAIL_MAX: usize = 400;

pub struct Report {
    json: bool,
    out: Box<dyn Write>,
    steps: Vec<Step>,
    notes: Vec<String>,
}

impl Report {
    pub fn new(json: bool, out: Box<dyn Write>) -> Report {
        Report {
            json,
            out,
            steps: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn is_json(&self) -> bool {
        self.json
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn step(&mut self, status: Status, step: &str, detail: &str) {
        let step = clean(step, 80);
        let detail = clean(detail, DETAIL_MAX);
        if !self.json {
            let line = if detail.is_empty() {
                format!("{} {step}\n", status.word())
            } else {
                format!("{} {step}: {detail}\n", status.word())
            };
            // A closed stdout is not a reason to stop half way through an install.
            let _ = self.out.write_all(line.as_bytes());
        }
        self.steps.push(Step {
            status,
            step,
            detail,
        });
    }

    pub fn ok(&mut self, step: &str, detail: &str) {
        self.step(Status::Ok, step, detail);
    }

    pub fn skip(&mut self, step: &str, detail: &str) {
        self.step(Status::Skip, step, detail);
    }

    pub fn fail(&mut self, step: &str, detail: &str) {
        self.step(Status::Fail, step, detail);
    }

    /// A block of text for the person at the terminal (a settings diff). Kept under `notes` in
    /// the JSON document.
    pub fn note(&mut self, text: &str) {
        let text = clean_block(text);
        if !self.json {
            let _ = self.out.write_all(text.as_bytes());
        }
        self.notes.push(text);
    }

    /// Ends the run: with `--json`, the one document. `code` is the process exit code.
    pub fn finish(&mut self, command: &str, code: i32) {
        if !self.json {
            return;
        }
        let strings = |items: &[&str]| -> Value {
            Value::Array(items.iter().map(|s| Value::Str(s.to_string())).collect())
        };
        let steps = self
            .steps
            .iter()
            .map(|s| {
                let mut m = Map::new();
                m.insert("status".into(), Value::Str(s.status.word().into()));
                m.insert("step".into(), Value::Str(s.step.clone()));
                m.insert("detail".into(), Value::Str(s.detail.clone()));
                Value::Object(m)
            })
            .collect();
        let mut doc = Map::new();
        doc.insert("command".into(), Value::Str(command.into()));
        doc.insert("exitCode".into(), Value::Int(Int::Small(i64::from(code))));
        doc.insert("steps".into(), Value::Array(steps));
        let notes: Vec<&str> = self.notes.iter().map(String::as_str).collect();
        doc.insert("notes".into(), strings(&notes));
        let text = json::dumps(&Value::Object(doc), &json::INDENT2) + "\n";
        let _ = self.out.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Buf;

    #[test]
    fn clean_turns_control_characters_into_single_spaces() {
        assert_eq!(clean("a\x1b[31mred\x1b[0m\nb\tc", 100), "a [31mred [0m b c");
        assert_eq!(clean("  x  ", 100), "x");
        assert_eq!(clean("a\r\nb", 100), "a b");
    }

    #[test]
    fn clean_cuts_long_text_on_a_character_boundary() {
        assert_eq!(clean("abcdef", 3), "abc...");
        assert_eq!(clean("abc", 3), "abc");
        assert_eq!(clean("héllo", 2), "hé...");
    }

    #[test]
    fn clean_block_keeps_lines_but_not_escapes() {
        assert_eq!(clean_block("+a\n-b\tc\x1b[2J\r\n"), "+a\n-b\tc?[2J?\n");
    }

    #[test]
    fn a_step_prints_one_line_in_the_documented_shape() {
        let buf = Buf::default();
        let mut r = Report::new(false, Box::new(buf.clone()));
        r.ok("marketplace", "added rbonweb/anthology");
        r.skip("setup", "no terminal\nrun it yourself");
        r.fail("binary", "bad \x1b[31m news");
        assert_eq!(
            buf.text(),
            "ok marketplace: added rbonweb/anthology\n\
             skip setup: no terminal run it yourself\n\
             fail binary: bad [31m news\n"
        );
        assert_eq!(r.steps().len(), 3);
        assert_eq!(r.steps()[2].status, Status::Fail);
    }

    #[test]
    fn a_note_prints_in_text_mode_and_waits_for_the_document_in_json_mode() {
        let buf = Buf::default();
        let mut r = Report::new(false, Box::new(buf.clone()));
        r.note("+line\n-line\n");
        assert_eq!(buf.text(), "+line\n-line\n");
        r.finish("install", 0);
        assert_eq!(
            buf.text(),
            "+line\n-line\n",
            "text mode prints nothing at the end"
        );
    }

    #[test]
    fn json_mode_prints_only_the_final_document() {
        let buf = Buf::default();
        let mut r = Report::new(true, Box::new(buf.clone()));
        r.ok("preflight", "Linux x86_64");
        r.fail("plugins", "boom \"quoted\"");
        r.note("diff text\n");
        assert_eq!(buf.text(), "", "nothing before finish");
        r.finish("install", 1);
        let doc = json::loads(&buf.text()).unwrap();
        let doc = doc.as_object().unwrap();
        assert_eq!(doc.get("command").and_then(Value::as_str), Some("install"));
        assert_eq!(doc.get("exitCode").and_then(Value::as_i64), Some(1));
        let steps = doc.get("steps").and_then(Value::as_array).unwrap();
        assert_eq!(steps.len(), 2);
        let second = steps[1].as_object().unwrap();
        assert_eq!(second.get("status").and_then(Value::as_str), Some("fail"));
        assert_eq!(second.get("step").and_then(Value::as_str), Some("plugins"));
        assert_eq!(
            second.get("detail").and_then(Value::as_str),
            Some("boom \"quoted\"")
        );
        let notes = doc.get("notes").and_then(Value::as_array).unwrap();
        assert_eq!(notes[0].as_str(), Some("diff text\n"));
    }
}
