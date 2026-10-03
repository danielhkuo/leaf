//! Timezone names and calendar dates, shared by every surface.
//!
//! One rule for "which date did this post land on": the instant is read in
//! the guild's timezone. The gallery calendar, `/wrapped` and reminders all
//! go through here so chat and the Activity agree, and every viewer sees the
//! same calendar regardless of their device.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

use chrono::{DateTime, Utc};
pub use chrono_tz::Tz;

/// Resolves a timezone name, ignoring ASCII case and surrounding whitespace
/// (`america/chicago` → `America/Chicago`).
///
/// Accepts every IANA name chrono-tz knows, including the legacy aliases
/// browsers still report (`Asia/Calcutta`, `US/Central`). `None` for
/// anything else, including an empty string and abbreviations such as `CST`.
#[must_use]
pub fn parse_tz(name: &str) -> Option<Tz> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Exact names are the common case and parse without a scan.
    name.parse().ok().or_else(|| {
        chrono_tz::TZ_VARIANTS
            .iter()
            .copied()
            .find(|tz| tz.name().eq_ignore_ascii_case(name))
    })
}

/// How many distinct unknown names [`tz_or_utc`] reports before going quiet.
const WARNED_MAX: usize = 32;
/// How much of an unknown name is kept and logged.
const WARNED_NAME_CHARS: usize = 64;

/// As [`parse_tz`], falling back to UTC for an unknown name.
///
/// The fallback is logged once per distinct name per process (the scheduler
/// asks every minute): stored names are validated on save, so hitting it
/// means an older row or a hand-edited database.
#[must_use]
pub fn tz_or_utc(name: &str) -> Tz {
    static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
    parse_tz(name).unwrap_or_else(|| {
        let shown: String = name.trim().chars().take(WARNED_NAME_CHARS).collect();
        let mut warned = WARNED.lock().unwrap_or_else(PoisonError::into_inner);
        if first_sighting(&mut warned, &shown) {
            tracing::warn!(
                timezone = %shown,
                "unknown timezone; using UTC (reported once per name)"
            );
        }
        chrono_tz::UTC
    })
}

/// Records `name` in `seen` and says whether it is new. Once `seen` holds
/// [`WARNED_MAX`] names nothing more is added, so a database full of bad
/// zones cannot grow the set or flood the log.
fn first_sighting(seen: &mut HashSet<String>, name: &str) -> bool {
    seen.len() < WARNED_MAX && seen.insert(name.to_owned())
}

/// Timestamps `local_date` formats as they are: 0001-01-02 to 9999-12-30
/// UTC, a day inside the four-digit years so no zone offset can leave them.
const DATE_RANGE_UNIX: std::ops::RangeInclusive<i64> = -62_135_510_400..=253_402_214_399;

/// The calendar date of `unix` (seconds) in `tz`, as `YYYY-MM-DD`.
///
/// A timestamp outside years 1–9999 reads as the epoch rather than failing
/// or breaking the format.
#[must_use]
pub fn local_date(unix: i64, tz: Tz) -> String {
    let unix = if DATE_RANGE_UNIX.contains(&unix) {
        unix
    } else {
        0
    };
    DateTime::<Utc>::from_timestamp(unix, 0)
        .unwrap_or_default()
        .with_timezone(&tz)
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use chrono::TimeZone as _;

    use super::*;

    #[test]
    fn names_resolve_case_insensitively() {
        for (input, canonical) in [
            ("America/Chicago", "America/Chicago"),
            ("america/chicago", "America/Chicago"),
            ("  EUROPE/london ", "Europe/London"),
            ("utc", "UTC"),
            ("Asia/Calcutta", "Asia/Calcutta"),
            ("us/central", "US/Central"),
            (
                "america/argentina/buenos_aires",
                "America/Argentina/Buenos_Aires",
            ),
        ] {
            assert_eq!(parse_tz(input).map(Tz::name), Some(canonical), "{input}");
        }
    }

    #[test]
    fn unknown_names_do_not_resolve() {
        for input in ["", "   ", "Chicago", "Not/AZone", "America/Chicago!"] {
            assert_eq!(parse_tz(input), None, "{input}");
        }
        assert_eq!(tz_or_utc("Not/AZone"), chrono_tz::UTC);
        assert_eq!(tz_or_utc("america/chicago"), chrono_tz::America::Chicago);
    }

    #[test]
    fn each_unknown_name_is_reported_once_up_to_the_cap() {
        let mut seen = HashSet::new();
        assert!(first_sighting(&mut seen, "CST"));
        assert!(!first_sighting(&mut seen, "CST"));
        // A second bad zone is reported too, not hidden by the first.
        assert!(first_sighting(&mut seen, "Chicago"));
        assert!(!first_sighting(&mut seen, "Chicago"));

        for n in seen.len()..WARNED_MAX {
            assert!(first_sighting(&mut seen, &format!("Bad/Zone{n}")));
        }
        assert!(!first_sighting(&mut seen, "One/TooMany"));
        assert_eq!(seen.len(), WARNED_MAX);
    }

    #[test]
    fn date_is_read_in_the_given_zone() {
        // 19:30 on 9 June in Chicago is already 10 June in UTC and London.
        let evening = chrono_tz::America::Chicago
            .with_ymd_and_hms(2026, 6, 9, 19, 30, 0)
            .unwrap()
            .timestamp();
        assert_eq!(
            local_date(evening, chrono_tz::America::Chicago),
            "2026-06-09"
        );
        assert_eq!(local_date(evening, chrono_tz::UTC), "2026-06-10");
        assert_eq!(local_date(evening, chrono_tz::Europe::London), "2026-06-10");
        assert_eq!(local_date(evening, chrono_tz::Asia::Tokyo), "2026-06-10");
    }

    #[test]
    fn date_is_zero_padded_and_total() {
        assert_eq!(local_date(0, chrono_tz::UTC), "1970-01-01");
        // Just before midnight UTC on New Year's Eve is still the old year
        // west of Greenwich.
        assert_eq!(
            local_date(1_767_225_600 - 1, chrono_tz::America::Chicago),
            "2025-12-31"
        );
        // Absurd timestamps keep the format instead of panicking.
        for unix in [i64::MAX, i64::MIN, 253_402_300_800, -62_135_596_801] {
            assert_eq!(local_date(unix, chrono_tz::Asia::Tokyo), "1970-01-01");
        }
        assert_eq!(
            local_date(253_402_214_399, chrono_tz::Pacific::Kiritimati),
            "9999-12-31"
        );
        assert_eq!(local_date(-62_135_510_400, chrono_tz::UTC), "0001-01-02");
    }
}
