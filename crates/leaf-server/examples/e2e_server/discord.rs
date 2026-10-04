//! A stand-in for Discord.
//!
//! Answers every call the API makes from the seed, records the log lines it
//! is asked to post, and can be switched while the server runs between
//! answering, failing as an unreachable Discord does, and holding calls back.
//!
//! A held call is not left to a timer alone. It is counted while it waits (so
//! a test can wait for "leaf is now waiting on Discord" as a condition), the
//! next change of behaviour lets go of it, and a reset abandons it.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use leaf_server::api::auth::{
    DiscordApi, ExchangeError, GuildChannel, GuildMember, GuildRole, GuildSummary, LookupError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::seed::{self, Persona};

/// An `OAuth` code is this followed by a persona's key (`code-viewer`). Any
/// other code is refused, as Discord refuses a spent or made-up one. Unlike
/// Discord's, these can be exchanged more than once.
pub const CODE_PREFIX: &str = "code-";

/// The access token a persona's code exchanges to (`access-viewer`).
pub const ACCESS_TOKEN_PREFIX: &str = "access-";

/// How long a call is held at most when the request does not say.
const DEFAULT_DELAY_MS: u64 = 5_000;

/// What a failed call reports, as a connection that could not be made.
const UNREACHABLE: &str = "request failed: the e2e stub Discord is down";

/// What a call reports when a reset came while it was being held.
const ABANDONED: &str = "request failed: the e2e server was reset during this call";

/// How the stub answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Answers from the seed.
    Ok,
    /// Fails every call, as when Discord cannot be reached.
    Down,
    /// Holds the call for `delay_ms` (or until the behaviour is set again),
    /// then answers from the seed.
    Slow,
}

/// One of the calls leaf makes to Discord (the methods of [`DiscordApi`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// `exchange_code`
    ExchangeCode,
    /// `current_user_id`
    CurrentUser,
    /// `guild_member`
    GuildMember,
    /// `guild_channels`
    GuildChannels,
    /// `guild_roles`
    GuildRoles,
    /// `guild_summary`
    GuildSummary,
    /// `managed_guild_ids`
    ManagedGuilds,
    /// `send_message`
    SendMessage,
}

/// The stub's current behaviour, as `POST /__e2e/discord` sets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Behaviour {
    /// How affected calls are answered.
    pub mode: Mode,
    /// The longest a call is held in [`Mode::Slow`], in milliseconds.
    #[serde(default = "default_delay_ms")]
    pub delay_ms: u64,
    /// The calls the mode applies to; every other call answers normally.
    /// Left out, the mode applies to all of them.
    #[serde(default)]
    pub only: Option<Vec<Op>>,
}

const fn default_delay_ms() -> u64 {
    DEFAULT_DELAY_MS
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            mode: Mode::Ok,
            delay_ms: DEFAULT_DELAY_MS,
            only: None,
        }
    }
}

/// A log line the stub was asked to post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SentMessage {
    /// The channel it was posted in.
    pub channel_id: String,
    /// Its text.
    pub content: String,
}

/// What a call reads to decide how it is answered.
#[derive(Default)]
struct Stance {
    /// How many resets there have been. A call held across one belongs to a
    /// world that is gone.
    resets: u64,
    behaviour: Behaviour,
}

/// What the stance says to do with a call.
enum Verdict {
    /// Answer it from the seed.
    Answer,
    /// Fail it, with this as the reason.
    Fail(&'static str),
    /// Hold it for this long, or until the stance changes.
    Hold(Duration),
}

impl Stance {
    /// The verdict on a call to `op` that began when there had been
    /// `started_in` resets.
    fn verdict(&self, started_in: u64, op: Op) -> Verdict {
        if self.resets != started_in {
            return Verdict::Fail(ABANDONED);
        }
        let affected = self
            .behaviour
            .only
            .as_ref()
            .is_none_or(|only| only.contains(&op));
        match self.behaviour.mode {
            Mode::Down if affected => Verdict::Fail(UNREACHABLE),
            Mode::Slow if affected => Verdict::Hold(Duration::from_millis(self.behaviour.delay_ms)),
            Mode::Ok | Mode::Down | Mode::Slow => Verdict::Answer,
        }
    }
}

/// The stub. One per process; a reset puts it back to answering.
pub struct StubDiscord {
    /// The server's own origin: the only redirect address a code is
    /// exchanged for.
    origin: String,
    /// Watched, so a call being held notices when it changes.
    stance: watch::Sender<Stance>,
    /// How many calls are being held right now.
    held: watch::Sender<u64>,
    sent: Mutex<Vec<SentMessage>>,
    calls: Mutex<BTreeMap<Op, u64>>,
}

/// One call being held, counted until it is answered, fails, or its request
/// goes away.
struct Hold<'a>(&'a watch::Sender<u64>);

