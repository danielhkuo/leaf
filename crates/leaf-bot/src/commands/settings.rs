//! `/settings` — granular editors over the same rows `/setup` writes.

use poise::serenity_prelude as serenity;

use crate::{Context, Error, checks};

/// View or change leaf's settings for this server.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("show", "policy", "timezone")
)]
#[allow(
    clippy::unused_async,
    reason = "poise requires command fns to be async"
)]
pub async fn settings(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Show the current configuration.
#[poise::command(slash_command, guild_only)]
async fn show(ctx: Context<'_>) -> Result<(), Error> {
    let Some(gid) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    let Some(s) = ctx.data().guilds.get(&gid).await? else {
        ctx.say("🌱 Not set up yet — run `/setup`.").await?;
        return Ok(());
    };

    let watched: Vec<String> = s
        .watched_channels
        .iter()
        .map(|c| format!("<#{c}>"))
        .collect();
    let role = s
        .creator_role_id
        .as_ref()
        .map_or_else(|| "none".to_owned(), |r| format!("<@&{r}>"));
    let log = s
        .log_channel_id
        .as_ref()
        .map_or_else(|| "none".to_owned(), |c| format!("<#{c}>"));

    ctx.send(
        poise::CreateReply::default()
            .content(format!(
                "🍃 **leaf settings**\n\
                 Setup complete: {}\n\
                 Watched channels: {}\n\
                 Log channel: {log}\n\
                 Timezone: `{}`\n\
                 Creator role: {role}\n\
                 Max series per user: {}\n\
                 Min account age: {} days\n\
                 Min membership age: {} days\n\
                 Sprout probation: {} (threshold {})",
                if s.setup_complete { "yes" } else { "no" },
                if watched.is_empty() {
                    "none".to_owned()
                } else {
                    watched.join(" ")
                },
                s.timezone,
                s.max_series_per_user,
                s.min_account_age_days,
                s.min_membership_age_days,
                if s.sprout_enabled { "on" } else { "off" },
                s.sprout_threshold,
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Tune the series-creation policy. Only the options you pass change.
#[poise::command(slash_command, guild_only)]
#[allow(clippy::too_many_arguments, reason = "each option is one policy knob")]
async fn policy(
    ctx: Context<'_>,
    #[description = "Max live series per creator (1–25)"]
    #[min = 1]
    #[max = 25]
    max_series_per_user: Option<i64>,
    #[description = "Minimum Discord account age in days (0 = off)"]
    #[min = 0]
    #[max = 3650]
    min_account_age_days: Option<i64>,
    #[description = "Minimum server membership in days (0 = off)"]
    #[min = 0]
    #[max = 3650]
    min_membership_age_days: Option<i64>,
    #[description = "New series start hidden until they have a few posts"] sprout: Option<bool>,
    #[description = "Posts needed to leave sprout (1–25)"]
    #[min = 1]
    #[max = 25]
    sprout_threshold: Option<i64>,
    #[description = "Role required to create series"] creator_role: Option<serenity::Role>,
    #[description = "Remove the creator-role requirement"] clear_creator_role: Option<bool>,
) -> Result<(), Error> {
    let Some(gid) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    ctx.data().guilds.ensure_exists(&gid).await?;
    let Some(mut s) = ctx.data().guilds.get(&gid).await? else {
        return Ok(());
    };

    if let Some(v) = max_series_per_user {
        s.max_series_per_user = v;
    }
    if let Some(v) = min_account_age_days {
        s.min_account_age_days = v;
    }
    if let Some(v) = min_membership_age_days {
        s.min_membership_age_days = v;
    }
    if let Some(v) = sprout {
        s.sprout_enabled = v;
    }
    if let Some(v) = sprout_threshold {
        s.sprout_threshold = v;
    }
    if let Some(role) = creator_role {
        s.creator_role_id = Some(role.id.to_string());
    }
    if clear_creator_role == Some(true) {
        s.creator_role_id = None;
    }

    ctx.data().guilds.upsert(&s).await?;
    ctx.send(
        poise::CreateReply::default()
            .content("🍃 Policy updated. `/settings show` to review.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Set the server's default timezone (IANA name, e.g. America/Chicago).
#[poise::command(slash_command, guild_only)]
async fn timezone(
    ctx: Context<'_>,
    #[description = "IANA timezone"]
    #[autocomplete = "autocomplete_tz"]
    tz: String,
) -> Result<(), Error> {
    let Some(gid) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    if tz.parse::<chrono_tz::Tz>().is_err() {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "`{tz}` isn't a timezone I know — pick one from the list."
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    ctx.data().guilds.ensure_exists(&gid).await?;
    if let Some(mut s) = ctx.data().guilds.get(&gid).await? {
        s.timezone = tz.clone();
        ctx.data().guilds.upsert(&s).await?;
    }
    ctx.send(
        poise::CreateReply::default()
            .content(format!("🍃 Timezone set to `{tz}`."))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Filters the IANA list by the typed fragment.
#[allow(
    clippy::unused_async,
    reason = "poise requires autocomplete fns to be async"
)]
async fn autocomplete_tz(_ctx: Context<'_>, partial: &str) -> Vec<String> {
    let needle = partial.to_ascii_lowercase();
    chrono_tz::TZ_VARIANTS
        .iter()
        .map(|tz| tz.name().to_owned())
        .filter(|name| name.to_ascii_lowercase().contains(&needle))
        .take(25)
        .collect()
}
