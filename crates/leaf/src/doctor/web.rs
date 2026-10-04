//! Check 8 (`--url`): a running leaf, asked over HTTP the way a browser and
//! Discord's proxy ask it: is the web server up, is the bot online, is the
//! gallery's shell served with its assets, does an unknown API path answer
//! as the API does, and does a stored file answer a byte-range request.
//!
//! The media request carries a signed address. The signature is a
//! capability, so the address is never printed: findings name the
//! attachment only.

use serde::Deserialize;

use super::net::{Fetch, Request, Unanswered};
use super::report::{Check, Finding, Redactor};

/// An API path no leaf has: what a client built for another version asks.
const UNKNOWN_API_PATH: &str = "/api/leaf-doctor/no-such-route";
/// The one byte asked of a stored file.
const MEDIA_RANGE: &str = "bytes=0-0";
/// How the placeholder page (no gallery build mounted) can be told.
const PLACEHOLDER_MARK: &str = "leaf is running. The gallery build";

const NOT_REACHABLE: &str = "Check that leaf is up (docker compose ps) and, for the public \
    address, the tunnel or proxy in front of it (guide/07-troubleshooting.md, “Not reachable”).";
const BOT_OFFLINE: &str = "guide/07-troubleshooting.md, “Bot offline”, has the fix for each \
    cause; docker compose logs leaf shows the attempts.";
const NOT_RUN_MODE: &str = "Check that this address is leaf's and that leaf is past setup \
    (docker compose logs leaf).";
const REBUILD: &str = "Rebuild and restart leaf so the gallery and the server are one version \
    (git pull && docker compose up -d --build).";

/// The body of `GET /api/status`.
#[derive(Debug, Deserialize)]
struct StatusBody {
    gateway: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    notice: Option<String>,
}

/// The body of an API error.
#[derive(Debug, Deserialize)]
struct ProblemBody {
    error: String,
}

/// The stored file to ask the server for.
#[derive(Clone, PartialEq, Eq)]
pub enum Media {
    /// One the database lists, and its signed address (path and query).
    Signed {
        /// The attachment's id: what findings name.
        attachment_id: String,
        /// `/api/media/…?exp=…&sig=…`. Never printed.
        path: String,
    },
    /// Nothing to ask for, and why.
    Nothing(String),
}

// The signed path must not reach a log through a `{:?}`.
impl std::fmt::Debug for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Signed { attachment_id, .. } => f
                .debug_struct("Signed")
                .field("attachment_id", attachment_id)
                .field("path", &"<redacted>")
                .finish(),
            Self::Nothing(why) => f.debug_tuple("Nothing").field(why).finish(),
        }
    }
}

/// A base address as the checks use it: `http(s)://host[:port]`, without a
/// trailing slash. `Err` is a sentence for the command line, which repeats
/// what was typed there (no configuration is loaded yet to look for
/// credentials in it).
pub fn base_address(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let usable = reqwest::Url::parse(raw)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
    let Some(url) = usable else {
        return Err(format!(
            "--url takes an address such as https://leaf.example.com or http://127.0.0.1:3777, \
             not “{}”.",
            Redactor::default().quoted(raw)
        ));
    };
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "--url takes the address only, without a path: {}",
            url.origin().ascii_serialization()
        ));
    }
    Ok(url.origin().ascii_serialization())
}

/// Why a request to the server went unanswered, in words.
const fn unanswered(why: Unanswered) -> &'static str {
    match why {
        Unanswered::TimedOut => "nothing answered within 10 seconds",
        Unanswered::Unreachable => "no connection could be made",
        Unanswered::Unsendable => "the address could not be used",
    }
}

