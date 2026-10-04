//! The e2e server's own tests: what its doc comment and its manifest promise
//! the browser suites, checked through the same router a browser talks to.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "tests may panic; JSON indexing is fine in assertions"
)]

use std::collections::BTreeSet;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use leaf_core::db::LaunchIntentRepo;
use leaf_server::api::auth::{AuthError, DiscordApi, LookupError, now_unix};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::discord::StubDiscord;
use crate::seed::{self, LOG_CHANNEL_ID, MAIN_GUILD_ID, UNSET_GUILD_ID};
use crate::{Harness, app};

/// A guild id the seed does not have.
const UNKNOWN_GUILD_ID: &str = "900000000000000099";

/// A delay no test run outlasts (an hour): a call held with it is answered
/// only when something lets go of it.
const NEVER_MS: u64 = 3_600_000;

/// The answer of a request the stub Discord has just been told to let go
/// of. It is there at once; the limit only makes a call that was not let go
/// of a failure here and now, not a test that hangs for [`NEVER_MS`].
async fn let_go<T>(request: impl Future<Output = T>) -> T {
    let answer = tokio::time::timeout(Duration::from_secs(30), request).await;
    assert!(answer.is_ok(), "the stub Discord is still holding the call");
    answer.unwrap()
}

/// leaf-server keeps some caches for the whole process, keyed by ids that
/// are the same in every test here. One test at a time keeps what one cached
/// from answering for another.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A freshly seeded server, never bound to a port.
struct Server {
    app: Router,
    harness: Arc<Harness>,
}

/// The server for one test, and the lock that keeps other tests out while
/// the test holds it.
async fn server() -> (Server, tokio::sync::MutexGuard<'static, ()>) {
    let alone = ONE_AT_A_TIME.lock().await;
    let harness = Harness::start("http://127.0.0.1:3799").await.unwrap();
    let server = Server {
        app: app(Arc::clone(&harness)),
        harness,
    };
    (server, alone)
}

/// A path under the main guild's API.
fn main_guild(rest: &str) -> String {
    format!("/api/guilds/{MAIN_GUILD_ID}{rest}")
}

async fn bytes_of(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

impl Server {
    async fn send(&self, request: Request<Body>) -> Response {
        self.app.clone().oneshot(request).await.unwrap()
    }

    /// One JSON request: the status and the parsed body (`null` when empty).
    async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let body = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
        let response = self.send(request.body(body).unwrap()).await;
        let status = response.status();
        let bytes = bytes_of(response).await;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    async fn get(&self, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.call(Method::GET, path, token, None).await
    }

    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, path, token, Some(body)).await
    }

    /// A GET that has to succeed.
    async fn read(&self, path: &str, token: Option<&str>) -> Value {
        let (status, body) = self.get(path, token).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
        body
    }

    /// A control POST that has to succeed.
    async fn control(&self, path: &str, body: Value) -> Value {
        let (status, answer) = self.post(path, None, body).await;
        assert!(status.is_success(), "POST {path}: {status} {answer}");
        answer
    }

    /// Signs in through the real `POST /api/token`.
    async fn sign_in(&self, persona: &str) -> String {
        let code = json!({ "code": format!("code-{persona}") });
        let (status, body) = self.post("/api/token", None, code).await;
        assert_eq!(status, StatusCode::OK, "sign-in as {persona}: {body}");
        assert_eq!(body["access_token"], format!("access-{persona}"));
        body["token"].as_str().unwrap().to_owned()
    }

    /// The id the manifest gives for a seeded series.
    async fn series_id(&self, key: &str) -> i64 {
        self.read("/__e2e/state", None).await["series"][key]["id"]
            .as_i64()
            .unwrap()
    }

    /// The names of the series a persona's gallery lists in the main guild.
    async fn listed(&self, persona: &str) -> Vec<String> {
        let token = self.sign_in(persona).await;
        self.read(&main_guild("/series"), Some(&token))
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_owned())
            .collect()
    }

    /// How many times the stub Discord has been asked for `op`.
    async fn discord_calls(&self, op: &str) -> u64 {
        self.read("/__e2e/discord", None).await["calls"][op]
            .as_u64()
            .unwrap_or(0)
    }

    /// Runs `request` until the stub Discord is holding at least `calls`
    /// calls (`true`) or the request has been answered (`false`): a wait on
    /// whichever happens, not on a clock. A held request is left running,
    /// to be awaited for its answer once something lets go of it.
    async fn discord_holds<T>(
        &self,
        calls: u64,
        request: Pin<&mut impl Future<Output = T>>,
    ) -> bool {
        let mut held = self.harness.discord.held();
        tokio::select! {
            biased;
            _ = request => false,
            seen = held.wait_for(|&now| now >= calls) => seen.is_ok(),
        }
    }

    /// A panel token for `persona`, minted rather than signed in for, so no
    /// call to the stub Discord is spent on it.
    async fn admin_token(&self, persona: &str) -> String {
        let minted = self
            .control("/__e2e/admin-session", json!({ "persona": persona }))
            .await;
        minted["token"].as_str().unwrap().to_owned()
    }

    /// Follows one redirect and returns where it points.
    async fn redirect_from(&self, path: &str) -> String {
        let response = self
            .send(Request::get(path).body(Body::empty()).unwrap())
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "GET {path}");
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned()
    }
}

