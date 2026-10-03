//! Archive to Series: the core write path, as a message context menu.
//!
//! The flow, start to finish:
//!
//! 1. Cheap checks before anything is shown: setup, a series to archive
//!    into, a watched channel, files leaf can store, "already archived".
//! 2. The series is settled without typing: the creator's only series, or
//!    the one series bound to the message's channel, otherwise a button per
//!    series (a select when there are more than five).
//! 3. A modal asks for the day, pre-filled with that series' own next day
//!    (or the day named in the post's text).
//! 4. The submit is answered at once with a loading state, the files are
//!    stored a few at a time, the day is written, and the answer is edited
//!    into the result.
//! 5. The result and every recoverable problem carry buttons, so a slip is
//!    fixed in place instead of starting over from the long-press.
//!
//! Storage keys are a function of guild, series, day and attachment id, so
//! two attempts at the same message write the same objects. Two rules keep
//! that safe: one attempt at a time per message and per day (`Inflight`),
//! and no object is deleted unless the database says no entry points at it
//! (`release`).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use leaf_core::db::{DbError, DbResult, LaunchIntentRepo, PostRepo};
use leaf_core::domain::{GuildSettings, NewMediaAttachment, Post, Privacy, Series, SeriesState};
use leaf_core::media::{
    self, ArchivedMedia, DEFAULT_MAX_BYTES, MediaError, MediaMeta, MediaPipeline,
};
use leaf_core::milestone::{self, Milestone};
use leaf_core::{parser, policy, series_ops};
use poise::serenity_prelude as serenity;
use serenity::Builder as _;
use serenity::futures::{Stream, StreamExt as _};
use tokio::sync::Semaphore;

use crate::components::{self, FlightKey, INFLIGHT, Inflight, NONCE, OPEN_GALLERY_FALLBACK};
use crate::{Context, Data, Error, checks};

/// Attachments of one post stored at the same time.
const UPLOAD_CONCURRENCY: usize = 3;

/// How long the prompts of one archive stay live after the last tap. An
/// interaction token can edit its message for 15 minutes; this stays inside.
const SESSION_IDLE: Duration = Duration::from_mins(14);

/// How long Discord's loading state stands alone before a line of text
/// says what leaf is doing.
const SLOW_NOTICE_AFTER: Duration = Duration::from_millis(2500);

/// A day this many days (or more) past the next expected one is confirmed
/// before it is archived: it is more often a typo than a real jump.
const GAP_CONFIRM_DAYS: i64 = 7;

/// Milestones are announced only for posts this recent, so catching up on
/// an old series stays quiet.
const MILESTONE_MAX_AGE_SECS: i64 = 48 * 3600;

/// Discord's limit for a modal title and for a text input label.
const MODAL_TEXT_MAX_CHARS: usize = 45;

/// Share of the modal title given to the author's name.
const TITLE_AUTHOR_MAX_CHARS: usize = 14;

/// Room for a day with decoration ("Day 123456"); the range is enforced by
/// [`parser::parse_day_input`].
const DAY_INPUT_MAX_CHARS: u16 = 12;

/// Custom id of the modal's only text input.
const DAY_INPUT_ID: &str = "day";

/// Discord's 2000-character message limit, less a margin.
const MESSAGE_MAX_CHARS: usize = 1900;

/// Discord's limit for options in one select menu.
const SELECT_MAX_OPTIONS: usize = 25;

/// Discord's limit for a select option label.
const OPTION_LABEL_MAX_CHARS: usize = 100;

/// Discord's limit for a button label.
const BUTTON_LABEL_MAX_CHARS: usize = 80;

/// Up to this many series are offered as buttons (one row) rather than a
/// select. A button sends a press every time; a select sends nothing when
/// the series already showing in it is picked again, which is what someone
/// does after closing the day modal without submitting it.
const PICK_BUTTONS_MAX: usize = 5;

/// Skipped-file notes listed in one message before "and N more".
const NOTES_SHOWN_MAX: usize = 5;

/// Channels named in the "not a series channel" refusal.
const CHANNELS_SHOWN_MAX: usize = 10;

/// Reaction used when the series' own emoji cannot be.
const FALLBACK_REACTION: &str = "🍃";

/// Discord error: the emoji is not one Discord knows.
const UNKNOWN_EMOJI: isize = 10014;
/// Discord error: the bot cannot see the channel.
const MISSING_ACCESS: isize = 50001;
/// Discord error: the bot lacks a permission.
const MISSING_PERMISSIONS: isize = 50013;

/// First segment of this module's component ids. Deliberately not `leaf:`,
/// which belongs to the stateless buttons a global router answers.
const ID_PREFIX: &str = "arc";

/// Shown when an archive step fails for a reason the user cannot fix.
const GENERIC_FAILURE: &str = "🍂 Something went wrong on my end, so nothing was archived. \
     It's been logged. Try again in a moment.";

/// Shown when a follow-up step (a button, a prompt) fails.
const STEP_FAILURE: &str =
    "🍂 Something went wrong on my end. It's been logged. Try again in a moment.";

// ---------------------------------------------------------------------------
// Context-free archive core
// ---------------------------------------------------------------------------

/// Where files go. The pipeline in production; a fake in tests, so the
/// rules about what may be deleted are tested without object storage.
trait MediaSink: Clone + Send + Sync + 'static {
    /// Downloads `url` and stores the original and its thumbnail.
    fn store(
        &self,
        url: &str,
        meta: &MediaMeta,
    ) -> impl Future<Output = Result<ArchivedMedia, MediaError>> + Send;

    /// Removes stored objects, best effort.
    fn delete(&self, keys: &[String]) -> impl Future<Output = ()> + Send;
}

impl MediaSink for MediaPipeline {
    async fn store(&self, url: &str, meta: &MediaMeta) -> Result<ArchivedMedia, MediaError> {
        self.archive_from_url_detailed(url, meta).await
    }

    async fn delete(&self, keys: &[String]) {
        self.delete_keys(keys).await;
    }
}

/// One file of the source message that leaf can archive.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFile {
    attachment_id: String,
    filename: String,
    url: String,
    /// As worked out by [`media::content_type_for`], never empty.
    content_type: &'static str,
}

/// What an archive needs to know about the message, detached from serenity
/// so the core runs without a Discord context.
#[derive(Debug, Clone)]
struct SourcePost {
    guild_id: String,
    message_id: String,
    channel_id: String,
    caption: String,
    /// Unix seconds the message was posted.
    posted_at: i64,
    /// In attachment order.
    files: Vec<SourceFile>,
}

/// Whether a taken day stops the archive or gives way to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Archive into a free day.
    Insert,
    /// Take over a day another post holds.
    Replace,
}

/// One archive attempt.
#[derive(Debug, Clone, Copy)]
struct ArchiveRequest<'a> {
    series_id: i64,
    day: i64,
    source: &'a SourcePost,
    mode: Mode,
    /// Unix seconds, recorded as `archived_at`.
    now: i64,
}

/// What a finished archive stored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Archived {
    files: usize,
    /// One sentence per file that was left out.
    skipped: Vec<String>,
    /// Files whose thumbnail is the generic placeholder.
    placeholder_thumbs: usize,
    /// The entry of another post this one took the place of (Replace only).
    replaced: Option<Post>,
}

/// Why nothing was archived.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    /// One sentence per file involved.
    notes: Vec<String>,
    /// Whether the same attempt can succeed later.
    retryable: bool,
    /// Whether leaf's own storage failed (worth an admin's attention).
    storage: bool,
}

/// Which lock another attempt holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    /// This message is being archived right now.
    ThisPost,
    /// Another message is being archived into this day right now.
    ThisDay,
}

/// How an archive attempt ended.
#[derive(Debug)]
enum ArchiveOutcome {
    /// The day is written.
    Archived(Archived),
    /// This message already holds the day: nothing to do.
    AlreadyArchived,
    /// Another message holds the day.
    DayTaken(Post),
    /// Another attempt is running.
    Busy(Busy),
    /// Nothing was written.
    Failed(Failure),
}

/// Result of [`upload_all`].
#[derive(Debug, Default)]
struct Uploads {
    /// Rows for the files that were stored, in attachment order.
    rows: Vec<NewMediaAttachment>,
    /// Every key an upload may have written, stored or not.
    touched: Vec<String>,
    skipped: Vec<String>,
    placeholder_thumbs: usize,
    /// Set when the whole post was given up on.
    aborted: Option<Failure>,
}

/// Stores a post's files and writes the day. Needs no Discord context, so
/// the modal submit and every button share it.
///
/// Holds the in-flight guard from the first check until the last cleanup.
/// Nothing is removed from storage unless [`release`] finds it unreferenced.
async fn archive_core<S: MediaSink>(
    posts: &PostRepo,
    sink: &S,
    inflight: &Inflight,
    req: &ArchiveRequest<'_>,
) -> DbResult<ArchiveOutcome> {
    let keys = [
        FlightKey::Message(req.series_id, req.source.message_id.clone()),
        FlightKey::Day(req.series_id, req.day),
    ];
    let _guard = match inflight.try_begin(&keys) {
        Ok(guard) => guard,
        Err(FlightKey::Message(..)) => return Ok(ArchiveOutcome::Busy(Busy::ThisPost)),
        Err(FlightKey::Day(..)) => return Ok(ArchiveOutcome::Busy(Busy::ThisDay)),
    };

    let holder = posts
        .get(req.series_id, req.day)
        .await?
        .map(|(post, _)| post);
    if let Some(existing) = &holder {
        if existing.message_id == req.source.message_id {
            return Ok(ArchiveOutcome::AlreadyArchived);
        }
        if req.mode == Mode::Insert {
            return Ok(ArchiveOutcome::DayTaken(existing.clone()));
        }
    }
    // A replace also takes the entry it removes. That entry's stored files
    // are named after the day its message was first archived as, so an
    // archive of that same message running right now may be writing the
    // very objects this replace is about to find unused and delete.
    let _replaced_guard = match &holder {
        Some(existing) => {
            let key = FlightKey::Message(req.series_id, existing.message_id.clone());
            match inflight.try_begin(std::slice::from_ref(&key)) {
                Ok(guard) => Some(guard),
                Err(_) => return Ok(ArchiveOutcome::Busy(Busy::ThisDay)),
            }
        }
        None => None,
    };

    let uploads = upload_all(sink, req).await;
    if let Some(failure) = uploads.aborted {
        release(posts, sink, &uploads.touched).await;
        return Ok(ArchiveOutcome::Failed(failure));
    }
    if uploads.rows.is_empty() {
        release(posts, sink, &uploads.touched).await;
        return Ok(ArchiveOutcome::Failed(Failure {
            notes: uploads.skipped,
            retryable: false,
            storage: false,
        }));
    }

    let post = Post {
        series_id: req.series_id,
        day: req.day,
        message_id: req.source.message_id.clone(),
        channel_id: req.source.channel_id.clone(),
        caption: req.source.caption.clone(),
        posted_at: req.source.posted_at,
        archived_at: req.now,
    };
    // Replace uploads first and swaps the rows in one transaction, so a
    // failed upload never costs the entry that was there.
    let written = match req.mode {
        Mode::Insert => posts
            .insert_with_media(&post, &uploads.rows)
            .await
            .map(|()| Vec::new()),
        Mode::Replace => posts.replace_with_media(&post, &uploads.rows).await,
    };
    match written {
        Ok(replaced) => {
            // `replaced` is already narrowed to keys no row holds.
            sink.delete(&replaced).await;
            // A skipped file can leave an object behind.
            let leftovers: Vec<String> = uploads
                .touched
                .into_iter()
                .filter(|key| !holds_key(&uploads.rows, key))
                .collect();
            release(posts, sink, &leftovers).await;
            Ok(ArchiveOutcome::Archived(Archived {
                files: uploads.rows.len(),
                skipped: uploads.skipped,
                placeholder_thumbs: uploads.placeholder_thumbs,
                replaced: holder,
            }))
        }
        Err(DbError::DuplicateDay(_)) => {
            // The day was written by a path that does not take the guard
            // (an import, another process) while this one uploaded.
            release(posts, sink, &uploads.touched).await;
            Ok(match posts.get(req.series_id, req.day).await? {
                Some((existing, _)) if existing.message_id == req.source.message_id => {
                    ArchiveOutcome::AlreadyArchived
                }
                Some((existing, _)) => ArchiveOutcome::DayTaken(existing),
                None => ArchiveOutcome::Failed(Failure {
                    notes: Vec::new(),
                    retryable: true,
                    storage: false,
                }),
            })
        }
        Err(e) => {
            release(posts, sink, &uploads.touched).await;
            Err(e)
        }
    }
}

/// True when one of `rows` stores `key` as its original or thumbnail.
fn holds_key(rows: &[NewMediaAttachment], key: &str) -> bool {
    rows.iter().any(|row| {
        row.original_key.as_deref() == Some(key) || row.thumb_key.as_deref() == Some(key)
    })
}

/// Stores every file of the post, [`UPLOAD_CONCURRENCY`] at a time.
///
/// Each file is its own task; the handles are awaited in attachment order
/// because media rows are read back in insertion order, so finishing order
/// must not decide how a multi-image day is displayed.
///
/// A file that can never be archived (too large, unsupported, gone) is
/// skipped. A failure that may pass (download, storage) gives up on the
/// whole post, so a day is never silently archived with files missing.
async fn upload_all<S: MediaSink>(sink: &S, req: &ArchiveRequest<'_>) -> Uploads {
    let permits = Arc::new(Semaphore::new(UPLOAD_CONCURRENCY));
    let mut uploads = Uploads::default();
    let mut pending = Vec::with_capacity(req.source.files.len());

    for file in &req.source.files {
        let meta = MediaMeta {
            guild_id: req.source.guild_id.clone(),
            series_id: req.series_id,
            day: req.day,
            attachment_id: file.attachment_id.clone(),
            content_type: file.content_type.to_owned(),
        };
        // A failed upload can still leave either object behind.
        uploads.touched.push(media::original_key(&meta));
        uploads.touched.push(media::thumb_key(&meta));

        let (sink, permits) = (sink.clone(), Arc::clone(&permits));
        let (url, task_meta) = (file.url.clone(), meta.clone());
        let handle = tokio::spawn(async move {
            // Errors only once the semaphore is closed, which never happens.
            let _permit = permits.acquire_owned().await;
            sink.store(&url, &task_meta).await
        });
        pending.push((file, meta, handle));
    }

    for (file, meta, handle) in pending {
        if uploads.aborted.is_some() {
            // Stop the rest, and wait until it has stopped: the cleanup
            // that follows must not race a write still in progress.
            handle.abort();
            if let Err(e) = handle.await
                && e.is_panic()
            {
                tracing::error!(file = %file.filename, "upload task panicked");
            }
            continue;
        }
        match handle.await {
            Ok(Ok(archived)) => {
                uploads.placeholder_thumbs += usize::from(archived.placeholder_thumb);
                uploads.rows.push(NewMediaAttachment {
                    attachment_id: meta.attachment_id,
                    channel_id: req.source.channel_id.clone(),
                    message_id: req.source.message_id.clone(),
                    content_type: meta.content_type,
                    original_key: Some(archived.stored.original_key),
                    thumb_key: Some(archived.stored.thumb_key),
                    media_missing: false,
                });
            }
            Ok(Err(e)) => {
                // The full error is for the operator only: its text can
                // name the storage endpoint and bucket.
                tracing::error!(
                    series = req.series_id,
                    day = req.day,
                    file = %file.filename,
                    error = %e,
                    "archiving an attachment failed"
                );
                if e.is_retryable() {
                    uploads.aborted = Some(Failure {
                        notes: vec![e.user_message(&file.filename)],
                        retryable: true,
                        storage: matches!(e, MediaError::Store(_) | MediaError::Io(_)),
                    });
                } else {
                    uploads.skipped.push(e.user_message(&file.filename));
                }
            }
            Err(e) => {
                tracing::error!(
                    series = req.series_id,
                    day = req.day,
                    file = %file.filename,
                    error = %e,
                    "upload task failed"
                );
                uploads.aborted = Some(Failure {
                    notes: Vec::new(),
                    retryable: true,
                    storage: false,
                });
            }
        }
    }
    uploads
}

/// Removes from storage the `keys` no archived entry points at.
///
/// Every cleanup goes through here. Keys are deterministic and a moved day
/// keeps the keys of its old number, so "this attempt uploaded it" never
/// proves "nothing else needs it". If the database cannot answer, the
/// objects stay: an orphan costs storage, a wrong delete costs an archive.
async fn release<S: MediaSink>(posts: &PostRepo, sink: &S, keys: &[String]) {
    if keys.is_empty() {
        return;
    }
    match posts.unreferenced_keys(keys).await {
        Ok(free) => sink.delete(&free).await,
        Err(e) => tracing::warn!(
            error = %e,
            count = keys.len(),
            "could not check which stored objects are unreferenced; leaving them in place"
        ),
    }
}

/// How removing an entry ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// The entry and its files are gone.
    Removed,
    /// The day is not (or no longer) this message's entry: nothing done.
    NotThisPost,
    /// An archive touching this message or day is running.
    Busy,
}

/// Removes the entry `message_id` holds at `day`, and its files. Refuses
/// when the day now belongs to another message, so an Undo pressed late
/// cannot delete an entry archived there since.
async fn remove_core<S: MediaSink>(
    posts: &PostRepo,
    sink: &S,
    inflight: &Inflight,
    series_id: i64,
    day: i64,
    message_id: &str,
) -> DbResult<Removal> {
    let keys = [
        FlightKey::Message(series_id, message_id.to_owned()),
        FlightKey::Day(series_id, day),
    ];
    let Ok(_guard) = inflight.try_begin(&keys) else {
        return Ok(Removal::Busy);
    };
    match posts.get(series_id, day).await? {
        Some((post, _)) if post.message_id == message_id => {}
        _ => return Ok(Removal::NotThisPost),
    }
    // `delete` returns only the keys no other entry holds.
    let freed = posts.delete(series_id, day).await?;
    sink.delete(&freed).await;
    Ok(Removal::Removed)
}

/// How renumbering an entry ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    /// The entry now sits at the new day.
    Moved,
    /// The new day is already archived.
    Taken,
    /// The old day is not (or no longer) this message's entry.
    NotThisPost,
    /// An archive touching this message or either day is running.
    Busy,
}

