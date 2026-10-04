//! The migration orchestrator: walks the source archive and writes a leaf
//! series, its posts, and media into the target database.
//!
//! Two entry points: [`plan`] (the `--dry-run` read-only planner) and [`run`]
//! (the real import). [`run`] is **idempotent** — every day is committed in
//! its own [`PostRepo::insert_with_media`] transaction, days that already
//! hold their files are skipped, and days whose source message could not be
//! fetched are *deferred* (left unwritten) rather than recorded as missing.
//! That makes re-running the natural resume mechanism: kill it, run it
//! again, and it continues where it stopped and retries anything transient.
//!
//! A day that is archived but holds no stored file (the bot's `/import`
//! writes such days, and so does an earlier run that met a failed download)
//! is *repaired*: if its source message is the same and its files can be
//! stored now, the day is replaced in one transaction. Otherwise it is left
//! exactly as it is.
//!
//! No stored object is deleted unless the database says no entry points at
//! it ([`PostRepo::unreferenced_keys`]): keys are a function of series, day
//! and attachment, and a renumbered day keeps its old keys.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use anyhow::Context as _;
use leaf_core::db::{DbError, GuildSettingsRepo, PostRepo, SeriesRepo};
use leaf_core::domain::{
    Cadence, DetectionMode, NewMediaAttachment, NewSeries, Post, Privacy, Series, SeriesState,
};
use leaf_core::media::{self, MediaPipeline};
use leaf_core::transfer::TransferPost;

use crate::discord::{FetchedMessage, MessageSource};
use crate::mapping;

/// Inputs that come from the CLI rather than the source archive.
#[derive(Debug, Clone)]
pub struct ImportConfig {
    /// Target guild snowflake.
    pub guild_id: String,
    /// Creator (owner) snowflake for the imported series.
    pub creator_id: String,
    /// Name of the series to create or reuse.
    pub series_name: String,
    /// Explicit watched channels for the series; when empty, the distinct
    /// channels seen in the source are used.
    pub series_channels: Vec<String>,
    /// Offset added to every v2 day number (`--day-offset`).
    pub day_offset: i64,
}

/// The repositories and pipeline a real import writes through.
pub struct Target<'a> {
    /// Series repository.
    pub series: &'a SeriesRepo,
    /// Post + media repository.
    pub posts: &'a PostRepo,
    /// Guild-settings repository (the series FK target).
    pub guilds: &'a GuildSettingsRepo,
    /// Media pipeline (R2 upload + thumbnails).
    pub media: &'a MediaPipeline,
}

/// Why a particular day needs manual follow-up after the import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// The source message was deleted (HTTP 404); media was recorded as
    /// missing placeholders recovered from v2's stored URLs.
    MessageDeleted,
    /// The message was fetched, but an attachment could not be downloaded or
    /// transformed; recorded as a missing placeholder.
    MediaUnfetchable,
    /// The message could not be fetched due to a transient/unknown error; the
    /// day was left unimported so a re-run retries it.
    FetchDeferred,
    /// The message is gone and the source held no recoverable media URLs.
    NoMediaRecovered,
}

impl GapReason {
    /// Stable machine-readable label (used in the gaps report).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MessageDeleted => "message_deleted",
            Self::MediaUnfetchable => "media_unfetchable",
            Self::FetchDeferred => "fetch_deferred",
            Self::NoMediaRecovered => "no_media_recovered",
        }
    }
}

/// One day that did not import cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// Leaf day number.
    pub day: i64,
    /// Source message snowflake.
    pub message_id: String,
    /// Category of the gap.
    pub reason: GapReason,
    /// Human-readable explanation.
    pub detail: String,
}

/// What an import (or plan) did. Counts are days unless noted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Id of the created/reused series (0 in a dry run that found none).
    pub series_id: i64,
    /// Total posts in the source archive.
    pub total_source: usize,
    /// Days written (or, in a dry run, that *would* be written).
    pub imported: usize,
    /// Days skipped because they were already present.
    pub skipped_existing: usize,
    /// Days that were archived without any stored file and now have theirs
    /// (or, in a dry run, that would be tried).
    pub repaired: usize,
    /// Days left as they were after a transient fetch error (retry on
    /// re-run): not written at all, or still without their files.
    pub deferred: usize,
    /// Attachments fetched and stored in R2.
    pub media_stored: usize,
    /// Attachments recorded as missing placeholders.
    pub media_missing: usize,
    /// Per-day follow-ups for the gaps report.
    pub gaps: Vec<Gap>,
}

