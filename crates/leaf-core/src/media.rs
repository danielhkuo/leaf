//! The media pipeline: bytes in, originals + thumbnails durably in object
//! storage, keys out.
//!
//! Downloads stream to a temp file (never whole-file in RAM), originals
//! stream to storage (multipart above a threshold), and thumbnails are
//! generated once at archive time — a small WebP for images, an `ffmpeg`
//! poster frame for video. A file that cannot be previewed (undecodable
//! image, ffmpeg unavailable, failing or stalled) still archives, with a
//! deterministic placeholder thumbnail. All CPU-bound image work runs on
//! the blocking pool.

use std::io::Cursor;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use image::DynamicImage;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, WriteMultipart};
use tokio::io::AsyncReadExt as _;
use tracing::Instrument as _;

/// Thumbnail long-edge in pixels (gallery grid / heatmap tiles).
pub const THUMB_LONG_EDGE: u32 = 256;

/// Default per-file size cap in bytes (100 MB).
pub const DEFAULT_MAX_BYTES: u64 = 100 * 1024 * 1024;

/// Files larger than this stream to storage via multipart upload.
const DEFAULT_MULTIPART_THRESHOLD: u64 = 8 * 1024 * 1024;

/// Total time ffmpeg may spend on one video's poster frame (both attempts)
/// before the placeholder thumbnail is used instead.
const POSTER_TIMEOUT: Duration = Duration::from_secs(20);

/// Where the poster frame is taken, in seconds from the start: past the
/// black or fading first frame of edited clips and screen recordings.
const POSTER_SEEK_SECS: &str = "1";

/// ffmpeg filter that shrinks the poster frame to fit 512×512 (never
/// enlarging), so a 4K frame is not written and decoded at full size only
/// to be resized to [`THUMB_LONG_EDGE`] afterwards.
const POSTER_SCALE_FILTER: &str =
    "scale='min(512,iw)':'min(512,ih)':force_original_aspect_ratio=decrease";

/// How much of ffmpeg's stderr, in characters, goes into the log line for
/// a failed poster.
const STDERR_LOG_MAX_CHARS: usize = 400;

/// Longest filename, in characters, quoted back in a user-facing message.
const FILENAME_DISPLAY_MAX_CHARS: usize = 60;

/// Longest extension, in characters, kept on the end of a filename that is
/// cut to [`FILENAME_DISPLAY_MAX_CHARS`].
const FILENAME_EXTENSION_MAX_CHARS: usize = 8;

/// Content types leaf will archive.
pub const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/webp",
    "image/gif",
    "video/mp4",
    "video/webm",
    "video/quicktime",
];

/// The archivable formats as users know them, for messages about a file
/// that was skipped. Keep in step with [`ALLOWED_CONTENT_TYPES`].
pub const SUPPORTED_FORMATS: &str = "PNG, JPEG, WebP, GIF, MP4, WebM, MOV";

/// Declared types that say nothing about the file, so the filename decides.
const GENERIC_CONTENT_TYPES: &[&str] = &["application/octet-stream", "binary/octet-stream"];

/// Filename extensions and the content type each one stands for.
const EXTENSION_CONTENT_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
];

/// The content type to archive an attachment under, or `None` when leaf
/// does not archive that kind of file.
///
/// Discord's `declared` type wins when it names a real type (parameters and
/// letter case are ignored, `image/jpg` becomes `image/jpeg`). When it is
/// missing, blank or the generic `application/octet-stream`, the filename
/// extension decides instead, so an upload Discord did not label is not
/// mistaken for an unsupported file. Put the result in
/// [`MediaMeta::content_type`].
#[must_use]
pub fn content_type_for(filename: &str, declared: Option<&str>) -> Option<&'static str> {
    let declared = declared.map(base_type).filter(|d| {
        !d.is_empty()
            && !GENERIC_CONTENT_TYPES
                .iter()
                .any(|g| g.eq_ignore_ascii_case(d))
    });
    if let Some(declared) = declared {
        if declared.eq_ignore_ascii_case("image/jpg") {
            return Some("image/jpeg");
        }
        return ALLOWED_CONTENT_TYPES
            .iter()
            .copied()
            .find(|allowed| allowed.eq_ignore_ascii_case(declared));
    }

    let (_, extension) = filename.trim().rsplit_once('.')?;
    EXTENSION_CONTENT_TYPES
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(extension))
        .map(|&(_, content_type)| content_type)
}

/// The sentence for a file leaf skipped because of its format, for callers
/// that filter with [`content_type_for`] and so have no [`MediaError`].
#[must_use]
pub fn unsupported_message(filename: &str) -> String {
    format!(
        "{} isn't a supported format ({SUPPORTED_FORMATS}).",
        file_ref(filename)
    )
}

