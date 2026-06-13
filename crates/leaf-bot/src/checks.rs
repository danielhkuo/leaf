//! Shared command guards and small interaction helpers.

use leaf_core::domain::GuildSettings;
use poise::serenity_prelude as serenity;

use crate::{Context, Error};

/// Unix now, as the policy layer expects.
#[must_use]
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// True if `s` is a 24-hour `HH:MM` time (the reminder-time format).
#[must_use]
pub fn valid_hh_mm(s: &str) -> bool {
    chrono::NaiveTime::parse_from_str(s, "%H:%M").is_ok()
}

/// The invoking guild id as a string, or a friendly refusal in DMs.
/// (Commands are `guild_only`, so this is a belt-and-braces guard.)
pub async fn guild_id(ctx: &Context<'_>) -> Result<Option<String>, Error> {
    if let Some(id) = ctx.guild_id() {
        Ok(Some(id.to_string()))
    } else {
        ctx.say("🍂 This only works inside a server.").await?;
        Ok(None)
    }
}

/// Loads guild settings iff `/setup` has been completed; otherwise tells
/// the user what's missing and returns `None`. Single gate for every
/// series/archive feature.
pub async fn setup_settings(ctx: &Context<'_>) -> Result<Option<GuildSettings>, Error> {
    let Some(gid) = guild_id(ctx).await? else {
        return Ok(None);
    };
    let settings = ctx.data().guilds.get(&gid).await?;
    match settings {
        Some(s) if s.setup_complete => Ok(Some(s)),
        _ => {
            ctx.send(
                poise::CreateReply::default()
                    .content(
                        "🌱 leaf isn't set up here yet — an admin needs to run `/setup` first.",
                    )
                    .ephemeral(true),
            )
            .await?;
            Ok(None)
        }
    }
}

/// Whether the invoker holds Manage Guild (interaction-provided perms).
#[must_use]
pub fn is_admin(ctx: &Context<'_>) -> bool {
    ctx.author_member_permissions()
        .is_some_and(serenity::Permissions::manage_guild)
}

/// Extension-ish helper: permissions Discord attached to the invoking
/// member on this interaction.
trait MemberPerms {
    fn author_member_permissions(&self) -> Option<serenity::Permissions>;
}

impl MemberPerms for Context<'_> {
    fn author_member_permissions(&self) -> Option<serenity::Permissions> {
        match self {
            poise::Context::Application(app) => app
                .interaction
                .member
                .as_deref()
                .and_then(|m| m.permissions),
            poise::Context::Prefix(_) => None,
        }
    }
}

/// Sends an ephemeral confirm/cancel prompt; resolves to the user's pick
/// (`false` on timeout). Message-scoped collector: only this prompt's
/// buttons count, and only the invoker can press them.
pub async fn confirm(ctx: &Context<'_>, prompt: &str) -> Result<bool, Error> {
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .content(prompt.to_owned())
                .ephemeral(true)
                .components(vec![serenity::CreateActionRow::Buttons(vec![
                    serenity::CreateButton::new("confirm-yes")
                        .style(serenity::ButtonStyle::Danger)
                        .label("Yes, do it"),
                    serenity::CreateButton::new("confirm-no")
                        .style(serenity::ButtonStyle::Secondary)
                        .label("Cancel"),
                ])]),
        )
        .await?;

    let message = reply.message().await?;
    let pressed = message
        .await_component_interaction(ctx.serenity_context())
        .author_id(ctx.author().id)
        .timeout(std::time::Duration::from_mins(1))
        .await;

    let confirmed = pressed
        .as_ref()
        .is_some_and(|p| p.data.custom_id == "confirm-yes");

    let outcome = if confirmed {
        "Confirmed."
    } else {
        "Cancelled — nothing changed."
    };
    if let Some(press) = pressed {
        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(outcome)
                        .components(vec![]),
                ),
            )
            .await?;
    } else {
        reply
            .edit(
                *ctx,
                poise::CreateReply::default()
                    .content(outcome)
                    .components(vec![]),
            )
            .await?;
    }
    Ok(confirmed)
}

/// Writes a quiet one-liner to the guild's log channel, if configured.
/// Failures are logged, never surfaced to the user.
pub async fn log_line(ctx: &Context<'_>, settings: &GuildSettings, line: &str) {
    let Some(channel) = &settings.log_channel_id else {
        return;
    };
    let Ok(id) = channel.parse::<u64>() else {
        return;
    };
    if let Err(e) = serenity::ChannelId::new(id).say(ctx.http(), line).await {
        tracing::warn!(channel, error = %e, "log-channel write failed");
    }
}
