//! Stand-ins shared by the doctor's tests: a configuration whose credentials
//! are recognisable, and an HTTP client that answers from a table and never
//! opens a connection.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test stand-ins may panic"
)]

use std::sync::Mutex;

use leaf_core::config::{R2Config, Tier1Config};

use super::net::{Answer, Fetch, Request, Unanswered};
use super::report::{Finding, Redactor, Report, Status};

/// The credentials in [`config`]. None may appear in anything printed.
pub const SECRETS: [&str; 4] = [
    "SECRET_TOKEN_AAA",
    "SECRET_CLIENT_BBB",
    "SECRET_KEYID_CCC",
    "SECRET_KEY_DDD",
];

/// The stand-in for Discord's REST base.
pub const API: &str = "https://discord.test/api";

/// A loaded configuration for application 123.
pub fn config() -> Tier1Config {
    Tier1Config {
        discord_token: "SECRET_TOKEN_AAA".to_owned(),
        client_id: "123".to_owned(),
        client_secret: "SECRET_CLIENT_BBB".to_owned(),
        public_url: "https://leaf.example.com".to_owned(),
        r2: R2Config {
            endpoint: "https://acc.r2.cloudflarestorage.com".to_owned(),
            bucket: "leaf-media".to_owned(),
            access_key_id: "SECRET_KEYID_CCC".to_owned(),
            secret_access_key: "SECRET_KEY_DDD".to_owned(),
        },
    }
}

/// The redactor a run with [`config`] has: it knows [`SECRETS`].
pub fn redactor() -> Redactor {
    Redactor::for_config(&config())
}

/// The findings as they would be printed, text and JSON together, for
/// assertions about what the output may and may not contain.
pub fn printed(findings: &[Finding]) -> String {
    let report = Report {
        findings: findings.to_vec(),
    }
    .printable(&redactor());
    let mut out = Vec::new();
    report.write_text(&mut out).unwrap();
    report.write_json(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// Asserts that nothing in `findings` repeats a credential, even before the
/// redactor has seen it.
pub fn assert_no_secrets(findings: &[Finding]) {
    for finding in findings {
        let text = format!("{finding:?}");
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
}

/// The statuses of `findings`, in order.
pub fn statuses(findings: &[Finding]) -> Vec<Status> {
    findings.iter().map(|f| f.status).collect()
}

/// Which requests an entry of the table answers.
enum Route {
    /// Exactly this address.
    Exact(String),
    /// Any address that starts like this (a signed address has a query
    /// that changes with the clock).
    Prefix(String),
}

/// An HTTP client that answers from a table. A request the table does not
/// have is a mistake in the test and panics.
#[derive(Default)]
pub struct Canned {
    routes: Vec<(Route, Result<Answer, Unanswered>)>,
    seen: Mutex<Vec<Request>>,
}

impl Canned {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers `url` with `status` and a JSON body. A later entry for the
    /// same address replaces an earlier one, so a test starts from a
    /// healthy table and breaks one thing.
    #[must_use]
    pub fn json(self, url: &str, status: u16, body: &str) -> Self {
        self.answer(url, typed(status, "application/json", body))
    }

    /// Answers `url` with `status` and an HTML body.
    #[must_use]
    pub fn html(self, url: &str, status: u16, body: &str) -> Self {
        self.answer(url, typed(status, "text/html; charset=utf-8", body))
    }

    /// Answers `url` with `status` and a plain-text body.
    #[must_use]
    pub fn plain(self, url: &str, status: u16, body: &str) -> Self {
        self.answer(url, typed(status, "text/plain; charset=utf-8", body))
    }

    /// Answers `url` with `answer`.
    #[must_use]
    pub fn answer(mut self, url: &str, answer: Answer) -> Self {
        self.routes.push((Route::Exact(url.to_owned()), Ok(answer)));
        self
    }

    /// Answers every address starting with `prefix` with `answer`.
    #[must_use]
    pub fn answer_prefix(mut self, prefix: &str, answer: Answer) -> Self {
        self.routes
            .push((Route::Prefix(prefix.to_owned()), Ok(answer)));
        self
    }

    /// Leaves `url` without an answer, for this reason.
    #[must_use]
    pub fn unanswered(mut self, url: &str, why: Unanswered) -> Self {
        self.routes.push((Route::Exact(url.to_owned()), Err(why)));
        self
    }

    /// Every request made so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    /// The addresses asked so far, in order.
    pub fn asked(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.url).collect()
    }
}

/// An answer with a status, a content type and a body.
pub fn typed(status: u16, content_type: &str, body: &str) -> Answer {
    Answer {
        status,
        content_type: Some(content_type.to_owned()),
        content_range: None,
        body: body.as_bytes().to_vec(),
    }
}

impl Fetch for Canned {
    async fn send(&self, request: Request) -> Result<Answer, Unanswered> {
        let found = self
            .routes
            .iter()
            .rev()
            .find(|(route, _)| match route {
                Route::Exact(url) => *url == request.url,
                Route::Prefix(prefix) => request.url.starts_with(prefix.as_str()),
            })
            .map(|(_, answer)| answer.clone());
        let url = request.url.clone();
        self.seen.lock().unwrap().push(request);
        found.unwrap_or_else(|| panic!("the test has no answer for {url}"))
    }
}
