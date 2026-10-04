//! Check 1: `leaf.conf` loads, and the public address in it is one Discord
//! can use.
//!
//! The file holds the credentials, so nothing from it is quoted except the
//! public address: a file that does not parse is described by the line the
//! parser stopped at, never by the parser's own message (which can repeat
//! the value it choked on).

use std::ops::Range;
use std::path::Path;

use leaf_core::config::{CONFIG_FILE_NAME, ConfigError, Tier1Config};

use super::report::{Check, Finding, Redactor};

/// The way to change anything `leaf.conf` holds.
pub const RECONFIGURE: &str = "Run leaf once with --reconfigure and enter the values again \
    (guide/01-install.md, “Changing credentials later”).";

/// Loads the configuration in `data_dir` and reports on it. The loaded
/// configuration comes back for the checks that need it.
pub fn check(data_dir: &Path) -> (Vec<Finding>, Option<Tier1Config>) {
    let path = data_dir.join(CONFIG_FILE_NAME);
    // Until the file has loaded there are no credentials to look for, and
    // nothing from the file is quoted.
    let dir = Redactor::default().quoted(&data_dir.display().to_string());
    let config = match Tier1Config::load(&path) {
        Ok(Some(config)) => config,
        Ok(None) => {
            let finding = Finding::fail(
                Check::Config,
                format!("There is no leaf.conf in {dir}, so leaf is not set up here."),
                "Finish first-run setup at /setup (guide/01-install.md), or point DATA_DIR at \
                 the directory that holds leaf.conf.",
            );
            return (vec![finding], None);
        }
        Err(e) => return (vec![unloadable(&path, &dir, &e)], None),
    };
    let findings = vec![
        Finding::ok(Check::Config, format!("leaf.conf loads from {dir}.")),
        public_address(&config.public_url, &Redactor::for_config(&config)),
    ];
    (findings, Some(config))
}

/// Why a file that exists is not a configuration.
fn unloadable(path: &Path, dir: &str, e: &ConfigError) -> Finding {
    match e {
        ConfigError::Io(e) => Finding::fail(
            Check::Config,
            format!("leaf.conf in {dir} could not be read ({}).", e.kind()),
            "Make the file readable by the user leaf runs as.",
        ),
        ConfigError::Parse(e) => {
            let detail = missing_key(e.message()).map_or_else(
                || {
                    // Read again only to turn the parser's position into a
                    // line.
                    let raw = std::fs::read_to_string(path).unwrap_or_default();
                    line_of(&raw, e.span())
                        .map(|line| format!(" (the problem is on line {line})"))
                        .unwrap_or_default()
                },
                |key| format!(": the key {key} is missing"),
            );
            Finding::fail(
                Check::Config,
                format!("leaf.conf in {dir} is not a configuration leaf can read{detail}."),
                RECONFIGURE,
            )
        }
        ConfigError::EmptyField(name) => Finding::fail(
            Check::Config,
            format!("leaf.conf in {dir} has no value for {name}."),
            RECONFIGURE,
        ),
        ConfigError::BadPublicUrl => Finding::fail(
            Check::Config,
            format!("The public URL in leaf.conf ({dir}) does not start with https://."),
            RECONFIGURE,
        ),
        ConfigError::BadStorageFolder => Finding::fail(
            Check::Config,
            format!(
                "r2.endpoint in leaf.conf (in {dir}) starts with file: but is not file:// \
                 followed by a folder's full path without .. in it, such as \
                 file:///data/media."
            ),
            RECONFIGURE,
        ),
        // Only writing a configuration serializes one.
        ConfigError::Serialize(_) => Finding::fail(
            Check::Config,
            format!("leaf.conf in {dir} could not be loaded."),
            RECONFIGURE,
        ),
    }
}

/// Every key a configuration has.
const KEYS: [&str; 9] = [
    "discord_token",
    "client_id",
    "client_secret",
    "public_url",
    "r2",
    "endpoint",
    "bucket",
    "access_key_id",
    "secret_access_key",
];

/// The key a parser message says is missing. Of everything such a message
/// can say, this is the one part that cannot be a value from the file, and
/// it is only passed on when it is a key leaf knows.
fn missing_key(message: &str) -> Option<&'static str> {
    let named = message.strip_prefix("missing field `")?.strip_suffix('`')?;
    KEYS.into_iter().find(|key| *key == named)
}

