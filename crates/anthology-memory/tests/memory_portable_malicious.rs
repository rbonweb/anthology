//! Port of `tests/test_memory_portable.py`'s `MaliciousArchiveTest` (MX1): every case is
//! rejected as a whole, and not one byte is written anywhere (the whole temporary tree is
//! compared before and after).
//!
//! The archives are `anthology_testkit::mx`'s, built as the Python builds them with `tarfile`.
//! The three subTest loops iterate the committed rows of `tests/data/test_memory_portable.jsonl`.
//! `test_still_rejected_when_tarfile_has_no_data_filter` and
//! `test_data_filter_is_applied_when_present` patch `tarfile.data_filter`, a Python-version
//! difference: `dropped:py-compat` (the one refusal of 3.12's filter that `_check_name` lets
//! through, a name too long for its temporary folder, is a unit test in `portable::load`).

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::RealGit;
use anthology_memory::portable::load::CappedReader;
use anthology_memory::portable::{self, Error, load};
use anthology_pycompat::json::{Map, Value};
use anthology_testkit::mem::MemCase;
use anthology_testkit::mx::{self, MIB, Spec, make_archive, tree};
use anthology_testkit::rows::{Row, rows};
use anthology_testkit::run::run_bin;
use sha2::{Digest, Sha256};

const ROWS: &str = "test_memory_portable";

fn py_id(method: &str) -> String {
    format!("test_memory_portable.MaliciousArchiveTest.{method}")
}

/// `PortableCase` as `MaliciousArchiveTest` uses it.
struct Case {
    m: MemCase,
    cwd: PathBuf,
}

impl Case {
    fn new() -> Case {
        let m = MemCase::new();
        let cwd = m.tmp.join("plain");
        fs::create_dir(&cwd).expect("mkdir plain");
        fs::create_dir(m.tmp.join("out")).expect("mkdir out");
        Case { m, cwd }
    }

    fn tmp(&self) -> &Path {
        &self.m.tmp
    }

    /// `second_machine(name)`: an environment with its own empty `HOME` and config folder,
    /// and that machine's anthology home.
    fn second_machine(&self, name: &str) -> (Env, PathBuf) {
        let home = self.tmp().join(name).join("home");
        fs::create_dir_all(&home).expect("mkdir second home");
        let env = self
            .m
            .env
            .clone()
            .with_os("HOME", &home)
            .with_os("CLAUDE_CONFIG_DIR", home.join(".claude"));
        (env, home.join(".claude").join("anthology"))
    }

    fn bad(&self, kind: &str) -> PathBuf {
        mx::bad(self.tmp(), kind)
    }

    /// `assert_rejected(path, label, reason)`.
    fn assert_rejected(&self, path: &Path, label: &str, reason: Option<&str>) {
        let name = path
            .file_name()
            .expect("an archive file name")
            .to_string_lossy()
            .replace('.', "_");
        let (env_b, anth_b) = self.second_machine(&name);
        let before = tree(self.tmp());
        let got = portable::plan_import(path, &self.cwd, None, &env_b, &RealGit);
        let Err(Error::Rejected(message)) = got else {
            panic!("{label}: not rejected: {got:?}");
        };
        if let Some(reason) = reason {
            assert!(message.contains(reason), "{label}: {message:?}");
        }
        assert_eq!(tree(self.tmp()), before, "{label}");
        assert!(tree(&anth_b).is_empty(), "{label}");
    }
}

fn kinds(method: &str) -> Vec<String> {
    rows::<Row>(ROWS, &py_id(method))
        .iter()
        .map(|r| r.str("kind").to_string())
        .collect()
}

mod malicious_archive_test {
    use super::*;

    #[test]
    fn test_every_malicious_archive_is_rejected_and_nothing_is_written() {
        let c = Case::new();
        let kinds = kinds("test_every_malicious_archive_is_rejected_and_nothing_is_written");
        assert_eq!(kinds, mx::KINDS);
        for kind in &kinds {
            let path = c.bad(kind);
            c.assert_rejected(&path, kind, None);
        }
    }

