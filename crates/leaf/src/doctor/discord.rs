//! Checks 2 to 4, asked of Discord's REST API with the bot token: who the
//! token is, whether leaf's commands are registered, and whether the gateway
//! will take a connection. Nothing here connects to the gateway or changes
//! anything on Discord: every call but one is a `GET`, and that one (the
//! client-credentials grant setup also uses) only asks whether the client id
//! and secret belong together.

use leaf_bot::CommandKey;
use leaf_core::config::Tier1Config;
use serde::Deserialize;

use super::config::RECONFIGURE;
use super::net::{Answer, Auth, Fetch, Request, Unanswered};
use super::report::{Check, Finding, Redactor};

/// The `EMBEDDED` application flag: set while Activities is enabled.
const FLAG_EMBEDDED: u64 = 1 << 17;
/// Discord's command type for the Entry Point command of an Activity.
const KIND_ENTRY_POINT: u8 = 4;
/// Longest application or command name quoted.
const NAME_MAX_CHARS: usize = 60;
/// Longest ID quoted (a Discord ID has at most 20 digits).
const ID_MAX_CHARS: usize = 24;
/// Most command names listed in one finding.
const NAMES_LISTED_MAX: usize = 6;
/// A warning is raised when fewer than one in this many session starts
/// remain.
const SESSION_STARTS_LOW_DIVISOR: u64 = 10;

const RESET_TOKEN: &str = "Reset the token in the Developer Portal (Bot → Reset Token) and enter \
    the new one with --reconfigure (guide/01-install.md, “Changing credentials later”).";
const CHECK_NETWORK: &str = "Check that this machine can reach discord.com (DNS, firewall, \
    proxy), then run the doctor again.";
const TRY_LATER: &str = "Run the doctor again in a few minutes; https://discordstatus.com shows \
    whether Discord is having trouble.";
const RESTART_TO_REGISTER: &str = "Restart leaf (docker compose restart leaf): it registers its \
    commands each time the bot connects. If this stays, the log line starting “registering \
    commands failed” has Discord's reason (guide/07-troubleshooting.md, “Bot offline”).";

/// The part of `GET /users/@me` that is reported.
#[derive(Debug, Deserialize)]
struct BotUser {
    #[serde(default)]
    username: String,
}

/// The part of `GET /applications/@me` the checks need.
#[derive(Debug, Deserialize)]
struct ApplicationBody {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    flags: Option<u64>,
}

/// The application the bot token belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    /// Its id, which is also the id its command lists are filed under.
    pub id: String,
    /// Whether Activities is enabled for it.
    pub activities: bool,
}

impl Application {
    fn of(body: ApplicationBody) -> Self {
        Self {
            activities: body.flags.unwrap_or(0) & FLAG_EMBEDDED != 0,
            id: body.id,
        }
    }
}

/// Whether `id` can be a Discord id: digits only. An application's id goes
/// into the address of its command lists, so nothing else is accepted.
fn is_snowflake(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// What the identity check found, and what the checks after it need.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Identity {
    /// The findings, for the `discord` check.
    pub findings: Vec<Finding>,
    /// The token's application, when Discord named it.
    pub application: Option<Application>,
    /// Why nothing more can be asked with this token, when that is so.
    pub blocked: Option<Blocked>,
}

/// Why the calls that need the bot token cannot be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Discord refused the token.
    TokenRefused,
    /// Discord gave no answer at all.
    Unreachable,
}

impl Blocked {
    /// The reason, as the middle of a sentence.
    pub const fn why(self) -> &'static str {
        match self {
            Self::TokenRefused => "Discord does not accept the bot token",
            Self::Unreachable => "Discord could not be reached",
        }
    }
}

/// Where the commands are registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// In the application's global list (a real install).
    Global,
    /// In one server's list (`DEV_GUILD_ID`).
    Guild(u64),
}

/// One command in a list Discord returned.
#[derive(Debug, Deserialize)]
struct Remote {
    #[serde(rename = "type", default = "slash_kind")]
    kind: u8,
    name: String,
}

/// Discord's type for a command listed without one.
const fn slash_kind() -> u8 {
    1
}

/// The part of `GET /gateway/bot` that is checked.
#[derive(Debug, Deserialize)]
struct GatewayBody {
    session_start_limit: SessionStartLimit,
}

#[derive(Debug, Deserialize)]
struct SessionStartLimit {
    total: u64,
    remaining: u64,
    /// Milliseconds until the limit resets.
    #[serde(default)]
    reset_after: u64,
}

/// The bot's side of Discord's REST API.
struct Bot<'a, F> {
    fetch: &'a F,
    api: &'a str,
    token: &'a str,
}

/// Why a call gave nothing to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trouble {
    /// No HTTP answer.
    Unanswered(Unanswered),
    /// An answer that is not a success.
    Status(u16),
    /// A success whose body is not what Discord documents.
    Unreadable,
}

impl<F: Fetch> Bot<'_, F> {
    /// `GET <api><path>` as the bot, read as `T`.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, Trouble> {
        let request =
            Request::get(format!("{}{path}", self.api)).with_auth(Auth::Bot(self.token.to_owned()));
        let answer = self
            .fetch
            .send(request)
            .await
            .map_err(Trouble::Unanswered)?;
        read(&answer)
    }

    /// `GET /applications/@me`: the application the token belongs to.
    async fn application(&self) -> Result<ApplicationBody, Trouble> {
        let app: ApplicationBody = self.get("/applications/@me").await?;
        if is_snowflake(&app.id) {
            Ok(app)
        } else {
            Err(Trouble::Unreadable)
        }
    }
}

/// A successful answer's body as `T`.
fn read<T: serde::de::DeserializeOwned>(answer: &Answer) -> Result<T, Trouble> {
    if !(200..300).contains(&answer.status) {
        return Err(Trouble::Status(answer.status));
    }
    answer.json().ok_or(Trouble::Unreadable)
}

