//! `Date`, `today_local`, `now_epoch` (plan §3.3): `datetime.date.today()` and `time.time()`.
#![forbid(unsafe_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use crate::sys;

/// A calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u8,
    pub d: u8,
}

impl Date {
    /// `date.isoformat()`: `YYYY-MM-DD`.
    pub fn iso(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

/// The date [`pin_today`] set, packed as `y * 10_000 + m * 100 + d`; 0 is unpinned.
#[cfg(feature = "test-clock")]
static PINNED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Makes [`today_local`] answer `date` (or the real date again for `None`) for the whole
/// process. Only the `test-clock` feature has it: the difftest goldens hold the output of a
/// frozen day, so their replay has to run on that day (a later day writes its own date into
/// `approved`, `migrated-<date>` and `modified`).
#[cfg(feature = "test-clock")]
pub fn pin_today(date: Option<Date>) {
    let packed = date.map_or(0, |d| {
        d.y as u32 * 10_000 + u32::from(d.m) * 100 + u32::from(d.d)
    });
    PINNED.store(packed, std::sync::atomic::Ordering::SeqCst);
}

/// `datetime.date.today()`: the local date of the current second (`localtime_r` after
/// `tzset`, so the process's `TZ` applies). If the C library cannot convert the time, the
/// UTC date.
pub fn today_local() -> Date {
    #[cfg(feature = "test-clock")]
    {
        let packed = PINNED.load(std::sync::atomic::Ordering::SeqCst);
        if packed != 0 {
            return Date {
                y: (packed / 10_000) as i32,
                m: (packed / 100 % 100) as u8,
                d: (packed % 100) as u8,
            };
        }
    }
    let t = now_epoch().floor() as i64;
    match sys::local_date(t) {
        Some((y, m, d)) => Date { y, m, d },
        None => utc_date(t),
    }
}

/// `time.time()`: seconds since the epoch.
pub fn now_epoch() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// The UTC calendar date of an epoch second (proleptic Gregorian; Howard Hinnant's
/// `civil_from_days`).
pub fn utc_date(t: i64) -> Date {
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Date {
        y: i32::try_from(y).unwrap_or(i32::MAX),
        m: u8::try_from(m).unwrap_or(1),
        d: u8::try_from(d).unwrap_or(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_pads_like_isoformat() {
        assert_eq!(
            Date {
                y: 2026,
                m: 1,
                d: 5
            }
            .iso(),
            "2026-01-05"
        );
        assert_eq!(
            Date {
                y: 33,
                m: 12,
                d: 31
            }
            .iso(),
            "0033-12-31"
        );
    }

    #[test]
    fn utc_date_is_the_civil_date() {
        assert_eq!(
            utc_date(0),
            Date {
                y: 1970,
                m: 1,
                d: 1
            }
        );
        assert_eq!(
            utc_date(-1),
            Date {
                y: 1969,
                m: 12,
                d: 31
            }
        );
        assert_eq!(
            utc_date(951_782_400),
            Date {
                y: 2000,
                m: 2,
                d: 29
            }
        );
        assert_eq!(
            utc_date(1_795_219_200),
            Date {
                y: 2026,
                m: 11,
                d: 21
            }
        );
        assert_eq!(
            utc_date(4_107_542_400),
            Date {
                y: 2100,
                m: 3,
                d: 1
            }
        );
    }

    #[test]
    fn now_is_after_2026() {
        assert!(now_epoch() > 1_767_225_600.0);
        assert!(today_local().y >= 2026);
    }
}
