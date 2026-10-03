//! Media proxy: streams an attachment's bytes from R2 behind a signed URL.
//!
//! No Bearer auth here — the `sig`/`exp` query pair is the capability (see
//! `auth::MediaSigner`), because the gallery loads these via `<img src>`.
//! Bytes stream from object storage rather than buffering, and responses
//! carry immutable cache headers so Discord's proxy and Cloudflare's edge
//! serve repeat views without touching R2 (see PLAN.md § caching).
//!
//! Byte ranges: a `<video>` asks for parts of a file (`Range: bytes=a-b`) so
//! it can start before the download ends and seek to any point, and iOS will
//! not play a source that cannot answer such a request. A request for one
//! range is answered with `206` and just those bytes; every answer states
//! its length, `Accept-Ranges: bytes` and the object's `ETag`.

use std::ops::Range;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use object_store::path::Path as ObjectPath;
use object_store::{GetOptions, GetRange, GetResult, ObjectMeta, ObjectStore};
use serde::Deserialize;

use crate::api::auth::{DiscordApi, now_unix};
use crate::api::error::ApiError;
use crate::api::state::ApiState;

/// Signed-URL query parameters.
#[derive(Debug, Deserialize)]
pub struct MediaQuery {
    /// Expiry (unix seconds) the signature was minted for.
    exp: i64,
    /// Base64 HMAC over `media\0{attachment_id}\0{exp}`.
    sig: String,
    /// Any value selects the thumbnail variant.
    thumb: Option<String>,
}

/// Immutable, year-long cache: archive media never changes under a key.
/// This is what a browser may do with its own copy; what a cache shared
/// between people may do is narrowed per request by [`cache_control`].
const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Longest a shared cache is asked to keep a media answer: two signing
/// buckets, the longest a signed URL can live.
const SHARED_CACHE_MAX_SECS: i64 = 2 * crate::api::auth::SIGNED_URL_BUCKET_SECS;

/// The `Cache-Control` for an answer to a URL signed until `exp`.
///
/// The signature is only checked here, at the origin. A shared cache (the
/// owner's CDN, Discord's proxy) that kept the answer for the year a browser
/// may would go on serving the URL long after it expired, so `s-maxage` caps
/// shared caching at the URL's remaining life. URLs are re-minted every
/// signing bucket, so this costs the edge nothing it would have used.
fn cache_control(exp: i64, now_unix: i64) -> HeaderValue {
    let shared = exp.saturating_sub(now_unix).clamp(0, SHARED_CACHE_MAX_SECS);
    HeaderValue::from_str(&format!(
        "public, max-age=31536000, s-maxage={shared}, immutable"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(CACHE_CONTROL))
}

/// Sent when a stored content type cannot be written as a header value.
const FALLBACK_CONTENT_TYPE: &str = "application/octet-stream";

/// `GET /api/media/:attachment_id` — verify the signature, then stream the
/// original (or `?thumb=1` the thumbnail) from R2, in whole or the one byte
/// range the request asks for.
pub async fn media<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(attachment_id): Path<String>,
    Query(q): Query<MediaQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !st
        .key
        .verify_media(&attachment_id, q.exp, &q.sig, now_unix())
    {
        return Err(ApiError::Forbidden);
    }

    let Some((original_key, thumb_key, content_type)) =
        st.posts.media_location(&attachment_id).await?
    else {
        return Err(ApiError::NotFound);
    };

    let want_thumb = q.thumb.is_some();
    // Missing bytes (imported placeholder) → 404. A future enhancement can
    // 302 to a refreshed Discord CDN URL when the source still exists.
    let (key, ct) = if want_thumb {
        (thumb_key, "image/webp".to_owned())
    } else {
        (original_key, content_type)
    };
    let Some(key) = key else {
        return Err(ApiError::NotFound);
    };

    match serve(st.store.as_ref(), &ObjectPath::from(key), &ct, &headers).await {
        Ok(mut response) => {
            // Every answer about the object (200, 206, 304) is cacheable;
            // bound how long a shared cache may answer for the origin.
            if let Some(value) = response.headers_mut().get_mut(header::CACHE_CONTROL) {
                *value = cache_control(q.exp, now_unix());
            }
            Ok(response)
        }
        Err(object_store::Error::NotFound { .. }) => Err(ApiError::NotFound),
        Err(e) => {
            tracing::error!(error = %e, attachment = %attachment_id, "media fetch from store failed");
            Err(ApiError::Internal)
        }
    }
}

