//! Which of the channels leaf has stored it can still see, and how its
//! messages say so.
//!
//! leaf keeps channel ids: a server's series channels, its log channel, the
//! channel of each series. A channel can be deleted after that, or hidden
//! from leaf (from 2026-11-16 Discord obscures the channels a bot may not
//! view). A channel that has dropped out of what Discord lists for leaf is
//! one or the other, and leaf cannot tell which. Discord shows a mention of
//! a channel that is gone as "#unknown", so a message that sends people
//! there sends them nowhere.
//!
//! A message that lists stored channels therefore asks first which of them
//! leaf can see ([`Sight`]), mentions those and counts the rest
//! ([`Listed`]). What is said about the ones out of sight is written here
//! and nowhere else, so every message says it the same way.
//!
//! "Can see" means Discord lists the channel for leaf. A channel it still
//! lists, though leaf's role may not view it, counts as seen: it exists, its
//! mention resolves for whoever can view it, and `/setup` names it when it
//! warns that leaf lacks View Channel there. That holds for a channel the
//! gateway lists under [`HIDDEN_CHANNEL_NAME`] too.
//!
//! Discord's own list of the server's channels is asked for, and the gateway
//! cache only stands in when it does not come in time. The cache is not
//! enough on its own: serenity drops a gateway event it cannot decode without
//! a word, and a cache that missed the event that deleted a channel keeps
//! the channel for as long as the connection lasts.

use std::collections::HashSet;
use std::time::Duration;

use poise::serenity_prelude as serenity;

use crate::checks::SETUP_PLAIN;

/// Longest wait for Discord's channel list when nothing with a deadline is
/// waiting on it: a reminder, or a command that has already answered.
const LOOK_TIMEOUT: Duration = Duration::from_millis(1500);

/// Longest wait for it before the first answer to a command. Discord allows
/// that answer three seconds in all, and the command's way here, the
/// database and the answer's own way back come out of the same three.
const FIRST_ANSWER_LOOK_TIMEOUT: Duration = Duration::from_millis(800);

/// The name the gateway gives a channel the bot may not view (gateway
/// obfuscation, mandatory from 2026-11-16). Such a channel is known to
/// exist, with its real id.
pub const HIDDEN_CHANNEL_NAME: &str = "___hidden___";

/// What leaf can no longer do with a channel that is out of its sight.
const CANNOT_SEE: &str = "leaf can no longer see";

/// Why. Which of the two it is, leaf cannot tell.
const WHY: &str = "(deleted, or hidden from leaf)";

/// The channels of one server that leaf can see, as far as it could find
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sight(Option<HashSet<String>>);

impl Sight {
    /// Nothing could be found out. Every channel then counts as seen, and a
    /// message lists what is stored: that is all there is to go on.
    pub const fn unknown() -> Self {
        Self(None)
    }