/// Check 8.
pub async fn check(
    fetch: &impl Fetch,
    base: &str,
    media: Media,
    redactor: &Redactor,
) -> Vec<Finding> {
    let shown = redactor.quoted(base);
    let health = fetch.send(Request::get(format!("{base}/healthz"))).await;
    let up = health
        .as_ref()
        .is_ok_and(|answer| answer.status == 200 && answer.text().trim() == "ok");
    if !up {
        let message = match health {
            Err(why) => format!("{shown} did not answer: {}.", unanswered(why)),
            Ok(answer) if answer.status == 200 => {
                format!("{shown}/healthz answered, but not as leaf does (it says “ok”).")
            }
            Ok(answer) => format!("{shown}/healthz answered HTTP {}.", answer.status),
        };
        // Nothing else can be learned from a server that is not there.
        return vec![
            Finding::fail(Check::Url, message, NOT_REACHABLE),
            Finding::skip(
                Check::Url,
                "Not checked: the bot's status, the gallery, the API and media, because the \
                 server did not answer.",
            ),
        ];
    }
    let mut findings = vec![Finding::ok(
        Check::Url,
        format!("{shown}/healthz answers: the web server is up."),
    )];
    findings.push(bot_status(fetch, base, redactor).await);
    findings.push(shell(fetch, base, redactor).await);
    findings.push(unknown_api_path(fetch, base, redactor).await);
    findings.push(stored_file(fetch, base, media, redactor).await);
    findings
}

/// `GET /api/status` says the bot is online.
async fn bot_status(fetch: &impl Fetch, base: &str, redactor: &Redactor) -> Finding {
    let answer = match fetch.send(Request::get(format!("{base}/api/status"))).await {
        Ok(answer) => answer,
        Err(why) => {
            return Finding::fail(
                Check::Url,
                format!("/api/status did not answer: {}.", unanswered(why)),
                NOT_REACHABLE,
            );
        }
    };
    let status = (answer.status == 200)
        .then(|| answer.json::<StatusBody>())
        .flatten();
    let Some(status) = status else {
        return Finding::fail(
            Check::Url,
            format!(
                "/api/status did not answer as a running leaf does (HTTP {}).",
                answer.status
            ),
            NOT_RUN_MODE,
        );
    };
    let said = |text: Option<String>| {
        text.map(|text| redactor.quoted(&text))
            .filter(|text| !text.is_empty())
    };
    match status.gateway.as_str() {
        "online" => said(status.notice).map_or_else(
            || Finding::ok(Check::Url, "The bot is online (/api/status)."),
            |notice| {
                Finding::warn(
                    Check::Url,
                    format!("The bot is online, but something needs attention: {notice}"),
                    BOT_OFFLINE,
                )
            },
        ),
        "starting" => Finding::fail(
            Check::Url,
            "The bot is still connecting to Discord (/api/status says “starting”).",
            format!("Run the doctor again in a minute. If it stays: {BOT_OFFLINE}"),
        ),
        "error" => Finding::fail(
            Check::Url,
            format!(
                "The bot is not connected to Discord: {}",
                said(status.detail).unwrap_or_else(|| "leaf gave no reason.".to_owned())
            ),
            BOT_OFFLINE,
        ),
        other => Finding::fail(
            Check::Url,
            format!(
                "/api/status reports a gateway state this doctor does not know (“{}”).",
                redactor.quoted(other)
            ),
            REBUILD,
        ),
    }
}

/// The first hashed asset the shell loads: `/assets/…` up to the closing
/// quote, a script if there is one.
fn first_asset(html: &str) -> Option<&str> {
    let after = |mark: &str| {
        let start = html.find(mark)? + mark.len() - "/assets/".len();
        let rest = html.get(start..)?;
        let end = rest.find('"')?;
        rest.get(..end)
    };
    after("src=\"/assets/").or_else(|| after("\"/assets/"))
}