/// One byte range from a `Range` header, before the object's size is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteRange {
    /// `bytes=first-last`: both positions inclusive.
    Span {
        /// First byte wanted.
        first: u64,
        /// Last byte wanted; may lie past the end of the object.
        last: u64,
    },
    /// `bytes=first-`: from `first` to the end.
    From(u64),
    /// `bytes=-n`: the last `n` bytes.
    Suffix(u64),
}

impl ByteRange {
    /// The half-open span this range selects in an object of `size` bytes,
    /// or `None` when it selects nothing (the request is answered with 416).
    fn resolve(self, size: u64) -> Option<Range<u64>> {
        match self {
            Self::Span { first, last } => {
                (first < size).then(|| first..last.saturating_add(1).min(size))
            }
            Self::From(first) => (first < size).then_some(first..size),
            Self::Suffix(n) => (n > 0 && size > 0).then(|| size.saturating_sub(n)..size),
        }
    }

    /// The same range in the store's terms, so the store can resolve it and
    /// report the object's size in one round trip. `None` for `bytes=-0`,
    /// which selects nothing in any object.
    const fn for_store(self) -> Option<GetRange> {
        match self {
            Self::Span { first, last } => Some(GetRange::Bounded(first..last.saturating_add(1))),
            Self::From(first) => Some(GetRange::Offset(first)),
            Self::Suffix(0) => None,
            Self::Suffix(n) => Some(GetRange::Suffix(n)),
        }
    }
}

/// Parses a `Range` header that asks for exactly one byte range.
///
/// `None` for anything leaf does not answer in part: another unit, several
/// ranges, or broken syntax. The caller then sends the whole file with 200,
/// which RFC 9110 § 14.2 permits and every client copes with.
fn parse_range(header: &str) -> Option<ByteRange> {
    let (unit, spec) = header.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") || spec.contains(',') {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    match (first.trim(), last.trim()) {
        ("", "") => None,
        ("", n) => position(n).map(ByteRange::Suffix),
        (first, "") => position(first).map(ByteRange::From),
        (first, last) => {
            let (first, last) = (position(first)?, position(last)?);
            (first <= last).then_some(ByteRange::Span { first, last })
        }
    }
}

/// A byte position or length: decimal digits only (`u64::from_str` would
/// also take a sign). A number too large for `u64` is past the end of any
/// file, so it saturates rather than failing.
fn position(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(digits.parse().unwrap_or(u64::MAX))
}

/// What the store gave back for a request that named a byte range.
enum Opened {
    /// The selected part of the object.
    Part(GetResult),
    /// The whole object, because the store could not read just the part.
    Whole(GetResult),
    /// The range selects nothing in an object of this many bytes.
    Unsatisfiable(u64),
}

/// Opens `range` of the object at `path`.
///
/// The usual case is one request: the store resolves the range and its
/// answer carries the object's full size. When that request fails for any
/// reason other than a missing object, the size is read separately to tell
/// a range that lies outside the file (416) from a store that would not
/// serve this one. In the second case the whole file is sent instead, which
/// a client must accept in answer to a range request, so a ranged read can
/// never do worse than a plain one.
async fn open_range(
    store: &dyn ObjectStore,
    path: &ObjectPath,
    range: ByteRange,
) -> object_store::Result<Opened> {
    let mut failure = None;
    if let Some(wanted) = range.for_store() {
        let options = GetOptions {
            range: Some(wanted),
            ..GetOptions::default()
        };
        match store.get_opts(path, options).await {
            // A store may answer `bytes=-n` on an empty object with an empty
            // part; there is nothing to send, so it is unsatisfiable too.
            Ok(got) if got.range.is_empty() => return Ok(Opened::Unsatisfiable(got.meta.size)),
            Ok(got) => return Ok(Opened::Part(got)),
            Err(e @ object_store::Error::NotFound { .. }) => return Err(e),
            Err(e) => failure = Some(e),
        }
    }

    let size = store.head(path).await?.size;
    if range.resolve(size).is_none() {
        return Ok(Opened::Unsatisfiable(size));
    }
    if let Some(e) = failure {
        tracing::warn!(error = %e, ?range, size, "store could not read a byte range; sending the whole file");
    }
    store.get(path).await.map(Opened::Whole)
}

