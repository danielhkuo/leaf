//! Pure reminder logic: is a series behind schedule, and is a reminder due
//! right now?
//!
//! The bot runs a coarse tick (about every minute) and asks this predicate.
//! Because the condition is state-based — *behind, inside the window that
//! opens at the reminder time, not yet reminded for this missing day* —
//! at-most-once delivery and downtime catch-up need no cron bookkeeping: a
//! sent day stays recorded until the day is archived, and a window missed
//! entirely (bot down, reminders switched on late in the day) leaves no
//! mark, so the nudge goes out at the reminder time on the next due day
//! instead of at whatever hour the bot happens to notice.
//!
//! The window belongs to the local day its reminder time falls on, and
//! "behind" is judged for that day even when the window runs a little past
//! midnight. No window is shorter than [`MIN_DUE_WINDOW_SECS`], so the
//! predicate does not depend on the tick landing in any particular minute.

use chrono::{DateTime, Datelike as _, Days, NaiveDate, NaiveTime, Timelike as _, Utc, Weekday};

use crate::domain::Cadence;
use crate::localtime;

/// How long after the reminder time a nudge may still be sent.
///
/// Long enough to ride out a restart or an outage, short enough that an
/// evening reminder never turns into a late-night one. The window also ends
/// at local midnight, whichever comes first, but is never shorter than
/// [`MIN_DUE_WINDOW_SECS`].
pub const DUE_WINDOW_SECS: i64 = 6 * 3600;

/// The shortest a due window gets.
///
/// Ending the window at midnight would leave a 23:59 reminder one minute,
/// which a scheduler tick can step over. A reminder set for the last hour of
/// the day therefore keeps a full hour, running past midnight; it is still
/// about the day it was set for.
pub const MIN_DUE_WINDOW_SECS: i64 = 3600;

const DAY_SECS: i64 = 86_400;

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
    /// Whether every member may see the series (public and active). A
    /// reminder posted in a channel names the series only when this holds.
    pub listed: bool,
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

    // Stored zones are validated on save; an unknown one reads as UTC.
    let tz = localtime::tz_or_utc(i.timezone);
    let Some(now) = DateTime::<Utc>::from_timestamp(i.now_unix, 0) else {
        return false;
    };
    let now_local = now.with_timezone(&tz);

    let Ok(at) = NaiveTime::parse_from_str(i.reminder_time, "%H:%M") else {
        return false;
    };
    // The most recent occurrence of the reminder time on the local clock:
    // today's, or yesterday's while today's is still ahead. Wall-clock
    // arithmetic, so a DST change shifts one window by an hour at most.
    let now_wall = now_local.date_naive().and_time(now_local.time());
    let today_at = now_wall.date().and_time(at);
    let opened = if now_wall >= today_at {
        today_at
    } else {
        let Some(yesterday_at) = today_at.checked_sub_days(Days::new(1)) else {
            return false;
        };
        yesterday_at
    };

    // Only inside that occurrence's window: up to `DUE_WINDOW_SECS`, cut
    // off at midnight, but never under `MIN_DUE_WINDOW_SECS`. Yesterday's
    // occurrence can only still be open for a reminder set after 23:00.
    let until_midnight = DAY_SECS - i64::from(at.num_seconds_from_midnight());
    let window = until_midnight.clamp(MIN_DUE_WINDOW_SECS, DUE_WINDOW_SECS);
    let since = now_wall.signed_duration_since(opened).num_seconds();
    if !(0..window).contains(&since) {
        return false;
    }

    let Some(last) = DateTime::<Utc>::from_timestamp(last_post_at, 0) else {
        return false;
    };
    // Judged for the day the window opened on, not for `now`: a window that
    // runs past midnight must not count the new day as already missed.
    behind(
        i.cadence,
        last.with_timezone(&tz).date_naive(),
        opened.date(),
    )
}

