//! Live `DiscordApi`: OAuth code exchange, guild lookups and log lines.
//!
//! Talks to the real Discord API. Membership uses the bot token (the bot is
//! in the guild), so the user needs no extra OAuth scope beyond `identify`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::auth::{
    DiscordApi, ExchangeError, GuildChannel, GuildMember, GuildRole, GuildSummary, LookupError,
};

const API: &str = "https://discord.com/api/v10";
const TIMEOUT: Duration = Duration::from_secs(10);
/// How many times a rate-limited (429) bot lookup is retried before giving
/// up. With the server-side membership cache single-flighting the gallery's
/// cold burst, this only matters for many *distinct* viewers at once.
const LOOKUP_RETRIES: u8 = 2;
/// Cap on how long a single `Retry-After` backoff will sleep.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Discord JSON error code: the guild does not exist for this bot.
const UNKNOWN_GUILD: i64 = 10_004;
/// Discord JSON error code: the bot has no access to the resource.
const MISSING_ACCESS: i64 = 50_001;

/// Message flag: deliver without a push or desktop notification.
const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;

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

    /// A bot-token GET that honours `Retry-After` on a 429 for a couple of
    /// attempts. Any other status is returned for the caller to read.
    async fn bot_get(&self, url: &str) -> Result<reqwest::Response, String> {
        let mut attempt = 0u8;
        loop {
            let resp = self
                .http
                .get(url)
                .header("Authorization", format!("Bot {}", self.bot_token))
                .send()
                .await
                .map_err(|e| format!("request failed: {e}"))?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < LOOKUP_RETRIES {
                let wait = retry_after(&resp).min(MAX_RETRY_BACKOFF);
                attempt += 1;
                tokio::time::sleep(wait).await;
            } else {
                return Ok(resp);
            }
        }
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

#[derive(Deserialize, Default)]
struct MemberUser {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    global_name: Option<String>,
}

#[derive(Deserialize)]
struct MemberResponse {
    roles: Vec<String>,
    /// ISO-8601 join timestamp; absent for some lazily-loaded members.
    #[serde(default)]
    joined_at: Option<String>,
    /// Server nickname, if the member set one.
    #[serde(default)]
    nick: Option<String>,
    #[serde(default)]
    user: Option<MemberUser>,
}

#[derive(Deserialize)]
struct ChannelResponse {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    kind: u8,
    #[serde(default)]
    position: i64,
}

#[derive(Deserialize)]
struct RoleResponse {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    position: i64,
    /// What manages the role, when something does.
    #[serde(default)]
    tags: Option<RoleTags>,
}

#[derive(Deserialize)]
struct RoleTags {
    /// Set on the role Discord made for a bot.
    #[serde(default)]
    bot_id: Option<String>,
}

impl RoleResponse {
    /// Whether this is the role Discord made for a bot. Only that bot has
    /// it. Other managed roles (server boosters, a streamer's subscribers)
    /// are held by members, so they can gate a series or who starts one;
    /// the bot's `/setup` draws the same line.
    fn is_bot_role(&self) -> bool {
        self.tags.as_ref().is_some_and(|t| t.bot_id.is_some())
    }
}

/// The roles members can hold, from Discord's role list: everything but
/// `@everyone` (its id equals the guild id) and bots' own roles.
fn holdable_roles(roles: Vec<RoleResponse>, guild_id: &str) -> Vec<GuildRole> {
    roles
        .into_iter()
        .filter(|r| r.id != guild_id && !r.is_bot_role())
        .map(|r| GuildRole {
            id: r.id,
            name: r.name,
            position: r.position,
        })
        .collect()
}

/// Longest Discord id: a snowflake is a 64-bit number, at most 20 digits.
const MAX_SNOWFLAKE_DIGITS: usize = 20;

/// Whether `id` is shaped like a Discord id. Ids are put into API URLs
/// sent with the bot token, and some come from a request's path, so one
/// holding `/`, `..`, `?` or `#` must never get that far.
fn is_snowflake(id: &str) -> bool {
    (1..=MAX_SNOWFLAKE_DIGITS).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Deserialize)]