/// Renumbers the entry `message_id` holds from `from` to `to`, keeping its
/// stored files.
async fn move_core(
    posts: &PostRepo,
    inflight: &Inflight,
    series_id: i64,
    (from, to): (i64, i64),
    message_id: &str,
) -> DbResult<Move> {
    let keys = [
        FlightKey::Message(series_id, message_id.to_owned()),
        FlightKey::Day(series_id, from),
        FlightKey::Day(series_id, to),
    ];
    let Ok(_guard) = inflight.try_begin(&keys) else {
        return Ok(Move::Busy);
    };
    match posts.get(series_id, from).await? {
        Some((post, _)) if post.message_id == message_id => {}
        _ => return Ok(Move::NotThisPost),
    }
    match posts.move_day(series_id, from, to).await {
        Ok(()) => Ok(Move::Moved),
        Err(DbError::DuplicateDay(_)) => Ok(Move::Taken),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Reading the message
// ---------------------------------------------------------------------------

/// The content type to archive a file under, or the sentence explaining
/// why leaf leaves it out. Checked before the modal so a file that can
/// never work costs the creator nothing.
fn classify_file(
    filename: &str,
    declared: Option<&str>,
    size: u64,
) -> Result<&'static str, String> {
    let Some(content_type) = media::content_type_for(filename, declared) else {
        return Err(media::unsupported_message(filename));
    };
    if size > DEFAULT_MAX_BYTES {
        let too_large = MediaError::TooLarge {
            limit_mb: DEFAULT_MAX_BYTES / (1024 * 1024),
        };
        return Err(too_large.user_message(filename));
    }
    Ok(content_type)
}

/// Splits attachments into the files leaf can archive and a note per file
/// it cannot.
fn pick_files(attachments: &[serenity::Attachment]) -> (Vec<SourceFile>, Vec<String>) {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for att in attachments {
        match classify_file(
            &att.filename,
            att.content_type.as_deref(),
            u64::from(att.size),
        ) {
            Ok(content_type) => files.push(SourceFile {
                attachment_id: att.id.to_string(),
                filename: att.filename.clone(),
                url: att.url.clone(),
                content_type,
            }),
            Err(note) => skipped.push(note),
        }
    }
    (files, skipped)
}

/// What a message offers to archive.
#[derive(Debug)]
struct Gathered {
    files: Vec<SourceFile>,
    skipped: Vec<String>,
    caption: String,
    /// The message shows a link preview or GIF (which leaf does not store).
    has_embeds: bool,
}

/// Reads the message's own attachments, falling back to the forwarded
/// message it carries: a forward keeps its files in `message_snapshots`.
fn gather(msg: &serenity::Message) -> Gathered {
    let (mut files, mut skipped) = pick_files(&msg.attachments);
    let mut caption = msg.content.clone();
    let snapshot = msg.message_snapshots.first();
    if files.is_empty()
        && let Some(snapshot) = snapshot
    {
        let (forwarded, forwarded_skipped) = pick_files(&snapshot.attachments);
        files = forwarded;
        skipped.extend(forwarded_skipped);
        if caption.trim().is_empty() {
            caption.clone_from(&snapshot.content);
        }
    }
    Gathered {
        files,
        skipped,
        caption,
        has_embeds: !msg.embeds.is_empty() || snapshot.is_some_and(|s| !s.embeds.is_empty()),
    }
}

/// The channel a thread hangs off, when the command ran inside a thread.
fn thread_parent(channel: Option<&serenity::PartialChannel>) -> Option<String> {
    let channel = channel?;
    matches!(
        channel.kind,
        serenity::ChannelType::PublicThread
            | serenity::ChannelType::PrivateThread
            | serenity::ChannelType::NewsThread
    )
    .then_some(channel.parent_id)
    .flatten()
    .map(|id| id.to_string())
}

/// True when the message's channel, or the channel its thread hangs off,
/// is one the server archives from.
fn channel_watched(settings: &GuildSettings, channel_id: &str, parent: Option<&str>) -> bool {
    policy::channel_allowed(settings, channel_id)
        || parent.is_some_and(|parent| policy::channel_allowed(settings, parent))
}

/// The series a post goes into when no question is needed: the creator's
/// only series, or the one series bound to the post's channel (or to the
/// channel its thread hangs off). `None` means ask.
fn resolve_target<'a>(
    series: &'a [Series],
    channel_id: &str,
    parent: Option<&str>,
) -> Option<&'a Series> {
    if let [only] = series {
        return Some(only);
    }
    let mut bound = series.iter().filter(|s| {
        s.channels
            .iter()
            .any(|c| c == channel_id || Some(c.as_str()) == parent)
    });
    let first = bound.next()?;
    bound.next().is_none().then_some(first)
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/// `text` cut to `max` characters, ending in an ellipsis when cut.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// `content` kept inside Discord's message limit.
fn clamp_message(content: &str) -> String {
    truncate_chars(content, MESSAGE_MAX_CHARS)
}

/// "1 file" / "3 files".
fn files_phrase(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

/// Link that jumps to a message.
fn jump_link(guild_id: &str, channel_id: &str, message_id: &str) -> String {
    format!("https://discord.com/channels/{guild_id}/{channel_id}/{message_id}")
}

/// The modal title: where the post is going and, when it is someone
/// else's, whose it is. Plain text only (mentions do not render there).
fn modal_title(series_name: &str, other_author: Option<&str>) -> String {
    let title = other_author.map_or_else(
        || format!("Archive to {series_name}"),
        |author| {
            format!(
                "{}'s post to {series_name}",
                truncate_chars(author, TITLE_AUTHOR_MAX_CHARS)
            )
        },
    );
    truncate_chars(&title, MODAL_TEXT_MAX_CHARS)
}

/// What the day field's label says about its pre-filled value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DayHint {
    /// The series' next day.
    Next,
    /// The day named in the post's text.
    FromText,
    /// The post's text names this day, which is already archived; the
    /// value is the next free one.
    Taken(i64),
    /// Renumbering an entry that sits at this day.
    Current(i64),
}

/// Label of the day field.
fn day_label(hint: DayHint, value: i64) -> String {
    let label = match hint {
        DayHint::Next => format!("Day number (next is {value})"),
        DayHint::FromText => "Day number (from the post's text)".to_owned(),
        DayHint::Taken(wanted) => format!("Day number (Day {wanted} is taken)"),
        DayHint::Current(now) => format!("New day number (now Day {now})"),
    };
    truncate_chars(&label, MODAL_TEXT_MAX_CHARS)
}

/// What a typed day is for, which decides how the gap question is worded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DayUse {
    /// Archiving the post as that day.
    Archive,
    /// Renumbering the post's entry to that day.
    Move,
}

/// The question to ask before using a day far from the expected one, or
/// `None` when the day is plausible. `max_day` is the highest day of the
/// series, leaving out the entry being renumbered.
fn gap_warning(series: &Series, max_day: Option<i64>, day: i64, using: DayUse) -> Option<String> {
    let name = &series_ops::display_name(&series.name);
    let question = match using {
        DayUse::Archive => format!("Archive this post as Day {day} anyway?"),
        DayUse::Move => format!("Change this post to Day {day} anyway?"),
    };
    if day < series.start_day {
        return Some(format!(
            "Day {day} is before Day {}, the first day of **{name}**. {question}",
            series.start_day
        ));
    }
    let expected = max_day.map_or(series.start_day, |max| max.saturating_add(1));
    let skipped = day.saturating_sub(expected);
    if skipped < GAP_CONFIRM_DAYS {
        return None;
    }
    let reason = max_day.map_or_else(
        || {
            let days = match using {
                DayUse::Archive => "no days yet",
                DayUse::Move => "no other days",
            };
            format!(
                "**{name}** has {days} and starts at Day {}.",
                series.start_day
            )
        },
        |max| format!("Day {day} would skip {skipped} days after Day {max} of **{name}**."),
    );
    Some(format!("{reason} {question}"))
}

/// The line shown while files are being stored.
fn progress_text(series_name: &str, day: i64, files: &[SourceFile]) -> String {
    let patience = if files.iter().any(|f| f.content_type.starts_with("video/")) {
        " Videos can take a minute."
    } else {
        ""
    };
    format!(
        "🍃 Archiving Day {day} of **{series_name}** ({})…{patience}",
        files_phrase(files.len())
    )
}

/// Another member's display name for message content. It is free text
/// someone else chose, so it is escaped: it shows as typed, and cannot put
/// a masked link or formatting into leaf's reply. (A modal title shows text
/// literally and takes the name as it is.)
fn author_shown(author: &str) -> String {
    series_ops::display_name(author)
}

/// The question above the series picker. `other_author` is the post's
/// author when that is not the invoker, as Discord gives the name.
fn pick_series_text(other_author: Option<&str>) -> String {
    other_author.map_or_else(
        || "Which series is this post for?".to_owned(),
        |author| format!("Which series is {}'s post for?", author_shown(author)),
    )
}

/// First line of the result. `series_name` is already escaped for message
/// content; `other_author` is the name as Discord gives it.
fn archived_line(
    series_name: &str,
    day: i64,
    files: usize,
    other_author: Option<&str>,
    mode: Mode,
) -> String {
    let files = files_phrase(files);
    let other_author = other_author.map(author_shown);
    match (mode, other_author) {
        (Mode::Insert, None) => format!("🍃 Day {day} of **{series_name}** archived ({files})."),
        (Mode::Insert, Some(author)) => {
            format!("🍃 Day {day} of **{series_name}** archived from {author}'s post ({files}).")
        }
        (Mode::Replace, None) => {
            format!("🍃 Day {day} of **{series_name}** replaced with this post ({files}).")
        }
        (Mode::Replace, Some(author)) => {
            format!("🍃 Day {day} of **{series_name}** replaced with {author}'s post ({files}).")
        }
    }
}

/// One line per skipped file, capped so a long list cannot flood the reply.
fn skipped_lines(notes: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = notes
        .iter()
        .take(NOTES_SHOWN_MAX)
        .map(|note| format!("Skipped: {note}"))
        .collect();
    if notes.len() > NOTES_SHOWN_MAX {
        lines.push(format!(
            "…and {} more skipped.",
            notes.len() - NOTES_SHOWN_MAX
        ));
    }
    lines
}