impl<'a> Hold<'a> {
    fn new(held: &'a watch::Sender<u64>) -> Self {
        held.send_modify(|n| *n += 1);
        Self(held)
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// A poisoned lock still holds usable data here: every critical section is
/// one assignment or one push.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StubDiscord {
    /// A stub that answers normally, for a server at `origin`.
    pub fn new(origin: &str) -> Self {
        Self {
            origin: origin.to_owned(),
            stance: watch::Sender::new(Stance::default()),
            held: watch::Sender::new(0),
            sent: Mutex::default(),
            calls: Mutex::default(),
        }
    }

    /// How the stub answers at the moment.
    pub fn behaviour(&self) -> Behaviour {
        self.stance.borrow().behaviour.clone()
    }

    /// Changes how the stub answers, from the next call on. Calls being held
    /// are let go and answered as the new behaviour says: `ok` answers them
    /// now, `down` fails them, `slow` holds them again.
    pub fn set_behaviour(&self, behaviour: Behaviour) {
        self.stance
            .send_modify(|stance| stance.behaviour = behaviour);
    }

    /// The number of calls being held right now, to read or to wait on.
    pub fn held(&self) -> watch::Receiver<u64> {
        self.held.subscribe()
    }

    /// How many times each call has been made since the last reset.
    pub fn calls(&self) -> BTreeMap<Op, u64> {
        lock(&self.calls).clone()
    }

    /// Every log line posted since the last reset, oldest first.
    pub fn sent(&self) -> Vec<SentMessage> {
        lock(&self.sent).clone()
    }

    /// Back to answering normally, with nothing recorded. Calls being held
    /// fail: what they would answer belongs to the world before the reset,
    /// and leaf keeps no failed answer, so nothing of it reaches a cache
    /// the reset has just emptied.
    pub fn reset(&self) {
        self.stance.send_modify(|stance| {
            stance.resets += 1;
            stance.behaviour = Behaviour::default();
        });
        lock(&self.sent).clear();
        lock(&self.calls).clear();
    }

    /// Counts the call, then fails or holds it if the current behaviour
    /// says so.
    async fn gate(&self, op: Op) -> Result<(), String> {
        *lock(&self.calls).entry(op).or_default() += 1;
        let mut stance = self.stance.subscribe();
        // `borrow_and_update` marks the stance as seen, so `changed` below
        // resolves only for a change made after the verdict was read.
        let (started_in, mut verdict) = {
            let stance = stance.borrow_and_update();
            (stance.resets, stance.verdict(stance.resets, op))
        };
        // A call is held from its start or not at all.
        let _hold = matches!(verdict, Verdict::Hold(_)).then(|| Hold::new(&self.held));
        loop {
            match verdict {
                Verdict::Answer => return Ok(()),
                Verdict::Fail(why) => return Err(why.to_owned()),
                Verdict::Hold(delay) => {
                    tokio::select! {
                        () = tokio::time::sleep(delay) => return Ok(()),
                        // The sender is a field of `self`, so this is always
                        // a change: go round and read what it says now.
                        _ = stance.changed() => {}
                    }
                }
            }
            verdict = stance.borrow_and_update().verdict(started_in, op);
        }
    }

    /// Whether a code may be exchanged for `redirect_uri`: the server's own
    /// origin (the Activity) or its admin callback, the two addresses the
    /// application would have registered with Discord.
    fn is_registered_redirect(&self, redirect_uri: &str) -> bool {
        redirect_uri
            .strip_prefix(self.origin.as_str())
            .is_some_and(|rest| matches!(rest, "" | "/" | "/admin/callback"))
    }
}

/// The persona an access token belongs to.
fn token_owner(access_token: &str) -> Option<&'static Persona> {
    access_token
        .strip_prefix(ACCESS_TOKEN_PREFIX)
        .and_then(seed::persona)
}