/// The 1-based line a parser position falls on. `None` without a position,
/// or with the empty one at the very start (the parser's way of marking no
/// particular place).
fn line_of(raw: &str, span: Option<Range<usize>>) -> Option<usize> {
    let span = span?;
    if span.is_empty() && span.start == 0 {
        return None;
    }
    let before = raw.get(..span.start)?;
    Some(before.matches('\n').count() + 1)
}

/// Whether `host` (as a URL spells it) is this machine.
fn is_this_machine(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// The public address: https, or plain http on this machine only, and
/// nothing but the origin (Discord compares sign-in redirects exactly).
fn public_address(public_url: &str, redactor: &Redactor) -> Finding {
    let shown = redactor.quoted(public_url);
    let parsed = reqwest::Url::parse(public_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| (url.clone(), host.to_owned())));
    let Some((url, host)) = parsed else {
        return Finding::fail(
            Check::Config,
            format!("The public URL in leaf.conf, {shown}, is not an address."),
            RECONFIGURE,
        );
    };
    let local = is_this_machine(&host);
    if url.scheme() != "https" && !local {
        return Finding::fail(
            Check::Config,
            format!(
                "The public URL in leaf.conf, {shown}, is plain http on a host other than this \
                 machine. Discord only loads the gallery over https."
            ),
            RECONFIGURE,
        );
    }
    let origin = url.origin().ascii_serialization();
    if origin != public_url {
        return Finding::warn(
            Check::Config,
            format!(
                "The public URL in leaf.conf is {shown}, not the bare address {origin}. Discord \
                 compares sign-in redirects exactly, so sign-in may fail."
            ),
            format!("Enter {origin} as the public URL. {RECONFIGURE}"),
        );
    }
    if local {
        return Finding::ok(
            Check::Config,
            format!(
                "The public address is {shown}: plain http on this machine, which only local \
                 development can use."
            ),
        );
    }
    Finding::ok(Check::Config, format!("The public address is {shown}."))
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
    use crate::doctor::testing::{SECRETS, assert_no_secrets, config, printed, statuses};

    fn data_dir(config: &Tier1Config) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        config.save(&dir.path().join(CONFIG_FILE_NAME)).unwrap();
        dir
    }

    #[test]
    fn a_saved_configuration_loads_and_its_address_is_reported() {
        let dir = data_dir(&config());
        let (findings, loaded) = check(dir.path());
        assert_eq!(statuses(&findings), [Status::Ok, Status::Ok]);
        assert_eq!(loaded, Some(config()));
        assert!(findings[0].message.starts_with("leaf.conf loads from "));
        assert_eq!(
            findings[1].message,
            "The public address is https://leaf.example.com."
        );
        assert_no_secrets(&findings);
    }

    #[test]
    fn a_missing_file_fails_and_points_at_setup() {
        let dir = tempfile::tempdir().unwrap();
        let (findings, loaded) = check(dir.path());
        assert_eq!(loaded, None);
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("There is no leaf.conf"));
        assert!(findings[0].next.as_deref().unwrap().contains("/setup"));
    }

    #[test]
    fn a_file_that_does_not_parse_is_described_without_quoting_it() {
        // (file, what the finding says)
        let cases = [
            // The parser's own message would repeat this value.
            (
                "discord_token = \"x\"\nclient_id = \"1\"\nr2 = \"SECRET_KEY_DDD\"\n".to_owned(),
                "the problem is on line 3",
            ),
            (
                "discord_token = SECRET_TOKEN_AAA\n".to_owned(),
                "the problem is on line 1",
            ),
            // A key that is not there has no line, but has a name.
            (
                "discord_token = \"SECRET_TOKEN_AAA\"\n".to_owned(),
                "leaf can read: the key client_id is missing.",
            ),
            (
                "discord_token = \"SECRET_TOKEN_AAA\"\nclient_id = \"1\"\n\
                 client_secret = \"SECRET_CLIENT_BBB\"\npublic_url = \"https://x\"\n\
                 [r2]\nbucket = \"b\"\n"
                    .to_owned(),
                "leaf can read: the key endpoint is missing.",
            ),
            // Not TOML at all.
            ("SECRET_TOKEN_AAA".to_owned(), "the problem is on line 1"),
            (
                String::new(),
                "leaf can read: the key discord_token is missing.",
            ),
        ];
        for (file, says) in cases {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(CONFIG_FILE_NAME), &file).unwrap();
            let (findings, loaded) = check(dir.path());
            assert_eq!(loaded, None, "{file}");
            assert_eq!(statuses(&findings), [Status::Fail], "{file}");
            assert!(findings[0].message.contains(says), "{findings:?}");
            assert_eq!(findings[0].next.as_deref(), Some(RECONFIGURE));
            // No redactor can help here (nothing loaded): the finding
            // itself must be clean.
            assert_no_secrets(&findings);
        }
    }

    #[test]
    fn an_empty_field_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let raw = "discord_token = \"SECRET_TOKEN_AAA\"\nclient_id = \" \"\n\
                   client_secret = \"s\"\npublic_url = \"https://leaf.example.com\"\n\
                   [r2]\nendpoint = \"https://e\"\nbucket = \"b\"\n\
                   access_key_id = \"a\"\nsecret_access_key = \"k\"\n";
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), raw).unwrap();
        let (findings, loaded) = check(dir.path());
        assert_eq!(loaded, None);
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("no value for client_id"));
        assert_no_secrets(&findings);
    }

    /// A `leaf.conf` whose storage table is `r2`.
    fn file_with(r2: &str) -> String {
        format!(
            "discord_token = \"SECRET_TOKEN_AAA\"\nclient_id = \"123\"\n\
             client_secret = \"SECRET_CLIENT_BBB\"\npublic_url = \"https://leaf.example.com\"\n\
             [r2]\n{r2}"
        )
    }

    #[test]
    fn a_folder_config_loads_without_a_bucket_or_keys() {
        // As setup writes it (the endpoint alone), with empty values, and
        // with the placeholder text of a config from before setup offered a
        // folder.
        for rest in [
            "",
            "bucket = \"\"\naccess_key_id = \"\"\nsecret_access_key = \"\"\n",
            "bucket = \"local\"\naccess_key_id = \"local\"\nsecret_access_key = \"local\"\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let raw = file_with(&format!("endpoint = \"file:///data/media\"\n{rest}"));
            std::fs::write(dir.path().join(CONFIG_FILE_NAME), raw).unwrap();
            let (findings, loaded) = check(dir.path());
            assert_eq!(statuses(&findings), [Status::Ok, Status::Ok], "{rest:?}");
            assert_eq!(loaded.unwrap().r2.endpoint, "file:///data/media");
            assert_no_secrets(&findings);
        }
    }

    #[test]
    fn a_bucket_config_without_its_bucket_or_keys_is_named() {
        // (the storage table, the setting named)
        let cases = [
            ("endpoint = \"https://e\"\n", "r2.bucket"),
            (
                "endpoint = \"https://e\"\nbucket = \"b\"\nsecret_access_key = \"k\"\n",
                "r2.access_key_id",
            ),
            (
                "endpoint = \"https://e\"\nbucket = \"b\"\naccess_key_id = \"a\"\n",
                "r2.secret_access_key",
            ),
        ];
        for (r2, named) in cases {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(CONFIG_FILE_NAME), file_with(r2)).unwrap();
            let (findings, loaded) = check(dir.path());
            assert_eq!(loaded, None, "{r2}");
            assert_eq!(statuses(&findings), [Status::Fail], "{r2}");
            assert!(
                findings[0]
                    .message
                    .ends_with(&format!("has no value for {named}.")),
                "{findings:?}"
            );
            assert_eq!(findings[0].next.as_deref(), Some(RECONFIGURE));
            assert_no_secrets(&findings);
        }
    }

    #[test]
    fn a_storage_folder_that_is_not_a_full_path_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let raw = file_with("endpoint = \"file://media\"\n");
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), raw).unwrap();
        let (findings, loaded) = check(dir.path());
        assert_eq!(loaded, None);
        assert_eq!(statuses(&findings), [Status::Fail]);
        // The setting at fault is named, and the data directory is said to
        // be where the file is: it is not the folder that was refused.
        let message = &findings[0].message;
        assert!(
            message.starts_with("r2.endpoint in leaf.conf (in "),
            "{message}"
        );
        assert!(
            message.ends_with(
                ") starts with file: but is not file:// followed by a folder's full path \
                 without .. in it, such as file:///data/media."
            ),
            "{message}"
        );
        assert_eq!(findings[0].next.as_deref(), Some(RECONFIGURE));
    }

    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        // A directory where the file should be: reading it is an I/O error.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(CONFIG_FILE_NAME)).unwrap();
        let (findings, loaded) = check(dir.path());
        assert_eq!(loaded, None);
        assert_eq!(statuses(&findings), [Status::Fail]);
        assert!(findings[0].message.contains("could not be read"));
    }

    #[test]
    fn the_public_address_must_be_https_away_from_this_machine() {
        // (public URL, status, what the finding says)
        let cases = [
            (
                "https://leaf.example.com",
                Status::Ok,
                "The public address is",
            ),
            (
                "https://leaf.example.com:8443",
                Status::Ok,
                "The public address is",
            ),
            (
                "http://localhost:3777",
                Status::Ok,
                "plain http on this machine",
            ),
            (
                "http://127.0.0.1:3777",
                Status::Ok,
                "plain http on this machine",
            ),
            (
                "http://[::1]:3777",
                Status::Ok,
                "plain http on this machine",
            ),
            // `Tier1Config::validate` only looks at how the URL starts.
            (
                "http://localhost.example.com",
                Status::Fail,
                "plain http on a host other than this machine",
            ),
            (
                "http://127.0.0.1.example.com",
                Status::Fail,
                "plain http on a host other than this machine",
            ),
            ("http://leaf.example.com", Status::Fail, "plain http"),
            ("https://", Status::Fail, "is not an address"),
            (
                "https://leaf.example.com/",
                Status::Warn,
                "not the bare address",
            ),
            (
                "https://leaf.example.com/app",
                Status::Warn,
                "not the bare address",
            ),
            (
                "https://Leaf.Example.com",
                Status::Warn,
                "https://leaf.example.com",
            ),
        ];
        for (url, status, says) in cases {
            let finding = public_address(url, &Redactor::default());
            assert_eq!(finding.status, status, "{url}: {finding:?}");
            assert!(finding.message.contains(says), "{url}: {finding:?}");
            assert_eq!(finding.next.is_some(), status != Status::Ok, "{url}");
        }
    }

    #[test]
    fn a_plain_http_address_fails_the_check_and_loads_nothing() {
        // The path a real install takes: leaf itself refuses to load such a
        // file, so `public_address` never sees the address.
        let dir = tempfile::tempdir().unwrap();
        let raw = "discord_token = \"SECRET_TOKEN_AAA\"\nclient_id = \"123\"\n\
                   client_secret = \"SECRET_CLIENT_BBB\"\npublic_url = \"http://leaf.example.com\"\n\
                   [r2]\nendpoint = \"https://e\"\nbucket = \"b\"\n\
                   access_key_id = \"SECRET_KEYID_CCC\"\nsecret_access_key = \"SECRET_KEY_DDD\"\n";
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), raw).unwrap();
        let (findings, loaded) = check(dir.path());
        assert_eq!(loaded, None);
        assert_eq!(statuses(&findings), [Status::Fail], "{findings:?}");
        assert!(
            findings[0].message.contains("does not start with https://"),
            "{findings:?}"
        );
        assert_eq!(findings[0].next.as_deref(), Some(RECONFIGURE));
        assert_no_secrets(&findings);
    }

    #[test]
    fn a_bad_address_in_a_loaded_file_fails_the_check() {
        let mut config = config();
        config.public_url = "http://localhost.example.com".to_owned();
        let dir = data_dir(&config);
        let (findings, loaded) = check(dir.path());
        // The file loads (and the other checks can run), but the address is
        // a failure.
        assert!(loaded.is_some());
        assert_eq!(statuses(&findings), [Status::Ok, Status::Fail]);
    }

    #[test]
    fn line_numbers_come_from_the_parser_position() {
        assert_eq!(line_of("a\nb\nc", Some(4..5)), Some(3));
        assert_eq!(line_of("a\nb\nc", Some(0..1)), Some(1));
        assert_eq!(line_of("a\nb\nc", Some(0..5)), Some(1));
        assert_eq!(line_of("a\nb\nc", Some(0..0)), None);
        assert_eq!(line_of("a\nb\nc", Some(2..2)), Some(2));
        assert_eq!(line_of("a\nb\nc", None), None);
        assert_eq!(line_of("abc", Some(10..11)), None);
    }

    #[test]
    fn only_a_known_key_is_repeated_from_a_parser_message() {
        assert_eq!(missing_key("missing field `client_id`"), Some("client_id"));
        assert_eq!(missing_key("missing field `bucket`"), Some("bucket"));
        // Anything else could be a value from the file.
        assert_eq!(missing_key("missing field `SECRET_TOKEN_AAA`"), None);
        assert_eq!(
            missing_key("invalid type: string \"SECRET_KEY_DDD\", expected struct R2Config"),
            None
        );
        assert_eq!(missing_key(""), None);
    }

    #[test]
    fn nothing_printed_about_the_configuration_holds_a_credential() {
        let dir = data_dir(&config());
        let (findings, _) = check(dir.path());
        let printed = printed(&findings);
        for secret in SECRETS {
            assert!(!printed.contains(secret), "{secret} in {printed}");
        }
    }
}
