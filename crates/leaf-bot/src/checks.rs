//! Shared command guards and small interaction helpers.

use std::time::Duration;

use leaf_core::domain::GuildSettings;
use poise::serenity_prelude as serenity;

use crate::{Context, Data, Error};

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

/// The `/setup` command by its plain name: how it is written until its id
/// is known, and for someone who cannot run it (a chip they could tap would
/// only be refused).
pub const SETUP_PLAIN: &str = "`/setup`";

/// How to name the `/setup` command in a message to someone who can run it:
/// a mention Discord shows as a tappable chip once the command's id is
/// known, the plain name until then.
///
/// Every reply that tells an admin to run the command writes it with this.
#[must_use]
pub fn setup_mention(data: &Data) -> String {
    mention_for(*data.setup_command.borrow())
}

/// [`setup_mention`] for a message nobody is waiting on.
///
/// Gives the command registration up to `patience` to finish, so a message
/// sent right after connecting still gets the chip. The plain name after
/// that.
pub async fn setup_mention_once_registered(data: &Data, patience: Duration) -> String {
    mention_for(command_id_once_known(data.setup_command.clone(), patience).await)
}

/// The command id on `ids` once it is not zero, or whatever it is after
/// `patience` (or when the registration that would send it has ended).
async fn command_id_once_known(
    mut ids: tokio::sync::watch::Receiver<u64>,
    patience: Duration,
) -> u64 {
    // Timed out, or the sender is gone: either way the latest value is the
    // answer, so the outcome of the wait itself is not needed.
    let _waited = tokio::time::timeout(patience, ids.wait_for(|id| *id != 0))
        .await
        .map(|known| known.map(|id| *id));
    *ids.borrow()
}

fn mention_for(command_id: u64) -> String {
    if command_id == 0 {
        SETUP_PLAIN.to_owned()
    } else {
        format!("</setup:{command_id}>")
    }
}

/// What someone is told when the server has not been set up: an admin is
/// pointed at the command, anyone else at an admin.
#[must_use]
pub fn not_set_up_text(is_admin: bool, setup: &str) -> String {
    if is_admin {
        format!(
            "🌱 leaf isn't set up in this server yet. Run {setup} to choose the series channels."
        )
    } else {
        format!("🌱 leaf isn't set up in this server yet. Ask a server admin to run {SETUP_PLAIN}.")
    }
}

/// The invoking guild id as a string, or a friendly refusal in DMs.
/// (Commands are `guild_only`, so this is a belt-and-braces guard.)
pub async fn guild_id(ctx: &Context<'_>) -> Result<Option<String>, Error> {
    if let Some(id) = ctx.guild_id() {
        Ok(Some(id.to_string()))
    } else {
        ctx.send(
            poise::CreateReply::default()
                .content("🍂 This only works inside a server.")
                .ephemeral(true),
        )
        .await?;
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
            let text = not_set_up_text(is_admin(ctx), &setup_mention(ctx.data()));
            ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    #[test]
    fn setup_is_a_chip_once_its_id_is_known() {
        assert_eq!(mention_for(0), "`/setup`");
        assert_eq!(mention_for(42), "</setup:42>");
    }

    #[tokio::test]
    async fn a_message_can_wait_for_the_setup_command_s_id() {
        // Registration finishes while the message waits.
        let (tx, rx) = tokio::sync::watch::channel(0);
        let waiting = tokio::spawn(command_id_once_known(rx, Duration::from_secs(5)));
        tx.send(42).unwrap();
        assert_eq!(waiting.await.unwrap(), 42);

        // Already known: no wait.
        let (_tx, rx) = tokio::sync::watch::channel(7);
        assert_eq!(command_id_once_known(rx, Duration::ZERO).await, 7);

        // Still unknown when patience runs out: the plain name is used.
        let (_tx, rx) = tokio::sync::watch::channel(0);
        assert_eq!(
            command_id_once_known(rx, Duration::from_millis(20)).await,
            0
        );

        // Registration ended without an id (leaf is shutting down).
        let (tx, rx) = tokio::sync::watch::channel(0);
        drop(tx);
        assert_eq!(command_id_once_known(rx, Duration::from_secs(5)).await, 0);
    }

    #[test]
    fn not_set_up_copy_depends_on_who_can_fix_it() {
        let admin = not_set_up_text(true, "</setup:42>");
        assert!(admin.contains("Run </setup:42>"));
        let member = not_set_up_text(false, "</setup:42>");
        assert!(member.contains("Ask a server admin"));
        assert!(!member.contains("</setup:42>"));
    }
}
