//! `rows()`: table rows from `tests/data/*.jsonl` (F4, plan §0.1.5 and §3.7).
//!
//! A Python test's table rows are extracted (F4a's `port/extract/rows.py --jsonl`) into
//! `crates/<crate>/tests/data/<python_module>.jsonl`, one row per line:
//!
//! ```text
//! {"py": "<python test id>", "row": <n>, "params": {"<subTest keyword>": <value>, ...}}
//! ```
//!
//! Values that are not JSON are tagged: `{"$bytes": "<hex>"}`, `{"$set": [...]}`,
//! `{"$float": "nan" | "inf" | "-inf"}` and `{"$repr": "<repr>"}`. [`untag_bytes`] and
//! [`untag_float`] read the first two kinds that matter.
//!
//! [`rows`] reads the file of the crate under test (`CARGO_MANIFEST_DIR`, read at run time, which
//! cargo sets for every test binary) and returns the rows whose `"py"` is the given id, in file
//! order, each decoded by [`FromPyJson`]. A missing file, an unreadable line or no row panics
//! with the path and the id, so a renamed id cannot silently test nothing.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Int, Map, Value};

/// Decodes one row (or a part of one) from its JSON value.
pub trait FromPyJson: Sized {
    fn from_py_json(v: &Value) -> Result<Self, String>;
}

/// The rows of `tests/data/<file>.jsonl` whose `"py"` equals `py_id` (module docs).
pub fn rows<T: FromPyJson>(file: &str, py_id: &str) -> Vec<T> {
    rows_in(&rows_path(file), py_id)
}

/// [`rows`] from the file at `path`.
pub fn rows_in<T: FromPyJson>(path: &Path, py_id: &str) -> Vec<T> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("rows: cannot read {} (for {py_id}): {e}", path.display()));
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v = json::loads(line)
            .unwrap_or_else(|e| panic!("rows: {}:{}: not JSON: {e}", path.display(), i + 1));
        if v.as_object()
            .and_then(|o| o.get("py"))
            .and_then(Value::as_str)
            != Some(py_id)
        {
            continue;
        }
        let row = T::from_py_json(&v)
            .unwrap_or_else(|e| panic!("rows: {}:{} ({py_id}): {e}", path.display(), i + 1));
        out.push(row);
    }
    assert!(
        !out.is_empty(),
        "rows: no row for {py_id} in {}",
        path.display()
    );
    out
}

/// `<CARGO_MANIFEST_DIR>/tests/data/<file>.jsonl` (`file` may already end in `.jsonl`).
pub fn rows_path(file: &str) -> PathBuf {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .expect("rows: CARGO_MANIFEST_DIR is not set (run under cargo test)");
    let name = if file.ends_with(".jsonl") {
        file.to_string()
    } else {
        format!("{file}.jsonl")
    };
    PathBuf::from(dir).join("tests").join("data").join(name)
}

/// One extracted row: its id, its index and its `subTest` keywords in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub py: String,
    pub row: usize,
    pub params: Map,
}

impl Row {
    /// The parameter `k`; panics with the row when it is missing.
    pub fn param(&self, k: &str) -> &Value {
        self.params
            .get(k)
            .unwrap_or_else(|| panic!("{} row {}: no parameter {k:?}", self.py, self.row))
    }

    /// The parameter `k` as text; panics when it is missing or not a string.
    pub fn str(&self, k: &str) -> &str {
        self.param(k)
            .as_str()
            .unwrap_or_else(|| panic!("{} row {}: {k:?} is not a string", self.py, self.row))
    }

    /// The parameter `k` decoded as `T`; panics on an error.
    pub fn get<T: FromPyJson>(&self, k: &str) -> T {
        T::from_py_json(self.param(k))
            .unwrap_or_else(|e| panic!("{} row {}: {k:?}: {e}", self.py, self.row))
    }
}

