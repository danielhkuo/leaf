//! Re-fetching original messages from Discord.
//!
//! Migration is more than a JSON copy: v2 stored Discord CDN URLs, which
//! expire (~24h since 2024), so the bytes must be re-fetched from the live
//! message *while it still exists*. The fetch sits behind [`MessageSource`]
//! so the importer is testable without the network (Discord is never called
//! in tests — see docs/rust-guidelines.md); [`LiveMessageSource`] is the real
//! implementation and is exercised only by the manual migration run.

use std::time::Duration;

use serde::Deserialize;

use crate::mapping;

/// One attachment as Discord currently reports it, with a resolved content
/// type (Discord may omit it, in which case we guess from the filename).
#[derive(Debug, Clone)]
pub struct FetchedAttachment {
    /// Discord attachment snowflake.
    pub id: String,
    /// MIME type (resolved; never empty).
    pub content_type: String,
    /// Freshly-signed CDN URL the bytes can be downloaded from.
    pub url: String,
}

/// A message fetched fresh from Discord: its caption and live attachments.
#[derive(Debug, Clone)]
pub struct FetchedMessage {
    /// Message content, used as the archived caption.
    pub content: String,
    /// Live attachments with usable download URLs.
    pub attachments: Vec<FetchedAttachment>,
}

/// Fetches original messages so their media can be re-archived.
///
/// `Ok(Some)` — message found, archive its attachments. `Ok(None)` — message
/// is gone (HTTP 404); record media as missing. `Err` — a transient or
/// unknown failure; the caller should *defer* the day (leave it unimported)
/// so a later re-run retries it rather than losing recoverable bytes.
pub trait MessageSource {
    /// Fetches one message by channel and message snowflake.
    fn fetch(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<Option<FetchedMessage>, String>> + Send;
}

const API: &str = "https://discord.com/api/v10";
const TIMEOUT: Duration = Duration::from_secs(30);
/// Cap on consecutive 429 backoffs before giving up on a message.
const MAX_RATE_LIMIT_RETRIES: u32 = 5;
/// Fallback backoff when Discord sends a 429 without a usable hint.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(1);

/// Talks to the real Discord API with the bot token.
pub struct LiveMessageSource {
    http: reqwest::Client,
    bot_token: String,
    /// Politeness delay between fetches (a one-shot migration makes hundreds
    /// to thousands of sequential calls; this keeps us under rate limits).
    delay: Duration,
}

impl LiveMessageSource {
    /// Builds the client from the bot token and an inter-request delay.
    pub fn new(bot_token: &str, delay: Duration) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            bot_token: bot_token.to_owned(),
            delay,
        })
    }
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    attachments: Vec<RawAttachment>,
}

#[derive(Deserialize)]
struct RawAttachment {
    id: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    content_type: Option<String>,
    url: String,
}

impl From<RawMessage> for FetchedMessage {
    fn from(raw: RawMessage) -> Self {
        let attachments = raw
            .attachments
            .into_iter()
            .map(|a| FetchedAttachment {
                content_type: a
                    .content_type
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| mapping::guess_content_type(&a.filename)),
                id: a.id,
                url: a.url,
            })
            .collect();
        Self {
            content: raw.content,
            attachments,
        }
    }
}

/// Parses Discord's `Retry-After` header (seconds, possibly fractional).
fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map_or(DEFAULT_BACKOFF, Duration::from_secs_f64)
}

impl MessageSource for LiveMessageSource {
    async fn fetch(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<Option<FetchedMessage>, String> {
        let url = format!("{API}/channels/{channel_id}/messages/{message_id}");
        let mut retries = 0_u32;
        loop {
            // Pace every attempt, including the first.
            tokio::time::sleep(self.delay).await;

            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("Bot {}", self.bot_token))
                .send()
                .await
                .map_err(|e| format!("request failed: {e}"))?;

            match resp.status() {
                s if s.is_success() => {
                    let raw: RawMessage = resp
                        .json()
                        .await
                        .map_err(|e| format!("bad message json: {e}"))?;
                    return Ok(Some(raw.into()));
                }
                // The message was deleted — definitively gone.
                reqwest::StatusCode::NOT_FOUND => return Ok(None),
                reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    retries += 1;
                    if retries > MAX_RATE_LIMIT_RETRIES {
                        return Err("rate limited: retries exhausted".to_owned());
                    }
                    let wait = retry_after(&resp);
                    tracing::warn!(channel_id, message_id, ?wait, "rate limited; backing off");
                    tokio::time::sleep(wait).await;
                }
                s => return Err(format!("message fetch returned {s}")),
            }
        }
    }
}
