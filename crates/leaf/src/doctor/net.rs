//! The HTTP the doctor speaks: a request, its answer, and the client that
//! carries them. The checks are written over [`Fetch`], so tests hand them
//! canned answers and only [`LiveFetch`] ever opens a connection.

use std::time::Duration;

/// Limit for each request, from connecting to the last byte read.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Most of an answer's body that is read. The answers the doctor reads are
/// small; this only keeps a server that ignores a `Range` header from
/// sending a whole video.
const BODY_MAX_BYTES: usize = 2 * 1024 * 1024;

/// How a request proves who is asking.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// Nothing: a public address.
    None,
    /// `Authorization: Bot <token>`, as Discord's REST API takes it.
    Bot(String),
    /// HTTP Basic, as Discord's OAuth token endpoint takes a client id and
    /// secret.
    Basic {
        /// The client id.
        user: String,
        /// The client secret.
        password: String,
    },
}

// Requests are compared and printed in tests and may be logged: the
// credential itself never is.
impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Bot(_) => "Bot(<redacted>)",
            Self::Basic { .. } => "Basic(<redacted>)",
        })
    }
}

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The whole address.
    pub url: String,
    /// Who is asking.
    pub auth: Auth,
    /// A `Range` header to send, e.g. `bytes=0-0`.
    pub range: Option<&'static str>,
    /// Send as a `POST` with these form fields instead of as a `GET`.
    pub form: Option<Vec<(&'static str, &'static str)>>,
}

impl Request {
    /// A plain `GET`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth: Auth::None,
            range: None,
            form: None,
        }
    }

    /// The same request, asked as `auth`.
    #[must_use]
    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }

    /// The same request, for one byte range.
    #[must_use]
    pub const fn with_range(mut self, range: &'static str) -> Self {
        self.range = Some(range);
        self
    }

    /// The same address, as a form `POST`.
    #[must_use]
    pub fn with_form(mut self, form: Vec<(&'static str, &'static str)>) -> Self {
        self.form = Some(form);
        self
    }
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The `Content-Type` header.
    pub content_type: Option<String>,
    /// The `Content-Range` header.
    pub content_range: Option<String>,
    /// The body, up to [`BODY_MAX_BYTES`].
    pub body: Vec<u8>,
}

impl Answer {
    /// The body as text.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// The body read as JSON of the expected shape, if it is that.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_slice(&self.body).ok()
    }

    /// Whether the answer says it is of this media type (`text/html`,
    /// `application/json`), whatever parameters follow.
    pub fn is_type(&self, media_type: &str) -> bool {
        self.content_type.as_deref().is_some_and(|ct| {
            ct.split(';')
                .next()
                .is_some_and(|ct| ct.trim().eq_ignore_ascii_case(media_type))
        })
    }
}

/// Why a request got no HTTP answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unanswered {
    /// Nothing came back within the time limit.
    TimedOut,
    /// No connection: DNS, a closed port, TLS, a reset.
    Unreachable,
    /// The request could not be put together: the address is not one, or a
    /// credential holds a character no header can carry.
    Unsendable,
}

/// Carries a request and brings back the answer.
pub trait Fetch: Sync {
    /// Sends `request`. Any HTTP status is an answer; `Err` means none came.
    fn send(&self, request: Request) -> impl Future<Output = Result<Answer, Unanswered>> + Send;
}

/// The client that talks to the real network.
#[derive(Debug, Clone)]
pub struct LiveFetch {
    http: reqwest::Client,
}

impl LiveFetch {
    /// Builds the client.
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
        })
    }
}

impl Fetch for LiveFetch {
    async fn send(&self, request: Request) -> Result<Answer, Unanswered> {
        let mut builder = match &request.form {
            Some(form) => self.http.post(&request.url).form(form),
            None => self.http.get(&request.url),
        };
        builder = match &request.auth {
            Auth::None => builder,
            Auth::Bot(token) => {
                builder.header(reqwest::header::AUTHORIZATION, format!("Bot {token}"))
            }
            Auth::Basic { user, password } => builder.basic_auth(user, Some(password)),
        };
        if let Some(range) = request.range {
            builder = builder.header(reqwest::header::RANGE, range);
        }
        let mut response = builder.send().await.map_err(unanswered)?;
        let header = |name: reqwest::header::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned)
        };
        let mut answer = Answer {
            status: response.status().as_u16(),
            content_type: header(reqwest::header::CONTENT_TYPE),
            content_range: header(reqwest::header::CONTENT_RANGE),
            body: Vec::new(),
        };
        while answer.body.len() < BODY_MAX_BYTES {
            match response.chunk().await.map_err(unanswered)? {
                Some(chunk) => answer.body.extend_from_slice(&chunk),
                None => break,
            }
        }
        answer.body.truncate(BODY_MAX_BYTES);
        Ok(answer)
    }
}

