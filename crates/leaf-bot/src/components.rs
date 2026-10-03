//! What the bot's commands share about message components.
//!
//! That is: the ids, the router for buttons that outlive their command, the
//! guard that keeps two writers off the same archive entry, and the quiet
//! log-channel line.
//!
//! Component state is a hybrid. A button that must work long after the
//! command that sent it (an Open gallery button on a public card, "Turn off
//! reminders" on a DM) has a stateless `leaf:` id and is answered here, by
//! [`route`], from the gateway event. Everything else belongs to a collector
//! that lives as long as its command's interaction; those ids embed
//! [`NONCE`], which changes with every process start, so the router can tell
//! a prompt of this process (the collector answers it) from one left over
//! from before a restart (answered here as expired). A collector also gives
//! up after a while; [`SESSIONS`] knows which are still listening, so a
//! press or a form submit that arrives after that is answered as expired
//! too, instead of not at all.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use leaf_core::db::LaunchIntentRepo;
use leaf_core::domain::{GuildSettings, Series};
use leaf_core::policy::{self, Viewer};
use leaf_core::series_ops;
use poise::serenity_prelude as serenity;

use crate::{Data, Error, checks, reminders};

// ---------------------------------------------------------------------------
// Ids
// ---------------------------------------------------------------------------

/// Changes with every process start, so a prompt from before a restart is
/// never mistaken for one of this process's.
pub(crate) static NONCE: LazyLock<String> = LazyLock::new(|| {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{:x}", nanos & 0xffff_ffff)
});

/// First id segments of the collector-scoped components: the archive flow's
/// and the query, setup and import flows'.
const SCOPED_PREFIXES: [&str; 2] = ["arc", "qry"];

/// Id of the Open gallery button with no destination.
const OPEN_ID: &str = "leaf:open";

/// Shown when the Activity cannot be launched from a button.
pub(crate) const OPEN_GALLERY_FALLBACK: &str = "🍂 I couldn't open the gallery from here. On \
     mobile, tap + next to the message box, then Apps, then leaf. On desktop, open the app \
     launcher in the message box and choose leaf.";

/// Shown on a press of a prompt nothing listens for any more: an earlier
/// run of leaf sent it, or its command stopped waiting.
const PROMPT_EXPIRED: &str = "⏳ This prompt has expired. Run the command again.";

/// Shown on a submit of a form nothing listens for any more: an earlier run
/// of leaf opened it, or it was left open until its command stopped
/// waiting. What was typed is not kept, so it says so.
const FORM_EXPIRED: &str = "⏳ This form has expired, so what you entered wasn't saved. Run the \
     command again.";

/// The stateless id of an Open gallery button.
///
/// `leaf:open`, `leaf:open:<series>` or `leaf:open:<series>:<day>`.
/// [`route`] answers it: it records the destination as a launch intent for
/// the presser and opens the Activity.
pub(crate) fn open_gallery_id(target: Option<(i64, Option<i64>)>) -> String {
    match target {
        None => OPEN_ID.to_owned(),
        Some((series, None)) => format!("{OPEN_ID}:{series}"),
        Some((series, Some(day))) => format!("{OPEN_ID}:{series}:{day}"),
    }
}

/// A press on an Open gallery button: where it asks the gallery to open
/// (series, and day within it), or nowhere in particular.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenPress(Option<(i64, Option<i64>)>);

/// Reads an Open gallery id back; `None` when the id is not one.
fn parse_open_id(custom_id: &str) -> Option<OpenPress> {
    let rest = custom_id.strip_prefix(OPEN_ID)?;
    if rest.is_empty() {
        return Some(OpenPress(None));
    }
    let mut parts = rest.strip_prefix(':')?.split(':');
    let series = parts.next()?.parse::<i64>().ok().filter(|id| *id > 0)?;
    let day = match parts.next() {
        None => None,
        Some(day) => Some(day.parse::<i64>().ok().filter(|d| *d > 0)?),
    };
    parts
        .next()
        .is_none()
        .then_some(OpenPress(Some((series, day))))
}

/// Whether `custom_id` is a collector-scoped id from an earlier process:
/// nothing in this one is listening for it.
fn is_stale_scoped_id(custom_id: &str, live_nonce: &str) -> bool {
    let mut parts = custom_id.split(':');
    let (Some(prefix), Some(nonce)) = (parts.next(), parts.next()) else {
        return false;
    };
    SCOPED_PREFIXES.contains(&prefix) && nonce != live_nonce
}