    /// Exactly `ids` are in sight.
    #[cfg(test)]
    pub fn of<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(Some(ids.into_iter().map(Into::into).collect()))
    }

    /// Asks which channels of `guild` leaf can see: Discord's list of them,
    /// or the gateway cache's when Discord does not answer in time. With
    /// neither, nothing is known ([`Sight::unknown`]); that is not an error.
    pub async fn look(
        http: &serenity::Http,
        cache: &serenity::Cache,
        guild: serenity::GuildId,
    ) -> Self {
        Self::look_within(http, cache, guild, LOOK_TIMEOUT).await
    }

    /// [`Sight::look`] for the server a command was run in, once the command
    /// has answered (a defer counts): nothing is waiting on this.
    pub async fn of_command(ctx: &crate::Context<'_>) -> Self {
        Self::for_command(ctx, LOOK_TIMEOUT).await
    }

    /// The same, asked before the command's first answer. That answer has a
    /// deadline, so Discord's list gets less time
    /// ([`FIRST_ANSWER_LOOK_TIMEOUT`]) before the gateway cache stands in.
    pub async fn before_first_answer(ctx: &crate::Context<'_>) -> Self {
        Self::for_command(ctx, FIRST_ANSWER_LOOK_TIMEOUT).await
    }

    /// What leaf can see of the server `ctx`'s command was run in, waiting
    /// `patience` for Discord's list.
    async fn for_command(ctx: &crate::Context<'_>, patience: Duration) -> Self {
        let Some(guild) = ctx.guild_id() else {
            return Self::unknown();
        };
        let discord = ctx.serenity_context();
        Self::look_within(&discord.http, &discord.cache, guild, patience).await
    }

    /// [`Sight::look`], waiting `patience` for Discord's list.
    async fn look_within(
        http: &serenity::Http,
        cache: &serenity::Cache,
        guild: serenity::GuildId,
        patience: Duration,
    ) -> Self {
        let cached = Cached::of(cache, guild);
        match tokio::time::timeout(patience, listed(http, guild)).await {
            Ok(Ok(mut ids)) => {
                // Discord's list has no threads; the gateway names the
                // active ones. And a channel the gateway marks as hidden
                // exists, whether or not the list leaves it out.
                if let Some(cached) = cached {
                    ids.extend(cached.threads);
                    ids.extend(cached.hidden);
                }
                Self(Some(ids))
            }
            Ok(Err(e)) => {
                tracing::warn!(%guild, error = %e, "channel list refused; using the gateway cache");
                Self(cached.map(Cached::all))
            }
            Err(_) => {
                tracing::warn!(%guild, "channel list timed out; using the gateway cache");
                Self(cached.map(Cached::all))
            }
        }
    }

    /// Whether leaf can see the channel `id`.
    pub fn sees(&self, id: &str) -> bool {
        self.0.as_ref().is_none_or(|seen| seen.contains(id))
    }

    /// The ids in sight; `None` when nothing could be found out.
    pub const fn ids(&self) -> Option<&HashSet<String>> {
        self.0.as_ref()
    }

    /// Takes `ids` into sight: channels someone just picked in one of
    /// Discord's own menus, which offer only channels that exist.
    pub fn admit<I, S>(&mut self, ids: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if let Some(seen) = &mut self.0 {
            seen.extend(ids.into_iter().map(Into::into));
        }
    }

    /// `ids` split into the ones in sight, in their order, and a count of
    /// the others.
    pub fn sort<'a, S: AsRef<str>>(&self, ids: &'a [S]) -> Listed<'a> {
        let seen: Vec<&str> = ids
            .iter()
            .map(AsRef::as_ref)
            .filter(|id| self.sees(id))
            .collect();
        Listed {
            unseen: ids.len() - seen.len(),
            seen,
        }
    }
}

/// What the gateway cache has of a server.
struct Cached {
    channels: HashSet<String>,
    /// Its active threads.
    threads: HashSet<String>,
    /// Those of `channels` listed under [`HIDDEN_CHANNEL_NAME`].
    hidden: HashSet<String>,
}

impl Cached {
    /// `None` when the server is not cached.
    fn of(cache: &serenity::Cache, guild: serenity::GuildId) -> Option<Self> {
        let guild = cache.guild(guild)?;
        let channels = guild.channels.keys().map(ToString::to_string).collect();
        let threads = guild
            .threads
            .iter()
            .map(|thread| thread.id.to_string())
            .collect();
        let hidden = guild
            .channels
            .values()
            .filter(|channel| channel.name == HIDDEN_CHANNEL_NAME)
            .map(|channel| channel.id.to_string())
            .collect();
        drop(guild);
        Some(Self {
            channels,
            threads,
            hidden,
        })
    }

    fn all(self) -> HashSet<String> {
        let mut all = self.channels;
        all.extend(self.threads);
        all
    }
}

/// The ids of the server's channels, asked of Discord.
///
/// Only the ids are read. serenity's own decoder refuses the whole list
/// over one field it does not expect in one channel.
async fn listed(
    http: &serenity::Http,
    guild: serenity::GuildId,
) -> Result<HashSet<String>, serenity::Error> {
    let request = serenity::Request::new(
        serenity::Route::GuildChannels { guild_id: guild },
        serenity::LightMethod::Get,
    );
    let channels: Vec<serenity::json::Value> = http.fire(request).await?;
    Ok(channels
        .iter()
        .filter_map(|channel| channel.get("id").and_then(serenity::json::Value::as_str))
        .map(str::to_owned)
        .collect())
}

/// How the channels leaf can see are strung together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// "#a, #b": the first `cap` of them, then an ellipsis for the rest.
    Commas {
        /// Channels named before the ellipsis.
        cap: usize,
    },
    /// "#a #b": every one of them.
    Spaces,
}

