//! `/wrapped`: a yearly recap card for a series. Read-only and
//! privacy-checked like the other query commands; the number-crunching is
//! `leaf_core::wrapped` (pure, tested), this file only renders it.
//!
//! The year is counted in the server's timezone, the same rule as the
//! gallery calendar, and the card says which zone that is.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike as _};
use leaf_core::domain::Series;
use leaf_core::localtime::{self, Tz};
use leaf_core::series_ops;
use leaf_core::wrapped::{self, Wrapped, WrappedPost};
use poise::serenity_prelude as serenity;

use crate::commands::query::{
    EMBED_COLOUR, begin, day_thumbnail, empty_series_text, jump_link, plural, private_series,
    series_emoji,
};
use crate::commands::series_lookup::{
    Answer, Scope, and_list, autocomplete_viewable_series, open_gallery_button,
};
use crate::{Context, Error};

/// File name the recap's thumbnail is attached under.
const THUMB_NAME: &str = "last.webp";

/// A year in review for one series: posts, longest run, busiest month, first and last post.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn wrapped(
    ctx: Context<'_>,
    #[description = "Series (you can leave it out when there's only one)"]
    #[autocomplete = "autocomplete_viewable_series"]
    series: Option<String>,
    #[description = "Year (default: this year, or the latest year with posts)"]
    #[min = 2000]
    #[max = 2200]
    year: Option<i64>,
) -> Result<(), Error> {
    let Some(mut begun) = begin(ctx, series.as_deref(), Scope::Viewable).await? else {
        return Ok(());
    };
    let s = &begun.series;
    let data = begun.via.data();
    let tz = localtime::tz_or_utc(&begun.settings.timezone);

    // (day, posted_at, message_id, channel_id), ascending by day.
    let rows = data.posts.list_for_wrapped(s.id).await?;
    if rows.is_empty() {
        let text = empty_series_text(s, &begun.asker, &data.app_name());
        begun.via.send(Answer::private(text)).await?;
        return Ok(());
    }
    let posts: Vec<WrappedPost> = rows
        .iter()
        .map(|(day, posted_at, _, _)| WrappedPost {
            day: *day,
            posted_at: *posted_at,
        })
        .collect();
    let years = years_with_posts(&posts, tz);
    let this_year = chrono::Utc::now().with_timezone(&tz).year();
    let report = wrapped::summarize(&posts, pick_year(year, this_year, &years), tz);
    let private = private_series(s);
    let gallery = open_gallery_button(Some((s.id, None)));

    if report.posts_in_year == 0 {
        let embed = card(s, &report, tz).description(empty_year_text(&report, &years));
        let answer = Answer {
            embed: Some(embed),
            ephemeral: private,
            ..Answer::default()
        };
        begun.via.send(answer.button(gallery)).await?;
        return Ok(());
    }

    // The thumbnail comes from storage: defer first.
    begun.via.defer(private).await?;
    let endpoint = |day: Option<i64>| {
        let (day, posted_at, message_id, channel_id) = rows.iter().find(|r| Some(r.0) == day)?;
        Some(Endpoint {
            day: *day,
            posted_at: *posted_at,
            jump: jump_link(&s.guild_id, channel_id, message_id),
        })
    };
    let (first, last) = (endpoint(report.first_day), endpoint(report.last_day));
    let image = match report.last_day {
        Some(day) => day_thumbnail(data, s.id, day).await,
        None => None,
    };
    let mut embed =
        card(s, &report, tz).description(recap_text(&report, first.as_ref(), last.as_ref()));
    if image.is_some() {
        embed = embed.thumbnail(format!("attachment://{THUMB_NAME}"));
    }
    let answer = Answer {
        embed: Some(embed),
        image: image.map(|bytes| serenity::CreateAttachment::bytes(bytes, THUMB_NAME)),
        ephemeral: private,
        ..Answer::default()
    };
    begun.via.send(answer.button(gallery)).await?;
    Ok(())
}

/// The card's frame: title in the series' emoji (the name escaped, so it
/// shows as typed), and the zone the year was counted in (dates inside
/// render in each viewer's own zone).
fn card(series: &Series, w: &Wrapped, tz: Tz) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .title(format!(
            "{} {} — {} wrapped",
            series_emoji(series),
            series_ops::display_name(&series.name),
            w.year
        ))
        .colour(EMBED_COLOUR)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Year boundaries in {}",
            tz.name()
        )))
}

/// The first or last post of the year, for a link.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    day: i64,
    posted_at: i64,
    jump: String,
}

impl Endpoint {
    fn line(&self, label: &str) -> String {
        format!(
            "{label}: [Day {}]({}) · <t:{}:D>",
            self.day, self.jump, self.posted_at
        )
    }
}