/// The reply when the invoker has no series that takes posts.
fn no_series_text(revoked: &[Series]) -> String {
    match revoked {
        [] => "🌱 You don't have a series yet. Open the gallery to start one, \
               then archive this post again."
            .to_owned(),
        [one] => format!(
            "🍂 A server admin revoked **{}**, so it can't take new posts. Your archive is \
             kept: ask an admin to restore it. You can also start a new series in the gallery.",
            series_ops::display_name(&one.name)
        ),
        many => format!(
            "🍂 A server admin revoked your series ({}), so they can't take new posts. Your \
             archives are kept: ask an admin to restore them. You can also start a new series \
             in the gallery.",
            many.iter()
                .map(|s| format!("**{}**", series_ops::display_name(&s.name)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The refusal for a message outside the server's series channels.
fn not_watched_text(watched: &[String], is_admin: bool) -> String {
    if watched.is_empty() {
        let fix = if is_admin {
            "Pick them with `/setup`, then archive this post again."
        } else {
            "Ask a server admin to pick them with `/setup`."
        };
        return format!("🍂 This server has no series channels yet, so leaf can't archive. {fix}");
    }
    let mut channels = watched
        .iter()
        .take(CHANNELS_SHOWN_MAX)
        .map(|id| format!("<#{id}>"))
        .collect::<Vec<_>>()
        .join(", ");
    if watched.len() > CHANNELS_SHOWN_MAX {
        channels.push_str(", …");
    }
    let fix = if is_admin {
        "Post it there, or add this channel with `/setup`."
    } else {
        "Post it there, or ask a server admin to add this channel with `/setup`."
    };
    format!(
        "🍂 leaf doesn't archive from this channel. This server's series channels: {channels}. \
         {fix}"
    )
}

/// The refusal for a message with nothing leaf can store.
fn nothing_to_archive_text(skipped: &[String], has_embeds: bool) -> String {
    if !skipped.is_empty() {
        let mut lines = vec!["🍂 Nothing in that message can be archived.".to_owned()];
        lines.extend(skipped_lines(skipped));
        return lines.join("\n");
    }
    if has_embeds {
        return "🍂 That message has a link preview or GIF, not an uploaded file. leaf archives \
                uploaded images and videos: post the file itself, then archive that message."
            .to_owned();
    }
    format!(
        "🍂 That message has no image or video to archive. leaf stores uploaded files ({}).",
        media::SUPPORTED_FORMATS
    )
}

/// "Day 42 of **X**" for each entry, joined for a sentence.
fn entries_phrase(entries: &[(String, i64)]) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(name, day)| format!("Day {day} of **{name}**"))
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// The notice for a message that is already in the creator's archive.
/// `unchanged` is set when it follows an attempt that turned out to be a
/// repeat.
fn existing_text(entries: &[(String, i64)], unchanged: bool) -> String {
    let lead = if unchanged {
        format!(
            "🍃 This post is already {}, so nothing changed.",
            entries_phrase(entries)
        )
    } else {
        format!("🍃 This post is already {}.", entries_phrase(entries))
    };
    let next = if entries.len() == 1 {
        "You can change its day, archive it as another day as well, or remove this entry."
    } else {
        "You can archive it as another day. To remove an entry, use Remove Archive Entry on \
         the message."
    };
    format!("{lead}\n{next}")
}

/// The reply when another post holds the day.
fn duplicate_text(series_name: &str, day: i64, holder_link: &str) -> String {
    format!(
        "🍂 Day {day} of **{series_name}** is already archived from \
         [another post]({holder_link}).\nUse the next free day, pick another day, or replace \
         Day {day} with this post. Replacing deletes the files stored for the other post."
    )
}

/// The notice shown instead of the modal while an earlier try at the same
/// post is still storing its files.
const STILL_ARCHIVING: &str = "⏳ leaf is still archiving this post from an earlier try. \
     Wait for that result, or check again in a moment.";

/// The reply when another attempt is still running.
fn busy_text(busy: Busy, series_name: &str, day: i64) -> String {
    match busy {
        Busy::ThisPost => "⏳ leaf is still archiving this post from an earlier try. \
                           Give it a moment, then try again."
            .to_owned(),
        Busy::ThisDay => format!(
            "⏳ Another post is being archived as Day {day} of **{series_name}** right now. \
             Try again in a moment, or pick another day."
        ),
    }
}

/// The reply when nothing was archived.
fn failure_text(failure: &Failure, pre_skipped: &[String]) -> String {
    if failure.retryable {
        return failure.notes.first().map_or_else(
            || {
                "🍂 Something interrupted the archive, so nothing was archived. Try again."
                    .to_owned()
            },
            |note| format!("🍂 {note}\nNothing was archived."),
        );
    }
    let notes: Vec<String> = pre_skipped.iter().chain(&failure.notes).cloned().collect();
    let mut lines =
        vec!["🍂 Nothing was archived: no file in that post could be stored.".to_owned()];
    lines.extend(skipped_lines(&notes));
    lines.join("\n")
}

/// Whether `id` has the shape of a Discord id: digits only.
fn is_snowflake(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// Who sees a series once it is out of its sprout stage.
fn promotion_line(series: &Series) -> String {
    let name = &series_ops::display_name(&series.name);
    match (series.privacy, series.privacy_role_id.as_deref()) {
        (Privacy::Public, _) => format!(
            "🌿 **{name}** is out of its sprout stage: everyone in this server can now see it \
             in the gallery."
        ),
        // Only an id is written as a mention: a stored role that is not one
        // (from before roles were checked) must not reach the message.
        (Privacy::RoleGated, Some(role)) if is_snowflake(role) => format!(
            "🌿 **{name}** is out of its sprout stage: members with <@&{role}> can now see it \
             in the gallery."
        ),
        (Privacy::RoleGated, _) => format!(
            "🌿 **{name}** is out of its sprout stage: members with its role can now see it in \
             the gallery."
        ),
        (Privacy::CreatorOnly, _) => format!(
            "🌿 **{name}** is out of its sprout stage. Its privacy is \"Only me\", so other \
             members still can't see it."
        ),
    }
}

/// How far a sprout is from being listed.
fn sprout_progress_line(series_name: &str, archived: i64, threshold: i64) -> String {
    format!(
        "🌱 **{series_name}** is a sprout: {archived} of {threshold} days archived. Until then \
         only you can see it in the gallery (server admins see it in the admin panel and in \
         chat commands)."
    )
}

/// The milestone announcement posted in the channel.
///
/// It is public message content under leaf's own name, so the series name
/// goes in escaped: a name is free text, and unescaped it could put a
/// masked link or broken formatting into the bot's message. The creator is
/// a mention; the message is sent with mention parsing off.
fn announcement_text(series: &Series, reached: Milestone, day: i64) -> String {
    milestone::render(
        series.milestone_template.as_deref(),
        reached,
        day,
        &series_ops::display_name(&series.name),
        &format!("<@{}>", series.creator_id),
    )
}

/// The celebration kept in the private reply, for series that are not
/// announced in the channel.
fn private_milestone_line(milestone: Milestone, day: i64, series_name: &str) -> String {
    match milestone {
        Milestone::First => format!("🌱 **{series_name}** has begun: Day 1 is archived."),
        Milestone::Years(1) => format!("🎉 Day {day}: one year of **{series_name}**."),
        Milestone::Years(years) => format!("🎉 Day {day}: {years} years of **{series_name}**."),
        Milestone::Hundred(_) => format!("🎉 **{series_name}** reached Day {day}."),
    }
}

/// A line for the server's log channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogKind {
    Archived { files: usize },
    Replaced { files: usize },
    Removed,
    Moved { from: i64 },
    Sprouted,
    StorageFailed,
}

/// Renders a log line. The actor is a mention (sent with mention parsing
/// off, so it shows the name and pings nobody). A series members cannot see
/// (not public, or still a sprout) is not named: the log channel has its own
/// audience.
fn log_text(
    kind: LogKind,
    series: &Series,
    day: i64,
    actor: serenity::UserId,
    link: &str,
) -> String {
    let subject = if series.privacy == Privacy::Public && series.state == SeriesState::Active {
        format!("**{}**", series_ops::display_name(&series.name))
    } else {
        "A private series".to_owned()
    };
    match kind {
        LogKind::Archived { files } => format!(
            "🍃 {subject} · Day {day} archived by <@{actor}> · {} · {link}",
            files_phrase(files)
        ),
        LogKind::Replaced { files } => format!(
            "🍃 {subject} · Day {day} replaced by <@{actor}> · {} · {link}",
            files_phrase(files)
        ),
        LogKind::Removed => format!("🗑️ {subject} · Day {day} removed by <@{actor}>"),
        LogKind::Moved { from } => {
            format!("✏️ {subject} · Day {from} renumbered to Day {day} by <@{actor}>")
        }
        LogKind::Sprouted => format!("🌿 {subject} · out of its sprout stage"),
        LogKind::StorageFailed => format!(
            "⚠️ leaf's storage failed while <@{actor}> was archiving a post, so nothing was \
             saved. Check the storage settings and leaf's logs."
        ),
    }
}

/// The reaction for a series: its whole emoji string (flags, skin tones
/// and joined emoji are several code points), or the leaf when unset.
fn reaction_for(emoji: &str) -> serenity::ReactionType {
    let emoji = emoji.trim();
    let emoji = if emoji.is_empty() {
        FALLBACK_REACTION
    } else {
        emoji
    };
    serenity::ReactionType::Unicode(emoji.to_owned())
}

/// The note for a reaction that could not be added for lack of permission.
fn reaction_permission_note(channel: serenity::ChannelId) -> String {
    format!(
        "I couldn't add the reaction: leaf needs Add Reactions and Read Message History in \
         <#{channel}>, which a server admin can grant. The post is archived all the same."
    )
}

/// Discord's JSON error code, when `error` is a refused request.
const fn discord_code(error: &serenity::Error) -> Option<isize> {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => {
            Some(response.error.code)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Component ids and controls
// ---------------------------------------------------------------------------

/// A component id that belongs to this process: `arc:<nonce>:<kind>:<parts>`
/// (the nonce is the one the component router compares against).
fn scoped_id(kind: &str, parts: &[&str]) -> String {
    let mut id = format!("{ID_PREFIX}:{}:{kind}", *NONCE);
    for part in parts {
        id.push(':');
        id.push_str(part);
    }
    id
}

/// What every component and modal id of one archive starts with.
fn session_prefix(session: u64) -> String {
    let mut prefix = scoped_id("archive", &[&session.to_string()]);
    prefix.push(':');
    prefix
}

/// Id of a button or select: the step it was drawn for, and what it does.
fn component_id(prefix: &str, generation: u32, action: Action) -> String {
    format!("{prefix}{generation}:{}", action.as_str())
}

/// Id of a modal. `opener` is the interaction that opened it, so each open
/// gets a fresh id (iOS caches a modal's contents by custom id).
fn modal_id(prefix: &str, opener: u64) -> String {
    format!("{prefix}m:{opener}")
}

/// Id of a series button in the picker: a pick that carries its series.
fn pick_id(prefix: &str, generation: u32, series_id: i64) -> String {
    format!(
        "{}:{series_id}",
        component_id(prefix, generation, Action::Pick)
    )
}

/// Reads a component id back into its step and action. Only a pick may
/// carry more (its series, read by [`picked_series`]).
fn parse_component_id(prefix: &str, custom_id: &str) -> Option<(u32, Action)> {
    let (generation, rest) = custom_id.strip_prefix(prefix)?.split_once(':')?;
    let (action, argument) = rest
        .split_once(':')
        .map_or((rest, None), |(action, argument)| (action, Some(argument)));
    let action = Action::parse(action)?;
    if argument.is_some() && action != Action::Pick {
        return None;
    }
    Some((generation.parse().ok()?, action))
}

/// The series a pick button carries.
fn pick_button_series(prefix: &str, custom_id: &str) -> Option<i64> {
    let (_, rest) = custom_id.strip_prefix(prefix)?.split_once(':')?;
    rest.strip_prefix(Action::Pick.as_str())?
        .strip_prefix(':')?
        .parse()
        .ok()
}

/// What a button or select does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Series chosen with a picker button or in the select.
    Pick,
    /// Open the day modal.
    Day,
    /// Archive at the next free day.
    Next,
    /// Replace the day's entry with this post.
    Replace,
    /// Archive despite the gap.
    Confirm,
    /// Run the same attempt again.
    Retry,
    /// Open the modal that renumbers the entry.
    Move,
    /// Remove the entry just archived.
    Undo,
    /// Open the gallery.
    Open,
    /// Archive an already archived post as another day.
    Another,
    /// Ask before removing an entry.
    Remove,
    /// Remove the entry.
    RemoveYes,
    /// Back out of removing.
    Keep,
    /// Look again whether an earlier try at this post has finished.
    Recheck,
}

impl Action {
    const ALL: [Self; 14] = [
        Self::Pick,
        Self::Day,
        Self::Next,
        Self::Replace,
        Self::Confirm,
        Self::Retry,
        Self::Move,
        Self::Undo,
        Self::Open,
        Self::Another,
        Self::Remove,
        Self::RemoveYes,
        Self::Keep,
        Self::Recheck,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Day => "day",
            Self::Next => "next",
            Self::Replace => "replace",
            Self::Confirm => "confirm",
            Self::Retry => "retry",
            Self::Move => "move",
            Self::Undo => "undo",
            Self::Open => "open",
            Self::Another => "another",
            Self::Remove => "remove",
            Self::RemoveYes => "remove-yes",
            Self::Keep => "keep",
            Self::Recheck => "recheck",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.as_str() == text)
    }
}

/// One option of the series select.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeriesOption {
    series_id: i64,
    name: String,
    next_day: i64,
}

/// What is on screen, with what its controls need to act.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    /// Nothing that takes a press: before the first reply, or while
    /// working, or after a final message.
    Idle,
    /// "You have no series" with a way into the gallery.
    NoSeries,
    /// The series select.
    PickSeries { options: Vec<SeriesOption> },
    /// The post is already archived as these `(series_id, day)` entries.
    Existing { entries: Vec<(i64, i64)> },
    /// Asking before an entry is removed.
    ConfirmRemove {
        series_id: i64,
        day: i64,
        /// The notice to return to.
        back: Vec<(i64, i64)>,
    },
    /// The typed day could not be read.
    DayProblem { series_id: i64 },
    /// The day is far from the expected one.
    Gap { series_id: i64, day: i64 },
    /// Another post holds the day.
    Duplicate {
        series_id: i64,
        day: i64,
        next_free: i64,
    },
    /// The attempt can be run again.
    Retry {
        series_id: i64,
        day: i64,
        mode: Mode,
        /// Also offer another day (the day itself is the obstacle).
        change_day: bool,
    },
    /// The post is archived.
    Done {
        series_id: i64,
        day: i64,
        /// Undo is offered for a fresh archive, not after a replace: it
        /// could not bring the replaced entry back.
        undo: bool,
        /// This archive took the series out of its sprout stage.
        promoted: bool,
    },
    /// Change day was given a day far from the series' others. The result
    /// it came from (`undo`, `promoted`) comes back whichever way it goes.
    MoveGap {
        series_id: i64,
        from: i64,
        to: i64,
        undo: bool,
        promoted: bool,
    },
    /// The entry was removed.
    Removed { series_id: i64 },
    /// An earlier try at this post is still running.
    StillArchiving,
}

/// A button, as data: what it does, what it says, how it looks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Control {
    action: Action,
    label: String,
    style: serenity::ButtonStyle,
}

/// The buttons of a stage. A pure function of the stage, so what is on
/// screen and what a press may do cannot drift apart.
fn controls(stage: &Stage) -> Vec<Control> {
    use serenity::ButtonStyle::{Danger, Primary, Secondary};
    let control = |action, label: &str, style| Control {
        action,
        label: label.to_owned(),
        style,
    };
    match stage {
        Stage::Idle | Stage::PickSeries { .. } => Vec::new(),
        Stage::NoSeries => vec![control(Action::Open, "Open gallery", Primary)],
        Stage::Existing { entries } => {
            let mut row = vec![control(
                Action::Another,
                "Archive as another day",
                Secondary,
            )];
            if let [(_, day)] = entries.as_slice() {
                // A wrong day number is usually noticed later, from the
                // calendar: it can be fixed from here, not only from the
                // result of the archive.
                row.push(control(Action::Move, "Change day", Secondary));
                row.push(control(
                    Action::Remove,
                    &format!("Remove Day {day}"),
                    Danger,
                ));
            }
            row.push(control(Action::Open, "Open gallery", Secondary));
            row
        }
        Stage::ConfirmRemove { day, .. } => vec![
            control(Action::RemoveYes, &format!("Remove Day {day}"), Danger),
            control(Action::Keep, "Keep it", Secondary),
        ],
        Stage::DayProblem { .. } => vec![control(Action::Day, "Change day", Primary)],
        Stage::Gap { day, .. } => vec![
            control(Action::Confirm, &format!("Yes, Day {day}"), Primary),
            control(Action::Day, "Change day", Secondary),
        ],
        Stage::Duplicate { day, next_free, .. } => vec![
            control(Action::Next, &format!("Use Day {next_free}"), Primary),
            control(Action::Day, "Pick another day", Secondary),
            control(Action::Replace, &format!("Replace Day {day}"), Danger),
        ],
        Stage::Retry { change_day, .. } => {
            let mut row = vec![control(Action::Retry, "Try again", Primary)];
            if *change_day {
                row.push(control(Action::Day, "Pick another day", Secondary));
            }
            row
        }
        Stage::Done { undo, .. } => {
            let mut row = vec![control(Action::Move, "Change day", Secondary)];
            if *undo {
                row.push(control(Action::Undo, "Undo", Secondary));
            }
            row.push(control(Action::Open, "Open gallery", Secondary));
            row
        }
        Stage::MoveGap { from, to, .. } => vec![
            control(Action::Confirm, &format!("Yes, Day {to}"), Primary),
            control(Action::Move, "Change day", Secondary),
            control(Action::Keep, &format!("Keep Day {from}"), Secondary),
        ],
        Stage::Removed { .. } => vec![control(Action::Day, "Archive again", Secondary)],
        Stage::StillArchiving => vec![control(Action::Recheck, "Check again", Primary)],
    }
}

/// What the result on screen offers besides Change day, as `(undo,
/// promoted)`: kept when the entry changes day.
const fn result_extras(stage: &Stage) -> (bool, bool) {
    match stage {
        Stage::Done { undo, promoted, .. } | Stage::MoveGap { undo, promoted, .. } => {
            (*undo, *promoted)
        }
        _ => (false, false),
    }
}

/// The result for this post's entry at `day`, keeping what the result on
/// screen offers besides Change day.
const fn done_at(stage: &Stage, series_id: i64, day: i64) -> Stage {
    let (undo, promoted) = result_extras(stage);
    Stage::Done {
        series_id,
        day,
        undo,
        promoted,
    }
}

/// Whether `action` belongs to what `stage` shows.
fn stage_allows(stage: &Stage, action: Action) -> bool {
    match stage {
        Stage::PickSeries { .. } => action == Action::Pick,
        other => controls(other).iter().any(|c| c.action == action),
    }
}

/// The modal with the day field.
fn day_modal(custom_id: String, title: String, label: String, value: i64) -> serenity::CreateModal {
    serenity::CreateModal::new(custom_id, title).components(vec![
        serenity::CreateActionRow::InputText(
            serenity::CreateInputText::new(serenity::InputTextStyle::Short, label, DAY_INPUT_ID)
                .value(value.to_string())
                .placeholder("For example 42")
                .min_length(1)
                .max_length(DAY_INPUT_MAX_CHARS)
                .required(true),
        ),
    ])
}

/// What was typed into the day field.
fn submitted_day_text(data: &serenity::ModalInteractionData) -> &str {
    data.components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == DAY_INPUT_ID => {
                input.value.as_deref()
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// The day named in the post's text, when it is one the day field accepts
/// ("Day 0" is not).
fn usable_suggestion(suggested: Option<i64>) -> Option<i64> {
    suggested.filter(|day| (1..=parser::MAX_DAY).contains(day))
}

/// The day a modal opens on, and what its label says about it.
async fn day_default(
    posts: &PostRepo,
    series: &Series,
    suggested: Option<i64>,
) -> DbResult<(i64, DayHint)> {
    match usable_suggestion(suggested) {
        // The day named in the post's text wins over the series' next day,
        // unless it is already archived: then the modal opens on a free
        // day and the label says why.
        Some(day) if posts.exists(series.id, day).await? => {
            let free = posts
                .first_free_day_from(series.id, day.saturating_add(1))
                .await?;
            Ok((free.min(parser::MAX_DAY), DayHint::Taken(day)))
        }
        Some(day) => Ok((day, DayHint::FromText)),
        None => Ok((next_day(posts, series).await?, DayHint::Next)),
    }
}

/// The series' highest day not counting `day`: what renumbering the entry
/// at `day` is measured against.
async fn max_day_without(posts: &PostRepo, series_id: i64, day: i64) -> DbResult<Option<i64>> {
    match posts.max_day(series_id).await? {
        Some(max) if max == day => Ok(posts.neighbor_days(series_id, day).await?.0),
        other => Ok(other),
    }
}

/// The day after the series' highest, or its first day when it is empty.
async fn next_day(posts: &PostRepo, series: &Series) -> DbResult<i64> {
    let next = posts
        .max_day(series.id)
        .await?
        .map_or(series.start_day, |max| max.saturating_add(1));
    Ok(next.clamp(1, parser::MAX_DAY))
}

// ---------------------------------------------------------------------------
// Discord sends that never ping
// ---------------------------------------------------------------------------

/// Sends a message that notifies nobody: mentions render but do not ping,
/// and the message arrives silently. With `reply_to` it is sent as a reply
/// and, if Discord refuses that (no Read Message History, message gone),
/// once more as a plain message.
///
/// Stand-in for the shared `checks::send_quiet`.
async fn send_quiet(
    http: &serenity::Http,
    channel: serenity::ChannelId,
    content: &str,
    reply_to: Option<serenity::MessageId>,
) -> Result<serenity::Message, serenity::Error> {
    let message = |reply: Option<serenity::MessageId>| {
        let message = serenity::CreateMessage::new()
            .content(content)
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .flags(serenity::MessageFlags::SUPPRESS_NOTIFICATIONS);
        match reply {
            Some(id) => message.reference_message((channel, id)),
            None => message,
        }
    };
    match channel.send_message(http, message(reply_to)).await {
        Err(e) if reply_to.is_some() => {
            tracing::debug!(%channel, error = %e, "reply refused; sending without a reference");
            channel.send_message(http, message(None)).await
        }
        sent => sent,
    }
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Archive a message into your series.
#[poise::command(
    context_menu_command = "Archive to Series",
    guild_only,
    install_context = "Guild"
)]
pub async fn archive_menu(ctx: Context<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let poise::Context::Application(app) = ctx else {
        return Ok(());
    };
    let interaction = app.interaction;
    let data = ctx.data();
    let invoker = ctx.author().id;
    let is_admin = checks::is_admin(&ctx);

    // Everything before the first reply is local SQLite: a modal must be
    // the first response, and it has three seconds.
    let (live, revoked): (Vec<Series>, Vec<Series>) = data
        .series
        .list_by_creator(&settings.guild_id, &invoker.to_string())
        .await?
        .into_iter()
        .partition(|s| s.state != SeriesState::Revoked);

    let gathered = gather(&msg);
    let channel_id = msg.channel_id.to_string();
    let parent_channel = thread_parent(interaction.channel.as_ref());
    let no_series = live.is_empty();
    let watched = channel_watched(&settings, &channel_id, parent_channel.as_deref());
    let nothing_text = nothing_to_archive_text(&gathered.skipped, gathered.has_embeds);
    let no_files = gathered.files.is_empty();

    let source = SourcePost {
        guild_id: settings.guild_id.clone(),
        message_id: msg.id.to_string(),
        channel_id,
        caption: gathered.caption,
        // A forward keeps its own time: that is when it was posted here.
        posted_at: msg.timestamp.unix_timestamp(),
        files: gathered.files,
    };
    let mut session = Session {
        http: &ctx.serenity_context().http,
        data,
        settings: &settings,
        invoker,
        is_admin,
        app_permissions: interaction.app_permissions,
        channel: msg.channel_id,
        message: msg.id,
        suggested: parser::suggested_day(&source.caption),
        source: Arc::new(source),
        parent_channel,
        author: (msg.author.id != invoker).then(|| msg.author.display_name().to_owned()),
        skipped: gathered.skipped,
        series: live,
        prefix: session_prefix(interaction.id.get()),
        generation: 0,
        stage: Stage::Idle,
        surface: None,
        surface_until: tokio::time::Instant::now() + SESSION_IDLE,
        modal: None,
        deadline: tokio::time::Instant::now() + SESSION_IDLE,
        reacted: None,
        milestone_message: None,
    };
    // Listening starts before anything is shown, so no press is missed.
    // From then until this command returns, the session's ids are its own;
    // after that the component router answers them as expired.
    let _listening = components::SESSIONS.listen(&session.prefix);
    let events = incoming(
        &ctx.serenity_context().shard,
        invoker,
        session.prefix.clone(),
    );

    let via = Via::Command(interaction);
    if no_series {
        session
            .render(via, Stage::NoSeries, no_series_text(&revoked))
            .await?;
    } else if !watched {
        return refuse(&ctx, not_watched_text(&settings.watched_channels, is_admin)).await;
    } else if no_files {
        return refuse(&ctx, nothing_text).await;
    } else {
        session.open(via).await?;
    }
    // The command has its first response now: anything poise sends after
    // this (its error reply included) must go out as a follow-up.
    app.has_sent_initial_response.store(true, Ordering::SeqCst);

    session.run(events).await;
    Ok(())
}

/// When a session with nothing happening next has to act: strip the
/// controls before the token that edits them dies (`surface_until`), or end
/// at `deadline`, whichever comes first.
fn wake_at(
    deadline: tokio::time::Instant,
    surface_until: Option<tokio::time::Instant>,
) -> tokio::time::Instant {
    surface_until.map_or(deadline, |until| until.min(deadline))
}

/// Ends the command with a private one-line refusal.
async fn refuse(ctx: &Context<'_>, text: String) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The session: one archive, from the first reply to the last button
// ---------------------------------------------------------------------------

/// A tap or a submit that belongs to one archive.
enum Incoming {
    Press(Box<serenity::ComponentInteraction>),
    Submit(Box<serenity::ModalInteraction>),
}

/// Every button press and modal submit of one archive: the invoker's, with
/// this session's id prefix.
fn incoming(
    shard: &serenity::ShardMessenger,
    user: serenity::UserId,
    prefix: String,
) -> impl Stream<Item = Incoming> + Send + use<> {
    serenity::collect(shard, move |event| {
        let serenity::Event::InteractionCreate(created) = event else {
            return None;
        };
        match &created.interaction {
            serenity::Interaction::Component(press)
                if press.user.id == user && press.data.custom_id.starts_with(&prefix) =>
            {
                Some(Incoming::Press(Box::new(press.clone())))
            }
            serenity::Interaction::Modal(submit)
                if submit.user.id == user && submit.data.custom_id.starts_with(&prefix) =>
            {
                Some(Incoming::Submit(Box::new(submit.clone())))
            }
            _ => None,
        }
    })
}

/// The interaction a reply goes out on.
#[derive(Clone, Copy)]
enum Via<'i> {
    /// The command itself: a new private message.
    Command(&'i serenity::CommandInteraction),
    /// A button or select: its message is updated in place.
    Press(&'i serenity::ComponentInteraction),
    /// A modal submit: updates the message the modal was opened from, or
    /// is a new private message when it was opened by the command.
    Submit(&'i serenity::ModalInteraction),
    /// No new interaction: edit the message already on screen.
    Edit,
}

/// What an open modal is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// The day to archive into.
    Archive { series_id: i64 },
    /// The day to renumber an entry to.
    Move { series_id: i64, from: i64 },
}

/// The modal the session last opened. Discord sends no event when a modal
/// is dismissed, so this stays set until the next one replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenModal {
    custom_id: String,
    purpose: Purpose,
}

/// One run of the archive flow.
struct Session<'a> {
    http: &'a serenity::Http,
    data: &'a Data,
    settings: &'a GuildSettings,
    invoker: serenity::UserId,
    is_admin: bool,
    /// The bot's permissions in the message's channel, as Discord reported
    /// them on the command.
    app_permissions: Option<serenity::Permissions>,
    channel: serenity::ChannelId,
    message: serenity::MessageId,
    source: Arc<SourcePost>,
    /// The channel the message's thread hangs off, if it is in a thread.
    parent_channel: Option<String>,
    /// The day named in the post's text, if any.
    suggested: Option<i64>,
    /// The message's author, when it is not the invoker: their display
    /// name as Discord gives it, not escaped.
    author: Option<String>,
    /// Notes about files left out before the modal.
    skipped: Vec<String>,
    /// The invoker's series that take posts.
    series: Vec<Series>,
    /// Start of every component and modal id of this session.
    prefix: String,
    /// Counts what has been drawn: a press from an earlier drawing (a
    /// double tap, a stale button) carries an older number and is ignored.
    generation: u32,
    stage: Stage,
    /// Token that can edit the message on screen.
    surface: Option<String>,
    /// When `surface` stops working: 14 minutes after its interaction.
    surface_until: tokio::time::Instant,
    modal: Option<OpenModal>,
    /// When the session stops listening: 14 minutes after the last new
    /// interaction or modal opened.
    deadline: tokio::time::Instant,
    /// The reaction this session added to the post.
    reacted: Option<serenity::ReactionType>,
    /// The milestone announcement this session posted.
    milestone_message: Option<(serenity::ChannelId, serenity::MessageId)>,
}

impl Session<'_> {
    fn series_by_id(&self, id: i64) -> Option<&Series> {
        self.series.iter().find(|s| s.id == id)
    }

    /// The series' name as typed: for a modal title, which Discord shows
    /// literally.
    fn series_name(&self, id: i64) -> String {
        self.series_by_id(id)
            .map_or_else(|| "this series".to_owned(), |s| s.name.clone())
    }

    /// The series' name for message content, where Discord renders
    /// markdown: escaped so it shows exactly as typed.
    fn series_shown(&self, id: i64) -> String {
        self.series_by_id(id).map_or_else(
            || "this series".to_owned(),
            |s| series_ops::display_name(&s.name),
        )
    }

    /// Restarts the idle clock: a new interaction arrived, or a modal was
    /// opened whose submit may come at any time in the next minutes.
    fn touch(&mut self) {
        self.deadline = tokio::time::Instant::now() + SESSION_IDLE;
    }

    /// Makes `token`, from a new interaction, the one that edits the
    /// message on screen. It works for 15 minutes, so the controls can be
    /// stripped before it dies.
    fn set_surface(&mut self, token: String) {
        self.surface = Some(token);
        self.touch();
        self.surface_until = self.deadline;
    }

    fn source_link(&self) -> String {
        jump_link(
            &self.source.guild_id,
            &self.source.channel_id,
            &self.source.message_id,
        )
    }

    /// The entries this message already has in the invoker's series.
    async fn existing_entries(&self) -> DbResult<Vec<(i64, i64)>> {
        let all = self
            .data
            .posts
            .find_all_by_message(&self.source.message_id)
            .await?;
        Ok(all
            .into_iter()
            .filter(|(series_id, _)| self.series_by_id(*series_id).is_some())
            .collect())
    }

    /// Handles presses and submits until the prompts go quiet.
    async fn run(mut self, events: impl Stream<Item = Incoming> + Send) {
        let mut events = std::pin::pin!(events);
        while self.is_live() {
            let surface_until = self.surface.is_some().then_some(self.surface_until);
            let wake = wake_at(self.deadline, surface_until);
            let event = match tokio::time::timeout_at(wake, events.next()).await {
                Ok(Some(event)) => event,
                // A modal opened late outlives the token that edits the
                // message: take the controls off while they still can be,
                // and keep listening for the modal's submit (it brings a
                // token of its own).
                Err(_) if wake < self.deadline => {
                    self.expire().await;
                    self.surface = None;
                    continue;
                }
                Ok(None) | Err(_) => break,
            };
            match event {
                Incoming::Press(press) => {
                    if let Err(e) = self.on_press(&press).await {
                        tracing::error!(error = format!("{e:#}"), "archive button failed");
                        self.report_failure(Via::Press(&press)).await;
                    }
                }
                Incoming::Submit(submit) => {
                    if let Err(e) = self.on_submit(&submit).await {
                        tracing::error!(error = format!("{e:#}"), "archive submit failed");
                        self.report_failure(Via::Submit(&submit)).await;
                    }
                }
            }
        }
        self.expire().await;
    }

    /// Whether anything on screen (or an open modal) can still send an
    /// event.
    fn is_live(&self) -> bool {
        matches!(self.stage, Stage::PickSeries { .. })
            || !controls(&self.stage).is_empty()
            || (self.stage == Stage::Idle && self.modal.is_some() && self.surface.is_none())
    }

    /// Strips the controls from the message on screen: when the session
    /// ends, or earlier when the token that can edit them is about to die.
    async fn expire(&self) {
        if !matches!(self.stage, Stage::PickSeries { .. }) && controls(&self.stage).is_empty() {
            return;
        }
        let Some(token) = self.surface.as_deref() else {
            return;
        };
        let stripped = serenity::EditInteractionResponse::new().components(Vec::new());
        if let Err(e) = stripped.execute(self.http, token).await {
            tracing::debug!(error = %e, "could not strip expired archive buttons");
        }
    }

    /// Tells the user a step failed: as a reply if the interaction is still
    /// unanswered, else in place of a progress line left on screen. A
    /// message that still has its controls is left alone.
    async fn report_failure(&mut self, via: Via<'_>) {
        let reply = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .content(STEP_FAILURE)
                .ephemeral(true),
        );
        let replied = match via {
            Via::Press(press) => press.create_response(self.http, reply).await,
            Via::Submit(submit) => submit.create_response(self.http, reply).await,
            Via::Command(_) | Via::Edit => return,
        };
        if replied.is_err()
            && self.stage == Stage::Idle
            && let Err(e) = self
                .render(Via::Edit, Stage::Idle, STEP_FAILURE.to_owned())
                .await
        {
            tracing::debug!(error = %e, "could not report a failed archive step");
        }
    }

    // -- drawing ----------------------------------------------------------

    /// The action rows of `stage`, with ids for drawing number `generation`.
    fn rows(&self, stage: &Stage, generation: u32) -> Vec<serenity::CreateActionRow> {
        if let Stage::PickSeries { options } = stage {
            if options.is_empty() {
                return Vec::new();
            }
            if options.len() <= PICK_BUTTONS_MAX {
                let buttons = options
                    .iter()
                    .map(|option| {
                        serenity::CreateButton::new(pick_id(
                            &self.prefix,
                            generation,
                            option.series_id,
                        ))
                        .label(truncate_chars(&option.name, BUTTON_LABEL_MAX_CHARS))
                        .style(serenity::ButtonStyle::Secondary)
                    })
                    .collect();
                return vec![serenity::CreateActionRow::Buttons(buttons)];
            }
            let options = options
                .iter()
                .map(|option| {
                    serenity::CreateSelectMenuOption::new(
                        truncate_chars(&option.name, OPTION_LABEL_MAX_CHARS),
                        option.series_id.to_string(),
                    )
                    .description(format!("Next: Day {}", option.next_day))
                })
                .collect();
            let menu = serenity::CreateSelectMenu::new(
                component_id(&self.prefix, generation, Action::Pick),
                serenity::CreateSelectMenuKind::String { options },
            )
            .placeholder("Choose a series");
            return vec![serenity::CreateActionRow::SelectMenu(menu)];
        }
        let buttons: Vec<serenity::CreateButton> = controls(stage)
            .into_iter()
            .map(|control| {
                serenity::CreateButton::new(component_id(&self.prefix, generation, control.action))
                    .label(control.label)
                    .style(control.style)
            })
            .collect();
        if buttons.is_empty() {
            Vec::new()
        } else {
            vec![serenity::CreateActionRow::Buttons(buttons)]
        }
    }

    /// Puts `stage` on screen with `content`. The stage is only taken on
    /// once Discord accepted it, so a refused reply leaves the buttons that
    /// are still visible working.
    async fn render(
        &mut self,
        via: Via<'_>,
        stage: Stage,
        content: String,
    ) -> Result<(), serenity::Error> {
        let generation = self.generation.wrapping_add(1);
        let rows = self.rows(&stage, generation);
        let token = self
            .deliver(via, clamp_message(&content), rows, false)
            .await?;
        if let Some(token) = token {
            self.set_surface(token);
        }
        self.stage = stage;
        self.generation = generation;
        Ok(())
    }

    /// Answers the interaction that starts slow work: a loading state for a
    /// fresh reply, the progress line where a message is already on screen.
    /// Returns whether the line is visible.
    async fn begin_work(&mut self, via: Via<'_>, text: &str) -> Result<bool, serenity::Error> {
        let visible = !matches!(via, Via::Submit(submit) if submit.message.is_none());
        let token = self
            .deliver(via, clamp_message(text), Vec::new(), true)
            .await?;
        if let Some(token) = token {
            self.set_surface(token);
        }
        self.stage = Stage::Idle;
        self.generation = self.generation.wrapping_add(1);
        Ok(visible)
    }

    /// Sends `content` and `rows` through `via`. Returns the token that
    /// edits the message from now on, when the interaction is a new one.
    async fn deliver(
        &self,
        via: Via<'_>,
        content: String,
        rows: Vec<serenity::CreateActionRow>,
        working: bool,
    ) -> Result<Option<String>, serenity::Error> {
        use serenity::CreateInteractionResponse::{Defer, Message, UpdateMessage};
        // Private replies ping nobody, but a series name is user text.
        let quiet = serenity::CreateAllowedMentions::new;
        let message = || {
            serenity::CreateInteractionResponseMessage::new()
                .content(content.clone())
                .components(rows.clone())
                .allowed_mentions(quiet())
        };
        match via {
            Via::Command(command) => {
                let reply = Message(message().ephemeral(true));
                command.create_response(self.http, reply).await?;
                Ok(Some(command.token.clone()))
            }
            Via::Press(press) => {
                press
                    .create_response(self.http, UpdateMessage(message()))
                    .await?;
                Ok(Some(press.token.clone()))
            }
            Via::Submit(submit) => {
                let reply = if submit.message.is_some() {
                    UpdateMessage(message())
                } else if working {
                    Defer(serenity::CreateInteractionResponseMessage::new().ephemeral(true))
                } else {
                    Message(message().ephemeral(true))
                };
                submit.create_response(self.http, reply).await?;
                Ok(Some(submit.token.clone()))
            }
            Via::Edit => {
                let Some(token) = self.surface.as_deref() else {
                    return Ok(None);
                };
                serenity::EditInteractionResponse::new()
                    .content(content)
                    .components(rows)
                    .allowed_mentions(quiet())
                    .execute(self.http, token)
                    .await?;
                Ok(None)
            }
        }
    }

    // -- opening ----------------------------------------------------------

    /// The first screen for the post: what it already is in the archive, a
    /// notice while an earlier try is still storing it, or the way into the
    /// modal.
    async fn open(&mut self, via: Via<'_>) -> Result<(), Error> {
        let entries = self.existing_entries().await?;
        if !entries.is_empty() {
            return self.show_existing(via, entries, false).await;
        }
        // A second try during a slow upload would open the modal on the
        // same day and be turned away only after the submit.
        let message = &self.source.message_id;
        let running = self
            .series
            .iter()
            .any(|s| INFLIGHT.is_held(&FlightKey::Message(s.id, message.clone())));
        if running {
            let notice = STILL_ARCHIVING.to_owned();
            self.render(via, Stage::StillArchiving, notice).await?;
            return Ok(());
        }
        self.start(via).await
    }

    /// Settles the series, then asks for the day: straight to the modal
    /// when the series is not in question, else through the select.
    async fn start(&mut self, via: Via<'_>) -> Result<(), Error> {
        let target = resolve_target(
            &self.series,
            &self.source.channel_id,
            self.parent_channel.as_deref(),
        )
        .map(|s| s.id);
        match target {
            Some(series_id) => self.open_day_modal(via, series_id).await,
            None => self.show_picker(via).await,
        }
    }

    /// Shows the series picker: a button per series, or a select with
    /// each series' next day when there are more than a row holds.
    async fn show_picker(&mut self, via: Via<'_>) -> Result<(), Error> {
        if self.series.is_empty() {
            // Every series was revoked or deleted while the prompts were up.
            let content = "🍂 You have no series that can take posts now, so nothing was archived."
                .to_owned();
            self.render(via, Stage::Idle, content).await?;
            return Ok(());
        }
        let mut options = Vec::new();
        // Newest first: with more series than a select holds, the oldest
        // are the ones left out.
        for series in self.series.iter().rev().take(SELECT_MAX_OPTIONS) {
            options.push(SeriesOption {
                series_id: series.id,
                name: series.name.clone(),
                next_day: next_day(&self.data.posts, series).await?,
            });
        }
        let content = pick_series_text(self.author.as_deref());
        self.render(via, Stage::PickSeries { options }, content)
            .await?;
        Ok(())
    }

    /// Opens the day modal for `series_id`, pre-filled with the day named
    /// in the post or that series' next day.
    async fn open_day_modal(&mut self, via: Via<'_>, series_id: i64) -> Result<(), Error> {
        let Some(series) = self.series_by_id(series_id) else {
            return self.dismiss(via).await;
        };
        let (value, hint) = day_default(&self.data.posts, series, self.suggested).await?;
        let title = modal_title(&series.name, self.author.as_deref());
        self.open_modal(
            via,
            Purpose::Archive { series_id },
            title,
            day_label(hint, value),
            value,
        )
        .await
    }

    /// Opens a day modal. Only a command or a component can be answered
    /// with a modal.
    async fn open_modal(
        &mut self,
        via: Via<'_>,
        purpose: Purpose,
        title: String,
        label: String,
        value: i64,
    ) -> Result<(), Error> {
        let opener = match via {
            Via::Command(command) => command.id.get(),
            Via::Press(press) => press.id.get(),
            Via::Submit(_) | Via::Edit => return Ok(()),
        };
        let custom_id = modal_id(&self.prefix, opener);
        let modal = serenity::CreateInteractionResponse::Modal(day_modal(
            custom_id.clone(),
            title,
            label,
            value,
        ));
        match via {
            Via::Command(command) => command.create_response(self.http, modal).await?,
            Via::Press(press) => press.create_response(self.http, modal).await?,
            Via::Submit(_) | Via::Edit => {}
        }
        self.modal = Some(OpenModal { custom_id, purpose });
        self.touch();
        Ok(())
    }

    /// Shows the "already archived" notice.
    async fn show_existing(
        &mut self,
        via: Via<'_>,
        entries: Vec<(i64, i64)>,
        unchanged: bool,
    ) -> Result<(), Error> {
        let named: Vec<(String, i64)> = entries
            .iter()
            .map(|(series_id, day)| (self.series_shown(*series_id), *day))
            .collect();
        let content = existing_text(&named, unchanged);
        self.render(via, Stage::Existing { entries }, content)
            .await?;
        Ok(())
    }

    // -- events -----------------------------------------------------------

    async fn on_submit(&mut self, submit: &serenity::ModalInteraction) -> Result<(), Error> {
        let Some(open) = self
            .modal
            .take_if(|open| open.custom_id == submit.data.custom_id)
        else {
            // Not the modal this session has open; close it quietly.
            return self.dismiss(Via::Submit(submit)).await;
        };
        let typed = parser::parse_day_input(submitted_day_text(&submit.data));
        let via = Via::Submit(submit);
        match (open.purpose, typed) {
            (Purpose::Archive { series_id }, Ok(day)) => {
                self.attempt(via, series_id, day, Mode::Insert, true).await
            }
            (Purpose::Archive { series_id }, Err(problem)) => {
                let content = format!("🍂 {problem}");
                self.render(via, Stage::DayProblem { series_id }, content)
                    .await?;
                Ok(())
            }
            (Purpose::Move { series_id, from }, Ok(to)) => {
                self.move_entry(via, series_id, (from, to), true).await
            }
            (Purpose::Move { series_id, from }, Err(problem)) => {
                let content = format!(
                    "🍂 {problem}\nThis post is still Day {from} of **{}**.",
                    self.series_shown(series_id)
                );
                let stage = done_at(&self.stage, series_id, from);
                self.render(via, stage, content).await?;
                Ok(())
            }
        }
    }

    async fn on_press(&mut self, press: &serenity::ComponentInteraction) -> Result<(), Error> {
        let via = Via::Press(press);
        let pressed = parse_component_id(&self.prefix, &press.data.custom_id)
            .filter(|(generation, action)| {
                *generation == self.generation && stage_allows(&self.stage, *action)
            })
            .map(|(_, action)| action);
        let Some(action) = pressed else {
            // A double tap, or a button from an earlier step.
            return self.dismiss(via).await;
        };
        match (action, self.stage.clone()) {
            (Action::Pick, Stage::PickSeries { options }) => self.pick(press, &options).await,
            (
                Action::Day,
                Stage::DayProblem { series_id }
                | Stage::Gap { series_id, .. }
                | Stage::Duplicate { series_id, .. }
                | Stage::Retry { series_id, .. },
            ) => self.open_day_modal(via, series_id).await,
            (Action::Day, Stage::Removed { series_id }) => self.archive_again(via, series_id).await,
            (Action::Confirm, Stage::Gap { series_id, day }) => {
                self.attempt(via, series_id, day, Mode::Insert, false).await
            }
            (
                Action::Next,
                Stage::Duplicate {
                    series_id,
                    next_free,
                    ..
                },
            ) => {
                self.attempt(via, series_id, next_free, Mode::Insert, false)
                    .await
            }
            (Action::Replace, Stage::Duplicate { series_id, day, .. }) => {
                self.attempt(via, series_id, day, Mode::Replace, false)
                    .await
            }
            (
                Action::Retry,
                Stage::Retry {
                    series_id,
                    day,
                    mode,
                    ..
                },
            ) => self.attempt(via, series_id, day, mode, false).await,
            (
                Action::Confirm,
                Stage::MoveGap {
                    series_id,
                    from,
                    to,
                    ..
                },
            ) => self.move_entry(via, series_id, (from, to), false).await,
            (
                Action::Keep,
                Stage::MoveGap {
                    series_id, from, ..
                },
            ) => self.keep_day(via, series_id, from).await,
            (
                Action::Move,
                Stage::Done { series_id, day, .. }
                | Stage::MoveGap {
                    series_id,
                    from: day,
                    ..
                },
            ) => self.open_move_modal(via, series_id, day).await,
            (Action::Move, Stage::Existing { entries }) => match entries.as_slice() {
                [(series_id, day)] => self.open_move_modal(via, *series_id, *day).await,
                _ => self.dismiss(via).await,
            },
            (
                Action::Undo,
                Stage::Done {
                    series_id,
                    day,
                    promoted,
                    ..
                },
            ) => {
                self.remove_entry(press, series_id, day, promoted, true)
                    .await
            }
            (Action::Open, stage) => self.open_gallery(press, &stage).await,
            (Action::Another, Stage::Existing { .. }) => self.start(via).await,
            (Action::Recheck, Stage::StillArchiving) => self.open(via).await,
            (Action::Remove, Stage::Existing { entries }) => self.ask_remove(via, entries).await,
            (Action::RemoveYes, Stage::ConfirmRemove { series_id, day, .. }) => {
                self.remove_entry(press, series_id, day, false, false).await
            }
            (Action::Keep, Stage::ConfirmRemove { back, .. }) => {
                self.show_existing(via, back, false).await
            }
            _ => self.dismiss(via).await,
        }
    }

    /// Opens the day modal for the series picked, if it is one the picker
    /// offered.
    async fn pick(
        &mut self,
        press: &serenity::ComponentInteraction,
        options: &[SeriesOption],
    ) -> Result<(), Error> {
        let via = Via::Press(press);
        let picked = picked_series(&self.prefix, press)
            .filter(|id| options.iter().any(|option| option.series_id == *id));
        match picked {
            Some(series_id) => self.open_day_modal(via, series_id).await,
            None => self.dismiss(via).await,
        }
    }

    /// Starts over after an undo or a removal. The series may be what was
    /// wrong, so it is asked again when there is a choice.
    async fn archive_again(&mut self, via: Via<'_>, series_id: i64) -> Result<(), Error> {
        if self.series.len() > 1 {
            self.show_picker(via).await
        } else {
            self.open_day_modal(via, series_id).await
        }
    }

    /// Opens the modal that renumbers the entry at `from`.
    async fn open_move_modal(
        &mut self,
        via: Via<'_>,
        series_id: i64,
        from: i64,
    ) -> Result<(), Error> {
        let title = truncate_chars(
            &format!("Change day in {}", self.series_name(series_id)),
            MODAL_TEXT_MAX_CHARS,
        );
        let purpose = Purpose::Move { series_id, from };
        let label = day_label(DayHint::Current(from), from);
        self.open_modal(via, purpose, title, label, from).await
    }

    /// Backs out of a far-off Change day: the result returns as it was.
    async fn keep_day(&mut self, via: Via<'_>, series_id: i64, from: i64) -> Result<(), Error> {
        let content = format!(
            "🍃 This post is still Day {from} of **{}**.",
            self.series_shown(series_id)
        );
        let stage = done_at(&self.stage, series_id, from);
        self.render(via, stage, content).await?;
        Ok(())
    }

    /// Acknowledges an interaction that changes nothing, so the client does
    /// not report it as failed.
    async fn dismiss(&self, via: Via<'_>) -> Result<(), Error> {
        let ack = serenity::CreateInteractionResponse::Acknowledge;
        let acked = match via {
            Via::Press(press) => press.create_response(self.http, ack).await,
            Via::Submit(submit) => submit.create_response(self.http, ack).await,
            Via::Command(_) | Via::Edit => Ok(()),
        };
        if let Err(e) = acked {
            // Usually a press that waited behind a long upload and expired.
            tracing::debug!(error = %e, "could not acknowledge a stale archive interaction");
        }
        Ok(())
    }

    async fn ask_remove(&mut self, via: Via<'_>, entries: Vec<(i64, i64)>) -> Result<(), Error> {
        let [(series_id, day)] = entries.as_slice() else {
            return self.dismiss(via).await;
        };
        let (series_id, day) = (*series_id, *day);
        let content = format!(
            "Remove Day {day} of **{}**? Its stored files are deleted too. This can't be undone.",
            self.series_shown(series_id)
        );
        let stage = Stage::ConfirmRemove {
            series_id,
            day,
            back: entries,
        };
        self.render(via, stage, content).await?;
        Ok(())
    }

    // -- archiving ---------------------------------------------------------

    /// The series as it is now. The prompts stay up for minutes, and an
    /// admin may have revoked it meanwhile: then the user is told (about an
    /// archive when `archiving`, else about a change to an entry) and `None`
    /// returned. The interaction is answered whenever it is `None`.
    async fn current_series(
        &mut self,
        via: Via<'_>,
        series_id: i64,
        archiving: bool,
    ) -> Result<Option<Series>, Error> {
        let Some(cached) = self.series_by_id(series_id) else {
            self.dismiss(via).await?;
            return Ok(None);
        };
        let name = series_ops::display_name(&cached.name);
        match self.data.series.get(series_id).await? {
            Some(fresh) if fresh.state != SeriesState::Revoked => {
                if let Some(cached) = self.series.iter_mut().find(|s| s.id == series_id) {
                    cached.clone_from(&fresh);
                }
                Ok(Some(fresh))
            }
            gone => {
                self.series.retain(|s| s.id != series_id);
                let content = match (gone.is_some(), archiving) {
                    (true, true) => format!(
                        "🍂 A server admin revoked **{name}**, so it can't take new posts and \
                         nothing was archived. Your archive is kept: ask an admin to restore it."
                    ),
                    (true, false) => format!(
                        "🍂 A server admin revoked **{name}**, so its days can't be changed. \
                         Your archive is kept: ask an admin to restore it."
                    ),
                    (false, true) => {
                        format!("🍂 **{name}** no longer exists, so nothing was archived.")
                    }
                    (false, false) => format!("🍂 **{name}** no longer exists."),
                };
                self.render(via, Stage::Idle, content).await?;
                Ok(None)
            }
        }
    }

    /// Archives the post as `day` of `series_id` and shows how it went.
    async fn attempt(
        &mut self,
        via: Via<'_>,
        series_id: i64,
        day: i64,
        mode: Mode,
        check_gap: bool,
    ) -> Result<(), Error> {
        let Some(series) = self.current_series(via, series_id, true).await? else {
            return Ok(());
        };
        if check_gap && mode == Mode::Insert {
            let max_day = self.data.posts.max_day(series.id).await?;
            if let Some(question) = gap_warning(&series, max_day, day, DayUse::Archive) {
                self.render(via, Stage::Gap { series_id, day }, question)
                    .await?;
                return Ok(());
            }
        }

        let shown = series_ops::display_name(&series.name);
        let progress = progress_text(&shown, day, &self.source.files);
        let visible = self.begin_work(via, &progress).await?;

        let source = Arc::clone(&self.source);
        let request = ArchiveRequest {
            series_id,
            day,
            source: &source,
            mode,
            now: checks::now_unix(),
        };
        let data = self.data;
        let work = archive_core(&data.posts, &data.media, &INFLIGHT, &request);
        tokio::pin!(work);
        let outcome = if let Ok(outcome) = tokio::time::timeout(SLOW_NOTICE_AFTER, &mut work).await
        {
            outcome
        } else {
            if !visible {
                self.show_progress(&progress).await;
            }
            work.await
        };

        let retry = |change_day| Stage::Retry {
            series_id,
            day,
            mode,
            change_day,
        };
        let shown = match outcome {
            Ok(ArchiveOutcome::Archived(done)) => {
                self.after_commit(&series, day, mode, &done).await;
                Ok(())
            }
            Ok(ArchiveOutcome::AlreadyArchived) => {
                return self
                    .show_existing(Via::Edit, vec![(series_id, day)], true)
                    .await;
            }
            Ok(ArchiveOutcome::DayTaken(holder)) => {
                return self.show_duplicate(&series, day, &holder).await;
            }
            Ok(ArchiveOutcome::Busy(busy)) => {
                let content = busy_text(busy, &shown, day);
                self.render(Via::Edit, retry(busy == Busy::ThisDay), content)
                    .await
            }
            Ok(ArchiveOutcome::Failed(failure)) => {
                if failure.storage {
                    // Not a note for the creator: only an admin can fix it.
                    self.log(log_text(
                        LogKind::StorageFailed,
                        &series,
                        day,
                        self.invoker,
                        "",
                    ))
                    .await;
                }
                let content = failure_text(&failure, &self.skipped);
                let stage = if failure.retryable {
                    retry(false)
                } else {
                    Stage::Idle
                };
                self.render(Via::Edit, stage, content).await
            }
            Err(e) => {
                tracing::error!(series = series_id, day, error = %e, "archive failed");
                self.render(Via::Edit, retry(false), GENERIC_FAILURE.to_owned())
                    .await
            }
        };
        if let Err(e) = shown {
            tracing::warn!(series = series_id, day, error = %e, "could not show the archive outcome");
        }
        Ok(())
    }

    /// Replaces the loading state with a line of text, for uploads slow
    /// enough that "thinking" alone would look stuck.
    async fn show_progress(&self, progress: &str) {
        let Some(token) = self.surface.as_deref() else {
            return;
        };
        let edit = serenity::EditInteractionResponse::new()
            .content(progress)
            .allowed_mentions(serenity::CreateAllowedMentions::new());
        if let Err(e) = edit.execute(self.http, token).await {
            tracing::debug!(error = %e, "could not show archive progress");
        }
    }

    /// Shows who holds the day, with the ways forward.
    async fn show_duplicate(
        &mut self,
        series: &Series,
        day: i64,
        holder: &Post,
    ) -> Result<(), Error> {
        let next_free = self
            .data
            .posts
            .first_free_day_from(series.id, day.saturating_add(1))
            .await?
            .min(parser::MAX_DAY);
        let link = jump_link(
            &self.source.guild_id,
            &holder.channel_id,
            &holder.message_id,
        );
        let stage = Stage::Duplicate {
            series_id: series.id,
            day,
            next_free,
        };
        let shown = series_ops::display_name(&series.name);
        self.render(Via::Edit, stage, duplicate_text(&shown, day, &link))
            .await?;
        Ok(())
    }

    /// Everything after the day is written. All of it is best effort: the
    /// post is archived whatever happens here, so nothing in this function
    /// may turn into an error reply.
    async fn after_commit(&mut self, series: &Series, day: i64, mode: Mode, done: &Archived) {
        tracing::info!(
            guild = %self.source.guild_id,
            series = series.id,
            day,
            files = done.files,
            "archived post"
        );
        let mut lines = vec![archived_line(
            &series_ops::display_name(&series.name),
            day,
            done.files,
            self.author.as_deref(),
            mode,
        )];
        let skipped: Vec<String> = self.skipped.iter().chain(&done.skipped).cloned().collect();
        lines.extend(skipped_lines(&skipped));
        if done.placeholder_thumbs > 0 {
            lines.push(format!(
                "No preview could be made for {}, so the gallery shows a placeholder tile. \
                 The original is archived.",
                files_phrase(done.placeholder_thumbs)
            ));
        }

        let (promoted, sprout_line) = self.sprout_step(series).await;
        // The series as it is now, for the rules that depend on its state.
        let series = Series {
            state: if promoted {
                SeriesState::Active
            } else {
                series.state
            },
            ..series.clone()
        };
        let kind = match mode {
            Mode::Insert => LogKind::Archived { files: done.files },
            Mode::Replace => LogKind::Replaced { files: done.files },
        };
        let link = self.source_link();
        let mut log_lines = vec![log_text(kind, &series, day, self.invoker, &link)];
        if promoted {
            log_lines.push(log_text(
                LogKind::Sprouted,
                &series,
                day,
                self.invoker,
                &link,
            ));
        }

        // Independent Discord calls, made side by side so the result is not
        // held up by each of them in turn.
        let ((reacted, reaction_note), (announced, milestone_line), log_note, ()) = tokio::join!(
            self.react(&series),
            self.celebrate(&series, day, mode),
            self.log_all(&log_lines),
            self.unmark_replaced(&series, done.replaced.as_ref()),
        );
        self.reacted = reacted;
        self.milestone_message = announced;
        lines.extend(milestone_line);
        lines.extend(sprout_line);
        lines.extend(reaction_note);
        lines.extend(log_note);

        let stage = Stage::Done {
            series_id: series.id,
            day,
            undo: mode == Mode::Insert,
            promoted,
        };
        if let Err(e) = self.render(Via::Edit, stage, lines.join("\n")).await {
            tracing::warn!(series = series.id, day, error = %e, "could not show the archive result");
        }
    }

    /// Takes a sprout out of probation once it has enough days. Returns
    /// whether it was promoted, and the line to show about it.
    async fn sprout_step(&mut self, series: &Series) -> (bool, Option<String>) {
        if series.state != SeriesState::Sprout {
            return (false, None);
        }
        let threshold = self.settings.sprout_threshold;
        let archived = match self.data.posts.count(series.id).await {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(series = series.id, error = %e, "could not count days for sprout check");
                return (false, None);
            }
        };
        if archived < threshold {
            let shown = series_ops::display_name(&series.name);
            let line = sprout_progress_line(&shown, archived, threshold);
            return (false, Some(line));
        }
        // `series` was read before the upload. Only a series that is still
        // a sprout is promoted: one an admin revoked meanwhile stays revoked.
        match self
            .data
            .series
            .set_state_if(series.id, SeriesState::Sprout, SeriesState::Active)
            .await
        {
            Ok(true) => {
                self.set_local_state(series.id, SeriesState::Active);
                (true, Some(promotion_line(series)))
            }
            Ok(false) => (false, None),
            Err(e) => {
                tracing::warn!(series = series.id, error = %e, "could not promote sprout");
                (false, None)
            }
        }
    }

    fn set_local_state(&mut self, series_id: i64, state: SeriesState) {
        if let Some(series) = self.series.iter_mut().find(|s| s.id == series_id) {
            series.state = state;
        }
    }

    /// Marks the post with the series' reaction. Returns the reaction that
    /// landed, and a note when the mark is missing or not the one the
    /// creator chose.
    async fn react(&self, series: &Series) -> (Option<serenity::ReactionType>, Option<String>) {
        let needed =
            serenity::Permissions::ADD_REACTIONS | serenity::Permissions::READ_MESSAGE_HISTORY;
        if self
            .app_permissions
            .is_some_and(|granted| !granted.contains(needed))
        {
            return (None, Some(reaction_permission_note(self.channel)));
        }
        let wanted = reaction_for(&series.emoji);
        let fallback = reaction_for(FALLBACK_REACTION);
        let first = self
            .http
            .create_reaction(self.channel, self.message, &wanted)
            .await;
        let mut error = match first {
            Ok(()) => return (Some(wanted), None),
            Err(e) => e,
        };
        // The saved emoji is not one Discord knows (values saved before
        // validation existed, or an emoji newer than Discord's table).
        let unknown_emoji = discord_code(&error) == Some(UNKNOWN_EMOJI);
        if unknown_emoji && wanted != fallback {
            match self
                .http
                .create_reaction(self.channel, self.message, &fallback)
                .await
            {
                Ok(()) => {
                    let note = format!(
                        "Discord doesn't accept this series' reaction ({}), so I used \
                         {FALLBACK_REACTION}. Pick another emoji in the series settings.",
                        series.emoji.trim()
                    );
                    return (Some(fallback), Some(note));
                }
                Err(e) => error = e,
            }
        }
        tracing::warn!(
            channel = %self.channel,
            code = ?discord_code(&error),
            error = %error,
            "could not add the archive reaction"
        );
        let note = match discord_code(&error) {
            Some(MISSING_ACCESS | MISSING_PERMISSIONS) => reaction_permission_note(self.channel),
            _ => "I couldn't add the reaction to the post. It's archived all the same.".to_owned(),
        };
        (None, Some(note))
    }

    /// Takes the bot's mark off the post whose day Replace took over, once
    /// no entry is archived from it any more. Best effort.
    async fn unmark_replaced(&self, series: &Series, replaced: Option<&Post>) {
        let Some(old) = replaced else {
            return;
        };
        let snowflake = |raw: &str| raw.parse::<u64>().ok().filter(|id| *id != 0);
        let (Some(channel), Some(message)) =
            (snowflake(&old.channel_id), snowflake(&old.message_id))
        else {
            return;
        };
        match self.data.posts.find_all_by_message(&old.message_id).await {
            Ok(rest) if rest.is_empty() => {}
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, "could not check the replaced post's other entries");
                return;
            }
        }
        unreact(
            self.http,
            serenity::ChannelId::new(channel),
            serenity::MessageId::new(message),
            &[reaction_for(&series.emoji)],
        )
        .await;
    }

    /// Celebrates a milestone day. In the channel only when the series is
    /// public and active, the day is its new highest, and the post is
    /// recent: a reply to the post that pings nobody. Otherwise a line in
    /// the private result. Returns the announcement it posted, and that
    /// line or a note for an admin when the announcement failed.
    async fn celebrate(
        &self,
        series: &Series,
        day: i64,
        mode: Mode,
    ) -> (
        Option<(serenity::ChannelId, serenity::MessageId)>,
        Option<String>,
    ) {
        if mode != Mode::Insert {
            return (None, None);
        }
        let Some(reached) = milestone::classify_for(day, series.cadence) else {
            return (None, None);
        };
        match self.data.posts.max_day(series.id).await {
            Ok(Some(max)) if max == day => {}
            // A backfilled Day 100 is not news.
            Ok(_) => return (None, None),
            Err(e) => {
                tracing::warn!(series = series.id, error = %e, "could not check the highest day");
                return (None, None);
            }
        }
        if series.privacy != Privacy::Public || series.state != SeriesState::Active {
            return (
                None,
                Some(private_milestone_line(
                    reached,
                    day,
                    &series_ops::display_name(&series.name),
                )),
            );
        }
        if checks::now_unix().saturating_sub(self.source.posted_at) > MILESTONE_MAX_AGE_SECS {
            return (None, None);
        }
        let text = announcement_text(series, reached, day);
        match send_quiet(self.http, self.channel, &text, Some(self.message)).await {
            Ok(sent) => (Some((sent.channel_id, sent.id)), None),
            Err(e) => {
                tracing::warn!(
                    series = series.id,
                    day,
                    channel = %self.channel,
                    error = %e,
                    "milestone announcement failed"
                );
                let note = self.is_admin.then(|| {
                    format!(
                        "I couldn't post the milestone message in <#{}>. Check that I can send \
                         messages there.",
                        self.channel
                    )
                });
                (None, note)
            }
        }
    }

    /// Writes `lines` to the log channel in order. Returns the note from the
    /// first write that failed, if any.
    async fn log_all(&self, lines: &[String]) -> Option<String> {
        for line in lines {
            if let Some(note) = self.log(line.clone()).await {
                return Some(note);
            }
        }
        None
    }

    /// Writes a line to the server's log channel. Returns a note for the
    /// invoker when the write failed and they are an admin who can fix it.
    async fn log(&self, line: String) -> Option<String> {
        // Boxed: the channel check and the send would otherwise sit inside
        // every future that can log, which is most of the session.
        Box::pin(components::log_line(self.http, self.settings, &line))
            .await
            .filter(|_| self.is_admin)
    }

    // -- result buttons ----------------------------------------------------

    /// Renumbers the entry just archived. With `check_gap`, a day far from
    /// the series' others is asked about first, as when archiving.
    async fn move_entry(
        &mut self,
        via: Via<'_>,
        series_id: i64,
        (from, to): (i64, i64),
        check_gap: bool,
    ) -> Result<(), Error> {
        let Some(series) = self.current_series(via, series_id, false).await? else {
            return Ok(());
        };
        let name = &series_ops::display_name(&series.name);
        let (undo, promoted) = result_extras(&self.stage);
        let done = |day| Stage::Done {
            series_id,
            day,
            undo,
            promoted,
        };
        if to == from {
            let content = format!("🍃 This post is still Day {from} of **{name}**.");
            self.render(via, done(from), content).await?;
            return Ok(());
        }
        if check_gap {
            // 430 typed for 43 would make 431 the series' next day.
            let others = max_day_without(&self.data.posts, series_id, from).await?;
            if let Some(question) = gap_warning(&series, others, to, DayUse::Move) {
                let stage = Stage::MoveGap {
                    series_id,
                    from,
                    to,
                    undo,
                    promoted,
                };
                self.render(via, stage, question).await?;
                return Ok(());
            }
        }
        let moved = move_core(
            &self.data.posts,
            &INFLIGHT,
            series_id,
            (from, to),
            &self.source.message_id,
        )
        .await?;
        match moved {
            Move::Moved => {
                let content = format!("✏️ Day {from} of **{name}** is now Day {to}.");
                self.render(via, done(to), content).await?;
                // An announcement made for the old number is now wrong.
                self.retract_milestone().await;
                let line = log_text(
                    LogKind::Moved { from },
                    &series,
                    to,
                    self.invoker,
                    &self.source_link(),
                );
                self.log(line).await;
            }
            Move::Taken => {
                let content = format!(
                    "🍂 Day {to} of **{name}** is already archived, so this post is still \
                     Day {from}. Pick a free day."
                );
                self.render(via, done(from), content).await?;
            }
            Move::NotThisPost => {
                let content = format!(
                    "🍂 Day {from} of **{name}** is no longer this post's entry, so nothing \
                     was changed."
                );
                self.render(via, Stage::Idle, content).await?;
            }
            Move::Busy => {
                let content = busy_text(Busy::ThisPost, name, to);
                self.render(via, done(from), content).await?;
            }
        }
        Ok(())
    }

    /// Removes an entry of this post and its files. `undo` words it as
    /// taking back the archive just made.
    async fn remove_entry(
        &mut self,
        press: &serenity::ComponentInteraction,
        series_id: i64,
        day: i64,
        promoted: bool,
        undo: bool,
    ) -> Result<(), Error> {
        let via = Via::Press(press);
        let Some(series) = self.current_series(via, series_id, false).await? else {
            return Ok(());
        };
        let name = &series_ops::display_name(&series.name);
        let before = self.stage.clone();
        // Deleting stored files can outlast the three seconds a press has.
        self.begin_work(via, &format!("Removing Day {day} of **{name}**…"))
            .await?;
        let removal = remove_core(
            &self.data.posts,
            &self.data.media,
            &INFLIGHT,
            series_id,
            day,
            &self.source.message_id,
        )
        .await;
        let (stage, content) = match removal {
            Ok(Removal::Removed) => {
                self.after_remove(&series, day, promoted).await;
                let content = if undo {
                    format!("↩️ Undone: Day {day} of **{name}** is no longer archived.")
                } else {
                    format!("🗑️ Day {day} of **{name}** removed. Its stored files are deleted.")
                };
                (Stage::Removed { series_id }, content)
            }
            Ok(Removal::NotThisPost) => (
                Stage::Idle,
                format!(
                    "🍂 Day {day} of **{name}** is no longer this post's entry, so nothing was \
                     removed."
                ),
            ),
            Ok(Removal::Busy) => (before, busy_text(Busy::ThisPost, name, day)),
            Err(e) => {
                tracing::error!(series = series_id, day, error = %e, "removing an entry failed");
                (before, STEP_FAILURE.to_owned())
            }
        };
        self.render(Via::Edit, stage, content).await?;
        Ok(())
    }

    /// Tidies up around a removed entry: the reaction, the sprout state,
    /// the milestone announcement, the log. Best effort.
    async fn after_remove(&mut self, series: &Series, day: i64, promoted: bool) {
        // The mark stays while any archive still holds this message.
        match self
            .data
            .posts
            .find_all_by_message(&self.source.message_id)
            .await
        {
            Ok(rest) if rest.is_empty() => self.remove_reaction(series).await,
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "could not check the post's other entries"),
        }
        let reverted = promoted && self.revert_promotion(series).await;
        self.retract_milestone().await;
        // Logged as the series is now: back in its sprout stage, it is not
        // named.
        let series = Series {
            state: if reverted {
                SeriesState::Sprout
            } else {
                series.state
            },
            ..series.clone()
        };
        let line = log_text(LogKind::Removed, &series, day, self.invoker, "");
        self.log(line).await;
    }

    /// Takes the bot's reaction off the post.
    async fn remove_reaction(&mut self, series: &Series) {
        let mine = self
            .reacted
            .take()
            .unwrap_or_else(|| reaction_for(&series.emoji));
        unreact(self.http, self.channel, self.message, &[mine]).await;
    }

    /// Puts a series back into its sprout stage when the archive that took
    /// it out is undone and it is short of the threshold again. Returns
    /// whether it did.
    async fn revert_promotion(&mut self, series: &Series) -> bool {
        let below = match self.data.posts.count(series.id).await {
            Ok(count) => count < self.settings.sprout_threshold,
            Err(e) => {
                tracing::warn!(series = series.id, error = %e, "could not count days after undo");
                false
            }
        };
        if !below {
            return false;
        }
        match self
            .data
            .series
            .set_state_if(series.id, SeriesState::Active, SeriesState::Sprout)
            .await
        {
            Ok(true) => {
                self.set_local_state(series.id, SeriesState::Sprout);
                true
            }
            // No longer active (taken down since): left as it is.
            Ok(false) => false,
            Err(e) => {
                tracing::warn!(series = series.id, error = %e, "could not return series to sprout");
                false
            }
        }
    }

    /// Deletes the milestone announcement this session posted, if any.
    async fn retract_milestone(&mut self) {
        let Some((channel, message)) = self.milestone_message.take() else {
            return;
        };
        if let Err(e) = self.http.delete_message(channel, message, None).await {
            tracing::debug!(%channel, error = %e, "could not delete the milestone announcement");
        }
    }

    /// Opens the gallery on what the stage is about. Discord's launch
    /// response carries no destination, so the destination is left as a
    /// short-lived intent the Activity collects when it starts.
    async fn open_gallery(
        &self,
        press: &serenity::ComponentInteraction,
        stage: &Stage,
    ) -> Result<(), Error> {
        let target = match stage {
            Stage::Done { series_id, day, .. } => Some((*series_id, Some(*day))),
            Stage::Existing { entries } => {
                entries.first().map(|(series, day)| (*series, Some(*day)))
            }
            _ => None,
        };
        if let Some((series_id, day)) = target {
            let intents = LaunchIntentRepo::new(self.data.pool.clone());
            let stored = intents
                .put(
                    &self.invoker.to_string(),
                    &self.source.guild_id,
                    series_id,
                    day,
                    checks::now_unix(),
                )
                .await;
            if let Err(e) = stored {
                // The gallery still opens, on its usual first screen.
                tracing::warn!(series = series_id, error = %e, "could not store the launch intent");
            }
        }
        let launch = serenity::CreateInteractionResponse::LaunchActivity;
        if let Err(e) = press.create_response(self.http, launch).await {
            tracing::warn!(code = ?discord_code(&e), error = %e, "could not launch the Activity");
            let fallback = serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(OPEN_GALLERY_FALLBACK)
                    .ephemeral(true),
            );
            press.create_response(self.http, fallback).await?;
        }
        Ok(())
    }
}