/// What every id of one collector session starts with: the first four
/// segments, `<prefix>:<nonce>:<kind>:<session>` (the session is the id of
/// the interaction that started it). `None` for an id with fewer segments.
fn session_key(custom_id: &str) -> Option<&str> {
    let mut segments = 0_usize;
    for (at, _) in custom_id.match_indices(':') {
        segments += 1;
        if segments == SESSION_KEY_SEGMENTS {
            return custom_id.get(..at).filter(|key| !key.ends_with(':'));
        }
    }
    (segments + 1 == SESSION_KEY_SEGMENTS && !custom_id.ends_with(':')).then_some(custom_id)
}

/// How many `:`-separated segments make up a session key.
const SESSION_KEY_SEGMENTS: usize = 4;

/// Whether nothing in this process will answer `custom_id`: it is a
/// collector-scoped id, and either an earlier process made it or its
/// session is not listening (at `now`) any more.
fn nobody_listens(custom_id: &str, live_nonce: &str, sessions: &Sessions, now: Instant) -> bool {
    if is_stale_scoped_id(custom_id, live_nonce) {
        return true;
    }
    let mut parts = custom_id.split(':');
    let scoped = parts
        .next()
        .is_some_and(|prefix| SCOPED_PREFIXES.contains(&prefix))
        && parts.next() == Some(live_nonce);
    // An id of this process too short to name its session is left alone.
    scoped && session_key(custom_id).is_some_and(|key| !sessions.is_listening(key, now))
}

// ---------------------------------------------------------------------------
// Listening sessions
// ---------------------------------------------------------------------------

/// How long after a session stops listening its ids still count as its
/// own. A second press that lands while the session is finishing was never
/// answered; it stays that way rather than being called expired while the
/// first press is still being carried out.
const SESSION_GRACE: Duration = Duration::from_secs(5);

/// Where one session stands.
#[derive(Debug, Clone, Copy)]
enum Listening {
    /// This many collectors are listening for its ids.
    Live(usize),
    /// The last collector stopped at this time.
    Ended(Instant),
}

/// The collector sessions of this process that are listening right now.
///
/// A collector stops after its timeout, but what it showed can stay on
/// screen: a form left open on a phone, a prompt whose controls could not
/// be removed. The router asks here whether anyone will answer an id, and
/// answers it as expired when nobody will.
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    known: Mutex<HashMap<String, Listening>>,
}

impl Sessions {
    /// Marks the session `custom_id` belongs to as listening until the
    /// returned guard is dropped. `custom_id` is any id of the session, or
    /// the prefix its ids share. To be called before the session's
    /// components are shown, like starting its collector.
    pub(crate) fn listen(&self, custom_id: &str) -> SessionGuard<'_> {
        self.listen_at(custom_id, Instant::now())
    }

    fn listen_at(&self, custom_id: &str, now: Instant) -> SessionGuard<'_> {
        let key = session_key(custom_id).map(str::to_owned);
        if let Some(key) = &key {
            let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
            // Sessions long over would otherwise pile up for the life of
            // the process.
            known.retain(|_, state| match state {
                Listening::Live(_) => true,
                Listening::Ended(at) => now.saturating_duration_since(*at) < SESSION_GRACE,
            });
            let listeners = match known.get(key) {
                Some(Listening::Live(n)) => n.saturating_add(1),
                _ => 1,
            };
            known.insert(key.clone(), Listening::Live(listeners));
        }
        SessionGuard { owner: self, key }
    }

    /// Whether a collector will answer ids of the session `key` at `now`.
    fn is_listening(&self, key: &str, now: Instant) -> bool {
        let known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        match known.get(key) {
            Some(Listening::Live(_)) => true,
            Some(Listening::Ended(at)) => now.saturating_duration_since(*at) < SESSION_GRACE,
            None => false,
        }
    }

    fn stop(&self, key: &str, now: Instant) {
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let state = match known.get(key) {
            Some(Listening::Live(n)) if *n > 1 => Listening::Live(n.saturating_sub(1)),
            Some(_) => Listening::Ended(now),
            None => return,
        };
        known.insert(key.to_owned(), state);
    }
}