/// Stored channels, split by whether leaf can see them (see
/// [`Sight::sort`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed<'a> {
    /// The ones leaf can see, in the order they are stored.
    pub seen: Vec<&'a str>,
    /// How many it cannot.
    pub unseen: usize,
}

impl Listed<'_> {
    /// True when there are channels and leaf can see none of them. There is
    /// then nowhere to send anyone.
    pub const fn none_seen(&self) -> bool {
        self.seen.is_empty() && self.unseen > 0
    }

    /// The channels as one list: the ones leaf can see as mentions, the
    /// others counted after them ("#a, #b and 1 that leaf can no longer
    /// see"). When it can see none, the count stands alone, with the reason.
    /// Empty when there is no channel at all.
    pub fn text(&self, style: Style) -> String {
        if self.none_seen() {
            return format!("{} that {CANNOT_SEE} {WHY}", self.unseen);
        }
        let mention = |id: &&str| format!("<#{id}>");
        let shown = match style {
            Style::Commas { cap } => {
                let mut shown = self
                    .seen
                    .iter()
                    .take(cap)
                    .map(mention)
                    .collect::<Vec<_>>()
                    .join(", ");
                if self.seen.len() > cap {
                    shown.push_str(", …");
                }
                shown
            }
            Style::Spaces => self.seen.iter().map(mention).collect::<Vec<_>>().join(" "),
        };
        if self.unseen == 0 {
            return shown;
        }
        format!("{shown} and {} that {CANNOT_SEE}", self.unseen)
    }
}

/// The sentence for a server whose `count` series channels are all out of
/// sight.
pub fn none_seen_text(count: usize) -> String {
    if count == 1 {
        format!("This server's series channel is one {CANNOT_SEE} {WHY}.")
    } else {
        format!("This server's series channels are ones {CANNOT_SEE} {WHY}.")
    }
}

/// What to do about series channels that are out of sight. `setup` is how
/// the `/setup` command is written for someone who can run it (see
/// [`crate::checks::setup_mention`]); anyone else is pointed at an admin.
pub fn choose_new_text(is_admin: bool, setup: &str) -> String {
    if is_admin {
        format!("Choose new ones with {setup}.")
    } else {
        format!("A server admin can choose new ones with {SETUP_PLAIN}.")
    }
}

/// "leaf can no longer see the log channel (deleted, or hidden from leaf)":
/// the start of a sentence about `what`, which leaf has stored and lost
/// sight of.
pub fn lost(what: &str) -> String {
    format!("{CANNOT_SEE} {what} {WHY}")
}