/// The series chosen with a picker button or in the select, if the press
/// is one.
fn picked_series(prefix: &str, press: &serenity::ComponentInteraction) -> Option<i64> {
    match &press.data.kind {
        serenity::ComponentInteractionDataKind::StringSelect { values } => {
            values.first()?.parse().ok()
        }
        serenity::ComponentInteractionDataKind::Button => {
            pick_button_series(prefix, &press.data.custom_id)
        }
        _ => None,
    }
}

/// Takes the bot's own `reactions` off a message, and the leaf it falls
/// back to (which one is there is not recorded). Best effort: a reaction
/// that is not there is refused, which is expected.
async fn unreact(
    http: &serenity::Http,
    channel: serenity::ChannelId,
    message: serenity::MessageId,
    reactions: &[serenity::ReactionType],
) {
    let mut all = reactions.to_vec();
    let fallback = reaction_for(FALLBACK_REACTION);
    if !all.contains(&fallback) {
        all.push(fallback);
    }
    for reaction in &all {
        if let Err(e) = http.delete_reaction_me(channel, message, reaction).await {
            tracing::debug!(%channel, error = %e, "could not remove an archive reaction");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "tests may panic")]

    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use leaf_core::db::{GuildSettingsRepo, SeriesRepo};
    use leaf_core::domain::{Cadence, DetectionMode, NewSeries};
    use leaf_core::media::StoredMedia;

    use super::*;

    // -- fixtures ----------------------------------------------------------

    static NEXT_DB: AtomicUsize = AtomicUsize::new(0);

    /// A migrated database with one guild and one series, in its own temp
    /// directory (file-backed, like production), removed on drop.
    struct TestDb {
        dir: PathBuf,
        posts: PostRepo,
        series_id: i64,
    }

    impl TestDb {
        async fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "leaf-bot-archive-{}-{}-{}",
                std::process::id(),
                checks::now_unix(),
                NEXT_DB.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let pool = leaf_core::db::connect(&dir.join("test.db")).await.unwrap();
            GuildSettingsRepo::new(pool.clone())
                .ensure_exists("g")
                .await
                .unwrap();
            let series = SeriesRepo::new(pool.clone())
                .create(
                    &NewSeries {
                        guild_id: "g".to_owned(),
                        creator_id: "u".to_owned(),
                        name: "s".to_owned(),
                        description: String::new(),
                        channels: vec![],
                        cadence: Cadence::Daily,
                        detection_mode: DetectionMode::ContextMenu,
                        privacy: Privacy::Public,
                        privacy_role_id: None,
                        start_day: 1,
                        state: SeriesState::Active,
                    },
                    0,
                )
                .await
                .unwrap();
            Self {
                dir,
                posts: PostRepo::new(pool),
                series_id: series.id,
            }
        }

        async fn message_at(&self, day: i64) -> Option<String> {
            self.posts
                .get(self.series_id, day)
                .await
                .unwrap()
                .map(|(post, _)| post.message_id)
        }

        async fn attachments_at(&self, day: i64) -> Vec<String> {
            self.posts
                .get(self.series_id, day)
                .await
                .unwrap()
                .map(|(_, media)| media.into_iter().map(|m| m.attachment_id).collect())
                .unwrap_or_default()
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.dir));
        }
    }

    /// How the fake storage fails for one attachment.
    #[derive(Debug, Clone, Copy)]
    enum Fault {
        /// The download fails; nothing is stored. Worth a retry.
        Fetch,
        /// Discord no longer has the file; nothing is stored. Permanent.
        Gone,
        /// Storage fails after the original landed. Worth a retry.
        Storage,
    }

    struct FakeState {
        objects: Mutex<HashSet<String>>,
        deleted: Mutex<Vec<String>>,
        faults: Mutex<HashMap<String, Fault>>,
        /// A store call for a gated attachment waits for a permit.
        gates: Mutex<HashMap<String, Arc<Semaphore>>>,
        /// One permit per store call that has begun.
        started: Semaphore,
        active: AtomicUsize,
        most_active: AtomicUsize,
        /// Written to the database when the first store call begins: a day
        /// archived by a path that does not take the guard.
        race: tokio::sync::Mutex<Option<(PostRepo, Post)>>,
    }

    /// Storage that lives in a set, with switches for the failures and the
    /// orderings the tests need. No sleeps: tests wait on semaphores.
    #[derive(Clone)]
    struct FakeSink(Arc<FakeState>);

    impl FakeSink {
        fn new() -> Self {
            Self(Arc::new(FakeState {
                objects: Mutex::default(),
                deleted: Mutex::default(),
                faults: Mutex::default(),
                gates: Mutex::default(),
                started: Semaphore::new(0),
                active: AtomicUsize::new(0),
                most_active: AtomicUsize::new(0),
                race: tokio::sync::Mutex::new(None),
            }))
        }

        fn fail(&self, attachment: &str, fault: Fault) {
            self.0
                .faults
                .lock()
                .unwrap()
                .insert(attachment.to_owned(), fault);
        }

        fn heal(&self, attachment: &str) {
            self.0.faults.lock().unwrap().remove(attachment);
        }

        /// Holds back the store call for `attachment` until the returned
        /// semaphore gets a permit.
        fn gate(&self, attachment: &str) -> Arc<Semaphore> {
            let gate = Arc::new(Semaphore::new(0));
            self.0
                .gates
                .lock()
                .unwrap()
                .insert(attachment.to_owned(), Arc::clone(&gate));
            gate
        }

        async fn race_with(&self, posts: &PostRepo, post: Post) {
            *self.0.race.lock().await = Some((posts.clone(), post));
        }

        async fn wait_started(&self, calls: u32) {
            self.0.started.acquire_many(calls).await.unwrap().forget();
        }

        fn has(&self, key: &str) -> bool {
            self.0.objects.lock().unwrap().contains(key)
        }

        fn object_count(&self) -> usize {
            self.0.objects.lock().unwrap().len()
        }

        fn deleted(&self) -> Vec<String> {
            self.0.deleted.lock().unwrap().clone()
        }

        fn store_calls_begun(&self) -> usize {
            self.0.started.available_permits()
        }
    }

    impl MediaSink for FakeSink {
        async fn store(&self, _url: &str, meta: &MediaMeta) -> Result<ArchivedMedia, MediaError> {
            let state = &self.0;
            let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
            state.most_active.fetch_max(active, Ordering::SeqCst);
            state.started.add_permits(1);

            let racing = state.race.lock().await.take();
            if let Some((posts, post)) = racing {
                posts.insert_with_media(&post, &[]).await.unwrap();
            }
            let gate = state
                .gates
                .lock()
                .unwrap()
                .get(&meta.attachment_id)
                .cloned();
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }

            let fault = state
                .faults
                .lock()
                .unwrap()
                .get(&meta.attachment_id)
                .copied();
            let original_key = media::original_key(meta);
            let thumb_key = media::thumb_key(meta);
            let result = match fault {
                Some(Fault::Fetch) => Err(MediaError::Fetch("connection reset".to_owned())),
                Some(Fault::Gone) => Err(MediaError::Gone("404".to_owned())),
                Some(Fault::Storage) => {
                    // As in the pipeline: the original can be in storage
                    // before the failure shows.
                    state.objects.lock().unwrap().insert(original_key);
                    Err(MediaError::Io(std::io::Error::other("disk full")))
                }
                None => {
                    let mut objects = state.objects.lock().unwrap();
                    objects.insert(original_key.clone());
                    objects.insert(thumb_key.clone());
                    drop(objects);
                    Ok(ArchivedMedia {
                        stored: StoredMedia {
                            original_key,
                            thumb_key,
                            size: 1,
                        },
                        placeholder_thumb: false,
                    })
                }
            };
            state.active.fetch_sub(1, Ordering::SeqCst);
            result
        }

        async fn delete(&self, keys: &[String]) {
            for key in keys {
                self.0.objects.lock().unwrap().remove(key);
                self.0.deleted.lock().unwrap().push(key.clone());
            }
        }
    }

    fn file(id: &str) -> SourceFile {
        SourceFile {
            attachment_id: id.to_owned(),
            filename: format!("{id}.png"),
            url: format!("https://cdn.test/{id}"),
            content_type: "image/png",
        }
    }

    fn source(message: &str, files: &[&str]) -> SourcePost {
        SourcePost {
            guild_id: "g".to_owned(),
            message_id: message.to_owned(),
            channel_id: "c".to_owned(),
            caption: String::new(),
            posted_at: 100,
            files: files.iter().map(|id| file(id)).collect(),
        }
    }

    async fn run(
        db: &TestDb,
        sink: &FakeSink,
        inflight: &Inflight,
        source: &SourcePost,
        day: i64,
        mode: Mode,
    ) -> ArchiveOutcome {
        let request = ArchiveRequest {
            series_id: db.series_id,
            day,
            source,
            mode,
            now: 200,
        };
        archive_core(&db.posts, sink, inflight, &request)
            .await
            .unwrap()
    }

    fn archived(outcome: ArchiveOutcome) -> Archived {
        match outcome {
            ArchiveOutcome::Archived(done) => done,
            other => panic!("expected an archive, got {other:?}"),
        }
    }

    fn failed(outcome: ArchiveOutcome) -> Failure {
        match outcome {
            ArchiveOutcome::Failed(failure) => failure,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    fn series(id: i64, name: &str, channels: &[&str]) -> Series {
        Series {
            id,
            guild_id: "g".into(),
            creator_id: "u".into(),
            name: name.into(),
            description: String::new(),
            channels: channels.iter().map(|c| (*c).to_owned()).collect(),
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".into(),
            state: SeriesState::Active,
            created_at: 0,
        }
    }

    // -- in-flight guard ----------------------------------------------------

    #[test]
    fn guard_takes_all_keys_or_none_and_frees_them_on_drop() {
        let inflight = Inflight::default();
        let message = || FlightKey::Message(1, "m".to_owned());

        let held = inflight
            .try_begin(&[message(), FlightKey::Day(1, 5)])
            .unwrap();
        assert!(inflight.is_held(&message()));
        assert!(!inflight.is_held(&FlightKey::Day(1, 6)));
        // Either key alone blocks, and the refusal names the key in the way.
        assert_eq!(
            inflight
                .try_begin(&[FlightKey::Day(1, 6), message()])
                .unwrap_err(),
            message()
        );
        assert_eq!(
            inflight.try_begin(&[FlightKey::Day(1, 5)]).unwrap_err(),
            FlightKey::Day(1, 5)
        );
        // The refused attempt took nothing: Day 6 is still free.
        drop(inflight.try_begin(&[FlightKey::Day(1, 6)]).unwrap());
        // The same message in another series writes other objects.
        drop(
            inflight
                .try_begin(&[FlightKey::Message(2, "m".to_owned()), FlightKey::Day(2, 5)])
                .unwrap(),
        );

        drop(held);
        assert!(!inflight.is_held(&message()));
        drop(
            inflight
                .try_begin(&[message(), FlightKey::Day(1, 5)])
                .unwrap(),
        );
    }

    // -- archive core: data safety ------------------------------------------

    #[tokio::test]
    async fn second_attempt_during_an_upload_is_turned_away_and_costs_nothing() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Arc::new(Inflight::default());
        let gate = sink.gate("a1");
        let post = source("m1", &["a1"]);

        let first = tokio::spawn({
            let (posts, sink, inflight, post) = (
                db.posts.clone(),
                sink.clone(),
                Arc::clone(&inflight),
                post.clone(),
            );
            let series_id = db.series_id;
            async move {
                let request = ArchiveRequest {
                    series_id,
                    day: 5,
                    source: &post,
                    mode: Mode::Insert,
                    now: 200,
                };
                archive_core(&posts, &sink, &inflight, &request).await
            }
        });
        sink.wait_started(1).await;

        // The same post again, to the same day and to another one.
        for day in [5, 6] {
            let again = run(&db, &sink, &inflight, &post, day, Mode::Insert).await;
            assert!(matches!(again, ArchiveOutcome::Busy(Busy::ThisPost)));
        }
        // Another post aimed at the day being written.
        let other = source("m2", &["b1"]);
        let clash = run(&db, &sink, &inflight, &other, 5, Mode::Insert).await;
        assert!(matches!(clash, ArchiveOutcome::Busy(Busy::ThisDay)));

        // None of them uploaded or deleted anything.
        assert_eq!(sink.store_calls_begun(), 0);
        assert!(sink.deleted().is_empty());

        gate.add_permits(1);
        let done = archived(first.await.unwrap().unwrap());
        assert_eq!(done.files, 1);
        for key in db.posts.storage_keys(db.series_id, 5).await.unwrap() {
            assert!(sink.has(&key), "{key} was removed");
        }
        assert!(sink.deleted().is_empty());

        // The guard is released: the next attempt gets a real answer.
        let later = run(&db, &sink, &inflight, &post, 5, Mode::Insert).await;
        assert!(matches!(later, ArchiveOutcome::AlreadyArchived));
    }

    #[tokio::test]
    async fn losing_the_insert_never_deletes_files_another_day_points_at() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        let post = source("m1", &["a1", "a2"]);

        // Day 5, then renumbered to Day 6: its keys still say "d/5".
        archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        db.posts.move_day(db.series_id, 5, 6).await.unwrap();
        let keys = db.posts.storage_keys(db.series_id, 6).await.unwrap();
        assert_eq!(keys.len(), 4);

        // The same post as Day 5 again writes those very keys. While it
        // uploads, something else takes Day 5, so the insert is lost.
        let rival = Post {
            series_id: db.series_id,
            day: 5,
            message_id: "imported".to_owned(),
            channel_id: "c".to_owned(),
            caption: String::new(),
            posted_at: 1,
            archived_at: 1,
        };
        sink.race_with(&db.posts, rival).await;
        let outcome = run(&db, &sink, &inflight, &post, 5, Mode::Insert).await;
        assert!(
            matches!(&outcome, ArchiveOutcome::DayTaken(holder) if holder.message_id == "imported"),
            "{outcome:?}"
        );

        // Day 6 still has every file.
        assert!(sink.deleted().is_empty());
        for key in &keys {
            assert!(sink.has(key), "{key} was removed");
        }
        assert_eq!(db.message_at(6).await.as_deref(), Some("m1"));
    }

    #[tokio::test]
    async fn the_same_post_on_the_same_day_is_a_no_op() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        let post = source("m1", &["a1"]);

        archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        sink.wait_started(1).await;

        for mode in [Mode::Insert, Mode::Replace] {
            let again = run(&db, &sink, &inflight, &post, 5, mode).await;
            assert!(matches!(again, ArchiveOutcome::AlreadyArchived));
        }
        assert_eq!(sink.store_calls_begun(), 0, "nothing was uploaded again");
        assert!(sink.deleted().is_empty());
        assert_eq!(sink.object_count(), 2);
    }

    #[tokio::test]
    async fn a_day_held_by_another_post_is_reported_before_any_upload() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();

        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );
        sink.wait_started(1).await;

        let outcome = run(
            &db,
            &sink,
            &inflight,
            &source("m2", &["b1"]),
            5,
            Mode::Insert,
        )
        .await;
        assert!(
            matches!(&outcome, ArchiveOutcome::DayTaken(holder) if holder.message_id == "m1"),
            "{outcome:?}"
        );
        assert_eq!(sink.store_calls_begun(), 0);
        assert_eq!(db.message_at(5).await.as_deref(), Some("m1"));
    }

    #[tokio::test]
    async fn a_storage_failure_abandons_the_post_and_removes_what_it_uploaded() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        sink.fail("a2", Fault::Storage);

        let post = source("m1", &["a1", "a2", "a3"]);
        let failure = failed(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        assert!(failure.retryable);
        assert!(failure.storage);
        assert_eq!(failure.notes.len(), 1);
        assert!(
            !failure.notes.concat().contains("disk full"),
            "raw error leaked"
        );

        // Nothing is archived and nothing is left in storage, including the
        // original that landed before the failure.
        assert_eq!(db.message_at(5).await, None);
        assert_eq!(sink.object_count(), 0);

        // The retry works once storage is back.
        sink.heal("a2");
        let done = archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        assert_eq!(done.files, 3);
        assert_eq!(sink.object_count(), 6);
    }

    #[tokio::test]
    async fn a_failed_attempt_keeps_files_a_moved_day_still_uses() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        let post = source("m1", &["a1", "a2"]);

        archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        db.posts.move_day(db.series_id, 5, 6).await.unwrap();
        let keys = db.posts.storage_keys(db.series_id, 6).await.unwrap();

        // Archiving it as Day 5 again touches the same keys, then fails.
        sink.fail("a2", Fault::Fetch);
        let failure = failed(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        assert!(failure.retryable);
        assert!(!failure.storage);

        assert!(sink.deleted().is_empty());
        for key in &keys {
            assert!(sink.has(key), "{key} was removed");
        }
    }

    #[tokio::test]
    async fn a_file_that_can_never_be_archived_is_skipped_and_the_rest_archive() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        sink.fail("a2", Fault::Gone);

        let post = source("m1", &["a1", "a2", "a3"]);
        let done = archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        assert_eq!(done.files, 2);
        assert_eq!(done.skipped.len(), 1);
        assert!(done.skipped.concat().contains("a2.png"));
        assert_eq!(db.attachments_at(5).await, ["a1", "a3"]);
    }

    #[tokio::test]
    async fn nothing_is_written_when_every_file_is_skipped() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        sink.fail("a1", Fault::Gone);
        sink.fail("a2", Fault::Gone);

        let post = source("m1", &["a1", "a2"]);
        let failure = failed(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        assert!(!failure.retryable);
        assert_eq!(failure.notes.len(), 2);
        assert_eq!(db.message_at(5).await, None);
        assert_eq!(sink.object_count(), 0);
    }

    #[tokio::test]
    async fn files_are_stored_three_at_a_time_and_kept_in_attachment_order() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Arc::new(Inflight::default());
        let ids = ["a1", "a2", "a3", "a4", "a5"];
        let gates = ids.map(|id| sink.gate(id));
        let post = source("m1", &ids);

        let task = tokio::spawn({
            let (posts, sink, inflight) = (db.posts.clone(), sink.clone(), Arc::clone(&inflight));
            let series_id = db.series_id;
            async move {
                let request = ArchiveRequest {
                    series_id,
                    day: 5,
                    source: &post,
                    mode: Mode::Insert,
                    now: 200,
                };
                archive_core(&posts, &sink, &inflight, &request).await
            }
        });

        // Three uploads run; the other two wait for a free slot.
        sink.wait_started(3).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.store_calls_begun(), 0, "a fourth upload started early");

        // Finish them back to front, so the first file is the last one done.
        for gate in gates.iter().rev() {
            gate.add_permits(1);
        }
        let done = archived(task.await.unwrap().unwrap());

        assert_eq!(done.files, 5);
        assert_eq!(done.replaced, None);
        assert_eq!(db.attachments_at(5).await, ids);
        assert_eq!(
            sink.0.most_active.load(Ordering::SeqCst),
            UPLOAD_CONCURRENCY
        );
    }

    #[tokio::test]
    async fn replace_swaps_the_entry_and_frees_the_old_files() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();

        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );
        let old_keys = db.posts.storage_keys(db.series_id, 5).await.unwrap();

        let done = archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m2", &["b1"]),
                5,
                Mode::Replace,
            )
            .await,
        );
        assert_eq!(done.files, 1);
        // The post that held the day, so its mark can come off.
        assert_eq!(
            done.replaced.as_ref().map(|p| p.message_id.as_str()),
            Some("m1")
        );
        assert_eq!(db.message_at(5).await.as_deref(), Some("m2"));
        assert_eq!(db.attachments_at(5).await, ["b1"]);
        for key in &old_keys {
            assert!(!sink.has(key), "{key} was left behind");
        }
        for key in db.posts.storage_keys(db.series_id, 5).await.unwrap() {
            assert!(sink.has(&key), "{key} is missing");
        }
    }

    #[tokio::test]
    async fn replace_waits_for_an_archive_of_the_post_it_would_remove() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );
        let keys = db.posts.storage_keys(db.series_id, 5).await.unwrap();

        // m1 is being archived again (as another day, on another device):
        // its files may be the ones Day 5's entry points at.
        let elsewhere = inflight
            .try_begin(&[FlightKey::Message(db.series_id, "m1".to_owned())])
            .unwrap();
        let blocked = run(
            &db,
            &sink,
            &inflight,
            &source("m2", &["b1"]),
            5,
            Mode::Replace,
        )
        .await;
        assert!(matches!(blocked, ArchiveOutcome::Busy(Busy::ThisDay)));
        // Nothing was uploaded, replaced or deleted, and nothing stays held.
        assert_eq!(sink.store_calls_begun(), 1);
        assert!(sink.deleted().is_empty());
        assert_eq!(db.message_at(5).await.as_deref(), Some("m1"));
        for key in &keys {
            assert!(sink.has(key), "{key} is missing");
        }
        assert!(!inflight.is_held(&FlightKey::Day(db.series_id, 5)));

        // Once that archive is done, the replace goes through.
        drop(elsewhere);
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m2", &["b1"]),
                5,
                Mode::Replace,
            )
            .await,
        );
        assert_eq!(db.message_at(5).await.as_deref(), Some("m2"));
        assert!(!inflight.is_held(&FlightKey::Message(db.series_id, "m1".to_owned())));
    }

    #[test]
    fn the_public_announcement_shows_the_series_name_as_typed() {
        let mut series = series(1, "[free nitro](https://x.example) a**b", &[]);
        let text = announcement_text(&series, Milestone::First, 1);
        // No masked link and no broken bold: the name is escaped.
        assert!(text.starts_with(r"🌱 **\[free nitro\]("), "{text}");
        assert!(text.contains(r"a\*\*b** by "), "{text}");
        assert!(!text.contains("[free nitro]("), "{text}");
        assert!(text.contains(&format!("<@{}>", series.creator_id)));

        // The creator's own template gets the same escaped name.
        series.milestone_template = Some("{name} hit Day {day}".to_owned());
        let text = announcement_text(&series, Milestone::Hundred(100), 100);
        assert!(text.starts_with(r"\[free nitro\]"), "{text}");
        assert!(text.ends_with("hit Day 100"), "{text}");
    }

    #[test]
    fn private_replies_show_the_series_name_as_typed_too() {
        let named = series(1, "a**b", &[]);
        assert!(promotion_line(&named).contains(r"**a\*\*b**"));
        let warning = gap_warning(&named, Some(1), 500, DayUse::Archive).unwrap();
        assert!(warning.contains(r"**a\*\*b**"), "{warning}");
        let revoked = no_series_text(std::slice::from_ref(&named));
        assert!(revoked.contains(r"**a\*\*b**"), "{revoked}");
    }

    #[test]
    fn another_member_s_name_shows_as_typed_and_cannot_format_the_reply() {
        let masked = "[click](https://x.example)";
        for mode in [Mode::Insert, Mode::Replace] {
            let line = archived_line("daily", 4, 1, Some(masked), mode);
            assert!(
                line.contains(r"\[click\](https:\//x.example)'s post"),
                "{line}"
            );
            assert!(!line.contains(masked), "{line}");
        }
        let line = archived_line("daily", 4, 1, Some("a**b||c`d"), Mode::Insert);
        assert!(line.contains(r"from a\*\*b\|\|c\`d's post"), "{line}");
        assert_eq!(
            pick_series_text(Some(masked)),
            r"Which series is \[click\](https:\//x.example)'s post for?"
        );
        // An ordinary name is untouched, and no author means no name.
        assert_eq!(
            archived_line("daily", 4, 2, Some("Sam"), Mode::Insert),
            "🍃 Day 4 of **daily** archived from Sam's post (2 files)."
        );
        assert_eq!(
            pick_series_text(Some("Sam")),
            "Which series is Sam's post for?"
        );
        assert_eq!(pick_series_text(None), "Which series is this post for?");
        // The modal title shows text literally: no backslashes there.
        assert!(!modal_title("daily", Some("a*b")).contains('\\'));
    }

    #[tokio::test]
    async fn replace_keeps_the_old_entry_when_the_upload_fails() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();

        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );
        let old_keys = db.posts.storage_keys(db.series_id, 5).await.unwrap();

        sink.fail("b1", Fault::Fetch);
        let failure = failed(
            run(
                &db,
                &sink,
                &inflight,
                &source("m2", &["b1"]),
                5,
                Mode::Replace,
            )
            .await,
        );
        assert!(failure.retryable);
        assert_eq!(db.message_at(5).await.as_deref(), Some("m1"));
        for key in &old_keys {
            assert!(sink.has(key), "{key} was removed");
        }
    }

    // -- remove and move ------------------------------------------------------

    #[tokio::test]
    async fn remove_deletes_the_entry_and_its_files() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );

        let removal = remove_core(&db.posts, &sink, &inflight, db.series_id, 5, "m1")
            .await
            .unwrap();
        assert_eq!(removal, Removal::Removed);
        assert_eq!(db.message_at(5).await, None);
        assert_eq!(sink.object_count(), 0);
    }

    #[tokio::test]
    async fn remove_leaves_an_entry_that_belongs_to_another_post() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m2", &["b1"]),
                5,
                Mode::Insert,
            )
            .await,
        );

        // An Undo pressed late: Day 5 is another post's entry by now.
        for day in [5, 9] {
            let removal = remove_core(&db.posts, &sink, &inflight, db.series_id, day, "m1")
                .await
                .unwrap();
            assert_eq!(removal, Removal::NotThisPost);
        }
        assert_eq!(db.message_at(5).await.as_deref(), Some("m2"));
        assert_eq!(sink.object_count(), 2);
    }

    #[tokio::test]
    async fn remove_keeps_files_a_moved_day_shares() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        let post = source("m1", &["a1"]);

        // Day 5 moved to 6, then the same post archived as Day 5 again:
        // both days point at the same two objects.
        archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        db.posts.move_day(db.series_id, 5, 6).await.unwrap();
        archived(run(&db, &sink, &inflight, &post, 5, Mode::Insert).await);
        assert_eq!(sink.object_count(), 2);

        let removal = remove_core(&db.posts, &sink, &inflight, db.series_id, 5, "m1")
            .await
            .unwrap();
        assert_eq!(removal, Removal::Removed);
        assert_eq!(sink.object_count(), 2, "Day 6 lost its files");

        // Removing the last entry that uses them frees them.
        let removal = remove_core(&db.posts, &sink, &inflight, db.series_id, 6, "m1")
            .await
            .unwrap();
        assert_eq!(removal, Removal::Removed);
        assert_eq!(sink.object_count(), 0);
    }

    #[tokio::test]
    async fn remove_and_move_wait_for_a_running_archive() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );

        let _running = inflight
            .try_begin(&[FlightKey::Message(db.series_id, "m1".to_owned())])
            .unwrap();
        let removal = remove_core(&db.posts, &sink, &inflight, db.series_id, 5, "m1")
            .await
            .unwrap();
        assert_eq!(removal, Removal::Busy);
        let moved = move_core(&db.posts, &inflight, db.series_id, (5, 6), "m1")
            .await
            .unwrap();
        assert_eq!(moved, Move::Busy);
        assert_eq!(db.message_at(5).await.as_deref(), Some("m1"));
    }

    #[tokio::test]
    async fn a_moved_entry_is_measured_against_the_other_days() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        let max_without = |day| max_day_without(&db.posts, db.series_id, day);
        assert_eq!(max_without(5).await.unwrap(), None);

        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );
        // The only entry: there is nothing else to measure against.
        assert_eq!(max_without(5).await.unwrap(), None);

        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m2", &["b1"]),
                7,
                Mode::Insert,
            )
            .await,
        );
        // The highest entry is measured against the one below it.
        assert_eq!(max_without(7).await.unwrap(), Some(5));
        assert_eq!(max_without(5).await.unwrap(), Some(7));
        assert_eq!(max_without(9).await.unwrap(), Some(7));
    }

    #[tokio::test]
    async fn move_renumbers_only_this_posts_entry_into_a_free_day() {
        let db = TestDb::new().await;
        let sink = FakeSink::new();
        let inflight = Inflight::default();
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m1", &["a1"]),
                5,
                Mode::Insert,
            )
            .await,
        );
        archived(
            run(
                &db,
                &sink,
                &inflight,
                &source("m2", &["b1"]),
                7,
                Mode::Insert,
            )
            .await,
        );

        let go = |from, to, message: &'static str| {
            let (posts, inflight, series_id) = (&db.posts, &inflight, db.series_id);
            async move {
                move_core(posts, inflight, series_id, (from, to), message)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(go(5, 7, "m1").await, Move::Taken);
        assert_eq!(go(7, 8, "m1").await, Move::NotThisPost);
        assert_eq!(go(3, 4, "m1").await, Move::NotThisPost);
        assert_eq!(go(5, 6, "m1").await, Move::Moved);

        assert_eq!(db.message_at(5).await, None);
        assert_eq!(db.message_at(6).await.as_deref(), Some("m1"));
        assert_eq!(db.message_at(7).await.as_deref(), Some("m2"));
        // The files moved with the entry.
        assert_eq!(sink.object_count(), 4);
        assert!(sink.deleted().is_empty());
    }

    // -- reading the message --------------------------------------------------

    #[test]
    fn files_are_classified_before_the_modal() {
        assert_eq!(
            classify_file("a.png", Some("image/png"), 10),
            Ok("image/png")
        );
        // No declared type: the extension decides.
        assert_eq!(classify_file("clip.MOV", None, 10), Ok("video/quicktime"));
        assert_eq!(
            classify_file("a.jpg", Some("image/jpeg; charset=binary"), 10),
            Ok("image/jpeg")
        );

        let unsupported = classify_file("IMG_2041.heic", Some("image/heic"), 10).unwrap_err();
        assert!(unsupported.contains("`IMG_2041.heic`"));
        assert!(unsupported.contains(media::SUPPORTED_FORMATS));

        assert!(classify_file("big.mp4", Some("video/mp4"), DEFAULT_MAX_BYTES).is_ok());
        let too_large =
            classify_file("big.mp4", Some("video/mp4"), DEFAULT_MAX_BYTES + 1).unwrap_err();
        assert!(too_large.contains("`big.mp4`"));
        assert!(too_large.contains("100 MB"));
    }

    #[test]
    fn watched_channels_cover_threads_through_their_parent() {
        let mut settings = GuildSettings::defaults_for("g");
        settings.watched_channels = vec!["10".to_owned(), "11".to_owned()];

        assert!(channel_watched(&settings, "10", None));
        assert!(!channel_watched(&settings, "99", None));
        // A thread under a watched channel.
        assert!(channel_watched(&settings, "500", Some("11")));
        assert!(!channel_watched(&settings, "500", Some("99")));
    }

    #[test]
    fn target_series_is_settled_without_asking_when_it_can_be() {
        let one = [series(1, "Sketches", &["10"])];
        // An only series needs no channel match.
        assert_eq!(resolve_target(&one, "99", None).map(|s| s.id), Some(1));

        let three = [
            series(1, "Sketches", &["10"]),
            series(2, "Photos", &["20"]),
            series(3, "Ink", &["20"]),
        ];
        assert_eq!(resolve_target(&three, "10", None).map(|s| s.id), Some(1));
        // A thread under the series' channel.
        assert_eq!(
            resolve_target(&three, "500", Some("10")).map(|s| s.id),
            Some(1)
        );
        // Two series share the channel, or none is bound to it: ask.
        assert_eq!(resolve_target(&three, "20", None).map(|s| s.id), None);
        assert_eq!(resolve_target(&three, "30", None).map(|s| s.id), None);
        assert_eq!(resolve_target(&[], "10", None).map(|s| s.id), None);
    }

    // -- copy ---------------------------------------------------------------

    #[test]
    fn modal_text_fits_discords_limits() {
        let long_name = "é".repeat(40);
        for title in [
            modal_title("Daily Sketch", None),
            modal_title(&long_name, None),
            modal_title(&long_name, Some("A very long display name indeed")),
            modal_title("", Some("")),
        ] {
            assert!(title.chars().count() <= MODAL_TEXT_MAX_CHARS, "{title}");
        }
        assert_eq!(modal_title("Daily Sketch", None), "Archive to Daily Sketch");
        assert_eq!(
            modal_title("Daily Sketch", Some("Mika")),
            "Mika's post to Daily Sketch"
        );

        for label in [
            day_label(DayHint::Next, parser::MAX_DAY),
            day_label(DayHint::FromText, parser::MAX_DAY),
            day_label(DayHint::Taken(parser::MAX_DAY), parser::MAX_DAY),
            day_label(DayHint::Current(parser::MAX_DAY), parser::MAX_DAY),
        ] {
            assert!(label.chars().count() <= MODAL_TEXT_MAX_CHARS, "{label}");
        }
        assert_eq!(day_label(DayHint::Next, 43), "Day number (next is 43)");
        assert_eq!(
            day_label(DayHint::Taken(42), 44),
            "Day number (Day 42 is taken)"
        );
    }

    #[test]
    fn a_day_named_in_the_post_is_used_only_when_the_field_accepts_it() {
        assert_eq!(usable_suggestion(Some(42)), Some(42));
        assert_eq!(
            usable_suggestion(Some(parser::MAX_DAY)),
            Some(parser::MAX_DAY)
        );
        assert_eq!(usable_suggestion(None), None);
        // "Day 0" would pre-fill a value the same modal rejects.
        assert!(parser::parse_day_input("0").is_err());
        assert_eq!(usable_suggestion(Some(0)), None);
        assert_eq!(usable_suggestion(Some(parser::MAX_DAY + 1)), None);
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("short", 10), "short");
        assert_eq!(truncate_chars("exactly10!", 10), "exactly10!");
        assert_eq!(truncate_chars("日本語のシリーズ名", 5), "日本語の…");
        assert_eq!(truncate_chars("abc", 0), "…");
        assert_eq!(
            clamp_message(&"x".repeat(5000)).chars().count(),
            MESSAGE_MAX_CHARS
        );
    }

    #[test]
    fn far_off_days_are_questioned_and_near_ones_are_not() {
        let mut sketches = series(1, "Sketches", &[]);

        // The next day, a small skip, and any backfill pass.
        for (max, day) in [
            (Some(42), 43),
            (Some(42), 49),
            (Some(42), 7),
            (None, 1),
            (None, 7),
        ] {
            for using in [DayUse::Archive, DayUse::Move] {
                assert_eq!(
                    gap_warning(&sketches, max, day, using),
                    None,
                    "{max:?} -> {day}"
                );
            }
        }

        // 430 typed for 43.
        let typo = gap_warning(&sketches, Some(42), 430, DayUse::Archive).unwrap();
        assert!(typo.contains("skip 387 days after Day 42"), "{typo}");
        assert!(
            typo.ends_with("Archive this post as Day 430 anyway?"),
            "{typo}"
        );
        assert!(gap_warning(&sketches, Some(42), 50, DayUse::Archive).is_some());
        // The same slip in Change day.
        let moved = gap_warning(&sketches, Some(42), 430, DayUse::Move).unwrap();
        assert!(moved.contains("skip 387 days after Day 42"), "{moved}");
        assert!(
            moved.ends_with("Change this post to Day 430 anyway?"),
            "{moved}"
        );
        // An empty series far from its first day; for a move, a series
        // whose only entry is the one being moved.
        let empty = gap_warning(&sketches, None, 300, DayUse::Archive).unwrap();
        assert!(empty.contains("no days yet and starts at Day 1"), "{empty}");
        let only = gap_warning(&sketches, None, 300, DayUse::Move).unwrap();
        assert!(only.contains("no other days and starts at Day 1"), "{only}");

        // Below the series' first day.
        sketches.start_day = 100;
        let early = gap_warning(&sketches, Some(120), 3, DayUse::Archive).unwrap();
        assert!(early.contains("before Day 100"), "{early}");
        assert!(gap_warning(&sketches, Some(120), 3, DayUse::Move).is_some());
        assert_eq!(gap_warning(&sketches, None, 100, DayUse::Archive), None);
    }

    #[test]
    fn result_and_progress_lines_read_naturally() {
        assert_eq!(files_phrase(1), "1 file");
        assert_eq!(files_phrase(3), "3 files");
        assert_eq!(
            archived_line("X", 42, 1, None, Mode::Insert),
            "🍃 Day 42 of **X** archived (1 file)."
        );
        assert_eq!(
            archived_line("X", 42, 3, Some("Mika"), Mode::Insert),
            "🍃 Day 42 of **X** archived from Mika's post (3 files)."
        );
        assert_eq!(
            archived_line("X", 42, 2, None, Mode::Replace),
            "🍃 Day 42 of **X** replaced with this post (2 files)."
        );

        let photo = [file("a1")];
        assert_eq!(
            progress_text("X", 42, &photo),
            "🍃 Archiving Day 42 of **X** (1 file)…"
        );
        let video = [SourceFile {
            content_type: "video/mp4",
            ..file("v1")
        }];
        assert!(progress_text("X", 42, &video).ends_with("Videos can take a minute."));
    }

    #[test]
    fn skipped_files_are_listed_up_to_a_cap() {
        let notes: Vec<String> = (1..=7).map(|n| format!("note {n}")).collect();
        let lines = skipped_lines(&notes);
        assert_eq!(lines.len(), NOTES_SHOWN_MAX + 1);
        assert_eq!(lines.first().map(String::as_str), Some("Skipped: note 1"));
        assert_eq!(
            lines.last().map(String::as_str),
            Some("…and 2 more skipped.")
        );
        assert_eq!(skipped_lines(notes.get(..2).unwrap()).len(), 2);
        assert!(skipped_lines(&[]).is_empty());
    }

    #[test]
    fn no_series_reply_tells_revoked_from_never_created() {
        let none = no_series_text(&[]);
        assert!(none.contains("don't have a series yet"));

        let one = no_series_text(&[series(1, "Sketches", &[])]);
        assert!(one.contains("revoked **Sketches**"));
        assert!(one.contains("ask an admin to restore it"));
        assert!(!one.contains("don't have a series yet"));

        let two = no_series_text(&[series(1, "Sketches", &[]), series(2, "Ink", &[])]);
        assert!(two.contains("**Sketches**, **Ink**"));
    }

    #[test]
    fn refusals_say_what_to_do_next() {
        let watched = vec!["10".to_owned(), "11".to_owned()];
        let member = not_watched_text(&watched, false);
        assert!(member.contains("<#10>, <#11>"));
        assert!(member.contains("ask a server admin"));
        let admin = not_watched_text(&watched, true);
        assert!(admin.contains("add this channel with `/setup`"));
        assert!(!admin.contains("ask a server admin"));
        let many: Vec<String> = (0..30).map(|n| n.to_string()).collect();
        assert!(not_watched_text(&many, false).chars().count() < 500);
        // No channel picked at all: no empty list, and the way to fix it.
        let unset = not_watched_text(&[], false);
        assert!(unset.contains("no series channels yet"), "{unset}");
        assert!(!unset.contains(": ."), "{unset}");
        assert!(not_watched_text(&[], true).contains("Pick them with `/setup`"));

        // Skipped files are named; a link preview gets its own explanation.
        let skipped = nothing_to_archive_text(&["`a.heic` isn't supported.".to_owned()], true);
        assert!(skipped.contains("Skipped: `a.heic` isn't supported."));
        assert!(nothing_to_archive_text(&[], true).contains("link preview or GIF"));
        assert!(nothing_to_archive_text(&[], false).contains(media::SUPPORTED_FORMATS));
    }

    #[test]
    fn existing_and_duplicate_notices_name_the_day_and_the_holder() {
        let one = [("X".to_owned(), 42)];
        assert!(existing_text(&one, false).starts_with("🍃 This post is already Day 42 of **X**."));
        assert!(existing_text(&one, true).contains("so nothing changed"));
        assert!(existing_text(&one, false).contains("remove this entry"));

        let two = [("X".to_owned(), 42), ("X".to_owned(), 43)];
        let text = existing_text(&two, false);
        assert!(text.contains("Day 42 of **X** and Day 43 of **X**"));
        assert!(text.contains("Remove Archive Entry"));
        let three = [
            ("X".to_owned(), 1),
            ("Y".to_owned(), 2),
            ("Z".to_owned(), 3),
        ];
        assert_eq!(
            entries_phrase(&three),
            "Day 1 of **X**, Day 2 of **Y** and Day 3 of **Z**"
        );

        let link = jump_link("1", "2", "3");
        assert_eq!(link, "https://discord.com/channels/1/2/3");
        let dup = duplicate_text("X", 42, &link);
        assert!(dup.contains("[another post](https://discord.com/channels/1/2/3)"));
        assert!(dup.contains("Replacing deletes"));
        // The creator's own series: never "someone else".
        assert!(!dup.contains("someone else"));
    }

    #[test]
    fn failures_say_whether_to_retry_and_never_claim_a_partial_archive() {
        let retryable = Failure {
            notes: vec![
                "`a.png` couldn't be downloaded from Discord. Try again in a moment.".into(),
            ],
            retryable: true,
            storage: false,
        };
        let text = failure_text(&retryable, &["`b.heic` isn't supported.".into()]);
        assert!(text.starts_with("🍂 `a.png` couldn't be downloaded"));
        assert!(text.ends_with("Nothing was archived."));

        let interrupted = Failure {
            notes: vec![],
            retryable: true,
            storage: false,
        };
        assert!(failure_text(&interrupted, &[]).contains("Try again"));

        // Every file skipped: the notes from before and during the upload.
        let hopeless = Failure {
            notes: vec!["`c.png` is no longer available on Discord.".into()],
            retryable: false,
            storage: false,
        };
        let text = failure_text(&hopeless, &["`b.heic` isn't supported.".into()]);
        assert!(text.starts_with("🍂 Nothing was archived"));
        assert!(text.contains("Skipped: `b.heic` isn't supported."));
        assert!(text.contains("Skipped: `c.png` is no longer available on Discord."));

        assert!(busy_text(Busy::ThisPost, "X", 5).contains("still archiving this post"));
        assert!(STILL_ARCHIVING.contains("check again"));
        assert!(busy_text(Busy::ThisDay, "X", 5).contains("Day 5 of **X**"));
    }

    #[test]
    fn sprout_copy_follows_the_series_privacy() {
        let mut sketches = series(1, "Sketches", &[]);
        assert!(promotion_line(&sketches).contains("everyone in this server"));

        sketches.privacy = Privacy::RoleGated;
        sketches.privacy_role_id = Some("77".to_owned());
        let gated = promotion_line(&sketches);
        assert!(gated.contains("members with <@&77>"));
        assert!(!gated.contains("everyone"));

        // A stored role that is not an id is never written as a mention.
        sketches.privacy_role_id = Some("[x](https://y.example)".to_owned());
        let unchecked = promotion_line(&sketches);
        assert!(unchecked.contains("members with its role"), "{unchecked}");
        assert!(!unchecked.contains("https://"), "{unchecked}");

        sketches.privacy = Privacy::CreatorOnly;
        let private = promotion_line(&sketches);
        assert!(private.contains("other members still can't see it"));
        assert!(!private.contains("public"));
        assert!(!private.contains("everyone"));

        assert_eq!(
            sprout_progress_line("Sketches", 2, 5),
            "🌱 **Sketches** is a sprout: 2 of 5 days archived. Until then only you can see \
             it in the gallery (server admins see it in the admin panel and in chat commands)."
        );
    }

    #[test]
    fn private_milestones_have_copy_for_every_kind() {
        assert!(private_milestone_line(Milestone::First, 1, "X").contains("Day 1 is archived"));
        assert!(private_milestone_line(Milestone::Hundred(200), 200, "X").contains("Day 200"));
        assert!(private_milestone_line(Milestone::Years(1), 365, "X").contains("one year"));
        assert!(private_milestone_line(Milestone::Years(2), 730, "X").contains("2 years"));
    }

    #[test]
    fn log_lines_name_the_actor_and_hide_private_series() {
        let actor = serenity::UserId::new(7);
        let sketches = series(1, "Sketches", &[]);
        assert_eq!(
            log_text(LogKind::Archived { files: 1 }, &sketches, 42, actor, "LINK"),
            "🍃 **Sketches** · Day 42 archived by <@7> · 1 file · LINK"
        );
        assert_eq!(
            log_text(LogKind::Moved { from: 41 }, &sketches, 42, actor, ""),
            "✏️ **Sketches** · Day 41 renumbered to Day 42 by <@7>"
        );
        assert!(
            log_text(LogKind::Replaced { files: 2 }, &sketches, 42, actor, "LINK")
                .contains("replaced by <@7> · 2 files")
        );
        assert!(log_text(LogKind::Removed, &sketches, 42, actor, "").contains("removed by <@7>"));

        // Named again in the line that says it left its sprout stage.
        assert_eq!(
            log_text(LogKind::Sprouted, &sketches, 42, actor, "LINK"),
            "🌿 **Sketches** · out of its sprout stage"
        );

        // Hidden: a series that is not public, or a public one that is still
        // a sprout (only its creator and admins can see it).
        let mut sprout = sketches.clone();
        sprout.state = SeriesState::Sprout;
        let mut hidden = vec![sprout];
        for privacy in [Privacy::CreatorOnly, Privacy::RoleGated] {
            let mut private = sketches.clone();
            private.privacy = privacy;
            hidden.push(private);
        }
        for unlisted in &hidden {
            for kind in [
                LogKind::Archived { files: 1 },
                LogKind::Replaced { files: 1 },
                LogKind::Removed,
                LogKind::Moved { from: 1 },
                LogKind::Sprouted,
                LogKind::StorageFailed,
            ] {
                let line = log_text(kind, unlisted, 42, actor, "LINK");
                assert!(!line.contains("Sketches"), "{line}");
            }
        }
    }

    #[test]
    fn reaction_is_the_whole_emoji_or_the_leaf() {
        let unicode = |text: &str| serenity::ReactionType::Unicode(text.to_owned());
        // Several code points each: a flag, a skin tone, a joined family.
        for emoji in ["🇯🇵", "👍🏽", "👨‍👩‍👧", "1️⃣", "🌸"] {
            assert_eq!(reaction_for(emoji), unicode(emoji));
        }
        assert_eq!(reaction_for(" 🌸 "), unicode("🌸"));
        assert_eq!(reaction_for(""), unicode(FALLBACK_REACTION));
        assert_eq!(reaction_for("   "), unicode(FALLBACK_REACTION));
    }

    // -- ids and controls -----------------------------------------------------

    fn every_stage() -> Vec<Stage> {
        let day = parser::MAX_DAY;
        vec![
            Stage::Idle,
            Stage::NoSeries,
            Stage::PickSeries { options: vec![] },
            Stage::Existing {
                entries: vec![(1, day)],
            },
            Stage::Existing {
                entries: vec![(1, 1), (1, 2)],
            },
            Stage::ConfirmRemove {
                series_id: 1,
                day,
                back: vec![(1, day)],
            },
            Stage::DayProblem { series_id: 1 },
            Stage::Gap { series_id: 1, day },
            Stage::Duplicate {
                series_id: 1,
                day,
                next_free: day,
            },
            Stage::Retry {
                series_id: 1,
                day,
                mode: Mode::Insert,
                change_day: true,
            },
            Stage::Done {
                series_id: 1,
                day,
                undo: true,
                promoted: false,
            },
            Stage::MoveGap {
                series_id: 1,
                from: day,
                to: day,
                undo: true,
                promoted: true,
            },
            Stage::Removed { series_id: 1 },
            Stage::StillArchiving,
        ]
    }

    #[test]
    fn controls_fit_one_row_and_discords_limits() {
        let prefix = session_prefix(u64::MAX);
        for stage in every_stage() {
            let row = controls(&stage);
            assert!(row.len() <= 5, "{stage:?} has {} buttons", row.len());
            for control in &row {
                assert!(
                    control.label.chars().count() <= BUTTON_LABEL_MAX_CHARS,
                    "{}",
                    control.label
                );
                let id = component_id(&prefix, u32::MAX, control.action);
                assert!(id.len() <= 100, "{id}");
            }
            // No action appears twice: a press must mean one thing.
            let mut actions: Vec<&str> = row.iter().map(|c| c.action.as_str()).collect();
            actions.sort_unstable();
            actions.dedup();
            assert_eq!(actions.len(), row.len(), "{stage:?}");
        }
        assert!(modal_id(&prefix, u64::MAX).len() <= 100);
        assert!(pick_id(&prefix, u32::MAX, i64::MIN).len() <= 100);
    }

    #[test]
    fn destructive_and_one_off_controls_appear_only_where_they_apply() {
        let actions = |stage: &Stage| -> Vec<Action> {
            controls(stage).into_iter().map(|c| c.action).collect()
        };
        // Remove is offered for a single entry only.
        let single = Stage::Existing {
            entries: vec![(1, 5)],
        };
        let several = Stage::Existing {
            entries: vec![(1, 5), (1, 6)],
        };
        assert!(actions(&single).contains(&Action::Remove));
        assert!(!actions(&several).contains(&Action::Remove));
        // So is Change day: it needs one entry to renumber.
        assert_eq!(
            actions(&single),
            [Action::Another, Action::Move, Action::Remove, Action::Open]
        );
        assert!(stage_allows(&single, Action::Move));
        assert!(!stage_allows(&several, Action::Move));
        // A day changed from there comes back as a plain result: nothing
        // to undo, nothing promoted.
        assert_eq!(
            done_at(&single, 1, 6),
            Stage::Done {
                series_id: 1,
                day: 6,
                undo: false,
                promoted: false,
            }
        );

        // No Undo after a replace: it could not bring the old entry back.
        let done = |undo| Stage::Done {
            series_id: 1,
            day: 5,
            undo,
            promoted: false,
        };
        assert!(actions(&done(true)).contains(&Action::Undo));
        assert!(!actions(&done(false)).contains(&Action::Undo));

        // The select is not a button but still belongs to its stage.
        let picker = Stage::PickSeries { options: vec![] };
        assert!(controls(&picker).is_empty());
        assert!(stage_allows(&picker, Action::Pick));
        assert!(!stage_allows(&picker, Action::Undo));
        assert!(!stage_allows(&Stage::Idle, Action::Pick));
        assert!(stage_allows(&done(true), Action::Undo));
        assert!(!stage_allows(&done(true), Action::Replace));

        // While an earlier try runs, looking again is the only way on.
        assert_eq!(actions(&Stage::StillArchiving), [Action::Recheck]);

        // A far-off Change day: go ahead, type another, or keep the day.
        let far = Stage::MoveGap {
            series_id: 1,
            from: 43,
            to: 430,
            undo: true,
            promoted: true,
        };
        assert_eq!(actions(&far), [Action::Confirm, Action::Move, Action::Keep]);
        let labels: Vec<String> = controls(&far).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["Yes, Day 430", "Change day", "Keep Day 43"]);
    }

    #[test]
    fn a_change_of_day_keeps_what_the_result_offers() {
        let done = Stage::Done {
            series_id: 1,
            day: 43,
            undo: true,
            promoted: true,
        };
        let far = Stage::MoveGap {
            series_id: 1,
            from: 43,
            to: 430,
            undo: true,
            promoted: true,
        };
        let expected = Stage::Done {
            series_id: 1,
            day: 44,
            undo: true,
            promoted: true,
        };
        assert_eq!(done_at(&done, 1, 44), expected);
        assert_eq!(done_at(&far, 1, 44), expected);
        // After a replace there is no Undo to keep.
        let replaced = Stage::Done {
            series_id: 1,
            day: 43,
            undo: false,
            promoted: false,
        };
        assert_eq!(result_extras(&replaced), (false, false));
        // From anywhere else, nothing is offered that was not there.
        assert_eq!(result_extras(&Stage::Idle), (false, false));
    }

    #[test]
    fn the_session_strips_its_controls_before_their_token_dies() {
        let now = tokio::time::Instant::now();
        let later = now + SESSION_IDLE;
        // The usual case: the token and the session end together.
        assert_eq!(wake_at(later, Some(later)), later);
        // A modal opened late: strip first, then wait for its submit.
        assert_eq!(wake_at(later, Some(now)), now);
        // Nothing on screen to edit: only the session's end.
        assert_eq!(wake_at(later, None), later);
    }

    #[test]
    fn component_ids_round_trip_and_belong_to_their_session() {
        let prefix = session_prefix(1234);
        assert!(prefix.starts_with("arc:"));
        // Never the prefix of the stateless, globally routed buttons.
        assert!(!prefix.starts_with("leaf:"));

        for action in Action::ALL {
            let id = component_id(&prefix, 7, action);
            assert_eq!(parse_component_id(&prefix, &id), Some((7, action)));
            // Another session's id is not ours.
            assert_eq!(parse_component_id(&session_prefix(1235), &id), None);
        }
        // A modal id shares the prefix but is not a component id.
        let modal = modal_id(&prefix, 99);
        assert!(modal.starts_with(&prefix));
        assert_eq!(parse_component_id(&prefix, &modal), None);
        assert_eq!(parse_component_id(&prefix, "confirm-yes"), None);
        assert_eq!(
            parse_component_id(&prefix, &format!("{prefix}7:nonsense")),
            None
        );
        // Each open of a modal has its own id.
        assert_ne!(modal_id(&prefix, 1), modal_id(&prefix, 2));

        // A picker button is a pick that carries its series.
        let pick = pick_id(&prefix, 7, 42);
        assert_eq!(parse_component_id(&prefix, &pick), Some((7, Action::Pick)));
        assert_eq!(pick_button_series(&prefix, &pick), Some(42));
        assert_eq!(pick_button_series(&session_prefix(1235), &pick), None);
        // Only a pick carries anything, and a plain pick carries no series.
        let undo = format!("{}:42", component_id(&prefix, 7, Action::Undo));
        assert_eq!(parse_component_id(&prefix, &undo), None);
        assert_eq!(pick_button_series(&prefix, &undo), None);
        let plain = component_id(&prefix, 7, Action::Pick);
        assert_eq!(pick_button_series(&prefix, &plain), None);
        assert_eq!(pick_button_series(&prefix, &format!("{pick}:9")), None);
    }
}
