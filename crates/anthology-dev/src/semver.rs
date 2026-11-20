//! Semantic versions: `check_version_bump.parse_semver`, `compare_semver` and
//! `_compare_prerelease`, and `release_edit.range_accepts` (npm range semantics). Owner: DV2.
//!
//! The numbers are unbounded in Python, so they stay decimal strings here (ruling R6): the
//! grammar rules out leading zeros, so two numbers compare by length, then by bytes.

use std::cmp::Ordering;
use std::fmt;
use std::sync::LazyLock;

use anthology_pycompat::json::Value;
use regex::Regex;

use crate::json_edit::{py_repr, str_repr};
use crate::memory_diff::pystr;

const NUM: &str = "(0|[1-9][0-9]*)";
const PRE_ID: &str = "(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)";
const BUILD_ID: &str = "[0-9A-Za-z-]+";

/// `SEMVER_RE` for `re.fullmatch` (`\A…\z`). Python's pattern has no lookaround, so the `regex`
/// crate takes it as is.
static SEMVER_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(
        r"\A{NUM}\.{NUM}\.{NUM}(?:-({PRE_ID}(?:\.{PRE_ID})*))?(?:\+({BUILD_ID}(?:\.{BUILD_ID})*))?\z"
    );
    match Regex::new(&pattern) {
        Ok(re) => re,
        Err(e) => unreachable!("SEMVER_RE is a constant pattern: {e}"),
    }
});

/// A parsed version: `(major, minor, patch, prerelease identifiers)`. The three numbers are
/// canonical decimal strings (no leading zero), of any length. Build metadata is dropped, as
/// `parse_semver` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Semver {
    pub major: String,
    pub minor: String,
    pub patch: String,
    pub pre: Vec<String>,
}

impl Semver {
    fn core(&self) -> [&str; 3] {
        [&self.major, &self.minor, &self.patch]
    }
}

/// `parse_semver(str)`: `None` when it is not valid semver.
pub fn parse(version: &str) -> Option<Semver> {
    let m = SEMVER_RE.captures(version)?;
    let pre = m
        .get(4)
        .map(|g| g.as_str().split('.').map(str::to_string).collect())
        .unwrap_or_default();
    Some(Semver {
        major: m[1].to_string(),
        minor: m[2].to_string(),
        patch: m[3].to_string(),
        pre,
    })
}

/// `parse_semver(value)` for a JSON value: a non-string is `None`.
pub fn parse_value(version: &Value) -> Option<Semver> {
    parse(version.as_str()?)
}

/// Compares two non-negative decimal numbers without a leading zero.
fn num_cmp(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// `_compare_prerelease`. An empty list is a release, which outranks any prerelease.
fn compare_prerelease(a: &[String], b: &[String]) -> Ordering {
    if a.is_empty() && b.is_empty() {
        return Ordering::Equal;
    }
    if a.is_empty() {
        return Ordering::Greater;
    }
    if b.is_empty() {
        return Ordering::Less;
    }
    for (x, y) in a.iter().zip(b) {
        if x == y {
            continue;
        }
        let is_digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
        let (x_num, y_num) = (is_digits(x), is_digits(y));
        if x_num && y_num {
            return num_cmp(x, y);
        }
        if x_num {
            return Ordering::Less;
        }
        if y_num {
            return Ordering::Greater;
        }
        return x.cmp(y);
    }
    a.len().cmp(&b.len())
}

/// The precedence of two parsed versions (build metadata is not part of a [`Semver`]).
pub fn cmp_parsed(a: &Semver, b: &Semver) -> Ordering {
    for (x, y) in a.core().iter().zip(b.core().iter()) {
        let o = num_cmp(x, y);
        if o != Ordering::Equal {
            return o;
        }
    }
    compare_prerelease(&a.pre, &b.pre)
}

/// `compare_semver`'s `ValueError`: `not valid semver: 'a' / 'b'`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotSemver(pub String);

impl fmt::Display for NotSemver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotSemver {}

/// `compare_semver(a, b)`: the precedence of two valid versions (build metadata ignored).
pub fn compare(a: &str, b: &str) -> Result<Ordering, NotSemver> {
    match (parse(a), parse(b)) {
        (Some(pa), Some(pb)) => Ok(cmp_parsed(&pa, &pb)),
        _ => Err(NotSemver(format!(
            "not valid semver: {} / {}",
            str_repr(a),
            str_repr(b)
        ))),
    }
}

/// Why [`range_accepts`] gave no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeError {
    /// `UnsupportedRange`: a range in a form this tool does not evaluate.
    Unsupported(String),
    /// `ReleaseError`: the version is not valid semver.
    BadVersion(String),
    /// An exception Python does not catch (ruling R10): its class and message.
    Uncaught { ty: &'static str, msg: String },
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RangeError::Unsupported(m) | RangeError::BadVersion(m) => f.write_str(m),
            RangeError::Uncaught { ty, msg } => write!(f, "{ty}: {msg}"),
        }
    }
}

impl std::error::Error for RangeError {}

/// `_PARTIAL_RE`'s `0|[1-9][0-9]*`.
fn is_plain_number(s: &str) -> bool {
    s == "0"
        || (s
            .as_bytes()
            .first()
            .is_some_and(|c| (b'1'..=b'9').contains(c))
            && s.bytes().all(|c| c.is_ascii_digit()))
}