/// A failed call as a finding. `asked` names the call (`GET /users/@me`).
fn troubled(check: Check, asked: &str, trouble: Trouble) -> Finding {
    match trouble {
        Trouble::Unanswered(Unanswered::TimedOut) => Finding::fail(
            check,
            format!("Discord did not answer {asked} within 10 seconds."),
            TRY_LATER,
        ),
        Trouble::Unanswered(Unanswered::Unreachable) => Finding::fail(
            check,
            format!("leaf could not reach Discord ({asked})."),
            CHECK_NETWORK,
        ),
        Trouble::Unanswered(Unanswered::Unsendable) => Finding::fail(
            check,
            "The bot token in leaf.conf holds characters a token never has, so it cannot be sent.",
            RESET_TOKEN,
        ),
        Trouble::Status(401) => Finding::fail(
            check,
            "Discord does not accept the bot token in leaf.conf.",
            RESET_TOKEN,
        ),
        Trouble::Status(429) => Finding::fail(
            check,
            format!("Discord is rate limiting this bot (HTTP 429), so {asked} went unanswered."),
            TRY_LATER,
        ),
        Trouble::Status(status) => Finding::fail(
            check,
            format!("Discord answered {asked} with HTTP {status}."),
            TRY_LATER,
        ),
        Trouble::Unreadable => Finding::fail(
            check,
            format!("Discord's answer to {asked} was not in the shape leaf expects."),
            "Run the doctor again; if it stays, update leaf.",
        ),
    }
}

/// Why a trouble stops every later call with the token, if it does.
const fn blocks(trouble: Trouble) -> Option<Blocked> {
    match trouble {
        Trouble::Status(401) | Trouble::Unanswered(Unanswered::Unsendable) => {
            Some(Blocked::TokenRefused)
        }
        Trouble::Unanswered(Unanswered::TimedOut | Unanswered::Unreachable) => {
            Some(Blocked::Unreachable)
        }
        Trouble::Status(_) | Trouble::Unreadable => None,
    }
}

/// An application or command name as it is quoted.
fn named(name: &str, redactor: &Redactor) -> String {
    redactor.quoted_up_to(name, NAME_MAX_CHARS)
}

/// Whether two snowflakes are the same number; `None` when either is not
/// a number, so nothing can be concluded.
fn same_id(a: &str, b: &str) -> Option<bool> {
    Some(a.trim().parse::<u64>().ok()? == b.trim().parse::<u64>().ok()?)
}

/// Check 2: the bot token is accepted, it belongs to the configured
/// application, and the client id and secret are accepted together.
pub async fn identity(
    fetch: &impl Fetch,
    api: &str,
    config: &Tier1Config,
    redactor: &Redactor,
) -> Identity {
    let bot = Bot {
        fetch,
        api,
        token: &config.discord_token,
    };
    let mut identity = Identity::default();

    match bot.get::<BotUser>("/users/@me").await {
        Ok(user) => identity.findings.push(Finding::ok(
            Check::Discord,
            format!(
                "Discord accepts the bot token (bot user “{}”).",
                named(&user.username, redactor)
            ),
        )),
        Err(trouble) => {
            identity
                .findings
                .push(troubled(Check::Discord, "GET /users/@me", trouble));
            identity.blocked = blocks(trouble);
        }
    }

    // A token Discord refused (or a Discord that is not there) fails the
    // next call the same way: one finding says it.
    if identity.blocked.is_none() {
        match bot.application().await {
            Ok(app) => {
                identity
                    .findings
                    .push(application_matches(&app, &config.client_id, redactor));
                identity.application = Some(Application::of(app));
            }
            Err(trouble) => {
                identity
                    .findings
                    .push(troubled(Check::Discord, "GET /applications/@me", trouble));
                identity.blocked = blocks(trouble);
            }
        }
    }

    // The secret is checked whatever became of the token: a bad token must
    // not hide a bad secret. Not when Discord is unreachable, though.
    if identity.blocked != Some(Blocked::Unreachable) {
        identity
            .findings
            .push(client_pair(fetch, api, config).await);
    }
    identity
}

/// The token's application, for the commands check when the identity check
/// was not run. `Err` is the finding that says why Discord did not name it.
pub async fn application(
    fetch: &impl Fetch,
    api: &str,
    token: &str,
) -> Result<Application, Finding> {
    let bot = Bot { fetch, api, token };
    bot.application()
        .await
        .map(Application::of)
        .map_err(|trouble| troubled(Check::Commands, "GET /applications/@me", trouble))
}

/// Whether the token's application is the configured one.
fn application_matches(app: &ApplicationBody, client_id: &str, redactor: &Redactor) -> Finding {
    let name = named(app.name.as_deref().unwrap_or_default(), redactor);
    let id = redactor.quoted_up_to(&app.id, ID_MAX_CHARS);
    let configured = redactor.quoted_up_to(client_id, ID_MAX_CHARS);
    match same_id(&app.id, client_id) {
        Some(true) => Finding::ok(
            Check::Discord,
            format!(
                "The token belongs to application “{name}” (ID {id}), the client ID in leaf.conf."
            ),
        ),
        Some(false) => Finding::fail(
            Check::Discord,
            format!(
                "The bot token belongs to application “{name}” (ID {id}), but the client ID in \
                 leaf.conf is {configured}."
            ),
            format!(
                "The bot token, client ID and client secret must come from one application. \
                 {RECONFIGURE}"
            ),
        ),
        None => Finding::warn(
            Check::Discord,
            format!(
                "The token's application (ID {id}) could not be compared with the client ID in \
                 leaf.conf ({configured}): one of them is not a number."
            ),
            RECONFIGURE,
        ),
    }
}