/// Builds the response for the object at `path`: the whole of it, the part a
/// `Range` header asks for, `304` when `If-None-Match` shows the client
/// already holds it, or `416` when the range lies outside it.
async fn serve(
    store: &dyn ObjectStore,
    path: &ObjectPath,
    content_type: &str,
    request: &HeaderMap,
) -> object_store::Result<Response> {
    let mut range = request
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range);

    // Conditions are settled before the range is looked at (RFC 9110
    // § 13.2.2), from the object's metadata alone, so a 304 or a part that
    // no longer fits the client's copy costs no read of the body.
    let if_range = range.and_then(|_| request.get(header::IF_RANGE));
    if request.contains_key(header::IF_NONE_MATCH) || if_range.is_some() {
        let etag = entity_tag(&store.head(path).await?);
        if etag.as_ref().is_some_and(|etag| none_match(request, etag)) {
            return Ok((StatusCode::NOT_MODIFIED, object_headers(etag)).into_response());
        }
        // `If-Range`: the client holds part of an earlier copy and wants the
        // rest only if the file is still that copy. If it is not (or leaf
        // cannot tell), the part would not fit what the client has, so the
        // range is dropped and the whole file sent.
        if if_range.is_some_and(|held| !if_range_holds(held, etag.as_ref())) {
            range = None;
        }
    }

    let (got, partial) = match range {
        None => (store.get(path).await?, false),
        Some(range) => match open_range(store, path, range).await? {
            Opened::Part(got) => (got, true),
            Opened::Whole(got) => (got, false),
            Opened::Unsatisfiable(size) => return Ok(unsatisfiable(size)),
        },
    };

    let mut headers = object_headers(entity_tag(&got.meta));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static(FALLBACK_CONTENT_TYPE)),
    );
    let Range { start, end } = got.range;
    // The length lets a player size its seek bar and lets the edge cache the
    // file; without it the body goes out chunked and neither can happen.
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(end.saturating_sub(start)),
    );
    let status = if partial {
        headers.insert(
            header::CONTENT_RANGE,
            numeric_header(format_args!(
                "bytes {start}-{}/{}",
                end.saturating_sub(1),
                got.meta.size
            )),
        );
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    Ok((status, headers, Body::from_stream(got.into_stream())).into_response())
}

/// What every answer about an object carries, the 304 included: its cache
/// lifetime and, when the store reports one, its entity tag.
fn object_headers(etag: Option<HeaderValue>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    if let Some(etag) = etag {
        headers.insert(header::ETAG, etag);
    }
    headers
}

/// `416`: the range starts at or past the end of the object. The
/// `Content-Range` tells the client how long the object really is.
fn unsatisfiable(size: u64) -> Response {
    let mut response =
        ApiError::Coded(StatusCode::RANGE_NOT_SATISFIABLE, "range_not_satisfiable").into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_RANGE,
        numeric_header(format_args!("bytes */{size}")),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

/// A `Content-Range` value, written from digits and fixed ASCII punctuation.
fn numeric_header(value: std::fmt::Arguments<'_>) -> HeaderValue {
    // Such text is always a valid header value; the fallback (a range of
    // unknown extent) only keeps this free of a panic path.
    HeaderValue::from_str(&value.to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("bytes */*"))
}

/// The object's entity tag as an `ETag` header value. S3 and R2 report it
/// already quoted; other stores report a bare token, which HTTP wants in
/// quotes.
fn entity_tag(meta: &ObjectMeta) -> Option<HeaderValue> {
    let raw = meta.e_tag.as_deref()?;
    let opaque = opaque_tag(raw);
    let tag = if opaque.len() >= 2 && opaque.starts_with('"') && opaque.ends_with('"') {
        raw.to_owned()
    } else if raw.is_empty() || raw.contains('"') {
        return None;
    } else {
        format!("\"{raw}\"")
    };
    HeaderValue::from_str(&tag).ok()
}

/// An entity tag without its weakness marker.
fn opaque_tag(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}

/// Whether `If-None-Match` names `etag`, meaning the client already holds
/// this file (weak comparison, RFC 9110 § 13.1.2).
fn none_match(request: &HeaderMap, etag: &HeaderValue) -> bool {
    let Ok(ours) = etag.to_str() else {
        return false;
    };
    let ours = opaque_tag(ours);
    request
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|list| list.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || opaque_tag(candidate) == ours)
}

