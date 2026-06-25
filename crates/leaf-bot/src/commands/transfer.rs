//! `/export` and `/import` — bulk JSON transfer.
//!
//! The format is walpurgisbot-v2-compatible. Export is the backup story;
//! import is the safety net (rows + day numbers land; media bytes are
//! `leaf-migrate`'s job).

use leaf_core::domain::{NewMediaAttachment, Post};
use leaf_core::transfer::{self, TransferPost};
use poise::serenity_prelude as serenity;

use crate::commands::series_lookup::autocomplete_any_series;
use crate::{Context, Error, checks};

/// Import files larger than this are refused (matches v2's guard).
const MAX_IMPORT_BYTES: u32 = 24 * 1024 * 1024;

/// Export a series' archive as JSON (v2-compatible format).
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn export(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "autocomplete_any_series"]
    series: String,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some(s) = ctx
        .data()
        .series
        .get_by_name(&settings.guild_id, &series)
        .await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("No series named **{series}** here."))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let days = ctx.data().posts.all_days(s.id).await?;
    let mut out = Vec::with_capacity(days.len());
    for day in days {
        let Some((post, media)) = ctx.data().posts.get(s.id, day).await? else {
            continue;
        };
        out.push(TransferPost {
            day: post.day,
            message_id: post.message_id,
            channel_id: post.channel_id,
            user_id: s.creator_id.clone(),
            timestamp: post.posted_at,
            media: media.into_iter().filter_map(|m| m.original_key).collect(),
        });
    }

    if out.is_empty() {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("**{}** has nothing to export yet.", s.name))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let count = out.len();
    let bytes = transfer::serialize(&out)?;
    ctx.send(
        poise::CreateReply::default()
            .content(format!("🍃 **{}** — {count} archived days.", s.name))
            .attachment(serenity::CreateAttachment::bytes(
                bytes,
                format!("leaf-export-{}.json", s.name),
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Import a JSON export (leaf or walpurgisbot-v2) into a series.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn import(
    ctx: Context<'_>,
    #[description = "Series to import into (must exist)"]
    #[autocomplete = "autocomplete_any_series"]
    series: String,
    #[description = "The .json export file"] file: serenity::Attachment,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some(s) = ctx
        .data()
        .series
        .get_by_name(&settings.guild_id, &series)
        .await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "No series named **{series}** here — create it in the leaf Activity first."
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    if file.size > MAX_IMPORT_BYTES {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 That file is over the 24MB import limit.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.defer_ephemeral().await?;
    let raw = file.download().await?;
    let posts = match transfer::parse(&raw) {
        Ok(p) => p,
        Err(e) => {
            ctx.send(
                poise::CreateReply::default()
                    .content(format!("🍂 {e}"))
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    // Dry-run summary, then confirm before any write.
    let existing = ctx.data().posts.all_days(s.id).await?;
    let collisions = posts
        .iter()
        .filter(|p| existing.binary_search(&p.day).is_ok())
        .count();
    let fresh = posts.len() - collisions;
    if !checks::confirm(
        &ctx,
        &format!(
            "Import into **{}**: {} entries — {fresh} new, {collisions} already \
             archived (will be skipped). Media is recorded as missing; \
             `leaf-migrate` re-fetches the actual files. Proceed?",
            s.name,
            posts.len()
        ),
    )
    .await?
    {
        return Ok(());
    }

    let (imported, skipped) = insert_all(&ctx, s.id, posts).await?;

    ctx.send(
        poise::CreateReply::default()
            .content(format!(
                "🍃 Import into **{}** done: {imported} imported, {skipped} skipped \
                 (already archived).",
                s.name
            ))
            .ephemeral(true),
    )
    .await?;
    checks::log_line(
        &ctx,
        &settings,
        &format!("📥 {imported} days imported into **{}**", s.name),
    )
    .await;
    Ok(())
}

/// Inserts every parsed post, skipping already-archived days. Media rows
/// land as `media_missing` placeholders for `leaf-migrate` to fill.
async fn insert_all(
    ctx: &Context<'_>,
    series_id: i64,
    posts: Vec<TransferPost>,
) -> Result<(u32, u32), Error> {
    let mut imported = 0_u32;
    let mut skipped = 0_u32;
    for p in posts {
        let media: Vec<NewMediaAttachment> = p
            .media
            .iter()
            .enumerate()
            .map(|(i, _)| NewMediaAttachment {
                attachment_id: format!("import-{}-{i}", p.message_id),
                channel_id: p.channel_id.clone(),
                message_id: p.message_id.clone(),
                content_type: String::new(),
                original_key: None,
                thumb_key: None,
                media_missing: true,
            })
            .collect();
        let post = Post {
            series_id,
            day: p.day,
            message_id: p.message_id,
            channel_id: p.channel_id,
            caption: String::new(),
            posted_at: p.timestamp,
            archived_at: checks::now_unix(),
        };
        match ctx.data().posts.insert_with_media(&post, &media).await {
            Ok(()) => imported += 1,
            Err(leaf_core::db::DbError::DuplicateDay(_)) => skipped += 1,
            Err(e) => return Err(e.into()),
        }
    }
    Ok((imported, skipped))
}