/// `GET /` is the gallery's HTML shell, and the asset it loads is there.
async fn shell(fetch: &impl Fetch, base: &str, redactor: &Redactor) -> Finding {
    let answer = match fetch.send(Request::get(format!("{base}/"))).await {
        Ok(answer) => answer,
        Err(why) => {
            return Finding::fail(
                Check::Url,
                format!("The gallery's page did not answer: {}.", unanswered(why)),
                NOT_REACHABLE,
            );
        }
    };
    let html = answer.text();
    if answer.status != 200 || !answer.is_type("text/html") {
        return Finding::fail(
            Check::Url,
            format!(
                "The address did not answer with the gallery's page (HTTP {}, {}).",
                answer.status,
                redactor.quoted(answer.content_type.as_deref().unwrap_or("no content type"))
            ),
            NOT_RUN_MODE,
        );
    }
    if html.contains(PLACEHOLDER_MARK) {
        return Finding::fail(
            Check::Url,
            "leaf is serving its placeholder page: the gallery build is not where STATIC_DIR \
             points.",
            "Use the Docker image (it holds the build at /app/dist), or build the gallery (npm \
             run build in activity/) and point STATIC_DIR at its dist directory.",
        );
    }
    let Some(asset) = first_asset(&html).map(ToOwned::to_owned) else {
        return Finding::fail(
            Check::Url,
            "The page at this address is not the gallery's shell: it loads nothing from /assets/ \
             (a leaf in setup mode answers like this).",
            NOT_RUN_MODE,
        );
    };
    let shown = redactor.quoted(&asset);
    match fetch.send(Request::get(format!("{base}{asset}"))).await {
        Ok(found) if found.status == 200 && !found.is_type("text/html") => Finding::ok(
            Check::Url,
            format!("The gallery's shell is served, and so is the asset it loads ({shown})."),
        ),
        Ok(found) => Finding::fail(
            Check::Url,
            format!(
                "The gallery's shell asks for {shown}, which the server does not have (HTTP {}): \
                 the gallery stays on its loading screen.",
                found.status
            ),
            REBUILD,
        ),
        Err(why) => Finding::fail(
            Check::Url,
            format!(
                "The gallery's asset {shown} did not answer: {}.",
                unanswered(why)
            ),
            NOT_REACHABLE,
        ),
    }
}

/// An `/api/…` path no route claims answers 404 in the API's JSON shape,
/// not the gallery's HTML with a 200.
async fn unknown_api_path(fetch: &impl Fetch, base: &str, redactor: &Redactor) -> Finding {
    let request = Request::get(format!("{base}{UNKNOWN_API_PATH}"));
    let answer = match fetch.send(request).await {
        Ok(answer) => answer,
        Err(why) => {
            return Finding::fail(
                Check::Url,
                format!("An unknown API path did not answer: {}.", unanswered(why)),
                NOT_REACHABLE,
            );
        }
    };
    let problem = answer
        .is_type("application/json")
        .then(|| answer.json::<ProblemBody>())
        .flatten();
    match problem {
        Some(problem) if answer.status == 404 && problem.error == "not_found" => Finding::ok(
            Check::Url,
            "An unknown API path answers 404 as JSON, not the gallery's page.",
        ),
        _ => Finding::fail(
            Check::Url,
            format!(
                "An unknown API path answered HTTP {} as {}, not the API's JSON 404. A gallery \
                 from another version would take that for a real answer.",
                answer.status,
                redactor.quoted(answer.content_type.as_deref().unwrap_or("no content type"))
            ),
            "Check that nothing in front of leaf (a proxy rule, a cache) rewrites /api/ paths, \
             and that leaf is up to date.",
        ),
    }
}