/// The recap of a year with posts.
fn recap_text(w: &Wrapped, first: Option<&Endpoint>, last: Option<&Endpoint>) -> String {
    let mut lines = vec![format!(
        "**{}** post{} · longest run **{} day{}**",
        w.posts_in_year,
        plural(w.posts_in_year),
        w.longest_streak,
        plural(w.longest_streak)
    )];
    if let Some((month, n)) = w.busiest_month {
        lines.push(format!(
            "Busiest month: **{}** ({n} post{})",
            wrapped::month_name(month),
            plural(n)
        ));
    }
    match (first, last) {
        (Some(first), Some(last)) if first == last => lines.push(first.line("Only post")),
        (first, last) => {
            lines.extend(first.map(|e| e.line("First")));
            lines.extend(last.map(|e| e.line("Last")));
        }
    }
    format!(
        "{}\n\n{}",
        lines.join("\n"),
        all_time_line(w.total_all_time)
    )
}

/// The card for a year without posts: which years have some.
fn empty_year_text(w: &Wrapped, years: &[i32]) -> String {
    let years: Vec<String> = years.iter().map(ToString::to_string).collect();
    let with_posts = if years.is_empty() {
        String::new()
    } else {
        format!(" Years with posts: {}.", and_list(&years))
    };
    format!(
        "No posts in {}.{with_posts}\n\n{}",
        w.year,
        all_time_line(w.total_all_time)
    )
}

fn all_time_line(total: i64) -> String {
    format!("{total} day{} archived all-time.", plural(total))
}

/// The local years (in `tz`) that have at least one post, ascending.
fn years_with_posts(posts: &[WrappedPost], tz: Tz) -> Vec<i32> {
    posts
        .iter()
        .filter_map(|p| DateTime::from_timestamp(p.posted_at, 0))
        .map(|at| at.with_timezone(&tz).year())
        .collect::<BTreeSet<i32>>()
        .into_iter()
        .collect()
}

/// The year to recap: the one asked for; else this year when it has posts;
/// else the latest year that does (so early January still shows last
/// year's recap); else this year.
fn pick_year(requested: Option<i64>, this_year: i32, years: &[i32]) -> i32 {
    if let Some(year) = requested.and_then(|y| i32::try_from(y).ok()) {
        return year;
    }
    if years.contains(&this_year) {
        return this_year;
    }
    years.last().copied().unwrap_or(this_year)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use chrono::TimeZone as _;

    use super::*;

    fn wrapped(posts_in_year: i64, streak: i64, total_all_time: i64) -> Wrapped {
        Wrapped {
            year: 2025,
            posts_in_year,
            first_day: Some(300),
            last_day: Some(427),
            busiest_month: Some((3, 31)),
            longest_streak: streak,
            total_all_time,
        }
    }

    fn endpoint(day: i64) -> Endpoint {
        Endpoint {
            day,
            posted_at: 1_700_000_000,
            jump: format!("https://discord.com/channels/g/c/{day}"),
        }
    }

    #[test]
    fn the_year_defaults_to_one_with_posts() {
        assert_eq!(pick_year(Some(2021), 2026, &[2024]), 2021);
        assert_eq!(pick_year(None, 2026, &[2025, 2026]), 2026);
        // Early January: nothing yet this year, so last year's recap.
        assert_eq!(pick_year(None, 2026, &[2024, 2025]), 2025);
        assert_eq!(pick_year(None, 2026, &[]), 2026);
    }

    #[test]
    fn years_are_counted_in_the_server_zone() {
        let chicago: Tz = "America/Chicago".parse().unwrap();
        // 23:30 on 31 December in Chicago is already 2026 in UTC.
        let late = chicago
            .with_ymd_and_hms(2025, 12, 31, 23, 30, 0)
            .unwrap()
            .timestamp();
        let posts = [
            WrappedPost {
                day: 1,
                posted_at: late,
            },
            WrappedPost {
                day: 2,
                posted_at: late + 3_600,
            },
        ];
        assert_eq!(years_with_posts(&posts, chicago), vec![2025, 2026]);
        assert_eq!(years_with_posts(&posts, chrono_tz::UTC), vec![2026]);
    }

    #[test]
    fn the_recap_links_its_first_and_last_post() {
        let w = wrapped(128, 41, 953);
        let text = recap_text(&w, Some(&endpoint(300)), Some(&endpoint(427)));
        assert_eq!(
            text,
            "**128** posts · longest run **41 days**\n\
             Busiest month: **March** (31 posts)\n\
             First: [Day 300](https://discord.com/channels/g/c/300) · <t:1700000000:D>\n\
             Last: [Day 427](https://discord.com/channels/g/c/427) · <t:1700000000:D>\n\n\
             953 days archived all-time."
        );
    }

    #[test]
    fn counts_of_one_read_as_one() {
        let mut w = wrapped(1, 1, 1);
        w.busiest_month = Some((1, 1));
        let only = endpoint(5);
        let text = recap_text(&w, Some(&only), Some(&only));
        assert!(
            text.starts_with("**1** post · longest run **1 day**"),
            "{text}"
        );
        assert!(text.contains("Busiest month: **January** (1 post)"));
        assert!(text.contains("Only post: [Day 5]"));
        assert!(text.ends_with("1 day archived all-time."));
    }

    #[test]
    fn an_empty_year_lists_the_years_that_have_posts() {
        let w = wrapped(0, 0, 40);
        assert_eq!(
            empty_year_text(&w, &[2023, 2024]),
            "No posts in 2025. Years with posts: 2023 and 2024.\n\n40 days archived all-time."
        );
    }
}
