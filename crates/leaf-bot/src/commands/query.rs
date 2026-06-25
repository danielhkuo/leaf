//! Chat-side read access: `/search`, `/status`, `/random`, `/delete`, and
//! the 🗑️ context menu. Privacy is enforced through `policy::can_view` on
//! every path; non-public series always answer ephemerally.

use leaf_core::domain::{Series, SeriesState};
use leaf_core::policy::{self, Viewer};
use poise::serenity_prelude as serenity;

use crate::commands::series_lookup::{autocomplete_any_series, owned_series};
use crate::{Context, Error, checks};

/// Builds the viewer facts for the invoker.
pub(crate) async fn viewer_parts(ctx: &Context<'_>) -> (String, Vec<String>, bool) {
    let roles = match ctx.author_member().await {
        Some(m) => m.roles.iter().map(ToString::to_string).collect(),
        None => Vec::new(),
    };
    (ctx.author().id.to_string(), roles, checks::is_admin(ctx))
}

/// Loads a series by name and checks the invoker may view it. Replies and
/// returns `None` otherwise. The bool is "reply should be ephemeral".
pub(crate) async fn viewable_series(
    ctx: &Context<'_>,
    guild_id: &str,
    name: &str,
) -> Result<Option<(Series, bool)>, Error> {
    let found = ctx.data().series.get_by_name(guild_id, name).await?;
    let (user_id, roles, is_admin) = viewer_parts(ctx).await;
    let viewer = Viewer {
        user_id: &user_id,
        role_ids: &roles,
        is_admin,
    };

    match found {
        Some(s) if policy::can_view(&s, &viewer) => {
            let sensitive =
                s.privacy != leaf_core::domain::Privacy::Public || s.state != SeriesState::Active;
            Ok(Some((s, sensitive)))
        }
        // Not found and forbidden are indistinguishable on purpose.
        _ => {
            ctx.send(
                poise::CreateReply::default()
                    .content(format!("No series named **{name}** here."))
                    .ephemeral(true),
            )
            .await?;
            Ok(None)
        }
    }
}

/// Renders one archived day as an embed (thumbnail attached from storage).
async fn day_reply(
    ctx: &Context<'_>,
    series: &Series,
    day: i64,
    ephemeral: bool,
) -> Result<bool, Error> {
    let Some((post, media)) = ctx.data().posts.get(series.id, day).await? else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("**{}** has no Day {day}.", series.name))
                .ephemeral(true),
        )
        .await?;
        return Ok(false);
    };

    let jump = format!(
        "https://discord.com/channels/{}/{}/{}",
        series.guild_id, post.channel_id, post.message_id
    );
    let caption = if post.caption.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", post.caption)
    };
    let extra = if media.len() > 1 {
        format!(" · {} files", media.len())
    } else {
        String::new()
    };

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("{} Day {day} — {}", series.emoji, series.name))
        .description(format!(
            "{caption}[Jump to the original post]({jump}){extra}"
        ))
        .timestamp(serenity::Timestamp::from_unix_timestamp(post.posted_at)?)
        .colour(serenity::Colour::new(0x006F_BF73));

    let mut reply = poise::CreateReply::default().ephemeral(ephemeral);
    let thumb_key = media.iter().find_map(|m| m.thumb_key.clone());
    if let Some(key) = thumb_key {
        match ctx.data().media.get_bytes(&key).await {
            Ok(bytes) => {
                reply = reply.attachment(serenity::CreateAttachment::bytes(bytes, "thumb.webp"));
                embed = embed.image("attachment://thumb.webp");
            }
            Err(e) => tracing::warn!(key, error = %e, "thumbnail fetch failed for embed"),
        }
    }

    ctx.send(reply.embed(embed)).await?;
    Ok(true)
}

/// Look up one archived day of a series.
#[poise::command(slash_command, guild_only)]
pub async fn search(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "autocomplete_any_series"]
    series: String,
    #[description = "Day number"]
    #[min = 1]
    day: i64,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some((s, ephemeral)) = viewable_series(&ctx, &settings.guild_id, &series).await? else {
        return Ok(());
    };
    day_reply(&ctx, &s, day, ephemeral).await?;
    Ok(())
}