/// Cadence-aware "has the series missed its rhythm as of the local date
/// `on`?", given the local date of its newest post.
fn behind(cadence: Cadence, last_post: NaiveDate, on: NaiveDate) -> bool {
    match cadence {
        Cadence::Freeform => false,
        Cadence::Daily => last_post < on,
        Cadence::Weekdays => !matches!(on.weekday(), Weekday::Sat | Weekday::Sun) && last_post < on,
        // "Weekly" means once per ISO calendar week (Mon–Sun), due on the
        // weekday of the previous post: that is the creator's own rhythm, so
        // someone who posts on Sundays is nudged on a Sunday with no post,
        // not six days early on Monday. Later weekdays stay due as well, so
        // a nudge missed on the usual day (bot down) goes out the next day.
        // That has to hold across the week boundary too: once a full week
        // has passed since the post, the series is behind on any weekday,
        // or a Sunday poster whose Sunday nudge was missed would hear
        // nothing until the Sunday after. `last_reminder_day` still keeps
        // it to one nudge per missing day.
        Cadence::Weekly => {
            let last_week = (last_post.iso_week().year(), last_post.iso_week().week());
            let this_week = (on.iso_week().year(), on.iso_week().week());
            let usual_weekday = last_post.weekday().num_days_from_monday();
            let on_or_after_usual = on.weekday().num_days_from_monday() >= usual_weekday;
            let a_week_gone = on.signed_duration_since(last_post).num_days() >= 7;
            last_week < this_week && (on_or_after_usual || a_week_gone)
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
    fn weekly_is_due_from_the_previous_posts_weekday() {
        // now = Tue 2026-06-09 18:00 (ISO week Mon 06-08 .. Sun 06-14).
        let weekly = |last_offset_days: i64, now_offset_days: i64| {
            let mut i = inputs(last_offset_days);
            i.cadence = Cadence::Weekly;
            i.now_unix = tue_18() + now_offset_days * 86_400;
            reminder_due(&i)
        };

        // Posted Mon 06-08: this ISO week already has its post.
        assert!(!weekly(1, 0));
        assert!(!weekly(1, 5)); // ...all the way to Sunday
        // Posted Tue 06-02: last week, and today is their usual weekday.
        assert!(weekly(7, 0));
        // Posted Mon 06-01: usual day was yesterday, still due today.
        assert!(weekly(8, 0));

        // Posted Wed 06-03: not behind on Mon or Tue, due from Wednesday.
        assert!(!weekly(6, -1)); // Mon 06-08
        assert!(!weekly(6, 0)); // Tue 06-09
        assert!(weekly(6, 1)); // Wed 06-10
        assert!(weekly(6, 2)); // Thu 06-11 (caught up after a missed tick)

        // Posted Sun 06-07: quiet all week, due only on Sunday 06-14.
        for day in -1..=4 {
            assert!(!weekly(2, day), "offset {day}");
        }
        assert!(weekly(2, 5));

        // Posted Wed 05-27 and nothing since: more than a week behind, so
        // it is due on any day (the nudge for Wed 06-03 is the one that
        // would have gone out; if it did, the once-per-missing-day guard
        // keeps these days quiet, see below).
        assert!(weekly(13, -1)); // Mon 06-08
        assert!(weekly(13, 0)); // Tue 06-09
        assert!(weekly(13, 1)); // Wed 06-10
    }

    #[test]
    fn a_missed_weekly_nudge_is_caught_up_across_the_week_boundary() {
        // now = Tue 2026-06-09 18:00.
        let weekly = |last_offset_days: i64, now_offset_days: i64| {
            let mut i = inputs(last_offset_days);
            i.cadence = Cadence::Weekly;
            i.now_unix = tue_18() + now_offset_days * 86_400;
            i
        };

        // Posts on Sundays, last on Sun 06-07. The nudge for Sun 06-14 never
        // went out (leaf was down that evening): it goes out on Mon 06-15,
        // not a week later.
        assert!(reminder_due(&weekly(2, 5))); // Sun 06-14, the usual day
        assert!(reminder_due(&weekly(2, 6))); // Mon 06-15, caught up
        assert!(reminder_due(&weekly(2, 8))); // Wed 06-17, still due

        // Had the Sunday nudge gone out, Monday stays quiet: one nudge per
        // missing day.
        let mut nudged = weekly(2, 6);
        nudged.last_reminder_day = Some(nudged.expected_day);
        assert!(!reminder_due(&nudged));

        // An ordinary week is unchanged: the Monday right after a Sunday
        // post is one day on, not behind.
        assert!(!reminder_due(&weekly(2, -1))); // Mon 06-08

        // Posts on Wednesdays, last on Wed 06-03; leaf was down Wed 06-10
        // through Sun 06-14. Due on Mon 06-15 instead of Wed 06-17.
        assert!(reminder_due(&weekly(6, 6)));
    }

    #[test]
    fn bad_timezone_falls_back_to_utc_and_bad_time_disables() {
        let mut i = inputs(1);
        i.timezone = "Not/AZone";
        assert!(reminder_due(&i)); // 23:00 UTC is inside the 17:30 UTC window
        let mut i = inputs(1);
        i.reminder_time = "25:99";
        assert!(!reminder_due(&i));
    }

    #[test]
    fn timezone_names_match_case_insensitively() {
        let mut i = inputs(1);
        i.timezone = "america/chicago";
        i.now_unix = tue_18() - 3600; // 17:00 Chicago, 22:00 UTC
        // Read as Chicago (before 17:30), not as a UTC fallback (after it).
        assert!(!reminder_due(&i));
    }

    #[test]
    fn late_ticks_are_due_only_inside_the_window() {
        // Reminder at 17:30; the bot was down and comes back later.
        let at = |offset_secs: i64| {
            let mut i = inputs(1);
            i.now_unix = tue_18() + offset_secs;
            reminder_due(&i)
        };
        assert!(at(4 * 3600)); // 22:00, 4.5 h late: still worth sending
        assert!(at(5 * 3600 + 29 * 60)); // 23:29, the last minute
        assert!(!at(5 * 3600 + 30 * 60)); // 23:30, six hours on: too late

        // Nothing was marked, so the next day's window opens on schedule.
        assert!(!at(86_400 - 31 * 60)); // Wed 17:29
        assert!(at(86_400 - 30 * 60)); // Wed 17:30
    }

    #[test]
    fn enabling_late_in_the_day_waits_for_the_next_reminder_time() {
        // An 08:00 reminder switched on at 22:30 must not fire at 22:30.
        let mut i = inputs(1);
        i.reminder_time = "08:00";
        i.now_unix = tue_18() + 4 * 3600 + 30 * 60; // Tue 22:30
        assert!(!reminder_due(&i));
        i.now_unix = tue_18() + 14 * 3600; // Wed 08:00
        assert!(reminder_due(&i));
    }

    #[test]
    fn window_ends_at_local_midnight() {
        // A 22:00 reminder has two hours, not six: an evening nudge must
        // not turn into a small-hours one.
        let mut i = inputs(1);
        i.reminder_time = "22:00";
        i.now_unix = tue_18() + 6 * 3600 - 1; // Tue 23:59:59
        assert!(reminder_due(&i));
        i.now_unix = tue_18() + 6 * 3600; // Wed 00:00
        assert!(!reminder_due(&i));
        // 23:00 is the latest time whose window still stops at midnight.
        i.reminder_time = "23:00";
        i.now_unix = tue_18() + 6 * 3600 - 1;
        assert!(reminder_due(&i));
        i.now_unix = tue_18() + 6 * 3600;
        assert!(!reminder_due(&i));
    }

    /// Wednesday 2026-06-10 00:00 Chicago, as unix.
    fn wed_00() -> i64 {
        tue_18() + 6 * 3600
    }

    #[test]
    fn last_hour_reminders_keep_a_full_hour_past_midnight() {
        // A 23:59 reminder would have a one-minute window if it stopped at
        // midnight, and a tick that lands at 23:58:59 and then 00:00:05
        // would step over it.
        let at = |now_unix: i64| {
            let mut i = inputs(1); // last post Mon 18:00
            i.reminder_time = "23:59";
            i.now_unix = now_unix;
            reminder_due(&i)
        };
        assert!(!at(wed_00() - 61)); // Tue 23:58:59: not yet
        assert!(at(wed_00() - 60)); // Tue 23:59:00
        assert!(at(wed_00() + 5)); // Wed 00:00:05, the tick after
        assert!(at(wed_00() + 59 * 60 - 1)); // Wed 00:58:59
        assert!(!at(wed_00() + 59 * 60)); // Wed 00:59:00, an hour on

        // A 23:30 reminder survives a 45-minute outage.
        let mut i = inputs(1);
        i.reminder_time = "23:30";
        i.now_unix = wed_00() + 15 * 60; // Wed 00:15
        assert!(reminder_due(&i));
        i.now_unix = wed_00() + 30 * 60; // Wed 00:30
        assert!(!reminder_due(&i));
    }

    #[test]
    fn a_window_past_midnight_is_judged_for_the_day_it_opened_on() {
        let mut i = inputs(1);
        i.reminder_time = "23:59";
        i.now_unix = wed_00() + 5; // Wed 00:00:05, inside Tuesday's window

        // Posted on Tuesday: not behind, although Tuesday < Wednesday.
        i.last_post_at = Some(tue_18() + 2 * 3600); // Tue 20:00
        assert!(!reminder_due(&i));
        // Posted just after midnight: nothing to nudge about either.
        i.last_post_at = Some(wed_00() + 2);
        assert!(!reminder_due(&i));
        // Last posted on Monday: Tuesday was missed.
        i.last_post_at = Some(tue_18() - 86_400);
        assert!(reminder_due(&i));

        // Weekdays: Friday's window still counts after midnight on
        // Saturday; Sunday's does not open early on Monday.
        i.cadence = Cadence::Weekdays;
        i.reminder_time = "23:30";
        i.last_post_at = Some(tue_18()); // Tue 06-09
        i.now_unix = wed_00() + 3 * 86_400 + 10 * 60; // Sat 06-13 00:10
        assert!(reminder_due(&i));
        i.now_unix = wed_00() + 5 * 86_400 + 10 * 60; // Mon 06-15 00:10
        assert!(!reminder_due(&i));

        // Weekly: a Sunday poster's window on Sunday 06-14 runs into
        // Monday 06-15 without becoming next week's reminder.
        i.cadence = Cadence::Weekly;
        i.last_post_at = Some(tue_18() - 2 * 86_400); // Sun 06-07
        i.now_unix = wed_00() + 5 * 86_400 + 10 * 60; // Mon 06-15 00:10
        assert!(reminder_due(&i));
        i.last_post_at = Some(tue_18() + 5 * 86_400); // Sun 06-14 18:00
        assert!(!reminder_due(&i));
    }
}
