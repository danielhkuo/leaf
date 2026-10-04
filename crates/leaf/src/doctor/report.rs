//! What the doctor reports: one finding per fact it checked, printed as one
//! line of text or as an entry of a JSON list.
//!
//! Nothing here knows how a fact was checked. The checks build [`Finding`]s,
//! quoting text from outside leaf only through the [`Redactor`]; this module
//! keeps the findings to one line each, takes the credentials out of them
//! once more as a last line of defence, and writes them.

use std::io::{self, Write};

use leaf_core::config::Tier1Config;
use serde::Serialize;

/// The doctor's checks, in the order they run and print. The names are what
/// `--only` takes and what each line and JSON entry carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Check {
    /// `leaf.conf` loads and names a usable public address.
    Config,
    /// The bot token, its application, and the client id and secret.
    Discord,
    /// leaf's commands as Discord lists them.
    Commands,
    /// The gateway can be connected to.
    Gateway,
    /// A write, read, ranged read and delete in the bucket.
    Storage,
    /// The configured database and its migrations, read-only.
    Database,
    /// The migrations, tried on a copy of a database file (`--db-copy`).
    DbCopy,
    /// A running leaf, asked over HTTP (`--url`).
    Url,
}

impl Check {
    /// Every check, in order.
    pub const ALL: [Self; 8] = [
        Self::Config,
        Self::Discord,
        Self::Commands,
        Self::Gateway,
        Self::Storage,
        Self::Database,
        Self::DbCopy,
        Self::Url,
    ];

    /// The checks a run makes when `--only` does not say otherwise. The
    /// other two need an argument of their own.
    pub const STANDING: [Self; 6] = [
        Self::Config,
        Self::Discord,
        Self::Commands,
        Self::Gateway,
        Self::Storage,
        Self::Database,
    ];

    /// The name used on the command line and in the output.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Discord => "discord",
            Self::Commands => "commands",
            Self::Gateway => "gateway",
            Self::Storage => "storage",
            Self::Database => "database",
            Self::DbCopy => "db-copy",
            Self::Url => "url",
        }
    }

    /// The check a name stands for.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|check| check.name() == name)
    }
}

/// How one finding turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// As it should be.
    Ok,
    /// Works, but someone should look.
    Warn,
    /// Broken, or could not be confirmed. Makes the run exit non-zero.
    Fail,
    /// Not checked: something it depends on failed (and is reported as
    /// such), or there was nothing to check.
    Skip,
}

impl Status {
    /// The word that starts the line. Only a failure is in capitals, so it
    /// is the one that stands out and the one `grep FAIL` finds.
    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Skip => "skip",
        }
    }
}

/// One fact the doctor checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// The check it belongs to.
    pub check: Check,
    /// How it turned out.
    pub status: Status,
    /// What was found, as a plain sentence.
    pub message: String,
    /// The next step to take, for a warning or a failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl Finding {
    /// Something that is as it should be.
    pub fn ok(check: Check, message: impl Into<String>) -> Self {
        Self::new(check, Status::Ok, message, None)
    }

    /// Something that works but needs a look, and what to do about it.
    pub fn warn(check: Check, message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new(check, Status::Warn, message, Some(next.into()))
    }

    /// Something broken, and the next step to take.
    pub fn fail(check: Check, message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new(check, Status::Fail, message, Some(next.into()))
    }

    /// Something that was not checked, and why.
    pub fn skip(check: Check, message: impl Into<String>) -> Self {
        Self::new(check, Status::Skip, message, None)
    }

    fn new(check: Check, status: Status, message: impl Into<String>, next: Option<String>) -> Self {
        Self {
            check,
            status,
            message: message.into(),
            next,
        }
    }
}

/// Longest piece of text from outside leaf (an application's name, a
/// status detail, a driver's error) quoted in a finding.
const QUOTED_MAX_CHARS: usize = 160;

/// `text` on one line: control characters (line breaks, terminal escapes)
/// become spaces, and runs of spaces collapse.
fn one_line(text: &str) -> String {
    let mut clean = String::new();
    let mut gap = false;
    for c in text.chars() {
        if c.is_control() || c.is_whitespace() {
            gap = !clean.is_empty();
            continue;
        }
        if gap {
            clean.push(' ');
            gap = false;
        }
        clean.push(c);
    }
    clean
}

