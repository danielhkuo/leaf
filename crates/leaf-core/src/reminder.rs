//! Pure reminder logic: is a series behind schedule, and is a reminder due
//! right now?
//!
//! The bot runs a coarse tick (every minute) and asks this predicate.
//! Because the condition is state-based — *behind, past the reminder time,
//! not yet reminded for this missing day* — at-most-once delivery and
//! downtime catch-up need no cron bookkeeping: a missed window stays "due"
//! until sent, and a sent day stays recorded until the day is archived.

use chrono::{DateTime, Datelike as _, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

use crate::domain::Cadence;

/// A reminder-enabled series joined with its post aggregates and resolved
/// timezone, as produced by `SeriesRepo::reminder_candidates`. Owned so the
/// scheduler can hold a batch without borrowing the pool.
#[derive(Debug, Clone)]
pub struct ReminderCandidate {
    /// Series id.
    pub series_id: i64,
    /// Guild snowflake (for the message destination / logging).
    pub guild_id: String,
    /// Series name.
    pub name: String,
    /// Creator snowflake (DM target and `{creator}` substitution).
    pub creator_id: String,
    /// Watched channels; a channel reminder pings the first.
    pub channels: Vec<String>,
    /// Posting cadence.
    pub cadence: Cadence,
    /// Reminder time of day, `HH:MM` (guaranteed present by the query).
    pub reminder_time: String,
    /// Resolved timezone: series override, else guild default.
    pub timezone: String,
    /// Remind via DM (true) or channel ping (false).
    pub reminder_dm: bool,
    /// First archived day (so an empty series is recognised).
    pub start_day: i64,
    /// Highest archived day, if any.
    pub max_day: Option<i64>,
    /// `posted_at` of the newest post, if any.
    pub last_post_at: Option<i64>,
    /// Last day a reminder was sent for.
    pub last_reminder_day: Option<i64>,
}

impl ReminderCandidate {
    /// The day a reminder would name (`max_day + 1`, else `start_day`).
    #[must_use]
    pub fn expected_day(&self) -> i64 {
        self.max_day.map_or(self.start_day, |d| d + 1)
    }

    /// Borrows the candidate as predicate inputs for `now_unix`.
    #[must_use]
    pub fn inputs(&self, now_unix: i64) -> ReminderInputs<'_> {
        ReminderInputs {
            cadence: self.cadence,
            reminder_time: &self.reminder_time,
            timezone: &self.timezone,
            last_post_at: self.last_post_at,
            expected_day: self.expected_day(),
            last_reminder_day: self.last_reminder_day,
            now_unix,
        }
    }
}

/// Everything the predicate needs about one series at one instant.
#[derive(Debug, Clone)]
pub struct ReminderInputs<'a> {
    /// Posting cadence ([`Cadence::Freeform`] never reminds).
    pub cadence: Cadence,
    /// Reminder time of day, `HH:MM` (series-local).
    pub reminder_time: &'a str,
    /// IANA timezone (series override, else guild default).
    pub timezone: &'a str,
    /// `posted_at` of the newest archived post; `None` = empty series.
    pub last_post_at: Option<i64>,
    /// The day a reminder would name (`max_day + 1`).
    pub expected_day: i64,
    /// Last day number a reminder was sent for, if any.
    pub last_reminder_day: Option<i64>,
    /// Now, unix seconds.
    pub now_unix: i64,
}

/// True when a reminder should be sent right now.
#[must_use]
pub fn reminder_due(i: &ReminderInputs<'_>) -> bool {
    // Empty series never remind: there is no rhythm to fall behind.
    let Some(last_post_at) = i.last_post_at else {
        return false;
    };
    // Already nudged about this exact missing day.
    if i.last_reminder_day == Some(i.expected_day) {
        return false;
    }

    let tz: Tz = i.timezone.parse().unwrap_or(chrono_tz::UTC);
    let Some(now) = DateTime::<Utc>::from_timestamp(i.now_unix, 0) else {
        return false;
    };
    let now_local = now.with_timezone(&tz);

    // Not yet the appointed hour.
    let Ok(at) = NaiveTime::parse_from_str(i.reminder_time, "%H:%M") else {
        return false;
    };
    if now_local.time() < at {
        return false;
    }

    let Some(last) = DateTime::<Utc>::from_timestamp(last_post_at, 0) else {
        return false;
    };
    behind(i.cadence, &last.with_timezone(&tz), &now_local)
}