/// The client-credentials grant succeeds only for a client id and secret
/// that belong together: the pair every sign-in is exchanged with.
async fn client_pair(fetch: &impl Fetch, api: &str, config: &Tier1Config) -> Finding {
    let request = Request::get(format!("{api}/oauth2/token"))
        .with_auth(Auth::Basic {
            user: config.client_id.clone(),
            password: config.client_secret.clone(),
        })
        .with_form(vec![
            ("grant_type", "client_credentials"),
            ("scope", "identify"),
        ]);
    let asked = "POST /oauth2/token";
    match fetch.send(request).await {
        Ok(answer) if (200..300).contains(&answer.status) => Finding::ok(
            Check::Discord,
            "Discord accepts the client ID and client secret together, so sign-in can work.",
        ),
        Ok(answer) if answer.status == 400 || answer.status == 401 => Finding::fail(
            Check::Discord,
            "Discord does not accept the client ID and client secret in leaf.conf together, so \
             nobody can sign in to the gallery or the admin panel.",
            "Reset the secret in the Developer Portal (OAuth2 → Reset Secret) and enter the new \
             one with --reconfigure (guide/01-install.md, “Changing credentials later”).",
        ),
        Ok(answer) => troubled(Check::Discord, asked, Trouble::Status(answer.status)),
        // This request carries no bot token, so the sentence about one
        // that cannot be sent would be wrong here.
        Err(Unanswered::Unsendable) => Finding::fail(
            Check::Discord,
            "The request that checks the client ID and client secret could not be put together.",
            RECONFIGURE,
        ),
        Err(unanswered) => troubled(Check::Discord, asked, Trouble::Unanswered(unanswered)),
    }
}

/// A command as a person knows it: `/name` or the menu entry's name.
fn described(key: &CommandKey, redactor: &Redactor) -> String {
    let name = named(&key.name, redactor);
    match key.kind {
        1 => format!("/{name}"),
        2 => format!("“{name}” (user menu)"),
        3 => format!("“{name}” (message menu)"),
        KIND_ENTRY_POINT => format!("“{name}” (Entry Point)"),
        kind => format!("“{name}” (type {kind})"),
    }
}

/// Some commands by name: the first few, then how many more.
fn listed(keys: &[CommandKey], redactor: &Redactor) -> String {
    let mut names: Vec<String> = keys
        .iter()
        .take(NAMES_LISTED_MAX)
        .map(|key| described(key, redactor))
        .collect();
    if keys.len() > NAMES_LISTED_MAX {
        names.push(format!("and {} more", keys.len() - NAMES_LISTED_MAX));
    }
    names.join(", ")
}

/// `n command` or `n commands`.
fn count(n: usize) -> String {
    if n == 1 {
        "1 command".to_owned()
    } else {
        format!("{n} commands")
    }
}

