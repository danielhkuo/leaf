//! Gateway event handling. Currently: greet newly joined guilds and create
//! their settings row. The channel-selection logic is a pure function so
//! the policy is testable without Discord.

use leaf_core::db::GuildSettingsRepo;
use poise::serenity_prelude as serenity;
use tracing::{info, warn};

use crate::{Data, Error};

/// Greeting posted once when leaf joins a guild.
pub const GREETING: &str = "🍃 Thanks for planting leaf! Before anyone can start a series, \
     an admin needs to run `/setup` to pick the watched channels and policies.";

/// Poise event hook.
pub async fn handle(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    if let serenity::FullEvent::GuildCreate {
        guild,
        is_new: Some(true),
    } = event
    {
        on_guild_join(ctx, guild, data).await;
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

async fn on_guild_join(ctx: &serenity::Context, guild: &serenity::Guild, data: &Data) {
    let guild_id = guild.id.to_string();
    info!(guild = %guild_id, name = %guild.name, "joined guild");

    if let Err(e) = GuildSettingsRepo::new(data.pool.clone())
        .ensure_exists(&guild_id)
        .await
    {
        warn!(guild = %guild_id, error = %e, "could not create guild settings row");
    }

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
    if let Err(e) = channel.say(&ctx.http, GREETING).await {
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
    fn nowhere_to_speak_is_none() {
        assert_eq!(pick_greeting_channel(None, &[]), None);
        let cands = [ch(1, 0, true, false)];
        assert_eq!(pick_greeting_channel(Some(1), &cands), None);
    }
}