#[tokio::test]
async fn the_long_series_has_the_shape_the_manifest_describes() {
    let (s, _alone) = server().await;
    let viewer = s.sign_in("viewer").await;
    let manifest = s.read("/__e2e/state", None).await;
    let long = manifest["series"]["long"]["id"].as_i64().unwrap();

    let index = s
        .read(&main_guild(&format!("/series/{long}/days")), Some(&viewer))
        .await;
    let days = index.as_array().unwrap();
    assert_eq!(days.len(), 150);
    assert_eq!(manifest["series"]["long"]["archived_days"], 150);
    assert_eq!(days[0]["day"], 1);
    assert_eq!(days[0]["local_date"], "2025-11-13");
    assert_eq!(days[149]["day"], 187);
    assert_eq!(days[149]["local_date"], "2026-05-18");
    let day = |n: i64| days.iter().find(|d| d["day"] == n);
    let date = |d: &Value| d["local_date"].as_str().unwrap().to_owned();

    // Gaps: the day numbers the manifest lists are absent, their neighbours
    // are not, and one of the gaps is the whole of February.
    for gap in manifest["long_series"]["gaps"].as_array().unwrap() {
        let (first, last) = (gap[0].as_i64().unwrap(), gap[1].as_i64().unwrap());
        assert!((first..=last).all(|n| day(n).is_none()), "{first}..={last}");
        assert!(day(first - 1).is_some() && day(last + 1).is_some());
    }
    assert_eq!(manifest["long_series"]["empty_month"], "2026-02");
    let month_has_days = |month: &str| days.iter().any(|d| date(d).starts_with(month));
    assert!(!month_has_days("2026-02"));
    assert!(month_has_days("2026-01") && month_has_days("2026-03"));

    // Days 33 and 34 share a date in the guild's timezone and nowhere else:
    // in UTC they were posted on different days.
    assert_eq!(manifest["long_series"]["same_date_days"], json!([33, 34]));
    assert_eq!(manifest["long_series"]["same_date"], "2025-12-15");
    let (early, late) = (day(33).unwrap(), day(34).unwrap());
    assert_eq!(date(early), "2025-12-15");
    assert_eq!(date(late), "2025-12-15");
    let utc_day = |d: &Value| d["posted_at"].as_i64().unwrap() / 86_400;
    assert_eq!(utc_day(late), utc_day(early) + 1);
    let dates: BTreeSet<String> = days.iter().map(date).collect();
    assert_eq!(dates.len(), 149, "no other two days share a date");

    assert_eq!(day(20).unwrap()["count"], 3);
    let missing = day(75).unwrap();
    assert_eq!(missing["missing"], true);
    assert_eq!(missing["thumb_url"], Value::Null);

    let full = |n: i64| {
        let path = main_guild(&format!("/series/{long}/days/{n}"));
        let (s, viewer) = (&s, &viewer);
        async move { s.read(&path, Some(viewer)).await }
    };
    assert_eq!(full(20).await["media"].as_array().unwrap().len(), 3);
    assert_eq!(full(66).await["media"][0]["content_type"], "video/mp4");
    assert_eq!(full(120).await["media"][0]["content_type"], "video/webm");
    assert_eq!(full(75).await["media"][0]["missing"], true);
    assert_eq!(full(12).await["caption"], "");
    assert!(full(145).await["caption"].as_str().unwrap().len() > 150);
    // Every seventh day says a sentence; the others only their number.
    let weekly = "another week of pencil shavings.";
    assert_eq!(full(7).await["caption"], format!("Day 7: {weekly}"));
    assert_eq!(full(182).await["caption"], format!("Day 182: {weekly}"));
    assert_eq!(full(8).await["caption"], "Day 8");

    let stats = s
        .read(&main_guild(&format!("/series/{long}/stats")), Some(&viewer))
        .await;
    assert_eq!(
        stats,
        json!({
            "total": 150, "max_day": 187, "missed": 37,
            "longest_streak": 41, "current_streak": 33,
        })
    );
}

#[tokio::test]
async fn each_persona_sees_what_its_membership_allows() {
    let (s, _alone) = server().await;
    let public = ["Daily Sketch", "Evening Walks"];
    assert_eq!(s.listed("viewer").await, public);
    assert_eq!(s.listed("admin").await, public);
    assert_eq!(s.listed("newcomer").await, public);
    assert_eq!(
        s.listed("patron").await,
        ["Daily Sketch", "Patron Studies", "Evening Walks"]
    );
    // Ids 1 to 6 in this order, on every fresh world.
    let creator = s.sign_in("creator").await;
    let own = s.read(&main_guild("/series"), Some(&creator)).await;
    let seen: Vec<(i64, &str, &str, &str)> = own
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_i64().unwrap(),
                s["emoji"].as_str().unwrap(),
                s["name"].as_str().unwrap(),
                s["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (1, "✏️", "Daily Sketch", "active"),
            (2, "☕", "Morning Coffee", "sprout"),
            (3, "🎨", "Patron Studies", "active"),
            (4, "📓", "Private Notes", "active"),
            (5, "📸", "Old Polaroids", "revoked"),
            (6, "🌆", "Evening Walks", "active"),
        ]
    );
    assert_eq!(own[1]["sprout"], json!({ "archived": 2, "threshold": 3 }));
    // Every post is in the past of the clock the manifest suggests fixing the
    // browser at (2026-05-20 15:00 UTC), so nothing renders as "in the future".
    let now = s.read("/__e2e/state", None).await["suggested_now_unix"].clone();
    assert_eq!(now, 1_779_289_200_i64);
    let newest = |s: &Value| s["last_posted_at"].as_i64().unwrap();
    assert!(
        own.as_array()
            .unwrap()
            .iter()
            .all(|s| newest(s) < now.as_i64().unwrap())
    );
    // A revoked series is listed for its creator but cannot be opened.
    let (status, _) = s.get(&main_guild("/series/5/days"), Some(&creator)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The outsider signs in (Discord knows them) and is in neither guild.
    let outsider = s.sign_in("outsider").await;
    let (status, body) = s.get(&main_guild("/series"), Some(&outsider)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body, json!({ "error": "forbidden" }));
    // A guild leaf's bot is not in says that, not "you are not a member".
    let nowhere = format!("/api/guilds/{UNKNOWN_GUILD_ID}/series");
    let (status, body) = s.get(&nowhere, Some(&outsider)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let said = body["message"].as_str().unwrap();
    assert!(said.contains("bot isn't in this server"), "{said}");
    // A code that names nobody is refused the way Discord refuses one.
    let (status, body) = s
        .post("/api/token", None, json!({ "code": "code-nobody" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "code_rejected");
    // So is a good code presented for an address the application never
    // registered: only the server's own origin exchanges.
    let elsewhere = json!({ "code": "code-viewer", "redirect_uri": "https://elsewhere.example" });
    let (status, body) = s.post("/api/token", None, elsewhere).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "code_rejected");

    // The guild nobody set up: members get in, and there is nothing there.
    let viewer = s.sign_in("viewer").await;
    let unset = format!("/api/guilds/{UNSET_GUILD_ID}/series");
    assert_eq!(s.read(&unset, Some(&viewer)).await, json!([]));
    let eligibility = s.read(&format!("{unset}/eligibility"), Some(&viewer)).await;
    assert_eq!(eligibility["violations"][0]["code"], "guild_not_setup");
}

#[tokio::test]
async fn eligibility_and_reminders_are_seeded_as_promised() {
    let (s, _alone) = server().await;
    let eligibility = |persona: &'static str| {
        let s = &s;
        async move {
            let token = s.sign_in(persona).await;
            s.read(&main_guild("/series/eligibility"), Some(&token))
                .await
        }
    };

    let creator = eligibility("creator").await;
    assert_eq!(creator["can_create"], true, "{creator}");
    assert_eq!(creator["owns_any"], true);

    // No role and too short a membership, with a date that never moves.
    let newcomer = eligibility("newcomer").await;
    assert_eq!(newcomer["can_create"], false);
    let violations = newcomer["violations"].as_array().unwrap();
    let codes: Vec<&str> = violations
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["missing_creator_role", "membership_too_new"]);
    assert_eq!(violations[0]["params"]["role_name"], "Artists");
    assert_eq!(violations[1]["params"]["days"], 7);
    assert_eq!(
        violations[1]["params"]["eligible_at"],
        seed::NEWCOMER_ELIGIBLE_AT
    );
    let manifest = s.read("/__e2e/state", None).await;
    assert_eq!(manifest["newcomer_eligible_at"], seed::NEWCOMER_ELIGIBLE_AT);

    // A long-standing member without the role is stopped by the role alone.
    let viewer = eligibility("viewer").await;
    let codes: Vec<&str> = viewer["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["missing_creator_role"]);

    let token = s.sign_in("creator").await;
    let reminder = s.series_id("reminder").await;
    let mine = s.read(&main_guild("/series/mine"), Some(&token)).await;
    let failing: Vec<&Value> = mine
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| !m["reminder_error"].is_null())
        .collect();
    assert_eq!(failing.len(), 1, "{mine}");
    assert_eq!(failing[0]["id"], reminder);
    assert_eq!(failing[0]["reminder_error"], "dm_closed");
    let settings = s
        .read(
            &main_guild(&format!("/series/{reminder}/settings")),
            Some(&token),
        )
        .await;
    assert_eq!(settings["reminder_enabled"], true);
    assert_eq!(settings["reminder_time"], "18:30");
    assert_eq!(settings["reminder_dm"], true);
    assert_eq!(settings["reminder_error"], "dm_closed");
    // 2026-05-18 00:30 UTC: half an hour after the reminder was due.
    assert_eq!(settings["reminder_error_at"], 1_779_064_200_i64);
}