/// `_PARTIAL_RE.match(rest)`: `X` or `X.Y`.
fn parse_partial(rest: &str) -> Option<(&str, Option<&str>)> {
    match rest.split_once('.') {
        None => is_plain_number(rest).then_some((rest, None)),
        Some((x, y)) => (is_plain_number(x) && is_plain_number(y)).then_some((x, Some(y))),
    }
}

/// `n + 1` on a decimal string.
fn increment(n: &str) -> String {
    let mut digits: Vec<u8> = n.bytes().collect();
    let mut i = digits.len();
    loop {
        if i == 0 {
            digits.insert(0, b'1');
            break;
        }
        i -= 1;
        if digits[i] == b'9' {
            digits[i] = b'0';
        } else {
            digits[i] += 1;
            break;
        }
    }
    String::from_utf8_lossy(&digits).into_owned()
}

fn tuple_lt(a: [&str; 3], b: &[String; 3]) -> bool {
    for (x, y) in a.iter().zip(b.iter()) {
        let o = num_cmp(x, y);
        if o != Ordering::Equal {
            return o == Ordering::Less;
        }
    }
    false
}

/// `range_accepts(rng, version)`: does the npm range `rng` accept `version`?
///
/// Forms: `~X`, `~X.Y`, `~X.Y.Z`, `^X`, `^X.Y`, `^X.Y.Z`, `X.Y.Z`, `>=X.Y.Z`; see the Python
/// docstring for the partial forms and the prerelease rule. `rng` is any JSON value, because a
/// non-string must raise `UnsupportedRange` as it does in Python.
pub fn range_accepts(rng: &Value, version: &str) -> Result<bool, RangeError> {
    let unsupported = || RangeError::Unsupported(format!("can't check range {}", py_repr(rng)));
    let Some(s) = rng.as_str() else {
        return Err(unsupported());
    };
    let stripped = pystr::strip(s);
    if stripped.is_empty() || s != stripped {
        return Err(unsupported());
    }
    // `_RANGE_RE.match(rng)`: `.` does not match `\n`, and `$` allows only a final one, which
    // the strip check has ruled out, so a `\n` inside means no match: `m.group` raises.
    if s.contains('\n') {
        return Err(RangeError::Uncaught {
            ty: "AttributeError",
            msg: "'NoneType' object has no attribute 'group'".into(),
        });
    }
    // `(~|\^|>=)?(.+)`: the operator only when something follows it.
    let (op, rest) = ["~", "^", ">="]
        .iter()
        .find_map(|op| {
            s.strip_prefix(op)
                .filter(|r| !r.is_empty())
                .map(|r| (*op, r))
        })
        .unwrap_or(("", s));
    let Some(pv) = parse(version) else {
        return Err(RangeError::BadVersion(format!(
            "{} is not a valid semver version",
            str_repr(version)
        )));
    };
    let partial = parse_partial(rest);
    let mut minor_named = false;
    let base = match partial {
        Some((x, y)) if op == "~" || op == "^" => {
            minor_named = y.is_some();
            format!("{x}.{}.0", y.unwrap_or("0"))
        }
        _ if !rest.contains('+') && parse(rest).is_some() => rest.to_string(),
        _ => return Err(unsupported()),
    };
    let Some(bp) = parse(&base) else {
        return Err(unsupported());
    };
    if !pv.pre.is_empty() && !(!bp.pre.is_empty() && pv.core() == bp.core()) {
        return Ok(false);
    }
    let order = cmp_parsed(&pv, &bp);
    if op.is_empty() {
        return Ok(order == Ordering::Equal);
    }
    if order == Ordering::Less {
        return Ok(false);
    }
    if op == ">=" {
        return Ok(true);
    }
    let zero = || "0".to_string();
    let upper: [String; 3] = if op == "~" {
        if minor_named || partial.is_none() {
            [bp.major.clone(), increment(&bp.minor), zero()]
        } else {
            [increment(&bp.major), zero(), zero()]
        }
    } else if bp.major != "0" || (partial.is_some() && !minor_named) {
        [increment(&bp.major), zero(), zero()]
    } else if partial.is_some() || bp.minor != "0" {
        [zero(), increment(&bp.minor), zero()]
    } else {
        [zero(), zero(), increment(&bp.patch)]
    };
    Ok(tuple_lt(pv.core(), &upper))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increments_decimal_strings() {
        assert_eq!(increment("0"), "1");
        assert_eq!(increment("9"), "10");
        assert_eq!(increment("1999"), "2000");
        assert_eq!(increment("99999999999999999999"), "100000000000000000000");
    }

    #[test]
    fn numbers_are_unbounded() {
        let big = "99999999999999999999999999.0.0";
        assert!(parse(big).is_some());
        assert_eq!(
            compare(big, "100000000000000000000000000.0.0"),
            Ok(Ordering::Less)
        );
    }

    #[test]
    fn partial_forms() {
        assert_eq!(parse_partial("1"), Some(("1", None)));
        assert_eq!(parse_partial("1.2"), Some(("1", Some("2"))));
        assert_eq!(parse_partial("01"), None);
        assert_eq!(parse_partial("1.2.3"), None);
        assert_eq!(parse_partial("1."), None);
    }
}