/// Plans an import without writing anything (`--dry-run`).
///
/// Reports how many days would import vs. are already present, and flags
/// source days that carry no media URLs. Media outcomes that depend on the
/// live message (deleted / unfetchable) are only known during a real [`run`].
pub async fn plan(
    source: &[TransferPost],
    cfg: &ImportConfig,
    series_repo: &SeriesRepo,
    post_repo: &PostRepo,
) -> anyhow::Result<Summary> {
    let existing = series_repo
        .get_by_name(&cfg.guild_id, &cfg.series_name)
        .await?;
    let series_id = existing.as_ref().map_or(0, |s| s.id);
    let (existing_days, placeholders): (BTreeSet<i64>, BTreeSet<i64>) = match &existing {
        Some(s) => (
            post_repo.all_days(s.id).await?.into_iter().collect(),
            post_repo
                .placeholder_days(s.id)
                .await?
                .into_iter()
                .collect(),
        ),
        None => (BTreeSet::new(), BTreeSet::new()),
    };

    let mut summary = Summary {
        series_id,
        total_source: source.len(),
        ..Summary::default()
    };
    for p in source {
        let day = mapping::leaf_day(p.day, cfg.day_offset);
        if existing_days.contains(&day) {
            // A day with no stored file is tried again; whether its files
            // can still be fetched is only known in a real run.
            if placeholders.contains(&day) {
                summary.repaired += 1;
            } else {
                summary.skipped_existing += 1;
            }
            continue;
        }
        summary.imported += 1;
        if p.media.is_empty() {
            summary.gaps.push(Gap {
                day,
                message_id: p.message_id.clone(),
                reason: GapReason::NoMediaRecovered,
                detail: "source has no media URLs for this day".to_owned(),
            });
        }
    }
    Ok(summary)
}

/// Runs the import: ensures the guild + series exist, then imports every
/// not-yet-present day and repairs the days that hold no stored file.
pub async fn run<S: MessageSource + Sync>(
    source: &[TransferPost],
    cfg: &ImportConfig,
    target: &Target<'_>,
    messages: &S,
    now_unix: i64,
) -> anyhow::Result<Summary> {
    target
        .guilds
        .ensure_exists(&cfg.guild_id)
        .await
        .context("ensuring guild settings row")?;
    let series = ensure_series(source, cfg, target, now_unix).await?;
    let existing_days: BTreeSet<i64> = target
        .posts
        .all_days(series.id)
        .await?
        .into_iter()
        .collect();
    let placeholders: BTreeSet<i64> = target
        .posts
        .placeholder_days(series.id)
        .await?
        .into_iter()
        .collect();

    let runner = Run {
        cfg,
        posts: target.posts,
        media: target.media,
        messages,
        now_unix,
    };
    let mut summary = Summary {
        series_id: series.id,
        total_source: source.len(),
        ..Summary::default()
    };
    for p in source {
        let day = mapping::leaf_day(p.day, cfg.day_offset);
        if existing_days.contains(&day) {
            if placeholders.contains(&day) {
                runner.repair_one(p, day, series.id, &mut summary).await?;
            } else {
                summary.skipped_existing += 1;
            }
            continue;
        }
        runner.import_one(p, day, series.id, &mut summary).await?;
    }
    Ok(summary)
}

