//! `clock::pin_today` (the `test-clock` feature): the difftest golden replay runs Rust on the day
//! the goldens were frozen. Its own test binary, because the pin is process-wide.
#![cfg(feature = "test-clock")]
#![forbid(unsafe_code)]

use anthology_core::clock::{Date, pin_today, today_local};

#[test]
fn a_pinned_day_is_what_today_local_answers_until_it_is_unpinned() {
    // A day no run can be on yet, so the pin always differs from the real one.
    let frozen = Date {
        y: 2099,
        m: 1,
        d: 2,
    };
    assert_ne!(today_local(), frozen);
    pin_today(Some(frozen));
    assert_eq!(today_local(), frozen);
    assert_eq!(today_local().iso(), "2099-01-02");
    pin_today(None);
    assert_ne!(today_local(), frozen);
}