struct GuildResponse {
    #[serde(default)]
    name: String,
    #[serde(default)]
    icon: Option<String>,
}

/// Discord's JSON error body; only the numeric code matters here.
#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(default)]
    code: i64,
}

#[derive(Serialize)]
struct AllowedMentions {
    /// Empty: mentions in the content render but ping nobody.
    parse: &'static [&'static str],
}

#[derive(Serialize)]
struct MessageRequest<'a> {
    content: &'a str,
    allowed_mentions: AllowedMentions,
    flags: u64,
}

/// Parses Discord's RFC-3339 `joined_at` into unix seconds.
fn parse_joined_at(raw: Option<String>) -> Option<i64> {
    raw.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.timestamp())
}

/// What the server calls a member: nickname, else global display name, else
/// username. Blank values are skipped.
fn display_name(nick: Option<String>, user: Option<MemberUser>) -> Option<String> {
    let user = user.unwrap_or_default();
    [nick, user.global_name, user.username]
        .into_iter()
        .flatten()
        .map(|n| n.trim().to_owned())
        .find(|n| !n.is_empty())
}

/// True when a failed bot lookup means "the bot is not in that guild":
/// Discord answers 404 Unknown Guild or 403 Missing Access.
const fn bot_is_absent(status: reqwest::StatusCode, code: i64) -> bool {
    (status.as_u16() == 404 && code == UNKNOWN_GUILD)
        || (status.as_u16() == 403 && code == MISSING_ACCESS)
}

