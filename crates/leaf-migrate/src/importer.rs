//! The migration orchestrator: walks the source archive and writes a leaf
//! series, its posts, and media into the target database.
//!
//! Two entry points: [`plan`] (the `--dry-run` read-only planner) and [`run`]
//! (the real import). [`run`] is **idempotent** — every day is committed in
//! its own [`PostRepo::insert_with_media`] transaction, already-present days
//! are skipped, and days whose source message could not be fetched are
//! *deferred* (left unwritten) rather than recorded as missing. That makes
//! re-running the natural resume mechanism: kill it, run it again, and it
//! continues where it stopped and retries anything transient.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use anyhow::Context as _;
use leaf_core::db::{DbError, GuildSettingsRepo, PostRepo, SeriesRepo};
use leaf_core::domain::{
    Cadence, DetectionMode, NewMediaAttachment, NewSeries, Post, Privacy, Series, SeriesState,
};
use leaf_core::media::MediaPipeline;
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
    /// Days left unwritten after a transient fetch error (retry on re-run).
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
    let existing_days: BTreeSet<i64> = match &existing {
        Some(s) => post_repo.all_days(s.id).await?.into_iter().collect(),
        None => BTreeSet::new(),
    };

    let mut summary = Summary {
        series_id,
        total_source: source.len(),
        ..Summary::default()
    };
    for p in source {
        let day = mapping::leaf_day(p.day, cfg.day_offset);
        if existing_days.contains(&day) {
            summary.skipped_existing += 1;
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
/// not-yet-present day.
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
            summary.skipped_existing += 1;
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

impl<S: MessageSource + Sync> Run<'_, S> {
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
                summary.skipped_existing += 1;
                Ok(())
            }
            Err(e) => Err(anyhow::Error::new(e)).with_context(|| format!("inserting day {day}")),
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