/// Sorts a failed request, and logs its cause without the address: a media
/// address carries its signature in the query.
fn unanswered(e: reqwest::Error) -> Unanswered {
    let kind = if e.is_builder() {
        Unanswered::Unsendable
    } else if e.is_timeout() {
        Unanswered::TimedOut
    } else {
        Unanswered::Unreachable
    };
    tracing::warn!(error = %e.without_url(), ?kind, "doctor: a request got no answer");
    kind
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::{get, post};

    use super::*;

    /// A local HTTP server for the duration of a test.
    struct Stub {
        base: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn serve(app: Router) -> Stub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Stub {
            base: format!("http://{addr}"),
            task,
        }
    }

    /// The headers of every request the stand-in got.
    type Seen = Arc<Mutex<Vec<HeaderMap>>>;

    fn seen_header(seen: &Seen, name: &header::HeaderName) -> Option<String> {
        let last = seen.lock().unwrap().last().unwrap().clone();
        last.get(name).map(|v| v.to_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn a_get_carries_the_bot_token_and_the_range_and_reads_the_answer() {
        let seen = Seen::default();
        let record = Arc::clone(&seen);
        let stub = serve(Router::new().route(
            "/thing",
            get(move |headers: HeaderMap| async move {
                record.lock().unwrap().push(headers);
                (
                    StatusCode::PARTIAL_CONTENT,
                    [
                        (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                        (header::CONTENT_RANGE, "bytes 0-0/10"),
                    ],
                    r#"{"a":1}"#,
                )
            }),
        ))
        .await;

        let request = Request::get(format!("{}/thing", stub.base))
            .with_auth(Auth::Bot("tok.en".to_owned()))
            .with_range("bytes=0-0");
        let answer = LiveFetch::new().unwrap().send(request).await.unwrap();

        assert_eq!(answer.status, 206);
        assert!(answer.is_type("application/json"));
        assert!(!answer.is_type("text/html"));
        assert_eq!(answer.content_range.as_deref(), Some("bytes 0-0/10"));
        assert_eq!(answer.text(), r#"{"a":1}"#);
        assert_eq!(
            answer.json::<serde_json::Value>(),
            Some(serde_json::json!({"a": 1}))
        );
        assert_eq!(answer.json::<Vec<u8>>(), None);
        assert_eq!(
            seen_header(&seen, &header::AUTHORIZATION).as_deref(),
            Some("Bot tok.en")
        );
        assert_eq!(
            seen_header(&seen, &header::RANGE).as_deref(),
            Some("bytes=0-0")
        );
    }

    #[tokio::test]
    async fn a_form_post_carries_basic_auth_and_any_status_is_an_answer() {
        let seen = Seen::default();
        let record = Arc::clone(&seen);
        let stub = serve(Router::new().route(
            "/token",
            post(move |headers: HeaderMap, body: String| async move {
                record.lock().unwrap().push(headers);
                (StatusCode::UNAUTHORIZED, body)
            }),
        ))
        .await;

        let request = Request::get(format!("{}/token", stub.base))
            .with_auth(Auth::Basic {
                user: "123".to_owned(),
                password: "sec".to_owned(),
            })
            .with_form(vec![("grant_type", "client_credentials")]);
        let answer = LiveFetch::new().unwrap().send(request).await.unwrap();

        assert_eq!(answer.status, 401);
        // The form arrived as the body.
        assert_eq!(answer.text(), "grant_type=client_credentials");
        // base64("123:sec")
        assert_eq!(
            seen_header(&seen, &header::AUTHORIZATION).as_deref(),
            Some("Basic MTIzOnNlYw==")
        );
        assert_eq!(seen_header(&seen, &header::RANGE), None);
    }

    #[tokio::test]
    async fn a_request_without_an_answer_says_why() {
        let live = LiveFetch::new().unwrap();

        // Nothing listens here (bound, then released).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert_eq!(
            live.send(Request::get(&closed)).await,
            Err(Unanswered::Unreachable)
        );

        // Neither an address nor a token that can be sent ever leaves.
        assert_eq!(
            live.send(Request::get("not an address")).await,
            Err(Unanswered::Unsendable)
        );
        let request = Request::get(&closed).with_auth(Auth::Bot("tok\nen".to_owned()));
        assert_eq!(live.send(request).await, Err(Unanswered::Unsendable));
    }

    #[tokio::test]
    async fn a_silent_server_times_out() {
        // Accepts connections and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let _open = socket;
                    std::future::pending::<()>().await;
                });
            }
        });
        let live = LiveFetch {
            http: reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap(),
        };
        let outcome = live.send(Request::get(format!("http://{addr}"))).await;
        hold.abort();
        assert_eq!(outcome, Err(Unanswered::TimedOut));
    }

    #[tokio::test]
    async fn a_body_past_the_limit_is_cut_not_downloaded() {
        let stub =
            serve(Router::new().route("/big", get(|| async { vec![b'x'; BODY_MAX_BYTES + 4096] })))
                .await;
        let answer = LiveFetch::new()
            .unwrap()
            .send(Request::get(format!("{}/big", stub.base)))
            .await
            .unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body.len(), BODY_MAX_BYTES);
    }

    #[test]
    fn a_printed_request_shows_no_credential() {
        let bot = Request::get("https://discord.test").with_auth(Auth::Bot("SECRET_A".to_owned()));
        let basic = Request::get("https://discord.test").with_auth(Auth::Basic {
            user: "123".to_owned(),
            password: "SECRET_B".to_owned(),
        });
        let printed = format!("{bot:?} {basic:?}");
        assert!(!printed.contains("SECRET"), "{printed}");
        assert!(printed.contains("<redacted>"));
    }
}
