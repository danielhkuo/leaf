//! Slash commands and context menus. Each command's doc comment doubles
//! as its description in the Discord UI (poise picks it up).

#![allow(
    missing_docs,
    reason = "poise::command emits undocumented public wrapper fns; every \
              command here still carries a doc comment as its UI description"
)]

use std::sync::atomic::Ordering;
use std::time::Duration;

use leaf_core::db::LaunchIntentRepo;
use poise::serenity_prelude as serenity;

use crate::components::OPEN_GALLERY_FALLBACK;
use crate::{Context, Data, Error, checks};

pub mod archive;
pub mod query;
pub mod series_lookup;
pub mod setup;
pub mod transfer;
pub mod wrapped;

use series_lookup::{
    Asker, Scope, autocomplete_gallery_series, open_gallery_button, resolve_series,
};

/// Every command leaf registers, in menu order.
#[must_use]
pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![
        gallery(),
        leaf(),
        setup::setup(),
        archive::archive_menu(),
        query::search(),
        query::status(),
        query::random(),
        query::delete(),
        query::delete_menu(),
        transfer::export(),
        transfer::import(),
        wrapped::wrapped(),
        ping(),
    ]
}

/// Check that leaf is alive (version and uptime).
#[poise::command(slash_command, install_context = "Guild")]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let uptime = format_uptime(ctx.data().started.elapsed());
    let text = format!("🍃 leaf v{} is up ({uptime}).", env!("CARGO_PKG_VERSION"));
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

/// Open leaf's gallery.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn gallery(
    ctx: Context<'_>,
    #[description = "Series to open (leave it out to open the gallery itself)"]
    #[autocomplete = "autocomplete_gallery_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let poise::Context::Application(app) = ctx else {
        return Ok(());
    };
    let Some(guild_id) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };

    // Discord's launch response carries no destination, so a named series
    // is left as a short-lived intent the gallery collects when it starts.
    if let Some(name) = series.as_deref().filter(|n| !n.trim().is_empty()) {
        let Some(found) = resolve_series(&ctx, &guild_id, name, Scope::Viewable).await? else {
            return Ok(());
        };
        // An admin can name any series, but the gallery shows them only
        // what a member may view: say so, rather than leave a destination
        // the gallery would drop and open somewhere else.
        if !Asker::of(&ctx).await.gallery_shows(&found) {
            let text =
                crate::components::not_in_gallery_text(&found, ctx.data().public_url.as_deref());
            ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
                .await?;
            return Ok(());
        }
        let stored = LaunchIntentRepo::new(ctx.data().pool.clone())
            .put(
                &ctx.author().id.to_string(),
                &guild_id,
                found.id,
                None,
                checks::now_unix(),
            )
            .await;
        if let Err(e) = stored {
            // The gallery still opens, on its usual first screen.
            tracing::warn!(series = found.id, error = %e, "could not store the launch intent");
        }
    }

    let launch = serenity::CreateInteractionResponse::LaunchActivity;
    match app.interaction.create_response(ctx.http(), launch).await {
        Ok(()) => {
            // poise did not send this response: tell it one went out, so a
            // later error reply becomes a follow-up.
            app.has_sent_initial_response.store(true, Ordering::SeqCst);
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not launch the Activity");
            ctx.send(
                poise::CreateReply::default()
                    .content(OPEN_GALLERY_FALLBACK)
                    .ephemeral(true),
            )
            .await?;
        }
    }
    Ok(())
}

/// How leaf works here: starting a series, archiving a post, the gallery.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn leaf(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    let settings = ctx.data().guilds.get(&guild_id).await?;
    let reply = settings.filter(|s| s.setup_complete).map_or_else(
        || {
            let setup = checks::setup_mention(ctx.data());
            poise::CreateReply::default()
                .content(checks::not_set_up_text(checks::is_admin(&ctx), &setup))
        },
        |settings| {
            poise::CreateReply::default()
                .content(setup::how_to_text(&settings))
                .components(vec![serenity::CreateActionRow::Buttons(vec![
                    open_gallery_button(None).style(serenity::ButtonStyle::Primary),
                ])])
        },
    );
    ctx.send(reply.ephemeral(true)).await?;
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
    fn the_gallery_and_help_commands_are_registered_once_each() {
        let names: Vec<String> = all().into_iter().map(|c| c.name).collect();
        for expected in ["gallery", "leaf", "setup", "ping"] {
            assert_eq!(
                names.iter().filter(|n| *n == expected).count(),
                1,
                "{expected} in {names:?}"
            );
        }
    }

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