/// `text` cut to `max_chars`, with an ellipsis where something was cut off.
fn cut(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Takes the configured credentials out of text, and is the one way text
/// from outside leaf gets into a finding.
///
/// No check puts a credential in a finding. What a check quotes from
/// elsewhere (an application's name, a server's status detail, a driver's
/// error) may repeat one, so it is quoted through [`Redactor::quoted`],
/// which takes the credentials out before it cuts the text. The order is the
/// point: cut first, and a credential lying across the cut leaves its first
/// half in the output, where no later pass can recognise it.
/// [`Report::printable`] runs every finding through the redactor once more.
#[derive(Clone, Default)]
pub struct Redactor {
    /// The credentials, each as it reads on one line: the form a text is
    /// in when it is searched.
    secrets: Vec<String>,
}

/// What a credential is replaced with.
const REDACTED: &str = "<redacted>";
/// Shorter values would blank out ordinary words, and are no credentials.
const SECRET_MIN_LEN: usize = 6;

impl Redactor {
    /// A redactor for the credentials in `config`.
    pub fn for_config(config: &Tier1Config) -> Self {
        // Storage in a folder has no keys. What the two key settings hold
        // then is placeholder text at most, and taking a word such as
        // "folder" out of every finding would hide what the doctor says.
        let folder = leaf_core::media::is_local_endpoint(&config.r2.endpoint);
        let keys = [&config.r2.secret_access_key, &config.r2.access_key_id];
        let secrets = [&config.discord_token, &config.client_secret]
            .into_iter()
            .chain(keys.into_iter().filter(|_| !folder))
            .map(|secret| one_line(secret))
            .filter(|secret| secret.len() >= SECRET_MIN_LEN)
            .collect();
        Self { secrets }
    }

    /// `text` on one line and free of credentials. The line comes first,
    /// so that a hand-edited credential with a tab or a run of spaces in it
    /// is found however the text spaces it.
    fn clean(&self, text: &str) -> String {
        let mut clean = one_line(text);
        for secret in &self.secrets {
            if clean.contains(secret.as_str()) {
                clean = clean.replace(secret.as_str(), REDACTED);
            }
        }
        clean
    }

    /// Text that came from outside leaf, made safe to print inside a
    /// finding: it is put on one line (control characters such as line
    /// breaks and terminal escapes become spaces, runs of spaces collapse),
    /// the credentials are taken out, and only then is a long text cut.
    pub fn quoted(&self, text: &str) -> String {
        self.quoted_up_to(text, QUOTED_MAX_CHARS)
    }

    /// [`Redactor::quoted`] with its own limit, for values that are short by
    /// nature.
    pub fn quoted_up_to(&self, text: &str, max_chars: usize) -> String {
        cut(&self.clean(text), max_chars)
    }
}

// The secrets must not reach a log through a `{:?}` either.
impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// How many findings ended in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// As it should be.
    pub ok: usize,
    /// Needs a look.
    pub warn: usize,
    /// Broken.
    pub fail: usize,
    /// Not checked.
    pub skip: usize,
}

/// Everything one run found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The findings, in the order the checks ran.
    pub findings: Vec<Finding>,
}

/// The shape of `--json`: whether the run passed, the counts, and the
/// findings.
#[derive(Serialize)]
struct JsonReport<'a> {
    ok: bool,
    summary: Summary,
    results: &'a [Finding],
}

impl Report {
    /// Whether anything failed: what decides the exit code.
    pub fn failed(&self) -> bool {
        self.findings.iter().any(|f| f.status == Status::Fail)
    }