/// Errors from the media pipeline, split by phase so callers can tell a
/// user problem (too large, wrong type) from an infrastructure problem.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// Downloading from the source URL failed.
    #[error("fetching media: {0}")]
    Fetch(String),
    /// The source refused the download for good (deleted attachment,
    /// expired link): asking again gets the same answer.
    #[error("media no longer available at its source: {0}")]
    Gone(String),
    /// The file exceeds the configured size cap.
    #[error("file exceeds the {limit_mb} MB limit")]
    TooLarge {
        /// The configured cap, in megabytes.
        limit_mb: u64,
    },
    /// The content type is not archivable.
    #[error("unsupported content type: {0}")]
    UnsupportedType(String),
    /// Decoding / thumbnailing failed.
    #[error("transforming media: {0}")]
    Transform(String),
    /// Object storage failed.
    #[error("storing media: {0}")]
    Store(#[from] object_store::Error),
    /// Local temp-file IO failed.
    #[error("media io: {0}")]
    Io(#[from] std::io::Error),
}

impl MediaError {
    /// What to tell the person whose file `filename` failed: what happened
    /// and what to do next, with none of the internal detail `Display`
    /// carries (CDN URLs, the storage endpoint, decoder messages). Log the
    /// error itself for the operator; send only this to chat.
    ///
    /// The text is about this one file. It does not say what became of the
    /// rest of the post; the caller adds that.
    #[must_use]
    pub fn user_message(&self, filename: &str) -> String {
        let file = file_ref(filename);
        match self {
            Self::TooLarge { limit_mb: 0 } => format!(
                "{file} is over leaf's size limit. Trim or compress it, then post it again."
            ),
            Self::TooLarge { limit_mb } => format!(
                "{file} is over leaf's {limit_mb} MB limit. \
                 Trim or compress it, then post it again."
            ),
            Self::UnsupportedType(_) => unsupported_message(filename),
            Self::Fetch(_) => {
                format!("{file} couldn't be downloaded from Discord. Try again in a moment.")
            }
            Self::Gone(_) => format!(
                "{file} is no longer available on Discord. \
                 Check the message still has the file, then archive it again."
            ),
            Self::Transform(_) => format!(
                "{file} couldn't be processed, so it wasn't archived. \
                 Tell a server admin if this keeps happening."
            ),
            Self::Store(_) | Self::Io(_) => format!(
                "{file} wasn't archived because leaf's storage is unavailable right now. \
                 Try again in a few minutes, and tell a server admin if this keeps happening."
            ),
        }
    }

    /// Whether trying the same file again can succeed: true for download
    /// and storage trouble, false when the file itself is the problem (too
    /// large, unsupported, unprocessable) or Discord no longer has it.
    /// Callers offer Retry for the first kind and skip the file for the
    /// second.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Fetch(_) | Self::Store(_) | Self::Io(_))
    }
}

/// How a message refers to a file: its name as inline code, cleaned so it
/// cannot break out of the code span or flood the message, or "That file"
/// when there is no usable name.
///
/// A name over [`FILENAME_DISPLAY_MAX_CHARS`] is cut in the middle so a
/// short extension survives: in "isn't a supported format" the extension
/// is the part that explains the message.
fn file_ref(filename: &str) -> String {
    let clean: String = filename
        .chars()
        .filter(|c| *c != '`' && !c.is_control())
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return "That file".to_owned();
    }
    if clean.chars().count() <= FILENAME_DISPLAY_MAX_CHARS {
        return format!("`{clean}`");
    }
    let extension = clean
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| {
            (1..=FILENAME_EXTENSION_MAX_CHARS).contains(&extension.chars().count())
                && extension.chars().all(char::is_alphanumeric)
        });
    let Some(extension) = extension else {
        let head: String = clean.chars().take(FILENAME_DISPLAY_MAX_CHARS).collect();
        return format!("`{head}…`");
    };
    // The dot and the extension come out of the head's share.
    let kept = FILENAME_DISPLAY_MAX_CHARS.saturating_sub(extension.chars().count() + 1);
    let head: String = clean.chars().take(kept).collect();
    format!("`{head}….{extension}`")
}

/// A download failure for the operator's log: reqwest's message without
/// the request URL (Discord CDN URLs are signed) but with the cause chain,
/// which reqwest's own `Display` leaves out.
///
/// A status that will not change on a second request becomes
/// [`MediaError::Gone`]; everything else is [`MediaError::Fetch`].
fn fetch_error(e: reqwest::Error) -> MediaError {
    let gone = e.status().is_some_and(is_permanent_status);
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    if gone {
        MediaError::Gone(text)
    } else {
        MediaError::Fetch(text)
    }
}

/// Whether a download answered with `status` fails the same way every
/// time: a 4xx (the CDN's 403/404 for a deleted attachment or an expired
/// link), other than the two that ask the client to come back later.
fn is_permanent_status(status: reqwest::StatusCode) -> bool {
    status.is_client_error()
        && status != reqwest::StatusCode::REQUEST_TIMEOUT
        && status != reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// Identifies where an attachment lands in storage.
#[derive(Debug, Clone)]
pub struct MediaMeta {
    /// Guild snowflake.
    pub guild_id: String,
    /// Series id.
    pub series_id: i64,
    /// Day number.
    pub day: i64,
    /// Discord attachment snowflake.
    pub attachment_id: String,
    /// MIME type as reported by Discord, or as [`content_type_for`]
    /// worked it out when Discord reported none.
    pub content_type: String,
}

/// Result of archiving one attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMedia {
    /// Object key of the original file.
    pub original_key: String,
    /// Object key of the WebP thumbnail.
    pub thumb_key: String,
    /// Original size in bytes.
    pub size: u64,
}

/// [`StoredMedia`] plus how its thumbnail came out, for callers that tell
/// the user. (`StoredMedia` itself keeps its shape: other crates build it
/// by literal.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedMedia {
    /// Where the original and the thumbnail were stored.
    pub stored: StoredMedia,
    /// True when no preview could be made from the file and the thumbnail
    /// is the generic placeholder tile. The original is archived intact.
    pub placeholder_thumb: bool,
}

/// Object key of an original: `g/<guild>/s/<series>/d/<day>/<attachment>`.
#[must_use]
pub fn original_key(m: &MediaMeta) -> String {
    format!(
        "g/{}/s/{}/d/{}/{}",
        m.guild_id, m.series_id, m.day, m.attachment_id
    )
}

/// Object key of a thumbnail: original path with a `thumb/` leaf + `.webp`.
#[must_use]
pub fn thumb_key(m: &MediaMeta) -> String {
    format!(
        "g/{}/s/{}/d/{}/thumb/{}.webp",
        m.guild_id, m.series_id, m.day, m.attachment_id
    )
}

/// Builds the R2-backed object store from Tier-1 configuration.
pub fn r2_store(cfg: &crate::config::R2Config) -> Result<Arc<dyn ObjectStore>, MediaError> {
    let store = object_store::aws::AmazonS3Builder::new()
        .with_endpoint(&cfg.endpoint)
        .with_bucket_name(&cfg.bucket)
        .with_access_key_id(&cfg.access_key_id)
        .with_secret_access_key(&cfg.secret_access_key)
        .with_region("auto")
        .build()?;
    Ok(Arc::new(store))
}