/// A random archived day.
#[poise::command(slash_command, guild_only)]
pub async fn random(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "autocomplete_any_series"]
    series: String,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some((s, ephemeral)) = viewable_series(&ctx, &settings.guild_id, &series).await? else {
        return Ok(());
    };
    match ctx.data().posts.random_day(s.id).await? {
        Some(day) => {
            day_reply(&ctx, &s, day, ephemeral).await?;
        }
        None => {
            ctx.send(
                poise::CreateReply::default()
                    .content(format!("**{}** has nothing archived yet.", s.name))
                    .ephemeral(true),
            )
            .await?;
        }
    }
    Ok(())
}

/// Which days are archived and which are missing.
#[poise::command(slash_command, guild_only)]
pub async fn status(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "autocomplete_any_series"]
    series: String,
    #[description = "First day to audit (default: series start)"]
    #[min = 1]
    start: Option<i64>,
    #[description = "Last day to audit (default: start + 19)"]
    #[min = 1]
    end: Option<i64>,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some((s, _)) = viewable_series(&ctx, &settings.guild_id, &series).await? else {
        return Ok(());
    };

    let max_day = ctx.data().posts.max_day(s.id).await?;
    let from = start.unwrap_or(s.start_day);
    let to = end.unwrap_or_else(|| max_day.unwrap_or(from).min(from + 19).max(from));
    if to < from {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 The start day has to come before the end day.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let to = to.min(from + 199); // hard cap: 200 days per audit

    let have = ctx.data().posts.days_in_range(s.id, from, to).await?;
    let lines: Vec<String> = (from..=to)
        .map(|d| {
            let mark = if have.binary_search(&d).is_ok() {
                "✅"
            } else {
                "❌"
            };
            format!("Day {d} {mark}")
        })
        .collect();

    let archived = have.len();
    let total = lines.len();
    for chunk in lines.chunks(20) {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "**{}** — {archived}/{total} archived in {from}–{to}\n{}",
                    s.name,
                    chunk.join("\n")
                ))
                .ephemeral(true),
        )
        .await?;
    }
    Ok(())
}

/// Remove an archived day from your series (storage included).
#[poise::command(slash_command, guild_only)]
pub async fn delete(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "autocomplete_any_series"]
    series: String,
    #[description = "Day number to remove"]
    #[min = 1]
    day: i64,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some(s) = owned_series(&ctx, &settings.guild_id, &series).await? else {
        return Ok(());
    };
    delete_day(&ctx, &settings, &s, day).await
}

/// Remove an archived entry by right-clicking its original message.
#[poise::command(context_menu_command = "Remove Archive Entry", guild_only)]
pub async fn delete_menu(ctx: Context<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some((series_id, day)) = ctx
        .data()
        .posts
        .find_by_message(&msg.id.to_string())
        .await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 That message isn't in any archive.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let Some(s) = ctx.data().series.get(series_id).await? else {
        return Ok(());
    };
    if s.creator_id != ctx.author().id.to_string() && !checks::is_admin(&ctx) {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 Only the series creator (or an admin) can remove entries.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    delete_day(&ctx, &settings, &s, day).await
}

async fn delete_day(
    ctx: &Context<'_>,
    settings: &leaf_core::domain::GuildSettings,
    s: &Series,
    day: i64,
) -> Result<(), Error> {
    if !ctx.data().posts.exists(s.id, day).await? {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("**{}** has no Day {day}.", s.name))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if !checks::confirm(
        ctx,
        &format!(
            "Remove Day {day} of **{}**? The stored files are deleted too. \
             This cannot be undone.",
            s.name
        ),
    )
    .await?
    {
        return Ok(());
    }

    let keys = ctx.data().posts.delete(s.id, day).await?;
    ctx.data().media.delete_keys(&keys).await;
    checks::log_line(
        ctx,
        settings,
        &format!(
            "🗑️ Day {day} of **{}** removed by {}",
            s.name,
            ctx.author().name
        ),
    )
    .await;
    Ok(())
}
