//! `/wrapped` — a yearly recap embed for a series. Read-only and
//! privacy-checked like the other query commands; the number-crunching is
//! `leaf_core::wrapped` (pure, tested), this file only renders it.

use leaf_core::wrapped::{self, Wrapped, WrappedPost};
use poise::serenity_prelude as serenity;

use crate::commands::query::viewable_series;
use crate::{Context, Error, checks};

/// A year in review for one series: counts, streak, busiest month.
#[poise::command(slash_command, guild_only)]
pub async fn wrapped(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "crate::commands::series_lookup::autocomplete_any_series"]
    series: String,
    #[description = "Year (default: this year)"]
    #[min = 2000]
    #[max = 2200]
    year: Option<i64>,
) -> Result<(), Error> {
    let Some(gid) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    // Bucket by the guild's timezone; fall back to UTC if unset/unknown.
    let tz: chrono_tz::Tz = ctx
        .data()
        .guilds
        .get(&gid)
        .await?
        .and_then(|s| s.timezone.parse().ok())
        .unwrap_or(chrono_tz::UTC);

    let Some((s, ephemeral)) = viewable_series(&ctx, &gid, &series).await? else {
        return Ok(());
    };

    let year = year.map_or_else(
        || {
            chrono::Utc::now()
                .with_timezone(&tz)
                .format("%Y")
                .to_string()
                .parse()
                .unwrap_or(1970)
        },
        |y| i32::try_from(y).unwrap_or(1970),
    );

    let posts: Vec<WrappedPost> = ctx
        .data()
        .posts
        .list_for_wrapped(s.id)
        .await?
        .into_iter()
        .map(|(day, posted_at, _, _)| WrappedPost { day, posted_at })
        .collect();
    let report = wrapped::summarize(&posts, year, tz);

    ctx.send(
        poise::CreateReply::default()
            .embed(embed(&s.name, &report))
            .ephemeral(ephemeral),
    )
    .await?;
    Ok(())
}

/// Builds the recap embed; reads gently when the year was empty.
fn embed(series_name: &str, w: &Wrapped) -> serenity::CreateEmbed {
    let title = format!("🍃 {series_name} — {} wrapped", w.year);

    if w.posts_in_year == 0 {
        return serenity::CreateEmbed::new()
            .title(title)
            .description(format!(
                "No posts in {} yet. The archive holds {} day{} all-time. 🌱",
                w.year,
                w.total_all_time,
                plural(w.total_all_time)
            ))
            .colour(0x6f_bf73);
    }

    let busiest = w.busiest_month.map_or_else(
        || "—".to_owned(),
        |(m, n)| format!("{} ({n} post{})", wrapped::month_name(m), plural(n)),
    );
    let span = match (w.first_day, w.last_day) {
        (Some(a), Some(b)) => format!("Day {a} → Day {b}"),
        _ => "—".to_owned(),
    };

    serenity::CreateEmbed::new()
        .title(title)
        .colour(0x6f_bf73)
        .field("Posts this year", w.posts_in_year.to_string(), true)
        .field("Longest streak", format!("{} days", w.longest_streak), true)
        .field("Busiest month", busiest, true)
        .field("Range", span, true)
        .field("All-time total", w.total_all_time.to_string(), true)
        .footer(serenity::CreateEmbedFooter::new("🍃 one leaf at a time"))
}

const fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}
