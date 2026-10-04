//! `leaf doctor`: a live check of a configured install, to run instead of
//! working through a checklist by hand ("is the bot online, are the commands
//! registered, does storage work").
//!
//! It never starts the server or the gateway, never writes to the database
//! it is pointed at, and never prints a credential. Each check is a function
//! over a client it is handed ([`net::Fetch`] for HTTP,
//! [`storage::StorageProbe`] for the bucket), so the tests run every check
//! against stand-ins and only [`main`] builds the clients that reach the
//! network.
//!
//! One finding is one line: `ok`, `warn`, `FAIL` or `skip`, the check's
//! name, a sentence, and for a warning or failure the next step. A `skip` is
//! a check that could not be made because something it depends on failed
//! (that failure is on a line of its own) or because there was nothing to
//! check. The exit status is 1 when anything failed.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use leaf_core::config::Tier1Config;
use leaf_server::api::auth::{MediaSigner, SessionKey};

use self::args::{Command, Options, UrlTarget};
use self::net::Fetch;
use self::report::{Check, Finding, Redactor, Report};
use self::storage::StorageProbe;
use crate::DevGuild;

mod args;
mod config;
mod database;
mod discord;
mod net;
mod report;
mod storage;
#[cfg(test)]
mod testing;
mod web;

/// File name of the database inside the data directory.
const DATABASE_FILE_NAME: &str = "leaf.db";
/// Exit status for a command line that cannot be read.
const EXIT_USAGE: u8 = 2;

/// What a run takes from its surroundings.
#[derive(Debug, Clone)]
pub struct Env {
    /// The data directory (`DATA_DIR`): where `leaf.conf` and `leaf.db` are.
    pub data_dir: PathBuf,
    /// Where leaf registers its commands (`DEV_GUILD_ID`).
    pub dev_guild: DevGuild,
    /// The time, in unix seconds: what a media address is signed against.
    pub now_unix: i64,
}

/// The clients the checks talk through.
struct Clients<'a, F, S> {
    fetch: &'a F,
    storage: &'a S,
    /// Base URL of Discord's REST API.
    discord_api: &'a str,
}