/// The pipeline. Cheap to clone; share one per process.
#[derive(Clone)]
pub struct MediaPipeline {
    store: Arc<dyn ObjectStore>,
    http: reqwest::Client,
    max_bytes: u64,
    multipart_threshold: u64,
    ffmpeg_bin: String,
}

impl std::fmt::Debug for MediaPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPipeline")
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl MediaPipeline {
    /// Builds a pipeline over `store` with default limits.
    pub fn new(store: Arc<dyn ObjectStore>) -> Result<Self, MediaError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_mins(2))
            .build()
            .map_err(|e| MediaError::Fetch(e.to_string()))?;
        Ok(Self {
            store,
            http,
            max_bytes: DEFAULT_MAX_BYTES,
            multipart_threshold: DEFAULT_MULTIPART_THRESHOLD,
            ffmpeg_bin: "ffmpeg".to_owned(),
        })
    }

    /// Overrides the size cap (e.g. from guild policy).
    #[must_use]
    pub const fn with_max_bytes(mut self, max: u64) -> Self {
        self.max_bytes = max;
        self
    }

    /// Test/tuning hook: multipart threshold in bytes.
    #[must_use]
    pub const fn with_multipart_threshold(mut self, threshold: u64) -> Self {
        self.multipart_threshold = threshold;
        self
    }

    /// Test hook: which `ffmpeg` binary to invoke for video posters.
    #[must_use]
    pub fn with_ffmpeg_bin(mut self, bin: impl Into<String>) -> Self {
        self.ffmpeg_bin = bin.into();
        self
    }

    /// Downloads `url` and archives it (original + thumbnail).
    ///
    /// See [`Self::archive_file_detailed`] for what may be left in storage
    /// when this fails.
    pub async fn archive_from_url(
        &self,
        url: &str,
        meta: &MediaMeta,
    ) -> Result<StoredMedia, MediaError> {
        self.archive_from_url_detailed(url, meta)
            .await
            .map(|archived| archived.stored)
    }

    /// [`Self::archive_from_url`], also reporting whether the thumbnail is
    /// the placeholder.
    pub async fn archive_from_url_detailed(
        &self,
        url: &str,
        meta: &MediaMeta,
    ) -> Result<ArchivedMedia, MediaError> {
        check_content_type(&meta.content_type)?;

        let tmp = tempfile::NamedTempFile::new()?;
        let tmp_path = tmp.path().to_path_buf();

        let mut resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(fetch_error)?
            .error_for_status()
            .map_err(fetch_error)?;

        // Stream to disk with the cap enforced as bytes arrive — a lying
        // Content-Length header cannot bypass it.
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        let mut written: u64 = 0;
        while let Some(chunk) = resp.chunk().await.map_err(fetch_error)? {
            written += chunk.len() as u64;
            if written > self.max_bytes {
                return Err(MediaError::TooLarge {
                    limit_mb: self.max_bytes / (1024 * 1024),
                });
            }
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);

        self.archive_file_detailed(&tmp_path, meta).await
    }

    /// Archives an already-downloaded file (also the migrator's entry).
    ///
    /// See [`Self::archive_file_detailed`] for what may be left in storage
    /// when this fails.
    pub async fn archive_file(
        &self,
        path: &Path,
        meta: &MediaMeta,
    ) -> Result<StoredMedia, MediaError> {
        self.archive_file_detailed(path, meta)
            .await
            .map(|archived| archived.stored)
    }

    /// [`Self::archive_file`], also reporting whether the thumbnail is the
    /// placeholder.
    ///
    /// A file that cannot be previewed still archives. On `Err`, an object
    /// may already sit under [`original_key`] or [`thumb_key`] for `meta`
    /// (a storage failure part-way). Nothing is removed here: after a day
    /// is moved, another day's rows can hold the same keys, and only the
    /// database knows. The caller releases those two keys once
    /// [`PostRepo::unreferenced_keys`](crate::db::PostRepo::unreferenced_keys)
    /// confirms no row references them.
    pub async fn archive_file_detailed(
        &self,
        path: &Path,
        meta: &MediaMeta,
    ) -> Result<ArchivedMedia, MediaError> {
        check_content_type(&meta.content_type)?;
        let size = tokio::fs::metadata(path).await?.len();
        if size > self.max_bytes {
            return Err(MediaError::TooLarge {
                limit_mb: self.max_bytes / (1024 * 1024),
            });
        }

        // The thumbnail is made while the original uploads: a video's
        // poster frame can take seconds, and neither step needs the other.
        let orig_key = original_key(meta);
        let ((), (thumb, placeholder_thumb)) = tokio::try_join!(
            self.upload_file(path, &orig_key, size),
            self.make_thumbnail(path, meta),
        )?;

        let t_key = thumb_key(meta);
        self.store
            .put(&ObjectPath::from(t_key.clone()), thumb.into())
            .await?;

        Ok(ArchivedMedia {
            stored: StoredMedia {
                original_key: orig_key,
                thumb_key: t_key,
                size,
            },
            placeholder_thumb,
        })
    }

    /// Fetches an object's bytes (e.g. a thumbnail for a chat embed).
    pub async fn get_bytes(&self, key: &str) -> Result<Vec<u8>, MediaError> {
        let got = self.store.get(&ObjectPath::from(key.to_owned())).await?;
        Ok(got.bytes().await?.to_vec())
    }

    /// Best-effort removal of stored objects (post deletion). Failures are
    /// logged, not returned: the DB row is already gone and an orphaned
    /// object is preferable to a phantom database entry.
    pub async fn delete_keys(&self, keys: &[String]) {
        for key in keys {
            if let Err(e) = self.store.delete(&ObjectPath::from(key.clone())).await {
                tracing::warn!(key, error = %e, "failed to delete stored media object");
            }
        }
    }

    /// Uploads a file: single put when small, multipart stream when large.
    async fn upload_file(&self, path: &Path, key: &str, size: u64) -> Result<(), MediaError> {
        let object_path = ObjectPath::from(key.to_owned());
        if size <= self.multipart_threshold {
            let bytes = tokio::fs::read(path).await?;
            self.store.put(&object_path, bytes.into()).await?;
            return Ok(());
        }

        let upload = self.store.put_multipart(&object_path).await?;
        let mut writer = WriteMultipart::new(upload);
        let mut file = tokio::fs::File::open(path).await?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            writer.write(buf.get(..n).unwrap_or(&buf));
        }
        writer.finish().await?;
        Ok(())
    }

    /// Produces the WebP thumbnail bytes for an image or video file, and
    /// whether they are the placeholder.
    ///
    /// A file that cannot be previewed (an image the decoder rejects or
    /// that is over its limits, a video with no extractable frame) gets
    /// the placeholder rather than failing the archive: the original is
    /// what matters, and Discord shows plenty of files this decoder
    /// cannot read.
    async fn make_thumbnail(
        &self,
        path: &Path,
        meta: &MediaMeta,
    ) -> Result<(Vec<u8>, bool), MediaError> {
        let attachment = meta.attachment_id.as_str();
        let source = if meta.content_type.starts_with("video/") {
            extract_poster(&self.ffmpeg_bin, &[], path, POSTER_TIMEOUT)
                .instrument(tracing::warn_span!("poster", attachment))
                .await
        } else {
            match tokio::fs::read(path).await {
                Ok(bytes) => Some(bytes),
                Err(e) => {
                    tracing::warn!(
                        attachment,
                        error = %e,
                        "could not read the image for its thumbnail; using placeholder thumb"
                    );
                    None
                }
            }
        };

        if let Some(source) = source {
            match tokio::task::spawn_blocking(move || thumbnail_webp(&source)).await {
                Ok(Ok(thumb)) => return Ok((thumb, false)),
                Ok(Err(e)) => tracing::warn!(
                    attachment,
                    error = %e,
                    "thumbnail failed; using placeholder thumb"
                ),
                Err(e) => tracing::warn!(
                    attachment,
                    error = %e,
                    "thumbnail task failed; using placeholder thumb"
                ),
            }
        }
        Ok((placeholder_webp()?, true))
    }
}

