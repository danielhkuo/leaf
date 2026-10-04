//! `leaf doctor` as a person runs it: the built binary, its arguments, what
//! it prints and how it exits.
//!
//! Only the checks that stay on this machine are run (`config`, `database`,
//! `db-copy`, and `url` against a server on a loopback port): the others
//! talk to Discord and R2, and are covered against stand-ins by the unit
//! tests in `src/doctor/`.
//!
//! The leaf that `url` asks runs in a process of its own ([`Served`]). Run
//! mode's router reads `STATIC_DIR` from its environment, and a test cannot
//! change its own environment (that is `unsafe`), so a router built inside
//! this process would serve whatever gallery build the machine running the
//! tests happens to have, or none. The process is this same test binary,
//! started on the one function below that is not a test.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "tests may panic; JSON indexing is fine in assertions"
)]

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use leaf_core::config::{CONFIG_FILE_NAME, R2Config, Tier1Config};
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo, SqlitePool};
use leaf_core::status::{GatewayState, set_gateway};
use leaf_server::api::auth::SessionKey;
use leaf_server::api::discord::LiveDiscord;
use leaf_server::api::state::{ApiState, channels_cache, membership_cache};
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, PutPayload};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// The credentials in the test's `leaf.conf`. None may be printed.
const SECRETS: [&str; 4] = [
    "SECRET_TOKEN_AAA",
    "SECRET_CLIENT_BBB",
    "SECRET_KEYID_CCC",
    "SECRET_KEY_DDD",
];
/// The checks that never leave this machine.
const LOCAL_CHECKS: [&str; 4] = ["config", "database", "db-copy", "url"];
/// The stored file the server holds, and where.
const MEDIA_KEY: &str = "g/g1/s/1/d/1/a1";
const MEDIA_BYTES: &[u8] = b"0123456789";
/// A gallery's shell as Vite builds one, and the hashed script it loads.
const SHELL: &str = r#"<!doctype html><html><head>
    <script type="module" crossorigin src="/assets/index-test.js"></script>
    </head><body><div id="app"></div></body></html>"#;
const ASSET: &str = "assets/index-test.js";

/// The function below that is the server process, by its name.
const SERVER: &str = "a_leaf_for_the_url_tests_to_ask";
/// Tells the server process which data directory to serve; without it the
/// function does nothing.
const SERVE_DATA_DIR: &str = "LEAF_DOCTOR_TEST_SERVE_DATA_DIR";
/// Tells the server process where its bot stands: `online` or `offline`.
const SERVE_GATEWAY: &str = "LEAF_DOCTOR_TEST_SERVE_GATEWAY";
/// What the server process writes, followed by its address, once it listens.
const SERVING: &str = "leaf-doctor-test serving ";

fn config(public_url: &str) -> Tier1Config {
    Tier1Config {
        discord_token: "SECRET_TOKEN_AAA".to_owned(),
        client_id: "123".to_owned(),
        client_secret: "SECRET_CLIENT_BBB".to_owned(),
        public_url: public_url.to_owned(),
        r2: R2Config {
            endpoint: "https://acc.r2.cloudflarestorage.com".to_owned(),
            bucket: "leaf-media".to_owned(),
            access_key_id: "SECRET_KEYID_CCC".to_owned(),
            secret_access_key: "SECRET_KEY_DDD".to_owned(),
        },
    }
}

