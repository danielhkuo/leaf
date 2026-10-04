//! The reminder scheduler: a coarse one-minute tick that asks the pure
//! `reminder_due` predicate about every reminder-enabled series and nudges
//! the ones that are behind.
//!
//! There is no cron table and no per-slot bookkeeping. "Due" is a state
//! predicate (behind schedule, inside the window that opens at the reminder
//! time, not yet reminded for this missing day), so a restart inside the
//! window still sends the nudge.
//!
//! One reminder goes out per missing day. The day is marked before the
//! send, so a crash cannot double-send, and what happens to the mark
//! afterwards depends on how the send went (see [`Outcome`]):
//!
//! - delivered: the mark stays and any recorded failure is cleared;
//! - refused for a reason that will repeat (DMs closed, channel gone, no
//!   permission): the mark stays, so there is one attempt per missing day
//!   rather than one per minute, and the reason is stored on the series for
//!   the gallery's settings screen to show;
//! - refused for any other reason that will repeat (an archived thread, an
//!   `AutoMod` rule: Discord's other 4xx answers): the mark stays too, so it
//!   is also one attempt per missing day, but leaf has no advice to give,
//!   so only the log says why;
//! - nobody to remind (leaf was removed from the server, or the creator
//!   left it): the mark stays and nothing is recorded;
//! - anything that may pass (network, timeout, rate limit, a Discord 5xx,
//!   a token Discord does not accept): the mark is rolled back and the send
//!   is tried again after a doubling wait, at most [`MAX_ATTEMPTS`] times
//!   per due window. (A send whose answer was lost may have arrived, so
//!   this is the one path that can repeat a reminder.)
//!
//! A DM that cannot be delivered never falls back to a channel ping: that
//! would name the creator in public, which they did not choose.
//!
//! A reminder says how to archive, and those steps name leaf's app as
//! Discord's menus list it. The gateway says what that is when it connects,
//! which is after the scheduler starts, so the first pass waits a moment for
//! it ([`APP_NAME_PATIENCE`]).
//!
//! A DM names the series' channel and links to it only while leaf can see
//! the channel (see [`crate::channels`]). Once it cannot, the DM says so
//! instead and points at Series settings, where the creator picks another.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::error::Error as _;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use leaf_core::db::{DbResult, SeriesRepo};
use leaf_core::domain::{Privacy, ReminderFailureKind, SeriesState};
use leaf_core::reminder::{DUE_WINDOW_SECS, ReminderCandidate, reminder_due};
use leaf_core::series_ops::display_name;
use poise::serenity_prelude as serenity;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::channels::{self, Sight};
use crate::checks::now_unix;
use crate::menus;

/// How often the scheduler wakes. One minute is fine: reminder times have
/// minute resolution and the predicate is idempotent across ticks.
const TICK: Duration = Duration::from_mins(1);

/// Longest one reminder may take, lookups included. serenity sets no
/// timeout of its own, and one request that hangs must not hold up every
/// series behind it. The reply to a button press gets the same limit.
const SEND_TIMEOUT: Duration = Duration::from_secs(20);

/// Sends one reminder gets inside one due window when Discord keeps failing
/// in a way that may pass. With the doubling wait they fall at 0, 1, 3, 7,
/// 15 and 31 minutes after the first failure.
const MAX_ATTEMPTS: u32 = 6;

/// Wait after the first failed send; doubles after each further one.
const RETRY_BASE_SECS: i64 = 60;

/// Prefix of the "Turn off reminders" button id; the series id follows.
/// Stateless, so the button still works after a restart.
pub const REM_OFF_PREFIX: &str = "leaf:rem-off:";

/// How long the first pass waits for the gateway to say what leaf's app is
/// called. Connecting takes a second or two; past this the gateway is
/// unreachable, and a reminder that is due goes out describing the app
/// instead of naming it.
const APP_NAME_PATIENCE: Duration = Duration::from_secs(15);

/// Where the rest of the reminder settings live. A DM has no server, so the
/// gallery cannot be opened from it.
const SETTINGS_HINT: &str =
    "open leaf's gallery in the server, open the series and tap the gear (Series settings)";

// Discord JSON error codes that say who is missing.
/// The server no longer exists.
const UNKNOWN_GUILD: isize = 10_004;
/// leaf is not in the server.
const MISSING_ACCESS: isize = 50_001;
/// The user is not in the server.
const UNKNOWN_MEMBER: isize = 10_007;
/// The account no longer exists.
const UNKNOWN_USER: isize = 10_013;

// The 4xx statuses that are not a "no" to the request itself: the same
// request can be accepted later.
/// leaf's token was not accepted. Nothing gets through until the operator
/// fixes it, and then everything does.
const UNAUTHORIZED: u16 = 401;
/// Discord gave up waiting for the request.
const REQUEST_TIMEOUT: u16 = 408;
/// Rate limited.
const TOO_MANY_REQUESTS: u16 = 429;

/// The code serenity reports when an error answer was not Discord's JSON:
/// an error page from something in front of Discord.
const NOT_DISCORDS_ANSWER: isize = -1;

/// Keeps reminder scheduling to one task per process. A scheduler is
/// started with every gateway connection attempt, and two ticking side by
/// side would both find the same series due in the same minute, and both
/// would send.
struct Gate {
    /// Held by the scheduler that is ticking.
    active: tokio::sync::Mutex<()>,
    /// Held by the one scheduler waiting to take over when that one ends.
    next: tokio::sync::Mutex<()>,
}

impl Gate {
    const fn new() -> Self {
        Self {
            active: tokio::sync::Mutex::const_new(()),
            next: tokio::sync::Mutex::const_new(()),
        }
    }
}

static GATE: Gate = Gate::new();

/// Runs the scheduler until `shutdown` flips. The first tick fires on
/// startup, so a reminder whose window is still open after a restart goes
/// out then.
///
/// Safe to call once per gateway connection attempt: while a scheduler is
/// ticking, one more call waits to take over when it ends and any further
/// call returns at once.
///
/// `app_name` is the name Discord's menus list leaf's app under, empty until
/// the gateway has said it. The reminders name it in their steps, so the
/// first tick waits for it, [`APP_NAME_PATIENCE`] at most.
///
/// `cache` is the gateway's: what stands in for Discord's channel list when
/// that cannot be had (see [`Sight::look`]).
pub async fn run(
    http: Arc<serenity::Http>,
    cache: Arc<serenity::Cache>,
    series: SeriesRepo,
    app_name: watch::Receiver<String>,
    shutdown: watch::Receiver<bool>,
) {
    drive(
        &GATE,
        Live { http, cache },
        series,
        app_name,
        APP_NAME_PATIENCE,
        shutdown,
        now_unix,
    )
    .await;
}

/// Waits until the app's name is known, for `patience` at most. False when
/// it still is not: the time ran out, or the gateway connection that would
/// have said it is gone.
async fn app_name_known(app_name: &mut watch::Receiver<String>, patience: Duration) -> bool {
    let said = app_name.wait_for(|name| !name.trim().is_empty());
    tokio::time::timeout(patience, said)
        .await
        .is_ok_and(|waited| waited.is_ok())
}

/// [`run`], with the gate, the Discord calls, the clock and how long the
/// first pass waits for the app's name (`patience`) passed in.
async fn drive<C: Courier>(
    gate: &Gate,
    courier: C,
    series: SeriesRepo,
    mut app_name: watch::Receiver<String>,
    patience: Duration,
    mut shutdown: watch::Receiver<bool>,
    clock: fn() -> i64,
) {
    // One successor is enough, and it is needed: a caller may end the
    // running scheduler and start another before the first has let go.
    // Waiting without a limit would park one task per gateway retry, each
    // holding an HTTP client, for as long as the first scheduler runs.
    let Ok(queued) = gate.next.try_lock() else {
        debug!("reminder scheduler not started: one is running and one is waiting to take over");
        return;
    };
    let _active = tokio::select! {
        biased;
        guard = gate.active.lock() => guard,
        _ = shutdown.changed() => return,
    };
    drop(queued);
    // The first pass sends at once, and what it sends names the app.
    let named = tokio::select! {
        biased;
        named = app_name_known(&mut app_name, patience) => named,
        _ = shutdown.changed() => return,
    };
    if !named {
        // A reminder that describes the app is worth more than none: the
        // gateway may stay unreachable for the whole due window.
        warn!("reminders: Discord has not said what leaf's app is called; going on without it");
    }
    info!("reminder scheduler started");
    let mut scheduler = Scheduler::new(courier, series, app_name);
    loop {
        if *shutdown.borrow() {
            break;
        }
        scheduler.tick(clock(), &shutdown).await;
        tokio::select! {
            () = tokio::time::sleep(TICK) => {}
            _ = shutdown.changed() => break,
        }
    }
    info!("reminder scheduler stopped");
}

// ---------------------------------------------------------------------------
// The off switch
// ---------------------------------------------------------------------------

/// Custom id of the "Turn off reminders" button for a series:
/// `leaf:rem-off:{series_id}`.
#[must_use]
pub fn rem_off_id(series_id: i64) -> String {
    format!("{REM_OFF_PREFIX}{series_id}")
}

/// The series a "Turn off reminders" button belongs to, or `None` when
/// `custom_id` is not one of those buttons.
#[must_use]
pub fn parse_rem_off_id(custom_id: &str) -> Option<i64> {
    let id = custom_id.strip_prefix(REM_OFF_PREFIX)?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse().ok().filter(|id| *id > 0)
}

/// What a press on "Turn off reminders" came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOff {
    /// Reminders were on and are now off. Carries the series name.
    Done(String),
    /// Reminders were off already. Carries the series name.
    AlreadyOff(String),
    /// Someone other than the series creator pressed the button (it is
    /// public when the reminder is a channel ping). Nothing changed.
    NotCreator,
    /// The series no longer exists.
    Gone,
}

impl TurnOff {
    /// The reply for the person who pressed.
    #[must_use]
    pub fn text(&self) -> String {
        self.text_for(true)
    }

    /// The reply, naming the series only when `named`. A reminder posted in
    /// a channel is edited in place, where everyone reads the result, so a
    /// series not everyone may see is called "this series" there.
    #[must_use]
    pub fn text_for(&self, named: bool) -> String {
        let series = |name: &str| {
            if named {
                format!("**{}**", display_name(name))
            } else {
                "this series".to_owned()
            }
        };
        match self {
            Self::Done(name) => format!(
                "🍃 Reminders for {} are off. To turn them back on, {SETTINGS_HINT}.",
                series(name)
            ),
            Self::AlreadyOff(name) => {
                format!("🍃 Reminders for {} are already off.", series(name))
            }
            Self::NotCreator => {
                "🍂 Only the person who created this series can turn its reminders off.".to_owned()
            }
            Self::Gone => {
                "🍂 That series no longer exists, so it has no reminders to turn off.".to_owned()
            }
        }
    }

    /// True when the button has nothing left to do and should come off the
    /// message. False for [`Self::NotCreator`]: the creator may still want
    /// to press it.
    #[must_use]
    pub const fn settled(&self) -> bool {
        !matches!(self, Self::NotCreator)
    }
}

/// Turns reminders off for a series on behalf of `user_id`, who pressed
/// "Turn off reminders".
///
/// Only the creator may. The reminder time, timezone and DM choice are
/// kept, so switching back on in the gallery restores the same reminder. A
/// recorded delivery failure is cleared with it.
pub async fn turn_off(series: &SeriesRepo, series_id: i64, user_id: &str) -> DbResult<TurnOff> {
    let Some(s) = series.get(series_id).await? else {
        return Ok(TurnOff::Gone);
    };
    if s.creator_id != user_id {
        return Ok(TurnOff::NotCreator);
    }
    if !s.reminder_enabled {
        return Ok(TurnOff::AlreadyOff(s.name));
    }
    series
        .set_reminder_config(
            series_id,
            false,
            s.reminder_time.as_deref(),
            s.reminder_timezone.as_deref(),
            s.reminder_dm,
        )
        .await?;
    // The warning describes a reminder that no longer exists. Reminders are
    // already off, so a failure here is not the presser's problem.
    if let Err(e) = series.set_reminder_error(series_id, None, 0).await {
        warn!(series = series_id, error = %e, "reminder: could not clear the recorded failure");
    }
    info!(
        series = series_id,
        "reminders turned off from the reminder message"
    );
    Ok(TurnOff::Done(s.name))
}

/// Answers a press on "Turn off reminders", for the component router.
///
/// Turns the reminders off when the presser is the creator ([`turn_off`])
/// and tells them how it went. Returns `false`, having done nothing, when
/// `press` is not on that button.
///
/// The work is one database write, so the answer is direct (no defer). A
/// press on the DM carries no server, and the series may be revoked;
/// neither matters here.
pub async fn answer_off_press(
    http: &serenity::Http,
    press: &serenity::ComponentInteraction,
    series: &SeriesRepo,
) -> bool {
    let Some(series_id) = parse_rem_off_id(&press.data.custom_id) else {
        return false;
    };
    let response = match turn_off(series, series_id, &press.user.id.to_string()).await {
        Ok(outcome) => {
            // A DM is the creator's alone. In a channel the series is named
            // only when every member may see it anyway.
            let named = press.guild_id.is_none() || listed(series, series_id).await;
            off_response(&outcome, named, &press.message)
        }
        Err(e) => {
            warn!(series = series_id, error = %e, "reminder: could not turn reminders off");
            private_reply(&format!(
                "🍂 Reminders could not be turned off just now. Try the button again in a \
                 minute, or {SETTINGS_HINT}."
            ))
        }
    };
    // The change, if any, is already saved. A second press finds the
    // reminders off and settles the message then.
    match tokio::time::timeout(SEND_TIMEOUT, press.create_response(http, response)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            warn!(series = series_id, error = %e, "reminder: could not answer the off button");
        }
        Err(_) => warn!(
            series = series_id,
            "reminder: answering the off button timed out"
        ),
    }
    true
}

