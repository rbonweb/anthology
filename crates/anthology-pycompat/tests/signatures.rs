//! Plan §3.2 is exact: lanes code against these signatures before P1 merges. This only has to
//! compile.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anthology_pycompat::re::LimitError;
use anthology_pycompat::{
    argparse, difflib, fnmatch, ipaddress, path, pyerr, quote, re, shlex, str as pystr,
};

type UnifiedDiff = fn(&[&str], &[&str], &str, &str, usize) -> Vec<String>;
type Parse = fn(&argparse::Spec, &[OsString]) -> Result<argparse::Parsed, argparse::UsageError>;

#[test]
fn plan_3_2_signatures() {
    let _: fn(&str) -> String = pystr::lower;
    let _: fn(char) -> bool = pystr::is_space;
    let _: for<'a> fn(&'a str) -> Vec<&'a str> = pystr::split_ws;
    let _: for<'a> fn(&'a str) -> &'a str = pystr::strip_ws;
    let _: fn(char) -> bool = pystr::is_digit;
    let _: fn(char) -> bool = pystr::is_decimal;
    let _: fn(char) -> bool = pystr::is_alpha;
    let _: fn(char) -> bool = pystr::is_alnum;
    let _: fn(&str) -> usize = pystr::char_len;
    let _: for<'a> fn(&'a str, usize, usize) -> &'a str = pystr::char_slice;
    let _: fn(char) -> &'static str = pystr::unicode_category;

    let _: [&str; 3] = [re::W, re::S, re::D];
    let _: fn(&str) -> Result<String, String> = re::translate;
    let _: fn(&str, bool) -> Result<re::Regex, String> = re::compile;
    let r = re::compile("a", false).unwrap();
    let _: Result<bool, LimitError> = r.is_match("a");
    let spans: Vec<Result<(usize, usize), LimitError>> = r.find_iter("aa").collect();
    assert_eq!(spans.len(), 2);
    // Exhaustive, with typed payloads: a new variant or a re-typed one fails to compile.
    match &r {
        re::Regex::Plain(x) => {
            let _: &regex::Regex = x;
        }
        re::Regex::Fancy(x) => {
            let _: &fancy_regex::Regex = x;
        }
    }

    let _: UnifiedDiff = difflib::unified_diff;
    let _: fn(&str, &str) -> bool = fnmatch::fnmatchcase;
    let _: fn(&str, &str) -> String = quote::quote;
    let _: fn(&str) -> String = quote::unquote;
    let _: fn(&str) -> String = path::normpath;
    let _: fn(&str, &str) -> String = path::join;
    let _: fn(&Path) -> PathBuf = path::realpath;
    let _: fn(&str) -> Option<[u8; 4]> = ipaddress::parse_v4;
    let _: fn(&str) -> Option<[u16; 8]> = ipaddress::parse_v6;
    let _: fn(&str) -> Result<Vec<String>, String> = shlex::split;
    let _: fn(&str) -> String = shlex::quote;

    let e = pyerr::PyExc::OSError {
        errno: 2,
        filename: None,
    };
    let _: &'static str = e.type_name();
    let _: String = e.py_str();
    let _: pyerr::PyExc = std::io::Error::from_raw_os_error(2).into();
    for e in [
        e,
        pyerr::PyExc::ValueError(String::new()),
        pyerr::PyExc::KeyError(String::new()),
        pyerr::PyExc::JSONDecodeError(String::new()),
        pyerr::PyExc::UnicodeDecodeError(String::new()),
        pyerr::PyExc::TimeoutExpired(String::new()),
        pyerr::PyExc::Other("X", String::new()),
    ] {
        match &e {
            pyerr::PyExc::OSError { errno, filename } => {
                let _: (&i32, &Option<String>) = (errno, filename);
            }
            pyerr::PyExc::ValueError(m)
            | pyerr::PyExc::KeyError(m)
            | pyerr::PyExc::JSONDecodeError(m)
            | pyerr::PyExc::UnicodeDecodeError(m)
            | pyerr::PyExc::TimeoutExpired(m) => {
                let _: &String = m;
            }
            pyerr::PyExc::Other(name, m) => {
                let _: (&&'static str, &String) = (name, m);
            }
        }
    }

    let _: Parse = argparse::parse;
}
