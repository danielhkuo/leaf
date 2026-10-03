//! Gateway event handling.
//!
//! Guilds becoming available (create the settings row, greet once) and
//! component presses and modal submits that no collector owns. The channel-selection logic is
//! a pure function so the policy is testable without Discord.

use leaf_core::db::GuildSettingsRepo;
use poise::serenity_prelude as serenity;
use tracing::{info, warn};

use crate::{Data, Error, checks, components};

/// The greeting for a server leaf has just been added to. `setup` is how
/// the `/setup` command is written (see [`checks::setup_mention`]).
#[must_use]
pub fn greeting(setup: &str) -> String {
    format!(
        "🍃 Thanks for adding leaf. It keeps a gallery of ongoing series, one numbered post at \
         a time. To get started, a server admin runs {setup} and chooses the series channels."
    )
}

/// The greeting for a server that was set up before leaf left and came
/// back: nothing needs doing again.
#[must_use]
pub fn greeting_back(setup: &str) -> String {
    format!(
        "🍃 leaf is back. This server's series and settings are as they were. A server admin \
         can review them with {setup}."
    )
}

/// How long a greeting waits for the commands to be registered before it
/// names `/setup` in plain text instead.
const GREETING_PATIENCE: std::time::Duration = std::time::Duration::from_secs(15);

/// Poise event hook.
pub async fn handle(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::GuildCreate { guild, is_new } => {
            on_guild_available(ctx, guild, *is_new, data).await;
        }
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(press),
        } => components::route(ctx, press, data).await?,
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Modal(submit),
        } => components::route_modal(ctx, submit).await,
        _ => {}
    }
    Ok(())
}

/// One text channel the bot could greet in.
#[derive(Debug, Clone, Copy)]
pub struct ChannelCandidate {
    /// Channel id.
    pub id: u64,
    /// Sort position in the guild sidebar.
    pub position: u16,
    /// Is a plain text channel.
    pub is_text: bool,
    /// Bot holds View Channel + Send Messages here.
    pub can_send: bool,
}

/// Picks where the greeting goes: the system channel when usable, else the
/// top-most (lowest position, then lowest id) sendable text channel, else
/// nowhere (greeting silently skipped).
#[must_use]
pub fn pick_greeting_channel(system: Option<u64>, candidates: &[ChannelCandidate]) -> Option<u64> {
    let usable = |c: &&ChannelCandidate| c.is_text && c.can_send;

    if let Some(sys) = system
        && candidates.iter().filter(usable).any(|c| c.id == sys)
    {
        return Some(sys);
    }
    candidates
        .iter()
        .filter(usable)
        .min_by_key(|c| (c.position, c.id))
        .map(|c| c.id)
}

/// Whether a guild that just became available should be greeted.
///
/// `inserted` is true when leaf had no settings row for it: the first time
/// leaf sees the guild, which covers a guild joined while leaf was offline
/// (it arrives at connect with `is_new` false). `is_new` true is a join
/// while online, which also covers a guild leaf was removed from and added
/// to again. A guild leaf already knew, seen again at connect, is neither.
#[must_use]
pub fn should_greet(inserted: bool, is_new: Option<bool>) -> bool {
    inserted || is_new == Some(true)
}