    /// The counts per state.
    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for finding in &self.findings {
            match finding.status {
                Status::Ok => summary.ok += 1,
                Status::Warn => summary.warn += 1,
                Status::Fail => summary.fail += 1,
                Status::Skip => summary.skip += 1,
            }
        }
        summary
    }

    /// The same report, fit to print: every text on one line and free of
    /// the credentials `redactor` knows.
    #[must_use]
    pub fn printable(&self, redactor: &Redactor) -> Self {
        let fit = |text: &str| redactor.clean(text);
        Self {
            findings: self
                .findings
                .iter()
                .map(|finding| Finding {
                    check: finding.check,
                    status: finding.status,
                    message: fit(&finding.message),
                    next: finding.next.as_deref().map(fit),
                })
                .collect(),
        }
    }

    /// One line per finding (status, check, sentence, and the next step
    /// when there is one), then a line of counts under the same four words.
    pub fn write_text(&self, out: &mut impl Write) -> io::Result<()> {
        for finding in &self.findings {
            write!(
                out,
                "{:<4}  {:<8}  {}",
                finding.status.label(),
                finding.check.name(),
                finding.message
            )?;
            if let Some(next) = &finding.next {
                write!(out, " Next: {next}")?;
            }
            writeln!(out)?;
        }
        let Summary {
            ok,
            warn,
            fail,
            skip,
        } = self.summary();
        writeln!(
            out,
            "{} checked: {ok} ok, {warn} warn, {fail} FAIL, {skip} skip.",
            self.findings.len()
        )
    }

    /// The report as one JSON object: `ok`, `summary` and `results`.
    pub fn write_json(&self, out: &mut impl Write) -> io::Result<()> {
        let report = JsonReport {
            ok: !self.failed(),
            summary: self.summary(),
            results: &self.findings,
        };
        serde_json::to_writer_pretty(&mut *out, &report)?;
        writeln!(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; JSON indexing is fine in assertions"
    )]

    use super::*;
    use crate::doctor::testing::{SECRETS, config};

    fn text(report: &Report) -> String {
        let mut out = Vec::new();
        report.write_text(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn json(report: &Report) -> serde_json::Value {
        let mut out = Vec::new();
        report.write_json(&mut out).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    fn sample() -> Report {
        Report {
            findings: vec![
                Finding::ok(Check::Config, "leaf.conf loads."),
                Finding::warn(Check::Database, "One migration is pending.", "Start leaf."),
                Finding::fail(Check::Storage, "The bucket is gone.", "Create it."),
                Finding::skip(Check::DbCopy, "Nothing to check."),
            ],
        }
    }

    #[test]
    fn check_names_round_trip_and_are_unique() {
        for check in Check::ALL {
            assert_eq!(Check::from_name(check.name()), Some(check));
        }
        let mut names: Vec<_> = Check::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Check::ALL.len());
        assert_eq!(Check::from_name("Config"), None);
        assert_eq!(Check::from_name(""), None);
        // The standing checks are the ones that need no argument.
        assert!(!Check::STANDING.contains(&Check::DbCopy));
        assert!(!Check::STANDING.contains(&Check::Url));
    }

    #[test]
    fn each_finding_is_one_line_with_its_next_step() {
        let text = text(&sample());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "ok    config    leaf.conf loads.",
                "warn  database  One migration is pending. Next: Start leaf.",
                "FAIL  storage   The bucket is gone. Next: Create it.",
                "skip  db-copy   Nothing to check.",
                "4 checked: 1 ok, 1 warn, 1 FAIL, 1 skip.",
            ]
        );
    }

    #[test]
    fn only_a_failure_fails_the_run() {
        assert!(sample().failed());
        let mut report = sample();
        report.findings.retain(|f| f.status != Status::Fail);
        assert!(!report.failed());
        assert!(!Report::default().failed());
    }

    #[test]
    fn json_carries_the_verdict_the_counts_and_every_finding() {
        let json = json(&sample());
        assert_eq!(json["ok"], false);
        assert_eq!(
            json["summary"],
            serde_json::json!({"ok": 1, "warn": 1, "fail": 1, "skip": 1})
        );
        assert_eq!(
            json["results"],
            serde_json::json!([
                {"check": "config", "status": "ok", "message": "leaf.conf loads."},
                {"check": "database", "status": "warn",
                 "message": "One migration is pending.", "next": "Start leaf."},
                {"check": "storage", "status": "fail",
                 "message": "The bucket is gone.", "next": "Create it."},
                {"check": "db-copy", "status": "skip", "message": "Nothing to check."},
            ])
        );

        let mut passing = sample();
        passing.findings.truncate(2);
        assert_eq!(super::tests::json(&passing)["ok"], true);
    }

    #[test]
    fn json_names_match_the_command_line_names() {
        for check in Check::ALL {
            assert_eq!(serde_json::to_value(check).unwrap(), check.name());
        }
    }

    #[test]
    fn quoted_text_stays_on_one_line_and_is_cut() {
        let plain = Redactor::default();
        assert_eq!(
            plain.quoted("  leaf\n\tbot \u{1b}[31m red "),
            "leaf bot [31m red"
        );
        assert_eq!(plain.quoted(""), "");
        assert_eq!(plain.quoted_up_to("🍃🍃🍃", 2), "🍃🍃…");
        assert_eq!(plain.quoted_up_to("leaf", 4), "leaf");
        assert_eq!(plain.quoted(&"x".repeat(500)).chars().count(), 161);
    }

    #[test]
    fn a_credential_lying_across_the_cut_is_taken_out_whole() {
        let redactor = Redactor::for_config(&config());
        // Wherever the cut falls (before the credential, anywhere inside
        // it, after it), no part of the credential is left: every one of
        // the test's credentials starts with "SECRET".
        for secret in SECRETS {
            for lead in 0..=QUOTED_MAX_CHARS + 2 {
                let text = format!("{}{secret} was rejected", "x".repeat(lead));
                let quoted = redactor.quoted(&text);
                assert!(!quoted.contains("SECRET"), "{lead}: {quoted}");
                assert!(quoted.chars().count() <= QUOTED_MAX_CHARS + 1, "{lead}");
            }
            // The same with a limit of its own.
            let quoted = redactor.quoted_up_to(&format!("leaf {secret}"), 12);
            assert_eq!(quoted, "leaf <redact…");
        }
        // Where there is room, the reader is told something was there.
        assert_eq!(
            redactor.quoted("token SECRET_TOKEN_AAA was rejected"),
            "token <redacted> was rejected"
        );
    }

    #[test]
    fn a_credential_is_found_however_the_text_spaces_it() {
        // A hand-edited value with a tab in it: outside text repeats it as
        // it is, or (once on one line) with a space instead.
        let mut config = config();
        config.client_secret = " SECRET\tCLIENT  BBB ".to_owned();
        let redactor = Redactor::for_config(&config);
        for text in [
            "said SECRET\tCLIENT  BBB.",
            "said SECRET CLIENT BBB.",
            "said SECRET\nCLIENT\r\nBBB.",
        ] {
            let quoted = redactor.quoted(text);
            assert!(!quoted.contains("CLIENT"), "{text:?}: {quoted}");
            let report = Report {
                findings: vec![Finding::ok(Check::Url, text)],
            };
            let printable = report.printable(&redactor);
            assert!(
                !printable.findings[0].message.contains("CLIENT"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_printable_report_has_no_line_breaks_and_no_credentials() {
        let config = config();
        let redactor = Redactor::for_config(&config);
        let report = Report {
            findings: vec![Finding::fail(
                Check::Discord,
                format!(
                    "The application is called “{}”\nand\r\nmore {}",
                    config.discord_token, config.r2.access_key_id
                ),
                format!(
                    "Use {} and {}.",
                    config.client_secret, config.r2.secret_access_key
                ),
            )],
        };
        let printable = report.printable(&redactor);
        let rendered = format!("{}{}", text(&printable), json(&printable));
        for secret in SECRETS {
            assert!(!rendered.contains(secret), "{secret} in {rendered}");
        }
        assert_eq!(rendered.matches(REDACTED).count(), 8, "{rendered}");
        // One finding, one line (and the line of counts).
        assert_eq!(text(&printable).lines().count(), 2);
        // The status and the check are untouched.
        assert_eq!(printable.findings[0].status, Status::Fail);
        assert_eq!(printable.findings[0].check, Check::Discord);
    }

    #[test]
    fn the_placeholder_keys_of_a_folder_are_not_treated_as_credentials() {
        let mut config = config();
        config.r2.access_key_id = "folder".to_owned();
        config.r2.secret_access_key = "machine".to_owned();
        let said = "Storage is a local folder on this machine.";
        // For a bucket those two are its keys, whatever they spell.
        let redactor = Redactor::for_config(&config);
        assert_eq!(
            redactor.quoted(said),
            "Storage is a local <redacted> on this <redacted>."
        );
        // For a folder they are not used, and are no credentials.
        config.r2.endpoint = "file:///data/media".to_owned();
        let redactor = Redactor::for_config(&config);
        assert_eq!(redactor.quoted(said), said);
        // The Discord credentials still are.
        assert_eq!(
            redactor.quoted("token SECRET_TOKEN_AAA, secret SECRET_CLIENT_BBB"),
            "token <redacted>, secret <redacted>"
        );
    }

    #[test]
    fn short_values_are_not_treated_as_credentials() {
        let mut config = config();
        config.client_secret = "leaf".to_owned();
        let redactor = Redactor::for_config(&config);
        assert_eq!(redactor.quoted("leaf.conf loads"), "leaf.conf loads");
        assert!(!format!("{redactor:?}").contains("SECRET"));
    }
}
