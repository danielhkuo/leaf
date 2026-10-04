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
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use image::DynamicImage;
use object_store::local::LocalFileSystem;
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
    /// The folder a `file://` storage endpoint names cannot be used.
    #[error("storage folder: {0}")]
    Folder(#[from] LocalStoreError),
}

/// Why a folder on this machine cannot be the media store.
#[derive(Debug, thiserror::Error)]
pub enum LocalStoreError {
    /// The endpoint starts with `file:` but is not `file://` followed by an
    /// absolute path that names a folder.
    #[error(
        "the storage endpoint is not file:// followed by a folder's full path \
         (such as file:///data/media)"
    )]
    NotAbsolute,
    /// The folder is not there and could not be created.
    #[error("creating {}: {source}", dir.display())]
    Create {
        /// The folder asked for.
        dir: PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },
    /// The folder is there and could not be opened as a store.
    #[error("opening {}: {source}", dir.display())]
    Open {
        /// The folder asked for.
        dir: PathBuf,
        /// What the store said.
        source: object_store::Error,
    },
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
            Self::Store(_) | Self::Io(_) | Self::Folder(_) => format!(
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
        matches!(
            self,
            Self::Fetch(_) | Self::Store(_) | Self::Io(_) | Self::Folder(_)
        )
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

/// How a storage endpoint that names a folder on this machine starts. What
/// follows is the folder's absolute path, taken as written.
const LOCAL_ENDPOINT_PREFIX: &str = "file://";

/// The scheme of a folder endpoint. Like any URL scheme it is read without
/// regard to case: `FILE:///data/media` is a folder too.
const LOCAL_SCHEME: &str = "file:";

/// `rest` when `text` starts with `prefix`, whatever the case of its ASCII
/// letters.
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let (head, rest) = text.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

/// Whether `endpoint` is meant as a folder on this machine (any `file:`
/// address, in any case) and not as an S3 endpoint. Whether it names a
/// folder leaf can use is [`local_store_dir`]'s question.
#[must_use]
pub fn is_local_endpoint(endpoint: &str) -> bool {
    strip_prefix_ignore_ascii_case(endpoint.trim_start(), LOCAL_SCHEME).is_some()
}

/// The folder a `file://` storage endpoint names: `file:///data/media` is
/// `/data/media`. Media is then kept on this machine's disk instead of in a
/// bucket, and the bucket and key settings are not used.
///
/// `None` unless the endpoint is `file://` followed by an absolute path
/// that names a folder. A relative path (`file://media`), a host
/// (`file://nas/media`), no path (`file://`) and the root alone
/// (`file:///`) are all refused: where media lands must never depend on the
/// directory leaf was started in. So is any path with `..` in it
/// (`file:///data/..` is the root again, in other words). The path is taken
/// as written; nothing in it is percent-decoded.
#[must_use]
pub fn local_store_dir(endpoint: &str) -> Option<PathBuf> {
    let path = strip_prefix_ignore_ascii_case(endpoint.trim(), LOCAL_ENDPOINT_PREFIX)?;
    let mut parts = Path::new(path).components();
    let names_a_folder = parts
        .clone()
        .any(|part| matches!(part, Component::Normal(_)));
    let climbs = parts.any(|part| matches!(part, Component::ParentDir));
    (path.starts_with('/') && names_a_folder && !climbs).then(|| PathBuf::from(path))
}

/// The storage endpoint for the folder at `path`: what [`local_store_dir`]
/// reads back. `None` unless `path` is absolute and names a folder.
#[must_use]
pub fn local_endpoint(path: &str) -> Option<String> {
    let endpoint = format!("{LOCAL_ENDPOINT_PREFIX}{}", path.trim());
    local_store_dir(&endpoint).map(|_| endpoint)
}

/// Opens the folder `dir` as the media store, creating it (and the folders
/// above it) when it is not there yet.
///
/// A delete also removes the folders it leaves empty, so the day folders of
/// an undone post do not pile up. `dir` itself is never removed.
pub fn local_store(dir: &Path) -> Result<LocalFileSystem, LocalStoreError> {
    std::fs::create_dir_all(dir).map_err(|source| LocalStoreError::Create {
        dir: dir.to_owned(),
        source,
    })?;
    let store = LocalFileSystem::new_with_prefix(dir).map_err(|source| LocalStoreError::Open {
        dir: dir.to_owned(),
        source,
    })?;
    Ok(store.with_automatic_cleanup(true))
}

/// Where the configured storage keeps media.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageTarget {
    /// In a folder on this machine.
    Folder(PathBuf),
    /// In a bucket behind an S3 API (Cloudflare R2).
    Bucket {
        /// Host of the S3 endpoint; empty when the endpoint is not a URL.
        host: String,
        /// The bucket's name.
        bucket: String,
    },
}

