//! `clock::today_local` under a `TZ` matrix. `TZ` is set on a child copy of this test binary
//! (setting it in-process is `unsafe` in edition 2024), and the child's date is compared with
//! `date +%F` in the same `TZ` and with the UTC date shifted by the zone's offset.
//!
//! The named zones need the tz database. Where it is missing (the slim Rust image), libc and
//! `date` both fall back to UTC, so the POSIX `TZ` strings (no database needed) carry the
//! +14 / -12 offset checks there.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::Command;

use anthology_core::clock::{self, Date};

const CHILD: &str = "ANTHOLOGY_CORE_TZ_CHILD";

/// Runs only in the child: prints the epoch second before and after `today_local()`.
#[test]
fn tz_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let before = clock::now_epoch().floor();
    let today = clock::today_local().iso();
    let after = clock::now_epoch().floor();
    println!("TZCHILD {before} {today} {after}");
}

/// `(epoch before, today_local, epoch after)` in a child with `TZ=tz`.
fn child(tz: &str) -> (i64, String, i64) {
    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .args(["tz_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("TZ", tz)
        .output()
        .expect("run the child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("TZCHILD ").map(|(_, rest)| rest))
        .unwrap_or_else(|| panic!("TZ={tz}: no child output: {stdout}"));
    let parts: Vec<&str> = line.split(' ').collect();
    (
        parts[0].parse().expect("epoch"),
        parts[1].to_string(),
        parts[2].parse().expect("epoch"),
    )
}

fn date_f(tz: &str) -> Option<String> {
    let out = Command::new("date")
        .arg("+%F")
        .env("TZ", tz)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn shifted(epoch: i64, offset_hours: i64) -> String {
    clock::utc_date(epoch + offset_hours * 3600).iso()
}

#[test]
fn today_local_matches_date_in_each_tz() {
    for tz in [
        "UTC",
        "Pacific/Kiritimati",
        "Etc/GMT+12",
        "Not/A_Zone",
        "XST-14",
        "XST+12",
        "UTC0",
    ] {
        let before = date_f(tz);
        let (_, today, _) = child(tz);
        let after = date_f(tz);
        match (before, after) {
            (Some(b), Some(a)) => assert!(today == b || today == a, "TZ={tz}: {today} vs {b}/{a}"),
            _ => eprintln!("TZ={tz}: no `date` here; offset checks only"),
        }
    }
}

#[test]
fn today_local_applies_the_offset() {
    let mut zones = vec![("XST-14", 14), ("XST+12", -12), ("UTC0", 0), ("UTC", 0)];
    if Path::new("/usr/share/zoneinfo/Pacific/Kiritimati").exists() {
        zones.push(("Pacific/Kiritimati", 14));
    }
    if Path::new("/usr/share/zoneinfo/Etc/GMT+12").exists() {
        zones.push(("Etc/GMT+12", -12));
    }
    for (tz, hours) in zones {
        let (before, today, after) = child(tz);
        let want = [shifted(before, hours), shifted(after, hours)];
        assert!(want.contains(&today), "TZ={tz}: {today} vs {want:?}");
    }
    // +14 and -12 are a day or two apart at every moment.
    let (_, east, _) = child("XST-14");
    let (_, west, _) = child("XST+12");
    assert!(east > west, "{east} vs {west}");
}

#[test]
fn an_invalid_tz_is_utc() {
    let (before, today, after) = child("Not/A_Zone");
    assert!([shifted(before, 0), shifted(after, 0)].contains(&today));
}

#[test]
fn iso_is_isoformat() {
    assert_eq!(
        Date {
            y: 2026,
            m: 11,
            d: 21
        }
        .iso(),
        "2026-11-21"
    );
}