/// Whether an `If-Range` condition holds, so the requested part may be sent
/// (strong comparison, RFC 9110 § 13.1.5). leaf sends no `Last-Modified`, so
/// a date here can never be one it issued and does not hold.
fn if_range_holds(held: &HeaderValue, etag: Option<&HeaderValue>) -> bool {
    etag.is_some_and(|etag| held == etag && !etag.as_bytes().starts_with(b"W/"))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; slicing the fixture is fine in assertions"
    )]

    use object_store::memory::InMemory;

    use super::*;

    /// 26 bytes, so every position is easy to read off in an assertion.
    const BYTES: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

    async fn store() -> (InMemory, ObjectPath) {
        let store = InMemory::new();
        let path = ObjectPath::from("k-video");
        store
            .put(&path, bytes::Bytes::from_static(BYTES).into())
            .await
            .unwrap();
        (store, path)
    }

    fn request(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    async fn ask(pairs: &[(header::HeaderName, &str)]) -> Response {
        let (store, path) = store().await;
        serve(&store, &path, "video/mp4", &request(pairs))
            .await
            .unwrap()
    }

    fn header_of<'a>(response: &'a Response, name: &header::HeaderName) -> Option<&'a str> {
        response.headers().get(name).map(|v| v.to_str().unwrap())
    }

    async fn body_of(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap()
            .to_vec()
    }

    #[test]
    fn parses_the_three_range_forms() {
        assert_eq!(
            parse_range("bytes=0-1"),
            Some(ByteRange::Span { first: 0, last: 1 })
        );
        assert_eq!(parse_range("bytes=500-"), Some(ByteRange::From(500)));
        assert_eq!(parse_range("bytes=-200"), Some(ByteRange::Suffix(200)));
        // Unit is case-insensitive; stray spaces are tolerated.
        assert_eq!(
            parse_range("Bytes = 7 - 7"),
            Some(ByteRange::Span { first: 7, last: 7 })
        );
        // A position too large for u64 is simply "past the end".
        assert_eq!(
            parse_range("bytes=0-99999999999999999999999"),
            Some(ByteRange::Span {
                first: 0,
                last: u64::MAX
            })
        );
    }

    #[test]
    fn ignores_what_it_does_not_serve_in_part() {
        for header in [
            "",
            "bytes",
            "bytes=",
            "bytes=-",
            "bytes=abc-",
            "bytes=1-x",
            "bytes=+1-2",
            "bytes=-+5",
            "bytes=5-2",
            "bytes=0-1,5-9",
            "bytes=0-1, -1",
            "items=0-1",
            "bytes=1-2-3",
        ] {
            assert_eq!(parse_range(header), None, "{header:?}");
        }
    }

    #[test]
    fn resolves_against_the_object_size() {
        let span = |first, last| ByteRange::Span { first, last };
        assert_eq!(span(0, 1).resolve(26), Some(0..2));
        assert_eq!(span(25, 25).resolve(26), Some(25..26));
        // An end past the object is cut to it; a start past it is not served.
        assert_eq!(span(20, 999).resolve(26), Some(20..26));
        assert_eq!(span(0, u64::MAX).resolve(26), Some(0..26));
        assert_eq!(span(26, 30).resolve(26), None);
        assert_eq!(ByteRange::From(10).resolve(26), Some(10..26));
        assert_eq!(ByteRange::From(26).resolve(26), None);
        assert_eq!(ByteRange::Suffix(4).resolve(26), Some(22..26));
        assert_eq!(ByteRange::Suffix(100).resolve(26), Some(0..26));
        assert_eq!(ByteRange::Suffix(0).resolve(26), None);
        // Nothing can be selected in an empty object.
        assert_eq!(span(0, 0).resolve(0), None);
        assert_eq!(ByteRange::From(0).resolve(0), None);
        assert_eq!(ByteRange::Suffix(5).resolve(0), None);
    }

    #[test]
    fn shared_caches_are_held_to_the_signature_s_remaining_life() {
        let value = |exp, now| cache_control(exp, now).to_str().unwrap().to_owned();
        assert_eq!(
            value(1_000_600, 1_000_000),
            "public, max-age=31536000, s-maxage=600, immutable"
        );
        // Never longer than a signed URL can live, never negative.
        assert_eq!(
            value(i64::MAX, 0),
            format!("public, max-age=31536000, s-maxage={SHARED_CACHE_MAX_SECS}, immutable")
        );
        assert_eq!(
            value(5, 10),
            "public, max-age=31536000, s-maxage=0, immutable"
        );
    }

    #[test]
    fn translates_ranges_for_the_store() {
        assert_eq!(
            ByteRange::Span { first: 2, last: 5 }.for_store(),
            Some(GetRange::Bounded(2..6))
        );
        assert_eq!(ByteRange::From(9).for_store(), Some(GetRange::Offset(9)));
        assert_eq!(ByteRange::Suffix(3).for_store(), Some(GetRange::Suffix(3)));
        assert_eq!(ByteRange::Suffix(0).for_store(), None);
    }

    #[test]
    fn entity_tags_are_quoted_once() {
        let meta = |e_tag: Option<&str>| ObjectMeta {
            location: ObjectPath::from("k"),
            last_modified: chrono::DateTime::UNIX_EPOCH,
            size: 1,
            e_tag: e_tag.map(ToOwned::to_owned),
            version: None,
        };
        let tag = |e_tag| entity_tag(&meta(e_tag)).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(tag(Some("\"abc-3\"")).as_deref(), Some("\"abc-3\""));
        assert_eq!(tag(Some("W/\"abc\"")).as_deref(), Some("W/\"abc\""));
        assert_eq!(tag(Some("7")).as_deref(), Some("\"7\""));
        assert_eq!(tag(Some("")), None);
        assert_eq!(tag(Some("a\"b")), None);
        assert_eq!(tag(None), None);
    }

    #[test]
    fn conditional_headers_compare_tags() {
        let etag = HeaderValue::from_static("\"v1\"");
        let none = |value| none_match(&request(&[(header::IF_NONE_MATCH, value)]), &etag);
        assert!(none("\"v1\""));
        assert!(none("\"v0\", W/\"v1\""));
        assert!(none("*"));
        assert!(!none("\"v2\""));
        assert!(!none_match(&HeaderMap::new(), &etag));

        let held = |value| HeaderValue::from_static(value);
        assert!(if_range_holds(&held("\"v1\""), Some(&etag)));
        assert!(!if_range_holds(&held("\"v2\""), Some(&etag)));
        // A date, a weak tag, or no tag to compare with: send the whole file.
        assert!(!if_range_holds(
            &held("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(&etag)
        ));
        let weak = HeaderValue::from_static("W/\"v1\"");
        assert!(!if_range_holds(&weak, Some(&weak)));
        assert!(!if_range_holds(&held("\"v1\""), None));
    }

    #[tokio::test]
    async fn whole_file_states_length_ranges_and_tag() {
        let response = ask(&[]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_of(&response, &header::CONTENT_LENGTH), Some("26"));
        assert_eq!(header_of(&response, &header::ACCEPT_RANGES), Some("bytes"));
        assert_eq!(
            header_of(&response, &header::CONTENT_TYPE),
            Some("video/mp4")
        );
        assert_eq!(
            header_of(&response, &header::CACHE_CONTROL),
            Some(CACHE_CONTROL)
        );
        let etag = header_of(&response, &header::ETAG).unwrap();
        assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
        assert_eq!(header_of(&response, &header::CONTENT_RANGE), None);
        assert_eq!(body_of(response).await, BYTES);
    }

    #[tokio::test]
    async fn a_single_range_is_answered_with_206() {
        // The probe iOS sends before it will play a video.
        let response = ask(&[(header::RANGE, "bytes=0-1")]).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_of(&response, &header::CONTENT_RANGE),
            Some("bytes 0-1/26")
        );
        assert_eq!(header_of(&response, &header::CONTENT_LENGTH), Some("2"));
        assert_eq!(header_of(&response, &header::ACCEPT_RANGES), Some("bytes"));
        assert_eq!(
            header_of(&response, &header::CACHE_CONTROL),
            Some(CACHE_CONTROL)
        );
        assert!(header_of(&response, &header::ETAG).is_some());
        assert_eq!(body_of(response).await, b"ab");

        for (range, content_range, body) in [
            ("bytes=10-", "bytes 10-25/26", &BYTES[10..]),
            ("bytes=-4", "bytes 22-25/26", &BYTES[22..]),
            ("bytes=-100", "bytes 0-25/26", BYTES),
            ("bytes=20-999", "bytes 20-25/26", &BYTES[20..]),
            ("bytes=25-25", "bytes 25-25/26", &BYTES[25..]),
        ] {
            let response = ask(&[(header::RANGE, range)]).await;
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{range}");
            assert_eq!(
                header_of(&response, &header::CONTENT_RANGE),
                Some(content_range),
                "{range}"
            );
            assert_eq!(
                header_of(&response, &header::CONTENT_LENGTH),
                Some(body.len().to_string().as_str()),
                "{range}"
            );
            assert_eq!(body_of(response).await, body, "{range}");
        }
    }

    #[tokio::test]
    async fn a_range_outside_the_file_is_416_with_its_size() {
        for range in ["bytes=26-", "bytes=26-30", "bytes=999-", "bytes=-0"] {
            let response = ask(&[(header::RANGE, range)]).await;
            assert_eq!(
                response.status(),
                StatusCode::RANGE_NOT_SATISFIABLE,
                "{range}"
            );
            assert_eq!(
                header_of(&response, &header::CONTENT_RANGE),
                Some("bytes */26"),
                "{range}"
            );
            let body: serde_json::Value = serde_json::from_slice(&body_of(response).await).unwrap();
            assert_eq!(body["error"], "range_not_satisfiable", "{range}");
        }
    }

    #[tokio::test]
    async fn an_empty_file_has_no_satisfiable_range() {
        let store = InMemory::new();
        let path = ObjectPath::from("k-empty");
        store.put(&path, bytes::Bytes::new().into()).await.unwrap();
        for range in ["bytes=0-", "bytes=-5"] {
            let response = serve(
                &store,
                &path,
                "image/png",
                &request(&[(header::RANGE, range)]),
            )
            .await
            .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::RANGE_NOT_SATISFIABLE,
                "{range}"
            );
            assert_eq!(
                header_of(&response, &header::CONTENT_RANGE),
                Some("bytes */0"),
                "{range}"
            );
        }
        let whole = serve(&store, &path, "image/png", &HeaderMap::new())
            .await
            .unwrap();
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(header_of(&whole, &header::CONTENT_LENGTH), Some("0"));
    }

    #[tokio::test]
    async fn unusable_range_headers_get_the_whole_file() {
        for range in ["bytes=0-1,5-9", "items=0-1", "bytes=5-2", "nonsense"] {
            let response = ask(&[(header::RANGE, range)]).await;
            assert_eq!(response.status(), StatusCode::OK, "{range}");
            assert_eq!(body_of(response).await, BYTES, "{range}");
        }
    }

    #[tokio::test]
    async fn if_none_match_answers_304_without_a_body() {
        let etag = header_of(&ask(&[]).await, &header::ETAG)
            .unwrap()
            .to_owned();

        let response = ask(&[(header::IF_NONE_MATCH, &etag)]).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_of(&response, &header::ETAG), Some(etag.as_str()));
        assert_eq!(
            header_of(&response, &header::CACHE_CONTROL),
            Some(CACHE_CONTROL)
        );
        assert!(body_of(response).await.is_empty());

        // It is checked before the range, even one outside the file, and a
        // different tag changes nothing.
        for range in ["bytes=0-1", "bytes=999-"] {
            let response = ask(&[(header::IF_NONE_MATCH, &etag), (header::RANGE, range)]).await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{range}");
        }
        let response = ask(&[(header::IF_NONE_MATCH, "\"something-else\"")]).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = ask(&[
            (header::IF_NONE_MATCH, "\"something-else\""),
            (header::RANGE, "bytes=999-"),
        ])
        .await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[tokio::test]
    async fn a_304_is_answered_without_reading_the_file() {
        use std::time::Duration;

        use object_store::throttle::{ThrottleConfig, ThrottledStore};

        // Every read of a body from this store takes an hour, so a 304 that
        // comes back at once came from the object's metadata alone.
        let slow = ThrottledStore::new(
            InMemory::new(),
            ThrottleConfig {
                wait_get_per_call: Duration::from_hours(1),
                ..ThrottleConfig::default()
            },
        );
        let path = ObjectPath::from("k-video");
        slow.put(&path, bytes::Bytes::from_static(BYTES).into())
            .await
            .unwrap();
        let etag = entity_tag(&slow.head(&path).await.unwrap()).unwrap();
        let etag = etag.to_str().unwrap();

        for pairs in [
            &[(header::IF_NONE_MATCH, etag)][..],
            &[(header::IF_NONE_MATCH, etag), (header::RANGE, "bytes=0-1")][..],
        ] {
            let answered = tokio::time::timeout(
                Duration::from_secs(10),
                serve(&slow, &path, "video/mp4", &request(pairs)),
            )
            .await;
            assert!(answered.is_ok(), "the 304 waited on a read of the file");
            let response = answered.unwrap().unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        }
    }

    #[tokio::test]
    async fn if_range_sends_the_part_only_for_the_same_file() {
        let etag = header_of(&ask(&[]).await, &header::ETAG)
            .unwrap()
            .to_owned();

        let response = ask(&[(header::RANGE, "bytes=2-3"), (header::IF_RANGE, &etag)]).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body_of(response).await, b"cd");

        let response = ask(&[
            (header::RANGE, "bytes=2-3"),
            (header::IF_RANGE, "\"an-older-copy\""),
        ])
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_of(&response, &header::CONTENT_RANGE), None);
        assert_eq!(header_of(&response, &header::CONTENT_LENGTH), Some("26"));
        assert_eq!(body_of(response).await, BYTES);

        // A stale copy makes the range moot, so even one outside the file is
        // answered with the whole file rather than 416.
        let response = ask(&[
            (header::RANGE, "bytes=999-"),
            (header::IF_RANGE, "\"an-older-copy\""),
        ])
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_of(response).await, BYTES);
        // An `If-Range` without a `Range` is ignored.
        let response = ask(&[(header::IF_RANGE, "\"an-older-copy\"")]).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_missing_object_is_not_found_with_or_without_a_range() {
        let store = InMemory::new();
        let path = ObjectPath::from("k-gone");
        for pairs in [
            &[][..],
            &[(header::RANGE, "bytes=0-1")][..],
            &[(header::IF_NONE_MATCH, "\"v1\"")][..],
        ] {
            let outcome = serve(&store, &path, "video/mp4", &request(pairs)).await;
            assert!(matches!(outcome, Err(object_store::Error::NotFound { .. })));
        }
    }

    #[tokio::test]
    async fn an_unwritable_content_type_falls_back() {
        let (store, path) = store().await;
        let response = serve(&store, &path, "video/mp4\nx", &HeaderMap::new())
            .await
            .unwrap();
        assert_eq!(
            header_of(&response, &header::CONTENT_TYPE),
            Some(FALLBACK_CONTENT_TYPE)
        );
    }
}
