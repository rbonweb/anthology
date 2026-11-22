//! `Observed`: what one side did for one case (F4, plan §3.7).
//!
//! An oracle module's `run(case)` returns `{"exit": N, "stdout": S, "stderr": S}`, optionally
//! with `"files"` (the `anthology_testkit::Tree` JSON form), `"stdout_hex"`/`"stderr_hex"` in
//! place of a text that is not UTF-8, `"divergence": "D-n"` (an allowed GC-16 divergence the
//! oracle already applied to the expected result) and `"python"` (Python's own result, kept for
//! the report). [`from_oracle`] reads that; the oracle records the divergence mark itself
//! (`Oracle::call`), so `Observed` keeps exactly the published fields.
#![allow(
    dead_code,
    reason = "published for the surface lanes; not every item has a caller yet"
)]

use anthology_pycompat::json::{Int, Value};
use anthology_testkit::Tree;

use crate::case::cut;

/// What one side did (plan §3.7, exactly).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Observed {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub files: Option<Tree>,
}

/// The exit code `from_oracle` gives a response it cannot read (and an `{"error": ...}`).
pub const BROKEN: i32 = -1000;

impl Observed {
    /// An exit code with text output.
    pub fn text(exit: i32, stdout: &str, stderr: &str) -> Observed {
        Observed {
            exit,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            files: None,
        }
    }

    /// Each difference from `self` (Python) to `rust`, one readable line each.
    pub fn diff(&self, rust: &Observed) -> Vec<String> {
        let mut out = Vec::new();
        if self.exit != rust.exit {
            out.push(format!("exit: python {} rust {}", self.exit, rust.exit));
        }
        for (name, a, b) in [
            ("stdout", &self.stdout, &rust.stdout),
            ("stderr", &self.stderr, &rust.stderr),
        ] {
            if a != b {
                let at = a
                    .iter()
                    .zip(b)
                    .position(|(x, y)| x != y)
                    .unwrap_or(a.len().min(b.len()));
                out.push(format!(
                    "{name}: first difference at byte {at} (python {} bytes, rust {} bytes): \
                     python {} rust {}",
                    a.len(),
                    b.len(),
                    excerpt(a, at),
                    excerpt(b, at)
                ));
            }
        }
        match (&self.files, &rust.files) {
            (Some(a), Some(b)) => {
                for line in a.diff(b) {
                    out.push(format!("files: {line}"));
                }
            }
            (None, Some(_)) => out.push("files: only rust has a tree".into()),
            (Some(_), None) => out.push("files: only python has a tree".into()),
            (None, None) => {}
        }
        out
    }

    /// A short rendering for reports: exit, stdout, stderr and the file count.
    pub fn render(&self, max: usize) -> String {
        let files = match &self.files {
            Some(t) => format!(" files {}", t.len()),
            None => String::new(),
        };
        format!(
            "exit {} stdout {} stderr {}{files}",
            self.exit,
            cut(&format!("{:?}", String::from_utf8_lossy(&self.stdout)), max),
            cut(&format!("{:?}", String::from_utf8_lossy(&self.stderr)), max)
        )
    }
}

fn excerpt(b: &[u8], at: usize) -> String {
    let start = at.saturating_sub(20);
    let end = (at + 40).min(b.len());
    format!("{:?}", String::from_utf8_lossy(&b[start..end]))
}

/// Reads an oracle response (module docs). A response that is an `{"error": ...}` or that this
/// cannot read gives exit [`BROKEN`] and the reason on stderr; the oracle has already counted it
/// as an error.
pub fn from_oracle(resp: &Value) -> Observed {
    match read(resp) {
        Ok(o) => o,
        Err(why) => Observed {
            exit: BROKEN,
            stdout: Vec::new(),
            stderr: why.into_bytes(),
            files: None,
        },
    }
}

/// The error text of an oracle response, if it is not a usable result.
pub fn oracle_error(resp: &Value) -> Option<String> {
    read(resp).err()
}