/// Where a series posts when its channel is out of sight, for after "posts
/// in": "a channel leaf can no longer see (deleted, or hidden from leaf)".
pub fn unseen_place(plural: bool) -> String {
    if plural {
        format!("channels {CANNOT_SEE} {WHY}")
    } else {
        format!("a channel {CANNOT_SEE} {WHY}")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "tests may panic")]

    use super::*;
    use crate::stub;

    const COMMAS: Style = Style::Commas { cap: 3 };

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| (*id).to_owned()).collect()
    }

    // -- the list ------------------------------------------------------------

    #[test]
    fn channels_leaf_can_see_are_listed_as_mentions() {
        let stored = ids(&["1", "2"]);
        let listed = Sight::of(["1", "2", "9"]).sort(&stored);
        assert_eq!(listed.seen, ["1", "2"]);
        assert_eq!(listed.unseen, 0);
        assert!(!listed.none_seen());
        assert_eq!(listed.text(COMMAS), "<#1>, <#2>");
        assert_eq!(listed.text(Style::Spaces), "<#1> <#2>");
    }

    #[test]
    fn channels_out_of_sight_are_counted_and_never_mentioned() {
        let stored = ids(&["1", "2"]);
        let listed = Sight::of(["9"]).sort(&stored);
        assert!(listed.seen.is_empty());
        assert_eq!(listed.unseen, 2);
        assert!(listed.none_seen());
        for style in [COMMAS, Style::Spaces] {
            let text = listed.text(style);
            assert_eq!(
                text,
                "2 that leaf can no longer see (deleted, or hidden from leaf)"
            );
            assert!(!text.contains("<#"), "{text}");
        }

        assert_eq!(
            none_seen_text(1),
            "This server's series channel is one leaf can no longer see (deleted, or hidden \
             from leaf)."
        );
        assert_eq!(
            none_seen_text(2),
            "This server's series channels are ones leaf can no longer see (deleted, or hidden \
             from leaf)."
        );
    }

    #[test]
    fn a_mix_lists_the_ones_in_sight_and_counts_the_rest() {
        let stored = ids(&["1", "gone", "2", "hidden"]);
        let listed = Sight::of(["2", "1"]).sort(&stored);
        // In stored order, whatever order the sight has them in.
        assert_eq!(listed.seen, ["1", "2"]);
        assert_eq!(listed.unseen, 2);
        assert!(!listed.none_seen());
        assert_eq!(
            listed.text(COMMAS),
            "<#1>, <#2> and 2 that leaf can no longer see"
        );
        assert_eq!(
            listed.text(Style::Spaces),
            "<#1> <#2> and 2 that leaf can no longer see"
        );

        let one = ids(&["1", "gone"]);
        assert_eq!(
            Sight::of(["1"]).sort(&one).text(COMMAS),
            "<#1> and 1 that leaf can no longer see"
        );
    }

    #[test]
    fn a_long_list_stops_at_the_cap_and_still_counts_what_is_out_of_sight() {
        let stored: Vec<String> = (1..=6).map(|n| n.to_string()).collect();
        let all = Sight::of(stored.clone());
        assert_eq!(all.sort(&stored).text(COMMAS), "<#1>, <#2>, <#3>, …");
        // Exactly the cap: nothing is left out, so no ellipsis.
        assert_eq!(
            all.sort(stored.get(..3).unwrap()).text(COMMAS),
            "<#1>, <#2>, <#3>"
        );
        // The count is of what leaf cannot see, not of what the cap hides.
        let most = Sight::of(["1", "2", "3", "4", "5"]);
        assert_eq!(
            most.sort(&stored).text(COMMAS),
            "<#1>, <#2>, <#3>, … and 1 that leaf can no longer see"
        );
        // Spaced lists name every channel.
        assert_eq!(
            all.sort(&stored).text(Style::Spaces),
            "<#1> <#2> <#3> <#4> <#5> <#6>"
        );
    }

    #[test]
    fn nothing_stored_is_nothing_listed() {
        let listed = Sight::of(["1"]).sort::<String>(&[]);
        assert!(!listed.none_seen());
        assert_eq!(listed.text(COMMAS), "");
    }

    #[test]
    fn without_a_sight_every_stored_channel_is_listed() {
        let stored = ids(&["1", "2"]);
        let unknown = Sight::unknown();
        assert!(unknown.sees("anything"));
        assert_eq!(unknown.ids(), None);
        assert_eq!(unknown.sort(&stored).text(COMMAS), "<#1>, <#2>");
    }

    #[test]
    fn a_channel_picked_in_discord_s_own_menu_is_in_sight() {
        let mut sight = Sight::of(["1"]);
        assert!(!sight.sees("2"));
        sight.admit(["2"]);
        assert!(sight.sees("2"));

        // Nothing to add to when nothing is known: everything is in sight.
        let mut unknown = Sight::unknown();
        unknown.admit(["2"]);
        assert_eq!(unknown, Sight::unknown());
    }

    #[test]
    fn what_is_said_about_a_lost_channel_is_said_one_way() {
        assert_eq!(
            lost("the log channel"),
            "leaf can no longer see the log channel (deleted, or hidden from leaf)"
        );
        assert_eq!(
            unseen_place(false),
            "a channel leaf can no longer see (deleted, or hidden from leaf)"
        );
        assert_eq!(
            unseen_place(true),
            "channels leaf can no longer see (deleted, or hidden from leaf)"
        );
        // Whoever can run the command gets it as written for them; anyone
        // else is pointed at an admin, with nothing to tap.
        assert_eq!(
            choose_new_text(true, "</setup:42>"),
            "Choose new ones with </setup:42>."
        );
        assert_eq!(
            choose_new_text(false, "</setup:42>"),
            "A server admin can choose new ones with `/setup`."
        );
    }

    // -- finding out ---------------------------------------------------------

    const GUILD: serenity::GuildId = serenity::GuildId::new(100);

    /// A gateway cache that has server 100 with `channels` and one active
    /// thread, 300.
    fn cache_with(channels: &[&str]) -> serenity::Cache {
        let named: Vec<(&str, String)> =
            channels.iter().map(|id| (*id, format!("c{id}"))).collect();
        let named: Vec<(&str, &str)> = named
            .iter()
            .map(|(id, name)| (*id, name.as_str()))
            .collect();
        cache_with_named(&named)
    }

    /// [`cache_with`], each channel under the name given.
    fn cache_with_named(channels: &[(&str, &str)]) -> serenity::Cache {
        let channels: Vec<String> = channels
            .iter()
            .map(|(id, name)| {
                format!(r#"{{"id":"{id}","type":0,"name":"{name}","position":0,"flags":0}}"#)
            })
            .collect();
        let created = format!(
            r#"{{"t":"GUILD_CREATE","d":{{
                "id":"100","name":"leaf test","owner_id":"5","unavailable":false,
                "afk_timeout":300,"verification_level":0,"default_message_notifications":0,
                "explicit_content_filter":0,"mfa_level":0,"premium_tier":0,"nsfw_level":0,
                "system_channel_flags":0,"preferred_locale":"en-US","features":[],
                "roles":[],"emojis":[],"stickers":[],"members":[],"voice_states":[],
                "presences":[],"stage_instances":[],"guild_scheduled_events":[],
                "joined_at":"2026-10-03T00:00:00+00:00","large":false,"member_count":2,
                "premium_progress_bar_enabled":false,
                "channels":[{}],
                "threads":[{{"id":"300","type":11,"name":"t","parent_id":"1",
                    "thread_metadata":{{"archived":false,"auto_archive_duration":1440,
                        "archive_timestamp":"2026-10-03T00:00:00+00:00","locked":false}}}}]
            }}}}"#,
            channels.join(",")
        );
        let cache = serenity::Cache::new();
        let serenity::Event::GuildCreate(mut event) = serenity::json::from_str(&created).unwrap()
        else {
            panic!("the fixture is not a guild as the gateway sends it");
        };
        cache.update(&mut event);
        cache
    }

    #[test]
    fn the_gateway_cache_lists_channels_and_active_threads() {
        let cache = cache_with(&["1", "2"]);
        let cached = Cached::of(&cache, GUILD).unwrap();
        assert_eq!(cached.channels, HashSet::from(["1".into(), "2".into()]));
        assert_eq!(cached.threads, HashSet::from(["300".into()]));
        assert_eq!(cached.all().len(), 3);
        // A server the gateway never described.
        assert!(Cached::of(&cache, serenity::GuildId::new(101)).is_none());
    }

    #[tokio::test]
    async fn discord_s_list_decides_what_is_in_sight() {
        // The gateway cache still has channel 2: it missed the delete.
        let cache = cache_with(&["1", "2"]);
        let discord = stub::stub(&[(
            "GET /api/v10/guilds/100/channels ",
            200,
            // One channel as the full decoder would refuse it: only the id
            // is read.
            r#"[{"id":"1","type":0,"name":"daily","position":-1},{"id":"3","type":999}]"#,
        )])
        .await;

        let sight = Sight::look(&stub::http(&discord.base), &cache, GUILD).await;

        assert!(sight.sees("1"));
        assert!(!sight.sees("2"), "a deleted channel the cache kept");
        assert!(sight.sees("3"), "a channel the cache never heard of");
        // Discord's list leaves threads out; the gateway's active ones stay.
        assert!(sight.sees("300"));
        assert_eq!(discord.requests.lock().unwrap().len(), 1);

        let stored = ids(&["1", "2"]);
        assert_eq!(
            sight.sort(&stored).text(COMMAS),
            "<#1> and 1 that leaf can no longer see"
        );
    }

    /// Answers that are not a channel list.
    const NO_LIST: [&[(&str, u16, &str)]; 3] = [
        &[(
            "GET /api/v10/guilds/100/channels ",
            502,
            "<html>Bad Gateway</html>",
        )],
        &[(
            "GET /api/v10/guilds/100/channels ",
            403,
            r#"{"message":"Missing Access","code":50001}"#,
        )],
        &[("GET /api/v10/guilds/100/channels ", 200, r#"{"id":"1"}"#)],
    ];

    #[tokio::test]
    async fn a_channel_the_gateway_marks_as_hidden_is_known_to_exist() {
        // Channel 7 is hidden from leaf: the gateway lists it under the
        // placeholder name, and Discord's list leaves it out. Channel 2 is
        // an ordinary channel the cache kept after it was deleted.
        let cache = cache_with_named(&[("1", "daily"), ("2", "old"), ("7", HIDDEN_CHANNEL_NAME)]);
        let discord =
            stub::stub(&[("GET /api/v10/guilds/100/channels ", 200, r#"[{"id":"1"}]"#)]).await;

        let sight = Sight::look(&stub::http(&discord.base), &cache, GUILD).await;

        assert_eq!(sight, Sight::of(["1", "7", "300"]));
        assert_eq!(
            Cached::of(&cache, GUILD).unwrap().hidden,
            HashSet::from(["7".to_owned()])
        );
    }

    #[tokio::test]
    async fn the_gateway_cache_stands_in_when_discord_does_not_answer() {
        let cache = cache_with(&["1", "2"]);
        for answers in NO_LIST {
            let discord = stub::stub(answers).await;
            let sight = Sight::look(&stub::http(&discord.base), &cache, GUILD).await;
            assert_eq!(sight, Sight::of(["1", "2", "300"]), "{answers:?}");
        }
    }

    #[tokio::test]
    async fn with_no_answer_and_no_cache_nothing_is_known() {
        let discord = stub::stub(&[]).await;
        let empty = serenity::Cache::new();
        let sight = Sight::look(&stub::http(&discord.base), &empty, GUILD).await;
        assert_eq!(sight, Sight::unknown());
        // So what is stored is listed, as before leaf could check.
        assert!(sight.sees("1"));
    }

    #[tokio::test]
    async fn discord_s_list_is_enough_when_the_gateway_has_no_server() {
        let discord =
            stub::stub(&[("GET /api/v10/guilds/100/channels ", 200, r#"[{"id":"1"}]"#)]).await;
        let empty = serenity::Cache::new();
        let sight = Sight::look(&stub::http(&discord.base), &empty, GUILD).await;
        assert_eq!(sight, Sight::of(["1"]));
    }

    #[tokio::test]
    async fn a_list_that_takes_too_long_is_not_waited_for() {
        // A listener that accepts and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let cache = cache_with(&["1"]);
        let http = stub::http(&base);

        let patience = Duration::from_millis(30);
        let looking = Sight::look_within(&http, &cache, GUILD, patience);
        // Far longer than the patience, far shorter than serenity's own.
        let sight = tokio::time::timeout(Duration::from_secs(5), looking)
            .await
            .unwrap_or_else(|_| panic!("the lookup outlived its patience"));

        assert_eq!(sight, Sight::of(["1", "300"]));
        drop(listener);
    }

    #[test]
    fn a_first_answer_waits_less_for_the_list_than_anything_else_does() {
        // Discord allows a first answer three seconds. The list is one step
        // of several before it, so it gets under a third of them.
        assert!(FIRST_ANSWER_LOOK_TIMEOUT < Duration::from_secs(1));
        assert!(FIRST_ANSWER_LOOK_TIMEOUT < LOOK_TIMEOUT);
        // A deferred command is still watched by whoever ran it.
        assert!(LOOK_TIMEOUT < Duration::from_secs(3));
    }

    #[test]
    fn a_gateway_event_serenity_cannot_read_never_reaches_its_cache() {
        // Why Discord's list is asked for and the cache is not taken at its
        // word: this delete is dropped without an error, and the channel
        // stays cached.
        let cache = cache_with(&["1", "2"]);
        let delete = r#"{"t":"CHANNEL_DELETE","d":
            {"id":"2","type":0,"guild_id":"100","name":"c2","position":-1}}"#;
        let event: serenity::Event = serenity::json::from_str(delete).unwrap();
        assert!(
            matches!(event, serenity::Event::Unknown(_)),
            "serenity now reads this event: {event:?}"
        );
        assert!(Cached::of(&cache, GUILD).unwrap().channels.contains("2"));

        // One it can read is applied.
        let delete = r#"{"t":"CHANNEL_DELETE","d":
            {"id":"2","type":0,"guild_id":"100","name":"c2","position":0}}"#;
        let serenity::Event::ChannelDelete(mut event) = serenity::json::from_str(delete).unwrap()
        else {
            panic!("not a channel delete as the gateway sends it");
        };
        cache.update(&mut event);
        assert!(!Cached::of(&cache, GUILD).unwrap().channels.contains("2"));
    }
}