/// Makes sure the guild has a settings row (the admin panel only lists
/// guilds that have one) and greets it when it is new to leaf.
async fn on_guild_available(
    ctx: &serenity::Context,
    guild: &serenity::Guild,
    is_new: Option<bool>,
    data: &Data,
) {
    let guild_id = guild.id.to_string();
    let guilds = GuildSettingsRepo::new(data.pool.clone());
    let inserted = match guilds.ensure_exists(&guild_id).await {
        Ok(inserted) => inserted,
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "could not create guild settings row");
            false
        }
    };
    if !should_greet(inserted, is_new) {
        return;
    }
    info!(guild = %guild_id, name = %guild.name, "joined guild");

    let set_up = match guilds.get(&guild_id).await {
        Ok(settings) => settings.is_some_and(|s| s.setup_complete),
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "could not read guild settings");
            false
        }
    };
    // A guild present at connect arrives before the commands are
    // registered, and nobody is waiting on a greeting: give registration a
    // moment so `/setup` is a chip the admin can tap.
    let setup = checks::setup_mention_once_registered(data, GREETING_PATIENCE).await;
    let text = if set_up {
        greeting_back(&setup)
    } else {
        greeting(&setup)
    };

    let bot_id = ctx.cache.current_user().id;
    let member = match guild.member(ctx, bot_id).await {
        Ok(member) => member,
        Err(e) => {
            warn!(guild = %guild_id, error = %e, "could not fetch own member; skipping greeting");
            return;
        }
    };

    let candidates: Vec<ChannelCandidate> = guild
        .channels
        .values()
        .map(|ch| {
            let perms = guild.user_permissions_in(ch, &member);
            ChannelCandidate {
                id: ch.id.get(),
                position: ch.position,
                is_text: ch.kind == serenity::ChannelType::Text,
                can_send: perms.view_channel() && perms.send_messages(),
            }
        })
        .collect();

    let Some(target) = pick_greeting_channel(
        guild.system_channel_id.map(serenity::ChannelId::get),
        &candidates,
    ) else {
        warn!(guild = %guild_id, "no channel I can speak in; greeting skipped");
        return;
    };

    let channel = serenity::ChannelId::new(target);
    // The setup mention is a command chip, not a ping: nobody is notified.
    let message = serenity::CreateMessage::new()
        .content(text)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Err(e) = channel.send_message(&ctx.http, message).await {
        warn!(guild = %guild_id, channel = target, error = %e, "greeting failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: u64, position: u16, is_text: bool, can_send: bool) -> ChannelCandidate {
        ChannelCandidate {
            id,
            position,
            is_text,
            can_send,
        }
    }

    #[test]
    fn prefers_usable_system_channel() {
        let cands = [ch(1, 5, true, true), ch(2, 0, true, true)];
        assert_eq!(pick_greeting_channel(Some(1), &cands), Some(1));
    }

    #[test]
    fn unusable_system_channel_falls_back_to_topmost() {
        // System channel exists but bot can't send there.
        let cands = [
            ch(1, 0, true, false),
            ch(2, 3, true, true),
            ch(3, 1, true, true),
        ];
        assert_eq!(pick_greeting_channel(Some(1), &cands), Some(3));
    }

    #[test]
    fn ignores_non_text_and_unsendable() {
        let cands = [
            ch(1, 0, false, true), // voice-ish
            ch(2, 1, true, false), // no perms
            ch(3, 2, true, true),
        ];
        assert_eq!(pick_greeting_channel(None, &cands), Some(3));
    }

    #[test]
    fn position_ties_break_by_id_deterministically() {
        let cands = [ch(9, 1, true, true), ch(4, 1, true, true)];
        assert_eq!(pick_greeting_channel(None, &cands), Some(4));
    }

    #[test]
    fn greets_a_guild_once_however_it_arrived() {
        // Joined while leaf was offline: seen at connect, no row yet.
        assert!(should_greet(true, Some(false)));
        // Joined while online, first time or again after a kick.
        assert!(should_greet(true, Some(true)));
        assert!(should_greet(false, Some(true)));
        // Already known, seen again at every connect.
        assert!(!should_greet(false, Some(false)));
        assert!(!should_greet(false, None));
    }

    #[test]
    fn greetings_name_the_next_step_in_the_terms_setup_uses() {
        let first = greeting("</setup:9>");
        assert!(first.contains("</setup:9>"));
        assert!(first.contains("series channels"));
        assert!(!first.contains("watched"));
        let back = greeting_back("`/setup`");
        assert!(back.contains("as they were"));
    }

    #[test]
    fn nowhere_to_speak_is_none() {
        assert_eq!(pick_greeting_channel(None, &[]), None);
        let cands = [ch(1, 0, true, false)];
        assert_eq!(pick_greeting_channel(Some(1), &cands), None);
    }
}