/// Marks its session as no longer listening when dropped, however the
/// command ends.
#[derive(Debug)]
pub(crate) struct SessionGuard<'a> {
    owner: &'a Sessions,
    key: Option<String>,
}

impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        if let Some(key) = &self.key {
            self.owner.stop(key, Instant::now());
        }
    }
}

/// Every collector session of this process.
pub(crate) static SESSIONS: LazyLock<Sessions> = LazyLock::new(Sessions::default);

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Answers a component press that no collector owns. Presses on prompts a
/// collector is listening for are left alone: it answers them, and a second
/// answer would fail one of the two.
pub(crate) async fn route(
    ctx: &serenity::Context,
    press: &serenity::ComponentInteraction,
    data: &Data,
) -> Result<(), Error> {
    // The reminder's own button: works from a DM, so no guild is required.
    if reminders::answer_off_press(&ctx.http, press, &data.series).await {
        return Ok(());
    }
    let id = press.data.custom_id.as_str();
    if let Some(OpenPress(target)) = parse_open_id(id) {
        return open_gallery(ctx, press, data, target).await;
    }
    if nobody_listens(id, &NONCE, &SESSIONS, Instant::now()) {
        let reply = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .content(PROMPT_EXPIRED)
                .ephemeral(true),
        );
        if let Err(e) = press.create_response(&ctx.http, reply).await {
            tracing::warn!(error = %e, "could not answer a press on an expired prompt");
        }
    }
    Ok(())
}

/// Answers a modal submit that no collector owns: one whose form an earlier
/// run of leaf opened, or one left open until its command stopped waiting.
/// Left unanswered, Discord shows its own "Something went wrong" inside the
/// form, with no hint to start again. A submit for a form whose session is
/// still listening is left to its collector.
pub(crate) async fn route_modal(ctx: &serenity::Context, submit: &serenity::ModalInteraction) {
    if !nobody_listens(&submit.data.custom_id, &NONCE, &SESSIONS, Instant::now()) {
        return;
    }
    let reply = serenity::CreateInteractionResponse::Message(
        serenity::CreateInteractionResponseMessage::new()
            .content(FORM_EXPIRED)
            .ephemeral(true),
    );
    if let Err(e) = submit.create_response(&ctx.http, reply).await {
        tracing::warn!(error = %e, "could not answer a submit of an expired form");
    }
}

/// Opens the gallery for whoever pressed an Open gallery button.
///
/// Discord's launch response carries no destination, so the destination is
/// left as a short-lived intent the Activity collects when it starts. The
/// button may sit on a public card that outlived the series being public,
/// so the intent is only written when the presser may view the series now;
/// otherwise the gallery opens on its usual first screen.
async fn open_gallery(
    ctx: &serenity::Context,
    press: &serenity::ComponentInteraction,
    data: &Data,
    target: Option<(i64, Option<i64>)>,
) -> Result<(), Error> {
    let Some(guild) = press.guild_id else {
        let reply = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .content("🍂 The gallery opens from inside a server. Press this in the server.")
                .ephemeral(true),
        );
        press.create_response(&ctx.http, reply).await?;
        return Ok(());
    };
    if let Some((series_id, day)) = target
        && let Some(hidden) = leave_intent(press, data, guild, series_id, day).await
    {
        // An admin pressed it for a series the gallery will not show them:
        // say so, rather than open the gallery somewhere else.
        let text = not_in_gallery_text(&hidden, data.public_url.as_deref());
        let reply = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .content(text)
                .ephemeral(true),
        );
        press.create_response(&ctx.http, reply).await?;
        return Ok(());
    }
    launch_or_explain(&ctx.http, press).await
}

/// Whether the gallery lists `series` for this member.
///
/// The gallery treats everyone as a member: Manage Server opens the admin
/// panel and the chat commands, not other people's private series. So this
/// is the visibility rule with no admin override, the same question the
/// gallery's own API asks before it follows a launch intent.
pub(crate) fn gallery_shows(series: &Series, user_id: &str, role_ids: &[String]) -> bool {
    policy::can_view(
        series,
        &Viewer {
            user_id,
            role_ids,
            is_admin: false,
        },
    )
}

/// A member's role ids, as the visibility rules compare them.
pub(crate) fn role_ids(member: Option<&serenity::Member>) -> Vec<String> {
    member.map_or_else(Vec::new, |m| {
        m.roles.iter().map(ToString::to_string).collect()
    })
}