/// Extracts a poster frame (PNG bytes) from `video` with the ffmpeg binary
/// `program`. `None` when ffmpeg is unavailable, fails or stalls: the
/// caller stores the placeholder thumbnail and the original video is
/// archived either way (documented degradation).
///
/// The first attempt seeks [`POSTER_SEEK_SECS`] in and scales the frame
/// down. A clip shorter than that yields no frame, so the second attempt is
/// the plain first-frame command. `budget` covers both attempts; a stalled
/// ffmpeg is killed when it runs out. `lead_args` go before leaf's own
/// arguments (empty outside tests).
async fn extract_poster(
    program: &str,
    lead_args: &[&str],
    video: &Path,
    budget: Duration,
) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + budget;
    // Why the last attempt gave no frame, for the one warning if both fail.
    let mut detail = String::new();
    for seek in [true, false] {
        let out = match tempfile::Builder::new().suffix(".png").tempfile() {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "no temp file for the video poster; using placeholder thumb"
                );
                return None;
            }
        };

        let mut cmd = tokio::process::Command::new(program);
        cmd.args(lead_args)
            .args(["-nostdin", "-y", "-loglevel", "error"]);
        if seek {
            cmd.args(["-ss", POSTER_SEEK_SECS]);
        }
        cmd.arg("-i").arg(video).args(["-frames:v", "1"]);
        if seek {
            cmd.args(["-vf", POSTER_SCALE_FILTER]);
        }
        // Without kill_on_drop the timeout would only stop the waiting: a
        // stalled ffmpeg would keep running after the archive had moved on.
        cmd.arg(out.path()).stdin(Stdio::null()).kill_on_drop(true);

        match tokio::time::timeout_at(deadline, cmd.output()).await {
            Ok(Ok(o)) if o.status.success() => {
                if let Ok(bytes) = tokio::fs::read(out.path()).await
                    && !bytes.is_empty()
                {
                    return Some(bytes);
                }
                // Success with nothing written: the seek point is past the
                // end of the clip (or there is no video stream at all).
                "ffmpeg wrote no frame".clone_into(&mut detail);
            }
            Ok(Ok(o)) => {
                detail = String::from_utf8_lossy(&o.stderr)
                    .chars()
                    .take(STDERR_LOG_MAX_CHARS)
                    .collect();
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "ffmpeg unavailable; using placeholder thumb");
                return None;
            }
            Err(_) => {
                tracing::warn!(
                    budget_secs = budget.as_secs(),
                    "ffmpeg poster extraction timed out and was stopped; using placeholder thumb"
                );
                return None;
            }
        }
    }
    tracing::warn!(
        detail = %detail.trim(),
        "ffmpeg poster extraction failed; using placeholder thumb"
    );
    None
}

/// A content type without its parameters (`image/png; charset=binary`).
fn base_type(ct: &str) -> &str {
    ct.split(';').next().unwrap_or(ct).trim()
}

/// Rejects content types outside the allowlist.
fn check_content_type(ct: &str) -> Result<(), MediaError> {
    if ALLOWED_CONTENT_TYPES.contains(&base_type(ct)) {
        Ok(())
    } else {
        Err(MediaError::UnsupportedType(ct.to_owned()))
    }
}

/// Decodes, EXIF-orients, resizes, and encodes a WebP thumbnail.
/// CPU-bound — call from `spawn_blocking`.
fn thumbnail_webp(source: &[u8]) -> Result<Vec<u8>, MediaError> {
    let mut reader = image::ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|e| MediaError::Transform(e.to_string()))?;

    // Defense against decompression bombs: the size cap limits the file,
    // these limit what it may decode into.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);

    let img = reader
        .decode()
        .map_err(|e| MediaError::Transform(e.to_string()))?;

    let img = apply_orientation(img, exif_orientation(source).unwrap_or(1));
    // `thumbnail` also UPSCALES smaller images; never do that — a 64px
    // sticker should stay 64px.
    let thumb = if img.width() <= THUMB_LONG_EDGE && img.height() <= THUMB_LONG_EDGE {
        img
    } else {
        img.thumbnail(THUMB_LONG_EDGE, THUMB_LONG_EDGE)
    };

    encode_webp(&thumb.to_rgba8())
}

