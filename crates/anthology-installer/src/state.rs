//! The installer's own files in `<Claude config>/anthology/`: the lock that stops two runs at
//! once, and `installer.json`, the record of what the last run did.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Int, Map, Value};

use crate::proc::{Cmd, Runner};

pub const LOCK_FILE: &str = "installer.lock";
pub const STATE_FILE: &str = "installer.json";

/// Held while a run changes things; the file goes when this is dropped.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    /// Takes `<ah>/installer.lock`. A lock whose process is gone (a crash) is taken over.
    pub fn acquire(ah: &Path, runner: &dyn Runner) -> Result<Lock, String> {
        fs::create_dir_all(ah).map_err(|e| format!("cannot create {}: {e}", ah.display()))?;
        let path = ah.join(LOCK_FILE);
        for _ in 0..2 {
            let created = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path);
            match created {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())
                        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                    return Ok(Lock { path });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let pid = fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok());
                    if let Some(pid) = pid {
                        // If `kill` cannot run there is no way to know, so assume it is alive.
                        let alive = runner
                            .run(&Cmd::new("kill", &["-0", &pid.to_string()]))
                            .is_ok_and(|out| out.success());
                        if alive {
                            return Err(format!(
                                "another installer run is already running (pid {pid}); if it is \
                                 not, delete {}",
                                path.display()
                            ));
                        }
                    }
                    // A dead process, or no pid in the file: left over from a crash.
                    let _ = fs::remove_file(&path);
                }
                Err(e) => return Err(format!("cannot take {}: {e}", path.display())),
            }
        }
        Err(format!("cannot take {}", path.display()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// `2023-11-14T22:13:20Z` for a count of seconds since 1970 (UTC).
pub fn iso_utc(secs: u64) -> String {
    // Howard Hinnant's days-to-civil algorithm, on days counted from 1970-01-01.
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    iso_utc(secs)
}

/// What the last install run recorded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub installer_version: String,
    pub updated_at: String,
    /// The marketplace source, when it is ours.
    pub marketplace: Option<String>,
    /// The installer (this run or an earlier one) added the marketplace.
    pub marketplace_added: bool,
    /// `(id, version)` of the three plugins.
    pub plugins: Vec<(String, String)>,
    /// `present` or `installed`.
    pub binary: String,
    /// `applied`, `current`, `declined` or `not-run`.
    pub setup: String,
}

impl State {
    pub fn to_json(&self) -> String {
        let string = |s: &str| Value::Str(s.to_string());
        let mut marketplace = Map::new();
        marketplace.insert(
            "source".into(),
            self.marketplace.as_deref().map_or(Value::Null, string),
        );
        marketplace.insert(
            "addedByInstaller".into(),
            Value::Bool(self.marketplace_added),
        );
        let mut plugins = Map::new();
        for (id, version) in &self.plugins {
            plugins.insert(id.clone(), string(version));
        }
        let mut doc = Map::new();
        doc.insert("schema".into(), Value::Int(Int::Small(1)));
        doc.insert("installer".into(), string(&self.installer_version));
        doc.insert("updatedAt".into(), string(&self.updated_at));
        doc.insert("marketplace".into(), Value::Object(marketplace));
        doc.insert("plugins".into(), Value::Object(plugins));
        doc.insert("binary".into(), string(&self.binary));
        doc.insert("setup".into(), string(&self.setup));
        json::dumps(&Value::Object(doc), &json::INDENT2_SORTED) + "\n"
    }

    pub fn from_json(text: &str) -> Option<State> {
        let doc = json::loads(text).ok()?;
        let doc = doc.as_object()?;
        let text_of = |m: &Map, key: &str| m.get(key).and_then(Value::as_str).map(str::to_string);
        let marketplace = doc.get("marketplace").and_then(Value::as_object);
        let mut plugins: Vec<(String, String)> = doc
            .get("plugins")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(id, v)| Some((id.to_string(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        plugins.sort();
        Some(State {
            installer_version: text_of(doc, "installer")?,
            updated_at: text_of(doc, "updatedAt")?,
            marketplace: marketplace.and_then(|m| text_of(m, "source")),
            marketplace_added: marketplace
                .and_then(|m| m.get("addedByInstaller"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            plugins,
            binary: text_of(doc, "binary").unwrap_or_default(),
            setup: text_of(doc, "setup").unwrap_or_default(),
        })
    }
}

pub fn read(ah: &Path) -> Option<State> {
    State::from_json(&fs::read_to_string(ah.join(STATE_FILE)).ok()?)
}

/// Writes `installer.json` through a temporary file, so a crash never leaves half of it.
pub fn write(ah: &Path, state: &State) -> io::Result<PathBuf> {
    fs::create_dir_all(ah)?;
    let tmp = ah.join(format!("{STATE_FILE}.tmp-{}", std::process::id()));
    fs::write(&tmp, state.to_json())?;
    let path = ah.join(STATE_FILE);
    fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Script;

    #[test]
    fn iso_utc_formats_known_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        // A leap day, and the end of a year.
        assert_eq!(iso_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_767_225_599), "2025-12-31T23:59:59Z");
    }

    fn sample() -> State {
        State {
            installer_version: "0.1.0".into(),
            updated_at: "2026-11-26T10:00:00Z".into(),
            marketplace: Some("rbonweb/anthology".into()),
            marketplace_added: true,
            // Sorted by id, which is how the file reads back.
            plugins: vec![
                ("anthology-guardrails@anthology".into(), "0.2.15".into()),
                ("anthology@anthology".into(), "0.5.5".into()),
            ],
            binary: "installed".into(),
            setup: "applied".into(),
        }
    }

    #[test]
    fn state_round_trips_through_json() {
        let state = sample();
        assert_eq!(State::from_json(&state.to_json()), Some(state));
        let none = State {
            marketplace: None,
            ..sample()
        };
        assert_eq!(State::from_json(&none.to_json()), Some(none));
    }

    #[test]
    fn state_json_is_sorted_and_readable() {
        let text = sample().to_json();
        assert!(text.contains("\"installer\": \"0.1.0\""), "{text}");
        assert!(
            text.contains("\"updatedAt\": \"2026-11-26T10:00:00Z\""),
            "{text}"
        );
        assert!(text.ends_with("}\n"), "{text}");
    }

    #[test]
    fn unreadable_state_is_none() {
        assert_eq!(State::from_json("not json"), None);
        assert_eq!(State::from_json("[]"), None);
        assert_eq!(State::from_json("{}"), None);
    }

    #[test]
    fn write_then_read_replaces_the_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        let ah = dir.path().join("anthology");
        let path = write(&ah, &sample()).unwrap();
        assert_eq!(path, ah.join("installer.json"));
        assert_eq!(read(&ah), Some(sample()));
        let second = State {
            binary: "present".into(),
            ..sample()
        };
        write(&ah, &second).unwrap();
        assert_eq!(read(&ah), Some(second));
        let leftovers: Vec<_> = fs::read_dir(&ah)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, ["installer.json"]);
    }

    #[test]
    fn a_second_run_is_refused_while_the_first_lives() {
        let dir = tempfile::tempdir().unwrap();
        let ah = dir.path().join("anthology");
        let none = Script::new();
        let first = Lock::acquire(&ah, &none).unwrap();
        assert!(ah.join(LOCK_FILE).is_file());
        // The second run asks whether the recorded pid is alive.
        let alive = Script::new().ok("kill -0", "");
        let err = Lock::acquire(&ah, &alive).err().unwrap();
        assert!(err.contains("already running"), "{err}");
        assert!(err.contains(&std::process::id().to_string()), "{err}");
        drop(first);
        assert!(!ah.join(LOCK_FILE).exists(), "dropping the lock removes it");
        assert!(Lock::acquire(&ah, &none).is_ok());
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let ah = dir.path().join("anthology");
        fs::create_dir_all(&ah).unwrap();
        fs::write(ah.join(LOCK_FILE), "424242\n").unwrap();
        let dead = Script::new().exit("kill -0 424242", 1, "", "no such process");
        let lock = Lock::acquire(&ah, &dead).unwrap();
        let pid = fs::read_to_string(ah.join(LOCK_FILE)).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        drop(lock);
    }

    #[test]
    fn a_lock_with_no_readable_pid_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let ah = dir.path().join("anthology");
        fs::create_dir_all(&ah).unwrap();
        fs::write(ah.join(LOCK_FILE), "").unwrap();
        assert!(Lock::acquire(&ah, &Script::new()).is_ok());
    }
}