impl StorageTarget {
    /// Reads the target out of the storage settings. A `file:` endpoint
    /// that names no usable folder is an error, never an S3 endpoint.
    pub fn of(cfg: &crate::config::R2Config) -> Result<Self, LocalStoreError> {
        if is_local_endpoint(&cfg.endpoint) {
            return local_store_dir(&cfg.endpoint)
                .map(Self::Folder)
                .ok_or(LocalStoreError::NotAbsolute);
        }
        // The host alone: enough to tell stores apart in a log, and free of
        // anything a URL can carry besides.
        let host = reqwest::Url::parse(cfg.endpoint.trim())
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(Self::Bucket {
            host,
            bucket: cfg.bucket.clone(),
        })
    }
}

/// Builds the object store from Tier-1 configuration.
///
/// That is R2 (or any S3 endpoint), or a folder on this machine for a
/// `file://` endpoint (see [`local_store_dir`]). One line is logged saying
/// which of the two is in use.
pub fn r2_store(cfg: &crate::config::R2Config) -> Result<Arc<dyn ObjectStore>, MediaError> {
    match StorageTarget::of(cfg)? {
        StorageTarget::Folder(dir) => {
            let store = local_store(&dir)?;
            tracing::info!(
                folder = %dir.display(),
                "storing media in a folder on this machine (the files are kept nowhere else)"
            );
            Ok(Arc::new(store))
        }
        StorageTarget::Bucket { host, bucket } => {
            let store = object_store::aws::AmazonS3Builder::new()
                .with_endpoint(&cfg.endpoint)
                .with_bucket_name(&cfg.bucket)
                .with_access_key_id(&cfg.access_key_id)
                .with_secret_access_key(&cfg.secret_access_key)
                .with_region("auto")
                .build()?;
            tracing::info!(endpoint = %host, %bucket, "storing media in an S3 bucket");
            Ok(Arc::new(store))
        }
    }
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
            if let Err(e) = self.delete_key(key).await {
                tracing::warn!(key, error = %e, "failed to delete stored media object");
            }
        }
    }

    /// Removes one stored object. One that is not there counts as removed:
    /// an S3 store answers such a delete with success, and a folder must
    /// not differ, or releasing the keys of an archive that failed before
    /// anything was stored would log a failure for nothing.
    async fn delete_key(&self, key: &str) -> object_store::Result<()> {
        match self.store.delete(&ObjectPath::from(key.to_owned())).await {
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            outcome => outcome,
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
            (
                MediaError::Folder(LocalStoreError::Create {
                    dir: PathBuf::from("/sekrit/media"),
                    source: std::io::Error::other("Permission denied"),
                }),
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
            (MediaError::Folder(LocalStoreError::NotAbsolute), true),
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

    // ---- a folder on this machine as the store ----

    use crate::config::R2Config;

    /// Storage settings for a folder. The bucket and keys hold placeholder
    /// text, as in a config written before the setup page offered a folder.
    fn folder_config(endpoint: &str) -> R2Config {
        R2Config {
            endpoint: endpoint.to_owned(),
            bucket: "local".to_owned(),
            access_key_id: "local".to_owned(),
            secret_access_key: "local".to_owned(),
        }
    }

    /// A folder that does not exist yet, two levels down in a fresh temp
    /// dir, and the endpoint that names it. The space in its path must
    /// survive the trip.
    fn new_folder() -> (tempfile::TempDir, PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("leaf data").join("media");
        let endpoint = local_endpoint(dir.to_str().unwrap()).unwrap();
        (tmp, dir, endpoint)
    }

    /// The names in `dir`, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_endpoint_names_a_folder_only_by_its_absolute_path() {
        let folders = [
            ("file:///data/media", "/data/media"),
            ("  file:///data/media\n", "/data/media"),
            ("file:///data/media/", "/data/media/"),
            // Taken as written: a space is a space, and %20 is not one.
            ("file:///Users/me/leaf media", "/Users/me/leaf media"),
            ("file:///srv/leaf%20media", "/srv/leaf%20media"),
            // A scheme is read in any case; the path keeps its own.
            ("FILE:///data/Media", "/data/Media"),
            ("File:///data/media", "/data/media"),
            // Dots that are part of a name climb nowhere.
            ("file:///data/..media/a..b", "/data/..media/a..b"),
        ];
        for (endpoint, dir) in folders {
            assert_eq!(
                local_store_dir(endpoint),
                Some(PathBuf::from(dir)),
                "{endpoint}"
            );
            assert!(is_local_endpoint(endpoint), "{endpoint}");
        }

        let refused = [
            // Relative: where it lands would depend on the working directory.
            "file://media",
            "file://./media",
            "file://../media",
            "file:media",
            // A host is not this machine.
            "file://nas/media",
            "file://localhost/data/media",
            // No path, or the root alone.
            "file://",
            "file:///",
            "file:////",
            "file:///.",
            "file:///..",
            // `..` anywhere: the path could be the root, or anything else.
            "file:///data/..",
            "file:///data/../..",
            "file:///data/../media",
            "file:///data/media/..",
            "FILE:///data/..",
            // One slash short.
            "file:/data/media",
            // Any case of the scheme is still a folder, never an S3 endpoint.
            "FILE://media",
            "File:media",
            "FILE:",
        ];
        for endpoint in refused {
            assert_eq!(local_store_dir(endpoint), None, "{endpoint}");
            // Still meant as a folder: never to be tried as an S3 endpoint.
            assert!(is_local_endpoint(endpoint), "{endpoint}");
        }

        let not_folders = [
            "https://acc.r2.cloudflarestorage.com",
            "/data/media",
            "profile://data/media",
            "files:///data/media",
            "fil",
            // Not ASCII where the scheme would be: no folder, and no panic.
            "fil\u{e9}:///data/media",
            "",
        ];
        for endpoint in not_folders {
            assert_eq!(local_store_dir(endpoint), None, "{endpoint}");
            assert!(!is_local_endpoint(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn a_folder_path_becomes_the_endpoint_that_reads_back_as_it() {
        assert_eq!(
            local_endpoint("/data/media").as_deref(),
            Some("file:///data/media")
        );
        assert_eq!(
            local_endpoint("  /data/media ").as_deref(),
            Some("file:///data/media")
        );
        for path in ["/data/media", "/Users/me/leaf media", "/data/media/"] {
            let endpoint = local_endpoint(path).unwrap();
            assert_eq!(
                local_store_dir(&endpoint),
                Some(PathBuf::from(path)),
                "{path}"
            );
        }
        let refused = [
            "",
            "  ",
            "media",
            "./media",
            "~/media",
            "nas/media",
            "C:\\media",
            "/",
            "//",
        ];
        for path in refused {
            assert_eq!(local_endpoint(path), None, "{path:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_folder_store_is_created_and_round_trips_an_object() {
        let (_tmp, dir, endpoint) = new_folder();
        assert!(!dir.exists());
        let store = r2_store(&folder_config(&endpoint)).unwrap();
        assert!(dir.is_dir(), "the folder is created");

        let path = ObjectPath::from("g/1/s/7/d/42/att9");
        store
            .put(&path, b"leaf on disk".to_vec().into())
            .await
            .unwrap();
        // A plain file, where the key says.
        let on_disk = tokio::fs::read(dir.join("g/1/s/7/d/42/att9"))
            .await
            .unwrap();
        assert_eq!(on_disk, b"leaf on disk");

        let whole = store.get(&path).await.unwrap().bytes().await.unwrap();
        assert_eq!(whole.as_ref(), b"leaf on disk");
        // Byte ranges, as a video player asks for them: a span, and the tail.
        let part = store.get_range(&path, 5..7).await.unwrap();
        assert_eq!(part.as_ref(), b"on");
        let options = object_store::GetOptions {
            range: Some(object_store::GetRange::Suffix(4)),
            ..object_store::GetOptions::default()
        };
        let tail = store.get_opts(&path, options).await.unwrap();
        assert_eq!(tail.range, 8..12);
        assert_eq!(tail.meta.size, 12);
        assert_eq!(tail.bytes().await.unwrap().as_ref(), b"disk");

        store.delete(&path).await.unwrap();
        assert!(matches!(
            store.get(&path).await,
            Err(object_store::Error::NotFound { .. })
        ));
        // The folders the delete emptied are gone; the store's own stays.
        assert!(names_in(&dir).is_empty());
        assert!(dir.is_dir());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_pipeline_archives_into_a_folder() {
        let (_tmp, dir, endpoint) = new_folder();
        let store = r2_store(&folder_config(&endpoint)).unwrap();
        let p = MediaPipeline::new(store).unwrap();
        let payload = png_bytes(1024, 512);
        let (_d, path) = write_temp(&payload).await;

        let stored = p.archive_file(&path, &meta("image/png")).await.unwrap();
        let day = dir.join("g/g1/s/7/d/42");
        assert_eq!(tokio::fs::read(day.join("att9")).await.unwrap(), payload);
        let thumb = tokio::fs::read(day.join("thumb/att9.webp")).await.unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 128));
        assert_eq!(p.get_bytes(&stored.thumb_key).await.unwrap(), thumb);

        // Undoing the day leaves nothing of it on disk.
        p.delete_keys(&[stored.original_key, stored.thumb_key])
            .await;
        assert!(names_in(&dir).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_large_file_reaches_a_folder_through_the_multipart_path() {
        let (_tmp, dir, endpoint) = new_folder();
        let store = r2_store(&folder_config(&endpoint)).unwrap();
        // No poster frame is tried: the bytes are no video.
        let p = MediaPipeline::new(store)
            .unwrap()
            .with_ffmpeg_bin("leaf-test-no-such-ffmpeg");
        // Over the threshold, and long enough to go up in three parts whose
        // order matters: no two parts hold the same bytes.
        let len = usize::try_from(DEFAULT_MULTIPART_THRESHOLD).unwrap() + 3 * 1024 * 1024;
        let payload: Vec<u8> = (0..len)
            .map(|i| u8::try_from((i ^ (i >> 16)) % 251).unwrap())
            .collect();
        let (_d, path) = write_temp(&payload).await;

        let archived = p
            .archive_file_detailed(&path, &meta("video/mp4"))
            .await
            .unwrap();
        assert_eq!(archived.stored.size, payload.len() as u64);
        let day = dir.join("g/g1/s/7/d/42");
        let on_disk = tokio::fs::read(day.join("att9")).await.unwrap();
        assert!(
            on_disk == payload,
            "the stored file differs from the original"
        );
        // The parts are staged in a file beside the target: none is left.
        assert_eq!(names_in(&day), ["att9", "thumb"]);
        // A part of it reads back from where it should.
        let store = r2_store(&folder_config(&endpoint)).unwrap();
        let start = payload.len() - 1024;
        let tail = store
            .get_range(
                &ObjectPath::from(archived.stored.original_key),
                start as u64..payload.len() as u64,
            )
            .await
            .unwrap();
        assert_eq!(tail.as_ref(), payload.get(start..).unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn deleting_a_key_that_is_not_there_is_not_a_failure() {
        let (_tmp, _dir, endpoint) = new_folder();
        let store = r2_store(&folder_config(&endpoint)).unwrap();
        // A folder answers this delete with "not found", where S3 and the
        // in-memory store answer success.
        let missing = ObjectPath::from("g/1/s/7/d/42/never-stored");
        assert!(matches!(
            store.delete(&missing).await,
            Err(object_store::Error::NotFound { .. })
        ));
        let p = MediaPipeline::new(store).unwrap();
        p.delete_key("g/1/s/7/d/42/never-stored").await.unwrap();
    }

    #[test]
    fn a_file_endpoint_that_names_no_folder_builds_no_store() {
        for endpoint in ["file://media", "file://nas/media", "file://", "file:///"] {
            let err = r2_store(&folder_config(endpoint)).unwrap_err();
            assert!(
                matches!(err, MediaError::Folder(LocalStoreError::NotAbsolute)),
                "{endpoint}: {err:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_created_is_named_with_the_reason() {
        let tmp = tempfile::tempdir().unwrap();
        // A file where a folder on the way should be.
        std::fs::write(tmp.path().join("taken"), b"").unwrap();
        let dir = tmp.path().join("taken").join("media");
        let endpoint = local_endpoint(dir.to_str().unwrap()).unwrap();

        let err = r2_store(&folder_config(&endpoint)).unwrap_err();
        assert!(
            matches!(&err, MediaError::Folder(LocalStoreError::Create { dir: named, .. }) if *named == dir),
            "{err:?}"
        );
        let said = err.to_string();
        let start = format!("storage folder: creating {}: ", dir.display());
        assert!(said.starts_with(&start), "{said}");
    }

    #[test]
    fn the_storage_target_is_the_folder_or_the_bucket_at_its_host() {
        assert_eq!(
            StorageTarget::of(&folder_config("file:///data/media")).unwrap(),
            StorageTarget::Folder(PathBuf::from("/data/media"))
        );
        assert!(matches!(
            StorageTarget::of(&folder_config("file://media")),
            Err(LocalStoreError::NotAbsolute)
        ));

        // (endpoint, host): the host alone, whatever else the URL carries.
        let cases = [
            (
                "https://acc.r2.cloudflarestorage.com",
                "acc.r2.cloudflarestorage.com",
            ),
            (" https://ACC.example:8443/path/ ", "acc.example"),
            ("https://user:sekrit@acc.example", "acc.example"),
            ("not a url", ""),
        ];
        for (endpoint, host) in cases {
            let cfg = R2Config {
                endpoint: endpoint.to_owned(),
                bucket: "leaf-media".to_owned(),
                ..folder_config("")
            };
            assert_eq!(
                StorageTarget::of(&cfg).unwrap(),
                StorageTarget::Bucket {
                    host: host.to_owned(),
                    bucket: "leaf-media".to_owned(),
                },
                "{endpoint}"
            );
        }
    }

    /// One logged event as a string: the message, then each field as
    /// ` name=value`.
    #[derive(Default)]
    struct Line {
        message: String,
        fields: String,
    }

    impl tracing::field::Visit for Line {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            if field.name() == "message" {
                self.message = format!("{value:?}");
            } else {
                write!(self.fields, " {}={value:?}", field.name()).unwrap();
            }
        }
    }

    /// What this crate logged, with the thread that logged it.
    static LOGGED: std::sync::Mutex<Vec<(std::thread::ThreadId, String)>> =
        std::sync::Mutex::new(Vec::new());

    /// [`LOGGED`], whatever became of a test that held it.
    fn all_logged() -> std::sync::MutexGuard<'static, Vec<(std::thread::ThreadId, String)>> {
        LOGGED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Takes the lines this thread logged out of [`LOGGED`].
    fn take_logged() -> Vec<String> {
        let me = std::thread::current().id();
        let mut all = all_logged();
        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *all)
            .into_iter()
            .partition(|(thread, _)| *thread == me);
        *all = others;
        drop(all);
        mine.into_iter().map(|(_, line)| line).collect()
    }

    /// Records this crate's events into [`LOGGED`]. The S3 client logs
    /// lines of its own while it is built; those are not leaf's.
    struct Recorder;

    impl tracing::Subscriber for Recorder {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.target().starts_with("leaf_core")
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut line = Line::default();
            event.record(&mut line);
            let line = format!("{}{}", line.message, line.fields);
            all_logged().push((std::thread::current().id(), line));
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// Runs `work` and returns what leaf logged on this thread meanwhile.
    ///
    /// The recorder is installed once, for the whole test process. A
    /// subscriber scoped to this thread would be the shorter way, and would
    /// miss lines now and then: tracing remembers per call site whether
    /// anyone listens, and when another test's thread reaches the call site
    /// first it asks that thread's subscriber, which is none.
    fn logged<T>(work: impl FnOnce() -> T) -> (T, Vec<String>) {
        static INSTALLED: std::sync::Once = std::sync::Once::new();
        INSTALLED.call_once(|| tracing::subscriber::set_global_default(Recorder).unwrap());

        let _earlier = take_logged();
        let out = work();
        (out, take_logged())
    }

    #[cfg(unix)]
    #[test]
    fn building_the_store_logs_one_line_saying_which_storage_is_in_use() {
        let (_tmp, dir, endpoint) = new_folder();
        let (store, lines) = logged(|| r2_store(&folder_config(&endpoint)));
        store.unwrap();
        assert_eq!(
            lines,
            [format!(
                "storing media in a folder on this machine (the files are kept nowhere else) \
                 folder={}",
                dir.display()
            )]
        );

        let cfg = R2Config {
            endpoint: "https://acc.r2.cloudflarestorage.com".to_owned(),
            bucket: "leaf-media".to_owned(),
            access_key_id: "SECRET_KEYID".to_owned(),
            secret_access_key: "SECRET_KEY".to_owned(),
        };
        let (store, lines) = logged(|| r2_store(&cfg));
        store.unwrap();
        // The endpoint's host and the bucket; no key.
        assert_eq!(
            lines,
            [
                "storing media in an S3 bucket endpoint=acc.r2.cloudflarestorage.com bucket=leaf-media"
            ]
        );

        // A store that was not built is not announced.
        let (store, lines) = logged(|| r2_store(&folder_config("file://media")));
        assert!(store.is_err());
        assert!(lines.is_empty(), "{lines:?}");
    }
}