    #[test]
    fn test_rejected_through_the_cli_with_exit_code_2() {
        let c = Case::new();
        for kind in kinds("test_rejected_through_the_cli_with_exit_code_2") {
            let path = c.bad(&kind);
            let (env_b, _anth_b) = c.second_machine(&kind);
            let before = tree(c.tmp());
            let path = path.to_str().expect("a UTF-8 test path");
            let r = run_bin(&["memory", "import", path, "--yes"], b"", &env_b, &c.cwd);
            let (out, err) = (r.stdout_text(), r.stderr_text());
            assert_eq!(r.code, 2, "{kind}: {out}{err}");
            assert!(err.contains("rejected"), "{kind}: {err}");
            assert_eq!(tree(c.tmp()), before, "{kind}");
        }
    }

    #[test]
    fn test_a_hand_forged_plan_pointing_at_a_malicious_archive_is_rejected_by_apply() {
        // apply_import re-validates the archive itself; it never trusts a plan.
        let c = Case::new();
        let (_env_b, anth_b) = c.second_machine("forged");
        let mut good_entry = Map::new();
        for (k, v) in [
            ("member", Value::Str("personal/good.md".into())),
            ("scope", Value::Str("personal".into())),
            ("agent", Value::Null),
            ("name", Value::Str("good.md".into())),
        ] {
            good_entry.insert(k.into(), v);
        }
        let kinds =
            kinds("test_a_hand_forged_plan_pointing_at_a_malicious_archive_is_rejected_by_apply");
        for kind in kinds {
            let path = c.bad(&kind);
            let before = tree(c.tmp());
            let sha: String = Sha256::digest(fs::read(&path).expect("read the archive"))
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let mut plan = Map::new();
            for (k, v) in [
                ("archive", Value::Str(path.to_string_lossy().into_owned())),
                ("sha256", Value::Str(sha)),
                ("date", Value::Str("2026-04-06".into())),
                ("home", Value::Str(anth_b.to_string_lossy().into_owned())),
                ("repo_root", Value::Null),
                ("new", Value::Array(vec![Value::Object(good_entry.clone())])),
                ("identical", Value::Array(vec![])),
                ("conflicts", Value::Array(vec![])),
            ] {
                plan.insert(k.into(), v);
            }
            // Python's apply_import(plan) reads os.environ: the case's environment.
            let got = portable::apply_import_env(&Value::Object(plan), &c.m.env, &RealGit);
            assert!(matches!(got, Err(Error::Rejected(_))), "{kind}: {got:?}");
            assert_eq!(tree(c.tmp()), before, "{kind}");
        }
    }

    #[test]
    fn test_names_with_a_trailing_newline_are_not_note_names() {
        for bad in ["foo\n.md", "foo.md\n", "\nfoo.md"] {
            assert!(!load::ok_note_name(bad), "{bad:?}");
        }
        assert!(!load::name_ok("ag\n"));
        assert!(load::ok_note_name("foo.md"));
    }

    #[test]
    fn test_archive_file_over_50_mib_is_rejected_without_reading_it() {
        let c = Case::new();
        let path = c.tmp().join("huge.tar.gz");
        // Sparse: costs no disk.
        fs::File::create(&path)
            .expect("create")
            .set_len(50 * MIB + 1)
            .expect("truncate");
        c.assert_rejected(&path, "", Some("larger than 50 MiB"));
    }

    #[test]
    fn test_total_size_over_50_mib_single_member() {
        let c = Case::new();
        let path = make_archive(
            &c.tmp().join("inflate.tar.gz"),
            &[("personal/big.md", Spec::Zero(51 * MIB))],
            true,
            true,
        );
        // A small gzip that inflates big.
        assert!(fs::metadata(&path).expect("stat").len() < 5 * MIB);
        c.assert_rejected(&path, "", Some("more than 50 MiB"));
    }