/// Whether every member may see the series (public and active). Unknown
/// (gone, or the read failed) counts as no.
async fn listed(series: &SeriesRepo, series_id: i64) -> bool {
    matches!(
        series.get(series_id).await,
        Ok(Some(s)) if s.privacy == Privacy::Public && s.state == SeriesState::Active
    )
}

/// The answer to a press on `message`'s "Turn off reminders" button.
///
/// Once the press is settled the reminder itself is edited: the result goes
/// under its text and the button comes off (the "Open channel" link stays).
/// Anyone else's press on a channel ping gets a private reply and leaves
/// the message as it is.
fn off_response(
    outcome: &TurnOff,
    named: bool,
    message: &serenity::Message,
) -> serenity::CreateInteractionResponse {
    if !outcome.settled() {
        return private_reply(&outcome.text());
    }
    serenity::CreateInteractionResponse::UpdateMessage(
        serenity::CreateInteractionResponseMessage::new()
            .content(format!("{}\n{}", message.content, outcome.text_for(named)))
            // An edit pings nobody, and the series name must not start to.
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .components(without_off_button(&message.components)),
    )
}

/// A reply only the presser sees.
fn private_reply(text: &str) -> serenity::CreateInteractionResponse {
    serenity::CreateInteractionResponse::Message(
        serenity::CreateInteractionResponseMessage::new()
            .content(text)
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .ephemeral(true),
    )
}

/// A reminder's buttons (it has no other components) minus "Turn off
/// reminders". A row left empty is dropped: Discord rejects one.
fn without_off_button(rows: &[serenity::ActionRow]) -> Vec<serenity::CreateActionRow> {
    rows.iter()
        .filter_map(|row| {
            let kept: Vec<serenity::CreateButton> = row
                .components
                .iter()
                .filter_map(|component| match component {
                    serenity::ActionRowComponent::Button(button) if !is_off_button(button) => {
                        Some(button.clone().into())
                    }
                    _ => None,
                })
                .collect();
            (!kept.is_empty()).then_some(serenity::CreateActionRow::Buttons(kept))
        })
        .collect()
}

fn is_off_button(button: &serenity::Button) -> bool {
    matches!(
        &button.data,
        serenity::ButtonKind::NonLink { custom_id, .. } if parse_rem_off_id(custom_id).is_some()
    )
}

// ---------------------------------------------------------------------------
// Discord seam
// ---------------------------------------------------------------------------

/// A Discord call that did not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    /// The HTTP status, when the request was answered at all.
    status: Option<u16>,
    /// Discord's JSON error code, when Discord answered with one.
    code: Option<isize>,
    /// What went wrong, for the log.
    detail: String,
}

impl Failure {
    fn of(error: &serenity::Error) -> Self {
        let (status, code) = match error {
            serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => (
                Some(response.status_code.as_u16()),
                Some(response.error.code),
            ),
            _ => (None, None),
        };
        // serenity's own text for a transport failure is one fixed
        // sentence; the cause is further down the chain.
        let mut parts = vec![error.to_string()];
        let mut cause = error.source();
        while let Some(inner) = cause {
            parts.push(inner.to_string());
            cause = inner.source();
        }
        // serenity's wrappers repeat the text of the error they wrap.
        parts.dedup();
        let parts: Vec<&str> = parts.iter().map(|p| p.trim_end_matches('.')).collect();
        Self {
            status,
            code,
            detail: parts.join(": "),
        }
    }

    /// True when Discord said "no" to this request and would say it again:
    /// one of its own 4xx answers, other than the three that are not about
    /// the request. Everything else may pass: no answer at all (network,
    /// timeout), a 5xx, an error page from something in front of Discord.
    const fn is_final(&self) -> bool {
        matches!(
            (self.status, self.code),
            (Some(status), Some(code)) if status >= 400
                && status < 500
                && !matches!(status, UNAUTHORIZED | REQUEST_TIMEOUT | TOO_MANY_REQUESTS)
                && code != NOT_DISCORDS_ANSWER
        )
    }
}

/// The Discord calls a reminder needs. A seam, so the delivery rules can be
/// tested without Discord.
trait Courier: Send + Sync {
    /// The server's name.
    fn guild_name(
        &self,
        guild: serenity::GuildId,
    ) -> impl Future<Output = Result<String, Failure>> + Send;

    /// Succeeds when `user` is a member of `guild`.
    fn member(
        &self,
        guild: serenity::GuildId,
        user: serenity::UserId,
    ) -> impl Future<Output = Result<(), Failure>> + Send;

    /// Sends `message` to `user` as a DM.
    fn dm(
        &self,
        user: serenity::UserId,
        message: serenity::CreateMessage,
    ) -> impl Future<Output = Result<(), Failure>> + Send;

    /// Sends `message` in `channel`.
    fn post(
        &self,
        channel: serenity::ChannelId,
        message: serenity::CreateMessage,
    ) -> impl Future<Output = Result<(), Failure>> + Send;

    /// Which channels of `guild` leaf can see. Never fails: when nothing
    /// can be found out, every channel counts as seen.
    fn sight(&self, guild: serenity::GuildId) -> impl Future<Output = Sight> + Send;
}

/// Discord itself, over the gateway client's HTTP handle and its cache.
struct Live {
    http: Arc<serenity::Http>,
    cache: Arc<serenity::Cache>,
}

impl Courier for Live {
    async fn guild_name(&self, guild: serenity::GuildId) -> Result<String, Failure> {
        // Only the name is read. `get_guild` decodes the whole server
        // (roles, emojis, stickers), and one field serenity does not expect
        // there would cost every DM its server name.
        let request = serenity::Request::new(
            serenity::Route::Guild { guild_id: guild },
            serenity::LightMethod::Get,
        );
        match self.http.fire::<serenity::json::Value>(request).await {
            Ok(found) => Ok(found
                .get("name")
                .and_then(serenity::json::Value::as_str)
                .unwrap_or_default()
                .to_owned()),
            Err(e) => Err(Failure::of(&e)),
        }
    }

    async fn member(
        &self,
        guild: serenity::GuildId,
        user: serenity::UserId,
    ) -> Result<(), Failure> {
        match self.http.get_member(guild, user).await {
            Ok(_) => Ok(()),
            Err(e) => Err(Failure::of(&e)),
        }
    }

    async fn dm(
        &self,
        user: serenity::UserId,
        message: serenity::CreateMessage,
    ) -> Result<(), Failure> {
        let channel = user
            .create_dm_channel(&self.http)
            .await
            .map_err(|e| Failure::of(&e))?;
        self.post(channel.id, message).await
    }

    async fn post(
        &self,
        channel: serenity::ChannelId,
        message: serenity::CreateMessage,
    ) -> Result<(), Failure> {
        // Sent with `request`, which stops at the status: a 2xx means the
        // reminder is out. `send_message` goes on to decode the message
        // Discord echoes back, and an echo serenity cannot decode would
        // turn a delivered reminder into an error, to be sent again.
        let body = serenity::json::to_vec(&message).map_err(|e| Failure::of(&e))?;
        let request = serenity::Request::new(
            serenity::Route::ChannelMessages {
                channel_id: channel,
            },
            serenity::LightMethod::Post,
        )
        .body(Some(body));
        match self.http.request(request).await {
            Ok(_) => Ok(()),
            Err(e) => Err(Failure::of(&e)),
        }
    }

    async fn sight(&self, guild: serenity::GuildId) -> Sight {
        Sight::look(&self.http, &self.cache, guild).await
    }
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// What became of one reminder.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// Delivered.
    Sent,
    /// Discord refused, and will keep refusing until the creator or an
    /// admin changes something.
    Refused(ReminderFailureKind),
    /// Discord refused for a reason leaf has no advice for, and would again.
    Rejected(Failure),
    /// There is nobody to remind.
    Skipped(Absent),
    /// It did not go through, but may on a later try.
    Failed(Failure),
}

/// Why there was nobody to remind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absent {
    /// leaf is no longer in the server (or the server is gone).
    Server,
    /// The creator is no longer in the server.
    Creator,
    /// The stored server or creator id is not a Discord id.
    BadId,
}

impl Absent {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "leaf is not in the server",
            Self::Creator => "the creator is not in the server",
            Self::BadId => "the stored server or creator id is not a Discord id",
        }
    }
}

/// What the scheduler learned about a server during one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Server {
    /// Its name.
    Named(String),
    /// The lookup failed for a reason that says nothing about the server;
    /// the DM goes out without the name.
    Unnamed,
    /// leaf is not in it.
    Gone,
}

/// What the scheduler asked about servers during one pass, so several
/// series in one server cost one request of each kind.
#[derive(Debug, Default)]
struct Servers {
    named: HashMap<serenity::GuildId, Server>,
    sights: HashMap<serenity::GuildId, Sight>,
}

impl Servers {
    fn new() -> Self {
        Self::default()
    }
}

/// Parses a stored snowflake. `None` for anything that is not a non-zero
/// number: serenity's id constructors panic on zero.
fn snowflake<T: From<NonZeroU64>>(raw: &str) -> Option<T> {
    raw.parse::<NonZeroU64>().ok().map(T::from)
}

async fn lookup_server<C: Courier>(
    courier: &C,
    guild: serenity::GuildId,
    seen: &mut Servers,
) -> Server {
    if let Some(known) = seen.named.get(&guild) {
        return known.clone();
    }
    let server = match courier.guild_name(guild).await {
        Ok(name) if name.trim().is_empty() => Server::Unnamed,
        Ok(name) => Server::Named(name),
        Err(failure) if matches!(failure.code, Some(UNKNOWN_GUILD | MISSING_ACCESS)) => {
            Server::Gone
        }
        Err(failure) => {
            debug!(%guild, error = %failure.detail, "reminder: server name lookup failed");
            Server::Unnamed
        }
    };
    seen.named.insert(guild, server.clone());
    server
}

/// Where a DM says the series posts: its channel while leaf can see it.
async fn lookup_place<C: Courier>(
    courier: &C,
    guild: serenity::GuildId,
    channel: Option<serenity::ChannelId>,
    seen: &mut Servers,
) -> Place {
    let Some(channel) = channel else {
        return Place::Unset;
    };
    let sight = match seen.sights.entry(guild) {
        Entry::Occupied(known) => known.into_mut(),
        Entry::Vacant(unknown) => unknown.insert(courier.sight(guild).await),
    };
    if sight.sees(&channel.to_string()) {
        Place::Channel(channel)
    } else {
        Place::Lost
    }
}

/// Sends the nudge for `day`: a DM to the creator, or a ping in the series'
/// first channel. `app` is the name Discord lists leaf's app under.
async fn send<C: Courier>(
    courier: &C,
    c: &ReminderCandidate,
    day: i64,
    app: &str,
    servers: &mut Servers,
) -> Outcome {
    let (Some(guild), Some(creator)) = (
        snowflake::<serenity::GuildId>(&c.guild_id),
        snowflake::<serenity::UserId>(&c.creator_id),
    ) else {
        return Outcome::Skipped(Absent::BadId);
    };

    // Someone who left the server should not hear about it again, by DM or
    // by a ping in a channel they can no longer see.
    if let Err(failure) = courier.member(guild, creator).await {
        match failure.code {
            Some(UNKNOWN_GUILD | MISSING_ACCESS) => return Outcome::Skipped(Absent::Server),
            Some(UNKNOWN_MEMBER | UNKNOWN_USER) => return Outcome::Skipped(Absent::Creator),
            // Not an answer about membership: let the send itself decide.
            _ => debug!(
                series = c.series_id,
                error = %failure.detail,
                "reminder: membership check failed"
            ),
        }
    }

    let channel = c
        .channels
        .first()
        .and_then(|raw| snowflake::<serenity::ChannelId>(raw));
    let sent = if c.reminder_dm {
        let server = match lookup_server(courier, guild, servers).await {
            Server::Gone => return Outcome::Skipped(Absent::Server),
            Server::Named(name) => Some(name),
            Server::Unnamed => None,
        };
        let place = lookup_place(courier, guild, channel, servers).await;
        let message = dm_message(c, day, guild, place, server.as_deref(), app);
        courier.dm(creator, message).await
    } else {
        let Some(channel) = channel else {
            return Outcome::Refused(ReminderFailureKind::ChannelMissing);
        };
        courier
            .post(channel, channel_message(c, day, creator, app))
            .await
    };

    match sent {
        Ok(()) => Outcome::Sent,
        Err(failure) => settle(failure, c.reminder_dm),
    }
}

/// What a send that did not go through comes to, for a DM (`dm`) or a
/// channel ping.
fn settle(failure: Failure, dm: bool) -> Outcome {
    let kind = failure
        .code
        .and_then(ReminderFailureKind::from_discord_code)
        // The gallery explains the reason in terms of the route the creator
        // chose, so only a reason that fits it is recorded: a DM fails as
        // "DMs closed", a ping as a channel problem.
        .filter(|kind| (*kind == ReminderFailureKind::DmClosed) == dm);
    match kind {
        Some(kind) => Outcome::Refused(kind),
        None if failure.is_final() => Outcome::Rejected(failure),
        None => Outcome::Failed(failure),
    }
}

/// The scheduler's state between ticks.
struct Scheduler<C> {
    courier: C,
    series: SeriesRepo,
    /// The name Discord lists leaf's app under, as the gateway last said it.
    app_name: watch::Receiver<String>,
    retries: Retries,
}

impl<C: Courier> Scheduler<C> {
    fn new(courier: C, series: SeriesRepo, app_name: watch::Receiver<String>) -> Self {
        Self {
            courier,
            series,
            app_name,
            retries: Retries::default(),
        }
    }