/// The keys of a list Discord returned, sorted, without duplicates.
fn keys(remote: Vec<Remote>) -> Vec<CommandKey> {
    let mut keys: Vec<CommandKey> = remote
        .into_iter()
        .map(|c| CommandKey {
            kind: c.kind,
            name: c.name,
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Check 3: every command leaf registers is listed where leaf registers it,
/// nothing is listed that leaf no longer has, and the Activity's Entry Point
/// command is there.
///
/// `local` is the list registration submits (`leaf_bot::registered_commands`).
pub async fn commands(
    fetch: &impl Fetch,
    api: &str,
    token: &str,
    app: &Application,
    scope: Scope,
    local: &[CommandKey],
    redactor: &Redactor,
) -> Vec<Finding> {
    let bot = Bot { fetch, api, token };
    let app_id = &app.id;
    let global_path = format!("/applications/{app_id}/commands");
    let global = match bot.get::<Vec<Remote>>(&global_path).await {
        Ok(global) => keys(global),
        Err(trouble) => {
            let asked = format!("GET {global_path}");
            return vec![troubled(Check::Commands, &asked, trouble)];
        }
    };
    let (entry_points, global): (Vec<_>, Vec<_>) = global
        .into_iter()
        .partition(|key| key.kind == KIND_ENTRY_POINT);

    let mut findings = Vec::new();
    match scope {
        Scope::Global => {
            findings.extend(registration(local, &global, "globally", redactor));
        }
        Scope::Guild(guild) => {
            let path = format!("/applications/{app_id}/guilds/{guild}/commands");
            match bot.get::<Vec<Remote>>(&path).await {
                Ok(listed) => {
                    let place = format!("in server {guild} (DEV_GUILD_ID)");
                    findings.extend(registration(local, &keys(listed), &place, redactor));
                }
                Err(Trouble::Status(status @ (403 | 404))) => findings.push(Finding::fail(
                    Check::Commands,
                    format!(
                        "Discord would not list the commands of server {guild} (HTTP {status}): \
                         the bot is not in that server, or DEV_GUILD_ID is not a server's ID."
                    ),
                    "Invite the bot to that server, correct DEV_GUILD_ID, or remove it so the \
                     commands register globally (guide/01-install.md, “Command registration and \
                     DEV_GUILD_ID”).",
                )),
                Err(trouble) => {
                    findings.push(troubled(Check::Commands, &format!("GET {path}"), trouble));
                }
            }
            if !global.is_empty() {
                findings.push(Finding::warn(
                    Check::Commands,
                    format!(
                        "The global list still holds {} from a start without DEV_GUILD_ID ({}), \
                         so they show twice in that server.",
                        count(global.len()),
                        listed(&global, redactor)
                    ),
                    RESTART_TO_REGISTER,
                ));
            }
        }
    }
    findings.push(entry_point(&entry_points, app.activities, redactor));
    findings
}

/// Compares what leaf registers with what Discord lists in one place.
fn registration(
    local: &[CommandKey],
    listed_there: &[CommandKey],
    place: &str,
    redactor: &Redactor,
) -> Vec<Finding> {
    let missing: Vec<CommandKey> = local
        .iter()
        .filter(|key| !listed_there.contains(key))
        .cloned()
        .collect();
    let stale: Vec<CommandKey> = listed_there
        .iter()
        .filter(|key| !local.contains(key))
        .cloned()
        .collect();
    let mut findings = Vec::new();
    if !missing.is_empty() {
        findings.push(Finding::fail(
            Check::Commands,
            format!(
                "{} of leaf's {} {} not registered {place}: {}.",
                missing.len(),
                count(local.len()),
                if missing.len() == 1 { "is" } else { "are" },
                listed(&missing, redactor)
            ),
            RESTART_TO_REGISTER,
        ));
    }
    if !stale.is_empty() {
        findings.push(Finding::fail(
            Check::Commands,
            format!(
                "Discord lists {} {place} that leaf no longer has: {}.",
                count(stale.len()),
                listed(&stale, redactor)
            ),
            RESTART_TO_REGISTER,
        ));
    }
    if findings.is_empty() {
        findings.push(Finding::ok(
            Check::Commands,
            format!(
                "All {} leaf registers are listed {place}, and nothing else is.",
                count(local.len())
            ),
        ));
    }
    findings
}

/// The Entry Point command: what puts the gallery in Discord's app launcher.
fn entry_point(entry_points: &[CommandKey], activities: bool, redactor: &Redactor) -> Finding {
    match (entry_points.first(), activities) {
        (Some(command), _) => Finding::ok(
            Check::Commands,
            format!(
                "The Activity's Entry Point command (“{}”) is registered.",
                named(&command.name, redactor)
            ),
        ),
        (None, true) => Finding::fail(
            Check::Commands,
            "Activities is enabled for this application, but its Entry Point command is gone, \
             so leaf is not in Discord's app launcher.",
            "Create the command again as Discord's guide “Setting Up an Entry Point Command” \
             describes (guide/07-troubleshooting.md, “Commands”, has the request).",
        ),
        (None, false) => Finding::warn(
            Check::Commands,
            "This application has no Entry Point command, and Discord does not list it as having \
             Activities enabled: the gallery cannot open inside Discord.",
            "Enable Activities in the Developer Portal (guide/02-discord.md, “Activities: the \
             gallery”).",
        ),
    }
}

/// A wait in the largest unit that says enough.
fn wait_text(millis: u64) -> String {
    let minutes = millis / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "under a minute".to_owned(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Check 4: Discord hands out a gateway address, and the bot may still
/// start sessions (each connection uses one of a daily allowance).
pub async fn gateway(fetch: &impl Fetch, api: &str, token: &str) -> Vec<Finding> {
    let bot = Bot { fetch, api, token };
    let limit = match bot.get::<GatewayBody>("/gateway/bot").await {
        Ok(body) => body.session_start_limit,
        Err(trouble) => return vec![troubled(Check::Gateway, "GET /gateway/bot", trouble)],
    };
    let SessionStartLimit {
        total,
        remaining,
        reset_after,
    } = limit;
    let reset = wait_text(reset_after);
    let finding = if remaining == 0 {
        Finding::fail(
            Check::Gateway,
            format!(
                "Discord allows this bot no more gateway connections for now: 0 of {total} \
                 session starts remain, and the allowance resets in {reset}."
            ),
            "Wait for the reset. Each connection uses one start, so find what made leaf \
             reconnect so often (docker compose logs leaf).",
        )
    } else if remaining.saturating_mul(SESSION_STARTS_LOW_DIVISOR) < total {
        Finding::warn(
            Check::Gateway,
            format!(
                "Only {remaining} of {total} gateway session starts remain (the allowance resets \
                 in {reset})."
            ),
            "Look for a reconnect loop in the log (docker compose logs leaf) before the \
             allowance runs out.",
        )
    } else {
        Finding::ok(
            Check::Gateway,
            format!("Discord's gateway answers, and {remaining} of {total} session starts remain."),
        )
    };
    vec![finding]
}

/// What a check that needs the token says when it was not run. `why` is
/// leaf's own wording (the middle of a sentence), never text from outside.
pub fn not_asked(check: Check, why: &'static str) -> Finding {
    Finding::skip(check, format!("Not checked: {why}."))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use super::*;
    use crate::doctor::report::Status;
    use crate::doctor::testing::{
        API, Canned, SECRETS, assert_no_secrets, config, printed, redactor, statuses,
    };

    const USER: &str = r#"{"id":"999","username":"leaf","bot":true}"#;
    /// Application 123 with Activities enabled (`1 << 17`) and another flag.
    const APP: &str = r#"{"id":"123","name":"leaf","flags":8519680}"#;
    const TOKEN_GRANT: &str = r#"{"access_token":"SECRET_GRANTED","token_type":"Bearer"}"#;
    const GATEWAY: &str = r#"{"url":"wss://gateway.discord.gg","shards":1,
        "session_start_limit":{"total":1000,"remaining":998,"reset_after":14400000,
        "max_concurrency":1}}"#;

    fn url(path: &str) -> String {
        format!("{API}{path}")
    }

    /// A Discord where everything about application 123 is in order.
    fn healthy() -> Canned {
        Canned::new()
            .json(&url("/users/@me"), 200, USER)
            .json(&url("/applications/@me"), 200, APP)
            .json(&url("/oauth2/token"), 200, TOKEN_GRANT)
            .json(&url("/gateway/bot"), 200, GATEWAY)
    }

    fn key(kind: u8, name: &str) -> CommandKey {
        CommandKey {
            kind,
            name: name.to_owned(),
        }
    }

    /// What leaf registers in these tests.
    fn local() -> Vec<CommandKey> {
        vec![
            key(1, "gallery"),
            key(1, "setup"),
            key(3, "Archive to Series"),
        ]
    }

    const LISTED: &str = r#"[
        {"id":"1","type":1,"name":"gallery","description":"x"},
        {"id":"2","name":"setup","description":"x"},
        {"id":"3","type":3,"name":"Archive to Series"}
    ]"#;
    const LISTED_WITH_ENTRY_POINT: &str = r#"[
        {"id":"1","type":1,"name":"gallery","description":"x"},
        {"id":"2","type":1,"name":"setup","description":"x"},
        {"id":"3","type":3,"name":"Archive to Series"},
        {"id":"4","type":4,"name":"launch","description":"Launch leaf","handler":2}
    ]"#;
    const ONLY_ENTRY_POINT: &str =
        r#"[{"id":"4","type":4,"name":"launch","description":"Launch leaf","handler":2}]"#;

    fn app(activities: bool) -> Application {
        Application {
            id: "123".to_owned(),
            activities,
        }
    }

    // ---- identity ----

    #[tokio::test]
    async fn a_good_token_application_and_secret_are_three_oks() {
        let discord = healthy();
        let identity = identity(&discord, API, &config(), &redactor()).await;
        assert_eq!(
            statuses(&identity.findings),
            [Status::Ok, Status::Ok, Status::Ok]
        );
        assert_eq!(identity.application, Some(app(true)));
        assert_eq!(identity.blocked, None);
        assert!(identity.findings[0].message.contains("bot user “leaf”"));
        assert!(
            identity.findings[1]
                .message
                .contains("application “leaf” (ID 123)")
        );

        // Each call went out once, with the credential it needs and no
        // other.
        let requests = discord.requests();
        assert_eq!(
            discord.asked(),
            [
                url("/users/@me"),
                url("/applications/@me"),
                url("/oauth2/token")
            ]
        );
        assert_eq!(requests[0].auth, Auth::Bot("SECRET_TOKEN_AAA".to_owned()));
        assert_eq!(requests[0].form, None);
        assert_eq!(requests[1].auth, Auth::Bot("SECRET_TOKEN_AAA".to_owned()));
        assert_eq!(
            requests[2].auth,
            Auth::Basic {
                user: "123".to_owned(),
                password: "SECRET_CLIENT_BBB".to_owned()
            }
        );
        assert_eq!(
            requests[2].form,
            Some(vec![
                ("grant_type", "client_credentials"),
                ("scope", "identify")
            ])
        );
        assert_no_secrets(&identity.findings);
    }

    #[tokio::test]
    async fn a_refused_token_fails_once_and_blocks_what_needs_it() {
        let discord = healthy()
            .json(
                &url("/users/@me"),
                401,
                r#"{"message":"401: Unauthorized"}"#,
            )
            .json(&url("/applications/@me"), 401, "{}");
        let identity = identity(&discord, API, &config(), &redactor()).await;
        // The token once, and the secret still checked.
        assert_eq!(statuses(&identity.findings), [Status::Fail, Status::Ok]);
        assert_eq!(
            identity.findings[0].message,
            "Discord does not accept the bot token in leaf.conf."
        );
        assert!(
            identity.findings[0]
                .next
                .as_deref()
                .unwrap()
                .contains("--reconfigure")
        );
        assert_eq!(identity.application, None);
        assert_eq!(identity.blocked, Some(Blocked::TokenRefused));
        // The application was not asked for with a token already refused.
        assert_eq!(discord.asked(), [url("/users/@me"), url("/oauth2/token")]);
    }

    #[tokio::test]
    async fn a_bad_token_does_not_hide_a_bad_secret() {
        let discord = healthy().json(&url("/users/@me"), 401, "{}").json(
            &url("/oauth2/token"),
            401,
            r#"{"error":"invalid_client"}"#,
        );
        let identity = identity(&discord, API, &config(), &redactor()).await;
        assert_eq!(statuses(&identity.findings), [Status::Fail, Status::Fail]);
        assert!(identity.findings[1].message.contains("client secret"));
        assert!(
            identity.findings[1]
                .next
                .as_deref()
                .unwrap()
                .contains("Reset Secret")
        );
    }

    #[tokio::test]
    async fn a_rejected_secret_fails_whichever_status_says_so() {
        for status in [400, 401] {
            let discord = healthy().json(&url("/oauth2/token"), status, "{}");
            let identity = identity(&discord, API, &config(), &redactor()).await;
            assert_eq!(
                statuses(&identity.findings),
                [Status::Ok, Status::Ok, Status::Fail],
                "{status}"
            );
            // The token still works: nothing is blocked.
            assert_eq!(identity.blocked, None);
            assert!(identity.application.is_some());
        }
    }

    #[tokio::test]
    async fn a_token_from_another_application_fails_and_names_both() {
        let mut config = config();
        config.client_id = "456".to_owned();
        let identity = identity(&healthy(), API, &config, &redactor()).await;
        assert_eq!(
            statuses(&identity.findings),
            [Status::Ok, Status::Fail, Status::Ok]
        );
        let message = &identity.findings[1].message;
        assert!(message.contains("application “leaf” (ID 123)"), "{message}");
        assert!(message.contains("leaf.conf is 456"), "{message}");
        // The commands are filed under the token's application.
        assert_eq!(identity.application, Some(app(true)));
    }

    #[tokio::test]
    async fn ids_that_are_not_numbers_cannot_be_compared() {
        let mut config = config();
        config.client_id = "abc".to_owned();
        let identity = identity(&healthy(), API, &config, &redactor()).await;
        assert_eq!(identity.findings[1].status, Status::Warn);

        assert_eq!(same_id("123", " 0123 "), Some(true));
        assert_eq!(same_id("123", "456"), Some(false));
        assert_eq!(same_id("", "123"), None);
    }

    #[tokio::test]
    async fn an_unreachable_discord_is_one_failure_not_three() {
        for (why, says) in [
            (Unanswered::Unreachable, "could not reach Discord"),
            (Unanswered::TimedOut, "within 10 seconds"),
        ] {
            let discord = healthy().unanswered(&url("/users/@me"), why);
            let identity = identity(&discord, API, &config(), &redactor()).await;
            assert_eq!(statuses(&identity.findings), [Status::Fail], "{why:?}");
            assert!(identity.findings[0].message.contains(says), "{why:?}");
            assert_eq!(identity.blocked, Some(Blocked::Unreachable));
            assert_eq!(discord.asked(), [url("/users/@me")]);
        }
    }

    #[tokio::test]
    async fn a_token_that_cannot_be_sent_is_a_token_problem() {
        let discord = healthy().unanswered(&url("/users/@me"), Unanswered::Unsendable);
        let identity = identity(&discord, API, &config(), &redactor()).await;
        assert_eq!(identity.findings[0].status, Status::Fail);
        assert!(
            identity.findings[0]
                .message
                .contains("characters a token never has")
        );
        assert_eq!(identity.blocked, Some(Blocked::TokenRefused));
    }

    #[tokio::test]
    async fn an_outage_or_a_rate_limit_fails_without_blaming_the_token() {
        for (status, says) in [(503, "HTTP 503"), (429, "rate limiting")] {
            let discord = healthy().json(&url("/users/@me"), status, "{}");
            let identity = identity(&discord, API, &config(), &redactor()).await;
            assert_eq!(identity.findings[0].status, Status::Fail, "{status}");
            assert!(identity.findings[0].message.contains(says), "{status}");
            assert!(!identity.findings[0].message.contains("token"), "{status}");
            // Discord did answer: the other calls are still made.
            assert_eq!(identity.blocked, None, "{status}");
            assert_eq!(identity.findings.len(), 3, "{status}");
        }
    }

    #[tokio::test]
    async fn an_answer_in_another_shape_fails_as_unreadable() {
        let discord = healthy().json(&url("/applications/@me"), 200, "<html>");
        let identity = identity(&discord, API, &config(), &redactor()).await;
        assert_eq!(
            statuses(&identity.findings),
            [Status::Ok, Status::Fail, Status::Ok]
        );
        assert!(identity.findings[1].message.contains("shape leaf expects"));
        assert_eq!(identity.application, None);
    }

    #[tokio::test]
    async fn an_application_id_that_is_not_a_number_is_not_put_in_an_address() {
        for id in ["../../users/@me", "12 3", "", "123456789012345678901"] {
            let body = format!(r#"{{"id":"{id}","name":"leaf","flags":0}}"#);
            let discord = healthy().json(&url("/applications/@me"), 200, &body);
            let identity = identity(&discord, API, &config(), &redactor()).await;
            assert_eq!(identity.application, None, "{id}");
            assert_eq!(identity.findings[1].status, Status::Fail, "{id}");
            assert!(
                application(&discord, API, "SECRET_TOKEN_AAA")
                    .await
                    .is_err(),
                "{id}"
            );
        }
        assert!(is_snowflake("800000000000000001"));
    }

    #[tokio::test]
    async fn activities_is_read_from_the_embedded_flag() {
        for (flags, activities) in [("131072", true), ("8388608", false), ("0", false)] {
            let body = format!(r#"{{"id":"123","name":"leaf","flags":{flags}}}"#);
            let discord = healthy().json(&url("/applications/@me"), 200, &body);
            let identity = identity(&discord, API, &config(), &redactor()).await;
            assert_eq!(identity.application, Some(app(activities)), "{flags}");
        }
        // No flags at all reads as none set.
        let discord = healthy().json(&url("/applications/@me"), 200, r#"{"id":"123"}"#);
        let identity = identity(&discord, API, &config(), &redactor()).await;
        assert_eq!(identity.application, Some(app(false)));
    }

    #[tokio::test]
    async fn what_discord_sends_back_never_puts_a_credential_in_the_output() {
        // A Discord that repeats what it was sent, and a granted token.
        let echo = r#"{"id":"123","name":"SECRET_TOKEN_AAA\nSECRET_CLIENT_BBB","flags":0}"#;
        let discord = healthy()
            .json(
                &url("/users/@me"),
                200,
                r#"{"id":"9","username":"SECRET_TOKEN_AAA"}"#,
            )
            .json(&url("/applications/@me"), 200, echo);
        let identity = identity(&discord, API, &config(), &redactor()).await;
        let printed = printed(&identity.findings);
        for secret in SECRETS {
            assert!(!printed.contains(secret), "{secret} in {printed}");
        }
        assert!(!printed.contains("SECRET_GRANTED"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }

    // ---- commands ----

    async fn check_commands(discord: &Canned, scope: Scope, activities: bool) -> Vec<Finding> {
        commands(
            discord,
            API,
            "SECRET_TOKEN_AAA",
            &app(activities),
            scope,
            &local(),
            &redactor(),
        )
        .await
    }

    #[tokio::test]
    async fn commands_registered_globally_with_the_entry_point_are_ok() {
        let discord = Canned::new().json(
            &url("/applications/123/commands"),
            200,
            LISTED_WITH_ENTRY_POINT,
        );
        let findings = check_commands(&discord, Scope::Global, true).await;
        assert_eq!(statuses(&findings), [Status::Ok, Status::Ok]);
        assert_eq!(
            findings[0].message,
            "All 3 commands leaf registers are listed globally, and nothing else is."
        );
        assert!(findings[1].message.contains("“launch”"));
        assert_eq!(
            discord.requests()[0].auth,
            Auth::Bot("SECRET_TOKEN_AAA".to_owned())
        );
        assert_no_secrets(&findings);
    }

    #[tokio::test]
    async fn a_missing_command_fails_and_is_named() {
        let listed = r#"[{"id":"1","type":1,"name":"gallery"},
                         {"id":"4","type":4,"name":"launch"}]"#;
        let discord = Canned::new().json(&url("/applications/123/commands"), 200, listed);
        let findings = check_commands(&discord, Scope::Global, true).await;
        assert_eq!(statuses(&findings), [Status::Fail, Status::Ok]);
        assert_eq!(
            findings[0].message,
            "2 of leaf's 3 commands are not registered globally: /setup, “Archive to Series” \
             (message menu)."
        );
        assert!(
            findings[0]
                .next
                .as_deref()
                .unwrap()
                .contains("Restart leaf")
        );
    }

    #[tokio::test]
    async fn a_command_of_the_same_name_and_another_type_does_not_count() {
        // A slash command called like the message menu is not the menu.
        let listed = r#"[{"id":"1","type":1,"name":"gallery"},{"id":"2","type":1,"name":"setup"},
                         {"id":"3","type":1,"name":"Archive to Series"},
                         {"id":"4","type":4,"name":"launch"}]"#;
        let discord = Canned::new().json(&url("/applications/123/commands"), 200, listed);
        let findings = check_commands(&discord, Scope::Global, true).await;
        assert_eq!(
            statuses(&findings),
            [Status::Fail, Status::Fail, Status::Ok]
        );
        assert!(
            findings[0]
                .message
                .contains("“Archive to Series” (message menu)")
        );
        assert!(findings[1].message.contains("/Archive to Series"));
    }

    #[tokio::test]
    async fn a_stale_command_fails_and_is_named() {
        let listed = r#"[{"id":"1","type":1,"name":"gallery"},{"id":"2","type":1,"name":"setup"},
                         {"id":"3","type":3,"name":"Archive to Series"},
                         {"id":"5","type":1,"name":"oldcommand"},
                         {"id":"6","type":2,"name":"Old Menu"},
                         {"id":"4","type":4,"name":"launch"}]"#;
        let discord = Canned::new().json(&url("/applications/123/commands"), 200, listed);
        let findings = check_commands(&discord, Scope::Global, true).await;
        assert_eq!(statuses(&findings), [Status::Fail, Status::Ok]);
        assert_eq!(
            findings[0].message,
            "Discord lists 2 commands globally that leaf no longer has: /oldcommand, “Old Menu” \
             (user menu)."
        );
    }

    #[tokio::test]
    async fn a_missing_entry_point_fails_only_when_activities_is_enabled() {
        let discord = Canned::new().json(&url("/applications/123/commands"), 200, LISTED);
        let findings = check_commands(&discord, Scope::Global, true).await;
        assert_eq!(statuses(&findings), [Status::Ok, Status::Fail]);
        assert!(findings[1].message.contains("Entry Point command is gone"));

        let findings = check_commands(&discord, Scope::Global, false).await;
        assert_eq!(statuses(&findings), [Status::Ok, Status::Warn]);
        assert!(findings[1].message.contains("Activities enabled"));
    }

    #[tokio::test]
    async fn the_entry_point_is_never_counted_as_stale() {
        let discord = Canned::new().json(
            &url("/applications/123/commands"),
            200,
            LISTED_WITH_ENTRY_POINT,
        );
        let findings = check_commands(&discord, Scope::Global, false).await;
        // Listed although the flag says otherwise: the command is what counts.
        assert_eq!(statuses(&findings), [Status::Ok, Status::Ok]);
    }

    #[tokio::test]
    async fn with_a_dev_guild_the_guild_list_is_the_one_compared() {
        let discord = Canned::new()
            .json(&url("/applications/123/commands"), 200, ONLY_ENTRY_POINT)
            .json(&url("/applications/123/guilds/77/commands"), 200, LISTED);
        let findings = check_commands(&discord, Scope::Guild(77), true).await;
        assert_eq!(statuses(&findings), [Status::Ok, Status::Ok]);
        assert_eq!(
            findings[0].message,
            "All 3 commands leaf registers are listed in server 77 (DEV_GUILD_ID), and nothing \
             else is."
        );
        assert_eq!(
            discord.asked(),
            [
                url("/applications/123/commands"),
                url("/applications/123/guilds/77/commands")
            ]
        );
    }

    #[tokio::test]
    async fn with_a_dev_guild_missing_and_leftover_commands_are_reported() {
        // The guild lacks a command; the global list still has the old set.
        let guild = r#"[{"id":"1","type":1,"name":"gallery"},{"id":"2","type":1,"name":"setup"}]"#;
        let discord = Canned::new()
            .json(
                &url("/applications/123/commands"),
                200,
                LISTED_WITH_ENTRY_POINT,
            )
            .json(&url("/applications/123/guilds/77/commands"), 200, guild);
        let findings = check_commands(&discord, Scope::Guild(77), true).await;
        assert_eq!(
            statuses(&findings),
            [Status::Fail, Status::Warn, Status::Ok]
        );
        assert!(findings[0].message.contains("not registered in server 77"));
        assert!(
            findings[1]
                .message
                .contains("global list still holds 3 commands")
        );
    }

    #[tokio::test]
    async fn a_dev_guild_the_bot_is_not_in_fails_with_the_way_out() {
        for status in [403, 404] {
            let discord = Canned::new()
                .json(&url("/applications/123/commands"), 200, ONLY_ENTRY_POINT)
                .json(&url("/applications/123/guilds/77/commands"), status, "{}");
            let findings = check_commands(&discord, Scope::Guild(77), true).await;
            assert_eq!(statuses(&findings), [Status::Fail, Status::Ok], "{status}");
            assert!(findings[0].message.contains("server 77"), "{status}");
            assert!(
                findings[0]
                    .next
                    .as_deref()
                    .unwrap()
                    .contains("DEV_GUILD_ID"),
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn a_command_list_discord_will_not_give_fails() {
        // (answer, what the finding says)
        let unreadable = Canned::new().json(&url("/applications/123/commands"), 200, "{}");
        let refused = Canned::new().json(&url("/applications/123/commands"), 401, "{}");
        let down = Canned::new().json(&url("/applications/123/commands"), 502, "{}");
        let gone =
            Canned::new().unanswered(&url("/applications/123/commands"), Unanswered::Unreachable);
        for (discord, says) in [
            (unreadable, "shape leaf expects"),
            (refused, "does not accept the bot token"),
            (down, "HTTP 502"),
            (gone, "could not reach Discord"),
        ] {
            let findings = check_commands(&discord, Scope::Global, true).await;
            assert_eq!(statuses(&findings), [Status::Fail], "{says}");
            assert!(findings[0].message.contains(says), "{findings:?}");
        }
    }

    #[tokio::test]
    async fn long_lists_and_odd_names_stay_readable() {
        let many: Vec<String> = (0..9)
            .map(|n| format!(r#"{{"id":"{n}","type":1,"name":"old{n}\n\u001b[31m"}}"#))
            .collect();
        let listed = format!("[{}]", many.join(","));
        let discord = Canned::new().json(&url("/applications/123/commands"), 200, &listed);
        let findings = commands(
            &discord,
            API,
            "SECRET_TOKEN_AAA",
            &app(false),
            Scope::Global,
            &[],
            &redactor(),
        )
        .await;
        let stale = &findings[0].message;
        assert!(stale.contains("9 commands"), "{stale}");
        assert!(stale.contains("and 3 more"), "{stale}");
        assert!(
            !stale.contains('\n') && !stale.contains('\u{1b}'),
            "{stale:?}"
        );
    }

    #[test]
    fn commands_are_described_as_people_see_them() {
        let described = |kind, name| described(&key(kind, name), &redactor());
        assert_eq!(described(1, "gallery"), "/gallery");
        assert_eq!(described(2, "Profile"), "“Profile” (user menu)");
        assert_eq!(described(3, "Archive"), "“Archive” (message menu)");
        assert_eq!(described(4, "launch"), "“launch” (Entry Point)");
        assert_eq!(described(9, "x"), "“x” (type 9)");
        assert_eq!(count(1), "1 command");
        assert_eq!(count(0), "0 commands");
    }

    #[test]
    fn the_real_command_list_matches_itself() {
        // The list registration submits, compared with itself as Discord
        // would return it: nothing missing, nothing stale.
        let local = leaf_bot::registered_commands().unwrap();
        assert!(local.len() >= 10, "{local:?}");
        let findings = registration(&local, &local, "globally", &redactor());
        assert_eq!(statuses(&findings), [Status::Ok]);
    }

    // ---- gateway ----

    fn gateway_body(total: u64, remaining: u64, reset_after: u64) -> String {
        format!(
            r#"{{"url":"wss://gateway.discord.gg","shards":1,"session_start_limit":
               {{"total":{total},"remaining":{remaining},"reset_after":{reset_after},
               "max_concurrency":1}}}}"#
        )
    }

    #[tokio::test]
    async fn session_starts_decide_the_gateway_finding() {
        // (total, remaining, reset_after ms, status, what the finding says)
        let cases = [
            (
                1000,
                998,
                14_400_000,
                Status::Ok,
                "998 of 1000 session starts remain",
            ),
            (1000, 100, 0, Status::Ok, "100 of 1000"),
            (1000, 99, 5_400_000, Status::Warn, "Only 99 of 1000"),
            (1000, 1, 60_000, Status::Warn, "resets in 1 min"),
            (1000, 0, 11_520_000, Status::Fail, "resets in 3 h 12 min"),
            (1000, 0, 30_000, Status::Fail, "resets in under a minute"),
        ];
        for (total, remaining, reset_after, status, says) in cases {
            let body = gateway_body(total, remaining, reset_after);
            let discord = Canned::new().json(&url("/gateway/bot"), 200, &body);
            let findings = gateway(&discord, API, "SECRET_TOKEN_AAA").await;
            assert_eq!(statuses(&findings), [status], "{remaining}");
            assert!(findings[0].message.contains(says), "{findings:?}");
            assert_eq!(findings[0].next.is_some(), status != Status::Ok);
            assert_no_secrets(&findings);
        }
    }

    #[tokio::test]
    async fn a_gateway_discord_will_not_describe_fails() {
        let refused = Canned::new().json(&url("/gateway/bot"), 401, "{}");
        let odd = Canned::new().json(&url("/gateway/bot"), 200, r#"{"url":"wss://x"}"#);
        let gone = Canned::new().unanswered(&url("/gateway/bot"), Unanswered::TimedOut);
        for (discord, says) in [
            (refused, "does not accept the bot token"),
            (odd, "shape leaf expects"),
            (gone, "within 10 seconds"),
        ] {
            let findings = gateway(&discord, API, "SECRET_TOKEN_AAA").await;
            assert_eq!(statuses(&findings), [Status::Fail], "{says}");
            assert!(findings[0].message.contains(says), "{findings:?}");
        }
    }

    #[test]
    fn waits_read_in_hours_and_minutes() {
        assert_eq!(wait_text(0), "under a minute");
        assert_eq!(wait_text(59_999), "under a minute");
        assert_eq!(wait_text(60_000), "1 min");
        assert_eq!(wait_text(3_600_000), "1 h");
        assert_eq!(wait_text(86_340_000), "23 h 59 min");
    }

    #[test]
    fn a_skipped_check_says_why_in_one_sentence() {
        let finding = not_asked(Check::Gateway, Blocked::TokenRefused.why());
        assert_eq!(finding.status, Status::Skip);
        assert_eq!(
            finding.message,
            "Not checked: Discord does not accept the bot token."
        );
    }
}
