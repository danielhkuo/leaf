//! Slash commands. Each command's doc comment doubles as its description
//! in the Discord UI (poise picks it up).

#![allow(
    missing_docs,
    reason = "poise::command emits undocumented public wrapper fns; every \
              command here still carries a doc comment as its UI description"
)]

use std::time::Duration;

use crate::{Context, Error};

/// Check that leaf is alive (version and uptime).
#[poise::command(slash_command)]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let uptime = format_uptime(ctx.data().started.elapsed());
    ctx.say(format!(
        "🍃 leaf v{} — up {uptime}",
        env!("CARGO_PKG_VERSION")
    ))
    .await?;
    Ok(())
}

/// Renders a duration as the largest two units, e.g. `3d 7h` or `12m 40s`.
#[must_use]
pub fn format_uptime(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let (days, hours, mins) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    match (days, hours, mins) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m {}s", secs % 60),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::duration_suboptimal_units,
        reason = "tests spell durations in raw seconds on purpose"
    )]

    use super::*;

    #[test]
    fn uptime_formatting_picks_two_largest_units() {
        assert_eq!(format_uptime(Duration::from_secs(42)), "42s");
        assert_eq!(format_uptime(Duration::from_secs(125)), "2m 5s");
        assert_eq!(format_uptime(Duration::from_secs(3 * 3600 + 240)), "3h 4m");
        assert_eq!(
            format_uptime(Duration::from_secs(2 * 86_400 + 5 * 3600 + 59)),
            "2d 5h"
        );
        assert_eq!(format_uptime(Duration::ZERO), "0s");
    }
}