/// What an admin is told when they ask the gallery for a series it does
/// not show them. `public_url` is leaf's address, for the admin panel.
pub(crate) fn not_in_gallery_text(series: &Series, public_url: Option<&str>) -> String {
    let name = series_ops::display_name(&series.name);
    let panel = public_url.map_or_else(
        || "the admin panel".to_owned(),
        |url| format!("the admin panel ({}/admin)", url.trim_end_matches('/')),
    );
    format!(
        "🍂 The gallery can't open **{name}** for you. It shows each person only the series \
         they can view as a member, and Manage Server doesn't change that. You can manage \
         this series in {panel}, or look it up here with `/search`."
    )
}

/// Records where the gallery should open for the presser, when the gallery
/// will show them that series. Failures only cost the destination.
///
/// Returns the series when it is one the presser can reach as an admin but
/// the gallery will not show them: the caller explains instead of opening
/// the gallery on its usual first screen. Anyone else who may not view it
/// (the button sat on a card that outlived the series being public) just
/// gets the gallery.
async fn leave_intent(
    press: &serenity::ComponentInteraction,
    data: &Data,
    guild: serenity::GuildId,
    series_id: i64,
    day: Option<i64>,
) -> Option<Series> {
    let guild_id = guild.to_string();
    let user_id = press.user.id.to_string();
    let series = match data.series.get(series_id).await {
        Ok(Some(series)) if series.guild_id == guild_id => series,
        Ok(_) => return None,
        Err(e) => {
            tracing::warn!(series = series_id, error = %e, "open gallery: series lookup failed");
            return None;
        }
    };
    if !gallery_shows(&series, &user_id, &role_ids(press.member.as_ref())) {
        let is_admin = press
            .member
            .as_ref()
            .and_then(|m| m.permissions)
            .is_some_and(serenity::Permissions::manage_guild);
        return is_admin.then_some(series);
    }
    let stored = LaunchIntentRepo::new(data.pool.clone())
        .put(&user_id, &guild_id, series_id, day, checks::now_unix())
        .await;
    if let Err(e) = stored {
        tracing::warn!(series = series_id, error = %e, "could not store the launch intent");
    }
    None
}

/// Answers `press` by launching the Activity, or, when Discord refuses
/// that, by saying where the gallery is.
pub(crate) async fn launch_or_explain(
    http: &serenity::Http,
    press: &serenity::ComponentInteraction,
) -> Result<(), Error> {
    let launch = serenity::CreateInteractionResponse::LaunchActivity;
    if let Err(e) = press.create_response(http, launch).await {
        tracing::warn!(error = %e, "could not launch the Activity");
        let fallback = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .content(OPEN_GALLERY_FALLBACK)
                .ephemeral(true),
        );
        press.create_response(http, fallback).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// In-flight guard
// ---------------------------------------------------------------------------

/// One thing a writer holds for as long as it runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum FlightKey {
    /// The source message within a series (`series_id`, `message_id`): one
    /// attempt at a time, so a double submit cannot archive it twice or
    /// renumber an entry that is still being written. Per series, because
    /// only the creator archives into a series, so the attempt in the way is
    /// always the same person's.
    Message(i64, String),
    /// A day of a series (`series_id`, `day`): two posts aimed at one day
    /// would race for it, and the loser's cleanup must not run beside the
    /// winner's upload.
    Day(i64, i64),
}

/// The archive writes running right now.
///
/// Without it, a second submit of the same message during a slow upload
/// writes the same objects, loses the insert, and its cleanup removes what
/// the first attempt just stored. Deleting and importing take the same keys,
/// so they cannot run beside an archive of the same entry either.
#[derive(Debug, Default)]
pub(crate) struct Inflight {
    held: Mutex<HashSet<FlightKey>>,
}

impl Inflight {
    /// Takes every key in `keys`, or none of them. `Err` names the first
    /// key another attempt holds.
    pub(crate) fn try_begin(&self, keys: &[FlightKey]) -> Result<FlightGuard<'_>, FlightKey> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(taken) = keys.iter().find(|key| held.contains(*key)) {
            return Err(taken.clone());
        }
        held.extend(keys.iter().cloned());
        drop(held);
        Ok(FlightGuard {
            owner: self,
            keys: keys.to_vec(),
        })
    }

    /// Whether an attempt holds `key` right now.
    pub(crate) fn is_held(&self, key: &FlightKey) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(key)
    }
}

