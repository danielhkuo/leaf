//! Streak and coverage statistics over a series' archived day numbers.
//!
//! Day-number based: a "streak" is a run of consecutive day numbers,
//! independent of calendar dates (cadence-aware calendar stats are the
//! reminder service's concern, not this module's).

/// Aggregate statistics for one series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SeriesStats {
    /// Total archived days.
    pub total: i64,
    /// Length of the consecutive run ending at the highest archived day.
    pub current_streak: i64,
    /// Length of the longest consecutive run anywhere in the series.
    pub longest_streak: i64,
    /// Days in `[start_day, max_day]` with no archive. Zero when empty.
    pub missed: i64,
    /// Highest archived day, if any.
    pub max_day: Option<i64>,
}

/// Computes [`SeriesStats`] from `days`.
///
/// `days` must be **sorted ascending and unique** (as returned by
/// `PostRepo::all_days`). `start_day` is the series' configured first day;
/// days below it still count toward streaks but never toward `missed`.
#[must_use]
pub fn compute(days: &[i64], start_day: i64) -> SeriesStats {
    let Some((&max_day, _)) = days.split_last() else {
        return SeriesStats {
            total: 0,
            current_streak: 0,
            longest_streak: 0,
            missed: 0,
            max_day: None,
        };
    };

    let total = i64::try_from(days.len()).unwrap_or(i64::MAX);

    let mut longest: i64 = 0;
    let mut run: i64 = 0;
    let mut prev: Option<i64> = None;
    for &d in days {
        run = match prev {
            Some(p) if d == p + 1 => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(d);
    }
    // The loop ends on the highest day, so `run` is the current streak.
    let current = run;

    let expected = (max_day - start_day + 1).max(0);
    let counted_in_window = days.iter().filter(|&&d| d >= start_day).count();
    let counted = i64::try_from(counted_in_window).unwrap_or(i64::MAX);
    let missed = (expected - counted).max(0);

    SeriesStats {
        total,
        current_streak: current,
        longest_streak: longest,
        missed,
        max_day: Some(max_day),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(days: &[i64]) -> SeriesStats {
        compute(days, 1)
    }

    #[test]
    fn empty_series() {
        assert_eq!(
            stats(&[]),
            SeriesStats {
                total: 0,
                current_streak: 0,
                longest_streak: 0,
                missed: 0,
                max_day: None
            }
        );
    }

    #[test]
    fn single_day() {
        let s = stats(&[1]);
        assert_eq!(
            (s.total, s.current_streak, s.longest_streak, s.missed),
            (1, 1, 1, 0)
        );
        assert_eq!(s.max_day, Some(1));
    }

    #[test]
    fn unbroken_run() {
        let s = stats(&[1, 2, 3, 4, 5]);
        assert_eq!(
            (s.total, s.current_streak, s.longest_streak, s.missed),
            (5, 5, 5, 0)
        );
    }

    #[test]
    fn gap_resets_current_but_keeps_longest() {
        // 1..=4 then 7,8: longest is 4, current is 2, missed are 5 and 6.
        let s = stats(&[1, 2, 3, 4, 7, 8]);
        assert_eq!((s.current_streak, s.longest_streak, s.missed), (2, 4, 2));
    }

    #[test]
    fn longest_run_at_the_end() {
        let s = stats(&[1, 5, 6, 7]);
        assert_eq!((s.current_streak, s.longest_streak, s.missed), (3, 3, 3));
    }

    #[test]
    fn isolated_days_everywhere() {
        let s = stats(&[2, 4, 6, 8]);
        assert_eq!(
            (s.total, s.current_streak, s.longest_streak, s.missed),
            (4, 1, 1, 4)
        );
    }

    #[test]
    fn start_day_offsets_missed_window() {
        // Series declared to start at day 100; days 100..=102 plus 104.
        let s = compute(&[100, 101, 102, 104], 100);
        assert_eq!(
            (s.total, s.current_streak, s.longest_streak, s.missed),
            (4, 1, 3, 1)
        );
    }

    #[test]
    fn days_before_start_day_count_for_streaks_not_missed() {
        // Backfilled days below start_day: streaks still measure them,
        // the missed window does not.
        let s = compute(&[98, 99, 100, 101], 100);
        assert_eq!(
            (s.total, s.current_streak, s.longest_streak, s.missed),
            (4, 4, 4, 0)
        );
    }

    #[test]
    fn multi_year_scale() {
        let days: Vec<i64> = (1..=1095).collect();
        let s = stats(&days);
        assert_eq!(
            (s.total, s.current_streak, s.longest_streak, s.missed),
            (1095, 1095, 1095, 0)
        );
        assert_eq!(s.max_day, Some(1095));
    }
}