    #[test]
    fn test_total_size_over_50_mib_many_members() {
        let c = Case::new();
        let names: Vec<String> = (0..9).map(|i| format!("personal/part{i}.md")).collect();
        let members: Vec<(&str, Spec)> = names
            .iter()
            .map(|n| (n.as_str(), Spec::Zero(6 * MIB)))
            .collect();
        let path = make_archive(&c.tmp().join("sum.tar.gz"), &members, true, true);
        assert!(fs::metadata(&path).expect("stat").len() < 5 * MIB);
        c.assert_rejected(&path, "", Some("more than 50 MiB"));
    }

    #[test]
    fn test_declared_size_is_checked_before_any_data_is_read() {
        let c = Case::new();
        let path = mx::declared_size_archive(&c.tmp().join("declared.tar.gz"), 5 * 1024 * MIB);
        c.assert_rejected(&path, "", Some("more than 50 MiB"));
    }

    #[test]
    fn test_oversized_extended_header_is_not_read_into_memory() {
        let c = Case::new();
        let path = mx::pax_header_bomb(&c.tmp().join("paxbomb.tar.gz"), 80 * MIB);
        assert!(fs::metadata(&path).expect("stat").len() < 5 * MIB);
        c.assert_rejected(&path, "", Some("size limit"));
    }

    #[test]
    fn test_capped_reader_never_reads_far_past_its_limit() {
        let mut capped = CappedReader::new(Cursor::new(vec![0u8; 1000]), 50);
        // A hostile header asking for a huge read.
        assert!(capped.read(Some(10u64.pow(9))).is_err());
        assert!(capped.get_ref().position() <= 51);
        let mut capped = CappedReader::new(Cursor::new(vec![0u8; 1000]), 50);
        // Read-everything.
        assert!(capped.read(None).is_err());
        assert!(capped.get_ref().position() <= 51);
        assert!(
            CappedReader::new(Cursor::new(vec![0u8; 1000]), 50)
                .seek(51)
                .is_err()
        );
        let mut capped = CappedReader::new(Cursor::new(b"abc".repeat(30)), 50);
        // Normal reads are untouched.
        assert_eq!(capped.read(Some(10)).expect("read"), b"abcabcabca");
    }

    #[test]
    fn test_headers_that_the_member_count_does_not_see_are_capped_too() {
        // Extended headers are consumed by tarfile, not listed as members: the stream cap
        // bounds them.
        let c = Case::new();
        let path = mx::pax_flood(&c.tmp().join("paxflood.tar.gz"), 8000, 8192);
        assert!(fs::metadata(&path).expect("stat").len() < 20 * MIB);
        c.assert_rejected(&path, "", Some("size limit"));
    }

    #[test]
    fn test_more_than_10000_members() {
        let c = Case::new();
        let names: Vec<String> = (0..10001).map(|i| format!("personal/n{i:05}.md")).collect();
        let members: Vec<(&str, Spec)> = names
            .iter()
            .map(|n| (n.as_str(), Spec::File(b"x".to_vec())))
            .collect();
        let path = make_archive(&c.tmp().join("many.tar.gz"), &members, true, false);
        c.assert_rejected(&path, "", Some("more than 10000 members"));
    }

    #[test]
    fn test_exactly_10000_members_is_fine() {
        let c = Case::new();
        let names: Vec<String> = (0..9999).map(|i| format!("personal/n{i:05}.md")).collect();
        let members: Vec<(&str, Spec)> = names
            .iter()
            .map(|n| (n.as_str(), Spec::File(b"x".to_vec())))
            .collect();
        let path = make_archive(&c.tmp().join("ok-many.tar.gz"), &members, true, false);
        let (env_b, _) = c.second_machine("b");
        let plan = portable::plan_import(&path, &c.cwd, None, &env_b, &RealGit).expect("plan");
        let new = plan.get("new").and_then(Value::as_array).expect("new");
        assert_eq!(new.len(), 9999);
    }
}
