//! Live `DiscordApi`: OAuth code exchange and guild-membership lookups.
//!
//! Talks to the real Discord API. Membership uses the bot token (the bot is
//! in the guild), so the user needs no extra OAuth scope beyond `identify`.

use serde::Deserialize;

use crate::api::auth::DiscordApi;

const API: &str = "https://discord.com/api/v10";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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

    async fn guild_member_roles(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<Vec<String>>, String> {
        let resp = self
            .http
            .get(format!("{API}/guilds/{guild_id}/members/{user_id}"))
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
                Ok(Some(m.roles))
            }
            // 404 = the user is not a member of this guild.
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            s => Err(format!("member lookup returned {s}")),
        }
    }
}