/// The Discord JSON error code of a failed response (0 when unreadable).
async fn error_code(resp: reqwest::Response) -> i64 {
    resp.json::<ErrorResponse>().await.map_or(0, |e| e.code)
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
    async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<String, ExchangeError> {
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
            // `without_url`: the error text is logged, keep it free of noise.
            .map_err(|e| {
                ExchangeError::Unavailable(format!("request failed: {}", e.without_url()))
            })?;
        let status = resp.status();
        if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ExchangeError::Unavailable(format!("status {status}")));
        }
        if !status.is_success() {
            // 4xx: invalid_grant (spent or expired code, redirect mismatch)
            // or invalid_client (wrong credentials).
            return Err(ExchangeError::Rejected(format!("status {status}")));
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| ExchangeError::Unavailable(format!("bad token response: {e}")))?;
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
    ) -> Result<Option<GuildMember>, LookupError> {
        if !is_snowflake(guild_id) {
            // No such guild can exist, so the bot is not in it.
            return Err(LookupError::BotNotInGuild);
        }
        if !is_snowflake(user_id) {
            return Ok(None);
        }
        let url = format!("{API}/guilds/{guild_id}/members/{user_id}");
        let resp = self
            .bot_get(&url)
            .await
            .map_err(|e| LookupError::Unavailable(format!("member lookup: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            let m: MemberResponse = resp
                .json()
                .await
                .map_err(|e| LookupError::Unavailable(format!("bad member response: {e}")))?;
            return Ok(Some(GuildMember {
                roles: m.roles,
                joined_at: parse_joined_at(m.joined_at),
                name: display_name(m.nick, m.user),
            }));
        }
        let code = error_code(resp).await;
        if bot_is_absent(status, code) {
            return Err(LookupError::BotNotInGuild);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            // Unknown Member (or Unknown User): not in this guild.
            return Ok(None);
        }
        Err(LookupError::Unavailable(format!(
            "member lookup returned {status} (code {code})"
        )))
    }

    async fn guild_channels(&self, guild_id: &str) -> Result<Vec<GuildChannel>, String> {
        if !is_snowflake(guild_id) {
            return Err("channel list: not a guild id".to_owned());
        }
        let resp = self
            .bot_get(&format!("{API}/guilds/{guild_id}/channels"))
            .await
            .map_err(|e| format!("channel list: {e}"))?;
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
                kind: c.kind,
                position: c.position,
            })
            .collect())
    }

    async fn guild_roles(&self, guild_id: &str) -> Result<Vec<GuildRole>, String> {
        if !is_snowflake(guild_id) {
            return Err("role list: not a guild id".to_owned());
        }
        let resp = self
            .bot_get(&format!("{API}/guilds/{guild_id}/roles"))
            .await
            .map_err(|e| format!("role list: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("role list returned {}", resp.status()));
        }
        let roles: Vec<RoleResponse> = resp
            .json()
            .await
            .map_err(|e| format!("bad role list response: {e}"))?;
        Ok(holdable_roles(roles, guild_id))
    }

    async fn guild_summary(&self, guild_id: &str) -> Result<Option<GuildSummary>, String> {
        if !is_snowflake(guild_id) {
            return Ok(None);
        }
        let resp = self
            .bot_get(&format!("{API}/guilds/{guild_id}"))
            .await
            .map_err(|e| format!("guild lookup: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            let g: GuildResponse = resp
                .json()
                .await
                .map_err(|e| format!("bad guild response: {e}"))?;
            return Ok(Some(GuildSummary {
                name: g.name,
                icon: g.icon.filter(|i| !i.is_empty()),
            }));
        }
        let code = error_code(resp).await;
        if bot_is_absent(status, code) || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Err(format!("guild lookup returned {status} (code {code})"))
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

    async fn send_message(&self, channel_id: &str, content: &str) -> Result<(), String> {
        if !is_snowflake(channel_id) {
            return Err("message send: not a channel id".to_owned());
        }
        let resp = self
            .http
            .post(format!("{API}/channels/{channel_id}/messages"))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&MessageRequest {
                content,
                allowed_mentions: AllowedMentions { parse: &[] },
                flags: SUPPRESS_NOTIFICATIONS,
            })
            .send()
            .await
            .map_err(|e| format!("message send failed: {e}"))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let code = error_code(resp).await;
        Err(format!("message send returned {status} (code {code})"))
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    fn user(global: Option<&str>, username: Option<&str>) -> MemberUser {
        MemberUser {
            username: username.map(ToOwned::to_owned),
            global_name: global.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn display_name_prefers_nick_then_global_then_username() {
        let nick = Some("Nick".to_owned());
        assert_eq!(
            display_name(nick, Some(user(Some("Global"), Some("user")))).as_deref(),
            Some("Nick")
        );
        assert_eq!(
            display_name(None, Some(user(Some("Global"), Some("user")))).as_deref(),
            Some("Global")
        );
        assert_eq!(
            display_name(None, Some(user(None, Some("user")))).as_deref(),
            Some("user")
        );
        // Blank values are skipped rather than shown.
        assert_eq!(
            display_name(Some("  ".to_owned()), Some(user(Some(""), Some("user")))).as_deref(),
            Some("user")
        );
        assert_eq!(display_name(None, None), None);
    }

    #[test]
    fn member_response_reads_names_and_tolerates_their_absence() {
        let full: MemberResponse = serde_json::from_str(
            r#"{"roles":["r1"],"joined_at":"2024-01-02T03:04:05+00:00","nick":null,
                "user":{"id":"1","username":"sam","global_name":"Sam"}}"#,
        )
        .unwrap();
        assert_eq!(display_name(full.nick, full.user).as_deref(), Some("Sam"));
        assert_eq!(parse_joined_at(full.joined_at), Some(1_704_164_645));

        let bare: MemberResponse = serde_json::from_str(r#"{"roles":[]}"#).unwrap();
        assert_eq!(display_name(bare.nick, bare.user), None);
    }

    #[test]
    fn channel_and_role_responses_read_type_and_position() {
        let c: ChannelResponse =
            serde_json::from_str(r#"{"id":"1","name":"art","type":5,"position":3}"#).unwrap();
        assert_eq!((c.kind, c.position), (5, 3));
        let r: RoleResponse =
            serde_json::from_str(r#"{"id":"2","name":"VIP","position":7}"#).unwrap();
        assert_eq!(r.position, 7);
        assert!(!r.is_bot_role());
    }

    #[test]
    fn roles_members_hold_are_kept_and_bot_roles_dropped() {
        // As Discord sends them: the booster role's tag is a present-but-null
        // key, an integration (subscriber) role names its integration, and a
        // bot's role names the bot.
        let roles: Vec<RoleResponse> = serde_json::from_str(
            r#"[
                {"id":"100","name":"@everyone","position":0,"managed":false},
                {"id":"1","name":"VIP","position":4,"managed":false},
                {"id":"2","name":"Server Booster","position":3,"managed":true,
                 "tags":{"premium_subscriber":null}},
                {"id":"3","name":"Twitch Subscriber","position":2,"managed":true,
                 "tags":{"integration_id":"77"}},
                {"id":"4","name":"leaf","position":1,"managed":true,
                 "tags":{"bot_id":"55"}},
                {"id":"5","name":"Other bot","position":1,"managed":true,
                 "tags":{"bot_id":"56","integration_id":"78"}}
            ]"#,
        )
        .unwrap();
        let kept: Vec<String> = holdable_roles(roles, "100")
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(kept, ["VIP", "Server Booster", "Twitch Subscriber"]);
    }

    #[test]
    fn only_digit_ids_are_put_into_urls() {
        assert!(is_snowflake("1"));
        assert!(is_snowflake("123456789012345678"));
        assert!(is_snowflake(&"9".repeat(20)));
        for bad in [
            "",
            "g1",
            "1/members/2?",
            "../channels/3?",
            "1#",
            "1 ",
            "١٢٣",
            &"9".repeat(21),
        ] {
            assert!(!is_snowflake(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn lookups_with_a_malformed_id_answer_without_a_request() {
        // An unroutable proxy: any request that was sent would fail as
        // `Unavailable` / a "request failed" error instead of the answers below.
        let discord = LiveDiscord {
            http: reqwest::Client::builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap(),
            client_id: String::new(),
            client_secret: String::new(),
            bot_token: "t".to_owned(),
        };
        let bad = "1/members/2?";
        assert!(matches!(
            discord.guild_member(bad, "2").await,
            Err(LookupError::BotNotInGuild)
        ));
        assert!(matches!(discord.guild_member("1", bad).await, Ok(None)));
        assert_eq!(
            discord.guild_channels(bad).await.unwrap_err(),
            "channel list: not a guild id"
        );
        assert_eq!(
            discord.guild_roles(bad).await.unwrap_err(),
            "role list: not a guild id"
        );
        assert_eq!(discord.guild_summary(bad).await, Ok(None));
        assert_eq!(
            discord.send_message(bad, "hi").await.unwrap_err(),
            "message send: not a channel id"
        );
    }

    #[test]
    fn bot_absence_is_told_apart_from_a_missing_member() {
        use reqwest::StatusCode as S;
        assert!(bot_is_absent(S::NOT_FOUND, UNKNOWN_GUILD));
        assert!(bot_is_absent(S::FORBIDDEN, MISSING_ACCESS));
        // 10007 Unknown Member: the bot is there, the user is not.
        assert!(!bot_is_absent(S::NOT_FOUND, 10_007));
        assert!(!bot_is_absent(S::FORBIDDEN, 0));
    }

    #[test]
    fn log_lines_are_sent_without_pings_or_push() {
        let body = serde_json::to_value(MessageRequest {
            content: "hello <@1>",
            allowed_mentions: AllowedMentions { parse: &[] },
            flags: SUPPRESS_NOTIFICATIONS,
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "content": "hello <@1>",
                "allowed_mentions": { "parse": [] },
                "flags": 4096
            })
        );
    }
}
