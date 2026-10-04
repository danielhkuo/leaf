//! `leaf doctor`'s command line, read by hand like the binary's other
//! argument (`--reconfigure`).

use std::path::PathBuf;

use super::report::Check;
use super::web;

/// What `leaf doctor --help` prints.
pub const USAGE: &str = "\
leaf doctor: check a configured leaf, live. It starts neither the server nor the
bot, never writes to the database and never prints a credential.

Usage: leaf doctor [--only <check,...>] [--json] [--db-copy <path>] [--url [<address>]]

Checks (each prints ok, warn, FAIL or skip, a sentence, and the next step):
  config     leaf.conf loads; the public URL is https
  discord    the bot token is accepted and belongs to the configured application;
             the client ID and secret are accepted together
  commands   leaf's commands are registered (globally, or in DEV_GUILD_ID),
             nothing stale is, and the Activity's Entry Point command exists
  gateway    Discord's gateway answers and session starts remain
  storage    write, read, ranged read and delete of a test object in the bucket
             (or of a test file in the folder, when storage is a local folder)
  database   leaf.db opens read-only and has exactly this leaf's migrations
  db-copy    with --db-copy <path>: migrate a copy of that database in a temp
             directory, check its integrity and foreign keys, compare row counts
  url        with --url: /healthz, /api/status, the gallery's shell, an unknown
             API path and a stored file's byte range, asked of a running leaf

Options:
  --only <check,...>   run only these checks
  --json               print one JSON object instead of lines
  --db-copy <path>     also run db-copy on this SQLite file (never written to)
  --url [<address>]    also run url against this address; without one, against
                       the public URL in leaf.conf
  -h, --help           print this

Reads DATA_DIR (default ./data) and DEV_GUILD_ID like leaf itself.
Exit status: 0 when nothing failed, 1 when a check failed, 2 for a bad command line.
";

/// The server the `url` check asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlTarget {
    /// The public URL in `leaf.conf`.
    Configured,
    /// This address (already reduced to its origin).
    Base(String),
}

/// One run's options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// The checks `--only` named; `None` runs the standing ones.
    pub only: Option<Vec<Check>>,
    /// Print JSON instead of lines.
    pub json: bool,
    /// The database file to rehearse the migrations on.
    pub db_copy: Option<PathBuf>,
    /// The server to ask over HTTP.
    pub url: Option<UrlTarget>,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run these checks.
    Run(Options),
    /// Print the usage text.
    Help,
}

impl Options {
    /// The checks this run makes, in order. `--db-copy` and `--url` add
    /// their check to whatever else is selected.
    pub fn selected(&self) -> Vec<Check> {
        let mut selected = self
            .only
            .clone()
            .unwrap_or_else(|| Check::STANDING.to_vec());
        if self.db_copy.is_some() {
            selected.push(Check::DbCopy);
        }
        if self.url.is_some() {
            selected.push(Check::Url);
        }
        selected.sort_unstable();
        selected.dedup();
        selected
    }
}

/// The names `--only` takes, for an error message.
fn check_names() -> String {
    let names: Vec<&str> = Check::ALL.iter().map(|check| check.name()).collect();
    names.join(", ")
}