/// Encodes pixels as (lossless) WebP.
fn encode_webp(pixels: &image::RgbaImage) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| MediaError::Transform(e.to_string()))?;
    Ok(out)
}

/// Reads the EXIF orientation tag (1–8), if present.
fn exif_orientation(source: &[u8]) -> Option<u32> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(source))
        .ok()?;
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
        .value
        .get_uint(0)
}

/// Applies an EXIF orientation (1–8) to pixels so thumbnails render
/// upright regardless of how the camera stored them.
fn apply_orientation(img: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// The thumbnail stored when a file cannot be previewed: a flat 256×144
/// leaf-green WebP. Cheap enough (one constant tile) to encode in place.
/// The error is unreachable in practice; it is returned rather than
/// swallowed so an empty object is never stored as a thumbnail.
fn placeholder_webp() -> Result<Vec<u8>, MediaError> {
    encode_webp(&image::RgbaImage::from_pixel(
        256,
        144,
        image::Rgba([29, 43, 31, 255]),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use object_store::memory::InMemory;

    use super::*;

    fn meta(ct: &str) -> MediaMeta {
        MediaMeta {
            guild_id: "g1".to_owned(),
            series_id: 7,
            day: 42,
            attachment_id: "att9".to_owned(),
            content_type: ct.to_owned(),
        }
    }

    fn pipeline() -> (Arc<InMemory>, MediaPipeline) {
        let store = Arc::new(InMemory::new());
        let p = MediaPipeline::new(Arc::clone(&store) as Arc<dyn ObjectStore>).unwrap();
        (store, p)
    }

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, _| {
            image::Rgba([u8::try_from(x % 256).unwrap(), 80, 120, 255])
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    async fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media.bin");
        tokio::fs::write(&path, bytes).await.unwrap();
        (dir, path)
    }

    async fn stored_bytes(store: &InMemory, key: &str) -> Vec<u8> {
        store
            .get(&ObjectPath::from(key.to_owned()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .to_vec()
    }

    #[test]
    fn key_layout_is_stable() {
        let m = meta("image/png");
        assert_eq!(original_key(&m), "g/g1/s/7/d/42/att9");
        assert_eq!(thumb_key(&m), "g/g1/s/7/d/42/thumb/att9.webp");
    }

    #[tokio::test]
    async fn archives_image_with_thumbnail() {
        let (store, p) = pipeline();
        let (_d, path) = write_temp(&png_bytes(1024, 512)).await;

        let stored = p.archive_file(&path, &meta("image/png")).await.unwrap();
        assert_eq!(stored.original_key, "g/g1/s/7/d/42/att9");

        // Original stored byte-identical.
        let orig = store
            .get(&ObjectPath::from(stored.original_key.clone()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(orig.len() as u64, stored.size);

        // Thumbnail is a real WebP with the long edge capped at 256 and
        // aspect preserved (1024x512 → 256x128).
        let thumb = store
            .get(&ObjectPath::from(stored.thumb_key.clone()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 128));
    }

    #[tokio::test]
    async fn small_image_is_not_upscaled() {
        let (store, p) = pipeline();
        let (_d, path) = write_temp(&png_bytes(64, 32)).await;
        let stored = p.archive_file(&path, &meta("image/png")).await.unwrap();
        let thumb = store
            .get(&ObjectPath::from(stored.thumb_key))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 32));
    }

    #[tokio::test]
    async fn rejects_oversize_and_bad_types() {
        let (_store, p) = pipeline();
        let p = p.with_max_bytes(1024);
        let (_d, path) = write_temp(&vec![0u8; 4096]).await;
        assert!(matches!(
            p.archive_file(&path, &meta("image/png")).await,
            Err(MediaError::TooLarge { limit_mb: 0 })
        ));

        let (_store, p) = pipeline();
        let (_d, path) = write_temp(&png_bytes(8, 8)).await;
        assert!(matches!(
            p.archive_file(&path, &meta("application/pdf")).await,
            Err(MediaError::UnsupportedType(_))
        ));
        // Parameters after the base type are tolerated.
        assert!(check_content_type("image/png; charset=binary").is_ok());
    }

    #[tokio::test]
    async fn large_files_take_the_multipart_path() {
        let (store, p) = pipeline();
        let p = p.with_multipart_threshold(1024);
        let payload = png_bytes(512, 512); // comfortably > 1KB
        assert!(payload.len() > 1024);
        let (_d, path) = write_temp(&payload).await;

        let stored = p.archive_file(&path, &meta("image/png")).await.unwrap();
        let orig = store
            .get(&ObjectPath::from(stored.original_key))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(orig.as_ref(), payload.as_slice());
    }

    #[tokio::test]
    async fn video_without_ffmpeg_gets_placeholder_thumb() {
        let (store, p) = pipeline();
        let p = p.with_ffmpeg_bin("leaf-test-no-such-ffmpeg");
        let (_d, path) = write_temp(b"not really a video").await;

        let archived = p
            .archive_file_detailed(&path, &meta("video/mp4"))
            .await
            .unwrap();
        assert!(archived.placeholder_thumb);
        let thumb = stored_bytes(&store, &archived.stored.thumb_key).await;
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 144));
    }

    #[tokio::test]
    async fn image_that_cannot_be_previewed_still_archives() {
        // Not an image at all, a truncated PNG, and one wider than the
        // decoder's 16384 px limit: Discord may show all three.
        let mut truncated = png_bytes(64, 64);
        truncated.truncate(truncated.len() / 2);
        let cases = [b"not an image".to_vec(), truncated, png_bytes(20_000, 1)];

        for payload in cases {
            let (store, p) = pipeline();
            let (_d, path) = write_temp(&payload).await;

            let archived = p
                .archive_file_detailed(&path, &meta("image/png"))
                .await
                .unwrap();
            assert!(archived.placeholder_thumb);
            assert_eq!(archived.stored.size, payload.len() as u64);

            // The original is intact and the thumbnail is the placeholder.
            let orig = stored_bytes(&store, &archived.stored.original_key).await;
            assert_eq!(orig, payload);
            let thumb = stored_bytes(&store, &archived.stored.thumb_key).await;
            let decoded = image::load_from_memory(&thumb).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (256, 144));

            // The plain entry point agrees, minus the flag.
            let stored = p.archive_file(&path, &meta("image/png")).await.unwrap();
            assert_eq!(stored, archived.stored);
        }
    }

    #[tokio::test]
    async fn previewable_image_is_not_reported_as_placeholder() {
        let (_store, p) = pipeline();
        let (_d, path) = write_temp(&png_bytes(32, 32)).await;
        let archived = p
            .archive_file_detailed(&path, &meta("image/png"))
            .await
            .unwrap();
        assert!(!archived.placeholder_thumb);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_ffmpeg_is_killed_at_the_timeout() {
        let (dir, path) = write_temp(b"clip").await;
        let pid_file = dir.path().join("ffmpeg.pid");
        let started = std::time::Instant::now();
        // `sh -c '<script>' <pid file> <ffmpeg args>` stands in for an
        // ffmpeg that never finishes. It records its pid, which `exec`
        // hands on to the sleep.
        let poster = extract_poster(
            "sh",
            &[
                "-c",
                r#"echo $$ > "$0"; exec sleep 30"#,
                pid_file.to_str().unwrap(),
            ],
            &path,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(poster, None);
        assert!(started.elapsed() < Duration::from_secs(10));

        // Giving up on the wait is not enough: the process must be gone
        // too, or every stalled poster leaves an ffmpeg running. The kill
        // lands when the child handle drops and the runtime reaps it a
        // moment later, so poll.
        let pid = tokio::fs::read_to_string(&pid_file).await.unwrap();
        let pid = pid.trim();
        assert!(pid.parse::<u32>().is_ok(), "no pid recorded: {pid:?}");
        let mut alive = true;
        for _ in 0..50 {
            alive = tokio::process::Command::new("sh")
                .args(["-c", r#"kill -0 "$0" 2>/dev/null"#, pid])
                .status()
                .await
                .unwrap()
                .success();
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !alive,
            "stalled ffmpeg stand-in (pid {pid}) is still running"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn poster_seeks_first_then_retries_from_the_start() {
        // A stand-in ffmpeg that logs its arguments, writes nothing when
        // asked to seek (a clip shorter than the seek point) and writes the
        // output file (its last argument) otherwise.
        const FAKE: &str = r#"echo "$*" >> "$0"
for a in "$@"; do if [ "$a" = "-ss" ]; then exit 0; fi; done
for out in "$@"; do :; done
printf poster > "$out""#;
        let (dir, path) = write_temp(b"clip").await;
        let log = dir.path().join("calls.log");

        let poster = extract_poster(
            "sh",
            &["-c", FAKE, log.to_str().unwrap()],
            &path,
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(poster.as_deref(), Some(b"poster".as_slice()));

        let calls = tokio::fs::read_to_string(&log).await.unwrap();
        let mut calls = calls.lines();
        let seeking = calls.next().unwrap();
        let plain = calls.next().unwrap();
        assert_eq!(calls.next(), None);
        assert!(seeking.contains("-ss 1 -i "), "{seeking}");
        assert!(seeking.contains("-vf scale="), "{seeking}");
        assert!(!plain.contains("-ss"), "{plain}");
        assert!(!plain.contains("-vf"), "{plain}");
        for call in [seeking, plain] {
            assert!(call.starts_with("-nostdin -y -loglevel error "), "{call}");
            assert!(call.contains(" -frames:v 1 "), "{call}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn poster_is_none_when_ffmpeg_fails_or_writes_nothing() {
        let (_d, path) = write_temp(b"clip").await;
        for script in ["echo boom >&2; exit 1", "exit 0"] {
            let poster =
                extract_poster("sh", &["-c", script, "sh"], &path, Duration::from_secs(30)).await;
            assert_eq!(poster, None, "{script}");
        }
    }

    #[tokio::test]
    #[ignore = "requires ffmpeg on PATH; run with --ignored locally"]
    async fn real_ffmpeg_extracts_a_poster_frame() {
        // Generate a 1-second test clip with ffmpeg itself, then archive it.
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("clip.mp4");
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=green:s=320x240:d=1",
            ])
            .arg(&clip)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        let (store, p) = pipeline();
        let stored = p.archive_file(&clip, &meta("video/mp4")).await.unwrap();
        let thumb = store
            .get(&ObjectPath::from(stored.thumb_key))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        // Poster preserves the clip aspect (320x240 → 256x192).
        assert_eq!((decoded.width(), decoded.height()), (256, 192));
    }

    #[test]
    fn orientation_transforms_move_pixels_correctly() {
        // 2x1 image: red pixel left, blue pixel right.
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        let img = DynamicImage::ImageRgba8(img);

        let red_at = |img: &DynamicImage, x: u32, y: u32| {
            image::GenericImageView::get_pixel(img, x, y).0 == [255, 0, 0, 255]
        };

        // 1 = unchanged; red stays top-left.
        assert!(red_at(&apply_orientation(img.clone(), 1), 0, 0));
        // 2 = flip horizontal; red moves right.
        assert!(red_at(&apply_orientation(img.clone(), 2), 1, 0));
        // 3 = rotate 180; red moves right.
        assert!(red_at(&apply_orientation(img.clone(), 3), 1, 0));
        // 6 = rotate 90 CW: 2x1 → 1x2, red goes top.
        let r6 = apply_orientation(img.clone(), 6);
        assert_eq!((r6.width(), r6.height()), (1, 2));
        assert!(red_at(&r6, 0, 0));
        // 8 = rotate 270 CW: red goes bottom.
        let r8 = apply_orientation(img.clone(), 8);
        assert!(red_at(&r8, 0, 1));
        // Unknown orientation = unchanged.
        assert!(red_at(&apply_orientation(img, 99), 0, 0));
    }

    #[tokio::test]
    async fn download_respects_cap_mid_stream() {
        // A server announcing a large body and streaming well past the cap.
        // The streaming check must trip on accumulated bytes — not wait for
        // the whole file — yielding TooLarge (deterministically, since the
        // first ~100KB arrive before the 2KB cap is reached).
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            // Honest, large Content-Length so reqwest keeps reading until
            // our cap fires (a small CL would be honored and stop early).
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4000000\r\n\r\n")
                .await;
            for _ in 0..100 {
                if sock.write_all(&[0u8; 1024]).await.is_err() {
                    break; // client hung up after the cap tripped
                }
            }
        });

        let (_store, p) = pipeline();
        let p = p.with_max_bytes(2048);
        let result = p
            .archive_from_url(&format!("http://{addr}/file"), &meta("image/png"))
            .await;
        assert!(
            matches!(
                result,
                Err(MediaError::TooLarge { .. } | MediaError::Fetch(_))
            ),
            "expected TooLarge/Fetch, got {result:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires ffmpeg on PATH; run with --ignored locally"]
    async fn real_ffmpeg_poster_skips_a_black_opening_frame() {
        // Three seconds of 1080p white that fades in from black over the
        // first second: frame 0 is black, the frame at 1 s is white.
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("fade.mp4");
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=white:s=1920x1080:d=3,fade=t=in:st=0:d=1",
            ])
            .arg(&clip)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        let (store, p) = pipeline();
        let archived = p
            .archive_file_detailed(&clip, &meta("video/mp4"))
            .await
            .unwrap();
        assert!(!archived.placeholder_thumb);
        let thumb = stored_bytes(&store, &archived.stored.thumb_key).await;
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 144));
        let centre = image::GenericImageView::get_pixel(&decoded, 128, 72).0;
        assert!(centre[0] > 200, "poster is still dark: {centre:?}");
    }

    /// Serves one request with an empty response of `status_line` (such as
    /// `404 Not Found`) and returns the address to send it to.
    async fn serve_status(status_line: &'static str) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 1024];
            let _ = sock.read(&mut request).await;
            let response = format!("HTTP/1.1 {status_line}\r\nContent-Length: 0\r\n\r\n");
            let _ = sock.write_all(response.as_bytes()).await;
        });
        addr
    }

    #[tokio::test]
    async fn download_failure_does_not_keep_the_signed_url() {
        for status_line in ["404 Not Found", "503 Service Unavailable"] {
            let addr = serve_status(status_line).await;
            let (_store, p) = pipeline();
            let url = format!("http://{addr}/attachments/1/2/clip.mov?ex=66aa&hm=sekrit");
            let err = p
                .archive_from_url(&url, &meta("video/quicktime"))
                .await
                .unwrap_err();

            // Display is what reaches the operator's log.
            let detail = err.to_string();
            let code = status_line.split(' ').next().unwrap();
            assert!(detail.contains(code), "{detail}");
            assert!(!detail.contains("sekrit"), "{detail}");
            assert!(!detail.contains("attachments"), "{detail}");
        }
    }

    #[tokio::test]
    async fn a_file_discord_no_longer_has_is_not_retryable() {
        // The CDN's answer for a deleted attachment or an expired link.
        let addr = serve_status("404 Not Found").await;
        let (_store, p) = pipeline();
        let err = p
            .archive_from_url(&format!("http://{addr}/clip.mov"), &meta("video/quicktime"))
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::Gone(_)), "{err:?}");
        assert!(!err.is_retryable());

        // A CDN hiccup is worth another try.
        let addr = serve_status("503 Service Unavailable").await;
        let err = p
            .archive_from_url(&format!("http://{addr}/clip.mov"), &meta("video/quicktime"))
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::Fetch(_)), "{err:?}");
        assert!(err.is_retryable());

        // So is a refused connection, which has no status at all.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let err = p
            .archive_from_url(&format!("http://{addr}/clip.mov"), &meta("video/quicktime"))
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::Fetch(_)), "{err:?}");
    }

    #[test]
    fn only_lasting_client_errors_are_permanent() {
        let cases = [
            (400, true),
            (401, true),
            (403, true),
            (404, true),
            (410, true),
            // "Come back later", not "never".
            (408, false),
            (429, false),
            (500, false),
            (502, false),
            (503, false),
            (200, false),
            (304, false),
        ];
        for (code, want) in cases {
            let status = reqwest::StatusCode::from_u16(code).unwrap();
            assert_eq!(is_permanent_status(status), want, "{code}");
        }
    }

    #[test]
    fn user_messages_say_what_to_do_and_hide_internals() {
        let cases = [
            (
                MediaError::TooLarge { limit_mb: 100 },
                "`clip.mov` is over leaf's 100 MB limit. Trim or compress it, then post it again.",
            ),
            (
                MediaError::TooLarge { limit_mb: 0 },
                "`clip.mov` is over leaf's size limit. Trim or compress it, then post it again.",
            ),
            (
                MediaError::UnsupportedType("image/heic".to_owned()),
                "`clip.mov` isn't a supported format (PNG, JPEG, WebP, GIF, MP4, WebM, MOV).",
            ),
            (
                MediaError::Fetch(
                    "error sending request for url (https://cdn.example/a?hm=sekrit)".to_owned(),
                ),
                "`clip.mov` couldn't be downloaded from Discord. Try again in a moment.",
            ),
            (
                MediaError::Gone("HTTP status client error (404 Not Found) for sekrit".to_owned()),
                "`clip.mov` is no longer available on Discord. \
                 Check the message still has the file, then archive it again.",
            ),
            (
                MediaError::Transform("sekrit decoder detail".to_owned()),
                "`clip.mov` couldn't be processed, so it wasn't archived. \
                 Tell a server admin if this keeps happening.",
            ),
            (
                MediaError::Store(object_store::Error::Generic {
                    store: "S3",
                    source: "PUT https://sekrit.r2.cloudflarestorage.com/bucket/key".into(),
                }),
                "`clip.mov` wasn't archived because leaf's storage is unavailable right now. \
                 Try again in a few minutes, and tell a server admin if this keeps happening.",
            ),
            (
                MediaError::Io(std::io::Error::other(
                    "/sekrit/tmp: No space left on device",
                )),
                "`clip.mov` wasn't archived because leaf's storage is unavailable right now. \
                 Try again in a few minutes, and tell a server admin if this keeps happening.",
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.user_message("clip.mov"), want, "{err:?}");
        }
        assert_eq!(
            unsupported_message("IMG_2041.heic"),
            "`IMG_2041.heic` isn't a supported format (PNG, JPEG, WebP, GIF, MP4, WebM, MOV)."
        );
    }

    #[test]
    fn only_download_and_storage_failures_are_retryable() {
        let generic = || object_store::Error::Generic {
            store: "S3",
            source: "down".into(),
        };
        let cases = [
            (MediaError::Fetch(String::new()), true),
            (MediaError::Store(generic()), true),
            (MediaError::Io(std::io::Error::other("disk")), true),
            (MediaError::Gone(String::new()), false),
            (MediaError::TooLarge { limit_mb: 100 }, false),
            (MediaError::UnsupportedType(String::new()), false),
            (MediaError::Transform(String::new()), false),
        ];
        for (err, want) in cases {
            assert_eq!(err.is_retryable(), want, "{err:?}");
        }
    }

    #[test]
    fn filenames_are_quoted_safely() {
        let long = "a".repeat(FILENAME_DISPLAY_MAX_CHARS + 1);
        let cut = format!("`{}…`", "a".repeat(FILENAME_DISPLAY_MAX_CHARS));
        // A long name keeps its extension: the cut comes out of the middle
        // and the quoted name is no longer than any other cut name.
        let long_heic = format!("{}.heic", "b".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let cut_heic = format!("`{}….heic`", "b".repeat(FILENAME_DISPLAY_MAX_CHARS - 5));
        let long_wide = format!("{}.png", "日".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let cut_wide = format!("`{}….png`", "日".repeat(FILENAME_DISPLAY_MAX_CHARS - 4));
        // Not an extension worth keeping: too long, or not a plain word.
        let long_tail = format!("c.{}", "d".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let cut_tail = format!("`c.{}…`", "d".repeat(FILENAME_DISPLAY_MAX_CHARS - 2));
        let spaced = format!("{}. final", "e".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let cut_spaced = format!("`{}…`", "e".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let dotted = format!("{}.", "f".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let cut_dotted = format!("`{}…`", "f".repeat(FILENAME_DISPLAY_MAX_CHARS));
        let cases = [
            ("photo.png", "`photo.png`"),
            ("  photo.png  ", "`photo.png`"),
            // Nothing that could close the code span or add lines.
            ("we`ird`.png", "`weird.png`"),
            ("two\nlines\t.png", "`twolines.png`"),
            ("日本語.png", "`日本語.png`"),
            (long.as_str(), cut.as_str()),
            (long_heic.as_str(), cut_heic.as_str()),
            (long_wide.as_str(), cut_wide.as_str()),
            (long_tail.as_str(), cut_tail.as_str()),
            (spaced.as_str(), cut_spaced.as_str()),
            (dotted.as_str(), cut_dotted.as_str()),
            ("", "That file"),
            (" ` ", "That file"),
            ("` `", "That file"),
        ];
        for (input, want) in cases {
            assert_eq!(file_ref(input), want, "{input:?}");
        }
        assert_eq!(cut_heic.chars().count(), cut.chars().count());
        assert_eq!(
            unsupported_message(&long_heic),
            format!("{cut_heic} isn't a supported format ({SUPPORTED_FORMATS}).")
        );
        assert_eq!(
            MediaError::TooLarge { limit_mb: 100 }.user_message(""),
            "That file is over leaf's 100 MB limit. Trim or compress it, then post it again."
        );
    }

    #[test]
    fn content_type_falls_back_to_the_extension() {
        let cases = [
            // Declared and allowed: used as is, minus parameters and case.
            ("a.png", Some("image/png"), Some("image/png")),
            ("a.bin", Some("video/mp4"), Some("video/mp4")),
            (
                "a.png",
                Some("image/png; charset=binary"),
                Some("image/png"),
            ),
            ("a.jpg", Some("IMAGE/JPEG"), Some("image/jpeg")),
            ("a.jpg", Some("image/jpg"), Some("image/jpeg")),
            ("a.mov", Some("video/quicktime"), Some("video/quicktime")),
            // Missing, blank or generic: the extension decides.
            ("a.png", None, Some("image/png")),
            ("IMG_0001.JPG", None, Some("image/jpeg")),
            ("a.jpeg", Some(""), Some("image/jpeg")),
            ("a.webp", Some("  "), Some("image/webp")),
            ("a.gif", Some("application/octet-stream"), Some("image/gif")),
            ("a.mp4", Some("binary/octet-stream"), Some("video/mp4")),
            ("a.webm", None, Some("video/webm")),
            ("clip.final.MOV", None, Some("video/quicktime")),
            ("a.heic", None, None),
            ("a.tar.gz", None, None),
            ("png", None, None),
            ("", None, None),
            // A real but unsupported type is not overridden by the name.
            ("a.jpg", Some("image/heic"), None),
            ("a.png", Some("application/pdf"), None),
            ("a.mp4", Some("video/x-matroska"), None),
        ];
        for (filename, declared, want) in cases {
            assert_eq!(
                content_type_for(filename, declared),
                want,
                "{filename:?} {declared:?}"
            );
        }
    }

    #[test]
    fn every_content_type_for_result_is_archivable() {
        for (extension, content_type) in EXTENSION_CONTENT_TYPES {
            assert!(check_content_type(content_type).is_ok(), "{extension}");
        }
        for allowed in ALLOWED_CONTENT_TYPES {
            let resolved = content_type_for("file", Some(allowed)).unwrap();
            assert!(check_content_type(resolved).is_ok(), "{allowed}");
        }
    }
}