impl FromPyJson for Row {
    fn from_py_json(v: &Value) -> Result<Row, String> {
        let o = v.as_object().ok_or("a row is not an object")?;
        let py = o
            .get("py")
            .and_then(Value::as_str)
            .ok_or("a row has no \"py\" id")?
            .to_string();
        let row = match o.get("row") {
            Some(Value::Int(Int::Small(n))) if *n >= 0 => *n as usize,
            None => 0,
            Some(_) => return Err("\"row\" is not a non-negative integer".into()),
        };
        let params = match o.get("params") {
            Some(Value::Object(m)) => m.clone(),
            None => Map::new(),
            Some(_) => return Err("\"params\" is not an object".into()),
        };
        Ok(Row { py, row, params })
    }
}

/// The whole JSON line.
impl FromPyJson for Value {
    fn from_py_json(v: &Value) -> Result<Value, String> {
        Ok(v.clone())
    }
}

impl FromPyJson for String {
    fn from_py_json(v: &Value) -> Result<String, String> {
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("expected a string, got {v:?}"))
    }
}

impl FromPyJson for bool {
    fn from_py_json(v: &Value) -> Result<bool, String> {
        v.as_bool()
            .ok_or_else(|| format!("expected a bool, got {v:?}"))
    }
}

impl FromPyJson for i64 {
    fn from_py_json(v: &Value) -> Result<i64, String> {
        v.as_i64()
            .ok_or_else(|| format!("expected an i64, got {v:?}"))
    }
}

/// A number, or a `{"$float": ...}` tag.
impl FromPyJson for f64 {
    fn from_py_json(v: &Value) -> Result<f64, String> {
        match v {
            Value::Float(f) => Ok(*f),
            Value::Int(Int::Small(n)) => Ok(*n as f64),
            _ => untag_float(v).ok_or_else(|| format!("expected a float, got {v:?}")),
        }
    }
}

/// `null` is `None`.
impl<T: FromPyJson> FromPyJson for Option<T> {
    fn from_py_json(v: &Value) -> Result<Option<T>, String> {
        if v.is_null() {
            Ok(None)
        } else {
            T::from_py_json(v).map(Some)
        }
    }
}

/// An array (or a `{"$set": [...]}` tag).
impl<T: FromPyJson> FromPyJson for Vec<T> {
    fn from_py_json(v: &Value) -> Result<Vec<T>, String> {
        let items = match v {
            Value::Array(a) => a,
            Value::Object(o) if o.len() == 1 && o.get("$set").is_some() => o
                .get("$set")
                .and_then(Value::as_array)
                .ok_or("\"$set\" is not an array")?,
            _ => return Err(format!("expected an array, got {v:?}")),
        };
        items.iter().map(T::from_py_json).collect()
    }
}

/// Bytes: a `{"$bytes": hex}` tag, or a string's UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyBytes(pub Vec<u8>);

impl FromPyJson for PyBytes {
    fn from_py_json(v: &Value) -> Result<PyBytes, String> {
        if let Some(s) = v.as_str() {
            return Ok(PyBytes(s.as_bytes().to_vec()));
        }
        untag_bytes(v)
            .map(PyBytes)
            .ok_or_else(|| format!("expected bytes, got {v:?}"))
    }
}

/// The bytes of a `{"$bytes": "<hex>"}` tag.
pub fn untag_bytes(v: &Value) -> Option<Vec<u8>> {
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    let h = o.get("$bytes")?.as_str()?;
    if !h.len().is_multiple_of(2) || !h.is_ascii() {
        return None;
    }
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok())
        .collect()
}

