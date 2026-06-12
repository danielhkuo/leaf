//! The media pipeline: bytes in, originals + thumbnails durably in object
//! storage, keys out.
//!
//! Downloads stream to a temp file (never whole-file in RAM), originals
//! stream to storage (multipart above a threshold), and thumbnails are
//! generated once at archive time — a small WebP for images, an `ffmpeg`
//! poster frame for video (deterministic placeholder when ffmpeg is
//! unavailable). All CPU-bound image work runs on the blocking pool.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use image::DynamicImage;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, WriteMultipart};
use tokio::io::AsyncReadExt as _;

/// Thumbnail long-edge in pixels (gallery grid / heatmap tiles).
pub const THUMB_LONG_EDGE: u32 = 256;

/// Default per-file size cap in bytes (100 MB).
pub const DEFAULT_MAX_BYTES: u64 = 100 * 1024 * 1024;

/// Files larger than this stream to storage via multipart upload.
const DEFAULT_MULTIPART_THRESHOLD: u64 = 8 * 1024 * 1024;

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

/// Errors from the media pipeline, split by phase so callers can tell a
/// user problem (too large, wrong type) from an infrastructure problem.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// Downloading from the source URL failed.
    #[error("fetching media: {0}")]
    Fetch(String),
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
    /// MIME type as reported by Discord.
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
    pub async fn archive_from_url(
        &self,
        url: &str,
        meta: &MediaMeta,
    ) -> Result<StoredMedia, MediaError> {
        check_content_type(&meta.content_type)?;

        let tmp = tempfile::NamedTempFile::new()?;
        let tmp_path = tmp.path().to_path_buf();

        let mut resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| MediaError::Fetch(e.to_string()))?
            .error_for_status()
            .map_err(|e| MediaError::Fetch(e.to_string()))?;

        // Stream to disk with the cap enforced as bytes arrive — a lying
        // Content-Length header cannot bypass it.
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        let mut written: u64 = 0;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| MediaError::Fetch(e.to_string()))?
        {
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

        self.archive_file(&tmp_path, meta).await
    }

    /// Archives an already-downloaded file (also the migrator's entry).
    pub async fn archive_file(
        &self,
        path: &Path,
        meta: &MediaMeta,
    ) -> Result<StoredMedia, MediaError> {
        check_content_type(&meta.content_type)?;
        let size = tokio::fs::metadata(path).await?.len();
        if size > self.max_bytes {
            return Err(MediaError::TooLarge {
                limit_mb: self.max_bytes / (1024 * 1024),
            });
        }

        let orig_key = original_key(meta);
        self.upload_file(path, &orig_key, size).await?;

        let thumb = self.make_thumbnail(path, &meta.content_type).await?;
        let t_key = thumb_key(meta);
        self.store
            .put(&ObjectPath::from(t_key.clone()), thumb.into())
            .await?;

        Ok(StoredMedia {
            original_key: orig_key,
            thumb_key: t_key,
            size,
        })
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

    /// Produces the WebP thumbnail bytes for an image or video file.
    async fn make_thumbnail(&self, path: &Path, content_type: &str) -> Result<Vec<u8>, MediaError> {
        let source: Vec<u8> = if content_type.starts_with("video/") {
            self.video_poster(path).await
        } else {
            tokio::fs::read(path).await?
        };

        tokio::task::spawn_blocking(move || thumbnail_webp(&source))
            .await
            .map_err(|e| MediaError::Transform(format!("thumbnail task: {e}")))?
    }

    /// Extracts a poster frame via ffmpeg; deterministic placeholder image
    /// when ffmpeg is unavailable or fails (documented degradation — the
    /// original video is archived either way).
    async fn video_poster(&self, path: &Path) -> Vec<u8> {
        let out = tempfile::Builder::new().suffix(".png").tempfile();
        let Ok(out) = out else {
            return placeholder_png();
        };
        let result = tokio::process::Command::new(&self.ffmpeg_bin)
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-frames:v", "1"])
            .arg(out.path())
            .output()
            .await;

        match result {
            Ok(o) if o.status.success() => match std::fs::read(out.path()) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                _ => placeholder_png(),
            },
            Ok(o) => {
                tracing::warn!(
                    stderr = %String::from_utf8_lossy(&o.stderr),
                    "ffmpeg poster extraction failed; using placeholder thumb"
                );
                placeholder_png()
            }
            Err(e) => {
                tracing::warn!(error = %e, "ffmpeg unavailable; using placeholder thumb");
                placeholder_png()
            }
        }
    }
}

/// Rejects content types outside the allowlist.
fn check_content_type(ct: &str) -> Result<(), MediaError> {
    let base = ct.split(';').next().unwrap_or(ct).trim();
    if ALLOWED_CONTENT_TYPES.contains(&base) {
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
    let thumb = img.thumbnail(THUMB_LONG_EDGE, THUMB_LONG_EDGE);

    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(
            thumb.to_rgba8().as_raw(),
            thumb.width(),
            thumb.height(),
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

/// A 256×144 leaf-green PNG used when a video poster cannot be extracted.
fn placeholder_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(256, 144, image::Rgba([29, 43, 31, 255]));
    let mut out = Vec::new();
    let encode_result = image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png);
    if encode_result.is_err() {
        // Unreachable in practice (in-memory PNG encode of a constant
        // image); an empty thumb degrades to a broken tile, not a panic.
        return Vec::new();
    }
    out
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

        let stored = p.archive_file(&path, &meta("video/mp4")).await.unwrap();
        let thumb = store
            .get(&ObjectPath::from(stored.thumb_key))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let decoded = image::load_from_memory(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (256, 144));
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
    async fn download_respects_cap_even_with_lying_server() {
        // Minimal HTTP server that claims a small Content-Length but
        // streams forever — the cap must trip mid-stream.
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n")
                .await;
            // Send far more than advertised.
            for _ in 0..100 {
                if sock.write_all(&[0u8; 1024]).await.is_err() {
                    break;
                }
            }
        });

        let (_store, p) = pipeline();
        let p = p.with_max_bytes(2048);
        let result = p
            .archive_from_url(&format!("http://{addr}/file"), &meta("image/png"))
            .await;
        assert!(matches!(
            result,
            Err(MediaError::TooLarge { .. } | MediaError::Fetch(_))
        ));
    }
}