/// Releases its keys when dropped, however the attempt ends.
#[derive(Debug)]
pub(crate) struct FlightGuard<'a> {
    owner: &'a Inflight,
    keys: Vec<FlightKey>,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        let mut held = self
            .owner
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for key in &self.keys {
            held.remove(key);
        }
    }
}

/// The guard every archive write in this process goes through.
pub(crate) static INFLIGHT: LazyLock<Inflight> = LazyLock::new(Inflight::default);

/// What someone is told when the entry they want to change is being written.
pub(crate) const BUSY: &str =
    "⏳ leaf is still archiving that post. Give it a moment, then try again.";

// ---------------------------------------------------------------------------
// The log channel
// ---------------------------------------------------------------------------

/// Which guild each channel leaf has logged to belongs to. A channel never
/// changes guild, so one lookup per channel lasts the process.
static CHANNEL_GUILDS: LazyLock<Mutex<HashMap<u64, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Why a log line was not written.
#[derive(Debug)]
enum LogProblem {
    /// The stored id is not a snowflake.
    NotAnId,
    /// The channel belongs to another server (or is a DM).
    Elsewhere,
    /// Discord would not show the channel or take the message.
    Refused(serenity::Error),
}

/// The guild a channel belongs to, asked of Discord once per channel.
async fn channel_guild(
    http: &serenity::Http,
    channel: serenity::ChannelId,
) -> Result<Option<u64>, serenity::Error> {
    let known = CHANNEL_GUILDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&channel.get())
        .copied();
    if known.is_some() {
        return Ok(known);
    }
    let guild = match http.get_channel(channel).await? {
        serenity::Channel::Guild(c) => Some(c.guild_id.get()),
        _ => None,
    };
    if let Some(guild) = guild {
        CHANNEL_GUILDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(channel.get(), guild);
    }
    Ok(guild)
}

async fn try_log(
    http: &serenity::Http,
    settings: &GuildSettings,
    raw: &str,
    line: &str,
) -> Result<serenity::ChannelId, (Option<serenity::ChannelId>, LogProblem)> {
    let channel = raw
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .map(serenity::ChannelId::new)
        .ok_or((None, LogProblem::NotAnId))?;
    // The id comes from settings, which the admin panel also writes: never
    // post one server's activity into another server's channel.
    let owner = channel_guild(http, channel)
        .await
        .map_err(|e| (Some(channel), LogProblem::Refused(e)))?;
    if owner.map(|g| g.to_string()).as_deref() != Some(settings.guild_id.as_str()) {
        return Err((Some(channel), LogProblem::Elsewhere));
    }
    let message = serenity::CreateMessage::new()
        .content(line)
        .allowed_mentions(serenity::CreateAllowedMentions::new())
        .flags(serenity::MessageFlags::SUPPRESS_NOTIFICATIONS);
    channel
        .send_message(http, message)
        .await
        .map(|_| channel)
        .map_err(|e| (Some(channel), LogProblem::Refused(e)))
}