/// Whether `bytes` start the way a file of this content type does.
fn looks_like(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..]),
        "video/mp4" => bytes.get(4..8) == Some(&b"ftyp"[..]),
        "video/webm" => bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        _ => false,
    }
}

#[test]
fn every_fixture_is_a_file_of_its_type() {
    let fixtures = seed::STILLS.iter().chain([&seed::MP4, &seed::WEBM]);
    for fixture in fixtures {
        let ct = fixture.content_type;
        assert!(looks_like(ct, fixture.original), "{ct} original");
        assert!(looks_like("image/webp", fixture.thumb), "{ct} thumbnail");
    }
    // Large enough that a byte range is a real part of it, small enough to
    // commit.
    assert!((100_000..400_000).contains(&seed::MP4.original.len()));
}

#[tokio::test]
async fn media_is_served_from_the_seeded_store() {
    let (s, _alone) = server().await;
    let viewer = s.sign_in("viewer").await;
    let media_of = |day: i64| {
        let (s, viewer) = (&s, &viewer);
        async move {
            let path = main_guild(&format!("/series/1/days/{day}"));
            s.read(&path, Some(viewer)).await["media"][0].clone()
        }
    };
    let fetch = |url: String, range: Option<&'static str>| {
        let s = &s;
        async move {
            let mut request = Request::get(url);
            if let Some(range) = range {
                request = request.header(header::RANGE, range);
            }
            s.send(request.body(Body::empty()).unwrap()).await
        }
    };
    let url = |media: &Value, field: &str| media[field].as_str().unwrap().to_owned();

    // Day 1 is the still for 1: an original of its own type, a WebP thumb.
    let still = media_of(1).await;
    let expected = seed::still(1);
    assert_eq!(still["content_type"], expected.content_type);
    let original = fetch(url(&still, "url"), None).await;
    assert_eq!(original.status(), StatusCode::OK);
    assert_eq!(
        original.headers()[header::CONTENT_TYPE],
        expected.content_type
    );
    assert_eq!(bytes_of(original).await, expected.original);
    let thumb = fetch(url(&still, "thumb_url"), None).await;
    assert_eq!(thumb.status(), StatusCode::OK);
    assert_eq!(thumb.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(bytes_of(thumb).await, expected.thumb);

    // The video answers a byte range with just that part.
    let clip = media_of(66).await;
    let size = seed::MP4.original.len();
    let part = fetch(url(&clip, "url"), Some("bytes=1000-1999")).await;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        part.headers()[header::CONTENT_RANGE],
        format!("bytes 1000-1999/{size}")
    );
    assert_eq!(part.headers()[header::CONTENT_TYPE], "video/mp4");
    assert_eq!(bytes_of(part).await, &seed::MP4.original[1000..2000]);
    let poster = fetch(url(&clip, "thumb_url"), None).await;
    assert_eq!(bytes_of(poster).await, seed::MP4.thumb);

    // The day without a file has a row to point at and nothing behind it.
    let missing = media_of(75).await;
    assert_eq!(missing["missing"], true);
    let gone = fetch(url(&missing, "url"), None).await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_stub_discord_fails_holds_and_recovers() {
    let (s, _alone) = server().await;
    let list = main_guild("/series");
    let discord = s.read("/__e2e/discord", None).await;
    assert_eq!(discord["mode"], "ok");
    assert_eq!(discord["held"], 0);

    // Down for everything: nobody can sign in.
    s.control("/__e2e/discord", json!({ "mode": "down" })).await;
    let (status, body) = s
        .post("/api/token", None, json!({ "code": "code-viewer" }))
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "discord_unavailable");

    // Down for one call only: sign-in works, the membership check does not.
    let scoped = json!({ "mode": "down", "only": ["guild_member"] });
    let answer = s.control("/__e2e/discord", scoped).await;
    assert_eq!(answer["only"], json!(["guild_member"]));
    let viewer = s.sign_in("viewer").await;
    let (status, body) = s.get(&list, Some(&viewer)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "discord_unavailable");

    // Slow for that call only: a sign-in is answered, the membership check
    // is held, and the stub says so for as long as it is.
    let slow = json!({ "mode": "slow", "delay_ms": NEVER_MS, "only": ["guild_member"] });
    s.control("/__e2e/discord", slow.clone()).await;
    let sign_in = pin!(s.post("/api/token", None, json!({ "code": "code-admin" })));
    assert!(!s.discord_holds(1, sign_in).await, "a sign-in was held");
    let mut listing = pin!(s.get(&list, Some(&viewer)));
    assert!(s.discord_holds(1, listing.as_mut()).await, "not held");
    assert_eq!(s.read("/__e2e/discord", None).await["held"], 1);

    // Back up: the held call is let go and answered. It was a second call,
    // so the failure before it was not remembered; its own answer is.
    s.control("/__e2e/discord", json!({ "mode": "ok" })).await;
    let (status, series) = let_go(listing).await;
    assert_eq!(status, StatusCode::OK, "{series}");
    assert_eq!(series.as_array().unwrap().len(), 2);
    s.read(&list, Some(&viewer)).await;
    let discord = s.read("/__e2e/discord", None).await;
    assert_eq!(discord["held"], 0);
    assert_eq!(discord["calls"]["guild_member"], 2);

    // Held, and then Discord goes down: the call fails instead of waiting.
    s.control("/__e2e/caches/clear", json!({})).await;
    s.control("/__e2e/discord", slow.clone()).await;
    let mut listing = pin!(s.get(&list, Some(&viewer)));
    assert!(s.discord_holds(1, listing.as_mut()).await, "not held");
    s.control("/__e2e/discord", json!({ "mode": "down" })).await;
    let (status, body) = let_go(listing).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    // A held call whose request goes away is no longer counted.
    s.control("/__e2e/discord", slow).await;
    {
        let mut listing = pin!(s.get(&list, Some(&viewer)));
        assert!(s.discord_holds(1, listing.as_mut()).await, "not held");
    }
    assert_eq!(s.read("/__e2e/discord", None).await["held"], 0);

    // A mode there is no such thing as is refused in the control routes' own
    // shape, and changes nothing.
    let (status, body) = s
        .post("/__e2e/discord", None, json!({ "mode": "sideways" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"].as_str().unwrap().contains("sideways"),
        "{body}"
    );
    assert_eq!(s.read("/__e2e/discord", None).await["mode"], "slow");
}

#[tokio::test]
async fn a_reset_abandons_the_calls_the_stub_is_holding() {
    let (s, _alone) = server().await;
    let admin = s.admin_token("admin").await;
    let names = |guilds: &Value| -> Vec<Value> {
        let guilds = guilds.as_array().unwrap();
        guilds.iter().map(|g| g["name"].clone()).collect()
    };

    // The panel's guild list asks Discord for both guilds' names.
    let slow = json!({ "mode": "slow", "delay_ms": NEVER_MS, "only": ["guild_summary"] });
    s.control("/__e2e/discord", slow).await;
    let mut listing = pin!(s.read("/api/admin/guilds", Some(&admin)));
    assert!(s.discord_holds(2, listing.as_mut()).await, "not held");

    // The reset fails both lookups, so the list is answered without names
    // instead of an hour later...
    s.control("/__e2e/reset", json!({})).await;
    assert_eq!(names(&let_go(listing).await), [Value::Null, Value::Null]);
    assert_eq!(s.read("/__e2e/discord", None).await["held"], 0);
    // ...and nothing they would have answered is in a cache the reset
    // emptied: the next list asks Discord again.
    let listed = s.read("/api/admin/guilds", Some(&admin)).await;
    assert_eq!(names(&listed), ["Leaf Test Garden", "Unconfigured Server"]);
    assert_eq!(s.discord_calls("guild_summary").await, 2);
}

#[tokio::test]
async fn the_stub_answers_for_an_unknown_guild_as_discord_does() {
    let stub = StubDiscord::new("http://127.0.0.1:3799");
    let viewer = seed::persona("viewer").unwrap().id;
    let outsider = seed::persona("outsider").unwrap().id;

    // The bot cannot see into a guild it is not in, which is not the same
    // answer as "that user is not a member".
    let lookup = stub.guild_member(UNKNOWN_GUILD_ID, viewer).await;
    assert_eq!(lookup.unwrap_err(), LookupError::BotNotInGuild);
    assert!(stub.guild_channels(UNKNOWN_GUILD_ID).await.is_err());
    assert!(stub.guild_roles(UNKNOWN_GUILD_ID).await.is_err());
    assert_eq!(stub.guild_summary(UNKNOWN_GUILD_ID).await, Ok(None));
    assert!(matches!(
        stub.guild_member(MAIN_GUILD_ID, outsider).await,
        Ok(None)
    ));
    let member = stub.guild_member(MAIN_GUILD_ID, viewer).await.unwrap();
    assert_eq!(member.unwrap().name.as_deref(), Some("Sam"));
}

#[tokio::test]
async fn clearing_the_caches_makes_leaf_ask_discord_again() {
    let (s, _alone) = server().await;
    let creator = s.sign_in("creator").await;
    let options = main_guild("/series/options");

    s.read(&options, Some(&creator)).await;
    s.read(&options, Some(&creator)).await;
    // Membership is cached per state, roles for the whole process.
    assert_eq!(s.discord_calls("guild_member").await, 1);
    assert_eq!(s.discord_calls("guild_roles").await, 1);
    assert_eq!(s.discord_calls("guild_channels").await, 1);

    let (status, _) = s.post("/__e2e/caches/clear", None, json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    s.read(&options, Some(&creator)).await;
    assert_eq!(s.discord_calls("guild_member").await, 2);
    assert_eq!(s.discord_calls("guild_roles").await, 2);
    assert_eq!(s.discord_calls("guild_channels").await, 2);
}

#[tokio::test]
async fn clearing_the_caches_forgets_guild_and_creator_names() {
    let (s, _alone) = server().await;
    let admin = s.admin_token("admin").await;
    // What the panel loads: the guild list (each guild's name) and one
    // guild's page (its creators' names, by a membership lookup). Returns
    // how often Discord has been asked for each so far.
    let panel = || {
        let (s, admin) = (&s, &admin);
        async move {
            s.read("/api/admin/guilds", Some(admin)).await;
            let page = format!("/api/admin/guilds/{MAIN_GUILD_ID}");
            let page = s.read(&page, Some(admin)).await;
            assert_eq!(page["name"], "Leaf Test Garden");
            assert_eq!(page["series"][0]["creator_name"], "Mika");
            let asked = s.read("/__e2e/discord", None).await["calls"].clone();
            (
                asked["guild_summary"].clone(),
                asked["guild_member"].clone(),
            )
        }
    };

    assert_eq!(panel().await, (json!(2), json!(1)));
    assert_eq!(panel().await, (json!(2), json!(1)), "names are kept");
    let (status, _) = s.post("/__e2e/caches/clear", None, json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(panel().await, (json!(4), json!(2)), "asked again");

    // A reset forgets them too, and starts the counts again.
    s.control("/__e2e/reset", json!({})).await;
    assert_eq!(panel().await, (json!(2), json!(1)));
}

#[tokio::test]
async fn the_manifest_says_what_leaf_has_stored() {
    let (s, _alone) = server().await;
    let manifest = s.read("/__e2e/state", None).await;
    let (main, unset) = (&manifest["guilds"]["main"], &manifest["guilds"]["unset"]);
    let setup = [
        "setup_complete",
        "timezone",
        "watched_channel_ids",
        "log_channel_id",
        "creator_role_id",
        "max_series_per_user",
        "min_account_age_days",
        "min_membership_age_days",
        "sprout_enabled",
        "sprout_threshold",
    ];
    let setup_of =
        |guild: &Value| -> Value { setup.iter().map(|&key| (key, guild[key].clone())).collect() };

    // The main guild as the brief describes it; the other as the bot leaves
    // a guild it has just joined.
    assert_eq!(
        setup_of(main),
        json!({
            "setup_complete": true,
            "timezone": "America/Chicago",
            "watched_channel_ids": ["200000000000000001", "200000000000000002"],
            "log_channel_id": LOG_CHANNEL_ID,
            "creator_role_id": "300000000000000001",
            "max_series_per_user": 10,
            "min_account_age_days": 0,
            "min_membership_age_days": 7,
            "sprout_enabled": true,
            "sprout_threshold": 3,
        })
    );
    assert_eq!(
        setup_of(unset),
        json!({
            "setup_complete": false,
            "timezone": "UTC",
            "watched_channel_ids": [],
            "log_channel_id": null,
            "creator_role_id": null,
            "max_series_per_user": 3,
            "min_account_age_days": 0,
            "min_membership_age_days": 0,
            "sprout_enabled": false,
            "sprout_threshold": 3,
        })
    );

    // And leaf answers the same: the manifest is not a second copy of the
    // seed's constants.
    let admin = s.admin_token("admin").await;
    for guild in [main, unset] {
        let page = format!("/api/admin/guilds/{}", guild["id"].as_str().unwrap());
        let page = s.read(&page, Some(&admin)).await;
        assert_eq!(page["name"], guild["name"]);
        assert_eq!(page["setup_complete"], guild["setup_complete"]);
        // The panel's settings carry all of these but the two named.
        let not_settings = ["setup_complete", "watched_channel_ids"];
        for key in setup.iter().filter(|key| !not_settings.contains(key)) {
            assert_eq!(page["settings"][*key], guild[*key], "{key}");
        }
    }
    let creator = s.sign_in("creator").await;
    let options = s.read(&main_guild("/series/options"), Some(&creator)).await;
    assert_eq!(
        options["channels"],
        json!([
            { "id": "200000000000000001", "name": "daily-sketch" },
            { "id": "200000000000000002", "name": "photo-share" },
        ])
    );
    assert_eq!(options["guild_timezone"], "America/Chicago");
    assert_eq!(options["sprout_enabled"], true);
    assert_eq!(options["sprout_threshold"], 3);
}

#[tokio::test]
async fn reset_brings_back_exactly_the_seed() {
    let (s, _alone) = server().await;
    let before = s.read("/__e2e/state", None).await;
    let viewer = s.sign_in("viewer").await;
    let creator = s.sign_in("creator").await;
    let list = main_guild("/series");
    let new_series = json!({
        "name": "Made in a test", "channel_id": "200000000000000001",
        "cadence": "daily", "privacy": "public",
    });

    // Change something of every kind a reset has to undo.
    s.control("/__e2e/series/long/days", json!({})).await;
    let (status, made) = s.post(&list, Some(&creator), new_series.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["id"], 7);
    // Probation is on in the main guild: a new series starts as a sprout.
    assert_eq!(made["state"], "sprout");
    assert_eq!(made["sprout"], json!({ "archived": 0, "threshold": 3 }));
    s.read(&main_guild("/series/options"), Some(&creator)).await;
    let intent = json!({ "persona": "viewer", "series": "long", "day": 43 });
    s.control("/__e2e/launch-intent", intent).await;
    let collect = main_guild("/launch-intent?attempt=sameattempt");
    assert_eq!(s.read(&collect, Some(&viewer)).await["day"], 43);
    // The same attempt reads it again: leaf remembers what it handed out.
    assert_eq!(s.read(&collect, Some(&viewer)).await["day"], 43);
    DiscordApi::send_message(s.harness.discord.as_ref(), LOG_CHANNEL_ID, "a line")
        .await
        .unwrap();
    let sent = s.read("/__e2e/discord/messages", None).await;
    assert_eq!(sent.as_array().unwrap().len(), 1);
    s.control("/__e2e/discord", json!({ "mode": "down" })).await;

    let after = s.control("/__e2e/reset", json!({})).await;
    assert_eq!(after, before, "the manifest, ids included");
    let discord = s.read("/__e2e/discord", None).await;
    assert_eq!(discord["mode"], "ok");
    assert_eq!(discord["calls"], json!({}));
    assert_eq!(s.read("/__e2e/discord/messages", None).await, json!([]));

    // Sessions from before still work, on the seed as it started.
    let own = s.read(&list, Some(&creator)).await;
    assert_eq!(own.as_array().unwrap().len(), 6);
    assert_eq!(own[0]["total_days"], 150);
    // Nothing leaf cached before the reset answered that, or answers now.
    assert_eq!(s.discord_calls("guild_member").await, 1);
    s.read(&main_guild("/series/options"), Some(&creator)).await;
    assert_eq!(s.discord_calls("guild_roles").await, 1);
    assert_eq!(s.read(&collect, Some(&viewer)).await, Value::Null);
    // The next series made gets the id the last one had.
    let (_, made) = s.post(&list, Some(&creator), new_series).await;
    assert_eq!(made["id"], 7);
}

#[tokio::test]
async fn sessions_can_be_minted_in_each_state() {
    let (s, _alone) = server().await;
    let list = main_guild("/series");
    let mint = |body: Value| {
        let s = &s;
        async move { s.control("/__e2e/session", body).await }
    };
    let token = |minted: &Value| minted["token"].as_str().unwrap().to_owned();
    let refresh = |token: String| {
        let s = &s;
        async move { s.post("/api/token/refresh", Some(&token), json!({})).await }
    };

    // The answer can stand in for `POST /api/token`'s.
    let fresh = mint(json!({ "persona": "viewer" })).await;
    assert_eq!(fresh["access_token"], "access-viewer");
    assert_eq!(fresh["expires_in"], 21_600);
    assert_eq!(fresh["user_id"], "100000000000000002");
    assert_eq!(s.read(&list, Some(&token(&fresh))).await[0]["id"], 1);
    let (status, renewed) = refresh(token(&fresh)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renewed["expires_in"], 21_600);

    // Good for five seconds from the sign-in, checked against the key
    // itself rather than by waiting for them to pass.
    let expiring = mint(json!({ "persona": "viewer", "kind": "expiring" })).await;
    assert_eq!(expiring["expires_in"], 5);
    let signed_in = expiring["auth_at"].as_i64().unwrap();
    assert_eq!(expiring["exp"], signed_in + 5);
    let key = &s.harness.key;
    let claims = key
        .verify_session(&token(&expiring), signed_in + 4)
        .unwrap();
    assert_eq!((claims.auth_at, claims.exp), (signed_in, signed_in + 5));
    assert_eq!(
        key.verify(&token(&expiring), signed_in + 5),
        Err(AuthError::Expired)
    );

    let expired = mint(json!({ "persona": "viewer", "kind": "expired" })).await;
    let (status, _) = s.get(&list, Some(&token(&expired))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Past the 7-day cap: still good for reading, never renewed.
    let capped = mint(json!({ "persona": "viewer", "kind": "capped" })).await;
    let (status, _) = s.get(&list, Some(&token(&capped))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = refresh(token(&capped)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Ten minutes short of the cap: renewed for what is left of them, not
    // for another six hours.
    let near = json!({ "persona": "viewer", "age_secs": 7 * 86_400 - 600, "ttl_secs": 60 });
    let (status, renewed) = refresh(token(&mint(near).await)).await;
    assert_eq!(status, StatusCode::OK);
    let left = renewed["expires_in"].as_i64().unwrap();
    assert!((1..=600).contains(&left), "{left}");

    let (status, body) = s
        .post("/__e2e/session", None, json!({ "persona": "nobody" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // A sign-in that has not happened yet is not a state a session is in.
    let ahead = json!({ "persona": "viewer", "age_secs": -5 });
    let (status, body) = s.post("/__e2e/session", None, ahead).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("age_secs"));
}

#[tokio::test]
async fn a_launch_intent_is_collected_once_and_only_if_viewable() {
    let (s, _alone) = server().await;
    let viewer = s.sign_in("viewer").await;
    let collect = main_guild("/launch-intent");

    assert_eq!(s.read(&collect, Some(&viewer)).await, Value::Null);
    let intent = json!({ "persona": "viewer", "series": "long", "day": 43 });
    let put = s.control("/__e2e/launch-intent", intent).await;
    assert_eq!(put["guild_id"], MAIN_GUILD_ID);
    assert_eq!(
        s.read(&collect, Some(&viewer)).await,
        json!({ "series_id": 1, "day": 43 })
    );
    assert_eq!(s.read(&collect, Some(&viewer)).await, Value::Null);

    // By id, without a day, and for a series this viewer cannot see.
    let private = s.series_id("creator_only").await;
    let hidden = json!({ "persona": "viewer", "series": private.to_string() });
    s.control("/__e2e/launch-intent", hidden).await;
    assert_eq!(s.read(&collect, Some(&viewer)).await, Value::Null);

    // Filed under another guild, it is that guild's to collect, not this one's.
    let elsewhere = json!({ "persona": "viewer", "series": "long", "guild": UNSET_GUILD_ID });
    let put = s.control("/__e2e/launch-intent", elsewhere).await;
    assert_eq!(put["guild_id"], UNSET_GUILD_ID);
    assert_eq!(s.read(&collect, Some(&viewer)).await, Value::Null);
    let viewer_id = seed::persona("viewer").unwrap().id;
    let filed = LaunchIntentRepo::new(s.harness.world().state.series.pool().clone())
        .take(viewer_id, UNSET_GUILD_ID, now_unix())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((filed.series_id, filed.day), (1, None));

    let (status, _) = s
        .post(
            "/__e2e/launch-intent",
            None,
            json!({ "persona": "viewer", "series": "no-such" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_sign_in_runs_the_real_callback() {
    let (s, _alone) = server().await;
    let sign_in = |query: &'static str| {
        let s = &s;
        async move {
            let callback = s
                .redirect_from(&format!("/__e2e/admin/login?{query}"))
                .await;
            assert!(callback.starts_with("/admin/callback?code="), "{callback}");
            s.redirect_from(&callback).await
        }
    };

    let landed = sign_in("persona=admin").await;
    assert!(landed.starts_with("/admin#token="), "{landed}");
    let token = landed.trim_start_matches("/admin#token=");
    let guilds = s.read("/api/admin/guilds", Some(token)).await;
    assert_eq!(
        guilds,
        json!([
            { "guild_id": MAIN_GUILD_ID, "series_count": 6,
              "name": "Leaf Test Garden", "icon_url": null },
            { "guild_id": UNSET_GUILD_ID, "series_count": 0,
              "name": "Unconfigured Server", "icon_url": null },
        ])
    );

    // A revoke is reported in the guild's log channel, through the stub.
    let (status, revoked) = s
        .call(
            Method::PATCH,
            &format!("/api/admin/guilds/{MAIN_GUILD_ID}/series/1"),
            Some(token),
            Some(json!({ "state": "revoked" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["creator_name"], "Mika");
    let sent = s.read("/__e2e/discord/messages", None).await;
    assert_eq!(sent.as_array().unwrap().len(), 1, "{sent}");
    assert_eq!(sent[0]["channel_id"], LOG_CHANNEL_ID);
    let line = sent[0]["content"].as_str().unwrap();
    assert!(line.contains("Daily Sketch"), "{line}");
    assert!(line.contains("<@100000000000000003>"), "{line}");

    // Each way a sign-in fails lands on the panel's card for it.
    assert_eq!(sign_in("persona=outsider").await, "/admin#error=no_guilds");
    assert_eq!(sign_in("code=spent").await, "/admin#error=exchange_failed");
    s.control("/__e2e/discord", json!({ "mode": "down" })).await;
    assert_eq!(
        sign_in("persona=admin").await,
        "/admin#error=discord_unavailable"
    );
    // Neither or both of persona and code, a persona there is none of, and
    // a code that would need escaping in the callback's address.
    for refused in [
        "",
        "?persona=admin&code=spent",
        "?persona=nobody",
        "?code=a%20b",
        "?code=",
    ] {
        let (status, body) = s.get(&format!("/__e2e/admin/login{refused}"), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }

    // A minted admin token is as good as one from the callback; a lapsed
    // one is refused.
    let minted = s
        .control("/__e2e/admin-session", json!({ "persona": "admin" }))
        .await;
    assert_eq!(minted["guild_ids"], json!([MAIN_GUILD_ID, UNSET_GUILD_ID]));
    let (status, _) = s.get("/api/admin/guilds", minted["token"].as_str()).await;
    assert_eq!(status, StatusCode::OK);
    let lapsed = json!({ "persona": "admin", "ttl_secs": -1 });
    let lapsed = s.control("/__e2e/admin-session", lapsed).await;
    let (status, _) = s.get("/api/admin/guilds", lapsed["token"].as_str()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_added_day_shows_up_in_the_gallery() {
    let (s, _alone) = server().await;
    let viewer = s.sign_in("viewer").await;
    let days = main_guild("/series/1/days");

    // No body: the next day number, a day after the newest post.
    let (status, added) = s
        .call(Method::POST, "/__e2e/series/long/days", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(
        added,
        json!({ "series_id": 1, "day": 188, "posted_at": 1_779_213_600,
                "local_date": "2026-05-19", "state": "active", "promoted": false })
    );
    let index = s.read(&days, Some(&viewer)).await;
    let newest = index.as_array().unwrap().last().unwrap().clone();
    assert_eq!(index.as_array().unwrap().len(), 151);
    assert_eq!(newest["day"], 188);
    assert_eq!(newest["local_date"], "2026-05-19");
    let thumb = newest["thumb_url"].as_str().unwrap();
    let response = s
        .send(Request::get(thumb).body(Body::empty()).unwrap())
        .await;
    assert_eq!(bytes_of(response).await, seed::still(188).thumb);

    let (status, body) = s
        .post("/__e2e/series/long/days", None, json!({ "day": 188 }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("already archived"));

    // Every kind of day, by series id, with the fields given.
    let clip =
        json!({ "day": 300, "media": "mp4", "caption": "a clip", "posted_at": 1_784_000_000 });
    assert_eq!(s.control("/__e2e/series/1/days", clip).await["day"], 300);
    let day = s.read(&format!("{days}/300"), Some(&viewer)).await;
    assert_eq!(day["caption"], "a clip");
    assert_eq!(day["posted_at"], 1_784_000_000);
    assert_eq!(day["media"][0]["content_type"], "video/mp4");
    for (media, count, missing) in [
        ("images", 3, false),
        ("webm", 1, false),
        ("missing", 1, true),
        ("none", 0, true),
    ] {
        let added = s
            .control("/__e2e/series/long/days", json!({ "media": media }))
            .await;
        let index = s.read(&days, Some(&viewer)).await;
        let entry = index.as_array().unwrap().last().unwrap().clone();
        assert_eq!(entry["day"], added["day"], "{media}");
        assert_eq!(entry["count"], count, "{media}");
        assert_eq!(entry["missing"], missing, "{media}");
    }

    let (status, _) = s.post("/__e2e/series/no-such/days", None, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = s.post("/__e2e/series/999/days", None, json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_removed_day_is_gone_from_the_gallery_and_the_store() {
    let (s, _alone) = server().await;
    let viewer = s.sign_in("viewer").await;
    let days = main_guild("/series/1/days");
    let day = s.read(&format!("{days}/20"), Some(&viewer)).await;
    let photo = day["media"][0]["url"].as_str().unwrap().to_owned();
    let fetch = || s.send(Request::get(photo.as_str()).body(Body::empty()).unwrap());
    assert_eq!(fetch().await.status(), StatusCode::OK);

    let (status, body) = s
        .call(Method::DELETE, "/__e2e/series/long/days/20", None, None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // The day, its place in the index and its files are all gone; the days
    // around it are untouched.
    let (status, body) = s.get(&format!("{days}/20"), Some(&viewer)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let index = s.read(&days, Some(&viewer)).await;
    let listed: Vec<i64> = index
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["day"].as_i64().unwrap())
        .collect();
    assert_eq!(listed.len(), 149);
    assert!(!listed.contains(&20));
    assert!(listed.contains(&19) && listed.contains(&21));
    assert_eq!(fetch().await.status(), StatusCode::NOT_FOUND);

    // Only an archived day can be removed, and only from a series there is.
    let (status, body) = s
        .call(Method::DELETE, "/__e2e/series/long/days/20", None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].as_str().unwrap().contains("not archived"));
    let (status, _) = s
        .call(Method::DELETE, "/__e2e/series/no-such/days/1", None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_added_day_publishes_a_sprout_as_the_bot_would() {
    let (s, _alone) = server().await;
    let creator = s.sign_in("creator").await;
    let list = main_guild("/series");
    assert_eq!(s.listed("viewer").await, ["Daily Sketch", "Evening Walks"]);

    // The seeded sprout has two of the three days it needs. The third
    // publishes it: everyone sees it, and it has no progress left to show.
    let third = s.control("/__e2e/series/sprout/days", json!({})).await;
    assert_eq!(third["day"], 3);
    assert_eq!(third["state"], "active");
    assert_eq!(third["promoted"], true);
    let own = s.read(&list, Some(&creator)).await;
    assert_eq!(own[1]["state"], "active");
    assert_eq!(own[1]["sprout"], Value::Null);
    assert_eq!(
        s.listed("viewer").await,
        ["Daily Sketch", "Morning Coffee", "Evening Walks"]
    );
    // It is published once.
    let fourth = s.control("/__e2e/series/sprout/days", json!({})).await;
    assert_eq!(fourth["state"], "active");
    assert_eq!(fourth["promoted"], false);

    // A sprout still short of the threshold stays one.
    let new_series = json!({
        "name": "Made in a test", "channel_id": "200000000000000001",
        "cadence": "daily", "privacy": "public",
    });
    let (status, made) = s.post(&list, Some(&creator), new_series).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let first = s.control("/__e2e/series/7/days", json!({})).await;
    assert_eq!(first["state"], "sprout");
    assert_eq!(first["promoted"], false);
    let own = s.read(&list, Some(&creator)).await;
    assert_eq!(own[6]["sprout"], json!({ "archived": 1, "threshold": 3 }));

    // A revoked series takes no days, from the bot or from here.
    let (status, body) = s.post("/__e2e/series/revoked/days", None, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("revoked"));
    assert_eq!(own[4]["total_days"], 4);
    assert_eq!(s.read(&list, Some(&creator)).await[4]["total_days"], 4);
}

#[tokio::test]
async fn elsewhere_redirects_off_this_machine_as_the_admin_login_does() {
    let (s, _alone) = server().await;
    // The same kind of answer (`redirect_from` holds both to a 303)...
    let login = s.redirect_from("/admin/login").await;
    assert!(login.starts_with("https://discord.com/oauth2/authorize?"));
    // ...to a name nothing can be reached under.
    let elsewhere = s.redirect_from("/__e2e/elsewhere").await;
    assert_eq!(elsewhere, "http://leaf-e2e.invalid/landed");
}

#[tokio::test]
async fn control_paths_never_fall_through_to_the_gallery() {
    let (s, _alone) = server().await;
    let (status, body) = s.get("/__e2e/no-such-route", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({ "error": "no such control route" }));

    // Everything else is the real router's to answer.
    let health = s
        .send(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(bytes_of(health).await, b"ok");
    let (status, body) = s.get("/api/no-such-route", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({ "error": "not_found" }));
    let (status, _) = s.get(&main_guild("/series"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// What the setup page posts for a folder on this machine.
fn folder_submit(code: &str, folder: &std::path::Path) -> Value {
    json!({
        "setup_code": code,
        "discord_token": "tok",
        "client_id": "123",
        "client_secret": "sec",
        "public_url": "https://leaf.example.com",
        "storage": "folder",
        "storage_folder": folder,
    })
}

#[tokio::test]
async fn setup_mode_is_only_there_while_a_suite_has_it_open() {
    let (s, _alone) = server().await;
    // A configured leaf: the setup API says setup is over, and there is no
    // setup stage to ask about.
    let (status, body) = s
        .post("/setup/api/verify-code", None, json!({ "setup_code": "x" }))
        .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let (status, body) = s.get("/__e2e/setup", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({ "error": "no setup mode is open" }));

    // Opened (no body needed): the real page, and a code that verifies.
    let opened = s
        .send(Request::post("/__e2e/setup").body(Body::empty()).unwrap())
        .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: Value = serde_json::from_slice(&bytes_of(opened).await).unwrap();
    let code = opened["code"].as_str().unwrap();
    let data_dir = std::path::Path::new(opened["data_dir"].as_str().unwrap());
    let page = s
        .send(Request::get("/setup").body(Body::empty()).unwrap())
        .await;
    assert_eq!(page.status(), StatusCode::OK);
    let page = String::from_utf8(bytes_of(page).await).unwrap();
    assert!(page.contains("A folder on this machine"));
    let verify = json!({ "setup_code": code });
    let (status, body) = s.post("/setup/api/verify-code", None, verify).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["suggested_folder"],
        json!(data_dir.join("media").to_str().unwrap())
    );
    assert_eq!(
        s.read("/__e2e/setup", None).await,
        json!({ "saved": null, "raw": null, "entries": [] })
    );
    // Everything that is not the setup page is still the configured leaf.
    let health = s
        .send(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(health.status(), StatusCode::OK);
    let (status, _) = s.get("/setupx", None).await;
    assert_ne!(status, StatusCode::GONE);

    // A reset closes it again.
    s.control("/__e2e/reset", json!({})).await;
    let (status, _) = s.get("/__e2e/setup", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let verify = json!({ "setup_code": code });
    let (status, _) = s.post("/setup/api/verify-code", None, verify).await;
    assert_eq!(status, StatusCode::GONE);

    let (status, body) = s
        .post("/__e2e/setup", None, json!({ "discord": "maybe" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].is_string());
}

#[cfg(unix)]
#[tokio::test]
async fn setup_mode_checks_a_folder_on_the_disk_and_scripts_the_rest() {
    let (s, _alone) = server().await;
    let opened = s.control("/__e2e/setup", json!({})).await;
    let code = opened["code"].as_str().unwrap().to_owned();
    let data_dir = std::path::PathBuf::from(opened["data_dir"].as_str().unwrap());
    let folder = data_dir.join("media");

    // The folder check is the real one: a file in the way is found.
    std::fs::write(data_dir.join("taken"), b"").unwrap();
    let blocked = folder_submit(&code, &data_dir.join("taken").join("media"));
    let (status, body) = s.post("/setup/api/submit", None, blocked).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"][0]["field"], "storage_folder");
    std::fs::remove_file(data_dir.join("taken")).unwrap();

    let (status, body) = s
        .post("/setup/api/submit", None, folder_submit(&code, &folder))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = s.read("/__e2e/setup", None).await;
    let endpoint = format!("file://{}", folder.display());
    assert_eq!(
        after["saved"]["r2"],
        json!({ "endpoint": endpoint, "bucket": "", "access_key_id": "", "secret_access_key": "" })
    );
    assert_eq!(after["entries"], json!(["leaf.conf", "media"]));
    let raw = after["raw"].as_str().unwrap();
    assert!(raw.contains(&endpoint), "{raw}");
    assert!(!raw.contains("bucket"), "{raw}");

    // Opening again starts over, with what Discord and R2 are told to say.
    let script = json!({ "discord": "refuse_token", "r2": "refuse_bucket" });
    let opened = s.control("/__e2e/setup", script).await;
    let code = opened["code"].as_str().unwrap().to_owned();
    let data_dir = std::path::PathBuf::from(opened["data_dir"].as_str().unwrap());
    let r2 = json!({
        "setup_code": code,
        "discord_token": "tok",
        "client_id": "123",
        "client_secret": "sec",
        "public_url": "https://leaf.example.com",
        "storage": "r2",
        "r2_endpoint": "https://acc.r2.cloudflarestorage.com",
        "r2_bucket": "leaf",
        "r2_access_key_id": "ak",
        "r2_secret_access_key": "sk",
    });
    let (status, body) = s.post("/setup/api/submit", None, r2).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let fields: Vec<&str> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["discord_token", "r2_bucket"]);

    // A folder is on the disk whatever the script says about R2, and a
    // refused submit leaves the data directory as it found it.
    let (status, body) = s
        .post(
            "/setup/api/submit",
            None,
            folder_submit(&code, &data_dir.join("media")),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["errors"][0]["field"], "discord_token");
    assert_eq!(
        s.read("/__e2e/setup", None).await,
        json!({ "saved": null, "raw": null, "entries": [] })
    );
}
