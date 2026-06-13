//! The reminder scheduler: a coarse one-minute tick that asks the pure
//! `reminder_due` predicate about every reminder-enabled series and nudges
//! the ones that are behind.
//!
//! There is no cron table and no per-slot bookkeeping. Because "due" is a
//! state predicate (behind schedule, past the reminder time, not yet
//! reminded for this missing day), **downtime catch-up is automatic**: a
//! reminder missed while the bot was down is still due on the next tick.
//! Delivery is at-most-once per missing day via mark-before-send with
//! rollback on failure (the v2 policy).

use std::sync::Arc;
use std::time::Duration;

use leaf_core::db::SeriesRepo;
use leaf_core::reminder::{ReminderCandidate, reminder_due};
use poise::serenity_prelude as serenity;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::checks::now_unix;

/// How often the scheduler wakes. One minute is fine: reminder times have
/// minute resolution and the predicate is idempotent across ticks.
const TICK: Duration = Duration::from_mins(1);

/// Runs the scheduler until `shutdown` flips. The first tick fires
/// immediately on startup so missed reminders are caught up at once.
pub async fn run(
    http: Arc<serenity::Http>,
    series: SeriesRepo,
    mut shutdown: watch::Receiver<bool>,
) {
    info!("reminder scheduler started");
    loop {
        if *shutdown.borrow() {
            break;
        }
        tick(&http, &series).await;
        tokio::select! {
            () = tokio::time::sleep(TICK) => {}
            _ = shutdown.changed() => break,
        }
    }
    info!("reminder scheduler stopped");
}

/// One pass over all reminder candidates.
async fn tick(http: &Arc<serenity::Http>, series: &SeriesRepo) {
    let now = now_unix();
    let candidates = match series.reminder_candidates().await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "reminder tick: could not load candidates");
            return;
        }
    };

    for c in candidates {
        if !reminder_due(&c.inputs(now)) {
            // Record the check timestamp without touching the reminded day.
            if let Err(e) = series
                .set_reminder_state(c.series_id, c.last_reminder_day, now)
                .await
            {
                warn!(series = c.series_id, error = %e, "reminder: check-timestamp write failed");
            }
            continue;
        }
        deliver(http, series, &c, now).await;
    }
}

/// Marks (before sending, so a crash can't double-send), sends, and rolls
/// the mark back if the send fails so the next tick retries.
async fn deliver(http: &Arc<serenity::Http>, series: &SeriesRepo, c: &ReminderCandidate, now: i64) {
    let expected = c.expected_day();
    let previous = c.last_reminder_day;

    if let Err(e) = series
        .set_reminder_state(c.series_id, Some(expected), now)
        .await
    {
        warn!(series = c.series_id, error = %e, "reminder: mark-before-send failed; skipping");
        return;
    }

    if let Err(e) = send(http, c, expected).await {
        warn!(series = c.series_id, error = %e, "reminder send failed; rolling back to retry");
        if let Err(e2) = series.set_reminder_state(c.series_id, previous, now).await {
            warn!(series = c.series_id, error = %e2, "reminder: rollback failed");
        }
    } else {
        info!(
            series = c.series_id,
            day = expected,
            dm = c.reminder_dm,
            "reminder sent"
        );
    }
}

/// Sends the nudge: a DM to the creator, or a ping in the first channel.
async fn send(
    http: &Arc<serenity::Http>,
    c: &ReminderCandidate,
    expected: i64,
) -> anyhow::Result<()> {
    if c.reminder_dm {
        let uid: u64 = c.creator_id.parse()?;
        let dm = serenity::UserId::new(uid).create_dm_channel(http).await?;
        dm.say(http, dm_text(c, expected)).await?;
    } else {
        let Some(channel) = c.channels.first() else {
            anyhow::bail!("channel reminder but series has no channel");
        };
        let cid: u64 = channel.parse()?;
        serenity::ChannelId::new(cid)
            .say(http, channel_text(c, expected))
            .await?;
    }
    Ok(())
}

fn dm_text(c: &ReminderCandidate, expected: i64) -> String {
    let where_to = c
        .channels
        .first()
        .map_or_else(String::new, |ch| format!(" in <#{ch}>"));
    format!(
        "🍃 Gentle nudge: **{}** is waiting on Day {expected}. \
         Post it{where_to} and archive to keep the streak going.",
        c.name
    )
}

fn channel_text(c: &ReminderCandidate, expected: i64) -> String {
    format!(
        "🍃 <@{}> — **{}** is waiting on Day {expected}.",
        c.creator_id, c.name
    )
}