/// The value of a `{"$float": "nan" | "inf" | "-inf"}` tag.
pub fn untag_float(v: &Value) -> Option<f64> {
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    match o.get("$float")?.as_str()? {
        "nan" => Some(f64::NAN),
        "inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

/// The values of `params` in order (a row), or of an array.
fn positional(v: &Value) -> Result<Vec<&Value>, String> {
    match v {
        Value::Array(a) => Ok(a.iter().collect()),
        Value::Object(o) => match o.get("params") {
            Some(Value::Object(p)) => Ok(p.iter().map(|(_, v)| v).collect()),
            _ => Err("expected a row with \"params\", or an array".into()),
        },
        _ => Err(format!("expected a row or an array, got {v:?}")),
    }
}

macro_rules! tuple_impl {
    ($n:literal; $($t:ident $i:tt),+) => {
        /// A row's `params` values in order (or an array's items), one per element.
        impl<$($t: FromPyJson),+> FromPyJson for ($($t,)+) {
            fn from_py_json(v: &Value) -> Result<Self, String> {
                let items = positional(v)?;
                if items.len() != $n {
                    return Err(format!("expected {} values, got {}", $n, items.len()));
                }
                Ok(($($t::from_py_json(items[$i])?,)+))
            }
        }
    };
}

tuple_impl!(1; A 0);
tuple_impl!(2; A 0, B 1);
tuple_impl!(3; A 0, B 1, C 2);
tuple_impl!(4; A 0, B 1, C 2, D 3);
tuple_impl!(5; A 0, B 1, C 2, D 3, E 4);

#[cfg(test)]
mod tests {
    use super::*;

    const LINES: &str = concat!(
        r#"{"py": "test_x.T.test_a", "row": 0, "params": {"cmd": "ls", "blocked": false}}"#,
        "\n",
        r#"{"py": "test_x.T.test_b", "row": 0, "params": {"b": {"$bytes": "00ff"}, "#,
        r#""f": {"$float": "nan"}, "s": {"$set": [1, 2]}, "n": null}}"#,
        "\n\n",
        r#"{"py": "test_x.T.test_a", "row": 1, "params": {"cmd": "rm -rf /", "blocked": true}}"#,
        "\n",
    );

    fn file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test_x.jsonl");
        std::fs::write(&path, LINES).expect("write");
        (dir, path)
    }

    #[test]
    fn rows_keeps_the_rows_of_one_id_in_order() {
        let (_dir, path) = file();
        let got: Vec<Row> = rows_in(&path, "test_x.T.test_a");
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].row, got[1].row), (0, 1));
        assert_eq!(got[0].str("cmd"), "ls");
        assert!(got[1].get::<bool>("blocked"));
        let whole: Vec<Value> = rows_in(&path, "test_x.T.test_b");
        assert_eq!(whole.len(), 1);
    }

    #[test]
    fn tuples_take_the_params_in_order() {
        let (_dir, path) = file();
        let got: Vec<(String, bool)> = rows_in(&path, "test_x.T.test_a");
        assert_eq!(got, [("ls".into(), false), ("rm -rf /".into(), true)]);
    }

    #[test]
    fn tags_decode() {
        let (_dir, path) = file();
        let got: Vec<(PyBytes, f64, Vec<i64>, Option<String>)> = rows_in(&path, "test_x.T.test_b");
        let (b, f, set, none) = &got[0];
        assert_eq!(b.0, vec![0, 0xff]);
        assert!(f.is_nan());
        assert_eq!(set, &[1, 2]);
        assert_eq!(none, &None);
    }

    #[test]
    fn rows_reads_the_crate_data_folder() {
        let path = rows_path("test_guard_rules");
        let want = format!(
            "{}/tests/data/test_guard_rules.jsonl",
            env!("CARGO_MANIFEST_DIR")
        );
        assert_eq!(path, PathBuf::from(want));
        assert_eq!(rows_path("m.jsonl"), rows_path("m"));
    }

    #[test]
    fn a_missing_id_panics_with_the_path_and_the_id() {
        let (_dir, path) = file();
        let err = std::panic::catch_unwind(|| rows_in::<Row>(&path, "test_x.T.nope"))
            .expect_err("panics");
        let msg = err.downcast_ref::<String>().expect("message");
        assert!(msg.contains("no row for test_x.T.nope"), "{msg}");
        assert!(msg.contains("test_x.jsonl"), "{msg}");
    }

    #[test]
    fn a_missing_file_panics_with_the_path() {
        let err =
            std::panic::catch_unwind(|| rows::<Row>("no_such_module", "x")).expect_err("panics");
        let msg = err.downcast_ref::<String>().expect("message");
        assert!(msg.contains("tests/data/no_such_module.jsonl"), "{msg}");
    }

    #[test]
    fn a_bad_row_shape_is_an_error() {
        let v = json::loads(r#"{"py": "x", "row": -1}"#).expect("json");
        assert!(Row::from_py_json(&v).is_err());
        let v = json::loads(r#"[1, 2]"#).expect("json");
        assert!(<(i64,)>::from_py_json(&v).is_err());
        assert_eq!(<(i64, i64)>::from_py_json(&v), Ok((1, 2)));
    }
}
