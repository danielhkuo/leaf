//! 🍃 Archive to Series — the core write path, as a message context menu.
//!
//! Modal-first flow: right-click → modal (day pre-filled by the parser,
//! series field included only when the creator has several) → uploads to
//! R2 → transactional DB insert → reaction + log line. No session state.

use leaf_core::domain::{NewMediaAttachment, Post, Series, SeriesState};
use leaf_core::media::{ALLOWED_CONTENT_TYPES, MediaMeta};
use leaf_core::parser;
use poise::Modal as _;
use poise::serenity_prelude as serenity;

use crate::{Context, Error, checks};

/// Modal when the creator has exactly one series.
#[derive(poise::Modal)]
#[name = "Archive to Series"]
struct DayModal {
    /// Day number for this post.
    #[name = "Day number"]
    #[placeholder = "e.g. 42"]
    day: String,
}

/// Modal when the creator has several series.
#[derive(poise::Modal)]
#[name = "Archive to Series"]
struct SeriesDayModal {
    /// Which series to archive into.
    #[name = "Series name"]
    series: String,
    /// Day number for this post.
    #[name = "Day number"]
    #[placeholder = "e.g. 42"]
    day: String,
}

/// Archive a message into your series.
#[poise::command(context_menu_command = "Archive to Series", guild_only)]
pub async fn archive_menu(ctx: Context<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let gid = settings.guild_id.clone();
    let me = ctx.author().id.to_string();

    let mine: Vec<Series> = ctx
        .data()
        .series
        .list_by_creator(&gid, &me)
        .await?
        .into_iter()
        .filter(|s| s.state != SeriesState::Revoked)
        .collect();

    if mine.is_empty() {
        ctx.send(
            poise::CreateReply::default()
                .content("🌱 You don't have a series yet — `/series create` plants one.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    // Attachments worth archiving, checked before bothering with a modal.
    let usable: Vec<&serenity::Attachment> = msg
        .attachments
        .iter()
        .filter(|a| {
            a.content_type.as_deref().is_some_and(|ct| {
                ALLOWED_CONTENT_TYPES.contains(&ct.split(';').next().unwrap_or(ct).trim())
            })
        })
        .collect();
    if usable.is_empty() {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 That message has no image or video attachments I can archive.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let poise::Context::Application(app_ctx) = ctx else {
        return Ok(());
    };

    // Day suggestion: parser on the message text, else next expected day
    // of the (first) series.
    let newest = mine.last().map_or(0, |s| s.id);
    let fallback = ctx
        .data()
        .posts
        .max_day(newest)
        .await?
        .map_or_else(|| mine.last().map_or(1, |s| s.start_day), |max| max + 1);
    let day_default = parser::suggested_day(&msg.content)
        .unwrap_or(fallback)
        .to_string();

    let Some((series, day_raw)) = prompt_for_target(&ctx, app_ctx, &mine, day_default).await?
    else {
        return Ok(());
    };

    let Ok(day) = day_raw.trim().parse::<i64>() else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("🍂 `{day_raw}` isn't a day number."))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if day < 1 {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 Day numbers start at 1.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    execute_archive(&ctx, &settings, &series, day, &msg, &usable).await
}

/// Shows the archive modal (with a series field only when the creator has
/// several) and returns the chosen series + raw day text, or `None` when
/// the modal was dismissed (already replied where needed).
async fn prompt_for_target(
    ctx: &Context<'_>,
    app_ctx: poise::ApplicationContext<'_, crate::Data, Error>,
    mine: &[Series],
    day_default: String,
) -> Result<Option<(Series, String)>, Error> {
    if let [only] = mine {
        let Some(data) =
            DayModal::execute_with_defaults(app_ctx, DayModal { day: day_default }).await?
        else {
            return Ok(None);
        };
        return Ok(Some((only.clone(), data.day)));
    }

    let default_name = mine.last().map_or_else(String::new, |s| s.name.clone());
    let Some(data) = SeriesDayModal::execute_with_defaults(
        app_ctx,
        SeriesDayModal {
            series: default_name,
            day: day_default,
        },
    )
    .await?
    else {
        return Ok(None);
    };
    match resolve_series(mine, &data.series) {
        Ok(series) => Ok(Some((series.clone(), data.day))),
        Err(reason) => {
            ctx.send(
                poise::CreateReply::default()
                    .content(reason)
                    .ephemeral(true),
            )
            .await?;
            Ok(None)
        }
    }
}

/// Uploads the attachments, inserts the post, reacts, and reports.
async fn execute_archive(
    ctx: &Context<'_>,
    settings: &leaf_core::domain::GuildSettings,
    series: &Series,
    day: i64,
    msg: &serenity::Message,
    usable: &[&serenity::Attachment],
) -> Result<(), Error> {
    let gid = settings.guild_id.clone();

    // Cheap duplicate check before any upload work.
    if ctx.data().posts.exists(series.id, day).await? {
        let _react = msg.react(ctx.http(), '⚠').await;
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "⚠️ **{}** already has Day {day}. `/delete` it first if this should replace it.",
                    series.name
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let Some((media_rows, uploaded_keys)) =
        upload_attachments(ctx, &gid, series.id, day, msg, usable).await?
    else {
        return Ok(());
    };

    let post = Post {
        series_id: series.id,
        day,
        message_id: msg.id.to_string(),
        channel_id: msg.channel_id.to_string(),
        caption: msg.content.clone(),
        posted_at: msg.timestamp.unix_timestamp(),
        archived_at: checks::now_unix(),
    };

    if let Err(e) = ctx.data().posts.insert_with_media(&post, &media_rows).await {
        ctx.data().media.delete_keys(&uploaded_keys).await;
        if matches!(e, leaf_core::db::DbError::DuplicateDay(_)) {
            let _react = msg.react(ctx.http(), '⚠').await;
            ctx.send(
                poise::CreateReply::default()
                    .content(format!(
                        "⚠️ Day {day} was archived by someone else just now."
                    ))
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
        return Err(e.into());
    }

    // Sprout promotion check.
    let mut promoted = false;
    if series.state == SeriesState::Sprout {
        let count = ctx.data().posts.count(series.id).await?;
        if count >= settings.sprout_threshold {
            ctx.data()
                .series
                .set_state(series.id, SeriesState::Active)
                .await?;
            promoted = true;
        }
    }

    let reaction = series
        .emoji
        .chars()
        .next()
        .filter(|c| !c.is_ascii())
        .unwrap_or('🍃');
    let _react = msg.react(ctx.http(), reaction).await;

    let files = media_rows.len();
    let promo = if promoted {
        format!(
            "\n🌿 **{}** sprouted — it's now publicly listed!",
            series.name
        )
    } else {
        String::new()
    };
    ctx.send(
        poise::CreateReply::default()
            .content(format!(
                "🍃 Day {day} of **{}** archived ({files} file{}).{promo}",
                series.name,
                if files == 1 { "" } else { "s" }
            ))
            .ephemeral(true),
    )
    .await?;
    checks::log_line(
        ctx,
        settings,
        &format!(
            "🍃 Day {day} of **{}** archived ({files} files)",
            series.name
        ),
    )
    .await;

    announce_milestone(ctx, series, day, msg).await;
    Ok(())
}

/// Posts a celebratory line in the channel when `day` is a milestone
/// (first post, year marks, round hundreds). Best-effort; never fails the
/// archive.
async fn announce_milestone(ctx: &Context<'_>, series: &Series, day: i64, msg: &serenity::Message) {
    let Some(milestone) = leaf_core::milestone::classify(day) else {
        return;
    };
    let text = leaf_core::milestone::render(
        series.milestone_template.as_deref(),
        milestone,
        day,
        &series.name,
        &format!("<@{}>", series.creator_id),
    );
    if let Err(e) = msg.channel_id.say(ctx.http(), text).await {
        tracing::warn!(series = series.id, day, error = %e, "milestone announcement failed");
    }
}

/// Uploads originals + thumbnails for every usable attachment. On failure
/// the partial uploads are removed, the user is told, and `None` returns.
async fn upload_attachments(
    ctx: &Context<'_>,
    guild_id: &str,
    series_id: i64,
    day: i64,
    msg: &serenity::Message,
    usable: &[&serenity::Attachment],
) -> Result<Option<(Vec<NewMediaAttachment>, Vec<String>)>, Error> {
    let mut media_rows = Vec::new();
    let mut uploaded_keys = Vec::new();
    for att in usable {
        let meta = MediaMeta {
            guild_id: guild_id.to_owned(),
            series_id,
            day,
            attachment_id: att.id.to_string(),
            content_type: att.content_type.clone().unwrap_or_default(),
        };
        match ctx.data().media.archive_from_url(&att.url, &meta).await {
            Ok(stored) => {
                uploaded_keys.push(stored.original_key.clone());
                uploaded_keys.push(stored.thumb_key.clone());
                media_rows.push(NewMediaAttachment {
                    attachment_id: meta.attachment_id,
                    channel_id: msg.channel_id.to_string(),
                    message_id: msg.id.to_string(),
                    content_type: meta.content_type,
                    original_key: Some(stored.original_key),
                    thumb_key: Some(stored.thumb_key),
                    media_missing: false,
                });
            }
            Err(e) => {
                ctx.data().media.delete_keys(&uploaded_keys).await;
                ctx.send(
                    poise::CreateReply::default()
                        .content(format!("🍂 Couldn't store `{}`: {e}", att.filename))
                        .ephemeral(true),
                )
                .await?;
                return Ok(None);
            }
        }
    }
    Ok(Some((media_rows, uploaded_keys)))
}

/// Resolves the typed series name against the creator's list: exact match,
/// else unique case-insensitive match, else unique prefix.
fn resolve_series<'a>(mine: &'a [Series], input: &str) -> Result<&'a Series, String> {
    let trimmed = input.trim();
    if let Some(s) = mine.iter().find(|s| s.name == trimmed) {
        return Ok(s);
    }
    let lower = trimmed.to_ascii_lowercase();
    let ci: Vec<&Series> = mine
        .iter()
        .filter(|s| s.name.to_ascii_lowercase() == lower)
        .collect();
    if let [one] = ci.as_slice() {
        return Ok(one);
    }
    let prefixed: Vec<&Series> = mine
        .iter()
        .filter(|s| s.name.to_ascii_lowercase().starts_with(&lower))
        .collect();
    match prefixed.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "🍂 You don't have a series named **{trimmed}**. Yours: {}",
            mine.iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "🍂 **{trimmed}** matches several of your series — type the full name."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::domain::{Cadence, DetectionMode, Privacy};

    fn s(id: i64, name: &str) -> Series {
        Series {
            id,
            guild_id: "g".into(),
            creator_id: "u".into(),
            name: name.into(),
            description: String::new(),
            channels: vec![],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".into(),
            state: SeriesState::Active,
            created_at: 0,
        }
    }

    #[test]
    fn series_name_resolution() {
        let mine = vec![s(1, "Daily Sketch"), s(2, "daily ink"), s(3, "Photos")];
        assert_eq!(resolve_series(&mine, "Daily Sketch").map(|x| x.id), Ok(1));
        assert_eq!(resolve_series(&mine, "DAILY INK").map(|x| x.id), Ok(2));
        assert_eq!(resolve_series(&mine, "pho").map(|x| x.id), Ok(3));
        assert!(resolve_series(&mine, "daily").is_err()); // ambiguous prefix
        assert!(resolve_series(&mine, "nope").is_err()); // unknown
        assert_eq!(resolve_series(&mine, "  Photos  ").map(|x| x.id), Ok(3));
    }
}