/// Writes `line` to the server's log channel, if one is set.
///
/// The line notifies nobody: mentions show as names but never ping, and the
/// message arrives silently. Returns a note for someone who can fix it (an
/// admin) when the line could not be written; the caller decides whether
/// the person in front of it is one.
pub(crate) async fn log_line(
    http: &serenity::Http,
    settings: &GuildSettings,
    line: &str,
) -> Option<String> {
    let raw = settings.log_channel_id.as_deref()?;
    match try_log(http, settings, raw, line).await {
        Ok(_) => None,
        Err((_, LogProblem::NotAnId)) => {
            tracing::warn!(channel = raw, "log channel id is not a snowflake");
            None
        }
        Err((_, LogProblem::Elsewhere)) => {
            tracing::warn!(
                guild = %settings.guild_id,
                channel = raw,
                "log line skipped: the log channel is not in this server"
            );
            Some(
                "The log channel isn't a channel of this server, so I didn't write to it. \
                 Pick a log channel again with `/setup`."
                    .to_owned(),
            )
        }
        Err((channel, LogProblem::Refused(e))) => {
            tracing::warn!(channel = raw, error = %e, "log-channel write failed");
            channel.map(|channel| {
                format!(
                    "I couldn't write to the log channel <#{channel}>. Check that I can view it \
                     and send messages there."
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    #[test]
    fn open_ids_round_trip() {
        for target in [None, Some((7, None)), Some((7, Some(42)))] {
            assert_eq!(
                parse_open_id(&open_gallery_id(target)),
                Some(OpenPress(target))
            );
        }
        assert_eq!(open_gallery_id(Some((7, Some(42)))), "leaf:open:7:42");
    }

    #[test]
    fn other_ids_are_not_open_ids() {
        for id in [
            "leaf:rem-off:3",
            "leaf:opener",
            "leaf:open:",
            "leaf:open:x",
            "leaf:open:0",
            "leaf:open:3:",
            "leaf:open:3:0",
            "leaf:open:3:4:5",
            "arc:abc:archive:1:2:open",
            "",
        ] {
            assert_eq!(parse_open_id(id), None, "{id}");
        }
    }

    #[test]
    fn only_another_process_s_scoped_ids_are_stale() {
        // A prompt of this process: its collector answers.
        assert!(!is_stale_scoped_id("arc:abc:archive:1:2:day", "abc"));
        assert!(!is_stale_scoped_id("qry:abc:pick:1", "abc"));
        // The same ids after a restart.
        assert!(is_stale_scoped_id("arc:abc:archive:1:2:day", "def"));
        assert!(is_stale_scoped_id("qry:abc:setup:9:save", "def"));
        // Modal ids are scoped the same way, so a submit after a restart is
        // recognised too.
        assert!(is_stale_scoped_id("arc:abc:archive:1:m:2", "def"));
        assert!(is_stale_scoped_id("qry:abc:setup:9:m:2", "def"));
        assert!(!is_stale_scoped_id("arc:abc:archive:1:m:2", "abc"));
        // Not scoped ids at all: never answered as expired.
        for id in [
            "leaf:open",
            "leaf:rem-off:3",
            "confirm-yes",
            "arc",
            "",
            "x:y:z",
        ] {
            assert!(!is_stale_scoped_id(id, "def"), "{id}");
        }
    }

    #[test]
    fn a_session_key_is_the_first_four_segments() {
        for (id, key) in [
            ("arc:abc:archive:1:2:day", Some("arc:abc:archive:1")),
            ("arc:abc:archive:1:m:99", Some("arc:abc:archive:1")),
            ("arc:abc:archive:1:", Some("arc:abc:archive:1")),
            ("qry:abc:pick:1", Some("qry:abc:pick:1")),
            ("qry:abc:setup:9:save", Some("qry:abc:setup:9")),
            ("qry:abc:del:9:yes", Some("qry:abc:del:9")),
            // Too short to name a session, or naming an empty one.
            ("qry:abc:pick", None),
            ("qry:abc:pick:", None),
            ("qry:abc:pick::yes", None),
            ("arc", None),
            ("", None),
        ] {
            assert_eq!(session_key(id), key, "{id}");
        }
    }

    #[test]
    fn an_id_is_answered_as_expired_once_its_session_stops_listening() {
        let sessions = Sessions::default();
        let start = Instant::now();
        let button = "arc:abc:archive:1:2:day";
        let form = "arc:abc:archive:1:m:77";
        let unanswered = |id: &str, now: Instant| nobody_listens(id, "abc", &sessions, now);

        // Never started in this process (or long over): nobody answers.
        assert!(unanswered(button, start));

        // While the session listens, its buttons and forms are its own;
        // another session's are not.
        let guard = sessions.listen_at("arc:abc:archive:1:", start);
        assert!(!unanswered(button, start));
        assert!(!unanswered(form, start));
        assert!(unanswered("arc:abc:archive:2:m:77", start));
        assert!(unanswered("qry:abc:archive:1:x", start));

        // It stops listening: its ids stay its own for the grace period
        // (a press that lands while it finishes), then nobody answers them.
        drop(guard);
        assert!(!unanswered(form, Instant::now()));
        let later = Instant::now() + SESSION_GRACE;
        assert!(unanswered(form, later));
        assert!(unanswered(button, later));

        // An id from before a restart is expired whatever is listening,
        // and ids that are not collector-scoped are never answered here.
        let _again = sessions.listen_at(button, later);
        assert!(!unanswered(button, later));
        assert!(nobody_listens(button, "def", &sessions, later));
        for id in [
            "leaf:open",
            "leaf:rem-off:3",
            "confirm-yes",
            "x:abc:y:1",
            "",
        ] {
            assert!(!unanswered(id, later), "{id}");
        }
        // An id of this process too short to name a session is left alone.
        assert!(!unanswered("qry:abc:pick", later));
    }

    #[test]
    fn a_session_listens_until_its_last_collector_stops() {
        let sessions = Sessions::default();
        let now = Instant::now();
        let id = "qry:abc:imp:5:yes";
        let first = sessions.listen_at(id, now);
        let second = sessions.listen_at("qry:abc:imp:5:undo", now);
        drop(first);
        let later = Instant::now() + SESSION_GRACE;
        assert!(!nobody_listens(id, "abc", &sessions, later));
        drop(second);
        let later = Instant::now() + SESSION_GRACE;
        assert!(nobody_listens(id, "abc", &sessions, later));

        // Starting a session forgets the ones long over.
        let _other = sessions.listen_at("qry:abc:imp:6:yes", later);
        assert_eq!(sessions.known.lock().unwrap().len(), 1);
    }

    fn a_series(
        privacy: leaf_core::domain::Privacy,
        state: leaf_core::domain::SeriesState,
    ) -> Series {
        Series {
            id: 1,
            guild_id: "g".to_owned(),
            creator_id: "creator".to_owned(),
            name: "a**b".to_owned(),
            description: String::new(),
            channels: vec![],
            cadence: leaf_core::domain::Cadence::Daily,
            detection_mode: leaf_core::domain::DetectionMode::ContextMenu,
            privacy,
            privacy_role_id: Some("7".to_owned()),
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".to_owned(),
            state,
            created_at: 0,
        }
    }

    #[test]
    fn the_gallery_shows_what_a_member_may_view_and_no_more() {
        use leaf_core::domain::{Privacy, SeriesState};
        let none: [String; 0] = [];
        let vip = ["7".to_owned()];

        let public = a_series(Privacy::Public, SeriesState::Active);
        assert!(gallery_shows(&public, "someone", &none));

        // The same answers the gallery's API gives: no admin override, so
        // an intent is never left that the gallery would drop.
        let sprout = a_series(Privacy::Public, SeriesState::Sprout);
        assert!(!gallery_shows(&sprout, "an-admin", &none));
        assert!(gallery_shows(&sprout, "creator", &none));

        let private = a_series(Privacy::CreatorOnly, SeriesState::Active);
        assert!(!gallery_shows(&private, "an-admin", &none));

        let gated = a_series(Privacy::RoleGated, SeriesState::Active);
        assert!(!gallery_shows(&gated, "an-admin", &none));
        assert!(gallery_shows(&gated, "member", &vip));

        let revoked = a_series(Privacy::Public, SeriesState::Revoked);
        assert!(!gallery_shows(&revoked, "an-admin", &none));
        assert!(!gallery_shows(&revoked, "creator", &none));
    }

    #[test]
    fn an_admin_is_told_why_the_gallery_will_not_open_a_series() {
        use leaf_core::domain::{Privacy, SeriesState};
        let series = a_series(Privacy::CreatorOnly, SeriesState::Active);
        let text = not_in_gallery_text(&series, Some("https://leaf.example/"));
        // The name shows as typed, and the way on is named.
        assert!(text.contains(r"**a\*\*b**"), "{text}");
        assert!(text.contains("https://leaf.example/admin"), "{text}");
        assert!(text.contains("`/search`"), "{text}");
        let bare = not_in_gallery_text(&series, None);
        assert!(bare.contains("in the admin panel,"), "{bare}");
    }

    #[test]
    fn the_guard_takes_all_keys_or_none() {
        let inflight = Inflight::default();
        let a = FlightKey::Message(1, "m".to_owned());
        let b = FlightKey::Day(1, 5);
        let guard = inflight.try_begin(std::slice::from_ref(&b)).unwrap();
        // `a` is free, but `b` is held: neither is taken.
        assert_eq!(inflight.try_begin(&[a.clone(), b.clone()]).unwrap_err(), b);
        assert!(!inflight.is_held(&a));
        drop(guard);
        assert!(!inflight.is_held(&b));
        let _both = inflight.try_begin(&[a.clone(), b]).unwrap();
        assert!(inflight.is_held(&a));
    }
}
