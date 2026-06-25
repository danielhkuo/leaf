//! Live `DiscordApi`: OAuth code exchange and guild-membership lookups.
//!
//! Talks to the real Discord API. Membership uses the bot token (the bot is
//! in the guild), so the user needs no extra OAuth scope beyond `identify`.

use std::time::Duration;

use serde::Deserialize;

use crate::api::auth::{DiscordApi, GuildChannel, GuildMember, GuildRole};

const API: &str = "https://discord.com/api/v10";
const TIMEOUT: Duration = Duration::from_secs(10);
/// How many times a rate-limited (429) member lookup is retried before giving
/// up. With the server-side membership cache single-flighting the gallery's
/// cold burst, this only matters for many *distinct* viewers at once.
const MEMBER_LOOKUP_RETRIES: u8 = 2;
/// Cap on how long a single `Retry-After` backoff will sleep.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Talks to Discord with the application's credentials.
#[derive(Clone)]
pub struct LiveDiscord {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    bot_token: String,
}

impl LiveDiscord {
    /// Builds the client from Tier-1 credentials.
    pub fn new(
        client_id: &str,
        client_secret: &str,
        bot_token: &str,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            bot_token: bot_token.to_owned(),
        })
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct UserResponse {
    id: String,
}

#[derive(Deserialize)]
struct MemberResponse {
    roles: Vec<String>,
    /// ISO-8601 join timestamp; absent for some lazily-loaded members.
    #[serde(default)]
    joined_at: Option<String>,
}

#[derive(Deserialize)]
struct ChannelResponse {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct RoleResponse {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    managed: bool,
}

/// Parses Discord's RFC-3339 `joined_at` into unix seconds.
fn parse_joined_at(raw: Option<String>) -> Option<i64> {
    raw.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp())
}

/// Discord's Manage-Guild permission bit.
const MANAGE_GUILD: u64 = 1 << 5;

#[derive(Deserialize)]
struct UserGuild {
    id: String,
    #[serde(default)]
    owner: bool,
    /// Stringified permission bitfield for the current user in this guild.
    permissions: String,
}

impl DiscordApi for LiveDiscord {
    async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<String, String> {
        let resp = self
            .http
            .post(format!("{API}/oauth2/token"))
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| format!("token exchange request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("Discord rejected the code ({})", resp.status()));
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("bad token response: {e}"))?;
        Ok(body.access_token)
    }

    async fn current_user_id(&self, access_token: &str) -> Result<String, String> {
        let resp = self
            .http
            .get(format!("{API}/users/@me"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| format!("user lookup failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("user lookup returned {}", resp.status()));
        }
        let user: UserResponse = resp
            .json()
            .await
            .map_err(|e| format!("bad user response: {e}"))?;
        Ok(user.id)
    }

    async fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<GuildMember>, String> {
        let url = format!("{API}/guilds/{guild_id}/members/{user_id}");
        let mut attempt = 0u8;
        loop {
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("Bot {}", self.bot_token))
                .send()
                .await
                .map_err(|e| format!("member lookup failed: {e}"))?;
            match resp.status() {
                s if s.is_success() => {
                    let m: MemberResponse = resp
                        .json()
                        .await
                        .map_err(|e| format!("bad member response: {e}"))?;
                    return Ok(Some(GuildMember {
                        roles: m.roles,
                        joined_at: parse_joined_at(m.joined_at),
                    }));
                }
                // 404 = the user is not a member of this guild.
                reqwest::StatusCode::NOT_FOUND => return Ok(None),
                // Rate-limited: honor Retry-After for a couple of attempts.
                reqwest::StatusCode::TOO_MANY_REQUESTS if attempt < MEMBER_LOOKUP_RETRIES => {
                    let wait = retry_after(&resp).min(MAX_RETRY_BACKOFF);
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                }
                s => return Err(format!("member lookup returned {s}")),
            }
        }
    }

    async fn guild_channels(&self, guild_id: &str) -> Result<Vec<GuildChannel>, String> {
        let resp = self
            .http
            .get(format!("{API}/guilds/{guild_id}/channels"))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .send()
            .await
            .map_err(|e| format!("channel list failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("channel list returned {}", resp.status()));
        }
        let channels: Vec<ChannelResponse> = resp
            .json()
            .await
            .map_err(|e| format!("bad channel list response: {e}"))?;
        Ok(channels
            .into_iter()
            .map(|c| GuildChannel {
                id: c.id,
                name: c.name,
            })
            .collect())
    }

    async fn guild_roles(&self, guild_id: &str) -> Result<Vec<GuildRole>, String> {
        let resp = self
            .http
            .get(format!("{API}/guilds/{guild_id}/roles"))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .send()
            .await
            .map_err(|e| format!("role list failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("role list returned {}", resp.status()));
        }
        let roles: Vec<RoleResponse> = resp
            .json()
            .await
            .map_err(|e| format!("bad role list response: {e}"))?;
        // Drop `@everyone` (its id equals the guild id) and managed roles
        // (bot/integration roles a human can't assign as a gate).
        Ok(roles
            .into_iter()
            .filter(|r| r.id != guild_id && !r.managed)
            .map(|r| GuildRole {
                id: r.id,
                name: r.name,
            })
            .collect())
    }

    async fn managed_guild_ids(&self, access_token: &str) -> Result<Vec<String>, String> {
        let resp = self
            .http
            .get(format!("{API}/users/@me/guilds"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| format!("guild list failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("guild list returned {}", resp.status()));
        }
        let guilds: Vec<UserGuild> = resp
            .json()
            .await
            .map_err(|e| format!("bad guild list response: {e}"))?;
        Ok(guilds
            .into_iter()
            .filter(|g| {
                g.owner
                    || g.permissions
                        .parse::<u64>()
                        .is_ok_and(|p| p & MANAGE_GUILD != 0)
            })
            .map(|g| g.id)
            .collect())
    }
}

/// Backoff for a 429 from Discord's `Retry-After` header (seconds, possibly
/// fractional), defaulting to a conservative 1s when absent or unparseable.
/// `try_from_secs_f64` keeps a negative/NaN header from panicking.
fn retry_after(resp: &reqwest::Response) -> Duration {
    const DEFAULT: Duration = Duration::from_secs(1);
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
        .unwrap_or(DEFAULT)
}