/// What Discord answers for a guild the bot cannot see.
fn unknown_guild(guild_id: &str) -> String {
    format!("status 404 for guild {guild_id}")
}

impl DiscordApi for StubDiscord {
    async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<String, ExchangeError> {
        self.gate(Op::ExchangeCode)
            .await
            .map_err(ExchangeError::Unavailable)?;
        if !self.is_registered_redirect(redirect_uri) {
            return Err(ExchangeError::Rejected(format!(
                "redirect_uri {redirect_uri:?} is not registered"
            )));
        }
        code.strip_prefix(CODE_PREFIX)
            .and_then(seed::persona)
            .map(|persona| format!("{ACCESS_TOKEN_PREFIX}{}", persona.key))
            .ok_or_else(|| ExchangeError::Rejected("invalid_grant".to_owned()))
    }

    async fn current_user_id(&self, access_token: &str) -> Result<String, String> {
        self.gate(Op::CurrentUser).await?;
        token_owner(access_token)
            .map(|persona| persona.id.to_owned())
            .ok_or_else(|| "status 401".to_owned())
    }

    async fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<GuildMember>, LookupError> {
        self.gate(Op::GuildMember)
            .await
            .map_err(LookupError::Unavailable)?;
        if seed::guild(guild_id).is_none() {
            return Err(LookupError::BotNotInGuild);
        }
        Ok(seed::persona_by_id(user_id).and_then(|persona| {
            let membership = persona.membership(guild_id)?;
            Some(GuildMember {
                roles: membership.roles.iter().map(|&r| r.to_owned()).collect(),
                joined_at: Some(membership.joined_at),
                name: Some(persona.display_name.to_owned()),
            })
        }))
    }

    async fn guild_channels(&self, guild_id: &str) -> Result<Vec<GuildChannel>, String> {
        self.gate(Op::GuildChannels).await?;
        let guild = seed::guild(guild_id).ok_or_else(|| unknown_guild(guild_id))?;
        Ok(guild
            .channels
            .iter()
            .map(|c| GuildChannel {
                id: c.id.to_owned(),
                name: c.name.to_owned(),
                kind: c.kind,
                position: c.position,
            })
            .collect())
    }

    async fn guild_roles(&self, guild_id: &str) -> Result<Vec<GuildRole>, String> {
        self.gate(Op::GuildRoles).await?;
        let guild = seed::guild(guild_id).ok_or_else(|| unknown_guild(guild_id))?;
        Ok(guild
            .roles
            .iter()
            .map(|r| GuildRole {
                id: r.id.to_owned(),
                name: r.name.to_owned(),
                position: r.position,
            })
            .collect())
    }

    async fn guild_summary(&self, guild_id: &str) -> Result<Option<GuildSummary>, String> {
        self.gate(Op::GuildSummary).await?;
        // No icon: the panel would load it from Discord's CDN, and nothing
        // in an e2e run may leave this machine.
        Ok(seed::guild(guild_id).map(|guild| GuildSummary {
            name: guild.name.to_owned(),
            icon: None,
        }))
    }

    async fn managed_guild_ids(&self, access_token: &str) -> Result<Vec<String>, String> {
        self.gate(Op::ManagedGuilds).await?;
        token_owner(access_token)
            .map(|persona| persona.manages.iter().map(|&g| g.to_owned()).collect())
            .ok_or_else(|| "status 401".to_owned())
    }

    async fn send_message(&self, channel_id: &str, content: &str) -> Result<(), String> {
        self.gate(Op::SendMessage).await?;
        lock(&self.sent).push(SentMessage {
            channel_id: channel_id.to_owned(),
            content: content.to_owned(),
        });
        Ok(())
    }
}