/// Finds the named series, or creates it (Active/Public/Daily — an
/// established archive, not a sprout). Reusing on re-run keeps the series id
/// (and therefore the R2 keys) stable.
async fn ensure_series(
    source: &[TransferPost],
    cfg: &ImportConfig,
    target: &Target<'_>,
    now_unix: i64,
) -> anyhow::Result<Series> {
    if let Some(s) = target
        .series
        .get_by_name(&cfg.guild_id, &cfg.series_name)
        .await?
    {
        return Ok(s);
    }
    let start_day = source
        .iter()
        .map(|p| mapping::leaf_day(p.day, cfg.day_offset))
        .min()
        .unwrap_or(1);
    let new = NewSeries {
        guild_id: cfg.guild_id.clone(),
        creator_id: cfg.creator_id.clone(),
        name: cfg.series_name.clone(),
        description: String::new(),
        channels: resolve_channels(source, cfg),
        cadence: Cadence::Daily,
        detection_mode: DetectionMode::ContextMenu,
        privacy: Privacy::Public,
        privacy_role_id: None,
        start_day,
        state: SeriesState::Active,
    };
    match target.series.create(&new, now_unix).await {
        Ok(s) => Ok(s),
        // Lost a race (or a half-finished prior run created it): adopt it.
        Err(DbError::SeriesNameTaken) => target
            .series
            .get_by_name(&cfg.guild_id, &cfg.series_name)
            .await?
            .context("series name taken but the row could not be read back"),
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

/// The series' watched channels: the explicit CLI set if given, else the
/// distinct channels the source posts came from.
fn resolve_channels(source: &[TransferPost], cfg: &ImportConfig) -> Vec<String> {
    if !cfg.series_channels.is_empty() {
        return cfg.series_channels.clone();
    }
    source
        .iter()
        .map(|p| p.channel_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Borrowed context for importing one day (keeps per-call argument lists small).
struct Run<'a, S: MessageSource + Sync> {
    cfg: &'a ImportConfig,
    posts: &'a PostRepo,
    media: &'a MediaPipeline,
    messages: &'a S,
    now_unix: i64,
}

/// The media plan for a single day, accumulated before the (atomic) insert.
#[derive(Default)]
struct DayMedia {
    caption: String,
    media: Vec<NewMediaAttachment>,
    gaps: Vec<Gap>,
    stored: usize,
    missing: usize,
}

/// Whether an archived media row holds a stored file.
const fn holds_a_file(row: &leaf_core::domain::MediaAttachment) -> bool {
    !row.media_missing || row.original_key.is_some() || row.thumb_key.is_some()
}

/// The storage keys the stored attachments of a plan occupy.
fn stored_keys(media: &[NewMediaAttachment]) -> Vec<String> {
    media
        .iter()
        .flat_map(|m| [m.original_key.clone(), m.thumb_key.clone()])
        .flatten()
        .collect()
}

impl<S: MessageSource + Sync> Run<'_, S> {
    /// Deletes the objects under `keys` that no archived day points at. A
    /// failed upload can leave an object behind, and so can a day that lost
    /// its insert; a key another day still uses is never touched.
    async fn release(&self, keys: Vec<String>) {
        if keys.is_empty() {
            return;
        }
        match self.posts.unreferenced_keys(&keys).await {
            Ok(free) => self.media.delete_keys(&free).await,
            Err(e) => {
                tracing::warn!(error = %e, "could not check which stored files are unused; left in place");
            }
        }
    }

    /// Gives an archived day that holds no stored file its files, when its
    /// source message is still the one the day was written from and at least
    /// one file can be stored now. Anything else leaves the day untouched:
    /// counted as deferred (with a gap) when a re-run could still fill it,
    /// as already present otherwise.
    ///
    /// The list of such days was read before the run started, and a run is
    /// long: the day may have been archived for real since. It is read
    /// again here, and the replace itself only happens while the day is
    /// still without a file (see
    /// [`PostRepo::replace_placeholder_with_media`]).
    async fn repair_one(
        &self,
        p: &TransferPost,
        day: i64,
        series_id: i64,
        summary: &mut Summary,
    ) -> anyhow::Result<()> {
        let Some((existing, held)) = self.posts.get(series_id, day).await? else {
            // Removed since the listing: an ordinary import.
            return self.import_one(p, day, series_id, summary).await;
        };
        if existing.message_id != p.message_id {
            // Someone archived another post as this day: theirs to keep.
            summary.skipped_existing += 1;
            return Ok(());
        }
        if held.iter().any(holds_a_file) {
            // Filled in since the listing (archived again by its creator,
            // or named twice in the source): nothing left to repair.
            summary.skipped_existing += 1;
            return Ok(());
        }
        let msg = match self.messages.fetch(&p.channel_id, &p.message_id).await {
            Ok(Some(msg)) if !msg.attachments.is_empty() => msg,
            // Deleted or emptied: nothing to gain, now or on a re-run.
            Ok(_) => {
                summary.skipped_existing += 1;
                return Ok(());
            }
            // Unreachable right now (no access, rate limit, network): the
            // day stays as it is, and a re-run can still fill it.
            Err(e) => {
                summary.deferred += 1;
                summary.gaps.push(Gap {
                    day,
                    message_id: p.message_id.clone(),
                    reason: GapReason::FetchDeferred,
                    detail: e,
                });
                return Ok(());
            }
        };

        let plan = self.build_present(&msg, p, day, series_id).await;
        if plan.stored == 0 {
            // Every download failed: the day keeps its placeholder, and the
            // report says why each file is still missing.
            summary.skipped_existing += 1;
            summary.gaps.extend(plan.gaps);
            return Ok(());
        }
        let post = Post {
            series_id,
            day,
            message_id: existing.message_id,
            channel_id: existing.channel_id,
            caption: plan.caption,
            // The day keeps its place on the calendar.
            posted_at: existing.posted_at,
            archived_at: self.now_unix,
        };
        match self
            .posts
            .replace_placeholder_with_media(&post, &plan.media)
            .await
        {
            Ok(Some(freed)) => {
                self.media.delete_keys(&freed).await;
                summary.repaired += 1;
                summary.media_stored += plan.stored;
                summary.media_missing += plan.missing;
                summary.gaps.extend(plan.gaps);
                Ok(())
            }
            // The day was archived for real while its files were being
            // fetched: that entry stays, and only uploads it does not use
            // are removed.
            Ok(None) => {
                self.release(stored_keys(&plan.media)).await;
                summary.skipped_existing += 1;
                Ok(())
            }
            Err(e) => {
                self.release(stored_keys(&plan.media)).await;
                Err(anyhow::Error::new(e)).with_context(|| format!("repairing day {day}"))
            }
        }
    }

    async fn import_one(
        &self,
        p: &TransferPost,
        day: i64,
        series_id: i64,
        summary: &mut Summary,
    ) -> anyhow::Result<()> {
        let fetched = match self.messages.fetch(&p.channel_id, &p.message_id).await {
            Ok(opt) => opt,
            Err(e) => {
                // Transient/unknown: defer so a re-run retries this day rather
                // than freezing in recoverable bytes as "missing".
                summary.deferred += 1;
                summary.gaps.push(Gap {
                    day,
                    message_id: p.message_id.clone(),
                    reason: GapReason::FetchDeferred,
                    detail: e,
                });
                return Ok(());
            }
        };

        let plan = match fetched {
            Some(msg) => self.build_present(&msg, p, day, series_id).await,
            None => build_missing(p, day),
        };

        let post = Post {
            series_id,
            day,
            message_id: p.message_id.clone(),
            channel_id: p.channel_id.clone(),
            caption: plan.caption,
            posted_at: p.timestamp,
            archived_at: self.now_unix,
        };
        match self.posts.insert_with_media(&post, &plan.media).await {
            Ok(()) => {
                summary.imported += 1;
                summary.media_stored += plan.stored;
                summary.media_missing += plan.missing;
                summary.gaps.extend(plan.gaps);
                Ok(())
            }
            // Pre-checked as absent, so this only fires on a true race; treat
            // it as already-present rather than failing the whole run.
            Err(DbError::DuplicateDay(_)) => {
                self.release(stored_keys(&plan.media)).await;
                summary.skipped_existing += 1;
                Ok(())
            }
            Err(e) => {
                self.release(stored_keys(&plan.media)).await;
                Err(anyhow::Error::new(e)).with_context(|| format!("inserting day {day}"))
            }
        }
    }

    /// Builds the media plan when the live message was found: archive each
    /// attachment through the pipeline; failures become missing placeholders.
    async fn build_present(
        &self,
        msg: &FetchedMessage,
        p: &TransferPost,
        day: i64,
        series_id: i64,
    ) -> DayMedia {
        let mut plan = DayMedia {
            caption: msg.content.clone(),
            ..DayMedia::default()
        };

        if msg.attachments.is_empty() {
            // The message exists but has no attachments (e.g. edited to remove
            // them). Fall back to v2's stored URLs as missing placeholders.
            for url in &p.media {
                plan.media.push(mapping::missing_attachment_from_url(
                    url,
                    &p.channel_id,
                    &p.message_id,
                ));
                plan.missing += 1;
            }
            if !p.media.is_empty() {
                plan.gaps.push(Gap {
                    day,
                    message_id: p.message_id.clone(),
                    reason: GapReason::MediaUnfetchable,
                    detail: "live message has no attachments; recorded v2 URLs as missing"
                        .to_owned(),
                });
            }
            return plan;
        }

        for att in &msg.attachments {
            let meta = mapping::media_meta(
                &self.cfg.guild_id,
                series_id,
                day,
                &att.id,
                &att.content_type,
            );
            match self.media.archive_from_url(&att.url, &meta).await {
                Ok(stored) => {
                    plan.media.push(mapping::stored_attachment(
                        &att.id,
                        &p.channel_id,
                        &p.message_id,
                        &att.content_type,
                        &stored,
                    ));
                    plan.stored += 1;
                }
                Err(e) => {
                    // A failed archive may have stored the original before
                    // the step that failed.
                    self.release(vec![media::original_key(&meta), media::thumb_key(&meta)])
                        .await;
                    plan.media.push(mapping::missing_attachment_live(
                        &att.id,
                        &p.channel_id,
                        &p.message_id,
                        &att.content_type,
                    ));
                    plan.missing += 1;
                    plan.gaps.push(Gap {
                        day,
                        message_id: p.message_id.clone(),
                        reason: GapReason::MediaUnfetchable,
                        detail: format!("attachment {}: {e}", att.id),
                    });
                }
            }
        }
        plan
    }
}

/// Builds the media plan when the live message is gone: recover ids from the
/// v2 URLs as missing placeholders.
fn build_missing(p: &TransferPost, day: i64) -> DayMedia {
    let mut plan = DayMedia::default();
    if p.media.is_empty() {
        plan.gaps.push(Gap {
            day,
            message_id: p.message_id.clone(),
            reason: GapReason::NoMediaRecovered,
            detail: "source message deleted; no media URLs to recover".to_owned(),
        });
        return plan;
    }
    for url in &p.media {
        plan.media.push(mapping::missing_attachment_from_url(
            url,
            &p.channel_id,
            &p.message_id,
        ));
        plan.missing += 1;
    }
    plan.gaps.push(Gap {
        day,
        message_id: p.message_id.clone(),
        reason: GapReason::MessageDeleted,
        detail: format!(
            "source message deleted; {} media recorded as missing",
            p.media.len()
        ),
    });
    plan
}

/// Renders the gaps report as Markdown.
#[must_use]
pub fn render_gaps_markdown(series_name: &str, gaps: &[Gap]) -> String {
    // write! to a String is infallible.
    let mut out = String::new();
    let _ = writeln!(out, "# leaf-migrate gaps report — {series_name}\n");
    if gaps.is_empty() {
        out.push_str("No gaps: every imported day kept all of its media.\n");
        return out;
    }
    let _ = writeln!(out, "{} gap(s) need follow-up.\n", gaps.len());
    out.push_str("| day | message_id | reason | detail |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for g in gaps {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            g.day,
            g.message_id,
            g.reason.as_str(),
            sanitize_cell(&g.detail),
        );
    }
    out
}

/// Escapes a value for a Markdown table cell.
fn sanitize_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::similar_names,
        reason = "tests may panic; short paired fixture names are clear in context"
    )]

    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::Arc;

    use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo};
    use leaf_core::media::MediaPipeline;
    use object_store::ObjectStore;
    use object_store::memory::InMemory;
    use object_store::path::Path as ObjectPath;

    use super::*;
    use crate::discord::{FetchedAttachment, FetchedMessage, MessageSource};

    /// A day's fetch outcome in the fake source.
    enum Outcome {
        Present(FetchedMessage),
        Deleted,
        Error,
    }

    struct FakeSource {
        by_message: HashMap<String, Outcome>,
    }

    impl MessageSource for FakeSource {
        async fn fetch(
            &self,
            _channel: &str,
            message_id: &str,
        ) -> Result<Option<FetchedMessage>, String> {
            match self.by_message.get(message_id) {
                Some(Outcome::Present(m)) => Ok(Some(m.clone())),
                Some(Outcome::Deleted) | None => Ok(None),
                Some(Outcome::Error) => Err("simulated transient error".to_owned()),
            }
        }
    }

    fn png_bytes() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// Serves `body` as image/png for any GET, until the test ends. Returns
    /// the base URL.
    async fn serve_png(body: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _read = sock.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _w1 = sock.write_all(head.as_bytes()).await;
                    let _w2 = sock.write_all(&body).await;
                    let _f = sock.flush().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn present(content: &str, base: &str, atts: &[&str]) -> Outcome {
        Outcome::Present(FetchedMessage {
            content: content.to_owned(),
            attachments: atts
                .iter()
                .map(|id| FetchedAttachment {
                    id: (*id).to_owned(),
                    content_type: "image/png".to_owned(),
                    url: format!("{base}/{id}.png"),
                })
                .collect(),
        })
    }

    fn tp(day: i64, message_id: &str, media: Vec<String>) -> TransferPost {
        TransferPost {
            day,
            message_id: message_id.to_owned(),
            channel_id: "c1".to_owned(),
            user_id: "johan".to_owned(),
            timestamp: 1_000 + day,
            media,
        }
    }

    fn cfg() -> ImportConfig {
        ImportConfig {
            guild_id: "g1".to_owned(),
            creator_id: "johan".to_owned(),
            series_name: "Daily Johan".to_owned(),
            series_channels: Vec::new(),
            day_offset: 0,
        }
    }

    struct Db {
        _dir: tempfile::TempDir,
        series: SeriesRepo,
        posts: PostRepo,
        guilds: GuildSettingsRepo,
        media: MediaPipeline,
        store: Arc<InMemory>,
    }

    async fn db() -> Db {
        let dir = tempfile::tempdir().unwrap();
        let pool = leaf_core::db::connect(&dir.path().join("leaf.db"))
            .await
            .unwrap();
        let store = Arc::new(InMemory::new());
        let media = MediaPipeline::new(Arc::clone(&store) as Arc<dyn ObjectStore>).unwrap();
        Db {
            _dir: dir,
            series: SeriesRepo::new(pool.clone()),
            posts: PostRepo::new(pool.clone()),
            guilds: GuildSettingsRepo::new(pool),
            media,
            store,
        }
    }

    impl Db {
        fn target(&self) -> Target<'_> {
            Target {
                series: &self.series,
                posts: &self.posts,
                guilds: &self.guilds,
                media: &self.media,
            }
        }
    }

    #[tokio::test]
    async fn imports_present_recovers_deleted_and_defers_errors() {
        let db = db().await;
        let base = serve_png(png_bytes()).await;

        let source = vec![
            tp(1, "m1", vec![format!("{base}/att1.png")]),
            tp(
                2,
                "m2",
                vec!["https://cdn.discordapp.com/attachments/c1/2002/x.png".to_owned()],
            ),
            tp(3, "m3", vec![format!("{base}/att3.png")]),
        ];
        let mut by_message = HashMap::new();
        by_message.insert("m1".to_owned(), present("caption one", &base, &["att1"]));
        by_message.insert("m2".to_owned(), Outcome::Deleted);
        by_message.insert("m3".to_owned(), Outcome::Error);
        let fake = FakeSource { by_message };

        let summary = run(&source, &cfg(), &db.target(), &fake, 9_000)
            .await
            .unwrap();

        assert_eq!(summary.imported, 2); // day 1 + day 2
        assert_eq!(summary.deferred, 1); // day 3
        assert_eq!(summary.media_stored, 1);
        assert_eq!(summary.media_missing, 1);
        let sid = summary.series_id;

        // Day 1: live caption + stored media whose object actually exists.
        let (post1, media1) = db.posts.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(post1.caption, "caption one");
        assert_eq!(post1.posted_at, 1_001);
        assert_eq!(post1.archived_at, 9_000);
        let att1 = media1.first().unwrap();
        assert!(!att1.media_missing);
        let key = att1.original_key.clone().unwrap();
        assert!(db.store.head(&ObjectPath::from(key)).await.is_ok());

        // Day 2: deleted → one missing attachment, id recovered from the URL.
        let (post2, media2) = db.posts.get(sid, 2).await.unwrap().unwrap();
        assert_eq!(post2.caption, "");
        let att2 = media2.first().unwrap();
        assert!(att2.media_missing);
        assert_eq!(att2.attachment_id, "2002");
        assert!(att2.original_key.is_none());

        // Day 3: deferred → not written.
        assert!(db.posts.get(sid, 3).await.unwrap().is_none());

        assert!(
            summary
                .gaps
                .iter()
                .any(|g| g.day == 2 && g.reason == GapReason::MessageDeleted)
        );
        assert!(
            summary
                .gaps
                .iter()
                .any(|g| g.day == 3 && g.reason == GapReason::FetchDeferred)
        );

        // Series created Active/Public with the source channel.
        let series = db.series.get(sid).await.unwrap().unwrap();
        assert_eq!(series.state, SeriesState::Active);
        assert_eq!(series.channels, vec!["c1".to_owned()]);
        assert_eq!(series.start_day, 1);
    }

    #[tokio::test]
    async fn rerun_is_idempotent_and_resumes_deferred_days() {
        let db = db().await;
        let base = serve_png(png_bytes()).await;
        let source = vec![
            tp(1, "m1", vec![format!("{base}/att1.png")]),
            tp(2, "m2", vec![format!("{base}/att2.png")]),
        ];

        // Run 1: day 2's message errors → deferred; day 1 imports.
        let mut first = HashMap::new();
        first.insert("m1".to_owned(), present("c1", &base, &["att1"]));
        first.insert("m2".to_owned(), Outcome::Error);
        let s1 = run(
            &source,
            &cfg(),
            &db.target(),
            &FakeSource { by_message: first },
            1,
        )
        .await
        .unwrap();
        assert_eq!(s1.imported, 1);
        assert_eq!(s1.deferred, 1);
        let sid = s1.series_id;

        // Run 2: identical inputs (day 2 still errors). Day 1 skipped, no
        // re-upload, series reused.
        let mut second = HashMap::new();
        second.insert("m1".to_owned(), present("c1", &base, &["att1"]));
        second.insert("m2".to_owned(), Outcome::Error);
        let s2 = run(
            &source,
            &cfg(),
            &db.target(),
            &FakeSource { by_message: second },
            2,
        )
        .await
        .unwrap();
        assert_eq!(s2.series_id, sid);
        assert_eq!(s2.imported, 0);
        assert_eq!(s2.skipped_existing, 1);
        assert_eq!(s2.deferred, 1);
        assert_eq!(s2.media_stored, 0, "must not re-upload an existing day");

        // Run 3: day 2 now fetchable → retried and imported.
        let mut third = HashMap::new();
        third.insert("m1".to_owned(), present("c1", &base, &["att1"]));
        third.insert("m2".to_owned(), present("c2", &base, &["att2"]));
        let s3 = run(
            &source,
            &cfg(),
            &db.target(),
            &FakeSource { by_message: third },
            3,
        )
        .await
        .unwrap();
        assert_eq!(s3.imported, 1);
        assert_eq!(s3.skipped_existing, 1);
        assert_eq!(s3.deferred, 0);
        assert_eq!(s3.media_stored, 1);

        assert!(db.posts.get(sid, 2).await.unwrap().is_some());
        assert_eq!(db.posts.count(sid).await.unwrap(), 2);
    }

    /// Writes a day the way the bot's `/import` does: no caption, one media
    /// row that says the file was never captured.
    async fn placeholder(db: &Db, series_id: i64, day: i64, message_id: &str) {
        db.posts
            .insert_with_media(
                &Post {
                    series_id,
                    day,
                    message_id: message_id.to_owned(),
                    channel_id: "c1".to_owned(),
                    caption: String::new(),
                    posted_at: 500 + day,
                    archived_at: 600,
                },
                &[NewMediaAttachment {
                    attachment_id: format!("import-{message_id}-0"),
                    channel_id: "c1".to_owned(),
                    message_id: message_id.to_owned(),
                    content_type: String::new(),
                    original_key: None,
                    thumb_key: None,
                    media_missing: true,
                }],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn days_imported_without_media_get_their_files() {
        let db = db().await;
        let base = serve_png(png_bytes()).await;
        let source = vec![
            tp(1, "m1", vec![format!("{base}/att1.png")]),
            tp(2, "m2", vec![format!("{base}/att2.png")]),
            tp(3, "m3", vec![format!("{base}/att3.png")]),
        ];
        // An empty first run creates the series; then `/import` wrote three
        // days without media. Day 3 has since been archived from another post.
        let sid = run(
            &[],
            &cfg(),
            &db.target(),
            &FakeSource {
                by_message: HashMap::new(),
            },
            1,
        )
        .await
        .unwrap()
        .series_id;
        placeholder(&db, sid, 1, "m1").await;
        placeholder(&db, sid, 2, "m2").await;
        placeholder(&db, sid, 3, "someone-elses-post").await;

        // The dry run counts all three as days without files that would be
        // tried: which message a day holds is only read in a real run.
        let planned = plan(&source, &cfg(), &db.series, &db.posts).await.unwrap();
        assert_eq!((planned.repaired, planned.imported), (3, 0));

        let mut by_message = HashMap::new();
        by_message.insert("m1".to_owned(), present("caption one", &base, &["att1"]));
        by_message.insert("m2".to_owned(), Outcome::Deleted);
        by_message.insert("m3".to_owned(), present("three", &base, &["att3"]));
        let fake = FakeSource { by_message };
        let s = run(&source, &cfg(), &db.target(), &fake, 9_000)
            .await
            .unwrap();
        assert_eq!(s.repaired, 1);
        assert_eq!(s.imported, 0);
        assert_eq!(s.skipped_existing, 2);
        assert_eq!(s.deferred, 0);
        assert_eq!(s.media_stored, 1);

        // Day 1 now has its file and caption, and kept its date.
        let (post, media) = db.posts.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(post.caption, "caption one");
        assert_eq!(post.posted_at, 501);
        assert_eq!(post.archived_at, 9_000);
        assert_eq!(media.len(), 1);
        let att = media.first().unwrap();
        assert_eq!(att.attachment_id, "att1");
        assert!(!att.media_missing);
        let key = att.original_key.clone().unwrap();
        assert!(db.store.head(&ObjectPath::from(key)).await.is_ok());

        // Day 2's message is gone: left exactly as the import wrote it.
        let (post, media) = db.posts.get(sid, 2).await.unwrap().unwrap();
        assert_eq!(post.archived_at, 600);
        assert_eq!(media.first().unwrap().attachment_id, "import-m2-0");
        // Day 3 holds another post: not this file's to change.
        let (post, _) = db.posts.get(sid, 3).await.unwrap().unwrap();
        assert_eq!(post.message_id, "someone-elses-post");
        assert_eq!(post.archived_at, 600);

        // A second run finds day 1 whole and stores nothing again.
        let again = run(&source, &cfg(), &db.target(), &fake, 9_500)
            .await
            .unwrap();
        assert_eq!((again.repaired, again.media_stored), (0, 0));
        assert_eq!(again.skipped_existing, 3);
        assert_eq!(db.posts.placeholder_days(sid).await.unwrap(), [2, 3]);
    }

    /// A message source that, while the migration is fetching `m1`, does
    /// what a creator does on seeing an empty day: archives that message as
    /// Day 1 for real, with both of its files.
    struct ArchivedMeanwhile {
        posts: PostRepo,
        media: MediaPipeline,
        series_id: i64,
        base: String,
        /// What the migration is told the message holds.
        seen: FetchedMessage,
    }

    impl MessageSource for ArchivedMeanwhile {
        async fn fetch(
            &self,
            _channel: &str,
            _message_id: &str,
        ) -> Result<Option<FetchedMessage>, String> {
            let mut rows = Vec::new();
            for id in ["att1", "att2"] {
                let meta = mapping::media_meta("g1", self.series_id, 1, id, "image/png");
                let stored = self
                    .media
                    .archive_from_url(&format!("{}/{id}.png", self.base), &meta)
                    .await
                    .unwrap();
                rows.push(mapping::stored_attachment(
                    id,
                    "c1",
                    "m1",
                    "image/png",
                    &stored,
                ));
            }
            let post = Post {
                series_id: self.series_id,
                day: 1,
                message_id: "m1".to_owned(),
                channel_id: "c1".to_owned(),
                caption: "archived by its creator".to_owned(),
                posted_at: 501,
                archived_at: 8_000,
            };
            self.posts.replace_with_media(&post, &rows).await.unwrap();
            Ok(Some(self.seen.clone()))
        }
    }

    #[tokio::test]
    async fn a_day_archived_for_real_during_the_run_keeps_its_files() {
        let db = db().await;
        let base = serve_png(png_bytes()).await;
        let source = vec![tp(1, "m1", vec![format!("{base}/att1.png")])];
        let sid = run(
            &[],
            &cfg(),
            &db.target(),
            &FakeSource {
                by_message: HashMap::new(),
            },
            1,
        )
        .await
        .unwrap()
        .series_id;
        placeholder(&db, sid, 1, "m1").await;

        // The migration downloads att1 but not att2 (nothing listens on
        // that port), so its own version of the day would hold one file.
        let racing = ArchivedMeanwhile {
            posts: db.posts.clone(),
            media: db.media.clone(),
            series_id: sid,
            base: base.clone(),
            seen: FetchedMessage {
                content: "from the migration".to_owned(),
                attachments: vec![
                    FetchedAttachment {
                        id: "att1".to_owned(),
                        content_type: "image/png".to_owned(),
                        url: format!("{base}/att1.png"),
                    },
                    FetchedAttachment {
                        id: "att2".to_owned(),
                        content_type: "image/png".to_owned(),
                        url: "http://127.0.0.1:1/att2.png".to_owned(),
                    },
                ],
            },
        };
        let s = run(&source, &cfg(), &db.target(), &racing, 9_000)
            .await
            .unwrap();
        assert_eq!((s.repaired, s.skipped_existing), (0, 1));

        // The creator's entry is the one that stands, with both files in
        // the database and in storage.
        let (post, media) = db.posts.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(post.caption, "archived by its creator");
        assert_eq!(media.len(), 2);
        for row in &media {
            assert!(!row.media_missing, "{}", row.attachment_id);
            for key in [row.original_key.clone(), row.thumb_key.clone()] {
                let key = key.unwrap();
                assert!(
                    db.store.head(&ObjectPath::from(key.clone())).await.is_ok(),
                    "{key} was deleted"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_day_named_twice_is_repaired_once() {
        let db = db().await;
        let base = serve_png(png_bytes()).await;
        // The source lists Day 1 twice.
        let source = vec![
            tp(1, "m1", vec![format!("{base}/att1.png")]),
            tp(1, "m1", vec![format!("{base}/att1.png")]),
        ];
        let sid = run(
            &[],
            &cfg(),
            &db.target(),
            &FakeSource {
                by_message: HashMap::new(),
            },
            1,
        )
        .await
        .unwrap()
        .series_id;
        placeholder(&db, sid, 1, "m1").await;

        let mut by_message = HashMap::new();
        by_message.insert("m1".to_owned(), present("one", &base, &["att1"]));
        let s = run(
            &source,
            &cfg(),
            &db.target(),
            &FakeSource { by_message },
            9_000,
        )
        .await
        .unwrap();
        // The second pass finds the day whole and leaves it alone.
        assert_eq!((s.repaired, s.skipped_existing, s.media_stored), (1, 1, 1));
        let (_, media) = db.posts.get(sid, 1).await.unwrap().unwrap();
        let key = media.first().unwrap().original_key.clone().unwrap();
        assert!(db.store.head(&ObjectPath::from(key)).await.is_ok());
    }

    #[tokio::test]
    async fn a_repair_that_could_not_fetch_is_deferred_and_reported() {
        let db = db().await;
        let source = vec![
            tp(1, "m1", vec!["https://cdn.example/att1.png".to_owned()]),
            tp(2, "m2", vec!["https://cdn.example/att2.png".to_owned()]),
        ];
        let sid = run(
            &[],
            &cfg(),
            &db.target(),
            &FakeSource {
                by_message: HashMap::new(),
            },
            1,
        )
        .await
        .unwrap()
        .series_id;
        placeholder(&db, sid, 1, "m1").await;
        placeholder(&db, sid, 2, "m2").await;

        // Day 1: Discord refuses the fetch (a bad token, no channel access).
        // Day 2: the message is there, but its only file cannot be fetched.
        let mut by_message = HashMap::new();
        by_message.insert("m1".to_owned(), Outcome::Error);
        by_message.insert(
            "m2".to_owned(),
            present("two", "http://127.0.0.1:1", &["att2"]),
        );
        let s = run(
            &source,
            &cfg(),
            &db.target(),
            &FakeSource { by_message },
            9_000,
        )
        .await
        .unwrap();

        // Neither day changed, and the operator is told a re-run can help.
        assert_eq!((s.repaired, s.deferred, s.skipped_existing), (0, 1, 1));
        let reasons: Vec<_> = s.gaps.iter().map(|g| (g.day, g.reason)).collect();
        assert_eq!(
            reasons,
            [
                (1, GapReason::FetchDeferred),
                (2, GapReason::MediaUnfetchable)
            ]
        );
        assert_eq!(db.posts.placeholder_days(sid).await.unwrap(), [1, 2]);
        let (post, _) = db.posts.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(post.archived_at, 600);
    }

    #[test]
    fn stored_keys_lists_originals_and_thumbnails_only_for_stored_files() {
        let stored = NewMediaAttachment {
            attachment_id: "a".to_owned(),
            channel_id: "c".to_owned(),
            message_id: "m".to_owned(),
            content_type: "image/png".to_owned(),
            original_key: Some("o".to_owned()),
            thumb_key: Some("t".to_owned()),
            media_missing: false,
        };
        let missing = NewMediaAttachment {
            original_key: None,
            thumb_key: None,
            media_missing: true,
            ..stored.clone()
        };
        assert_eq!(stored_keys(&[stored, missing]), ["o", "t"]);
    }

    #[tokio::test]
    async fn dry_run_plans_without_writing() {
        let db = db().await;
        let source = vec![tp(1, "m1", vec!["u".to_owned()]), tp(2, "m2", vec![])];

        let summary = plan(&source, &cfg(), &db.series, &db.posts).await.unwrap();
        assert_eq!(summary.total_source, 2);
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.skipped_existing, 0);
        assert_eq!(summary.series_id, 0);
        assert!(
            summary
                .gaps
                .iter()
                .any(|g| g.day == 2 && g.reason == GapReason::NoMediaRecovered)
        );

        // Nothing was written: no series, no guild row.
        assert!(
            db.series
                .get_by_name("g1", "Daily Johan")
                .await
                .unwrap()
                .is_none()
        );
        assert!(db.guilds.get("g1").await.unwrap().is_none());
    }

    #[test]
    fn renders_empty_and_populated_reports() {
        assert!(render_gaps_markdown("S", &[]).contains("No gaps"));

        let gaps = vec![Gap {
            day: 2,
            message_id: "m2".to_owned(),
            reason: GapReason::MessageDeleted,
            detail: "a | b".to_owned(),
        }];
        let md = render_gaps_markdown("Daily Johan", &gaps);
        assert!(md.contains("Daily Johan"));
        assert!(md.contains("message_deleted"));
        assert!(md.contains("a \\| b"), "pipes must be escaped");
        assert!(md.contains("| 2 | m2 |"));
    }

    #[test]
    fn gap_reason_strings_are_stable() {
        assert_eq!(GapReason::MessageDeleted.as_str(), "message_deleted");
        assert_eq!(GapReason::MediaUnfetchable.as_str(), "media_unfetchable");
        assert_eq!(GapReason::FetchDeferred.as_str(), "fetch_deferred");
        assert_eq!(GapReason::NoMediaRecovered.as_str(), "no_media_recovered");
    }
}
