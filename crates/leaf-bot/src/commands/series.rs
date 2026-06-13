//! `/series` — self-serve series lifecycle. `create` IS the application:
//! policy checks decide, no human approval step exists.

use leaf_core::domain::{Cadence, DetectionMode, NewSeries, Privacy, Series, SeriesState};
use leaf_core::policy;
use poise::serenity_prelude as serenity;

use crate::{Context, Error, checks};

/// Cadence choices shown in the slash UI.
#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum CadenceChoice {
    /// One post every day.
    #[name = "daily"]
    Daily,
    /// Monday through Friday.
    #[name = "weekdays"]
    Weekdays,
    /// Once a week.
    #[name = "weekly"]
    Weekly,
    /// No schedule.
    #[name = "freeform"]
    Freeform,
}

impl From<CadenceChoice> for Cadence {
    fn from(value: CadenceChoice) -> Self {
        match value {
            CadenceChoice::Daily => Self::Daily,
            CadenceChoice::Weekdays => Self::Weekdays,
            CadenceChoice::Weekly => Self::Weekly,
            CadenceChoice::Freeform => Self::Freeform,
        }
    }
}

/// Privacy choices shown in the slash UI.
#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum PrivacyChoice {
    /// Everyone in the server can view.
    #[name = "public"]
    Public,
    /// Only holders of a chosen role can view.
    #[name = "role-gated"]
    RoleGated,
    /// Only you (and admins) can view.
    #[name = "creator-only"]
    CreatorOnly,
}

impl From<PrivacyChoice> for Privacy {
    fn from(value: PrivacyChoice) -> Self {
        match value {
            PrivacyChoice::Public => Self::Public,
            PrivacyChoice::RoleGated => Self::RoleGated,
            PrivacyChoice::CreatorOnly => Self::CreatorOnly,
        }
    }
}