/// What one run of the binary came to.
struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap()
    }

    /// (check, status) of every result in `--json` output.
    fn outline(&self) -> Vec<(String, String)> {
        self.json()["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["check"].as_str().unwrap().to_owned(),
                    r["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    /// The sentence of result `at` in `--json` output.
    fn said(&self, at: usize) -> String {
        self.json()["results"][at]["message"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Neither a credential nor any part of a signed media address (its
    /// query is the signature) is on either stream.
    fn assert_no_secrets(&self) {
        for (stream, text) in [("stdout", &self.stdout), ("stderr", &self.stderr)] {
            for secret in SECRETS {
                assert!(!text.contains(secret), "{secret} on {stream}: {text}");
            }
            assert!(!text.contains("sig="), "a signature on {stream}: {text}");
            assert!(!text.contains("exp="), "a signed query on {stream}: {text}");
        }
    }
}

/// An outline as [`Ran::outline`] gives it.
fn outline<const N: usize>(expected: [(&str, &str); N]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(check, status)| ((*check).to_owned(), (*status).to_owned()))
        .collect()
}

/// Whether these arguments keep the doctor on this machine: `--only` with
/// local checks, or a command line that is answered before any check runs.
fn stays_local(args: &[&str]) -> bool {
    let only = args
        .iter()
        .position(|arg| *arg == "--only")
        .and_then(|at| args.get(at + 1));
    only.map_or_else(
        || args.iter().any(|arg| *arg == "--help" || *arg == "--bogus"),
        |list| list.split(',').all(|check| LOCAL_CHECKS.contains(&check)),
    )
}

/// Runs `leaf doctor <args>` with `data_dir` as its `DATA_DIR`, logging at
/// its own default level.
async fn doctor(data_dir: &Path, args: &[&str]) -> Ran {
    doctor_logging(data_dir, args, None).await
}

/// [`doctor`] with `LOG_LEVEL` set to `log_level`.
async fn doctor_logging(data_dir: &Path, args: &[&str], log_level: Option<&str>) -> Ran {
    assert!(
        stays_local(args),
        "{args:?} would reach Discord or R2 from a test"
    );
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_leaf"));
    command
        .arg("doctor")
        .args(args)
        .env("DATA_DIR", data_dir)
        .env_remove("DEV_GUILD_ID")
        .env_remove("LOG_LEVEL")
        .kill_on_drop(true);
    if let Some(log_level) = log_level {
        command.env("LOG_LEVEL", log_level);
    }
    let output = command.output().await.unwrap();
    Ran {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// What a configured leaf has on disk: a `leaf.conf` whose public address
/// is `base`, and a migrated database with one archived day whose file is
/// attachment `a1`. The pool that comes back is the hold a running leaf has
/// on that database.
async fn install(data_dir: &Path, base: &str) -> SqlitePool {
    config(base).save(&data_dir.join(CONFIG_FILE_NAME)).unwrap();
    let pool = leaf_core::db::connect(&data_dir.join("leaf.db"))
        .await
        .unwrap();
    for sql in [
        "INSERT INTO guild_settings (guild_id) VALUES ('g1')",
        "INSERT INTO series (id, guild_id, creator_id, name, created_at) \
         VALUES (1, 'g1', 'u1', 'Daily Sketch', 1)",
        "INSERT INTO posts (series_id, day, message_id, channel_id, posted_at, archived_at) \
         VALUES (1, 1, 'm1', 'c1', 1, 1)",
        "INSERT INTO media_attachments \
         (series_id, day, attachment_id, channel_id, message_id, content_type, original_key) \
         VALUES (1, 1, 'a1', 'c1', 'm1', 'video/mp4', 'g/g1/s/1/d/1/a1')",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    pool
}

/// Not a test: the leaf the `url` tests ask, when this binary is started as
/// their server ([`Served::start`]). Run among the tests it does nothing.
///
/// It is run mode as far as the doctor can tell from outside: the real
/// router over a database in the data directory and an in-memory store, both
/// held open the way run mode holds them, with the gallery build `STATIC_DIR`
/// names. It serves until its stdin closes: the test that started it holds
/// the other end, so the leaf goes when the test does, however that ends.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "the server process of the url tests, which start it themselves"]
async fn a_leaf_for_the_url_tests_to_ask() {
    let Some(data_dir) = std::env::var_os(SERVE_DATA_DIR).map(PathBuf::from) else {
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let config = config(&base);
    let pool = install(&data_dir, &base).await;
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    store
        .put(
            &ObjectPath::from(MEDIA_KEY),
            PutPayload::from_static(MEDIA_BYTES),
        )
        .await
        .unwrap();

    let discord = LiveDiscord::new(
        &config.client_id,
        &config.client_secret,
        &config.discord_token,
    )
    .unwrap();
    let state = ApiState {
        series: SeriesRepo::new(pool.clone()),
        posts: PostRepo::new(pool.clone()),
        guilds: GuildSettingsRepo::new(pool.clone()),
        store,
        key: SessionKey::derive(&config.client_secret),
        discord: Arc::new(discord),
        membership: membership_cache(),
        channels: channels_cache(),
        redirect_uri: config.public_url.clone(),
        client_id: config.client_id.clone(),
    };
    set_gateway(match std::env::var(SERVE_GATEWAY).as_deref() {
        Ok("offline") => GatewayState::Error("Discord rejected the bot token.".to_owned()),
        _ => GatewayState::Online,
    });
    // `STATIC_DIR` is read here.
    let router = leaf_server::run::router(state);
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    // On a line of its own, straight to the pipe the test reads: the test
    // harness writes its own progress to the same stream.
    {
        let mut out = std::io::stdout().lock();
        write!(out, "\n{SERVING}{base}\n").unwrap();
        out.flush().unwrap();
    }
    let test_is_gone = tokio::task::spawn_blocking(|| {
        std::io::stdin()
            .lock()
            .read_to_end(&mut Vec::new())
            .unwrap();
    });
    tokio::select! {
        gone = test_is_gone => gone.unwrap(),
        served = serving => served.unwrap(),
    }
}

/// A running leaf in a process of its own, so that the gallery build it
/// serves is the one a test names and not the machine's.
struct Served {
    base: String,
    process: tokio::process::Child,
    /// Kept open so the process can go on writing to it.
    _stdout: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
}

impl Served {
    /// Starts a leaf that sets up `data_dir` (`leaf.conf`, the database and
    /// one archived day) and serves it with the gallery build in
    /// `static_dir`. `gateway` is where its bot stands: `online` or
    /// `offline`. Comes back once the leaf is listening.
    async fn start(data_dir: &Path, static_dir: &Path, gateway: &str) -> Self {
        let mut process = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", SERVER, "--ignored", "--nocapture"])
            .env(SERVE_DATA_DIR, data_dir)
            .env(SERVE_GATEWAY, gateway)
            .env("STATIC_DIR", static_dir)
            // Its lifeline: closed when this test ends, whichever way.
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = tokio::io::BufReader::new(process.stdout.take().unwrap()).lines();
        let base = loop {
            let Some(line) = stdout.next_line().await.unwrap() else {
                panic!("the server process ended before it was listening");
            };
            if let Some((_, base)) = line.split_once(SERVING) {
                break base.trim().to_owned();
            }
        };
        Self {
            base,
            process,
            _stdout: stdout,
        }
    }

    async fn stop(mut self) {
        self.process.kill().await.unwrap();
    }
}

/// Writes a gallery build into `dir`: the shell and the script it loads.
fn gallery_build(dir: &Path) {
    std::fs::create_dir(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), SHELL).unwrap();
    std::fs::write(dir.join(ASSET), "console.log('leaf')").unwrap();
}

/// Stands in for a leaf whose media route takes a request and hangs up
/// without a word, as a proxy does when it loses what is behind it.
/// Everything else answers as a healthy leaf does, one request to a
/// connection.
async fn hangs_up_on_media(listener: TcpListener) {
    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(answer_or_hang_up(socket));
    }
}

async fn answer_or_hang_up(mut socket: TcpStream) {
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !head.windows(4).any(|end| end == b"\r\n\r\n") {
        match socket.read(&mut buffer).await {
            Ok(read) if read > 0 => head.extend_from_slice(&buffer[..read]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let path = head.split_whitespace().nth(1).unwrap_or_default();
    let (status, content_type, body) = match path {
        "/healthz" => ("200 OK", "text/plain", "ok"),
        "/api/status" => ("200 OK", "application/json", r#"{"gateway":"online"}"#),
        "/" => ("200 OK", "text/html", SHELL),
        "/assets/index-test.js" => ("200 OK", "text/javascript", ""),
        // The socket closes here, with the request read and nothing sent.
        media if media.starts_with("/api/media/") => return,
        _ => (
            "404 Not Found",
            "application/json",
            r#"{"error":"not_found"}"#,
        ),
    };
    let answer = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(answer.as_bytes()).await.unwrap();
    socket.shutdown().await.unwrap();
}

#[tokio::test]
async fn help_prints_the_usage_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let ran = doctor(dir.path(), &["--help"]).await;
    assert_eq!(ran.code, Some(0));
    assert!(
        ran.stdout
            .starts_with("leaf doctor: check a configured leaf")
    );
    assert!(ran.stdout.contains("Usage: leaf doctor"));
    assert_eq!(ran.stderr, "");
}

#[tokio::test]
async fn a_bad_command_line_exits_two_and_checks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ran = doctor(dir.path(), &["--bogus"]).await;
    assert_eq!(ran.code, Some(2));
    assert_eq!(ran.stdout, "");
    assert_eq!(
        ran.stderr,
        "leaf doctor has no option “--bogus”.\nleaf doctor --help lists the options.\n"
    );

    let ran = doctor(dir.path(), &["--only", "db-copy"]).await;
    assert_eq!(ran.code, Some(2));
    assert_eq!(ran.stdout, "");
    assert!(
        ran.stderr.contains("add --db-copy <path>"),
        "{}",
        ran.stderr
    );
}

#[tokio::test]
async fn an_install_that_is_not_there_fails_and_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let nowhere = dir.path().join("data");
    let ran = doctor(&nowhere, &["--only", "config,database", "--json"]).await;
    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    assert_eq!(ran.json()["ok"], false);
    assert_eq!(
        ran.outline(),
        outline([("config", "fail"), ("database", "fail")])
    );
    // A failure says what to do next.
    for result in ran.json()["results"].as_array().unwrap() {
        assert!(result["next"].as_str().is_some_and(|next| !next.is_empty()));
    }
    // leaf itself creates its data directory; the doctor creates nothing.
    assert!(!nowhere.exists());
}

#[tokio::test]
async fn json_owns_stdout_and_the_log_goes_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing listens here (bound, then released): the request fails, and
    // the doctor logs why.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let ran = doctor(dir.path(), &["--only", "url", "--url", &closed, "--json"]).await;
    assert_eq!(ran.code, Some(1), "{}", ran.stderr);
    // stdout is one JSON object and nothing else.
    let json = ran.json();
    assert_eq!(json["ok"], false);
    assert_eq!(ran.outline(), outline([("url", "fail"), ("url", "skip")]));
    assert_eq!(json["summary"]["fail"], 1);
    assert!(ran.stdout.trim_start().starts_with('{'), "{}", ran.stdout);
    // The technical cause is in the log, on stderr.
    assert!(
        ran.stderr.contains("doctor: a request got no answer"),
        "{}",
        ran.stderr
    );
}

#[tokio::test]
async fn a_running_leaf_passes_every_check_over_http_and_through_its_open_database() {
    let dir = tempfile::tempdir().unwrap();
    let build = tempfile::tempdir().unwrap();
    gallery_build(build.path());
    let leaf = Served::start(dir.path(), build.path(), "online").await;

    let ran = doctor(dir.path(), &["--only", "config,database,url", "--json"]).await;
    assert_eq!(
        ran.outline(),
        outline([
            ("config", "ok"),
            ("config", "ok"),
            ("database", "ok"),
            ("url", "ok"),
            ("url", "ok"),
            ("url", "ok"),
            ("url", "ok"),
            ("url", "ok"),
        ]),
        "{}\n{}",
        ran.stdout,
        ran.stderr
    );
    assert_eq!(ran.code, Some(0));
    assert_eq!(ran.json()["ok"], true);
    assert!(
        ran.said(1).contains("plain http on this machine"),
        "{}",
        ran.said(1)
    );
    assert!(
        ran.said(3)
            .contains(&format!("{}/healthz answers", leaf.base))
    );
    assert_eq!(ran.said(4), "The bot is online (/api/status).");
    // The real router served the shell and the script the shell names.
    assert_eq!(
        ran.said(5),
        "The gallery's shell is served, and so is the asset it loads (/assets/index-test.js)."
    );
    assert_eq!(
        ran.said(6),
        "An unknown API path answers 404 as JSON, not the gallery's page."
    );
    // The real media route took the address the doctor signed with the
    // client secret in leaf.conf, and answered one byte of the stored file.
    assert_eq!(
        ran.said(7),
        "A stored file (attachment a1) answers a byte-range request with 206."
    );
    // Nothing went wrong, so nothing was logged.
    assert_eq!(ran.stderr, "");
    ran.assert_no_secrets();
    leaf.stop().await;
}

#[tokio::test]
async fn a_leaf_without_its_gallery_and_with_its_bot_offline_fails_on_both() {
    let dir = tempfile::tempdir().unwrap();
    // A static directory with no build in it: leaf serves its placeholder.
    let no_build = tempfile::tempdir().unwrap();
    let leaf = Served::start(dir.path(), no_build.path(), "offline").await;

    let ran = doctor(dir.path(), &["--only", "url"]).await;
    assert_eq!(ran.code, Some(1), "{}\n{}", ran.stdout, ran.stderr);
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(lines.len(), 6, "{}", ran.stdout);
    assert_eq!(
        lines[0],
        format!(
            "ok    url       {}/healthz answers: the web server is up.",
            leaf.base
        )
    );
    assert!(
        lines[1].starts_with(
            "FAIL  url       The bot is not connected to Discord: Discord rejected the bot token. \
             Next: "
        ),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("FAIL  url       leaf is serving its placeholder page"),
        "{}",
        lines[2]
    );
    assert_eq!(
        lines[3],
        "ok    url       An unknown API path answers 404 as JSON, not the gallery's page."
    );
    assert_eq!(
        lines[4],
        "ok    url       A stored file (attachment a1) answers a byte-range request with 206."
    );
    assert_eq!(lines[5], "5 checked: 3 ok, 0 warn, 2 FAIL, 0 skip.");
    ran.assert_no_secrets();
    leaf.stop().await;
}

#[tokio::test]
async fn a_media_request_without_an_answer_leaves_its_signed_address_out_of_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let pool = install(dir.path(), &base).await;
    let stand_in = tokio::spawn(hangs_up_on_media(listener));

    // At the doctor's own log level, and with everything it and the HTTP
    // client under it can say.
    for log_level in [None, Some("trace")] {
        let ran = doctor_logging(dir.path(), &["--only", "url", "--json"], log_level).await;
        assert_eq!(ran.code, Some(1), "{}\n{}", ran.stdout, ran.stderr);
        assert_eq!(
            ran.outline(),
            outline([
                ("url", "ok"),
                ("url", "ok"),
                ("url", "ok"),
                ("url", "ok"),
                ("url", "fail"),
            ]),
            "{}\n{}",
            ran.stdout,
            ran.stderr
        );
        assert_eq!(
            ran.said(4),
            "The file of attachment a1 did not answer: no connection could be made."
        );
        // The failed request is in the log. The address it was sent to is
        // not: its query is a signature that opens the file.
        assert!(
            ran.stderr.contains("doctor: a request got no answer"),
            "{}",
            ran.stderr
        );
        ran.assert_no_secrets();
    }
    stand_in.abort();
    pool.close().await;
}

#[tokio::test]
async fn a_database_in_use_is_rehearsed_as_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    // Held open, as a running leaf holds its database.
    let pool = install(dir.path(), "http://127.0.0.1:3777").await;
    let db = dir.path().join("leaf.db");

    let ran = doctor(
        dir.path(),
        &["--only", "db-copy", "--db-copy", db.to_str().unwrap()],
    )
    .await;
    assert_eq!(ran.code, Some(0), "{}\n{}", ran.stdout, ran.stderr);
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{}", ran.stdout);
    assert!(
        lines[0].starts_with("ok    db-copy   A copy of leaf.db needs no migration"),
        "{}",
        ran.stdout
    );
    assert_eq!(
        lines[1],
        "ok    db-copy   The migrated copy passes integrity_check and foreign_key_check."
    );
    assert_eq!(
        lines[2],
        "ok    db-copy   The row counts are the same before and after the migrations: 1 series, \
         1 post and 1 media file."
    );
    assert_eq!(lines[3], "3 checked: 3 ok, 0 warn, 0 FAIL, 0 skip.");
    assert_eq!(ran.stderr, "");

    // The running leaf was not disturbed: it still reads and writes.
    sqlx::query("INSERT INTO guild_settings (guild_id) VALUES ('g2')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