/// Reads the arguments after `doctor`. `Err` is a sentence saying what is
/// wrong with them.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut options = Options::default();
    let mut args = args.iter().map(String::as_str).peekable();
    while let Some(arg) = args.next() {
        // `--name=value` and `--name value` are the same thing.
        let (name, mut attached) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value)),
            _ => (arg, None),
        };
        // The value of an option that must have one.
        let mut value = |what: &str| {
            attached
                .take()
                .or_else(|| args.next_if(|next| !next.starts_with('-')))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{name} needs {what}."))
        };
        match name {
            "-h" | "--help" => return Ok(Command::Help),
            "--json" => options.json = true,
            "--only" => {
                let list = value("a list of checks, such as --only config,storage")?;
                let only = options.only.get_or_insert_with(Vec::new);
                for check in list.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                    only.push(Check::from_name(check).ok_or_else(|| {
                        format!("“{check}” is not one of the checks: {}.", check_names())
                    })?);
                }
            }
            "--db-copy" => {
                options.db_copy = Some(PathBuf::from(value("the path of a database file")?));
            }
            "--url" => {
                // The one option whose value may be left out.
                options.url = Some(match value("") {
                    Ok(address) => UrlTarget::Base(web::base_address(address)?),
                    Err(_) => UrlTarget::Configured,
                });
            }
            other => {
                return Err(format!("leaf doctor has no option “{other}”."));
            }
        }
        if attached.is_some() {
            return Err(format!("{name} takes no value."));
        }
    }
    let only = options.only.as_deref().unwrap_or_default();
    if options.only.is_some() && only.is_empty() {
        return Err("--only needs at least one check.".to_owned());
    }
    if only.contains(&Check::DbCopy) && options.db_copy.is_none() {
        return Err("The db-copy check needs the file to copy: add --db-copy <path>.".to_owned());
    }
    // Asking for the url check by name is asking for the configured address.
    if only.contains(&Check::Url) && options.url.is_none() {
        options.url = Some(UrlTarget::Configured);
    }
    Ok(Command::Run(options))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    fn run(args: &[&str]) -> Result<Options, String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        match parse(&args)? {
            Command::Run(options) => Ok(options),
            Command::Help => Err("help".to_owned()),
        }
    }

    #[test]
    fn no_arguments_run_the_standing_checks_as_text() {
        let options = run(&[]).unwrap();
        assert_eq!(options, Options::default());
        assert_eq!(options.selected(), Check::STANDING);
        assert!(!options.json);
    }

    #[test]
    fn only_limits_the_run_and_keeps_the_checks_in_order() {
        let options = run(&["--only", "storage,config"]).unwrap();
        assert_eq!(options.selected(), [Check::Config, Check::Storage]);
        // Both spellings, repeats and spaces.
        let options = run(&["--only=gateway", "--only", " config , gateway ,"]).unwrap();
        assert_eq!(options.selected(), [Check::Config, Check::Gateway]);
    }

    #[test]
    fn db_copy_and_url_add_their_check() {
        let options = run(&[
            "--db-copy",
            "/tmp/leaf.db",
            "--url",
            "https://leaf.example.com/",
        ])
        .unwrap();
        assert_eq!(options.db_copy, Some(PathBuf::from("/tmp/leaf.db")));
        assert_eq!(
            options.url,
            Some(UrlTarget::Base("https://leaf.example.com".to_owned()))
        );
        let mut expected = Check::STANDING.to_vec();
        expected.extend([Check::DbCopy, Check::Url]);
        assert_eq!(options.selected(), expected);

        // Together with --only they are added to what it names.
        let options = run(&["--only", "database", "--db-copy=/tmp/leaf.db"]).unwrap();
        assert_eq!(options.selected(), [Check::Database, Check::DbCopy]);
        let options = run(&["--only", "db-copy", "--db-copy", "x.db"]).unwrap();
        assert_eq!(options.selected(), [Check::DbCopy]);
    }

    #[test]
    fn url_without_an_address_means_the_configured_one() {
        for args in [
            &["--url"][..],
            &["--url", "--json"],
            &["--json", "--url"],
            &["--only", "url"],
        ] {
            let options = run(args).unwrap();
            assert_eq!(options.url, Some(UrlTarget::Configured), "{args:?}");
            assert!(options.selected().contains(&Check::Url), "{args:?}");
        }
        assert!(run(&["--url", "--json"]).unwrap().json);
        let options = run(&["--url=http://127.0.0.1:3777", "--json"]).unwrap();
        assert_eq!(
            options.url,
            Some(UrlTarget::Base("http://127.0.0.1:3777".to_owned()))
        );
        assert!(options.json);
    }

    #[test]
    fn a_bad_command_line_says_what_is_wrong() {
        // (arguments, what the error says)
        let cases = [
            (&["--only"][..], "--only needs a list of checks"),
            (&["--only", "--json"], "--only needs a list of checks"),
            (&["--only="], "--only needs a list of checks"),
            (&["--only", ","], "--only needs at least one check"),
            (
                &["--only", "config,r2"],
                "“r2” is not one of the checks: config, discord",
            ),
            (&["--only", "db-copy"], "add --db-copy <path>"),
            (
                &["--db-copy"],
                "--db-copy needs the path of a database file",
            ),
            (
                &["--url", "leaf.example.com"],
                "--url takes an address such as",
            ),
            (&["--url=https://leaf.example.com/admin"], "without a path"),
            (&["--json=yes"], "--json takes no value"),
            (&["--reconfigure"], "no option “--reconfigure”"),
            (&["storage"], "no option “storage”"),
        ];
        for (args, says) in cases {
            let err = run(args).unwrap_err();
            assert!(err.contains(says), "{args:?}: {err}");
        }
    }

    #[test]
    fn help_wins_and_names_every_check_and_option() {
        for args in [&["--help"][..], &["-h"], &["--json", "--help", "--bogus"]] {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            assert_eq!(parse(&args), Ok(Command::Help));
        }
        for check in Check::ALL {
            assert!(
                USAGE.contains(&format!("\n  {:<9}", check.name())),
                "{check:?}"
            );
        }
        for option in [
            "--only",
            "--json",
            "--db-copy",
            "--url",
            "DATA_DIR",
            "DEV_GUILD_ID",
        ] {
            assert!(USAGE.contains(option), "{option}");
        }
    }
}