/// Start and manage your series.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("create", "edit", "list", "remove", "reminder")
)]
#[allow(
    clippy::unused_async,
    reason = "poise requires command fns to be async"
)]
pub async fn series(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Start a new series — your ongoing daily archive.
#[poise::command(slash_command, guild_only)]
#[allow(
    clippy::too_many_arguments,
    reason = "each option is one creation field"
)]
async fn create(
    ctx: Context<'_>,
    #[description = "Series name (unique in this server)"]
    #[min_length = 2]
    #[max_length = 40]
    name: String,
    #[description = "The watched channel you'll post in"] channel: serenity::GuildChannel,
    #[description = "How often you plan to post"] cadence: CadenceChoice,
    #[description = "What this series is"]
    #[max_length = 200]
    description: Option<String>,
    #[description = "Who can view it (default: everyone here)"] privacy: Option<PrivacyChoice>,
    #[description = "Role for role-gated privacy"] privacy_role: Option<serenity::Role>,
    #[description = "First day number (default 1)"]
    #[min = 1]
    start_day: Option<i64>,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let gid = settings.guild_id.clone();
    let author = ctx.author();
    let creator_id = author.id.to_string();

    let channel_id = channel.id.to_string();
    if !policy::channel_allowed(&settings, &channel_id) {
        ctx.send(
            poise::CreateReply::default()
                .content(policy::PolicyViolation::ChannelNotWatched.to_string())
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let member = ctx.author_member().await;
    let creation = policy::CreationContext {
        now_unix: checks::now_unix(),
        account_created_unix: author.id.created_at().unix_timestamp(),
        joined_unix: member
            .as_ref()
            .and_then(|m| m.joined_at.map(|t| t.unix_timestamp())),
        live_series_count: ctx
            .data()
            .series
            .count_live_by_creator(&gid, &creator_id)
            .await?,
        has_creator_role: settings.creator_role_id.as_ref().map(|role| {
            let Ok(role_id) = role.parse::<u64>() else {
                return false;
            };
            member
                .as_ref()
                .is_some_and(|m| m.roles.contains(&serenity::RoleId::new(role_id)))
        }),
    };

    if let Err(violation) = policy::check_creation(&settings, &creation) {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("🍂 {violation}"))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let state = if settings.sprout_enabled {
        SeriesState::Sprout
    } else {
        SeriesState::Active
    };

    let new = NewSeries {
        guild_id: gid.clone(),
        creator_id,
        name: name.clone(),
        description: description.unwrap_or_default(),
        channels: vec![channel_id],
        cadence: cadence.into(),
        detection_mode: DetectionMode::ContextMenu,
        privacy: privacy.map_or(Privacy::Public, Into::into),
        privacy_role_id: privacy_role.map(|r| r.id.to_string()),
        start_day: start_day.unwrap_or(1),
        state,
    };

    match ctx.data().series.create(&new, checks::now_unix()).await {
        Ok(created) => {
            let sprout_note = if created.state == SeriesState::Sprout {
                format!(
                    "\n🌱 It starts as a *sprout*: archive {} posts and it goes public.",
                    settings.sprout_threshold
                )
            } else {
                String::new()
            };
            ctx.send(poise::CreateReply::default().content(format!(
                "🍃 **{}** is planted in <#{}>. Post, then right-click → \
                     Apps → *Archive to Series*.{sprout_note}",
                created.name, channel.id
            )))
            .await?;
            checks::log_line(
                &ctx,
                &settings,
                &format!("🍃 {} planted series **{}**", author.name, created.name),
            )
            .await;
        }
        Err(leaf_core::db::DbError::SeriesNameTaken) => {
            ctx.send(
                poise::CreateReply::default()
                    .content(format!("🍂 A series named **{name}** already exists here."))
                    .ephemeral(true),
            )
            .await?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Edit one of your series. Only the options you pass change.
#[poise::command(slash_command, guild_only)]
#[allow(
    clippy::too_many_arguments,
    reason = "each option is one editable field"
)]
async fn edit(
    ctx: Context<'_>,
    #[description = "Which series"]
    #[autocomplete = "autocomplete_own_series"]
    name: String,
    #[description = "New description"]
    #[max_length = 200]
    description: Option<String>,
    #[description = "New reaction emoji"]
    #[max_length = 8]
    emoji: Option<String>,
    #[description = "New cadence"] cadence: Option<CadenceChoice>,
    #[description = "New privacy"] privacy: Option<PrivacyChoice>,
    #[description = "Role for role-gated privacy"] privacy_role: Option<serenity::Role>,
    #[description = "Move to a different watched channel"] channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some(mut s) = owned_series(&ctx, &settings.guild_id, &name).await? else {
        return Ok(());
    };

    if let Some(v) = description {
        s.description = v;
    }
    if let Some(v) = emoji {
        s.emoji = v;
    }
    if let Some(v) = cadence {
        s.cadence = v.into();
    }
    if let Some(v) = privacy {
        s.privacy = v.into();
    }
    if let Some(r) = privacy_role {
        s.privacy_role_id = Some(r.id.to_string());
    }
    if let Some(ch) = channel {
        let id = ch.id.to_string();
        if !policy::channel_allowed(&settings, &id) {
            ctx.send(
                poise::CreateReply::default()
                    .content(policy::PolicyViolation::ChannelNotWatched.to_string())
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
        s.channels = vec![id];
    }

    ctx.data().series.update(&s).await?;
    ctx.send(
        poise::CreateReply::default()
            .content(format!("🍃 **{}** updated.", s.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// List series here — yours, or everyone's if you're an admin.
#[poise::command(slash_command, guild_only)]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let Some(gid) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    let all = ctx.data().series.list_by_guild(&gid).await?;
    let me = ctx.author().id.to_string();
    let admin = checks::is_admin(&ctx);

    let mut lines = Vec::new();
    for s in &all {
        if !admin && s.creator_id != me {
            continue;
        }
        let count = ctx.data().posts.count(s.id).await?;
        let state = match s.state {
            SeriesState::Sprout => " 🌱",
            SeriesState::Revoked => " (revoked)",
            SeriesState::Active => "",
        };
        lines.push(format!(
            "{} **{}**{state} — {count} days archived, {} in <#{}>",
            s.emoji,
            s.name,
            s.cadence,
            s.channels.first().map_or("?", String::as_str),
        ));
    }
    let content = if lines.is_empty() {
        "No series yet. `/series create` plants one.".to_owned()
    } else {
        lines.join("\n")
    };
    ctx.send(
        poise::CreateReply::default()
            .content(content)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Revoke a series (admin). It becomes hidden and read-only.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
async fn remove(
    ctx: Context<'_>,
    #[description = "Series name"]
    #[autocomplete = "autocomplete_any_series"]
    name: String,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some(s) = ctx
        .data()
        .series
        .get_by_name(&settings.guild_id, &name)
        .await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("No series named **{name}** here."))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    if !checks::confirm(
        &ctx,
        &format!(
            "Revoke **{}** by <@{}>? Its archive stays stored but becomes \
             hidden and read-only.",
            s.name, s.creator_id
        ),
    )
    .await?
    {
        return Ok(());
    }

    ctx.data()
        .series
        .set_state(s.id, SeriesState::Revoked)
        .await?;
    checks::log_line(
        &ctx,
        &settings,
        &format!("🗑️ series **{}** revoked by {}", s.name, ctx.author().name),
    )
    .await;
    Ok(())
}

/// Fetches a series by name iff the invoker owns it (or is an admin).
pub async fn owned_series(
    ctx: &Context<'_>,
    guild_id: &str,
    name: &str,
) -> Result<Option<Series>, Error> {
    let found = ctx.data().series.get_by_name(guild_id, name).await?;
    let Some(s) = found else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("No series named **{name}** here."))
                .ephemeral(true),
        )
        .await?;
        return Ok(None);
    };
    if s.creator_id != ctx.author().id.to_string() && !checks::is_admin(ctx) {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 Only the series creator (or an admin) can do that.")
                .ephemeral(true),
        )
        .await?;
        return Ok(None);
    }
    Ok(Some(s))
}

/// Turn reminders on or off for one of your series.
#[poise::command(slash_command, guild_only)]
async fn reminder(
    ctx: Context<'_>,
    #[description = "Which series"]
    #[autocomplete = "autocomplete_own_series"]
    name: String,
    #[description = "Enable or disable reminders"] enabled: bool,
    #[description = "Time of day, 24h HH:MM (required to enable)"] time: Option<String>,
    #[description = "DM me (default) or ping the channel"] dm: Option<bool>,
    #[description = "Timezone override (IANA, else server default)"] timezone: Option<String>,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let Some(s) = owned_series(&ctx, &settings.guild_id, &name).await? else {
        return Ok(());
    };

    if !enabled {
        ctx.data()
            .series
            .set_reminder_config(
                s.id,
                false,
                s.reminder_time.as_deref(),
                s.reminder_timezone.as_deref(),
                s.reminder_dm,
            )
            .await?;
        ctx.send(
            poise::CreateReply::default()
                .content(format!("🍃 Reminders off for **{}**.", s.name))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let Some(time) = time else {
        ctx.send(
            poise::CreateReply::default()
                .content("To enable reminders, give a `time` (24h HH:MM).")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if !checks::valid_hh_mm(&time) {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "`{time}` isn't a valid 24h time — use HH:MM, e.g. 17:30."
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if let Some(tz) = &timezone
        && tz.parse::<chrono_tz::Tz>().is_err()
    {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "`{tz}` isn't a timezone I know — pick a valid IANA name."
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if s.cadence == Cadence::Freeform {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 Freeform series have no schedule to remind against — set a cadence with `/series edit` first.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let dm = dm.unwrap_or(true);
    ctx.data()
        .series
        .set_reminder_config(s.id, true, Some(&time), timezone.as_deref(), dm)
        .await?;
    let tz_note = timezone
        .as_deref()
        .map_or_else(|| settings.timezone.clone(), ToOwned::to_owned);
    let how = if dm { "by DM" } else { "in the channel" };
    ctx.send(
        poise::CreateReply::default()
            .content(format!(
                "🍃 Reminders on for **{}** at `{time}` ({tz_note}), {how}, \
                 when you're behind.",
                s.name
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Autocomplete: the invoker's own non-revoked series names.
pub async fn autocomplete_own_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(gid) = ctx.guild_id().map(|g| g.to_string()) else {
        return Vec::new();
    };
    let me = ctx.author().id.to_string();
    let needle = partial.to_ascii_lowercase();
    ctx.data()
        .series
        .list_by_creator(&gid, &me)
        .await
        .map_or_else(
            |_| Vec::new(),
            |list| {
                list.into_iter()
                    .filter(|s| s.state != SeriesState::Revoked)
                    .map(|s| s.name)
                    .filter(|n| n.to_ascii_lowercase().contains(&needle))
                    .take(25)
                    .collect()
            },
        )
}

/// Autocomplete: every series name in the guild (admin commands).
pub async fn autocomplete_any_series(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(gid) = ctx.guild_id().map(|g| g.to_string()) else {
        return Vec::new();
    };
    let needle = partial.to_ascii_lowercase();
    ctx.data().series.list_by_guild(&gid).await.map_or_else(
        |_| Vec::new(),
        |list| {
            list.into_iter()
                .map(|s| s.name)
                .filter(|n| n.to_ascii_lowercase().contains(&needle))
                .take(25)
                .collect()
        },
    )
}