/// Cadence-aware "has the series missed its rhythm as of `now`?".
fn behind(cadence: Cadence, last_post: &DateTime<Tz>, now: &DateTime<Tz>) -> bool {
    match cadence {
        Cadence::Freeform => false,
        Cadence::Daily => last_post.date_naive() < now.date_naive(),
        Cadence::Weekdays => {
            !matches!(now.weekday(), Weekday::Sat | Weekday::Sun)
                && last_post.date_naive() < now.date_naive()
        }
        // "Weekly" means once per ISO calendar week (Mon–Sun): a new week
        // with no post yet is behind, regardless of how recent the last
        // post was. Posting Sunday then being nudged Monday is intended.
        Cadence::Weekly => {
            let (ly, lw) = (last_post.iso_week().year(), last_post.iso_week().week());
            let (ny, nw) = (now.iso_week().year(), now.iso_week().week());
            (ly, lw) < (ny, nw)
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    /// Tuesday 2026-06-09 18:00 Chicago, as unix.
    fn tue_18() -> i64 {
        chrono_tz::America::Chicago
            .with_ymd_and_hms(2026, 6, 9, 18, 0, 0)
            .unwrap()
            .timestamp()
    }

    fn inputs(last_offset_days: i64) -> ReminderInputs<'static> {
        ReminderInputs {
            cadence: Cadence::Daily,
            reminder_time: "17:30",
            timezone: "America/Chicago",
            last_post_at: Some(tue_18() - last_offset_days * 86_400),
            expected_day: 10,
            last_reminder_day: None,
            now_unix: tue_18(),
        }
    }

    #[test]
    fn daily_behind_past_time_is_due() {
        assert!(reminder_due(&inputs(1)));
    }

    #[test]
    fn posted_today_is_not_due() {
        assert!(!reminder_due(&inputs(0)));
    }

    #[test]
    fn before_reminder_time_is_not_due() {
        let mut i = inputs(1);
        i.now_unix = tue_18() - 3600; // 17:00 < 17:30
        assert!(!reminder_due(&i));
    }

    #[test]
    fn at_most_once_per_missing_day() {
        let mut i = inputs(1);
        i.last_reminder_day = Some(10);
        assert!(!reminder_due(&i));
        // A different (older) reminded day does not block.
        i.last_reminder_day = Some(9);
        assert!(reminder_due(&i));
    }

    #[test]
    fn empty_series_and_freeform_never_remind() {
        let mut i = inputs(1);
        i.last_post_at = None;
        assert!(!reminder_due(&i));
        let mut i = inputs(3);
        i.cadence = Cadence::Freeform;
        assert!(!reminder_due(&i));
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let sat = chrono_tz::America::Chicago
            .with_ymd_and_hms(2026, 6, 13, 18, 0, 0)
            .unwrap()
            .timestamp();
        let mut i = inputs(2);
        i.cadence = Cadence::Weekdays;
        i.now_unix = sat;
        assert!(!reminder_due(&i)); // Saturday: silent
        i.now_unix = sat + 2 * 86_400; // Monday
        assert!(reminder_due(&i));
    }

    #[test]
    fn weekly_compares_iso_weeks() {
        // now = Tue 2026-06-09 (ISO week Mon 06-08 .. Sun 06-14).
        // Posted Mon 06-08 (offset 1) → same ISO week → not behind.
        let mut i = inputs(1);
        i.cadence = Cadence::Weekly;
        assert!(!reminder_due(&i));
        // Posted Tue 06-02 (offset 7) → previous ISO week → behind.
        let mut i = inputs(7);
        i.cadence = Cadence::Weekly;
        assert!(reminder_due(&i));
    }

    #[test]
    fn bad_timezone_falls_back_to_utc_and_bad_time_disables() {
        let mut i = inputs(1);
        i.timezone = "Not/AZone";
        assert!(reminder_due(&i)); // 23:00 UTC ≥ 17:30 — still due under UTC
        let mut i = inputs(1);
        i.reminder_time = "25:99";
        assert!(!reminder_due(&i));
    }

    #[test]
    fn downtime_catchup_is_structural() {
        // Bot was down at 17:30; it is now 22:00 — still due, because the
        // condition is state, not a fired cron slot.
        let mut i = inputs(1);
        i.now_unix = tue_18() + 4 * 3600;
        assert!(reminder_due(&i));
    }
}