fn read(resp: &Value) -> Result<Observed, String> {
    let o = resp
        .as_object()
        .ok_or("oracle: the response is not an object")?;
    if let Some(e) = o.get("error") {
        return Err(format!(
            "oracle error: {}",
            e.as_str().unwrap_or("(not a string)")
        ));
    }
    let exit = match o.get("exit") {
        Some(Value::Int(Int::Small(n))) => {
            i32::try_from(*n).map_err(|_| format!("oracle: exit {n} is out of range"))?
        }
        _ => return Err("oracle: no integer \"exit\"".into()),
    };
    let stream = |name: &str| -> Result<Vec<u8>, String> {
        let hex_key = format!("{name}_hex");
        match (o.get(name), o.get(&hex_key)) {
            (Some(Value::Str(s)), None) => Ok(s.as_bytes().to_vec()),
            (None, Some(Value::Str(h))) => {
                unhex(h).ok_or_else(|| format!("oracle: \"{hex_key}\" is not hex"))
            }
            (None, None) => Ok(Vec::new()),
            _ => Err(format!("oracle: bad \"{name}\"")),
        }
    };
    let files = match o.get("files") {
        None | Some(Value::Null) => None,
        Some(v) => Some(Tree::from_json(v).map_err(|e| format!("oracle: {e}"))?),
    };
    Ok(Observed {
        exit,
        stdout: stream("stdout")?,
        stderr: stream("stderr")?,
        files,
    })
}

/// The `"divergence"` mark of an oracle response, if any.
pub fn divergence_mark(resp: &Value) -> Option<&str> {
    resp.as_object()?.get("divergence")?.as_str()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json::loads;

    fn v(s: &str) -> Value {
        loads(s).expect("json")
    }

    #[test]
    fn reads_a_result_with_hex_and_files() {
        let o = from_oracle(&v(
            r#"{"exit": 1, "stdout": "out", "stderr_hex": "ff00", "divergence": "D-6",
                "python": {"exit": 1}, "files": {"a": {"type": "dir", "mode": "0755"}}}"#,
        ));
        assert_eq!(o.exit, 1);
        assert_eq!(o.stdout, b"out");
        assert_eq!(o.stderr, [0xff, 0]);
        assert_eq!(o.files.as_ref().map(Tree::len), Some(1));
        assert_eq!(divergence_mark(&v(r#"{"divergence": "D-6"}"#)), Some("D-6"));
        assert_eq!(divergence_mark(&v(r#"{"exit": 0}"#)), None);
    }

    #[test]
    fn errors_and_bad_responses_are_broken() {
        for (resp, want) in [
            (
                r#"{"error": "Traceback ..."}"#,
                "oracle error: Traceback ...",
            ),
            (r#"[]"#, "not an object"),
            (r#"{"stdout": ""}"#, "no integer"),
            (r#"{"exit": 0, "stdout": 1}"#, "bad \"stdout\""),
            (r#"{"exit": 0, "stdout_hex": "zz"}"#, "not hex"),
            (
                r#"{"exit": 0, "files": {"a": {"type": "x"}}}"#,
                "unknown type",
            ),
        ] {
            let o = from_oracle(&v(resp));
            assert_eq!(o.exit, BROKEN, "{resp}");
            assert!(String::from_utf8_lossy(&o.stderr).contains(want), "{resp}");
            assert!(oracle_error(&v(resp)).is_some());
        }
        assert!(oracle_error(&v(r#"{"exit": 0}"#)).is_none());
    }

    #[test]
    fn diff_names_each_difference() {
        let a = Observed::text(0, "hello world", "");
        let mut b = Observed::text(1, "hello there", "x");
        assert_eq!(a.diff(&a), Vec::<String>::new());
        let d = a.diff(&b);
        assert_eq!(d.len(), 3, "{d:?}");
        assert_eq!(d[0], "exit: python 0 rust 1");
        assert!(
            d[1].starts_with("stdout: first difference at byte 6"),
            "{}",
            d[1]
        );
        b = a.clone();
        b.files = Some(Tree::default());
        assert_eq!(a.diff(&b), ["files: only rust has a tree"]);
        assert!(a.render(10).starts_with("exit 0 stdout \"hello wor"));
    }
}