/// Runs `leaf doctor` with the arguments after the word `doctor`: prints
/// the findings and returns the exit status.
pub async fn main(args: &[String], env: Env) -> ExitCode {
    let options = match args::parse(args) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => return print(args::USAGE.as_bytes()),
        Err(problem) => {
            let mut err = std::io::stderr().lock();
            // Nothing to do about a stderr that cannot be written to.
            let _unwritten = writeln!(err, "{problem}\nleaf doctor --help lists the options.");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let clients = net::LiveFetch::new().and_then(|fetch| {
        leaf_server::validate::LiveValidator::new().map(|storage| (fetch, storage))
    });
    let (fetch, storage) = match clients {
        Ok(clients) => clients,
        Err(e) => {
            tracing::error!(error = %e, "doctor: building the HTTP client failed");
            return ExitCode::FAILURE;
        }
    };
    let clients = Clients {
        fetch: &fetch,
        storage: &storage,
        discord_api: leaf_server::validate::DISCORD_API,
    };
    let report = examine(&options, &env, &clients).await;

    let mut out = Vec::new();
    let written = if options.json {
        report.write_json(&mut out)
    } else {
        report.write_text(&mut out)
    };
    if let Err(e) = written {
        tracing::error!(error = %e, "doctor: the report could not be written");
        return ExitCode::FAILURE;
    }
    match print(&out) {
        code if code == ExitCode::SUCCESS && report.failed() => ExitCode::FAILURE,
        code => code,
    }
}

/// Writes to stdout; a stdout that is gone is a failure.
fn print(bytes: &[u8]) -> ExitCode {
    let mut out = std::io::stdout().lock();
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// What a check that needs `leaf.conf` says when there is none.
fn no_config(check: Check) -> Finding {
    Finding::skip(check, "Not checked: leaf.conf did not load.")
}

/// Runs the selected checks in order and returns the findings, fit to
/// print (one line each, the configured credentials taken out).
async fn examine<F: Fetch, S: StorageProbe>(
    options: &Options,
    env: &Env,
    clients: &Clients<'_, F, S>,
) -> Report {
    let selected = options.selected();
    let wants = |check: Check| selected.contains(&check);
    let db_path = env.data_dir.join(DATABASE_FILE_NAME);
    let mut findings = Vec::new();

    // Most checks need the configuration, so it is always loaded. What it
    // found is reported when asked for, and when its absence is why another
    // check cannot run.
    let (config_findings, config) = config::check(&env.data_dir);
    let url_needs_config = matches!(options.url, Some(UrlTarget::Configured));
    let needed = [
        Check::Discord,
        Check::Commands,
        Check::Gateway,
        Check::Storage,
    ]
    .into_iter()
    .any(wants)
        || (wants(Check::Url) && url_needs_config);
    if wants(Check::Config) || (config.is_none() && needed) {
        findings.extend(config_findings);
    }
    let config = config.as_ref();
    // What the checks quote from outside leaf goes through this, so that
    // nothing a server or a driver says can put a credential in a finding.
    let redactor = config.map(Redactor::for_config).unwrap_or_default();
    let redactor = &redactor;

    let identity = match config {
        Some(config) if wants(Check::Discord) => {
            let (fetch, api) = (clients.fetch, clients.discord_api);
            let identity = discord::identity(fetch, api, config, redactor).await;
            findings.extend(identity.findings.iter().cloned());
            Some(identity)
        }
        Some(_) => None,
        None => {
            if wants(Check::Discord) {
                findings.push(no_config(Check::Discord));
            }
            None
        }
    };
    if wants(Check::Commands) {
        findings.extend(match config {
            Some(config) => {
                let (fetch, api) = (clients.fetch, clients.discord_api);
                let identity = identity.as_ref();
                commands(fetch, api, config, identity, env.dev_guild, redactor).await
            }
            None => vec![no_config(Check::Commands)],
        });
    }
    if wants(Check::Gateway) {
        // A token Discord has just refused would only be refused again.
        let blocked = identity.as_ref().and_then(|identity| identity.blocked);
        findings.extend(match (config, blocked) {
            (None, _) => vec![no_config(Check::Gateway)],
            (Some(_), Some(blocked)) => vec![discord::not_asked(Check::Gateway, blocked.why())],
            (Some(config), None) => {
                discord::gateway(clients.fetch, clients.discord_api, &config.discord_token).await
            }
        });
    }
    if wants(Check::Storage) {
        findings.extend(match config {
            Some(config) => storage::check(clients.storage, &config.r2, redactor).await,
            None => vec![no_config(Check::Storage)],
        });
    }
    if wants(Check::Database) {
        findings.extend(database::check(&db_path, &database::embedded(), redactor).await);
    }
    if wants(Check::DbCopy) {
        findings.extend(match &options.db_copy {
            Some(source) => database::check_copy(source, redactor).await,
            // The command line is refused before this can happen.
            None => vec![Finding::skip(
                Check::DbCopy,
                "Not checked: no file was given with --db-copy.",
            )],
        });
    }
    if wants(Check::Url) {
        let fetch = clients.fetch;
        findings.extend(url(fetch, options, config, &db_path, env.now_unix, redactor).await);
    }

    Report { findings }.printable(redactor)
}

/// Check 3, with what it needs gathered: the list leaf registers, the place
/// it registers it, and the token's application.
async fn commands(
    fetch: &impl Fetch,
    api: &str,
    config: &Tier1Config,
    identity: Option<&discord::Identity>,
    dev_guild: DevGuild,
    redactor: &Redactor,
) -> Vec<Finding> {
    let local = match leaf_bot::registered_commands() {
        Ok(local) => local,
        Err(e) => {
            tracing::error!(
                error = format!("{e:#}"),
                "doctor: leaf's command list is unreadable"
            );
            return vec![Finding::fail(
                Check::Commands,
                "leaf could not list its own commands.",
                "This is a bug in leaf: report it with the error logged for this run.",
            )];
        }
    };
    // The application the identity check named, or (when that check was not
    // run) the answer to the one call this check needs from it.
    let app = if let Some(identity) = identity {
        match (&identity.application, identity.blocked) {
            (Some(app), _) => app.clone(),
            (None, Some(blocked)) => {
                return vec![discord::not_asked(Check::Commands, blocked.why())];
            }
            (None, None) => {
                return vec![discord::not_asked(
                    Check::Commands,
                    "Discord did not say which application the bot token belongs to",
                )];
            }
        }
    } else {
        match discord::application(fetch, api, &config.discord_token).await {
            Ok(app) => app,
            Err(finding) => return vec![finding],
        }
    };
    let mut findings = Vec::new();
    let scope = match dev_guild {
        DevGuild::Unset => discord::Scope::Global,
        DevGuild::Guild(guild) => discord::Scope::Guild(guild),
        DevGuild::NotAnId => {
            findings.push(Finding::warn(
                Check::Commands,
                "DEV_GUILD_ID is set, but not to a server's ID, so leaf registers its commands \
                 globally.",
                "Remove DEV_GUILD_ID (a real install does not set it), or set it to a server's \
                 ID (guide/01-install.md, “Command registration and DEV_GUILD_ID”).",
            ));
            discord::Scope::Global
        }
    };
    let token = &config.discord_token;
    findings.extend(discord::commands(fetch, api, token, &app, scope, &local, redactor).await);
    findings
}

/// Check 8, with the address to ask and the stored file to ask it for.
async fn url(
    fetch: &impl Fetch,
    options: &Options,
    config: Option<&Tier1Config>,
    db_path: &Path,
    now_unix: i64,
    redactor: &Redactor,
) -> Vec<Finding> {
    let base = match (&options.url, config) {
        (Some(UrlTarget::Base(base)), _) => base.clone(),
        (_, None) => return vec![no_config(Check::Url)],
        (_, Some(config)) => match web::base_address(&config.public_url) {
            Ok(base) => base,
            Err(_) => {
                return vec![Finding::fail(
                    Check::Url,
                    "The public URL in leaf.conf is not an address the doctor can ask.",
                    "Give the address to check: leaf doctor --url https://leaf.example.com.",
                )];
            }
        },
    };
    let media = match config {
        None => web::Media::Nothing(
            "leaf.conf did not load, so no media address could be signed".to_owned(),
        ),
        Some(config) => match database::stored_attachment(db_path, redactor).await {
            Ok(Some(attachment_id)) => {
                // The address the gallery itself would be handed right now.
                let key = SessionKey::derive(&config.client_secret);
                let (path, _thumbnail) = MediaSigner::new(&key, now_unix).urls(&attachment_id);
                web::Media::Signed {
                    attachment_id,
                    path,
                }
            }
            Ok(None) => web::Media::Nothing("the database lists no stored media yet".to_owned()),
            Err(why) => web::Media::Nothing(why),
        },
    };
    web::check(fetch, &base, media, redactor).await
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::panic,
        reason = "tests may panic"
    )]

    use std::sync::atomic::{AtomicU32, Ordering};

    use leaf_core::config::{CONFIG_FILE_NAME, R2Config};
    use leaf_server::setup::{Field, FieldError};
    use leaf_server::validate::{Step, StorageFailure};

    use super::report::Status;
    use super::testing::{API, Canned, SECRETS, config, typed};
    use super::*;

    /// A bucket that works, or fails at its write, and counts the asks.
    #[derive(Default)]
    struct Bucket {
        broken: bool,
        asked: AtomicU32,
    }

    impl StorageProbe for Bucket {
        async fn round_trip(&self, _r2: &R2Config) -> Result<(), StorageFailure> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if self.broken {
                return Err(StorageFailure {
                    step: Some(Step::Write),
                    error: FieldError::new(Field::R2Bucket, "R2 has no bucket named “leaf-media”."),
                    left_behind: false,
                });
            }
            Ok(())
        }
    }

    const NOW: i64 = 1_779_289_200;
    const PUBLIC: &str = "https://leaf.example.com";
    const SHELL: &str = r#"<script type="module" src="/assets/index-1.js"></script>"#;

    /// A data directory with a saved configuration and a migrated database
    /// that nobody has open, whose newest stored file is attachment `a2`.
    async fn data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        config().save(&dir.path().join(CONFIG_FILE_NAME)).unwrap();
        let db = database::fixtures::closed(dir.path()).await;
        assert_eq!(db, dir.path().join(DATABASE_FILE_NAME));
        dir
    }

    fn env(dir: &tempfile::TempDir) -> Env {
        Env {
            data_dir: dir.path().to_owned(),
            dev_guild: DevGuild::Unset,
            now_unix: NOW,
        }
    }

    /// Discord's list of exactly what leaf registers, plus the Entry Point.
    fn registered() -> String {
        let mut listed: Vec<String> = leaf_bot::registered_commands()
            .unwrap()
            .iter()
            .map(|key| format!(r#"{{"type":{},"name":"{}"}}"#, key.kind, key.name))
            .collect();
        listed.push(r#"{"type":4,"name":"launch"}"#.to_owned());
        format!("[{}]", listed.join(","))
    }

    /// The whole world in order: Discord for application 123, and a leaf
    /// answering at its public address.
    fn healthy() -> Canned {
        let discord = |path: &str| format!("{API}{path}");
        let leaf = |path: &str| format!("{PUBLIC}{path}");
        let part = net::Answer {
            status: 206,
            content_type: Some("image/png".to_owned()),
            content_range: Some("bytes 0-0/2048".to_owned()),
            body: vec![0],
        };
        Canned::new()
            .json(
                &discord("/users/@me"),
                200,
                r#"{"id":"9","username":"leaf"}"#,
            )
            .json(
                &discord("/applications/@me"),
                200,
                r#"{"id":"123","name":"leaf","flags":131072}"#,
            )
            .json(&discord("/oauth2/token"), 200, r#"{"access_token":"x"}"#)
            .json(&discord("/applications/123/commands"), 200, &registered())
            .json(
                &discord("/gateway/bot"),
                200,
                r#"{"session_start_limit":{"total":1000,"remaining":999,"reset_after":0}}"#,
            )
            .plain(&leaf("/healthz"), 200, "ok")
            .json(&leaf("/api/status"), 200, r#"{"gateway":"online"}"#)
            .html(&leaf("/"), 200, SHELL)
            .answer(
                &leaf("/assets/index-1.js"),
                typed(200, "text/javascript", ""),
            )
            .json(
                &leaf("/api/leaf-doctor/no-such-route"),
                404,
                r#"{"error":"not_found"}"#,
            )
            .answer_prefix(&leaf("/api/media/"), part)
    }

    async fn run(args: &[&str], env: &Env, fetch: &Canned, bucket: &Bucket) -> Report {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let Ok(Command::Run(options)) = args::parse(&args) else {
            panic!("the test's arguments do not parse: {args:?}");
        };
        let clients = Clients {
            fetch,
            storage: bucket,
            discord_api: API,
        };
        examine(&options, env, &clients).await
    }

    /// (check, status) of every finding, in order.
    fn outline(report: &Report) -> Vec<(&'static str, Status)> {
        report
            .findings
            .iter()
            .map(|f| (f.check.name(), f.status))
            .collect()
    }

    fn rendered(report: &Report) -> String {
        let mut out = Vec::new();
        report.write_text(&mut out).unwrap();
        report.write_json(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn a_healthy_install_passes_every_standing_check() {
        let dir = data_dir().await;
        let (fetch, bucket) = (healthy(), Bucket::default());
        let report = run(&[], &env(&dir), &fetch, &bucket).await;
        assert_eq!(
            outline(&report),
            [
                ("config", Status::Ok),
                ("config", Status::Ok),
                ("discord", Status::Ok),
                ("discord", Status::Ok),
                ("discord", Status::Ok),
                ("commands", Status::Ok),
                ("commands", Status::Ok),
                ("gateway", Status::Ok),
                ("storage", Status::Ok),
                ("database", Status::Ok),
            ],
            "{report:?}"
        );
        assert!(!report.failed());
        assert_eq!(bucket.asked.load(Ordering::SeqCst), 1);
        // Nothing was asked of the leaf itself: --url was not given.
        assert!(fetch.asked().iter().all(|url| url.starts_with(API)));
    }

    #[tokio::test]
    async fn url_and_db_copy_join_the_run_when_asked_for() {
        let dir = data_dir().await;
        let (fetch, bucket) = (healthy(), Bucket::default());
        let db = dir.path().join(DATABASE_FILE_NAME);
        let args = ["--url", "--db-copy", db.to_str().unwrap()];
        let report = run(&args, &env(&dir), &fetch, &bucket).await;
        let outline = outline(&report);
        assert_eq!(outline.len(), 18, "{report:?}");
        assert_eq!(
            outline[10..],
            [
                ("db-copy", Status::Ok),
                ("db-copy", Status::Ok),
                ("db-copy", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
            ]
        );
        assert!(!report.failed());
    }

    #[tokio::test]
    async fn the_media_address_is_signed_as_the_server_would_sign_it() {
        let dir = data_dir().await;
        let (fetch, bucket) = (healthy(), Bucket::default());
        let report = run(&["--only", "url"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(outline(&report), [("url", Status::Ok); 5], "{report:?}");

        let asked = fetch.asked();
        let media = asked
            .iter()
            .find(|url| url.contains("/api/media/"))
            .unwrap();
        // The newest stored attachment, at the configured address.
        let query = media
            .strip_prefix("https://leaf.example.com/api/media/a2?")
            .unwrap();
        let (exp, sig) = query.split_once("&sig=").unwrap();
        let exp: i64 = exp.strip_prefix("exp=").unwrap().parse().unwrap();
        // A server holding the same client secret accepts it now.
        let key = SessionKey::derive(&config().client_secret);
        assert!(key.verify_media("a2", exp, sig, NOW));
        // The signature is in the request and nowhere in the output.
        let rendered = rendered(&report);
        assert!(!rendered.contains(sig), "{rendered}");
        assert!(!rendered.contains("sig="), "{rendered}");
    }

    #[tokio::test]
    async fn an_address_on_the_command_line_is_asked_instead_of_the_configured_one() {
        let dir = data_dir().await;
        let local = "http://127.0.0.1:3777";
        let fetch = Canned::new()
            .plain(&format!("{local}/healthz"), 200, "ok")
            .json(
                &format!("{local}/api/status"),
                200,
                r#"{"gateway":"online"}"#,
            )
            .html(&format!("{local}/"), 200, SHELL)
            .answer(
                &format!("{local}/assets/index-1.js"),
                typed(200, "text/javascript", ""),
            )
            .json(
                &format!("{local}/api/leaf-doctor/no-such-route"),
                404,
                r#"{"error":"not_found"}"#,
            )
            .answer_prefix(
                &format!("{local}/api/media/"),
                typed(404, "application/json", "{}"),
            );
        let args = ["--only", "url", "--url", local];
        let report = run(&args, &env(&dir), &fetch, &Bucket::default()).await;
        assert_eq!(
            outline(&report),
            [
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Fail),
            ]
        );
        assert!(report.failed());
        assert!(fetch.asked().iter().all(|url| url.starts_with(local)));
    }

    #[tokio::test]
    async fn only_runs_only_what_it_names() {
        let dir = data_dir().await;
        let (fetch, bucket) = (healthy(), Bucket::default());
        let report = run(&["--only", "storage,database"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(
            outline(&report),
            [("storage", Status::Ok), ("database", Status::Ok)]
        );
        // Not a single HTTP request.
        assert!(fetch.asked().is_empty());

        // The gateway alone asks Discord one thing.
        let fetch = healthy();
        let report = run(&["--only", "gateway"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(outline(&report), [("gateway", Status::Ok)]);
        assert_eq!(fetch.asked(), [format!("{API}/gateway/bot")]);

        // The commands alone ask for the application and the list.
        let fetch = healthy();
        let report = run(&["--only", "commands"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(
            outline(&report),
            [("commands", Status::Ok), ("commands", Status::Ok)]
        );
        assert_eq!(
            fetch.asked(),
            [
                format!("{API}/applications/@me"),
                format!("{API}/applications/123/commands")
            ]
        );
    }

    #[tokio::test]
    async fn without_a_configuration_the_cause_fails_once_and_the_rest_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (fetch, bucket) = (healthy(), Bucket::default());
        let report = run(&["--url"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(
            outline(&report),
            [
                ("config", Status::Fail),
                ("discord", Status::Skip),
                ("commands", Status::Skip),
                ("gateway", Status::Skip),
                ("storage", Status::Skip),
                // The database needs no configuration: it is checked, and
                // is not there either.
                ("database", Status::Fail),
                ("url", Status::Skip),
            ],
            "{report:?}"
        );
        assert!(report.failed());
        assert!(fetch.asked().is_empty());
        assert_eq!(bucket.asked.load(Ordering::SeqCst), 0);

        // Asked for one check that needs it: the cause is still reported,
        // and the run still fails.
        let report = run(&["--only", "storage"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(
            outline(&report),
            [("config", Status::Fail), ("storage", Status::Skip)]
        );
        assert!(report.failed());

        // A check that does not need it says nothing about it.
        let report = run(&["--only", "database"], &env(&dir), &fetch, &bucket).await;
        assert_eq!(outline(&report), [("database", Status::Fail)]);

        // An address on the command line needs no configuration, but there
        // is nothing to sign a media address with.
        let args = ["--only", "url", "--url", PUBLIC];
        let report = run(&args, &env(&dir), &fetch, &bucket).await;
        assert_eq!(
            outline(&report),
            [
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Ok),
                ("url", Status::Skip),
            ]
        );
        assert!(!report.failed());
    }

    #[tokio::test]
    async fn a_refused_token_fails_once_and_skips_what_needs_it() {
        let dir = data_dir().await;
        let fetch = healthy().json(&format!("{API}/users/@me"), 401, "{}");
        let report = run(&[], &env(&dir), &fetch, &Bucket::default()).await;
        assert_eq!(
            outline(&report),
            [
                ("config", Status::Ok),
                ("config", Status::Ok),
                ("discord", Status::Fail),
                ("discord", Status::Ok),
                ("commands", Status::Skip),
                ("gateway", Status::Skip),
                ("storage", Status::Ok),
                ("database", Status::Ok),
            ],
            "{report:?}"
        );
        assert!(report.failed());
        assert_eq!(
            report.findings[4].message,
            "Not checked: Discord does not accept the bot token."
        );
        // Nothing more was asked with the refused token.
        assert_eq!(
            fetch.asked(),
            [format!("{API}/users/@me"), format!("{API}/oauth2/token")]
        );

        // Asked for alone, each check reports the refusal itself, as a
        // failure: there is no other line to carry it.
        let fetch = healthy()
            .json(&format!("{API}/applications/@me"), 401, "{}")
            .json(&format!("{API}/gateway/bot"), 401, "{}");
        let args = ["--only", "commands,gateway"];
        let report = run(&args, &env(&dir), &fetch, &Bucket::default()).await;
        assert_eq!(
            outline(&report),
            [("commands", Status::Fail), ("gateway", Status::Fail)]
        );
    }

    #[tokio::test]
    async fn an_application_discord_does_not_name_skips_the_commands_and_says_why() {
        let dir = data_dir().await;
        // Discord takes the token and then fails at naming its application.
        // That is no refusal, so nothing else is held back, but the
        // commands have no application to be looked up under.
        let fetch = healthy().json(&format!("{API}/applications/@me"), 503, "{}");
        let report = run(&[], &env(&dir), &fetch, &Bucket::default()).await;
        assert_eq!(
            outline(&report),
            [
                ("config", Status::Ok),
                ("config", Status::Ok),
                ("discord", Status::Ok),
                ("discord", Status::Fail),
                ("discord", Status::Ok),
                ("commands", Status::Skip),
                ("gateway", Status::Ok),
                ("storage", Status::Ok),
                ("database", Status::Ok),
            ],
            "{report:?}"
        );
        assert_eq!(
            report.findings[5].message,
            "Not checked: Discord did not say which application the bot token belongs to."
        );
        assert!(report.failed());
        // No command list was asked for under a guessed application.
        assert!(!fetch.asked().iter().any(|url| url.ends_with("/commands")));
    }

    #[tokio::test]
    async fn one_broken_part_fails_the_run_and_leaves_the_others_standing() {
        let dir = data_dir().await;
        let bucket = Bucket {
            broken: true,
            ..Bucket::default()
        };
        let report = run(&[], &env(&dir), &healthy(), &bucket).await;
        let failed: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.status == Status::Fail)
            .collect();
        assert_eq!(failed.len(), 1, "{report:?}");
        assert_eq!(failed[0].check, Check::Storage);
        assert!(failed[0].next.is_some());
        assert!(report.failed());
        assert_eq!(report.summary().ok, 9);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn storage_in_a_folder_is_checked_on_disk_and_said_to_be_a_local_folder() {
        let dir = data_dir().await;
        // The folder setup suggests, inside the data directory. The bucket
        // and keys hold the placeholder text of a config written before
        // setup offered a folder.
        let media = dir.path().join("media");
        let mut config = config();
        config.r2 = R2Config {
            endpoint: leaf_core::media::local_endpoint(media.to_str().unwrap()).unwrap(),
            bucket: "local".to_owned(),
            access_key_id: "local".to_owned(),
            secret_access_key: "local".to_owned(),
        };
        config.save(&dir.path().join(CONFIG_FILE_NAME)).unwrap();

        // The storage probe is the real one: a folder needs no network.
        let storage = leaf_server::validate::LiveValidator::new().unwrap();
        let fetch = healthy();
        let clients = Clients {
            fetch: &fetch,
            storage: &storage,
            discord_api: API,
        };
        let args = ["--only".to_owned(), "config,storage".to_owned()];
        let Ok(Command::Run(options)) = args::parse(&args) else {
            panic!("the test's arguments do not parse: {args:?}");
        };
        let report = examine(&options, &env(&dir), &clients).await;
        assert_eq!(
            outline(&report),
            [
                ("config", Status::Ok),
                ("config", Status::Ok),
                ("storage", Status::Ok)
            ],
            "{report:?}"
        );
        let said = &report.findings[2].message;
        let start = format!("Storage is a local folder, “{}”: ", media.display());
        assert!(said.starts_with(&start), "{said}");
        assert!(!rendered(&report).contains("bucket"));
        // Nothing was asked over HTTP, and the test file is gone again.
        assert!(fetch.asked().is_empty());
        assert!(media.is_dir());
        assert_eq!(std::fs::read_dir(&media).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn dev_guild_id_decides_where_the_commands_are_looked_for() {
        let dir = data_dir().await;
        let entry_point = r#"[{"type":4,"name":"launch"}]"#;
        let fetch = healthy()
            .json(
                &format!("{API}/applications/123/commands"),
                200,
                entry_point,
            )
            .json(
                &format!("{API}/applications/123/guilds/77/commands"),
                200,
                &registered().replace(r#",{"type":4,"name":"launch"}"#, ""),
            );
        let env = Env {
            dev_guild: DevGuild::Guild(77),
            ..env(&dir)
        };
        let report = run(&["--only", "commands"], &env, &fetch, &Bucket::default()).await;
        assert_eq!(
            outline(&report),
            [("commands", Status::Ok), ("commands", Status::Ok)],
            "{report:?}"
        );
        assert!(
            report.findings[0]
                .message
                .contains("in server 77 (DEV_GUILD_ID)")
        );

        // A value that is no server ID: said, then checked globally, as
        // leaf itself would register.
        let env = Env {
            dev_guild: DevGuild::NotAnId,
            ..env
        };
        let report = run(
            &["--only", "commands"],
            &env,
            &healthy(),
            &Bucket::default(),
        )
        .await;
        assert_eq!(
            outline(&report),
            [
                ("commands", Status::Warn),
                ("commands", Status::Ok),
                ("commands", Status::Ok)
            ]
        );
        assert!(report.findings[1].message.contains("listed globally"));
    }

    #[tokio::test]
    async fn no_credential_reaches_the_output_of_a_whole_run() {
        let dir = data_dir().await;
        // A world that fails everywhere, and repeats credentials where it
        // can: in names, in details, in error bodies. The status detail
        // puts the bot token across the point where a long detail is cut.
        let detail = format!("{} SECRET_TOKEN_AAA was rejected", "x".repeat(145));
        let fetch = healthy()
            .json(
                &format!("{API}/users/@me"),
                200,
                r#"{"id":"9","username":"SECRET_TOKEN_AAA"}"#,
            )
            .json(
                &format!("{API}/applications/@me"),
                200,
                r#"{"id":"456","name":"SECRET_CLIENT_BBB","flags":0}"#,
            )
            .json(
                &format!("{API}/oauth2/token"),
                401,
                r#"{"error":"SECRET_CLIENT_BBB"}"#,
            )
            .json(
                &format!("{API}/applications/456/commands"),
                200,
                r#"[{"type":1,"name":"SECRET_KEY_DDD"}]"#,
            )
            .json(&format!("{API}/gateway/bot"), 500, "SECRET_TOKEN_AAA")
            .json(
                &format!("{PUBLIC}/api/status"),
                200,
                &format!(r#"{{"gateway":"error","detail":"{detail}"}}"#),
            );
        let bucket = Bucket {
            broken: true,
            ..Bucket::default()
        };
        let report = run(&["--url"], &env(&dir), &fetch, &bucket).await;
        assert!(report.failed());
        let rendered = rendered(&report);
        for secret in SECRETS {
            assert!(!rendered.contains(secret), "{secret} in {rendered}");
        }
        // Nor the first part of one, where a text was cut: each of the
        // test's credentials starts with this word, and leaf never says it.
        assert!(!rendered.contains("SECRET"), "{rendered}");
        // The places that quoted one say so instead.
        assert!(rendered.contains("<redacted>"), "{rendered}");
        // Every finding is one line of the text output.
        let mut text = Vec::new();
        report.write_text(&mut text).unwrap();
        let lines = String::from_utf8(text).unwrap().lines().count();
        assert_eq!(lines, report.findings.len() + 1);
    }

    #[tokio::test]
    async fn a_run_leaves_the_data_directory_exactly_as_it_was() {
        let dir = data_dir().await;
        let snapshot = database::fixtures::snapshot;
        let was = snapshot(dir.path());
        let names: Vec<&str> = was.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["leaf.conf", "leaf.db"]);
        let db = dir.path().join(DATABASE_FILE_NAME);
        let args = ["--url", "--db-copy", db.to_str().unwrap()];
        let report = run(&args, &env(&dir), &healthy(), &Bucket::default()).await;
        assert!(!report.failed(), "{report:?}");
        assert_eq!(snapshot(dir.path()), was);
    }
}
