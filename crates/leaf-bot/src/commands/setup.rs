//! `/setup` — the per-guild first-run flow: pick watched channels and the
//! log channel with native channel-select menus, then save. Re-runnable;
//! pre-existing policy settings are preserved.
//!
//! Component handling follows serenity's e17 pattern: a message-scoped
//! interaction stream rather than ad-hoc collectors, so no presses can
//! fall between awaits.

use leaf_core::domain::GuildSettings;
use poise::serenity_prelude as serenity;
use serenity::futures::StreamExt as _;

use crate::{Context, Error, checks};

const WATCHED_ID: &str = "setup-watched";
const LOG_ID: &str = "setup-log";
const SAVE_ID: &str = "setup-save";
const CANCEL_ID: &str = "setup-cancel";

/// Configure leaf for this server: watched channels and log channel.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn setup(ctx: Context<'_>) -> Result<(), Error> {
    let Some(gid) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    ctx.data().guilds.ensure_exists(&gid).await?;
    let existing = ctx
        .data()
        .guilds
        .get(&gid)
        .await?
        .unwrap_or_else(|| GuildSettings::defaults_for(&gid));

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .content(
                    "🍃 **leaf setup** — pick the channels, then Save.\n\
                     Policies (limits, sprout probation, timezone) keep their \
                     current values; tune them in the admin panel.",
                )
                .ephemeral(true)
                .components(components()),
        )
        .await?;
    let message = reply.message().await?;

    let mut watched = existing.watched_channels.clone();
    let mut log_channel = existing.log_channel_id.clone();

    let mut presses = message
        .await_component_interaction(ctx.serenity_context())
        .author_id(ctx.author().id)
        .timeout(std::time::Duration::from_mins(5))
        .stream();

    while let Some(press) = presses.next().await {
        if let serenity::ComponentInteractionDataKind::ChannelSelect { values } = &press.data.kind {
            let picked: Vec<String> = values.iter().map(ToString::to_string).collect();
            if press.data.custom_id == WATCHED_ID {
                watched = picked;
            } else {
                log_channel = picked.first().cloned();
            }
            press
                .create_response(ctx, serenity::CreateInteractionResponse::Acknowledge)
                .await?;
            continue;
        }

        let done = if press.data.custom_id == CANCEL_ID {
            "Setup cancelled — nothing changed.".to_owned()
        } else if watched.is_empty() {
            "Pick at least one watched channel, then Save — run `/setup` again.".to_owned()
        } else {
            let mut updated = existing.clone();
            updated.watched_channels = watched.clone();
            updated.log_channel_id = log_channel.clone();
            updated.setup_complete = true;
            ctx.data().guilds.upsert(&updated).await?;
            summary(&watched, log_channel.as_deref())
        };

        press
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(done)
                        .components(vec![]),
                ),
            )
            .await?;
        return Ok(());
    }

    reply
        .edit(
            ctx,
            poise::CreateReply::default()
                .content("Setup timed out — run `/setup` again.")
                .components(vec![]),
        )
        .await?;
    Ok(())
}

fn components() -> Vec<serenity::CreateActionRow> {
    vec![
        serenity::CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                WATCHED_ID,
                serenity::CreateSelectMenuKind::Channel {
                    channel_types: Some(vec![serenity::ChannelType::Text]),
                    default_channels: None,
                },
            )
            .placeholder("Watched channels (where series post)")
            .min_values(1)
            .max_values(10),
        ),
        serenity::CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                LOG_ID,
                serenity::CreateSelectMenuKind::Channel {
                    channel_types: Some(vec![serenity::ChannelType::Text]),
                    default_channels: None,
                },
            )
            .placeholder("Log channel (optional, quiet confirmations)")
            .min_values(0)
            .max_values(1),
        ),
        serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(SAVE_ID)
                .style(serenity::ButtonStyle::Success)
                .label("Save"),
            serenity::CreateButton::new(CANCEL_ID)
                .style(serenity::ButtonStyle::Secondary)
                .label("Cancel"),
        ]),
    ]
}

fn summary(watched: &[String], log_channel: Option<&str>) -> String {
    let mentions: Vec<String> = watched.iter().map(|c| format!("<#{c}>")).collect();
    let log_text = log_channel.map_or_else(|| "none".to_owned(), |c| format!("<#{c}>"));
    format!(
        "🍃 **leaf is set up.**\nWatched: {}\nLog channel: {log_text}\n\
         Anyone can now start a series — open the leaf Activity to create one.",
        mentions.join(" ")
    )
}
