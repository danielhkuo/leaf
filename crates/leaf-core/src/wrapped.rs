//! Pure logic for the `/wrapped` yearly recap: bucket a series' posts by a
//! timezone-local calendar and surface the year's headline numbers.
//!
//! Discord-free and clock-free (the year and timezone are passed in), so
//! the month bucketing and "busiest month" tie-breaking are table-tested.

use chrono::{DateTime, Datelike as _};
use chrono_tz::Tz;

/// One post, reduced to what the recap needs.
#[derive(Debug, Clone, Copy)]
pub struct WrappedPost {
    /// Day number.
    pub day: i64,
    /// Original post time, unix seconds.
    pub posted_at: i64,
}

/// Headline numbers for one calendar year of a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    /// The year summarised.
    pub year: i32,
    /// Posts archived in that year (timezone-local).
    pub posts_in_year: i64,
    /// First and last day numbers posted that year, if any.
    pub first_day: Option<i64>,
    /// Last day number posted that year.
    pub last_day: Option<i64>,
    /// Busiest month as (1–12, count); `None` if the year is empty.
    pub busiest_month: Option<(u32, i64)>,
    /// Longest run of consecutive day numbers within the year.
    pub longest_streak: i64,
    /// All-time total posts in the series (context for the year).
    pub total_all_time: i64,
}

/// English month name for 1–12 (out-of-range yields "?").
#[must_use]
pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES
        .get((month as usize).wrapping_sub(1))
        .copied()
        .unwrap_or("?")
}

/// Summarises `posts` for `year` in timezone `tz`. `posts` need not be
/// sorted. `tz` buckets each `posted_at` into a local calendar date.
#[must_use]
pub fn summarize(posts: &[WrappedPost], year: i32, tz: Tz) -> Wrapped {
    let total_all_time = i64::try_from(posts.len()).unwrap_or(i64::MAX);

    // Day numbers posted in the target year, ascending, plus month tallies.
    let mut year_days: Vec<i64> = Vec::new();
    let mut months = [0_i64; 12];
    for p in posts {
        let Some(dt) = DateTime::from_timestamp(p.posted_at, 0) else {
            continue;
        };
        let local = dt.with_timezone(&tz);
        if local.year() == year {
            year_days.push(p.day);
            let idx = (local.month() as usize).saturating_sub(1);
            if let Some(slot) = months.get_mut(idx) {
                *slot += 1;
            }
        }
    }
    year_days.sort_unstable();

    let posts_in_year = i64::try_from(year_days.len()).unwrap_or(i64::MAX);
    let first_day = year_days.first().copied();
    let last_day = year_days.last().copied();

    // Busiest month: highest count, earliest month wins ties. `None` when
    // the year had no posts.
    let busiest_month = months
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, count)| count > 0)
        .max_by_key(|&(idx, count)| (count, std::cmp::Reverse(idx)))
        .map(|(idx, count)| (u32::try_from(idx).unwrap_or(0) + 1, count));

    Wrapped {
        year,
        posts_in_year,
        first_day,
        last_day,
        busiest_month,
        longest_streak: longest_run(&year_days),
        total_all_time,
    }
}

/// Longest run of consecutive integers in an ascending, de-duplicated-ish
/// slice (duplicates don't extend a run).
fn longest_run(days: &[i64]) -> i64 {
    let mut longest = 0_i64;
    let mut run = 0_i64;
    let mut prev: Option<i64> = None;
    for &d in days {
        run = match prev {
            Some(p) if d == p + 1 => run + 1,
            Some(p) if d == p => run, // duplicate: ignore
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(d);
    }
    longest
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    fn at(tz: Tz, y: i32, m: u32, d: u32) -> i64 {
        tz.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap().timestamp()
    }

    fn post(day: i64, posted_at: i64) -> WrappedPost {
        WrappedPost { day, posted_at }
    }

    #[test]
    fn empty_year_is_blank_but_counts_all_time() {
        let tz = chrono_tz::UTC;
        let posts = [post(1, at(tz, 2024, 5, 1))];
        let w = summarize(&posts, 2025, tz);
        assert_eq!(w.posts_in_year, 0);
        assert_eq!(w.busiest_month, None);
        assert_eq!(w.longest_streak, 0);
        assert_eq!(w.total_all_time, 1); // the 2024 post still counts overall
        assert_eq!((w.first_day, w.last_day), (None, None));
    }

    #[test]
    fn buckets_by_year_and_finds_busiest_month() {
        let tz = chrono_tz::UTC;
        let posts = [
            post(1, at(tz, 2025, 1, 10)),
            post(2, at(tz, 2025, 3, 1)),
            post(3, at(tz, 2025, 3, 15)),
            post(4, at(tz, 2025, 3, 20)),
            post(5, at(tz, 2025, 7, 4)),
            post(99, at(tz, 2024, 12, 31)), // prior year, excluded
        ];
        let w = summarize(&posts, 2025, tz);
        assert_eq!(w.posts_in_year, 5);
        assert_eq!(w.busiest_month, Some((3, 3))); // March, 3 posts
        assert_eq!((w.first_day, w.last_day), (Some(1), Some(5)));
        assert_eq!(w.total_all_time, 6);
    }

    #[test]
    fn busiest_month_breaks_ties_toward_the_earlier_month() {
        let tz = chrono_tz::UTC;
        let posts = [
            post(1, at(tz, 2025, 2, 1)),
            post(2, at(tz, 2025, 2, 2)),
            post(3, at(tz, 2025, 9, 1)),
            post(4, at(tz, 2025, 9, 2)),
        ];
        let w = summarize(&posts, 2025, tz);
        assert_eq!(w.busiest_month, Some((2, 2))); // Feb, not Sep
    }

    #[test]
    fn timezone_shifts_a_post_across_a_year_boundary() {
        // 2025-01-01 04:00 UTC is still 2024-12-31 in Chicago (UTC-6).
        let utc = chrono_tz::UTC;
        let ts = utc
            .with_ymd_and_hms(2025, 1, 1, 4, 0, 0)
            .unwrap()
            .timestamp();
        let posts = [post(1, ts)];
        assert_eq!(summarize(&posts, 2025, chrono_tz::UTC).posts_in_year, 1);
        assert_eq!(
            summarize(&posts, 2024, chrono_tz::America::Chicago).posts_in_year,
            1
        );
    }

    #[test]
    fn longest_streak_within_year_only() {
        let tz = chrono_tz::UTC;
        // Days 1,2,3 consecutive in-year; 10 isolated; a 2024 day 4 must not
        // bridge the 3→(4)→... run across the year boundary.
        let posts = [
            post(4, at(tz, 2024, 12, 31)),
            post(1, at(tz, 2025, 1, 1)),
            post(2, at(tz, 2025, 1, 2)),
            post(3, at(tz, 2025, 1, 3)),
            post(10, at(tz, 2025, 6, 1)),
        ];
        let w = summarize(&posts, 2025, tz);
        assert_eq!(w.longest_streak, 3);
    }

    #[test]
    fn month_names() {
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(12), "December");
        assert_eq!(month_name(0), "?");
        assert_eq!(month_name(13), "?");
    }
}