    /// One pass over all reminder candidates. Stops starting new sends once
    /// `shutdown` is raised.
    async fn tick(&mut self, now: i64, shutdown: &watch::Receiver<bool>) {
        let candidates = match self.series.reminder_candidates().await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "reminder tick: could not load candidates");
                return;
            }
        };

        let mut servers = Servers::new();
        for c in &candidates {
            if *shutdown.borrow() {
                break;
            }
            if !reminder_due(&c.inputs(now)) {
                // Record the check timestamp without touching the reminded day.
                if let Err(e) = self
                    .series
                    .set_reminder_state(c.series_id, c.last_reminder_day, now)
                    .await
                {
                    warn!(series = c.series_id, error = %e, "reminder: check-timestamp write failed");
                }
                continue;
            }
            let day = c.expected_day();
            if !self.retries.ready(c.series_id, day, now) {
                // Waiting out the gap after a failed send.
                continue;
            }
            self.deliver(c, day, now, &mut servers).await;
        }
        self.retries.prune(now);
    }

    /// Marks (before sending, so a crash can't double-send), sends, and
    /// settles the mark by how the send went.
    async fn deliver(&mut self, c: &ReminderCandidate, day: i64, now: i64, servers: &mut Servers) {
        let previous = c.last_reminder_day;

        if let Err(e) = self
            .series
            .set_reminder_state(c.series_id, Some(day), now)
            .await
        {
            warn!(series = c.series_id, error = %e, "reminder: mark-before-send failed; skipping");
            return;
        }

        // Read for each reminder: the bot may be renamed while leaf runs.
        let app = self.app_name.borrow().clone();
        let sending = send(&self.courier, c, day, &app, servers);
        let outcome = tokio::time::timeout(SEND_TIMEOUT, sending)
            .await
            .unwrap_or_else(|_| {
                Outcome::Failed(Failure {
                    status: None,
                    code: None,
                    detail: format!("no answer from Discord within {SEND_TIMEOUT:?}"),
                })
            });

        match outcome {
            Outcome::Sent => {
                self.retries.clear(c.series_id);
                self.record(c.series_id, None, now).await;
                info!(
                    series = c.series_id,
                    day,
                    dm = c.reminder_dm,
                    "reminder sent"
                );
            }
            Outcome::Refused(kind) => {
                self.retries.clear(c.series_id);
                self.record(c.series_id, Some(kind), now).await;
                warn!(
                    series = c.series_id,
                    day,
                    dm = c.reminder_dm,
                    reason = kind.as_str(),
                    "reminder refused; recorded for the creator, no retry for this day"
                );
            }
            Outcome::Rejected(failure) => {
                // Nothing is recorded: the gallery has no advice for this,
                // and an older recorded reason may still be the one that
                // helps the creator.
                self.retries.clear(c.series_id);
                warn!(
                    series = c.series_id,
                    day,
                    dm = c.reminder_dm,
                    status = failure.status,
                    code = failure.code,
                    error = %failure.detail,
                    "reminder rejected by Discord; no retry for this day"
                );
            }
            Outcome::Skipped(absent) => {
                self.retries.clear(c.series_id);
                info!(
                    series = c.series_id,
                    day,
                    reason = absent.as_str(),
                    "reminder skipped: nobody to remind"
                );
            }
            Outcome::Failed(failure) => {
                let retry_at = self.retries.failed(c.series_id, day, now);
                if let Err(e) = self
                    .series
                    .set_reminder_state(c.series_id, previous, now)
                    .await
                {
                    warn!(series = c.series_id, error = %e, "reminder: rollback failed");
                }
                if let Some(at) = retry_at {
                    warn!(
                        series = c.series_id,
                        day,
                        status = failure.status,
                        code = failure.code,
                        error = %failure.detail,
                        retry_in_secs = at - now,
                        "reminder send failed; will retry"
                    );
                } else {
                    warn!(
                        series = c.series_id,
                        day,
                        status = failure.status,
                        code = failure.code,
                        error = %failure.detail,
                        attempts = MAX_ATTEMPTS,
                        "reminder send keeps failing; giving up until the next reminder time"
                    );
                }
            }
        }
    }

    /// Stores why delivery failed (`Some`), or clears it after a success.
    async fn record(&self, series_id: i64, kind: Option<ReminderFailureKind>, now: i64) {
        let reason = kind.map(ReminderFailureKind::as_str);
        if let Err(e) = self.series.set_reminder_error(series_id, reason, now).await {
            warn!(series = series_id, error = %e, "reminder: delivery-status write failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Retries
// ---------------------------------------------------------------------------

/// Sends that failed in a way that may pass, by series. In memory only: a
/// restart forgets them, which costs at most one more round of attempts.
#[derive(Debug, Default)]
struct Retries(HashMap<i64, Retry>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Retry {
    /// The missing day the failed sends were for.
    day: i64,
    /// Failed sends so far.
    attempts: u32,
    /// When the first of them failed, unix seconds.
    since: i64,
    /// Earliest time of the next try, unix seconds.
    not_before: i64,
}

impl Retries {
    /// The failures that still count for this series and day. A record
    /// older than the longest due window belongs to a window that has
    /// closed, so the next window starts with a full set of attempts.
    fn live(&self, series_id: i64, day: i64, now: i64) -> Option<Retry> {
        self.0
            .get(&series_id)
            .copied()
            .filter(|r| r.day == day && now.saturating_sub(r.since) < DUE_WINDOW_SECS)
    }

    /// Whether a send for this series and day may be tried now.
    fn ready(&self, series_id: i64, day: i64, now: i64) -> bool {
        self.live(series_id, day, now)
            .is_none_or(|r| now >= r.not_before)
    }

    /// Notes a failed send. Returns when the next try may happen, or `None`
    /// when that was the last one for this window.
    fn failed(&mut self, series_id: i64, day: i64, now: i64) -> Option<i64> {
        let mut retry = self.live(series_id, day, now).unwrap_or(Retry {
            day,
            attempts: 0,
            since: now,
            not_before: now,
        });
        retry.attempts = retry.attempts.saturating_add(1);
        let next = (retry.attempts < MAX_ATTEMPTS)
            .then(|| now.saturating_add(backoff_secs(retry.attempts)));
        // Out of tries: stay quiet until this record stops counting.
        retry.not_before = next.unwrap_or_else(|| retry.since.saturating_add(DUE_WINDOW_SECS));
        self.0.insert(series_id, retry);
        next
    }

    fn clear(&mut self, series_id: i64) {
        self.0.remove(&series_id);
    }

    /// Drops records that no longer count (the series caught up, was
    /// deleted, or its window closed).
    fn prune(&mut self, now: i64) {
        self.0
            .retain(|_, r| now.saturating_sub(r.since) < DUE_WINDOW_SECS);
    }
}

/// Seconds to wait after the `attempts`-th failed send in a row.
const fn backoff_secs(attempts: u32) -> i64 {
    RETRY_BASE_SECS.saturating_mul(2_i64.saturating_pow(attempts.saturating_sub(1)))
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

fn off_button(series_id: i64) -> serenity::CreateButton {
    serenity::CreateButton::new(rem_off_id(series_id))
        .style(serenity::ButtonStyle::Secondary)
        .label("Turn off reminders")
}

/// Where a series posts, as far as a DM can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// In its channel, which leaf can see: named, and linked to.
    Channel(serenity::ChannelId),
    /// Its channel is one leaf can no longer see. A mention of it would
    /// read "#unknown" and a link to it would lead nowhere, so the DM has
    /// neither and says what happened.
    Lost,
    /// It has no channel, or what is stored is not a channel id.
    Unset,
}

/// The DM: what is missing and where, how to archive it, and a way out.
/// `app` is the name Discord lists leaf's app under, for the steps.
fn dm_message(
    c: &ReminderCandidate,
    day: i64,
    guild: serenity::GuildId,
    place: Place,
    server: Option<&str>,
    app: &str,
) -> serenity::CreateMessage {
    let mut buttons = Vec::with_capacity(2);
    if let Place::Channel(channel) = place {
        let url = format!("https://discord.com/channels/{guild}/{channel}");
        buttons.push(serenity::CreateButton::new_link(url).label("Open channel"));
    }
    buttons.push(off_button(c.series_id));
    serenity::CreateMessage::new()
        .content(dm_text(c, day, place, server, app))
        // Series and server names are other people's text: shown, never
        // parsed for mentions.
        .allowed_mentions(serenity::CreateAllowedMentions::new())
        .components(vec![serenity::CreateActionRow::Buttons(buttons)])
}

/// The channel ping. Only the creator is pinged, whatever the series name
/// contains. The button is public; only the creator's press counts.
fn channel_message(
    c: &ReminderCandidate,
    day: i64,
    creator: serenity::UserId,
    app: &str,
) -> serenity::CreateMessage {
    serenity::CreateMessage::new()
        .content(channel_text(c, day, creator, app))
        .allowed_mentions(serenity::CreateAllowedMentions::new().users([creator]))
        .components(vec![serenity::CreateActionRow::Buttons(vec![off_button(
            c.series_id,
        )])])
}

/// "Day N isn't archived yet" holds whether the creator has not posted or
/// has posted and not archived, and however far behind the series is.
fn dm_text(
    c: &ReminderCandidate,
    day: i64,
    place: Place,
    server: Option<&str>,
    app: &str,
) -> String {
    let name = display_name(&c.name);
    let server = server.map_or_else(String::new, |s| format!(" in {}", display_name(s.trim())));
    let posted = match place {
        Place::Channel(channel) => format!(" in <#{channel}>"),
        Place::Lost | Place::Unset => String::new(),
    };
    // Discord renders the timestamp as "2 days ago", in the reader's language.
    let last_post = c.last_post_at.map_or_else(String::new, |at| {
        format!("Your last archived post was <t:{at}:R>. ")
    });
    let settings = if place == Place::Lost {
        format!(
            "{}. To pick another, or to change your reminders, {SETTINGS_HINT}.",
            channels::lost("this series' channel")
        )
    } else {
        format!("To change your reminders, {SETTINGS_HINT}.")
    };
    format!(
        "🍃 Day {day} of **{name}**{server} isn't archived yet.\n\
         Once it's posted{posted}, {}.\n\
         -# {last_post}{settings}",
        menus::archive_steps(app)
    )
}

/// The channel ping's text. Everyone in the channel reads it, so a series
/// not everyone may see (private, role-only, or still a sprout) is not
/// named: its creator is mentioned and knows which one is meant.
fn channel_text(c: &ReminderCandidate, day: i64, creator: serenity::UserId, app: &str) -> String {
    let series = if c.listed {
        format!("**{}**", display_name(&c.name))
    } else {
        "your series".to_owned()
    };
    format!(
        "🍃 <@{creator}> Day {day} of {series} isn't archived yet. \
         Once it's posted here, {}.",
        menus::archive_steps(app)
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "tests may panic")]

    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::TimeZone as _;
    use leaf_core::db::{GuildSettingsRepo, PostRepo};
    use leaf_core::domain::{Cadence, DetectionMode, NewSeries, Post, Privacy, SeriesState};
    use serenity::json::Value;

    use super::*;
    use crate::stub::{self, Stub, stub};

    // -- fixtures ----------------------------------------------------------

    const GUILD: &str = "100";
    const CREATOR: &str = "111";
    const CHANNEL: &str = "222";
    /// What Discord lists the app as in these tests: not "leaf".
    const APP: &str = "leaf-dev";
    /// A wait for the app's name that outlasts any test, so what ends it
    /// there is never the clock.
    const NO_HURRY: Duration = Duration::from_hours(1);

    /// Tuesday 2026-06-09 18:00 UTC: half an hour into the 17:30 window.
    fn now() -> i64 {
        chrono::Utc
            .with_ymd_and_hms(2026, 6, 9, 18, 0, 0)
            .unwrap()
            .timestamp()
    }

    /// Discord's answer with this HTTP status and JSON error code.
    fn answer(status: u16, code: isize) -> Failure {
        Failure {
            status: Some(status),
            code: Some(code),
            detail: format!("Discord error {code} (HTTP {status})"),
        }
    }

    /// A refusal with this JSON error code, under the status Discord sends
    /// it with: 404 for the "unknown ..." codes (10xxx), 403 for the rest
    /// used here.
    fn refusal(code: isize) -> Failure {
        let status = if (10_000..11_000).contains(&code) {
            404
        } else {
            403
        };
        answer(status, code)
    }

    /// No answer at all.
    fn outage() -> Failure {
        Failure {
            status: None,
            code: None,
            detail: "connection reset".to_owned(),
        }
    }

    static NEXT_DB: AtomicUsize = AtomicUsize::new(0);

    /// A migrated database with one guild (timezone UTC), in its own temp
    /// directory (file-backed, like production), removed on drop.
    struct Db {
        dir: PathBuf,
        /// Kept so a test can close it.
        pool: leaf_core::db::SqlitePool,
        series: SeriesRepo,
        posts: PostRepo,
    }

    impl Db {
        async fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "leaf-bot-reminders-{}-{}-{}",
                std::process::id(),
                now_unix(),
                NEXT_DB.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let pool = leaf_core::db::connect(&dir.join("test.db")).await.unwrap();
            GuildSettingsRepo::new(pool.clone())
                .ensure_exists(GUILD)
                .await
                .unwrap();
            Self {
                dir,
                series: SeriesRepo::new(pool.clone()),
                posts: PostRepo::new(pool.clone()),
                pool,
            }
        }

        /// A daily series with a 17:30 reminder whose only post (Day 1) is
        /// a day old at [`now`]: due, for Day 2.
        async fn behind(&self, name: &str, dm: bool, channels: &[&str]) -> i64 {
            let series = self
                .series
                .create(
                    &NewSeries {
                        guild_id: GUILD.to_owned(),
                        creator_id: CREATOR.to_owned(),
                        name: name.to_owned(),
                        description: String::new(),
                        channels: channels.iter().map(|c| (*c).to_owned()).collect(),
                        cadence: Cadence::Daily,
                        detection_mode: DetectionMode::ContextMenu,
                        privacy: Privacy::Public,
                        privacy_role_id: None,
                        start_day: 1,
                        state: SeriesState::Active,
                    },
                    0,
                )
                .await
                .unwrap();
            self.series
                .set_reminder_config(series.id, true, Some("17:30"), None, dm)
                .await
                .unwrap();
            self.posts
                .insert_with_media(
                    &Post {
                        series_id: series.id,
                        day: 1,
                        message_id: format!("m{}", series.id),
                        channel_id: CHANNEL.to_owned(),
                        caption: String::new(),
                        posted_at: now() - 86_400,
                        archived_at: now() - 86_400,
                    },
                    &[],
                )
                .await
                .unwrap();
            series.id
        }

        /// The day the series was last marked as reminded for.
        async fn mark(&self, id: i64) -> Option<i64> {
            self.series
                .reminder_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.series_id == id)
                .unwrap()
                .last_reminder_day
        }

        /// The recorded delivery failure, as `(reason, at)`.
        async fn error(&self, id: i64) -> Option<(String, i64)> {
            self.series
                .reminder_error(id)
                .await
                .unwrap()
                .map(|f| (f.reason, f.at))
        }
    }

    impl Drop for Db {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.dir));
        }
    }

    /// One call the scheduler made to Discord.
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Member { guild: u64, user: u64 },
        GuildName(u64),
        Dm { user: u64, message: Value },
        Post { channel: u64, message: Value },
    }

    /// A scripted Discord that records what it was asked.
    struct Fake {
        member: Mutex<Result<(), Failure>>,
        guild: Mutex<Result<String, Failure>>,
        sending: Mutex<Result<(), Failure>>,
        /// Which channels leaf can see. Unknown unless a test says.
        sight: Mutex<Sight>,
        /// How often the channels were asked for. Not among `calls`: those
        /// are the requests a reminder is made of.
        looks: AtomicUsize,
        /// Shared, so two schedulers can write to one record.
        calls: Arc<Mutex<Vec<Call>>>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                member: Mutex::new(Ok(())),
                guild: Mutex::new(Ok("Cozy Art Server".to_owned())),
                sending: Mutex::new(Ok(())),
                sight: Mutex::new(Sight::unknown()),
                looks: AtomicUsize::new(0),
                calls: Arc::default(),
            }
        }
    }

    impl Fake {
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        /// The messages sent so far (DMs and channel posts).
        fn sends(&self) -> Vec<Call> {
            self.calls()
                .into_iter()
                .filter(|c| matches!(c, Call::Dm { .. } | Call::Post { .. }))
                .collect()
        }

        fn sending(&self, answer: Result<(), Failure>) {
            *self.sending.lock().unwrap() = answer;
        }

        /// From now on leaf can see exactly `channels`.
        fn sees(&self, channels: &[&str]) {
            *self.sight.lock().unwrap() = Sight::of(channels.iter().copied());
        }
    }

    impl Courier for Fake {
        async fn guild_name(&self, guild: serenity::GuildId) -> Result<String, Failure> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::GuildName(guild.get()));
            self.guild.lock().unwrap().clone()
        }

        async fn member(
            &self,
            guild: serenity::GuildId,
            user: serenity::UserId,
        ) -> Result<(), Failure> {
            self.calls.lock().unwrap().push(Call::Member {
                guild: guild.get(),
                user: user.get(),
            });
            self.member.lock().unwrap().clone()
        }

        async fn dm(
            &self,
            user: serenity::UserId,
            message: serenity::CreateMessage,
        ) -> Result<(), Failure> {
            self.calls.lock().unwrap().push(Call::Dm {
                user: user.get(),
                message: serenity::json::to_value(&message).unwrap(),
            });
            self.sending.lock().unwrap().clone()
        }

        async fn post(
            &self,
            channel: serenity::ChannelId,
            message: serenity::CreateMessage,
        ) -> Result<(), Failure> {
            self.calls.lock().unwrap().push(Call::Post {
                channel: channel.get(),
                message: serenity::json::to_value(&message).unwrap(),
            });
            self.sending.lock().unwrap().clone()
        }

        async fn sight(&self, _guild: serenity::GuildId) -> Sight {
            self.looks.fetch_add(1, Ordering::SeqCst);
            self.sight.lock().unwrap().clone()
        }
    }

    fn scheduler(db: &Db) -> Scheduler<Fake> {
        Scheduler::new(Fake::default(), db.series.clone(), named())
    }

    /// The app's name as the gateway said it at connect.
    fn named() -> watch::Receiver<String> {
        watch::channel(APP.to_owned()).1
    }

    /// A shutdown flag that is never raised.
    fn running() -> watch::Receiver<bool> {
        watch::channel(false).1
    }

    /// A candidate that is a day behind, for the message builders.
    fn candidate() -> ReminderCandidate {
        ReminderCandidate {
            series_id: 7,
            guild_id: GUILD.to_owned(),
            name: "Daily Johan".to_owned(),
            creator_id: CREATOR.to_owned(),
            channels: vec![CHANNEL.to_owned()],
            cadence: Cadence::Daily,
            reminder_time: "17:30".to_owned(),
            timezone: "UTC".to_owned(),
            reminder_dm: true,
            start_day: 1,
            max_day: Some(41),
            last_post_at: Some(1_780_000_000),
            last_reminder_day: None,
            listed: true,
        }
    }

    fn text_at<'a>(message: &'a Value, pointer: &str) -> &'a str {
        message
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("no string at {pointer} in {message}"))
    }

    // -- delivery ----------------------------------------------------------

    #[tokio::test]
    async fn a_delivered_dm_keeps_the_mark_and_clears_an_old_failure() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        db.series
            .set_reminder_error(id, Some("dm_closed"), 5)
            .await
            .unwrap();
        let mut scheduler = scheduler(&db);

        scheduler.tick(now(), &running()).await;

        let calls = scheduler.courier.calls();
        assert_eq!(
            calls.first(),
            Some(&Call::Member {
                guild: 100,
                user: 111
            })
        );
        assert_eq!(calls.get(1), Some(&Call::GuildName(100)));
        let Some(Call::Dm { user: 111, message }) = calls.get(2) else {
            panic!("expected a DM to the creator, got {calls:?}");
        };
        assert!(
            text_at(message, "/content").contains("Day 2 of **Daily Johan** in Cozy Art Server")
        );
        assert_eq!(calls.len(), 3);
        assert_eq!(db.mark(id).await, Some(2));
        assert_eq!(db.error(id).await, None);

        // The day is marked: later ticks say nothing more about it.
        scheduler.tick(now() + 60, &running()).await;
        scheduler.tick(now() + 3600, &running()).await;
        assert_eq!(scheduler.courier.calls().len(), 3);
    }

    #[tokio::test]
    async fn closed_dms_are_recorded_and_tried_once_per_missing_day() {
        for code in [50_007, 50_278] {
            let db = Db::new().await;
            let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
            let mut scheduler = scheduler(&db);
            scheduler.courier.sending(Err(refusal(code)));

            scheduler.tick(now(), &running()).await;

            assert_eq!(scheduler.courier.sends().len(), 1, "code {code}");
            // The mark stays, so the next tick does not try again...
            assert_eq!(db.mark(id).await, Some(2));
            assert_eq!(db.error(id).await, Some(("dm_closed".to_owned(), now())));
            for later in [60, 120, 3600] {
                scheduler.tick(now() + later, &running()).await;
            }
            assert_eq!(scheduler.courier.sends().len(), 1, "code {code}");
            // ...and nothing was posted in the channel instead.
            assert!(
                scheduler
                    .courier
                    .calls()
                    .iter()
                    .all(|c| !matches!(c, Call::Post { .. }))
            );

            // The next missing day gets its own single attempt.
            db.posts
                .insert_with_media(
                    &Post {
                        series_id: id,
                        day: 2,
                        message_id: "m-day-2".to_owned(),
                        channel_id: CHANNEL.to_owned(),
                        caption: String::new(),
                        posted_at: now(),
                        archived_at: now(),
                    },
                    &[],
                )
                .await
                .unwrap();
            scheduler.courier.sending(Ok(()));
            scheduler.tick(now() + 86_400, &running()).await;
            assert_eq!(scheduler.courier.sends().len(), 2);
            assert_eq!(db.mark(id).await, Some(3));
            assert_eq!(db.error(id).await, None);
        }
    }

    #[tokio::test]
    async fn channel_refusals_are_recorded_by_kind() {
        for (code, reason) in [
            (10_003, "channel_missing"),
            (50_001, "no_permission"),
            (50_013, "no_permission"),
        ] {
            let db = Db::new().await;
            let id = db.behind("Daily Johan", false, &[CHANNEL]).await;
            let mut scheduler = scheduler(&db);
            scheduler.courier.sending(Err(refusal(code)));

            scheduler.tick(now(), &running()).await;
            scheduler.tick(now() + 60, &running()).await;

            let sends = scheduler.courier.sends();
            assert!(
                matches!(sends.as_slice(), [Call::Post { channel: 222, .. }]),
                "code {code}: {sends:?}"
            );
            assert_eq!(db.mark(id).await, Some(2));
            assert_eq!(db.error(id).await, Some((reason.to_owned(), now())));
            // A channel ping never turns into a DM either.
            assert!(
                scheduler
                    .courier
                    .calls()
                    .iter()
                    .all(|c| !matches!(c, Call::Dm { .. } | Call::GuildName(_)))
            );
        }
    }

    #[tokio::test]
    async fn a_channel_ping_with_no_usable_channel_is_recorded_without_a_send() {
        for channels in [&[][..], &["not-a-channel"][..], &["0"][..]] {
            let db = Db::new().await;
            let id = db.behind("Daily Johan", false, channels).await;
            let mut scheduler = scheduler(&db);

            scheduler.tick(now(), &running()).await;
            scheduler.tick(now() + 60, &running()).await;

            assert!(scheduler.courier.sends().is_empty(), "{channels:?}");
            assert_eq!(db.mark(id).await, Some(2));
            assert_eq!(
                db.error(id).await,
                Some(("channel_missing".to_owned(), now()))
            );
        }
    }

    #[tokio::test]
    async fn a_dm_names_and_links_the_channel_only_while_leaf_can_see_it() {
        let db = Db::new().await;
        db.behind("Daily Johan", true, &[CHANNEL]).await;
        db.behind("Sketchbook", true, &["333"]).await;
        let mut scheduler = scheduler(&db);
        // Channel 333 was deleted (or hidden from leaf).
        scheduler.courier.sees(&[CHANNEL]);

        scheduler.tick(now(), &running()).await;

        let sends = scheduler.courier.sends();
        let dm_about = |series: &str| -> Value {
            sends
                .iter()
                .find_map(|call| match call {
                    Call::Dm { message, .. } if text_at(message, "/content").contains(series) => {
                        Some(message.clone())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no DM about {series} in {sends:?}"))
        };

        let seen = dm_about("Daily Johan");
        assert!(text_at(&seen, "/content").contains("Once it's posted in <#222>, "));
        assert_eq!(
            text_at(&seen, "/components/0/components/0/url"),
            "https://discord.com/channels/100/222"
        );

        let lost = dm_about("Sketchbook");
        let text = text_at(&lost, "/content");
        // No mention that would read "#unknown", and no link to nowhere.
        assert!(!text.contains("<#"), "{text}");
        assert!(text.contains("\nOnce it's posted, on a phone, "), "{text}");
        assert!(
            text.contains(
                "leaf can no longer see this series' channel (deleted, or hidden from leaf). To \
                 pick another, or to change your reminders, open leaf's gallery"
            ),
            "{text}"
        );
        assert_eq!(
            text_at(&lost, "/components/0/components/0/custom_id"),
            rem_off_id(2)
        );
        assert_eq!(lost.pointer("/components/0/components/1"), None);
        assert!(!lost.to_string().contains("discord.com/channels"), "{lost}");

        // Both series are in one server: its channels were asked for once.
        assert_eq!(scheduler.courier.looks.load(Ordering::SeqCst), 1);
        // The next pass asks again: a channel may be back by then.
        let mut servers = Servers::new();
        let again = lookup_place(
            &scheduler.courier,
            serenity::GuildId::new(100),
            Some(serenity::ChannelId::new(333)),
            &mut servers,
        )
        .await;
        assert_eq!(again, Place::Lost);
        assert_eq!(scheduler.courier.looks.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_channels_are_asked_for_only_when_a_dm_would_name_one() {
        let db = Db::new().await;
        // A ping goes to the channel itself: Discord's answer to the post
        // says whether it is there. A DM for a series with no channel has
        // none to name.
        let ping = db.behind("Sketchbook", false, &[CHANNEL]).await;
        db.behind("Unbound", true, &[]).await;
        db.behind("Odd", true, &["not-a-channel"]).await;
        let mut scheduler = scheduler(&db);
        scheduler.courier.sees(&[]);
        scheduler.courier.sending(Err(refusal(10_003)));

        scheduler.tick(now(), &running()).await;

        assert_eq!(scheduler.courier.looks.load(Ordering::SeqCst), 0);
        // The ping was still tried, and its refusal recorded as before.
        let sends = scheduler.courier.sends();
        assert!(
            sends
                .iter()
                .any(|call| matches!(call, Call::Post { channel: 222, .. })),
            "{sends:?}"
        );
        assert_eq!(
            db.error(ping).await,
            Some(("channel_missing".to_owned(), now()))
        );
        // Without a sight (the default here) a DM names what is stored.
        let unknown = Fake::default();
        let place = lookup_place(
            &unknown,
            serenity::GuildId::new(100),
            Some(serenity::ChannelId::new(222)),
            &mut Servers::new(),
        )
        .await;
        assert_eq!(place, Place::Channel(serenity::ChannelId::new(222)));
        assert_eq!(
            lookup_place(
                &unknown,
                serenity::GuildId::new(100),
                None,
                &mut Servers::new()
            )
            .await,
            Place::Unset
        );
    }

    #[tokio::test]
    async fn a_code_from_the_other_route_is_not_recorded() {
        // The gallery would explain "no permission in the channel" to
        // someone who chose a DM, or "DMs closed" to someone who chose a
        // ping. Neither should happen; if Discord ever answers that way it
        // is a refusal leaf has no advice for.
        for (dm, code) in [(true, 50_013), (true, 10_003), (false, 50_007)] {
            let db = Db::new().await;
            let id = db.behind("Daily Johan", dm, &[CHANNEL]).await;
            let mut scheduler = scheduler(&db);
            scheduler.courier.sending(Err(refusal(code)));

            scheduler.tick(now(), &running()).await;
            scheduler.tick(now() + 60, &running()).await;

            assert_eq!(scheduler.courier.sends().len(), 1);
            assert_eq!(db.mark(id).await, Some(2), "dm {dm}, code {code}");
            assert_eq!(db.error(id).await, None, "dm {dm}, code {code}");
        }
    }

    #[tokio::test]
    async fn a_refusal_leaf_cannot_explain_is_tried_once_per_missing_day() {
        // An archived thread, an AutoMod rule, a malformed request, a user
        // Discord does not know: the same request would get the same answer
        // a minute later.
        for (dm, status, code) in [
            (false, 400, 50_083),
            (false, 400, 200_000),
            (true, 400, 50_035),
            (true, 404, 10_013),
        ] {
            let db = Db::new().await;
            let id = db.behind("Daily Johan", dm, &[CHANNEL]).await;
            // What an earlier day recorded is not replaced by a guess.
            let earlier = if dm { "dm_closed" } else { "no_permission" };
            db.series
                .set_reminder_error(id, Some(earlier), 5)
                .await
                .unwrap();
            let mut scheduler = scheduler(&db);
            scheduler.courier.sending(Err(answer(status, code)));

            // Every minute until the window closes, and again the next day.
            for minute in 0..330 {
                scheduler.tick(now() + minute * 60, &running()).await;
            }
            scheduler.tick(now() + 86_400, &running()).await;

            assert_eq!(
                scheduler.courier.sends().len(),
                1,
                "status {status}, code {code}"
            );
            assert_eq!(db.mark(id).await, Some(2));
            assert_eq!(db.error(id).await, Some((earlier.to_owned(), 5)));

            // The next missing day gets its own attempt.
            db.posts
                .insert_with_media(
                    &Post {
                        series_id: id,
                        day: 2,
                        message_id: "m-day-2".to_owned(),
                        channel_id: CHANNEL.to_owned(),
                        caption: String::new(),
                        posted_at: now() + 86_400,
                        archived_at: now() + 86_400,
                    },
                    &[],
                )
                .await
                .unwrap();
            scheduler.tick(now() + 2 * 86_400, &running()).await;
            assert_eq!(scheduler.courier.sends().len(), 2);
            assert_eq!(db.mark(id).await, Some(3));
        }
    }

    #[test]
    fn a_failed_send_is_settled_by_code_then_by_status() {
        use ReminderFailureKind::{ChannelMissing, DmClosed, NoPermission};
        // What became of the send, without the failure it carries.
        #[derive(Debug, PartialEq)]
        enum Settled {
            Recorded(ReminderFailureKind),
            OncePerDay,
            Retried,
        }
        let settled = |dm: bool, failure: Failure| match settle(failure, dm) {
            Outcome::Refused(kind) => Settled::Recorded(kind),
            Outcome::Rejected(_) => Settled::OncePerDay,
            Outcome::Failed(_) => Settled::Retried,
            other => panic!("a failed send cannot come to {other:?}"),
        };

        for (dm, status, code, expected) in [
            // The five codes, on the route they belong to.
            (true, 403, 50_007, Settled::Recorded(DmClosed)),
            (true, 403, 50_278, Settled::Recorded(DmClosed)),
            (false, 404, 10_003, Settled::Recorded(ChannelMissing)),
            (false, 403, 50_001, Settled::Recorded(NoPermission)),
            (false, 403, 50_013, Settled::Recorded(NoPermission)),
            // On the other route they are a "no" like any other.
            (false, 403, 50_007, Settled::OncePerDay),
            (true, 404, 10_003, Settled::OncePerDay),
            (true, 403, 50_013, Settled::OncePerDay),
            // Any other 4xx of Discord's own.
            (false, 400, 50_083, Settled::OncePerDay),
            (false, 400, 200_000, Settled::OncePerDay),
            (true, 400, 50_035, Settled::OncePerDay),
            (true, 404, 10_013, Settled::OncePerDay),
            (false, 403, 0, Settled::OncePerDay),
            (false, 499, 0, Settled::OncePerDay),
            // The 4xx answers that are not about the request: a token
            // Discord does not accept, a timeout, a rate limit.
            (true, 401, 0, Settled::Retried),
            (false, 401, 40_001, Settled::Retried),
            (true, 408, 0, Settled::Retried),
            (false, 429, 0, Settled::Retried),
            (true, 429, 20_028, Settled::Retried),
            // A 4xx that is not Discord's answer (serenity reports code -1
            // for a body that is not Discord's JSON): a block page.
            (false, 403, -1, Settled::Retried),
            (true, 400, -1, Settled::Retried),
            // Discord's side, or something in front of it.
            (true, 500, 0, Settled::Retried),
            (false, 502, -1, Settled::Retried),
            (true, 503, 0, Settled::Retried),
            (false, 599, 0, Settled::Retried),
        ] {
            assert_eq!(
                settled(dm, answer(status, code)),
                expected,
                "dm {dm}, status {status}, code {code}"
            );
        }
        // No answer: a dropped connection, a timeout.
        for dm in [true, false] {
            assert_eq!(settled(dm, outage()), Settled::Retried);
        }
    }

    #[tokio::test]
    async fn a_passing_failure_rolls_back_and_retries_with_a_growing_gap() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = scheduler(&db);
        // A transport failure, then a Discord 5xx (its body carries code 0).
        scheduler.courier.sending(Err(outage()));

        scheduler.tick(now(), &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 1);
        assert_eq!(db.mark(id).await, None);
        assert_eq!(db.error(id).await, None);

        // Not on the very next tick...
        scheduler.tick(now() + 59, &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 1);
        // ...but a minute on.
        scheduler.courier.sending(Err(answer(502, 0)));
        scheduler.tick(now() + 60, &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 2);
        assert_eq!(db.mark(id).await, None);

        // The gap doubles: the third try waits two minutes.
        scheduler.tick(now() + 120, &running()).await;
        scheduler.tick(now() + 179, &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 2);

        // Discord is back: the reminder goes out and the day is settled.
        scheduler.courier.sending(Ok(()));
        scheduler.tick(now() + 180, &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 3);
        assert_eq!(db.mark(id).await, Some(2));
        assert_eq!(db.error(id).await, None);
        scheduler.tick(now() + 240, &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 3);
    }

    #[tokio::test]
    async fn a_failure_that_does_not_pass_stops_after_six_tries_until_the_next_window() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = scheduler(&db);
        scheduler.courier.sending(Err(outage()));

        // Every minute from 18:00 until the 17:30 window closes at 23:30.
        for minute in 0..330 {
            scheduler.tick(now() + minute * 60, &running()).await;
        }
        assert_eq!(scheduler.courier.sends().len(), 6);
        // Unmarked and unrecorded: an outage is not the creator's doing.
        assert_eq!(db.mark(id).await, None);
        assert_eq!(db.error(id).await, None);

        // Still behind the next day: the new window starts afresh.
        scheduler.courier.sending(Ok(()));
        scheduler.tick(now() + 86_400, &running()).await;
        assert_eq!(scheduler.courier.sends().len(), 7);
        assert_eq!(db.mark(id).await, Some(2));
    }

    #[tokio::test]
    async fn nobody_is_messaged_when_leaf_or_the_creator_left_the_server() {
        for code in [UNKNOWN_GUILD, MISSING_ACCESS, UNKNOWN_MEMBER, UNKNOWN_USER] {
            for dm in [true, false] {
                let db = Db::new().await;
                let id = db.behind("Daily Johan", dm, &[CHANNEL]).await;
                let mut scheduler = scheduler(&db);
                *scheduler.courier.member.lock().unwrap() = Err(refusal(code));

                scheduler.tick(now(), &running()).await;
                scheduler.tick(now() + 60, &running()).await;

                // One membership check, no message, and no warning to show
                // in a gallery nobody can open.
                assert_eq!(
                    scheduler.courier.calls(),
                    [Call::Member {
                        guild: 100,
                        user: 111
                    }],
                    "code {code}, dm {dm}"
                );
                assert_eq!(db.mark(id).await, Some(2));
                assert_eq!(db.error(id).await, None);
            }
        }
    }

    #[tokio::test]
    async fn a_server_lookup_that_says_leaf_is_gone_stops_the_dm() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = scheduler(&db);
        // The membership check could not answer; the name lookup can.
        *scheduler.courier.member.lock().unwrap() = Err(outage());
        *scheduler.courier.guild.lock().unwrap() = Err(refusal(MISSING_ACCESS));

        scheduler.tick(now(), &running()).await;

        assert!(scheduler.courier.sends().is_empty());
        assert_eq!(db.mark(id).await, Some(2));
        assert_eq!(db.error(id).await, None);
    }

    #[tokio::test]
    async fn the_dm_still_goes_out_when_the_lookups_cannot_answer() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = scheduler(&db);
        *scheduler.courier.member.lock().unwrap() = Err(outage());
        *scheduler.courier.guild.lock().unwrap() = Err(answer(500, 0));

        scheduler.tick(now(), &running()).await;

        let sends = scheduler.courier.sends();
        let [Call::Dm { message, .. }] = sends.as_slice() else {
            panic!("expected one DM, got {sends:?}");
        };
        assert!(
            text_at(message, "/content")
                .starts_with("🍃 Day 2 of **Daily Johan** isn't archived yet.\n")
        );
        assert_eq!(db.mark(id).await, Some(2));
    }

    #[tokio::test]
    async fn one_server_lookup_serves_every_series_in_a_pass() {
        let db = Db::new().await;
        for name in ["One", "Two", "Three"] {
            db.behind(name, true, &[CHANNEL]).await;
        }
        let mut scheduler = scheduler(&db);

        scheduler.tick(now(), &running()).await;

        let calls = scheduler.courier.calls();
        let lookups = calls
            .iter()
            .filter(|c| matches!(c, Call::GuildName(_)))
            .count();
        assert_eq!(lookups, 1);
        assert_eq!(scheduler.courier.sends().len(), 3);
    }

    #[tokio::test]
    async fn a_series_that_is_not_due_is_left_alone() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = scheduler(&db);

        // 17:00, half an hour before the reminder time.
        scheduler.tick(now() - 3600, &running()).await;

        assert!(scheduler.courier.calls().is_empty());
        assert_eq!(db.mark(id).await, None);
    }

    #[tokio::test]
    async fn a_raised_shutdown_starts_no_new_sends() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = scheduler(&db);
        let (stop, stopping) = watch::channel(false);
        stop.send(true).unwrap();

        scheduler.tick(now(), &stopping).await;

        assert!(scheduler.courier.calls().is_empty());
        assert_eq!(db.mark(id).await, None);
    }

    /// Yields until `done` holds. The limit only keeps a broken build from
    /// hanging the suite.
    async fn until(mut done: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !done() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_second_scheduler_waits_for_the_first_and_takes_over_when_it_stops() {
        // This test's own gate: the process-wide one is shared with every
        // other test in the binary.
        static GATE: Gate = Gate::new();

        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        // Sends fail in a way that may pass, so the series stays due and a
        // scheduler with no memory of the failure would send straight away.
        let log = Arc::<Mutex<Vec<Call>>>::default();
        let failing = || {
            let fake = Fake {
                calls: Arc::clone(&log),
                ..Fake::default()
            };
            fake.sending(Err(outage()));
            fake
        };
        let sends = || {
            let calls = log.lock().unwrap();
            calls
                .iter()
                .filter(|c| matches!(c, Call::Dm { .. }))
                .count()
        };

        let start = |stopping| {
            tokio::spawn(drive(
                &GATE,
                failing(),
                db.series.clone(),
                named(),
                NO_HURRY,
                stopping,
                now,
            ))
        };

        let (stop_first, first_stopping) = watch::channel(false);
        let (stop_second, second_stopping) = watch::channel(false);
        let first = start(first_stopping);
        until(|| sends() == 1).await;
        let second = start(second_stopping);
        // The second is waiting once it holds the place in the queue.
        until(|| GATE.next.try_lock().is_err()).await;

        // A third and a fourth (one per gateway retry) do not pile up
        // behind it: they return, and let go of what they were given.
        for _ in 0..2 {
            let (_stop, stopping) = watch::channel(false);
            tokio::time::timeout(Duration::from_secs(30), start(stopping))
                .await
                .unwrap()
                .unwrap();
        }

        // Time for the second to send if it were going to: each read is a
        // round trip through the database both schedulers use.
        for _ in 0..50 {
            db.mark(id).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(sends(), 1);

        // The first stops (as it does when its task ends): the second runs.
        stop_first.send(true).unwrap();
        first.await.unwrap();
        assert_eq!(db.mark(id).await, None);
        until(|| sends() == 2).await;

        // The place in the queue is free again for the next successor.
        let (stop_third, third_stopping) = watch::channel(false);
        let third = start(third_stopping);
        until(|| GATE.next.try_lock().is_err()).await;
        assert_eq!(sends(), 2);

        stop_second.send(true).unwrap();
        second.await.unwrap();
        until(|| sends() == 3).await;

        stop_third.send(true).unwrap();
        third.await.unwrap();
        assert_eq!(sends(), 3);
        assert!(GATE.active.try_lock().is_ok());
        assert!(GATE.next.try_lock().is_ok());
    }

    #[tokio::test]
    async fn a_scheduler_that_is_told_to_stop_while_waiting_gives_up_its_place() {
        static GATE: Gate = Gate::new();

        let db = Db::new().await;
        // Something else is ticking.
        let ticking = GATE.active.lock().await;

        let (stop, stopping) = watch::channel(false);
        let waiting = tokio::spawn(drive(
            &GATE,
            Fake::default(),
            db.series.clone(),
            named(),
            NO_HURRY,
            stopping,
            now,
        ));
        until(|| GATE.next.try_lock().is_err()).await;

        stop.send(true).unwrap();
        waiting.await.unwrap();
        assert!(GATE.next.try_lock().is_ok());
        drop(ticking);
    }

    // -- the app's name ---------------------------------------------------

    /// The text of the only message sent so far.
    fn only_send(calls: &[Call]) -> String {
        let sends: Vec<&Value> = calls
            .iter()
            .filter_map(|call| match call {
                Call::Dm { message, .. } | Call::Post { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        let [message] = sends.as_slice() else {
            panic!("expected one message, got {sends:?}");
        };
        text_at(message, "/content").to_owned()
    }

    #[tokio::test]
    async fn the_wait_for_the_app_s_name_ends_when_it_is_said_or_cannot_be() {
        // Known already: no wait, even with no patience at all.
        assert!(app_name_known(&mut named(), Duration::ZERO).await);

        // Said while waiting: the gateway connected.
        let (names, mut unnamed) = watch::channel(String::new());
        let waiting =
            tokio::spawn(
                async move { app_name_known(&mut unnamed, Duration::from_secs(30)).await },
            );
        names.send(APP.to_owned()).unwrap();
        assert!(waiting.await.unwrap());

        // Still not said when the patience runs out. A blank name is none.
        let (names, mut unnamed) = watch::channel(String::new());
        assert!(!app_name_known(&mut unnamed, Duration::ZERO).await);
        names.send("  ".to_owned()).unwrap();
        assert!(!app_name_known(&mut unnamed, Duration::ZERO).await);

        // The connection that would have said it is gone: no point waiting.
        drop(names);
        assert!(!app_name_known(&mut unnamed, Duration::from_secs(30)).await);
    }

    #[tokio::test]
    async fn the_first_pass_waits_for_the_app_s_name_and_then_names_it() {
        static GATE: Gate = Gate::new();

        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let log = Arc::<Mutex<Vec<Call>>>::default();
        let fake = Fake {
            calls: Arc::clone(&log),
            ..Fake::default()
        };
        // leaf has just started: the gateway has not said who the bot is.
        let (names, unnamed) = watch::channel(String::new());
        let (stop, stopping) = watch::channel(false);
        let ticking = tokio::spawn(drive(
            &GATE,
            fake,
            db.series.clone(),
            unnamed,
            NO_HURRY,
            stopping,
            now,
        ));

        // It is the scheduler now, and a reminder is due. Time for it to
        // send if it were going to: each read is a round trip through the
        // database it uses.
        until(|| GATE.active.try_lock().is_err()).await;
        for _ in 0..50 {
            db.mark(id).await;
            tokio::task::yield_now().await;
        }
        assert!(log.lock().unwrap().is_empty());

        // The gateway connects, and the reminder goes out naming the app.
        names.send(APP.to_owned()).unwrap();
        until(|| {
            log.lock()
                .unwrap()
                .iter()
                .any(|c| matches!(c, Call::Dm { .. }))
        })
        .await;
        let sent = only_send(&log.lock().unwrap());
        assert!(
            sent.contains("(scroll down to find it), **leaf-dev**, then **Archive to Series**"),
            "{sent}"
        );

        stop.send(true).unwrap();
        ticking.await.unwrap();
    }

    #[tokio::test]
    async fn a_scheduler_told_to_stop_while_it_waits_for_the_name_stops() {
        static GATE: Gate = Gate::new();

        let db = Db::new().await;
        db.behind("Daily Johan", true, &[CHANNEL]).await;
        let log = Arc::<Mutex<Vec<Call>>>::default();
        let fake = Fake {
            calls: Arc::clone(&log),
            ..Fake::default()
        };
        // No name is coming and the wait for one outlasts this test: only
        // being told to stop can end it.
        let (_names, unnamed) = watch::channel(String::new());
        let (stop, stopping) = watch::channel(false);
        let waiting = tokio::spawn(drive(
            &GATE,
            fake,
            db.series.clone(),
            unnamed,
            NO_HURRY,
            stopping,
            now,
        ));
        until(|| GATE.active.try_lock().is_err()).await;

        stop.send(true).unwrap();
        until(|| waiting.is_finished()).await;
        waiting.await.unwrap();
        // Nothing went out, and the next scheduler can start.
        assert!(log.lock().unwrap().is_empty());
        assert!(GATE.active.try_lock().is_ok());
    }

    #[tokio::test]
    async fn a_scheduler_that_is_never_told_the_name_starts_without_it() {
        static GATE: Gate = Gate::new();

        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let log = Arc::<Mutex<Vec<Call>>>::default();
        let fake = Fake {
            calls: Arc::clone(&log),
            ..Fake::default()
        };
        // The gateway has not connected, and the wait for it is over as
        // soon as it begins.
        let (_names, unnamed) = watch::channel(String::new());
        let (stop, stopping) = watch::channel(false);
        let ticking = tokio::spawn(drive(
            &GATE,
            fake,
            db.series.clone(),
            unnamed,
            Duration::ZERO,
            stopping,
            now,
        ));

        // Discord's REST API still answers, so the reminder that is due goes
        // out, describing the app it cannot name. (A scheduler that gave up
        // instead ends the wait too, with nothing sent.)
        let sent = || {
            let calls = log.lock().unwrap();
            calls.iter().any(|c| matches!(c, Call::Dm { .. }))
        };
        until(|| sent() || ticking.is_finished()).await;
        let text = only_send(&log.lock().unwrap());
        assert!(
            text.contains(
                "tap **Apps** (scroll down to find it), this bot's name, then **Archive to \
                 Series**."
            ),
            "{text}"
        );

        stop.send(true).unwrap();
        ticking.await.unwrap();
        assert_eq!(db.mark(id).await, Some(2));
    }

    #[tokio::test]
    async fn a_reminder_names_the_app_as_the_gateway_last_said_it() {
        let db = Db::new().await;
        db.behind("Daily Johan", false, &[CHANNEL]).await;
        let (names, name) = watch::channel(APP.to_owned());
        let mut scheduler = Scheduler::new(Fake::default(), db.series.clone(), name);
        // Sends fail in a way that may pass, so the same day is sent twice.
        scheduler.courier.sending(Err(outage()));

        scheduler.tick(now(), &running()).await;
        // The bot is renamed in the Developer Portal while leaf runs.
        names.send("Daily Art Bot".to_owned()).unwrap();
        scheduler.tick(now() + RETRY_BASE_SECS, &running()).await;

        let sends = scheduler.courier.sends();
        let [
            Call::Post { message: first, .. },
            Call::Post {
                message: second, ..
            },
        ] = sends.as_slice()
        else {
            panic!("expected two pings, got {sends:?}");
        };
        let first = text_at(first, "/content");
        assert!(
            first.contains("(scroll down to find it), **leaf-dev**, then **Archive to Series**"),
            "{first}"
        );
        let second = text_at(second, "/content");
        assert!(
            second.contains(
                "(scroll down to find it), **Daily Art Bot**, then **Archive to Series**"
            ),
            "{second}"
        );
        assert!(!second.contains("leaf-dev"), "{second}");
    }

    #[tokio::test]
    async fn a_reminder_still_goes_out_when_the_app_s_name_never_arrived() {
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        // The gateway never connected, but Discord's REST API answers.
        let unnamed = watch::channel(String::new()).1;
        let mut scheduler = Scheduler::new(Fake::default(), db.series.clone(), unnamed);

        scheduler.tick(now(), &running()).await;

        let sent = only_send(&scheduler.courier.calls());
        assert!(
            sent.contains(
                "on a phone, press and hold the post, tap **Apps** (scroll down to find it), \
                 this bot's name, then **Archive to Series**."
            ),
            "{sent}"
        );
        assert_eq!(db.mark(id).await, Some(2));
    }

    #[tokio::test]
    async fn stored_ids_that_are_not_snowflakes_never_reach_discord() {
        let courier = Fake::default();
        for (guild, creator) in [("0", CREATOR), (GUILD, "not-a-user"), ("", "")] {
            let mut c = candidate();
            c.guild_id = guild.to_owned();
            c.creator_id = creator.to_owned();
            let outcome = send(&courier, &c, 42, APP, &mut Servers::new()).await;
            assert_eq!(outcome, Outcome::Skipped(Absent::BadId));
        }
        assert!(courier.calls().is_empty());

        assert_eq!(
            snowflake::<serenity::UserId>("111").map(serenity::UserId::get),
            Some(111)
        );
        for bad in ["0", "", "-1", "1.5", "abc", " 111"] {
            assert_eq!(snowflake::<serenity::UserId>(bad), None, "{bad:?}");
        }
    }

    // -- serenity glue -----------------------------------------------------

    /// Discord at `base`, with a gateway cache that has no server in it.
    fn live(base: &str) -> Live {
        Live {
            http: Arc::new(stub::http(base)),
            cache: Arc::new(serenity::Cache::new()),
        }
    }

    #[tokio::test]
    async fn discord_refusals_keep_their_code_through_serenity() {
        let stub = stub(&[
            (
                "POST /api/v10/users/@me/channels ",
                200,
                r#"{"id":"555","type":1,"recipients":[{"id":"111","username":"johan"}]}"#,
            ),
            (
                "POST /api/v10/channels/555/messages ",
                403,
                r#"{"message":"Cannot send messages to this user","code":50007}"#,
            ),
            (
                "POST /api/v10/channels/222/messages ",
                403,
                r#"{"message":"Missing Permissions","code":50013}"#,
            ),
            (
                "GET /api/v10/guilds/100/members/111 ",
                404,
                r#"{"message":"Unknown Member","code":10007}"#,
            ),
            (
                "GET /api/v10/guilds/100 ",
                403,
                r#"{"message":"Missing Access","code":50001}"#,
            ),
            ("GET /api/v10/guilds/101 ", 502, "<html>Bad Gateway</html>"),
        ])
        .await;
        let live = live(&stub.base);
        let guild = serenity::GuildId::new(100);
        let creator = serenity::UserId::new(111);
        let channel = serenity::ChannelId::new(222);

        // A DM to someone whose DMs are closed: the channel opens, the
        // message is refused.
        let message = dm_message(
            &candidate(),
            42,
            guild,
            Place::Channel(channel),
            Some("Cozy"),
            APP,
        );
        let refused = live.dm(creator, message).await.unwrap_err();
        assert_eq!(refused.status, Some(403));
        assert_eq!(refused.code, Some(50_007));
        assert_eq!(refused.detail, "Cannot send messages to this user");
        assert_eq!(
            settle(refused, true),
            Outcome::Refused(ReminderFailureKind::DmClosed)
        );
        let requests = stub.requests.lock().unwrap().clone();
        let [(_, opened), (_, sent)] = requests.as_slice() else {
            panic!("expected two requests, got {requests:?}");
        };
        assert_eq!(opened, r#"{"recipient_id":"111"}"#);
        // What went over the wire is what the builders made.
        let sent: Value = serenity::json::from_str(sent.as_str()).unwrap();
        let built = dm_message(
            &candidate(),
            42,
            guild,
            Place::Channel(channel),
            Some("Cozy"),
            APP,
        );
        assert_eq!(sent, serenity::json::to_value(built).unwrap());

        let message = channel_message(&candidate(), 42, creator, APP);
        let refused = live.post(channel, message).await.unwrap_err();
        assert_eq!(refused.code, Some(50_013));
        assert_eq!(
            settle(refused, false),
            Outcome::Refused(ReminderFailureKind::NoPermission)
        );

        let left = live.member(guild, creator).await.unwrap_err();
        assert_eq!(left.code, Some(UNKNOWN_MEMBER));
        let gone = live.guild_name(guild).await.unwrap_err();
        assert_eq!(gone.code, Some(MISSING_ACCESS));

        // A gateway error page has no Discord code: nothing permanent.
        let down = live
            .guild_name(serenity::GuildId::new(101))
            .await
            .unwrap_err();
        assert_eq!(down.status, Some(502));
        assert!(matches!(settle(down, true), Outcome::Failed(_)));
    }

    #[tokio::test]
    async fn a_refusal_outside_the_five_codes_keeps_its_status_through_serenity() {
        let stub = stub(&[
            (
                "POST /api/v10/channels/223/messages ",
                400,
                r#"{"message":"Thread is archived","code":50083}"#,
            ),
            (
                "POST /api/v10/channels/224/messages ",
                429,
                r#"{"message":"You are being rate limited.","retry_after":0.5,"global":false}"#,
            ),
            (
                "POST /api/v10/channels/225/messages ",
                403,
                "<html>error code: 1020</html>",
            ),
        ])
        .await;
        let live = live(&stub.base);
        let ping = |channel| {
            let message = channel_message(&candidate(), 42, serenity::UserId::new(111), APP);
            live.post(serenity::ChannelId::new(channel), message)
        };

        // The status is what tells a "no" from a failure that may pass.
        let archived = ping(223).await.unwrap_err();
        assert_eq!((archived.status, archived.code), (Some(400), Some(50_083)));
        assert!(matches!(settle(archived, false), Outcome::Rejected(_)));

        let limited = ping(224).await.unwrap_err();
        assert_eq!(limited.status, Some(429));
        assert!(matches!(settle(limited, false), Outcome::Failed(_)));

        // A block page from something in front of Discord is not Discord's
        // word on this reminder.
        let blocked = ping(225).await.unwrap_err();
        assert_eq!(
            (blocked.status, blocked.code),
            (Some(403), Some(NOT_DISCORDS_ANSWER))
        );
        assert!(matches!(settle(blocked, false), Outcome::Failed(_)));
    }

    /// Answers serenity decodes in full, as Discord sends them.
    const MEMBER: &str = r#"{"user":{"id":"111","username":"johan"},"roles":[],"joined_at":"2024-01-01T00:00:00+00:00","deaf":false,"mute":false,"flags":0}"#;
    const DM_CHANNEL: &str =
        r#"{"id":"555","type":1,"recipients":[{"id":"111","username":"johan"}]}"#;
    /// A server as the full decoder would refuse it: the role has an id and
    /// nothing else.
    const SPARSE_GUILD: &str = r#"{"id":"100","name":"Cozy Art Server","roles":[{"id":"1"}]}"#;

    #[tokio::test]
    async fn a_reminder_discord_accepted_is_never_sent_again() {
        // Discord took the message (2xx) and answered with something that
        // is not a message serenity can decode.
        let stub = stub(&[
            ("GET /api/v10/guilds/100/members/111 ", 200, MEMBER),
            ("GET /api/v10/guilds/100 ", 200, SPARSE_GUILD),
            ("POST /api/v10/users/@me/channels ", 200, DM_CHANNEL),
            ("POST /api/v10/channels/555/messages ", 200, "{}"),
            ("POST /api/v10/channels/222/messages ", 200, "<html>"),
        ])
        .await;
        let db = Db::new().await;
        let dm = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let ping = db.behind("Sketchbook", false, &[CHANNEL]).await;
        let mut scheduler = Scheduler::new(live(&stub.base), db.series.clone(), named());

        // The minutes a send that failed would be tried again at.
        for minute in [0, 1, 3, 7, 15, 31] {
            scheduler.tick(now() + minute * 60, &running()).await;
        }

        let requests = stub.requests.lock().unwrap().clone();
        let sent_to = |route: &str| -> Vec<Value> {
            requests
                .iter()
                .filter(|(line, _)| line.starts_with(route))
                .map(|(_, body)| serenity::json::from_str(body.as_str()).unwrap())
                .collect()
        };
        let dms = sent_to("POST /api/v10/channels/555/messages ");
        let [message] = dms.as_slice() else {
            panic!("expected one DM, got {dms:?}");
        };
        // The name came through although the server as a whole would not
        // decode.
        assert!(
            text_at(message, "/content").contains("Day 2 of **Daily Johan** in Cozy Art Server")
        );
        assert_eq!(sent_to("POST /api/v10/channels/222/messages ").len(), 1);
        for id in [dm, ping] {
            assert_eq!(db.mark(id).await, Some(2));
            assert_eq!(db.error(id).await, None);
        }
    }

    #[tokio::test]
    async fn a_live_dm_follows_discord_s_list_of_the_server_s_channels() {
        // The series posts in channel 222, and Discord no longer lists it.
        let stub = stub(&[
            ("GET /api/v10/guilds/100/members/111 ", 200, MEMBER),
            (
                "GET /api/v10/guilds/100/channels ",
                200,
                r#"[{"id":"999"}]"#,
            ),
            ("GET /api/v10/guilds/100 ", 200, SPARSE_GUILD),
            ("POST /api/v10/users/@me/channels ", 200, DM_CHANNEL),
            ("POST /api/v10/channels/555/messages ", 200, "{}"),
        ])
        .await;
        let db = Db::new().await;
        db.behind("Daily Johan", true, &[CHANNEL]).await;
        let mut scheduler = Scheduler::new(live(&stub.base), db.series.clone(), named());

        scheduler.tick(now(), &running()).await;

        let requests = stub.requests.lock().unwrap().clone();
        let (_, sent) = requests
            .iter()
            .find(|(line, _)| line.starts_with("POST /api/v10/channels/555/messages "))
            .unwrap_or_else(|| panic!("no DM among {requests:?}"));
        let sent: Value = serenity::json::from_str(sent.as_str()).unwrap();
        assert_eq!(
            sent,
            serenity::json::to_value(dm_message(
                &db.series
                    .reminder_candidates()
                    .await
                    .unwrap()
                    .into_iter()
                    .next()
                    .unwrap(),
                2,
                serenity::GuildId::new(100),
                Place::Lost,
                Some("Cozy Art Server"),
                APP,
            ))
            .unwrap()
        );
        assert!(!text_at(&sent, "/content").contains("<#222>"));
    }

    #[tokio::test]
    async fn the_server_lookup_reads_only_the_name() {
        let stub = stub(&[
            ("GET /api/v10/guilds/100 ", 200, SPARSE_GUILD),
            ("GET /api/v10/guilds/101 ", 200, r#"{"id":"101"}"#),
        ])
        .await;
        let live = live(&stub.base);

        let named = live.guild_name(serenity::GuildId::new(100)).await;
        assert_eq!(named, Ok("Cozy Art Server".to_owned()));
        // No name in the answer: the DM goes out without one.
        let nameless = lookup_server(&live, serenity::GuildId::new(101), &mut Servers::new()).await;
        assert_eq!(nameless, Server::Unnamed);
    }

    #[tokio::test]
    async fn a_connection_failure_has_no_code_and_says_why() {
        // A port nothing listens on.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);

        let message = channel_message(&candidate(), 42, serenity::UserId::new(111), APP);
        let failed = live(&base)
            .post(serenity::ChannelId::new(222), message)
            .await
            .unwrap_err();

        assert_eq!(failed.code, None);
        // More than serenity's fixed sentence: the cause is in the log line.
        let (first, cause) = failed.detail.split_once(": ").unwrap();
        assert_eq!(first, "Error while sending HTTP request");
        assert!(!cause.is_empty());
    }

    // -- retries -----------------------------------------------------------

    #[test]
    fn retries_wait_longer_each_time_and_then_stop() {
        let mut retries = Retries::default();
        assert!(retries.ready(1, 42, 1_000));

        // Failures at the earliest allowed moment each time.
        let mut at = 1_000;
        let mut gaps = Vec::new();
        while let Some(next) = retries.failed(1, 42, at) {
            assert!(!retries.ready(1, 42, next - 1));
            assert!(retries.ready(1, 42, next));
            gaps.push(next - at);
            at = next;
        }
        assert_eq!(gaps, [60, 120, 240, 480, 960]);
        assert_eq!(gaps.len() + 1, usize::try_from(MAX_ATTEMPTS).unwrap());

        // Out of tries: quiet for the rest of the window, however long.
        assert!(!retries.ready(1, 42, at + 60));
        assert!(!retries.ready(1, 42, 1_000 + DUE_WINDOW_SECS - 1));
        // Once no window that saw those failures can still be open, the
        // count starts again.
        assert!(retries.ready(1, 42, 1_000 + DUE_WINDOW_SECS));
        assert_eq!(
            retries.failed(1, 42, 1_000 + DUE_WINDOW_SECS),
            Some(1_000 + DUE_WINDOW_SECS + 60)
        );
    }

    #[test]
    fn retries_are_kept_per_series_and_per_day() {
        let mut retries = Retries::default();
        assert_eq!(retries.failed(1, 42, 1_000), Some(1_060));
        assert!(!retries.ready(1, 42, 1_030));
        // Another series, or the next missing day of the same one, is not
        // held back.
        assert!(retries.ready(2, 42, 1_030));
        assert!(retries.ready(1, 43, 1_030));
        assert_eq!(retries.failed(1, 43, 1_030), Some(1_090));

        retries.clear(1);
        assert!(retries.ready(1, 43, 1_031));

        // Old records are dropped.
        retries.failed(3, 9, 2_000);
        retries.prune(2_000 + DUE_WINDOW_SECS - 1);
        assert_eq!(retries.0.len(), 1);
        retries.prune(2_000 + DUE_WINDOW_SECS);
        assert!(retries.0.is_empty());
    }

    // -- messages ----------------------------------------------------------

    #[test]
    fn the_dm_says_what_is_missing_where_and_how_to_archive_it() {
        let guild = serenity::GuildId::new(100);
        let channel = serenity::ChannelId::new(222);
        assert_eq!(
            dm_text(
                &candidate(),
                42,
                Place::Channel(channel),
                Some("Cozy Art Server"),
                APP
            ),
            "🍃 Day 42 of **Daily Johan** in Cozy Art Server isn't archived yet.\n\
             Once it's posted in <#222>, on a phone, press and hold the post, tap **Apps** \
             (scroll down to find it), **leaf-dev**, then **Archive to Series**. On desktop: \
             right-click it, **Apps**, **Archive to Series**.\n\
             -# Your last archived post was <t:1780000000:R>. To change your reminders, open \
             leaf's gallery in the server, open the series and tap the gear (Series settings)."
        );

        // No server name, no channel, no post date: still whole sentences.
        let mut bare = candidate();
        bare.last_post_at = None;
        assert_eq!(
            dm_text(&bare, 42, Place::Unset, None, APP),
            format!(
                "🍃 Day 42 of **Daily Johan** isn't archived yet.\n\
                 Once it's posted, {}.\n\
                 -# To change your reminders, open leaf's gallery in the server, open the \
                 series and tap the gear (Series settings).",
                menus::archive_steps(APP)
            )
        );

        let message = serenity::json::to_value(dm_message(
            &candidate(),
            42,
            guild,
            Place::Channel(channel),
            Some("Cozy Art Server"),
            APP,
        ))
        .unwrap();
        // Nothing in the text can ping anyone.
        assert_eq!(
            message.pointer("/allowed_mentions"),
            Some(&serenity::json::json!({ "parse": [], "users": [], "roles": [] }))
        );
        // One row: where to post, and the way out.
        assert_eq!(
            message.pointer("/components"),
            Some(&serenity::json::json!([{
                "type": 1,
                "components": [
                    {
                        "type": 2,
                        "style": 5,
                        "label": "Open channel",
                        "url": "https://discord.com/channels/100/222",
                        "disabled": false,
                    },
                    {
                        "type": 2,
                        "style": 2,
                        "label": "Turn off reminders",
                        "custom_id": "leaf:rem-off:7",
                        "disabled": false,
                    },
                ],
            }]))
        );
        assert_eq!(message.pointer("/flags"), None);

        // A channel leaf can no longer see is neither named nor linked to;
        // the small print says what happened and where to pick another.
        assert_eq!(
            dm_text(&candidate(), 42, Place::Lost, Some("Cozy Art Server"), APP),
            format!(
                "🍃 Day 42 of **Daily Johan** in Cozy Art Server isn't archived yet.\n\
                 Once it's posted, {}.\n\
                 -# Your last archived post was <t:1780000000:R>. leaf can no longer see this \
                 series' channel (deleted, or hidden from leaf). To pick another, or to change \
                 your reminders, open leaf's gallery in the server, open the series and tap the \
                 gear (Series settings).",
                menus::archive_steps(APP)
            )
        );
        let lost = serenity::json::to_value(dm_message(
            &candidate(),
            42,
            guild,
            Place::Lost,
            Some("Cozy Art Server"),
            APP,
        ))
        .unwrap();
        assert_eq!(
            text_at(&lost, "/components/0/components/0/custom_id"),
            "leaf:rem-off:7"
        );
        assert_eq!(lost.pointer("/components/0/components/1"), None);

        // Without a channel there is nowhere to link to.
        let message =
            serenity::json::to_value(dm_message(&candidate(), 42, guild, Place::Unset, None, APP))
                .unwrap();
        assert_eq!(
            text_at(&message, "/components/0/components/0/custom_id"),
            "leaf:rem-off:7"
        );
        assert_eq!(message.pointer("/components/0/components/1"), None);
    }

    #[test]
    fn the_channel_ping_pings_only_the_creator() {
        let mut c = candidate();
        c.reminder_dm = false;
        let creator = serenity::UserId::new(111);
        assert_eq!(
            channel_text(&c, 42, creator, APP),
            "🍃 <@111> Day 42 of **Daily Johan** isn't archived yet. Once it's posted here, \
             on a phone, press and hold the post, tap **Apps** (scroll down to find it), \
             **leaf-dev**, then **Archive to Series**. On desktop: right-click it, **Apps**, \
             **Archive to Series**."
        );

        let message = serenity::json::to_value(channel_message(&c, 42, creator, APP)).unwrap();
        assert_eq!(
            message.pointer("/allowed_mentions"),
            Some(&serenity::json::json!({ "parse": [], "users": ["111"], "roles": [] }))
        );
        // It should notify: no silent flag.
        assert_eq!(message.pointer("/flags"), None);
        // The channel is already open, so only the off switch.
        assert_eq!(
            text_at(&message, "/components/0/components/0/custom_id"),
            "leaf:rem-off:7"
        );
        assert_eq!(message.pointer("/components/0/components/1"), None);
    }

    #[test]
    fn a_series_not_everyone_may_see_is_not_named_in_a_channel() {
        let mut c = candidate();
        c.reminder_dm = false;
        c.listed = false;
        let creator = serenity::UserId::new(111);
        let ping = channel_text(&c, 42, creator, APP);
        assert!(
            ping.starts_with("🍃 <@111> Day 42 of your series isn't archived yet."),
            "{ping}"
        );
        assert!(!ping.contains("Daily Johan"), "{ping}");
        // The DM is the creator's alone, so it still names the series.
        assert!(dm_text(&c, 42, Place::Unset, None, APP).contains("**Daily Johan**"));

        // The same goes for the line added when the button is pressed.
        let done = TurnOff::Done("Daily Johan".to_owned());
        assert!(done.text_for(true).contains("**Daily Johan**"));
        let unnamed = done.text_for(false);
        assert!(
            unnamed.contains("Reminders for this series are off."),
            "{unnamed}"
        );
        assert!(!unnamed.contains("Daily Johan"));
        assert_eq!(done.text(), done.text_for(true));
    }

    #[test]
    fn names_cannot_ping_or_break_the_formatting() {
        let mut c = candidate();
        // Names saved before validation may hold anything.
        c.name = "<@&999> **@everyone**".to_owned();
        let creator = serenity::UserId::new(111);

        let ping = channel_text(&c, 42, creator, APP);
        assert!(ping.contains(r"**\<@&999\> \*\*@everyone\*\***"), "{ping}");
        let message = serenity::json::to_value(channel_message(&c, 42, creator, APP)).unwrap();
        assert_eq!(
            message.pointer("/allowed_mentions"),
            Some(&serenity::json::json!({ "parse": [], "users": ["111"], "roles": [] }))
        );

        let dm = dm_text(&c, 42, Place::Unset, Some(" **Shouty** <@1> "), APP);
        assert!(
            dm.contains(r" in \*\*Shouty\*\* \<@1\> isn't archived yet."),
            "{dm}"
        );
    }

    #[test]
    fn the_copy_makes_no_promise_about_streaks() {
        // The series can be several days behind when the reminder goes out.
        let creator = serenity::UserId::new(111);
        for text in [
            dm_text(&candidate(), 42, Place::Unset, Some("Cozy Art Server"), APP),
            channel_text(&candidate(), 42, creator, APP),
        ] {
            assert!(!text.to_lowercase().contains("streak"), "{text}");
            assert!(text.contains(&menus::archive_steps(APP)), "{text}");
            assert!(text.contains("Day 42"), "{text}");
        }
    }

    // -- the off switch ----------------------------------------------------

    #[test]
    fn rem_off_ids_round_trip() {
        assert_eq!(rem_off_id(7), "leaf:rem-off:7");
        assert_eq!(parse_rem_off_id("leaf:rem-off:7"), Some(7));
        assert_eq!(parse_rem_off_id(&rem_off_id(i64::MAX)), Some(i64::MAX));
        for other in [
            "leaf:rem-off:",
            "leaf:rem-off:0",
            "leaf:rem-off:-7",
            "leaf:rem-off:+7",
            "leaf:rem-off:7:1",
            "leaf:rem-off:7 ",
            "leaf:rem-off:99999999999999999999",
            "leaf:open:7",
            "rem-off:7",
            "",
        ] {
            assert_eq!(parse_rem_off_id(other), None, "{other:?}");
        }
        // Well inside Discord's 100-character limit for custom ids.
        assert!(rem_off_id(i64::MAX).len() <= 100);
    }

    #[tokio::test]
    async fn only_the_creator_can_turn_reminders_off() {
        let db = Db::new().await;
        let id = db.behind("daily_sketch", false, &[CHANNEL]).await;
        db.series
            .set_reminder_error(id, Some("no_permission"), 5)
            .await
            .unwrap();

        let pressed = turn_off(&db.series, id, "999").await.unwrap();
        assert_eq!(pressed, TurnOff::NotCreator);
        assert!(!pressed.settled());
        assert!(db.series.get(id).await.unwrap().unwrap().reminder_enabled);
        assert!(db.error(id).await.is_some());

        let pressed = turn_off(&db.series, id, CREATOR).await.unwrap();
        assert_eq!(pressed, TurnOff::Done("daily_sketch".to_owned()));
        assert!(pressed.settled());
        assert_eq!(
            pressed.text(),
            r"🍃 Reminders for **daily\_sketch** are off. To turn them back on, open leaf's gallery in the server, open the series and tap the gear (Series settings)."
        );
        let stored = db.series.get(id).await.unwrap().unwrap();
        assert!(!stored.reminder_enabled);
        // Switching back on restores the same reminder.
        assert_eq!(stored.reminder_time.as_deref(), Some("17:30"));
        assert!(!stored.reminder_dm);
        assert_eq!(db.error(id).await, None);
        // The scheduler no longer considers it.
        assert!(db.series.reminder_candidates().await.unwrap().is_empty());

        let again = turn_off(&db.series, id, CREATOR).await.unwrap();
        assert_eq!(again, TurnOff::AlreadyOff("daily_sketch".to_owned()));
        assert!(again.settled());
        assert!(again.text().contains("already off"));

        let missing = turn_off(&db.series, id + 100, CREATOR).await.unwrap();
        assert_eq!(missing, TurnOff::Gone);
        assert!(missing.settled());
    }

    /// Discord's "noted" for an answered press.
    const CALLBACK: (&str, u16, &str) = (
        "POST /api/v10/interactions/900/press-token/callback ",
        204,
        "",
    );

    /// A press by `user` on the button `custom_id` under `reminder` (the
    /// message as the builders made it): in the series channel when
    /// `in_server`, else in the DM, where Discord sends no server and no
    /// member.
    fn press(
        custom_id: &str,
        user: &str,
        in_server: bool,
        reminder: serenity::CreateMessage,
    ) -> serenity::ComponentInteraction {
        let reminder = serenity::json::to_value(reminder).unwrap();
        let who = serenity::json::json!({ "id": user, "username": "someone" });
        let mut press = serenity::json::json!({
            "id": "900",
            "application_id": "901",
            "type": 3,
            "token": "press-token",
            "version": 1,
            "data": { "custom_id": custom_id, "component_type": 2 },
            "channel_id": CHANNEL,
            "locale": "en-US",
            "entitlements": [],
            "attachment_size_limit": 10_485_760,
            "message": {
                "id": "800",
                "channel_id": CHANNEL,
                "author": { "id": "901", "username": "leaf", "bot": true },
                "content": reminder.pointer("/content"),
                "components": reminder.pointer("/components"),
                "timestamp": "2026-06-09T18:00:00+00:00",
                "edited_timestamp": null,
                "tts": false,
                "mention_everyone": false,
                "mentions": [],
                "mention_roles": [],
                "attachments": [],
                "embeds": [],
                "pinned": false,
                "type": 0,
            },
        });
        let fields = press.as_object_mut().unwrap();
        if in_server {
            let member = serenity::json::json!({
                "user": who,
                "roles": [],
                "joined_at": "2024-01-01T00:00:00+00:00",
                "deaf": false,
                "mute": false,
                "flags": 0,
            });
            fields.insert("guild_id".to_owned(), GUILD.into());
            fields.insert("member".to_owned(), member);
        } else {
            fields.insert("user".to_owned(), who);
        }
        serenity::json::from_value(press).unwrap()
    }

    /// The reminder DM for series `id`, as sent.
    fn dm_reminder(id: i64) -> serenity::CreateMessage {
        let mut c = candidate();
        c.series_id = id;
        dm_message(
            &c,
            2,
            serenity::GuildId::new(100),
            Place::Channel(serenity::ChannelId::new(222)),
            Some("Cozy Art Server"),
            APP,
        )
    }

    /// The channel ping for series `id`, as sent.
    fn ping_reminder(id: i64) -> serenity::CreateMessage {
        let mut c = candidate();
        c.series_id = id;
        c.reminder_dm = false;
        channel_message(&c, 2, serenity::UserId::new(111), APP)
    }

    /// The answers to presses the stub has seen, in order.
    fn answers(stub: &Stub) -> Vec<Value> {
        let requests = stub.requests.lock().unwrap();
        requests
            .iter()
            .map(|(line, body)| {
                assert!(line.starts_with(CALLBACK.0), "{line}");
                serenity::json::from_str(body.as_str()).unwrap()
            })
            .collect()
    }

    const NO_MENTIONS: &str = r#"{"parse":[],"users":[],"roles":[]}"#;

    #[tokio::test]
    async fn a_press_in_the_dm_turns_reminders_off_and_takes_the_button_away() {
        let stub = stub(&[CALLBACK]).await;
        let http = live(&stub.base).http;
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let reminder = serenity::json::to_value(dm_reminder(id)).unwrap();
        // No server on the press: it comes from the DM.
        let press = press(&rem_off_id(id), CREATOR, false, dm_reminder(id));
        assert_eq!(press.guild_id, None);

        assert!(answer_off_press(&http, &press, &db.series).await);

        assert!(!db.series.get(id).await.unwrap().unwrap().reminder_enabled);
        let answers = answers(&stub);
        let [answer] = answers.as_slice() else {
            panic!("expected one answer, got {answers:?}");
        };
        // The reminder is edited in place: its text, then the result.
        assert_eq!(answer.pointer("/type"), Some(&Value::from(7)));
        assert_eq!(
            text_at(answer, "/data/content"),
            format!(
                "{}\n🍃 Reminders for **Daily Johan** are off. To turn them back on, open \
                 leaf's gallery in the server, open the series and tap the gear (Series settings).",
                text_at(&reminder, "/content")
            )
        );
        // The link stays; the switch is gone.
        assert_eq!(
            answer.pointer("/data/components"),
            Some(&serenity::json::json!([{
                "type": 1,
                "components": [{
                    "type": 2,
                    "style": 5,
                    "label": "Open channel",
                    "url": "https://discord.com/channels/100/222",
                    "disabled": false,
                }],
            }]))
        );
        assert_eq!(
            answer.pointer("/data/allowed_mentions"),
            Some(&serenity::json::from_str::<Value>(NO_MENTIONS).unwrap())
        );
        assert_eq!(answer.pointer("/data/flags"), None);
    }

    #[tokio::test]
    async fn a_press_on_the_ping_counts_only_from_the_creator() {
        let stub = stub(&[CALLBACK]).await;
        let http = live(&stub.base).http;
        let db = Db::new().await;
        let id = db.behind("Daily Johan", false, &[CHANNEL]).await;
        let reminder = serenity::json::to_value(ping_reminder(id)).unwrap();

        // Someone else in the channel: told so in private, nothing changes.
        let other = press(&rem_off_id(id), "999", true, ping_reminder(id));
        assert!(answer_off_press(&http, &other, &db.series).await);
        assert!(db.series.get(id).await.unwrap().unwrap().reminder_enabled);

        // The creator (named by the press's member, as in any server).
        let creator = press(&rem_off_id(id), CREATOR, true, ping_reminder(id));
        assert!(answer_off_press(&http, &creator, &db.series).await);
        assert!(!db.series.get(id).await.unwrap().unwrap().reminder_enabled);

        let answers = answers(&stub);
        let [private, edit] = answers.as_slice() else {
            panic!("expected two answers, got {answers:?}");
        };
        assert_eq!(private.pointer("/type"), Some(&Value::from(4)));
        assert_eq!(
            text_at(private, "/data/content"),
            "🍂 Only the person who created this series can turn its reminders off."
        );
        // Ephemeral, and it does not touch the ping.
        assert_eq!(private.pointer("/data/flags"), Some(&Value::from(64)));
        assert_eq!(private.pointer("/data/components"), None);

        assert_eq!(edit.pointer("/type"), Some(&Value::from(7)));
        assert_eq!(
            text_at(edit, "/data/content"),
            format!(
                "{}\n🍃 Reminders for **Daily Johan** are off. To turn them back on, open \
                 leaf's gallery in the server, open the series and tap the gear (Series settings).",
                text_at(&reminder, "/content")
            )
        );
        // The switch was the ping's only button: no row is left.
        assert_eq!(
            edit.pointer("/data/components"),
            Some(&serenity::json::json!([]))
        );
        // The edit pings nobody, whatever the series is called.
        for answer in [private, edit] {
            assert_eq!(
                answer.pointer("/data/allowed_mentions"),
                Some(&serenity::json::from_str::<Value>(NO_MENTIONS).unwrap())
            );
        }
    }

    #[tokio::test]
    async fn a_press_for_a_series_that_is_gone_or_already_off_settles_the_message() {
        let stub = stub(&[CALLBACK]).await;
        let http = live(&stub.base).http;
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        db.series
            .set_reminder_config(id, false, Some("17:30"), None, true)
            .await
            .unwrap();

        // Switched off in the gallery since the reminder went out.
        let stale = press(&rem_off_id(id), CREATOR, false, dm_reminder(id));
        assert!(answer_off_press(&http, &stale, &db.series).await);
        // Deleted since.
        let gone = id + 100;
        let orphan = press(&rem_off_id(gone), CREATOR, false, dm_reminder(gone));
        assert!(answer_off_press(&http, &orphan, &db.series).await);

        let answers = answers(&stub);
        let [off, deleted] = answers.as_slice() else {
            panic!("expected two answers, got {answers:?}");
        };
        assert!(
            text_at(off, "/data/content")
                .ends_with("\n🍃 Reminders for **Daily Johan** are already off.")
        );
        assert!(
            text_at(deleted, "/data/content").ends_with(
                "\n🍂 That series no longer exists, so it has no reminders to turn off."
            )
        );
        for answer in [off, deleted] {
            assert_eq!(answer.pointer("/type"), Some(&Value::from(7)));
            assert_eq!(
                text_at(answer, "/data/components/0/components/0/label"),
                "Open channel"
            );
            assert_eq!(answer.pointer("/data/components/0/components/1"), None);
        }
    }

    #[tokio::test]
    async fn other_buttons_are_left_to_the_router() {
        let stub = stub(&[CALLBACK]).await;
        let http = live(&stub.base).http;
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;

        for custom_id in ["leaf:open:7", "leaf:rem-off:", "leaf:rem-off:abc", ""] {
            let press = press(custom_id, CREATOR, false, dm_reminder(id));
            assert!(
                !answer_off_press(&http, &press, &db.series).await,
                "{custom_id:?}"
            );
        }

        assert!(stub.requests.lock().unwrap().is_empty());
        assert!(db.series.get(id).await.unwrap().unwrap().reminder_enabled);
    }

    #[tokio::test]
    async fn a_press_that_cannot_be_saved_gets_a_private_apology() {
        let stub = stub(&[CALLBACK]).await;
        let http = live(&stub.base).http;
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let press = press(&rem_off_id(id), CREATOR, false, dm_reminder(id));
        db.pool.close().await;

        assert!(answer_off_press(&http, &press, &db.series).await);

        let answers = answers(&stub);
        let [answer] = answers.as_slice() else {
            panic!("expected one answer, got {answers:?}");
        };
        // The reminder keeps its button for the next try.
        assert_eq!(answer.pointer("/type"), Some(&Value::from(4)));
        assert_eq!(answer.pointer("/data/flags"), Some(&Value::from(64)));
        assert_eq!(
            text_at(answer, "/data/content"),
            "🍂 Reminders could not be turned off just now. Try the button again in a minute, \
             or open leaf's gallery in the server, open the series and tap the gear (Series settings)."
        );
    }

    #[tokio::test]
    async fn the_change_stands_when_discord_does_not_take_the_answer() {
        // Every route answers 404: the press has expired.
        let stub = stub(&[]).await;
        let http = live(&stub.base).http;
        let db = Db::new().await;
        let id = db.behind("Daily Johan", true, &[CHANNEL]).await;
        let press = press(&rem_off_id(id), CREATOR, false, dm_reminder(id));

        assert!(answer_off_press(&http, &press, &db.series).await);

        assert_eq!(stub.requests.lock().unwrap().len(), 1);
        assert!(!db.series.get(id).await.unwrap().unwrap().reminder_enabled);
    }
}