/// A stored file's signed address answers one byte range with 206: what a
/// video player needs before it will play.
async fn stored_file(fetch: &impl Fetch, base: &str, media: Media, redactor: &Redactor) -> Finding {
    let (attachment, path) = match media {
        Media::Signed {
            attachment_id,
            path,
        } => (redactor.quoted(&attachment_id), path),
        Media::Nothing(why) => {
            return Finding::skip(
                Check::Url,
                format!("Media was not requested: {}.", redactor.quoted(&why)),
            );
        }
    };
    let request = Request::get(format!("{base}{path}")).with_range(MEDIA_RANGE);
    let answer = match fetch.send(request).await {
        Ok(answer) => answer,
        Err(why) => {
            return Finding::fail(
                Check::Url,
                format!(
                    "The file of attachment {attachment} did not answer: {}.",
                    unanswered(why)
                ),
                NOT_REACHABLE,
            );
        }
    };
    let ranged = answer
        .content_range
        .as_deref()
        .is_some_and(|range| range.trim().starts_with("bytes 0-0/"));
    match answer.status {
        206 if ranged => Finding::ok(
            Check::Url,
            format!(
                "A stored file (attachment {attachment}) answers a byte-range request with 206."
            ),
        ),
        206 => Finding::fail(
            Check::Url,
            format!(
                "The file of attachment {attachment} answered 206 without the range that was \
                 asked for ({}).",
                redactor.quoted(
                    answer
                        .content_range
                        .as_deref()
                        .unwrap_or("no Content-Range")
                )
            ),
            "Check what sits between this address and leaf (proxy, cache rule): it must pass \
             Range requests through.",
        ),
        200 => Finding::fail(
            Check::Url,
            format!(
                "The file of attachment {attachment} came back whole (HTTP 200) although one \
                 byte was asked for. Videos will not play on iPhone and iPad without byte \
                 ranges."
            ),
            "Check what sits between this address and leaf (proxy, cache rule): it must pass \
             the Range header through.",
        ),
        403 => Finding::fail(
            Check::Url,
            format!(
                "The server refused the signed address of attachment {attachment} (HTTP 403): \
                 it signs with another client secret than this leaf.conf holds."
            ),
            "Check that --url and DATA_DIR are the same install, and that leaf was restarted \
             after the last --reconfigure.",
        ),
        404 => Finding::fail(
            Check::Url,
            format!(
                "The database lists a stored file for attachment {attachment}, but the server \
                 answered 404: the file is not in the bucket, or the server uses another \
                 database."
            ),
            "Check that --url and DATA_DIR are the same install; if they are, look for the \
             day in the gallery and archive it again.",
        ),
        status => Finding::fail(
            Check::Url,
            format!(
                "The file of attachment {attachment} answered HTTP {status}: leaf could not \
                 fetch it from storage."
            ),
            "docker compose logs leaf has the cause (“media fetch from store failed”); leaf \
             doctor --only storage checks the bucket.",
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use super::*;
    use crate::doctor::net::Answer;
    use crate::doctor::report::Status;
    use crate::doctor::testing::{
        Canned, SECRETS, assert_no_secrets, printed, redactor, statuses, typed,
    };

    const BASE: &str = "https://leaf.example.com";
    const SHELL: &str = r#"<!doctype html><html><head>
        <link rel="preload" href="/assets/fraunces-latin-wght-ukD16Tqj.woff2" as="font" />
        <script type="module" crossorigin src="/assets/index-C3kq9x.js"></script>
        </head><body><div id="app"></div></body></html>"#;
    const SIGNED: &str = "/api/media/7000100006600?exp=1779408000&sig=SIGNATURE_ZZZ";

    fn at(path: &str) -> String {
        format!("{BASE}{path}")
    }

    fn part() -> Answer {
        Answer {
            status: 206,
            content_type: Some("video/mp4".to_owned()),
            content_range: Some("bytes 0-0/155042".to_owned()),
            body: vec![0],
        }
    }

    fn media() -> Media {
        Media::Signed {
            attachment_id: "7000100006600".to_owned(),
            path: SIGNED.to_owned(),
        }
    }

    /// A leaf in run mode with everything in order.
    fn healthy() -> Canned {
        Canned::new()
            .plain(&at("/healthz"), 200, "ok")
            .json(&at("/api/status"), 200, r#"{"gateway":"online"}"#)
            .html(&at("/"), 200, SHELL)
            .answer(
                &at("/assets/index-C3kq9x.js"),
                typed(200, "text/javascript", "console.log('leaf')"),
            )
            .json(&at(UNKNOWN_API_PATH), 404, r#"{"error":"not_found"}"#)
            .answer(&at(SIGNED), part())
    }

    #[tokio::test]
    async fn a_healthy_server_is_five_oks() {
        let server = healthy();
        let findings = check(&server, BASE, media(), &redactor()).await;
        assert_eq!(statuses(&findings), [Status::Ok; 5], "{findings:?}");
        assert_eq!(
            server.asked(),
            [
                at("/healthz"),
                at("/api/status"),
                at("/"),
                at("/assets/index-C3kq9x.js"),
                at(UNKNOWN_API_PATH),
                at(SIGNED),
            ]
        );
        // Only the media request asks for a range, and for one byte.
        let ranges: Vec<_> = server.requests().iter().map(|r| r.range).collect();
        assert_eq!(ranges, [None, None, None, None, None, Some("bytes=0-0")]);
        assert!(findings[4].message.contains("attachment 7000100006600"));
    }

    #[tokio::test]
    async fn the_signed_address_is_never_printed() {
        // Whatever the media request comes to, the signature stays out.
        let answers = [
            Ok(part()),
            Ok(typed(200, "video/mp4", "whole")),
            Ok(typed(403, "application/json", r#"{"error":"forbidden"}"#)),
            Ok(typed(404, "application/json", r#"{"error":"not_found"}"#)),
            Ok(typed(500, "application/json", r#"{"error":"internal"}"#)),
            Err(Unanswered::TimedOut),
        ];
        for answer in answers {
            let server = match answer.clone() {
                Ok(answer) => healthy().answer(&at(SIGNED), answer),
                Err(why) => healthy().unanswered(&at(SIGNED), why),
            };
            let findings = check(&server, BASE, media(), &redactor()).await;
            let printed = printed(&findings);
            assert!(!printed.contains("SIGNATURE_ZZZ"), "{answer:?}: {printed}");
            assert!(!printed.contains("sig="), "{answer:?}: {printed}");
            for secret in SECRETS {
                assert!(!printed.contains(secret), "{answer:?}: {printed}");
            }
            assert_no_secrets(&findings);
        }
        assert!(!format!("{:?}", media()).contains("SIGNATURE_ZZZ"));
    }

    #[tokio::test]
    async fn a_server_that_is_not_there_fails_once_and_skips_the_rest() {
        // (what /healthz does, what the finding says)
        let cases = [
            (
                healthy().unanswered(&at("/healthz"), Unanswered::Unreachable),
                "no connection could be made",
            ),
            (
                healthy().unanswered(&at("/healthz"), Unanswered::TimedOut),
                "within 10 seconds",
            ),
            (
                healthy().plain(&at("/healthz"), 502, "Bad gateway"),
                "HTTP 502",
            ),
            (
                healthy().html(&at("/healthz"), 200, SHELL),
                "not as leaf does",
            ),
        ];
        for (server, says) in cases {
            let findings = check(&server, BASE, media(), &redactor()).await;
            assert_eq!(statuses(&findings), [Status::Fail, Status::Skip], "{says}");
            assert!(findings[0].message.contains(says), "{findings:?}");
            assert!(
                findings[0]
                    .next
                    .as_deref()
                    .unwrap()
                    .contains("docker compose ps")
            );
            // Nothing else was asked.
            assert_eq!(server.asked(), [at("/healthz")], "{says}");
        }
    }

    #[tokio::test]
    async fn the_bot_must_be_online() {
        // (body of /api/status, status, what the finding says)
        let cases = [
            (r#"{"gateway":"online"}"#, Status::Ok, "The bot is online"),
            (
                r#"{"gateway":"online","notice":"Discord has not accepted leaf's command list."}"#,
                Status::Warn,
                "something needs attention: Discord has not accepted leaf's command list.",
            ),
            (
                r#"{"gateway":"starting"}"#,
                Status::Fail,
                "still connecting",
            ),
            (
                r#"{"gateway":"error","detail":"Discord rejected the bot token."}"#,
                Status::Fail,
                "not connected to Discord: Discord rejected the bot token.",
            ),
            (
                r#"{"gateway":"error"}"#,
                Status::Fail,
                "leaf gave no reason.",
            ),
            (
                r#"{"gateway":"resting"}"#,
                Status::Fail,
                "does not know (“resting”)",
            ),
            (
                r#"{"status":"fine"}"#,
                Status::Fail,
                "not answer as a running leaf does",
            ),
        ];
        for (body, status, says) in cases {
            let server = healthy().json(&at("/api/status"), 200, body);
            let findings = check(&server, BASE, media(), &redactor()).await;
            assert_eq!(findings[1].status, status, "{body}");
            assert!(findings[1].message.contains(says), "{findings:?}");
            // The other parts are still checked.
            assert_eq!(findings.len(), 5, "{body}");
        }
        // A leaf in setup mode has no such route.
        let server = healthy().html(&at("/api/status"), 404, "Not found");
        let findings = check(&server, BASE, media(), &redactor()).await;
        assert_eq!(findings[1].status, Status::Fail);
        assert!(findings[1].message.contains("HTTP 404"));
    }

    #[tokio::test]
    async fn a_status_detail_cannot_break_the_line() {
        let body = r#"{"gateway":"error","detail":"first\nsecond \u001b[2J third"}"#;
        let server = healthy().json(&at("/api/status"), 200, body);
        let findings = check(&server, BASE, media(), &redactor()).await;
        assert!(
            findings[1].message.ends_with("first second [2J third"),
            "{findings:?}"
        );
    }

    #[tokio::test]
    async fn a_credential_where_a_long_detail_is_cut_leaves_no_part_behind() {
        // The detail is cut at 160 characters. Wherever the bot token lies
        // in it (before the cut, across it, after it), none of the token is
        // in the finding, even before the report's own pass over it.
        for lead in [140, 145, 150, 159, 160, 170] {
            let detail = format!("{} SECRET_TOKEN_AAA was rejected", "x".repeat(lead));
            let body = format!(r#"{{"gateway":"error","detail":"{detail}"}}"#);
            let server = healthy().json(&at("/api/status"), 200, &body);
            let findings = check(&server, BASE, media(), &redactor()).await;
            assert_eq!(findings[1].status, Status::Fail, "{lead}");
            assert!(!findings[1].message.contains("SECRET"), "{findings:?}");
            assert!(findings[1].message.ends_with('…'), "{findings:?}");
            assert!(!printed(&findings).contains("SECRET"), "{lead}");
        }
    }

    #[tokio::test]
    async fn the_shell_must_be_the_gallery_and_its_asset_must_be_served() {
        let placeholder = "<!doctype html><title>leaf</title><div>leaf is running. The gallery \
                           build isn’t mounted here</div>";
        let setup_page = "<!doctype html><title>leaf setup</title><form></form>";
        // (server, what the finding says)
        let cases = [
            (
                healthy().html(&at("/"), 200, placeholder),
                "placeholder page",
            ),
            (
                healthy().html(&at("/"), 200, setup_page),
                "not the gallery's shell",
            ),
            (healthy().html(&at("/"), 502, "Bad gateway"), "HTTP 502"),
            (
                healthy().json(&at("/"), 200, "{}"),
                "did not answer with the gallery's page (HTTP 200, application/json)",
            ),
            // An asset from an older build: the server has no such file.
            (
                healthy().plain(&at("/assets/index-C3kq9x.js"), 404, ""),
                "which the server does not have (HTTP 404)",
            ),
            // Or answers the shell for it, as a careless fallback would.
            (
                healthy().html(&at("/assets/index-C3kq9x.js"), 200, SHELL),
                "which the server does not have (HTTP 200)",
            ),
            (
                healthy().unanswered(&at("/assets/index-C3kq9x.js"), Unanswered::TimedOut),
                "did not answer",
            ),
        ];
        for (server, says) in cases {
            let findings = check(&server, BASE, media(), &redactor()).await;
            assert_eq!(findings[2].status, Status::Fail, "{says}");
            assert!(findings[2].message.contains(says), "{findings:?}");
            assert_eq!(findings.len(), 5, "{says}");
        }
    }

    #[test]
    fn the_first_asset_is_the_script_when_there_is_one() {
        assert_eq!(first_asset(SHELL), Some("/assets/index-C3kq9x.js"));
        let styles_only = r#"<link rel="stylesheet" href="/assets/app-1.css">"#;
        assert_eq!(first_asset(styles_only), Some("/assets/app-1.css"));
        assert_eq!(first_asset("<p>/assets/ in prose</p>"), None);
        assert_eq!(first_asset(r#"<script src="/assets/unclosed"#), None);
        assert_eq!(first_asset(""), None);
    }

    #[tokio::test]
    async fn an_unknown_api_path_must_be_a_json_404() {
        // (answer, passes)
        let cases = [
            (
                typed(404, "application/json", r#"{"error":"not_found"}"#),
                true,
            ),
            // The gallery's shell with a 200: what a fallback would send.
            (typed(200, "text/html", SHELL), false),
            (typed(404, "text/html", "<h1>Not found</h1>"), false),
            (
                typed(404, "application/json", r#"{"error":"forbidden"}"#),
                false,
            ),
            (
                typed(200, "application/json", r#"{"error":"not_found"}"#),
                false,
            ),
            (typed(404, "application/json", "not json"), false),
        ];
        for (answer, passes) in cases {
            let server = healthy().answer(&at(UNKNOWN_API_PATH), answer.clone());
            let findings = check(&server, BASE, media(), &redactor()).await;
            let expected = if passes { Status::Ok } else { Status::Fail };
            assert_eq!(findings[3].status, expected, "{answer:?}");
        }
    }

    #[tokio::test]
    async fn a_stored_file_must_answer_a_range_with_206() {
        let wrong_range = Answer {
            content_range: Some("bytes 0-155041/155042".to_owned()),
            ..part()
        };
        let no_range = Answer {
            content_range: None,
            ..part()
        };
        // (answer, status, what the finding says)
        let cases = [
            (part(), Status::Ok, "answers a byte-range request with 206"),
            (
                wrong_range,
                Status::Fail,
                "without the range that was asked for",
            ),
            (no_range, Status::Fail, "no Content-Range"),
            (
                typed(200, "video/mp4", "whole"),
                Status::Fail,
                "came back whole",
            ),
            (
                typed(403, "application/json", "{}"),
                Status::Fail,
                "another client secret",
            ),
            (
                typed(404, "application/json", "{}"),
                Status::Fail,
                "not in the bucket",
            ),
            (
                typed(500, "application/json", "{}"),
                Status::Fail,
                "HTTP 500",
            ),
        ];
        for (answer, status, says) in cases {
            let server = healthy().answer(&at(SIGNED), answer.clone());
            let findings = check(&server, BASE, media(), &redactor()).await;
            assert_eq!(findings[4].status, status, "{answer:?}");
            assert!(findings[4].message.contains(says), "{findings:?}");
        }
    }

    #[tokio::test]
    async fn with_no_stored_file_the_media_part_is_skipped_not_passed() {
        let server = healthy();
        let nothing = Media::Nothing("the database has no stored media yet".to_owned());
        let findings = check(&server, BASE, nothing, &redactor()).await;
        assert_eq!(
            statuses(&findings),
            [Status::Ok, Status::Ok, Status::Ok, Status::Ok, Status::Skip]
        );
        assert_eq!(
            findings[4].message,
            "Media was not requested: the database has no stored media yet."
        );
        assert!(!server.asked().iter().any(|url| url.contains("/api/media/")));
    }

    #[test]
    fn a_base_address_is_an_origin() {
        assert_eq!(
            base_address(" https://leaf.example.com/ ").as_deref(),
            Ok("https://leaf.example.com")
        );
        assert_eq!(
            base_address("http://127.0.0.1:3777").as_deref(),
            Ok("http://127.0.0.1:3777")
        );
        assert_eq!(
            base_address("https://Leaf.Example.com:8443").as_deref(),
            Ok("https://leaf.example.com:8443")
        );
        for bad in [
            "leaf.example.com",
            "ftp://leaf.example.com",
            "",
            "--json",
            "https://",
        ] {
            let err = base_address(bad).unwrap_err();
            assert!(err.contains("--url takes an address"), "{bad}: {err}");
        }
        for with_path in [
            "https://leaf.example.com/admin",
            "https://leaf.example.com/?a=1",
        ] {
            let err = base_address(with_path).unwrap_err();
            assert!(
                err.ends_with("without a path: https://leaf.example.com"),
                "{err}"
            );
        }
    }
}
