//! `/export` and `/import`: a series' day index as a JSON file.
//!
//! The format is walpurgisbot-v2-compatible: one entry per archived day,
//! with its number, its date and the message it came from. It holds no
//! media and no captions, so an export is an index (not a backup) and an
//! import creates days without media. `leaf-migrate` is the tool that brings
//! files in: run with the same file, it stores the files of days that are
//! not archived yet and of days an import here left without any. The copy
//! says so before and after an import, and an import can be undone while
//! its result is on screen: the undo takes back exactly the days the import
//! wrote, and only while they are still as it wrote them.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use leaf_core::db::{DbError, LaunchIntentRepo};
use leaf_core::domain::{
    ExportRow, GuildSettings, NewMediaAttachment, Post, Privacy, Series, SeriesState,
};
use leaf_core::localtime;
use leaf_core::parser::MAX_DAY;
use leaf_core::transfer::{
    self, MAX_IMPORT_ENTRIES, MAX_MEDIA_PER_ENTRY, TransferParseError, TransferPost,
};
use poise::serenity_prelude as serenity;
use serenity::futures::{Stream, StreamExt as _};

use crate::commands::query::{begin, private_series};
use crate::commands::series_lookup::{
    Answer, App, Asker, PROMPT_TIMEOUT, Scope, Via, autocomplete_any_series, bold, clip,
    rerun_hint, scoped_id, settle,
};
use crate::components::{self, FlightKey, INFLIGHT, OPEN_GALLERY_FALLBACK};
use crate::{Context, Error, checks};

/// Import files larger than this are refused (matches v2's guard).
const MAX_IMPORT_BYTES: u32 = 24 * 1024 * 1024;
/// How long the download of the attached file may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// What some editors put in front of a UTF-8 file; not part of the JSON.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// 2015-01-01 UTC. Discord opened that year, so no post is older; the
/// gallery treats earlier dates as unknown for the same reason.
const EARLIEST_POST_UNIX: i64 = 1_420_070_400;
/// How far past now a post's date may lie (clock drift, timezones).
const FUTURE_SLACK_SECS: i64 = 86_400;

/// How long an import's result keeps its Undo import and Open gallery
/// buttons. The result is shown with the token of the Import press, which
/// lasts 15 minutes: that leaves room for the write before the window and
/// for taking the buttons off after it.
const UNDO_WINDOW: Duration = Duration::from_mins(10);

/// Discord's cap on a message's text.
const MESSAGE_MAX_CHARS: usize = 2_000;
/// Room kept in a full message for the line that counts left-out warnings.
const OVERFLOW_ROOM: usize = 60;
/// How much of a file name is echoed back.
const FILE_NAME_MAX_CHARS: usize = 60;
/// Longest series slug in an export's file name.
const SLUG_MAX_CHARS: usize = 40;

/// Names this module's prompts in their component ids (after the nonce).
const ID_KIND: &str = "import";

/// What an import brings in, and what it leaves out.
const IMPORT_SCOPE: &str = "This imports day numbers, dates and links to the original posts, \
     not images, videos or captions: in the gallery each imported day shows without media.";

// What the copy says about `leaf-migrate` is true of the tool as it is
// today: its importer gives a day that holds no stored file its files, as
// long as the day still holds the same message and that message still has
// them (leaf-migrate `importer::Run::repair_one`). The tests pin the wording.

/// How to get the files as well, said before the import.
const MIGRATE_FILLS: &str = "To bring the files in too, ask whoever hosts leaf to run \
     `leaf-migrate` with this file, before or after this import. It fetches them from the \
     original messages, for every day whose message still has them.";

/// The same, said after the import.
const MIGRATE_AFTER: &str = "To add the images, ask whoever hosts leaf to run `leaf-migrate` \
     with this file: it fetches them from the original messages.";

/// Shown when the server has no series at all. `/import` cannot create one.
const NO_SERIES: &str = "🍂 There's no series in this server to import into yet. Open the \
     gallery to start one, then run `/import` again.";

/// Shown when the write failed. `insert_many` is one transaction, so a
/// failure leaves nothing behind.
const IMPORT_FAILED: &str = "🍂 The import failed and nothing was written, so it's safe to run \
     `/import` again. The details are in leaf's logs.";

/// Shown above the result when Undo import failed. The removal is one
/// transaction as well, and the button is still there.
const UNDO_FAILED: &str = "🍂 The undo failed and nothing was removed, so it's safe to press \
     **Undo import** again. The details are in leaf's logs.";

/// Shown when the attached file could not be fetched from Discord.
const DOWNLOAD_FAILED: &str = "🍂 I couldn't download that file from Discord, so nothing was \
     imported. Run `/import` again and attach it once more.";

/// Shown when the admin lost Manage Server while the prompt was open.
const DEMOTED_IMPORT: &str =
    "🍂 You no longer have Manage Server in this server, so nothing was imported.";
/// The same, on the Undo import button.
const DEMOTED_UNDO: &str =
    "🍂 You no longer have Manage Server in this server, so the import was left as it is.";

// ---------------------------------------------------------------------------
// /export
// ---------------------------------------------------------------------------

/// Export a series' day index as a JSON file. Not a backup: it holds no images.
#[poise::command(
    slash_command,
    guild_only,
    install_context = "Guild",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn export(
    ctx: Context<'_>,
    #[description = "Series to export (you can leave it out when there is only one)"]
    #[autocomplete = "autocomplete_any_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let Some(mut begun) = begin(ctx, series.as_deref(), Scope::Any).await? else {
        return Ok(());
    };
    // Reading the whole series and uploading a file can take longer than
    // the three seconds a first answer has.
    begun.via.defer(true).await?;
    let series = &begun.series;
    let rows = begun.via.data().posts.export_rows(series.id).await?;
    let Some(text) = export_text(series, &rows) else {
        let empty = format!(
            "🍂 {} has no archived days yet, so there is nothing to export.",
            bold(&series.name)
        );
        begun.via.send(Answer::private(empty)).await?;
        return Ok(());
    };

    let posts: Vec<TransferPost> = rows
        .into_iter()
        .map(|row| transfer_post(row, &series.creator_id))
        .collect();
    let bytes = transfer::serialize(&posts)?;
    let zone = localtime::tz_or_utc(&begun.settings.timezone);
    let today = localtime::local_date(checks::now_unix(), zone);
    let file = serenity::CreateAttachment::bytes(bytes, export_filename(series, &today));
    begun
        .via
        .send(Answer {
            content: text,
            // The export itself: `image` is the answer's one attachment.
            image: Some(file),
            ephemeral: true,
            ..Answer::default()
        })
        .await?;
    Ok(())
}

/// One archived day in the transfer format. The format's `user_id` is the
/// series' creator: leaf records who a series belongs to, not who posted.
fn transfer_post(row: ExportRow, creator_id: &str) -> TransferPost {
    TransferPost {
        day: row.day,
        message_id: row.message_id,
        channel_id: row.channel_id,
        user_id: creator_id.to_owned(),
        timestamp: row.posted_at,
        media: row.media_keys,
    }
}

/// The reply that carries the export: what the file is, what it is not,
/// and to save it now. `None` for a series with no archived days.
fn export_text(series: &Series, rows: &[ExportRow]) -> Option<String> {
    let (first, last) = (rows.first()?.day, rows.last()?.day);
    Some(format!(
        "🍃 {}: {}, {}.\n\
         This file is an index, not a backup. It lists each day's number, date and original \
         post, but it holds no images, videos or captions, so importing it somewhere else \
         gives days without media.\n\
         The backup is leaf's data folder (the `leaf.db` database) together with the storage \
         bucket that holds the files. Whoever hosts leaf can copy those.\n\
         Save the file now: only you can see this message, and it goes away when you dismiss \
         it or restart Discord.",
        bold(&series.name),
        noun(rows.len(), "archived day", "archived days"),
        day_span(first, last)
    ))
}

/// `leaf-export-<series>-<date>.json`: safe on every platform, and one
/// export can be told from the next.
fn export_filename(series: &Series, date: &str) -> String {
    let name = slug(&series.name).unwrap_or_else(|| format!("series-{}", series.id));
    format!("leaf-export-{name}-{date}.json")
}

/// `name` as lowercase ASCII letters and digits, with one dash for each run
/// of anything else. `None` when nothing is left (a name in another script,
/// or all emoji): Discord would strip such a name from the file anyway.
fn slug(name: &str) -> Option<String> {
    let mut out = String::new();
    for c in name.chars() {
        if out.len() >= SLUG_MAX_CHARS {
            break;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    (!out.is_empty()).then(|| out.to_owned())
}

// ---------------------------------------------------------------------------
// /import
// ---------------------------------------------------------------------------

/// Import day numbers and dates from a JSON export into a series. Images are not imported.
#[poise::command(
    slash_command,
    guild_only,
    install_context = "Guild",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn import(
    ctx: Context<'_>,
    #[description = "The .json file from /export, or a walpurgisbot export"]
    file: serenity::Attachment,
    #[description = "Existing series to import into (you can leave it out when there is only one)"]
    #[autocomplete = "autocomplete_any_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let poise::Context::Application(app) = ctx else {
        return Ok(());
    };
    // The wrong kind of file is turned away before anything is fetched.
    let refusal = file_refusal(&file.filename, file.content_type.as_deref(), file.size);
    if let Some(refusal) = refusal {
        Via::command(app).send(Answer::private(refusal)).await?;
        return Ok(());
    }
    // `/import` cannot make a series: with none to import into, say where
    // one comes from rather than only that there is none.
    let guild_series = app.data.series.list_by_guild(&settings.guild_id).await?;
    if guild_series.is_empty() {
        Via::command(app).send(Answer::private(NO_SERIES)).await?;
        return Ok(());
    }
    let asker = Asker::of(&ctx).await;
    let found = settle(
        app,
        &settings.guild_id,
        series.as_deref(),
        Scope::Any,
        &asker,
    )
    .await?;
    let Some((series, mut via)) = found else {
        return Ok(());
    };

    // Downloading and reading the file takes longer than a first answer may.
    via.defer(true).await?;
    let label = file_label(&file.filename);
    let Some(posts) = read_file(&mut via, &file, &label).await? else {
        return Ok(());
    };
    let existing = app.data.posts.all_days(series.id).await?;
    let review = review(posts, &existing, checks::now_unix());
    let channels = guild_channels(app);
    let preview = Preview {
        file: &label,
        series: &series,
        review: &review,
        channels: channels.as_ref(),
    };
    if review.fresh.is_empty() {
        via.send(Answer::private(nothing_text(&preview))).await?;
        return Ok(());
    }
    let Some(via) = confirm(app, via, &preview).await? else {
        return Ok(());
    };

    let left_out = review.total - review.fresh.len();
    let job = Job {
        app,
        settings,
        series,
        fresh: review.fresh,
        left_out,
    };
    job.run(via).await
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

/// Why an attachment is not worth downloading, if it is not: the wrong
/// kind of file, an empty one, or one too large to be an export.
fn file_refusal(filename: &str, content_type: Option<&str>, size: u32) -> Option<String> {
    let label = file_label(filename);
    if !looks_like_json(filename, content_type) {
        return Some(format!(
            "🍂 {label} isn't a .json file, so nothing was imported. Run `/import` again and \
             attach the file `/export` gave you, or a walpurgisbot export."
        ));
    }
    if size == 0 {
        return Some(format!(
            "🍂 {label} is empty, so there is nothing to import."
        ));
    }
    if size > MAX_IMPORT_BYTES {
        return Some(format!(
            "🍂 {label} is over the 24 MB import limit. An export of a thousand days is well \
             under 1 MB, so check that it's the right file."
        ));
    }
    None
}

/// The refusal for a file with more entries than one import takes, if it
/// has. Nothing of such a file is imported: half a file would be harder to
/// follow up on than none.
fn too_many_entries(label: &str, entries: usize) -> Option<String> {
    (entries > MAX_IMPORT_ENTRIES).then(|| {
        format!(
            "🍂 {label} has {entries} entries, and one import takes at most \
             {MAX_IMPORT_ENTRIES}, so nothing was imported. Check that it's the right file. \
             If it is, split it into smaller files and import them one at a time."
        )
    })
}

/// Whether an attachment can be an export: named `.json`, or declared by
/// Discord as JSON or as text (an export saved under another name).
fn looks_like_json(filename: &str, content_type: Option<&str>) -> bool {
    let named = filename
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("json"));
    let declared = content_type.is_some_and(|declared| {
        let kind = declared.split(';').next().unwrap_or(declared).trim();
        kind.eq_ignore_ascii_case("application/json")
            || kind
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("text/"))
    });
    named || declared
}

/// A file name as it is shown back: in backticks, without backticks or
/// control characters of its own, and cut when long.
fn file_label(filename: &str) -> String {
    let clean: String = filename
        .chars()
        .filter(|c| *c != '`' && !c.is_control())
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "That file".to_owned()
    } else {
        format!("`{}`", clip(clean, FILE_NAME_MAX_CHARS))
    }
}

/// Downloads and parses the attached file. `None` when it could not be
/// used (unreadable, empty, or more entries than one import takes); the
/// invoker has then been told why.
async fn read_file(
    via: &mut Via<'_>,
    file: &serenity::Attachment,
    label: &str,
) -> Result<Option<Vec<TransferPost>>, Error> {
    let raw = match tokio::time::timeout(DOWNLOAD_TIMEOUT, file.download()).await {
        Ok(Ok(raw)) => raw,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not download an import file");
            via.send(Answer::private(DOWNLOAD_FAILED)).await?;
            return Ok(None);
        }
        Err(_) => {
            tracing::warn!("downloading an import file timed out");
            via.send(Answer::private(DOWNLOAD_FAILED)).await?;
            return Ok(None);
        }
    };
    let body = raw.strip_prefix(UTF8_BOM).unwrap_or(&raw);
    // Bounded while it is read: a crafted file cannot make the parse hold
    // more than the caps allow, whatever its size.
    let text = match transfer::parse_import(body) {
        Ok(import) => match too_many_entries(label, import.entries) {
            Some(refusal) => refusal,
            None if import.posts.is_empty() => {
                format!("🍂 {label} has no entries in it, so there is nothing to import.")
            }
            None => return Ok(Some(import.posts)),
        },
        Err(e) => {
            // The parser's own words are for the operator, not for chat.
            tracing::info!(path = %e.path, error = %e.message, "import file did not parse");
            format!(
                "🍂 {label} doesn't look like a leaf or walpurgisbot export, so nothing was \
                 imported. {} Check that it's the file `/export` gave you, then run `/import` \
                 again.",
                parse_problem(&e)
            )
        }
    };
    via.send(Answer::private(text)).await?;
    Ok(None)
}

/// A parse failure in words an admin can act on: which entry, and what is
/// wrong with it.
fn parse_problem(error: &TransferParseError) -> String {
    let message = error.message.as_str();
    // The parser's wording for a value of the wrong kind. Anything else
    // means the JSON itself is broken.
    let mistyped = ["invalid type", "invalid value", "number out of range"]
        .iter()
        .any(|wording| message.starts_with(wording));
    let Some((entry, field)) = entry_path(&error.path) else {
        // The file as a whole.
        return if mistyped {
            "It should be a list of posts.".to_owned()
        } else {
            "It isn't valid JSON: it may be cut off, or a different kind of file.".to_owned()
        };
    };
    let missing = message
        .strip_prefix("missing field `")
        .and_then(|rest| rest.split('`').next());
    if let Some(missing) = missing {
        return format!("Entry {entry} has no `{missing}`.");
    }
    if !mistyped {
        return format!("The file is cut off or damaged near entry {entry}.");
    }
    match field {
        Some(number @ ("day" | "timestamp")) => {
            format!("Entry {entry} has a `{number}` that isn't a whole number.")
        }
        Some(id @ ("message_id" | "channel_id" | "user_id")) => {
            format!("Entry {entry} has a `{id}` that isn't text: ids go in quotes.")
        }
        Some(other) => format!("Entry {entry} has a `{other}` I can't read."),
        None => format!(
            "Entry {entry} isn't a post: it needs `day`, `message_id`, `channel_id`, `user_id` \
             and `timestamp`."
        ),
    }
}

/// Reads the parser's path (`[3].day`, `[3]`, `.`) as the entry's position
/// counted from one, and the field inside it. `None` for the file itself.
/// The parser writes `?` for a place it cannot name; that is no field.
fn entry_path(path: &str) -> Option<(usize, Option<&str>)> {
    let (index, rest) = path.strip_prefix('[')?.split_once(']')?;
    let entry = index.parse::<usize>().ok()?.checked_add(1)?;
    let field = rest
        .strip_prefix('.')
        .and_then(|field| field.split(['.', '[']).next())
        .filter(|field| !field.is_empty() && *field != "?");
    Some((entry, field))
}

// ---------------------------------------------------------------------------
// Reviewing the entries
// ---------------------------------------------------------------------------

/// Why an entry cannot be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Problem {
    /// The day is not a day number.
    Day,
    /// The date is in milliseconds; the format is in seconds.
    Millis,
    /// The date is before Discord existed, or in the future.
    Timestamp,
    /// The message id is not a snowflake.
    MessageId,
    /// The channel id is not a snowflake.
    ChannelId,
}

impl Problem {
    /// What an entry with this problem has, to follow "N with".
    const fn phrase(self) -> &'static str {
        match self {
            Self::Day => "a day number outside 1 to 999999",
            Self::Millis => "a timestamp in milliseconds instead of seconds",
            Self::Timestamp => "a timestamp that isn't a date between 2015 and today",
            Self::MessageId => "a `message_id` that isn't a Discord id",
            Self::ChannelId => "a `channel_id` that isn't a Discord id",
        }
    }
}

/// A file's entries, sorted by what an import would do with each.
#[derive(Debug, Default, PartialEq, Eq)]
struct Review {
    /// Entries in the file.
    total: usize,
    /// Entries to import: usable, and new to the series. Ascending by day.
    fresh: Vec<TransferPost>,
    /// Usable entries whose day the series already has.
    archived: usize,
    /// Usable entries that repeat a day from earlier in the file.
    repeated: usize,
    /// Entries that cannot be imported: position (from one) and why.
    invalid: Vec<(usize, Problem)>,
}

/// Sorts `posts` against the days the series already has (`existing`,
/// ascending). Every entry lands in exactly one group, so the preview's
/// counts add up to the file and match what the import then writes.
fn review(posts: Vec<TransferPost>, existing: &[i64], now: i64) -> Review {
    let mut review = Review {
        total: posts.len(),
        ..Review::default()
    };
    let mut seen = HashSet::new();
    for (index, post) in posts.into_iter().enumerate() {
        if let Some(problem) = problem(&post, now) {
            review.invalid.push((index.saturating_add(1), problem));
        } else if !seen.insert(post.day) {
            review.repeated += 1;
        } else if existing.binary_search(&post.day).is_ok() {
            review.archived += 1;
        } else {
            review.fresh.push(post);
        }
    }
    review.fresh.sort_by_key(|post| post.day);
    review
}

/// What makes `post` unusable, if anything. Anything that passes can be
/// found with `/search`, removed with `/delete` and dated in the gallery.
fn problem(post: &TransferPost, now: i64) -> Option<Problem> {
    let latest = now.saturating_add(FUTURE_SLACK_SECS);
    let dated = |at: i64| (EARLIEST_POST_UNIX..=latest).contains(&at);
    if !(1..=MAX_DAY).contains(&post.day) {
        Some(Problem::Day)
    } else if !dated(post.timestamp) {
        Some(if dated(post.timestamp / 1_000) {
            Problem::Millis
        } else {
            Problem::Timestamp
        })
    } else if !is_snowflake(&post.message_id) {
        Some(Problem::MessageId)
    } else if !is_snowflake(&post.channel_id) {
        Some(Problem::ChannelId)
    } else {
        None
    }
}

/// Whether `raw` is a Discord id: digits only, and not zero.
fn is_snowflake(raw: &str) -> bool {
    raw.bytes().all(|b| b.is_ascii_digit()) && raw.parse::<u64>().is_ok_and(|id| id != 0)
}

/// The ids of the server's channels and active threads, from the gateway
/// cache. `None` when the server is not cached: nothing can then be said
/// about where an entry comes from.
fn guild_channels(app: App<'_>) -> Option<HashSet<String>> {
    let guild = app
        .serenity_context
        .cache
        .guild(app.interaction.guild_id?)?;
    let ids = guild
        .channels
        .keys()
        .map(ToString::to_string)
        .chain(guild.threads.iter().map(|thread| thread.id.to_string()))
        .collect();
    drop(guild);
    Some(ids)
}

// ---------------------------------------------------------------------------
// The preview
// ---------------------------------------------------------------------------

/// What the preview is written from.
struct Preview<'a> {
    /// The file, as shown (see [`file_label`]).
    file: &'a str,
    /// The series the import goes into.
    series: &'a Series,
    /// The file's entries, sorted.
    review: &'a Review,
    /// The server's channels and threads (see [`guild_channels`]).
    channels: Option<&'a HashSet<String>>,
}

/// The prompt: which series, what the file holds, what an import does and
/// does not bring in, and anything that suggests the wrong file or series.
/// The warnings come last, right above the buttons.
fn preview_text(preview: &Preview<'_>) -> String {
    let series = preview.series;
    let mut lines = vec![
        format!(
            "📥 Import into {} (by <@{}>)",
            bold(&series.name),
            series.creator_id
        ),
        format!(
            "{} has {}:",
            preview.file,
            noun(preview.review.total, "entry", "entries")
        ),
    ];
    lines.extend(tally_lines(preview));
    lines.push(String::new());
    lines.push(IMPORT_SCOPE.to_owned());
    lines.push(MIGRATE_FILLS.to_owned());
    with_warnings(lines, &warnings(preview))
}

/// The reply when the file holds nothing new for the series.
fn nothing_text(preview: &Preview<'_>) -> String {
    let mut lines = vec![format!(
        "🍂 Nothing to import into {}: {} has {}, and none is a new day.",
        bold(&preview.series.name),
        preview.file,
        noun(preview.review.total, "entry", "entries")
    )];
    lines.extend(tally_lines(preview));
    lines.join("\n")
}

/// One line per group of entries that is not empty.
fn tally_lines(preview: &Preview<'_>) -> Vec<String> {
    let review = preview.review;
    let mut lines = Vec::new();
    if let Some(span) = Span::of(&review.fresh) {
        lines.push(format!(
            "- **{}** new: {}, posted {}{}",
            review.fresh.len(),
            day_span(span.first, span.last),
            date_span(span.earliest, span.latest),
            sources_text(&review.fresh, preview.channels)
        ));
    }
    match review.archived {
        0 => {}
        1 => lines.push("- **1** already archived in this series (left as it is)".to_owned()),
        n => lines.push(format!(
            "- **{n}** already archived in this series (left as they are)"
        )),
    }
    match review.repeated {
        0 => {}
        1 => lines.push("- **1** repeats a day from earlier in the file (skipped)".to_owned()),
        n => lines.push(format!(
            "- **{n}** repeat a day from earlier in the file (skipped)"
        )),
    }
    if !review.invalid.is_empty() {
        lines.push(format!(
            "- **{}** can't be imported: {}",
            review.invalid.len(),
            invalid_text(&review.invalid)
        ));
    }
    lines
}

/// The unusable entries by problem: how many, and where the first one is.
fn invalid_text(invalid: &[(usize, Problem)]) -> String {
    let mut by_problem: BTreeMap<Problem, (usize, usize)> = BTreeMap::new();
    for (position, problem) in invalid {
        let (count, _first) = by_problem.entry(*problem).or_insert((0, *position));
        *count += 1;
    }
    let parts: Vec<String> = by_problem
        .into_iter()
        .map(|(problem, (count, first))| {
            let pointer = if count == 1 {
                "entry"
            } else {
                "first at entry"
            };
            format!("{count} with {} ({pointer} {first})", problem.phrase())
        })
        .collect();
    parts.join(", ")
}

/// The day and date range of the entries to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    first: i64,
    last: i64,
    earliest: i64,
    latest: i64,
}

impl Span {
    /// The range of `fresh` (ascending by day); `None` when it is empty.
    fn of(fresh: &[TransferPost]) -> Option<Self> {
        Some(Self {
            first: fresh.first()?.day,
            last: fresh.last()?.day,
            earliest: fresh.iter().map(|post| post.timestamp).min()?,
            latest: fresh.iter().map(|post| post.timestamp).max()?,
        })
    }
}

/// "Day 5", or "Day 1 to Day 953".
fn day_span(first: i64, last: i64) -> String {
    if first == last {
        format!("Day {first}")
    } else {
        format!("Day {first} to Day {last}")
    }
}

/// The posting dates, as timestamps Discord shows in the reader's own
/// language and timezone.
fn date_span(earliest: i64, latest: i64) -> String {
    if earliest == latest {
        format!("<t:{earliest}:D>")
    } else {
        format!("<t:{earliest}:D> to <t:{latest}:D>")
    }
}

/// Where the entries to import were posted: ", from #a and #b". Only
/// channels of this server are named (every channel when the cache cannot
/// tell); the others get a warning instead.
fn sources_text(fresh: &[TransferPost], known: Option<&HashSet<String>>) -> String {
    let mut sources: Vec<&str> = Vec::new();
    for post in fresh {
        let id = post.channel_id.as_str();
        if known.is_none_or(|known| known.contains(id)) && !sources.contains(&id) {
            sources.push(id);
        }
    }
    match sources.as_slice() {
        [] => String::new(),
        [one] => format!(", from <#{one}>"),
        [one, two] => format!(", from <#{one}> and <#{two}>"),
        [one, two, rest @ ..] => format!(
            ", from <#{one}>, <#{two}> and {}",
            noun(rest.len(), "more channel", "more channels")
        ),
    }
}

/// Signs that this is the wrong file or the wrong series.
fn warnings(preview: &Preview<'_>) -> Vec<String> {
    let fresh = preview.review.fresh.as_slice();
    let series = preview.series;
    let mut warnings = Vec::new();
    warnings.extend(author_warning(fresh, &series.creator_id));
    warnings.extend(channel_warning(fresh, preview.channels));
    if let Some(first) = fresh.first().filter(|post| post.day < series.start_day) {
        warnings.push(format!(
            "⚠️ This series starts at Day {}, and the file has days before that (from Day {}). \
             Its creator can lower the first day number in Series settings afterwards.",
            series.start_day, first.day
        ));
    }
    if series.state == SeriesState::Revoked {
        warnings.push(
            "⚠️ This series was revoked by a server admin, so it stays hidden after the \
             import."
                .to_owned(),
        );
    }
    warnings
}

/// The file's posts are by someone other than the series' creator.
fn author_warning(fresh: &[TransferPost], creator_id: &str) -> Option<String> {
    let others: Vec<&str> = fresh
        .iter()
        .map(|post| post.user_id.as_str())
        .filter(|author| *author != creator_id)
        .collect();
    let first = *others.first()?;
    let one_author = others.len() == fresh.len() && others.iter().all(|author| *author == first);
    Some(if one_author && is_snowflake(first) {
        format!(
            "⚠️ The file's posts are by <@{first}>, but this series belongs to <@{creator_id}>."
        )
    } else {
        format!(
            "⚠️ {} by someone other than <@{creator_id}>, who this series belongs to.",
            share(others.len(), fresh.len())
        )
    })
}

/// Some of the file's posts are from channels this server does not have,
/// as far as the cache can tell (an archived thread is not in it either,
/// so the wording stays careful).
fn channel_warning(fresh: &[TransferPost], known: Option<&HashSet<String>>) -> Option<String> {
    let known = known?;
    let unseen = fresh
        .iter()
        .filter(|post| !known.contains(&post.channel_id))
        .count();
    (unseen > 0).then(|| {
        format!(
            "⚠️ {} from a channel I can't see in this server. If the file comes from \
             another server, the links to the original posts won't work.",
            share(unseen, fresh.len())
        )
    })
}

/// "The new days are", "The new day is", or "3 of the 940 new days are".
fn share(part: usize, total: usize) -> String {
    match (part == total, total) {
        (true, 1) => "The new day is".to_owned(),
        (true, _) => "The new days are".to_owned(),
        (false, _) if part == 1 => format!("1 of the {total} new days is"),
        (false, _) => format!("{part} of the {total} new days are"),
    }
}

/// Joins `lines`, then adds, after a blank line, as many of `warnings` as
/// one message holds. The ones that do not fit are counted rather than cut.
fn with_warnings(mut lines: Vec<String>, warnings: &[String]) -> String {
    if warnings.is_empty() {
        return lines.join("\n");
    }
    lines.push(String::new());
    let used: usize = lines.iter().map(|line| line.chars().count() + 1).sum();
    let mut room = MESSAGE_MAX_CHARS.saturating_sub(used + OVERFLOW_ROOM);
    let mut shown = 0_usize;
    for warning in warnings {
        let cost = warning.chars().count() + 1;
        if cost > room {
            break;
        }
        room -= cost;
        lines.push(warning.clone());
        shown += 1;
    }
    match warnings.len().saturating_sub(shown) {
        0 => {}
        1 => lines.push("⚠️ One more warning doesn't fit in this message.".to_owned()),
        left => lines.push(format!(
            "⚠️ {left} more warnings don't fit in this message."
        )),
    }
    lines.join("\n")
}

/// "1 day", "940 days": a count with its noun.
fn noun(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// A count of days.
fn days(n: usize) -> String {
    noun(n, "day", "days")
}

// ---------------------------------------------------------------------------
// The prompt
// ---------------------------------------------------------------------------

/// The invoker's presses on the components `ids`, for `wait`. To be started
/// before the components are shown, so no press is missed.
fn presses(
    app: App<'_>,
    ids: Vec<String>,
    wait: Duration,
) -> impl Stream<Item = serenity::ComponentInteraction> + use<> {
    serenity::ComponentInteractionCollector::new(&app.serenity_context.shard)
        .author_id(app.interaction.user.id)
        .custom_ids(ids)
        .timeout(wait)
        .stream()
}

/// Whether a press comes from someone who may still manage the server, by
/// the permissions Discord sends with it. The command checks this when it
/// is run; its prompts stay open long enough for it to change.
fn may_manage(press: &serenity::ComponentInteraction) -> bool {
    press
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(|granted| granted.administrator() || granted.manage_guild())
}

/// Shows the preview with Import and Cancel and waits [`PROMPT_TIMEOUT`].
/// Gives the surface of the Import press to go on with, or `None` when the
/// import is off (cancelled, expired, or the admin is one no longer); the
/// prompt has then been replaced by a line that says so.
///
/// Stand-in for the shared `checks::confirm_with`.
async fn confirm<'a>(
    app: App<'a>,
    mut via: Via<'a>,
    preview: &Preview<'_>,
) -> Result<Option<Via<'a>>, Error> {
    let interaction = app.interaction.id.to_string();
    let yes = scoped_id(ID_KIND, &[&interaction, "yes"]);
    let no = scoped_id(ID_KIND, &[&interaction, "no"]);
    let _listening = components::SESSIONS.listen(&yes);
    let mut presses = std::pin::pin!(presses(app, vec![yes.clone(), no.clone()], PROMPT_TIMEOUT));
    let buttons = vec![
        serenity::CreateButton::new(yes.clone())
            .style(serenity::ButtonStyle::Primary)
            .label(format!("Import {}", days(preview.review.fresh.len()))),
        serenity::CreateButton::new(no)
            .style(serenity::ButtonStyle::Secondary)
            .label("Cancel"),
    ];
    via.send(Answer {
        content: preview_text(preview),
        components: vec![serenity::CreateActionRow::Buttons(buttons)],
        ephemeral: true,
        ..Answer::default()
    })
    .await?;

    let name = bold(&preview.series.name);
    let Some(press) = presses.next().await else {
        let expired = format!(
            "⏳ This prompt expired, so nothing was imported into {name}. {}",
            rerun_hint(app)
        );
        via.expire(expired).await;
        return Ok(None);
    };
    let confirmed = press.data.custom_id == yes;
    let allowed = may_manage(&press);
    let mut via = Via::pressed(app, press);
    if !confirmed {
        let cancelled = format!("Cancelled. Nothing was imported into {name}.");
        via.send(Answer::private(cancelled)).await?;
        return Ok(None);
    }
    if !allowed {
        via.send(Answer::private(DEMOTED_IMPORT)).await?;
        return Ok(None);
    }
    Ok(Some(via))
}

// ---------------------------------------------------------------------------
// The import
// ---------------------------------------------------------------------------

/// Where a sprout stands after an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sprout {
    /// The series is not a sprout (or the check could not be made).
    NotOne,
    /// The import took it out of its sprout stage.
    Promoted,
    /// Still short of the server's threshold.
    Still { archived: i64, threshold: i64 },
}

/// What an import wrote.
#[derive(Debug, PartialEq, Eq)]
struct Imported {
    /// Days written.
    days: usize,
    /// Days someone archived while the prompt was open; left as they were.
    raced: usize,
    /// The `archived_at` every written day carries. Undo import takes back
    /// only days that still carry it.
    stamp: i64,
    /// What the import meant for a sprout series.
    sprout: Sprout,
    /// For the admin: the log line could not be written.
    note: Option<String>,
}

/// What Undo import removed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Removal {
    /// Imported days deleted again.
    removed: usize,
    /// Days left alone: they hold something else by now.
    changed: usize,
}

/// A confirmed import: what goes in, and where.
struct Job<'a> {
    app: App<'a>,
    settings: GuildSettings,
    /// The target series; re-read just before the write.
    series: Series,
    /// The entries to write, ascending by day. Once written, the entries
    /// Undo import asks to take back: days that turn out to be archived
    /// already at the moment of the write are dropped first.
    fresh: Vec<TransferPost>,
    /// Entries of the file the review left out.
    left_out: usize,
}

impl<'a> Job<'a> {
    /// Imports, then shows the result with its buttons until they expire.
    async fn run(mut self, mut via: Via<'a>) -> Result<(), Error> {
        let working = format!(
            "📥 Importing {} into {}…",
            days(self.fresh.len()),
            bold(&self.series.name)
        );
        // Answered first: with the buttons gone, a second tap cannot
        // import twice. If this fails, nothing has been written.
        via.send(Answer::private(working)).await?;
        match self.write().await {
            Ok(done) => self.offer_undo(via, &done).await,
            Err(refusal) => via.send(Answer::private(refusal)).await?,
        }
        Ok(())
    }

    /// Writes the days in one transaction, then does what follows from
    /// them: the sprout check and the log line. `Err` is the line to show
    /// when nothing was written.
    async fn write(&mut self) -> Result<Imported, String> {
        let data = self.app.data;
        // Minutes may have passed since the preview.
        match data.series.get(self.series.id).await {
            Ok(Some(series)) => self.series = series,
            Ok(None) => {
                return Err(format!(
                    "🍂 {} no longer exists, so nothing was imported.",
                    bold(&self.series.name)
                ));
            }
            Err(e) => return Err(write_failed(self.series.id, &e)),
        }
        // Days archived in those minutes belong to whoever archived them:
        // they are not written, and Undo import is never asked about them.
        let archived = match data.posts.all_days(self.series.id).await {
            Ok(days) => days,
            Err(e) => return Err(write_failed(self.series.id, &e)),
        };
        let early = drop_archived(&mut self.fresh, &archived);
        // An archive aimed at one of these days may be mid-write.
        let keys = flight_keys(self.series.id, &self.fresh);
        let Ok(_flight) = INFLIGHT.try_begin(&keys) else {
            return Err(format!(
                "⏳ leaf is archiving a post into {} right now, so nothing was imported. \
                 Give it a moment, then run `/import` again.",
                bold(&self.series.name)
            ));
        };
        let stamp = checks::now_unix();
        let batch = placeholders(self.series.id, &self.fresh, stamp);
        // `late` counts days that were taken in the instant since that
        // read. They stay in `fresh`, and an undo still leaves them alone:
        // it deletes a day only while it is what this import wrote.
        let (written, late) = match data.posts.insert_many(&batch).await {
            Ok(counts) => counts,
            Err(e) => return Err(write_failed(self.series.id, &e)),
        };
        let raced = early.saturating_add(late);
        if written == 0 {
            return Err(format!(
                "🍂 Nothing was imported into {}: every one of those days was archived while \
                 this prompt was open.",
                bold(&self.series.name)
            ));
        }
        tracing::info!(
            series = self.series.id,
            written,
            raced,
            "days imported without media"
        );

        let sprout = self.sprout_step().await;
        let http = &self.app.serenity_context.http;
        let actor = self.app.interaction.user.id;
        let skipped = self.left_out.saturating_add(raced);
        let line = imported_log_line(&self.series, written, skipped, actor);
        let mut note = log_quiet(http, &self.settings, &line).await;
        if sprout == Sprout::Promoted {
            let sprouted = log_quiet(http, &self.settings, &sprouted_log_line(&self.series)).await;
            note = note.or(sprouted);
        }
        Ok(Imported {
            days: written,
            raced,
            stamp,
            sprout,
            note,
        })
    }

    /// Takes a sprout out of probation when the import brought it to the
    /// server's threshold, as archiving that many posts would have.
    async fn sprout_step(&mut self) -> Sprout {
        if self.series.state != SeriesState::Sprout {
            return Sprout::NotOne;
        }
        let data = self.app.data;
        let id = self.series.id;
        let threshold = self.settings.sprout_threshold;
        let archived = match data.posts.count(id).await {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(series = id, error = %e, "could not count days for sprout check");
                return Sprout::NotOne;
            }
        };
        if archived < threshold {
            return Sprout::Still {
                archived,
                threshold,
            };
        }
        // `self.series` was read before the import was written. Only a
        // series that is still a sprout is promoted: one taken down
        // meanwhile stays as it is.
        match data
            .series
            .set_state_if(id, SeriesState::Sprout, SeriesState::Active)
            .await
        {
            Ok(true) => {
                self.series.state = SeriesState::Active;
                Sprout::Promoted
            }
            Ok(false) => Sprout::NotOne,
            Err(e) => {
                tracing::warn!(series = id, error = %e, "could not promote sprout");
                Sprout::NotOne
            }
        }
    }

    /// Shows the result with Undo import and Open gallery, and answers
    /// them for [`UNDO_WINDOW`]. The buttons are then taken off: nothing
    /// listens after that.
    async fn offer_undo(&self, mut via: Via<'a>, done: &Imported) {
        let app = self.app;
        let (undo, open) = self.result_ids();
        let _listening = components::SESSIONS.listen(&undo);
        let mut presses = std::pin::pin!(presses(app, vec![undo.clone(), open], UNDO_WINDOW));
        let until = checks::now_unix().saturating_add_unsigned(UNDO_WINDOW.as_secs());
        // The days are in: a result the admin dismissed is no failure.
        if let Err(e) = via.send(self.result(done, until, None)).await {
            tracing::warn!(series = self.series.id, error = %e, "could not show an import's result");
            return;
        }
        while let Some(press) = presses.next().await {
            if press.data.custom_id != undo {
                self.open_gallery(&press).await;
            } else if self.undo(press, done, until).await {
                return;
            }
        }
        via.expire(done_text(&self.series, &self.fresh, done, None))
            .await;
    }

    /// The ids of the result's buttons: Undo import, then Open gallery.
    fn result_ids(&self) -> (String, String) {
        let interaction = self.app.interaction.id.to_string();
        (
            scoped_id(ID_KIND, &[&interaction, "undo"]),
            scoped_id(ID_KIND, &[&interaction, "open"]),
        )
    }

    /// The result with its buttons, which are answered until the unix time
    /// `until`. `above` goes in front of it: what a failed undo has to say.
    fn result(&self, done: &Imported, until: i64, above: Option<&str>) -> Answer {
        let (undo, open) = self.result_ids();
        let buttons = vec![
            serenity::CreateButton::new(undo)
                .style(serenity::ButtonStyle::Secondary)
                .label("Undo import"),
            serenity::CreateButton::new(open)
                .style(serenity::ButtonStyle::Secondary)
                .label("Open gallery"),
        ];
        let text = done_text(&self.series, &self.fresh, done, Some(until));
        Answer {
            content: match above {
                Some(above) => format!("{above}\n\n{text}"),
                None => text,
            },
            components: vec![serenity::CreateActionRow::Buttons(buttons)],
            ephemeral: true,
            ..Answer::default()
        }
    }

    /// Takes the imported days out again. Returns whether that settled it:
    /// `false` when the press could not even be acknowledged, or when the
    /// removal failed, so the result and its buttons are up (still, or
    /// again) and another press may come.
    async fn undo(
        &self,
        press: serenity::ComponentInteraction,
        done: &Imported,
        until: i64,
    ) -> bool {
        let app = self.app;
        let allowed = may_manage(&press);
        let mut via = Via::pressed(app, press);
        let name = bold(&self.series.name);
        // Answered first: with the button gone, a second tap finds nothing.
        let opening = if allowed {
            format!("↩️ Undoing the import into {name}…")
        } else {
            DEMOTED_UNDO.to_owned()
        };
        if let Err(e) = via.send(Answer::private(opening)).await {
            tracing::warn!(series = self.series.id, error = %e, "could not answer Undo import");
            return false;
        }
        if !allowed {
            return true;
        }

        // One transaction, and each day only while it is still what this
        // import wrote: the same message and stamp, and no stored file.
        let written: Vec<(i64, &str)> = self
            .fresh
            .iter()
            .map(|entry| (entry.day, entry.message_id.as_str()))
            .collect();
        // The same guard an archive takes: a day being replaced right now
        // is left to that archive.
        let keys = flight_keys(self.series.id, &self.fresh);
        let Ok(_flight) = INFLIGHT.try_begin(&keys) else {
            let again = self.result(done, until, Some(components::BUSY));
            if let Err(e) = via.send(again).await {
                tracing::debug!(error = %e, "could not put an import's result back up");
            }
            return false;
        };
        let taken_back = app
            .data
            .posts
            .delete_placeholders(self.series.id, done.stamp, &written)
            .await;
        let removal = match taken_back {
            Ok((removed, changed)) => Removal { removed, changed },
            Err(e) => {
                tracing::error!(series = self.series.id, error = %e, "undoing an import failed; nothing was removed");
                // Nothing was removed, so the result goes back up with its
                // buttons and the admin can press again.
                let again = self.result(done, until, Some(UNDO_FAILED));
                if let Err(e) = via.send(again).await {
                    tracing::debug!(error = %e, "could not put an import's result back up");
                }
                return false;
            }
        };
        let reverted = done.sprout == Sprout::Promoted && self.back_to_sprout().await;
        let note = if removal.removed > 0 {
            // Logged as the series is now: back in its sprout stage, it is
            // not named.
            let series = Series {
                state: if reverted {
                    SeriesState::Sprout
                } else {
                    self.series.state
                },
                ..self.series.clone()
            };
            let line = undone_log_line(&series, removal.removed, app.interaction.user.id);
            log_quiet(&app.serenity_context.http, &self.settings, &line).await
        } else {
            None
        };
        let text = undone_text(&name, &removal, reverted, note.as_deref());
        // The days are gone already: a result that cannot be shown is no
        // failure to report.
        if let Err(e) = via.send(Answer::private(text)).await {
            tracing::debug!(error = %e, "could not show an undo's outcome");
        }
        true
    }

    /// Puts the series back into its sprout stage when the import that
    /// took it out is undone and it is short of the threshold again.
    /// Returns whether it did. A series that is no longer active (taken
    /// down since) is left as it is.
    async fn back_to_sprout(&self) -> bool {
        let data = self.app.data;
        let id = self.series.id;
        let below = match data.posts.count(id).await {
            Ok(count) => count < self.settings.sprout_threshold,
            Err(e) => {
                tracing::warn!(series = id, error = %e, "could not count days after undo");
                false
            }
        };
        if !below {
            return false;
        }
        match data
            .series
            .set_state_if(id, SeriesState::Active, SeriesState::Sprout)
            .await
        {
            Ok(moved) => moved,
            Err(e) => {
                tracing::warn!(series = id, error = %e, "could not return series to sprout");
                false
            }
        }
    }

    /// Opens the gallery on the series. Discord's launch response carries
    /// no destination, so it is left as a short-lived intent the Activity
    /// collects when it starts. If the launch is refused, the way through
    /// the app launcher is spelled out.
    async fn open_gallery(&self, press: &serenity::ComponentInteraction) {
        let app = self.app;
        let http = &app.serenity_context.http;
        // Importing takes Manage Server, but the gallery shows an admin only
        // what a member may view: for someone else's private series, say so
        // instead of opening the gallery somewhere else.
        let presser = press.user.id.to_string();
        let roles = components::role_ids(press.member.as_ref());
        if !components::gallery_shows(&self.series, &presser, &roles) {
            let text =
                components::not_in_gallery_text(&self.series, app.data.public_url.as_deref());
            let reply = serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(text)
                    .ephemeral(true),
            );
            if let Err(e) = press.create_response(http, reply).await {
                tracing::warn!(error = %e, "could not answer Open gallery");
            }
            return;
        }
        let stored = LaunchIntentRepo::new(app.data.pool.clone())
            .put(
                &app.interaction.user.id.to_string(),
                &self.settings.guild_id,
                self.series.id,
                None,
                checks::now_unix(),
            )
            .await;
        if let Err(e) = stored {
            // The gallery still opens, on its usual first screen.
            tracing::warn!(series = self.series.id, error = %e, "could not store the launch intent");
        }
        let launch = serenity::CreateInteractionResponse::LaunchActivity;
        if let Err(e) = press.create_response(http, launch).await {
            tracing::warn!(error = %e, "could not launch the Activity");
            let fallback = serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(OPEN_GALLERY_FALLBACK)
                    .ephemeral(true),
            );
            if let Err(e) = press.create_response(http, fallback).await {
                tracing::warn!(error = %e, "could not answer Open gallery");
            }
        }
    }
}

/// Logs a failed write for the operator and gives the line for the admin.
fn write_failed(series_id: i64, error: &DbError) -> String {
    tracing::error!(series = series_id, error = %error, "import failed; nothing was written");
    IMPORT_FAILED.to_owned()
}

/// The rows an import writes: each day with its number, date and source
/// message, an empty caption, and one `media_missing` placeholder per media
/// entry of the file (no file is stored, whatever the entry names), up to
/// [`MAX_MEDIA_PER_ENTRY`].
fn placeholders(
    series_id: i64,
    fresh: &[TransferPost],
    now: i64,
) -> Vec<(Post, Vec<NewMediaAttachment>)> {
    fresh
        .iter()
        .map(|entry| {
            let media = (0..entry.media.len().min(MAX_MEDIA_PER_ENTRY))
                .map(|index| NewMediaAttachment {
                    attachment_id: format!("import-{}-{index}", entry.message_id),
                    channel_id: entry.channel_id.clone(),
                    message_id: entry.message_id.clone(),
                    content_type: String::new(),
                    original_key: None,
                    thumb_key: None,
                    media_missing: true,
                })
                .collect();
            let post = Post {
                series_id,
                day: entry.day,
                message_id: entry.message_id.clone(),
                channel_id: entry.channel_id.clone(),
                caption: String::new(),
                posted_at: entry.timestamp,
                archived_at: now,
            };
            (post, media)
        })
        .collect()
}

/// The in-flight keys of the days an import writes or takes back.
fn flight_keys(series_id: i64, entries: &[TransferPost]) -> Vec<FlightKey> {
    entries
        .iter()
        .map(|entry| FlightKey::Day(series_id, entry.day))
        .collect()
}

/// Takes the days in `archived` (ascending) out of `fresh` and returns how
/// many that were: days someone archived between the preview and the write.
/// What is left is what the import writes, and what its undo may take back.
fn drop_archived(fresh: &mut Vec<TransferPost>, archived: &[i64]) -> usize {
    let reviewed = fresh.len();
    fresh.retain(|entry| archived.binary_search(&entry.day).is_err());
    reviewed.saturating_sub(fresh.len())
}

// ---------------------------------------------------------------------------
// Results and log lines
// ---------------------------------------------------------------------------

/// The result of an import: what went in, what the gallery shows for it,
/// and how to get the files after all. While Undo import is up (until the
/// unix time `undo_until`) it points at the button and says when it goes
/// away; once the buttons are gone, at `/delete`.
fn done_text(
    series: &Series,
    fresh: &[TransferPost],
    done: &Imported,
    undo_until: Option<i64>,
) -> String {
    let name = bold(&series.name);
    let range = Span::of(fresh).map_or_else(String::new, |span| {
        format!(": {}", day_span(span.first, span.last))
    });
    let mut lines = vec![format!(
        "📥 Imported {} into {name}{range}.",
        days(done.days)
    )];
    match done.raced {
        0 => {}
        1 => lines.push(
            "1 day was archived by someone else while this prompt was open, and was left as \
             it is."
                .to_owned(),
        ),
        n => lines.push(format!(
            "{n} days were archived by someone else while this prompt was open, and were left \
             as they are."
        )),
    }
    // A relative timestamp: Discord counts it down on the reader's screen.
    let next = undo_until.map_or_else(
        || "To remove an imported day, use `/delete`.".to_owned(),
        |until| {
            format!(
                "If this import was a mistake, press **Undo import** (the button goes away \
                 <t:{until}:R>)."
            )
        },
    );
    lines.push(format!(
        "In the gallery each of these days shows its number and date, with no image and no \
         caption. {MIGRATE_AFTER} {next}"
    ));
    lines.extend(sprout_line(series, done.sprout));
    lines.extend(done.note.as_ref().map(|note| format!("⚠️ {note}")));
    lines.join("\n")
}

/// What the import meant for a sprout series: who sees it now, or how far
/// it still is from that. Same wording as the archive command's lines.
fn sprout_line(series: &Series, sprout: Sprout) -> Option<String> {
    let name = bold(&series.name);
    let (archived, threshold) = match sprout {
        Sprout::NotOne => return None,
        Sprout::Still {
            archived,
            threshold,
        } => (archived, threshold),
        Sprout::Promoted => {
            return Some(match (series.privacy, series.privacy_role_id.as_deref()) {
                (Privacy::Public, _) => format!(
                    "🌿 {name} is out of its sprout stage: everyone in this server can now see \
                     it in the gallery."
                ),
                // Only an id is written as a mention (see `is_snowflake`).
                (Privacy::RoleGated, Some(role)) if is_snowflake(role) => format!(
                    "🌿 {name} is out of its sprout stage: members with <@&{role}> can now see \
                     it in the gallery."
                ),
                (Privacy::RoleGated, _) => format!(
                    "🌿 {name} is out of its sprout stage: members with its role can now see it \
                     in the gallery."
                ),
                (Privacy::CreatorOnly, _) => format!(
                    "🌿 {name} is out of its sprout stage. Its privacy is \"Only me\", so other \
                     members still can't see it."
                ),
            });
        }
    };
    Some(format!(
        "🌱 {name} is still a sprout: {archived} of {threshold} days archived. Until then only \
         its creator can see it in the gallery (server admins see it in the admin panel and \
         in chat commands)."
    ))
}

/// The line that replaces the result once Undo import has run.
fn undone_text(name: &str, removal: &Removal, reverted: bool, note: Option<&str>) -> String {
    let mut lines = Vec::new();
    if removal.removed > 0 {
        lines.push(format!(
            "↩️ Import undone: {} removed from {name}.",
            days(removal.removed)
        ));
    } else {
        lines.push(format!(
            "🍂 Nothing was removed from {name}: none of the imported days is still as the \
             import left it."
        ));
    }
    match removal.changed {
        0 => {}
        1 => lines.push(
            "1 day was left in place: it was archived again or changed since the import."
                .to_owned(),
        ),
        n => lines.push(format!(
            "{n} days were left in place: they were archived again or changed since the import."
        )),
    }
    if reverted {
        lines.push(format!("🌱 {name} is back in its sprout stage."));
    }
    lines.extend(note.map(|note| format!("⚠️ {note}")));
    lines.join("\n")
}

/// A series as the log channel may name it. One that members cannot see
/// (not public, or not active) is not named: the log channel has its own
/// audience. Same rule as the archive and delete lines.
fn log_subject(series: &Series) -> String {
    if private_series(series) {
        "A private series".to_owned()
    } else {
        bold(&series.name)
    }
}

/// The log-channel line for an import. The actor is a mention; it is sent
/// with mention parsing off, so it shows the name and pings nobody.
fn imported_log_line(
    series: &Series,
    written: usize,
    skipped: usize,
    actor: serenity::UserId,
) -> String {
    let skipped = if skipped == 0 {
        String::new()
    } else {
        format!(" · {skipped} skipped")
    };
    format!(
        "📥 {} · {} imported by <@{actor}>, without media{skipped}",
        log_subject(series),
        days(written)
    )
}

/// The log-channel line for an undone import.
fn undone_log_line(series: &Series, removed: usize, actor: serenity::UserId) -> String {
    format!(
        "↩️ {} · import of {} undone by <@{actor}>",
        log_subject(series),
        days(removed)
    )
}

/// The log-channel line for a series the import took out of its sprout
/// stage. Same wording as the archive command's.
fn sprouted_log_line(series: &Series) -> String {
    format!("🌿 {} · out of its sprout stage", log_subject(series))
}

/// Writes `line` to the server's log channel (see
/// [`components::log_line`]). Returns a note for the invoker when the write
/// failed (they hold Manage Server, so they can fix it).
async fn log_quiet(http: &serenity::Http, settings: &GuildSettings, line: &str) -> Option<String> {
    components::log_line(http, settings, line).await
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use leaf_core::domain::{Cadence, DetectionMode};

    use super::*;

    /// The clock the reviews run against (September 2026).
    const NOW: i64 = 1_790_000_000;
    /// When Day 0 of the sample archive would have been posted.
    const START: i64 = 1_700_000_000;

    fn series(name: &str) -> Series {
        Series {
            id: 7,
            guild_id: "1".into(),
            creator_id: "100".into(),
            name: name.into(),
            description: String::new(),
            channels: vec!["200".into()],
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

    /// A usable entry: by the series' creator, in the series' channel.
    fn entry(day: i64) -> TransferPost {
        TransferPost {
            day,
            message_id: (5_000 + day).to_string(),
            channel_id: "200".into(),
            user_id: "100".into(),
            timestamp: START + day * 86_400,
            media: vec![format!(
                "https://cdn.discordapp.com/attachments/1/2/{day}.png"
            )],
        }
    }

    fn entries(days: impl IntoIterator<Item = i64>) -> Vec<TransferPost> {
        days.into_iter().map(entry).collect()
    }

    fn known(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn an_import_is_bounded_in_entries_and_in_files_per_day() {
        // A file of ordinary size passes; one past the cap is refused with
        // its count and what to do.
        assert_eq!(too_many_entries("`x.json`", MAX_IMPORT_ENTRIES), None);
        let refusal = too_many_entries("`x.json`", MAX_IMPORT_ENTRIES + 1).unwrap();
        assert!(refusal.contains("20001 entries"), "{refusal}");
        assert!(refusal.contains("at most 20000"), "{refusal}");
        assert!(refusal.contains("nothing was imported"), "{refusal}");

        // One entry cannot write more rows than a message has files.
        let mut flooded = entry(3);
        flooded.media = vec![String::new(); 5_000];
        let batch = placeholders(7, &[flooded, entry(4)], NOW);
        let rows: Vec<usize> = batch.iter().map(|(_, media)| media.len()).collect();
        assert_eq!(rows, [MAX_MEDIA_PER_ENTRY, 1]);
    }

    fn parse_failure(raw: &[u8]) -> String {
        parse_problem(&transfer::parse_import(raw).unwrap_err())
    }

    // -- the file ------------------------------------------------------------

    #[test]
    fn only_json_files_get_as_far_as_the_download() {
        assert!(looks_like_json("leaf-export-daily-2026-10-02.json", None));
        assert!(looks_like_json(
            "EXPORT.JSON",
            Some("application/octet-stream")
        ));
        assert!(looks_like_json(
            "export",
            Some("application/json; charset=utf-8")
        ));
        // An export saved under another name still reads as text.
        assert!(looks_like_json(
            "export.txt",
            Some("text/plain; charset=utf-8")
        ));
        assert!(!looks_like_json("IMG_0042.png", Some("image/png")));
        assert!(!looks_like_json("archive.zip", None));
        assert!(!looks_like_json("json", None));

        assert_eq!(
            file_refusal("IMG_0042.png", Some("image/png"), 4_000).unwrap(),
            "🍂 `IMG_0042.png` isn't a .json file, so nothing was imported. Run `/import` \
             again and attach the file `/export` gave you, or a walpurgisbot export."
        );
        assert_eq!(
            file_refusal("x.json", None, 0).unwrap(),
            "🍂 `x.json` is empty, so there is nothing to import."
        );
        let huge = file_refusal("x.json", None, MAX_IMPORT_BYTES + 1).unwrap();
        assert!(huge.contains("over the 24 MB import limit"), "{huge}");
        assert_eq!(MAX_IMPORT_BYTES / (1024 * 1024), 24);
        assert_eq!(
            file_refusal("x.json", Some("application/json"), MAX_IMPORT_BYTES),
            None
        );
    }

    #[test]
    fn file_names_are_shown_safely() {
        assert_eq!(file_label("export.json"), "`export.json`");
        assert_eq!(file_label("a`b`\n.json"), "`ab.json`");
        assert_eq!(file_label("``"), "That file");
        let long = format!("{}.json", "x".repeat(200));
        let label = file_label(&long);
        assert_eq!(label.chars().count(), FILE_NAME_MAX_CHARS + 2);
        assert!(label.ends_with("…`"), "{label}");
    }

    #[test]
    fn parse_failures_are_put_in_words() {
        assert_eq!(
            parse_failure(b"not json at all"),
            "It isn't valid JSON: it may be cut off, or a different kind of file."
        );
        assert_eq!(
            parse_failure(br#"{"day": 1}"#),
            "It should be a list of posts."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":1,"message_id":"1","channel_id":"2","user_id":"3","timestamp":4},
                    {"message_id":"1","channel_id":"2","user_id":"3","timestamp":4}]"#
            ),
            "Entry 2 has no `day`."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":"x","message_id":"1","channel_id":"2","user_id":"3","timestamp":4}]"#
            ),
            "Entry 1 has a `day` that isn't a whole number."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":1,"message_id":"1","channel_id":"2","user_id":"3","timestamp":4.5}]"#
            ),
            "Entry 1 has a `timestamp` that isn't a whole number."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":1,"message_id":111,"channel_id":"2","user_id":"3","timestamp":4}]"#
            ),
            "Entry 1 has a `message_id` that isn't text: ids go in quotes."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":1,"message_id":"1","channel_id":"2","user_id":"3","timestamp":4,
                     "media":"a.png"}]"#
            ),
            "Entry 1 has a `media` I can't read."
        );
        assert_eq!(
            parse_failure(b"[1, 2]"),
            "Entry 1 isn't a post: it needs `day`, `message_id`, `channel_id`, `user_id` and \
             `timestamp`."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":1,"message_id":"1","channel_id":"2","user_id":"3","timestamp":4},
                    {"day":2,"message_id":"1","#
            ),
            "The file is cut off or damaged near entry 2."
        );
        // Cut off inside a field: still the file, not the field.
        assert_eq!(
            parse_failure(br#"[{"day":1,"message_id":"1","timestamp":"#),
            "The file is cut off or damaged near entry 1."
        );
        assert_eq!(
            parse_failure(br#"[{"day":1 "message_id":"1"}]"#),
            "The file is cut off or damaged near entry 1."
        );
        assert_eq!(
            parse_failure(
                br#"[{"day":123456789012345678901234567890,"message_id":"1","channel_id":"2",
                     "user_id":"3","timestamp":4}]"#
            ),
            "Entry 1 has a `day` that isn't a whole number."
        );
    }

    #[test]
    fn the_parsers_own_words_never_reach_chat() {
        for raw in [
            &b"oops"[..],
            br#"[{"day":"x"}]"#,
            br#"[{"day":1}]"#,
            b"[[]]",
            br#"[{"day":1,"message_id":"1","channel_id":"2","user_id":"3","timestamp":"#,
        ] {
            let text = parse_failure(raw);
            for leak in ["line ", "column", "expected", "invalid type", "EOF"] {
                assert!(!text.contains(leak), "{text}");
            }
        }
    }

    #[test]
    fn parser_paths_name_the_entry_and_field() {
        assert_eq!(entry_path("."), None);
        assert_eq!(entry_path("?"), None);
        assert_eq!(entry_path("[0]"), Some((1, None)));
        assert_eq!(entry_path("[952].day"), Some((953, Some("day"))));
        assert_eq!(entry_path("[3].media[0]"), Some((4, Some("media"))));
        assert_eq!(entry_path("[1].?"), Some((2, None)));
        assert_eq!(entry_path("[x].day"), None);
    }

    // -- the review ----------------------------------------------------------

    #[test]
    fn every_entry_lands_in_exactly_one_group() {
        let mut posts = entries([3, 1, 2, 2, 5, 5, 5]);
        posts.push(TransferPost { day: 0, ..entry(9) });
        posts.push(TransferPost {
            message_id: "m-1".into(),
            ..entry(8)
        });
        let review = review(posts, &[2, 4], NOW);
        assert_eq!(review.total, 9);
        // New days come out in day order, whatever the file's order.
        let fresh: Vec<i64> = review.fresh.iter().map(|post| post.day).collect();
        assert_eq!(fresh, vec![1, 3, 5]);
        // Day 2 once as "already archived", once as a repeat; Day 5 twice.
        assert_eq!(review.archived, 1);
        assert_eq!(review.repeated, 3);
        assert_eq!(
            review.invalid,
            vec![(8, Problem::Day), (9, Problem::MessageId)]
        );
        assert_eq!(
            review.fresh.len() + review.archived + review.repeated + review.invalid.len(),
            review.total
        );
    }

    #[test]
    fn an_unusable_entry_does_not_claim_its_day() {
        // The first Day 4 is unusable, so the second one is not a repeat.
        let posts = vec![
            TransferPost {
                channel_id: "general".into(),
                ..entry(4)
            },
            entry(4),
        ];
        let review = review(posts, &[], NOW);
        assert_eq!(review.invalid, vec![(1, Problem::ChannelId)]);
        assert_eq!(review.repeated, 0);
        assert_eq!(review.fresh, vec![entry(4)]);
    }

    #[test]
    fn entries_are_checked_for_days_dates_and_ids() {
        let with = |change: fn(&mut TransferPost)| {
            let mut post = entry(10);
            change(&mut post);
            problem(&post, NOW)
        };
        assert_eq!(problem(&entry(10), NOW), None);
        assert_eq!(problem(&entry(1), NOW), None);
        assert_eq!(with(|p| p.day = 0), Some(Problem::Day));
        assert_eq!(with(|p| p.day = -3), Some(Problem::Day));
        assert_eq!(with(|p| p.day = MAX_DAY), None);
        assert_eq!(with(|p| p.day = MAX_DAY + 1), Some(Problem::Day));
        // The same moment in milliseconds is named as such.
        assert_eq!(with(|p| p.timestamp *= 1_000), Some(Problem::Millis));
        assert_eq!(with(|p| p.timestamp = 0), Some(Problem::Timestamp));
        assert_eq!(
            with(|p| p.timestamp = EARLIEST_POST_UNIX - 1),
            Some(Problem::Timestamp)
        );
        assert_eq!(with(|p| p.timestamp = EARLIEST_POST_UNIX), None);
        assert_eq!(with(|p| p.timestamp = NOW + FUTURE_SLACK_SECS), None);
        assert_eq!(
            with(|p| p.timestamp = NOW + FUTURE_SLACK_SECS + 1),
            Some(Problem::Timestamp)
        );
        assert_eq!(with(|p| p.timestamp = i64::MAX), Some(Problem::Timestamp));
        assert_eq!(with(|p| p.timestamp = i64::MIN), Some(Problem::Timestamp));
        assert_eq!(
            with(|p| p.message_id = String::new()),
            Some(Problem::MessageId)
        );
        assert_eq!(
            with(|p| p.message_id = "+12".into()),
            Some(Problem::MessageId)
        );
        assert_eq!(
            with(|p| p.channel_id = "0".into()),
            Some(Problem::ChannelId)
        );
        assert_eq!(
            with(|p| p.channel_id = "99999999999999999999999".into()),
            Some(Problem::ChannelId)
        );
        // Who posted is only reported, never a reason to refuse.
        assert_eq!(with(|p| p.user_id = "someone".into()), None);
    }

    #[test]
    fn the_day_limit_in_the_copy_is_the_parsers() {
        assert!(Problem::Day.phrase().ends_with(&MAX_DAY.to_string()));
    }

    // -- the preview ---------------------------------------------------------

    #[test]
    fn the_preview_shows_what_the_file_holds_and_what_an_import_does() {
        let mut posts = entries(1..=950);
        posts.extend(entries([1, 2]));
        posts.push(TransferPost {
            timestamp: (START + 86_400) * 1_000,
            ..entry(951)
        });
        let review = review(posts, &[10, 11, 12], NOW);
        let series = series("Daily Johan");
        let channels = known(&["200"]);
        let preview = Preview {
            file: "`walpurgis.json`",
            series: &series,
            review: &review,
            channels: Some(&channels),
        };
        let first = START + 86_400;
        let last = START + 950 * 86_400;
        assert_eq!(
            preview_text(&preview),
            format!(
                "📥 Import into **Daily Johan** (by <@100>)\n\
                 `walpurgis.json` has 953 entries:\n\
                 - **947** new: Day 1 to Day 950, posted <t:{first}:D> to <t:{last}:D>, from \
                 <#200>\n\
                 - **3** already archived in this series (left as they are)\n\
                 - **2** repeat a day from earlier in the file (skipped)\n\
                 - **1** can't be imported: 1 with a timestamp in milliseconds instead of \
                 seconds (entry 953)\n\
                 \n\
                 This imports day numbers, dates and links to the original posts, not images, \
                 videos or captions: in the gallery each imported day shows without media.\n\
                 To bring the files in too, ask whoever hosts leaf to run `leaf-migrate` with \
                 this file, before or after this import. It fetches them from the original \
                 messages, for every day whose message still has them."
            )
        );
    }

    #[test]
    fn a_single_new_day_reads_naturally() {
        let review = review(entries([5, 5, 6]), &[6], NOW);
        let series = series("Sketches");
        let preview = Preview {
            file: "`one.json`",
            series: &series,
            review: &review,
            channels: None,
        };
        let text = preview_text(&preview);
        let posted = START + 5 * 86_400;
        assert!(
            text.contains(&format!(
                "- **1** new: Day 5, posted <t:{posted}:D>, from <#200>\n"
            )),
            "{text}"
        );
        assert!(text.contains("- **1** already archived in this series (left as it is)\n"));
        assert!(text.contains("- **1** repeats a day from earlier in the file (skipped)\n"));
    }

    #[test]
    fn nothing_new_is_said_without_a_prompt() {
        let review = review(entries([1, 2, 2]), &[1, 2], NOW);
        let series = series("Daily Johan");
        let preview = Preview {
            file: "`again.json`",
            series: &series,
            review: &review,
            channels: None,
        };
        assert!(review.fresh.is_empty());
        assert_eq!(
            nothing_text(&preview),
            "🍂 Nothing to import into **Daily Johan**: `again.json` has 3 entries, and none \
             is a new day.\n\
             - **2** already archived in this series (left as they are)\n\
             - **1** repeats a day from earlier in the file (skipped)"
        );
    }

    #[test]
    fn unusable_entries_are_grouped_by_what_is_wrong() {
        let invalid = [
            (4, Problem::Millis),
            (2, Problem::Day),
            (9, Problem::Millis),
            (12, Problem::ChannelId),
            (30, Problem::Millis),
        ];
        assert_eq!(
            invalid_text(&invalid),
            "1 with a day number outside 1 to 999999 (entry 2), 3 with a timestamp in \
             milliseconds instead of seconds (first at entry 4), 1 with a `channel_id` that \
             isn't a Discord id (entry 12)"
        );
    }

    #[test]
    fn channels_of_this_server_are_named_and_others_warned_about() {
        let mut posts = entries(1..=6);
        posts[1].channel_id = "201".into();
        posts[2].channel_id = "202".into();
        posts[3].channel_id = "203".into();
        posts[4].channel_id = "999".into();
        // Nothing known about the server: every channel is named, none
        // is warned about.
        assert_eq!(
            sources_text(&posts, None),
            ", from <#200>, <#201> and 3 more channels"
        );
        assert_eq!(channel_warning(&posts, None), None);

        let server = known(&["200", "201", "202"]);
        assert_eq!(
            sources_text(&posts, Some(&server)),
            ", from <#200>, <#201> and 1 more channel"
        );
        assert_eq!(
            channel_warning(&posts, Some(&server)).unwrap(),
            "⚠️ 2 of the 6 new days are from a channel I can't see in this server. If the \
             file comes from another server, the links to the original posts won't work."
        );
        // A file from another server altogether.
        let elsewhere = known(&["1", "2"]);
        assert_eq!(sources_text(&posts, Some(&elsewhere)), "");
        assert!(
            channel_warning(&posts, Some(&elsewhere))
                .unwrap()
                .starts_with("⚠️ The new days are from a channel I can't see")
        );
        assert_eq!(
            sources_text(&posts[..2], Some(&server)),
            ", from <#200> and <#201>"
        );
    }

    #[test]
    fn someone_elses_posts_are_pointed_out() {
        let own = entries(1..=3);
        assert_eq!(author_warning(&own, "100"), None);

        let mut theirs = entries(1..=3);
        for post in &mut theirs {
            post.user_id = "555".into();
        }
        assert_eq!(
            author_warning(&theirs, "100").unwrap(),
            "⚠️ The file's posts are by <@555>, but this series belongs to <@100>."
        );

        let mut mixed = entries(1..=3);
        mixed[2].user_id = "555".into();
        assert_eq!(
            author_warning(&mixed, "100").unwrap(),
            "⚠️ 1 of the 3 new days is by someone other than <@100>, who this series belongs \
             to."
        );
        // An author that is not an id is never turned into a mention.
        let mut odd = entries(1..=2);
        for post in &mut odd {
            post.user_id = "@everyone".into();
        }
        assert_eq!(
            author_warning(&odd, "100").unwrap(),
            "⚠️ The new days are by someone other than <@100>, who this series belongs to."
        );
    }

    #[test]
    fn the_wrong_series_shows_in_the_warnings() {
        let review = review(entries(1..=3), &[], NOW);
        let mut target = series("Late Start");
        target.start_day = 100;
        target.state = SeriesState::Revoked;
        let preview = Preview {
            file: "`f.json`",
            series: &target,
            review: &review,
            channels: None,
        };
        assert_eq!(
            warnings(&preview),
            vec![
                "⚠️ This series starts at Day 100, and the file has days before that (from \
                 Day 1). Its creator can lower the first day number in Series settings \
                 afterwards."
                    .to_owned(),
                "⚠️ This series was revoked by a server admin, so it stays hidden after \
                 the import."
                    .to_owned(),
            ]
        );
        // A matching file and series raise nothing.
        let fine = series("Daily Johan");
        let preview = Preview {
            series: &fine,
            ..preview
        };
        assert_eq!(warnings(&preview), Vec::<String>::new());
        // The warnings sit last, right above the buttons.
        let preview = Preview {
            series: &target,
            ..preview
        };
        assert!(preview_text(&preview).ends_with("hidden after the import."));
    }

    #[test]
    fn the_longest_preview_still_fits_one_message() {
        // Everything at once: a name full of markdown, a long file name,
        // every kind of unusable entry, every warning.
        let mut posts: Vec<TransferPost> = (0..5)
            .flat_map(|kind| {
                (0..3).map(move |n| {
                    let mut post = entry(900_000 + kind * 10 + n);
                    post.timestamp = START;
                    match kind {
                        0 => post.day = 0,
                        1 => post.timestamp = START * 1_000,
                        2 => post.timestamp = 5,
                        3 => post.message_id = "x".into(),
                        _ => post.channel_id = "x".into(),
                    }
                    post
                })
            })
            .collect();
        posts.extend((1..=6).map(|day| TransferPost {
            channel_id: format!("{}", 1_234_567_890_123_456_000 + day),
            user_id: format!("{}", 1_234_567_890_123_456_700 + day % 2),
            ..entry(day)
        }));
        posts.extend(entries([1, 2]));
        let review = review(posts, &[6], NOW);
        assert!(review.archived > 0 && review.repeated > 0);
        assert_eq!(
            invalid_text(&review.invalid)
                .matches("first at entry")
                .count(),
            5
        );
        let mut target = series(&"*_".repeat(20));
        target.creator_id = "1234567890123456789".into();
        target.start_day = 999_999;
        target.state = SeriesState::Revoked;
        let file = file_label(&format!("{}.json", "long name ".repeat(20)));
        let server = known(&["1234567890123456001", "1234567890123456002", "9"]);
        let preview = Preview {
            file: &file,
            series: &target,
            review: &review,
            channels: Some(&server),
        };
        assert_eq!(warnings(&preview).len(), 4);
        let text = preview_text(&preview);
        assert!(
            text.chars().count() <= MESSAGE_MAX_CHARS,
            "{}",
            text.chars().count()
        );
        assert!(text.contains(MIGRATE_FILLS));
    }

    #[test]
    fn warnings_that_do_not_fit_are_counted_not_cut() {
        let body = vec!["x".repeat(MESSAGE_MAX_CHARS - 200)];
        let notes = vec!["⚠️ a".repeat(20), "⚠️ b".repeat(20), "⚠️ c".repeat(20)];
        let text = with_warnings(body, &notes);
        assert!(text.chars().count() <= MESSAGE_MAX_CHARS);
        assert!(text.contains(&notes[0]));
        assert!(!text.contains(&notes[1]));
        assert!(text.ends_with("⚠️ 2 more warnings don't fit in this message."));
        // With room for all of them, nothing is added.
        let text = with_warnings(vec!["short".to_owned()], &notes);
        assert_eq!(text, format!("short\n\n{}", notes.join("\n")));
        assert_eq!(with_warnings(vec!["short".to_owned()], &[]), "short");
    }

    // -- the write and its undo ----------------------------------------------

    #[test]
    fn imported_days_carry_no_files_and_no_caption() {
        let mut two = entry(2);
        two.media = vec!["a".into(), "b".into()];
        let mut bare = entry(3);
        bare.media.clear();
        let batch = placeholders(7, &[two, bare], NOW);
        assert_eq!(batch.len(), 2);

        let (post, media) = &batch[0];
        assert_eq!(
            *post,
            Post {
                series_id: 7,
                day: 2,
                message_id: "5002".into(),
                channel_id: "200".into(),
                caption: String::new(),
                posted_at: START + 2 * 86_400,
                archived_at: NOW,
            }
        );
        let ids: Vec<&str> = media.iter().map(|m| m.attachment_id.as_str()).collect();
        assert_eq!(ids, vec!["import-5002-0", "import-5002-1"]);
        assert!(media.iter().all(|m| {
            m.media_missing
                && m.original_key.is_none()
                && m.thumb_key.is_none()
                && m.content_type.is_empty()
                && m.message_id == "5002"
                && m.channel_id == "200"
        }));
        // An entry without media is a day without media rows.
        assert!(batch[1].1.is_empty());
    }

    #[test]
    fn days_archived_since_the_preview_are_not_the_imports() {
        // Days 2 and 5 were archived while the prompt was open.
        let mut fresh = entries([1, 2, 3, 5, 8]);
        assert_eq!(drop_archived(&mut fresh, &[2, 4, 5, 9]), 2);
        // What is left is what gets written, and what an undo asks about.
        assert_eq!(fresh, entries([1, 3, 8]));
        let batch = placeholders(7, &fresh, NOW);
        let written: Vec<i64> = batch.iter().map(|(post, _)| post.day).collect();
        assert_eq!(written, vec![1, 3, 8]);
        // One stamp for the whole batch: the undo matches on it.
        assert!(batch.iter().all(|(post, _)| post.archived_at == NOW));

        // Nothing archived since: nothing dropped.
        assert_eq!(drop_archived(&mut fresh, &[]), 0);
        assert_eq!(fresh.len(), 3);
        // Everything archived since: nothing left to write.
        assert_eq!(drop_archived(&mut fresh, &[1, 3, 8]), 3);
        assert!(fresh.is_empty());
    }

    // -- results ---------------------------------------------------------------

    fn imported(days: usize) -> Imported {
        Imported {
            days,
            raced: 0,
            stamp: NOW,
            sprout: Sprout::NotOne,
            note: None,
        }
    }

    #[test]
    fn the_result_says_what_the_gallery_shows_and_how_to_get_the_files() {
        let series = series("Daily Johan");
        let fresh = entries(1..=940);
        assert_eq!(
            done_text(&series, &fresh, &imported(940), Some(NOW + 600)),
            "📥 Imported 940 days into **Daily Johan**: Day 1 to Day 940.\n\
             In the gallery each of these days shows its number and date, with no image and \
             no caption. To add the images, ask whoever hosts leaf to run `leaf-migrate` with \
             this file: it fetches them from the original messages. If this import was a \
             mistake, press **Undo import** (the button goes away <t:1790000600:R>)."
        );
        // Once the buttons are gone, the copy stops pointing at them.
        let closed = done_text(&series, &fresh, &imported(940), None);
        assert!(!closed.contains("Undo import"), "{closed}");
        assert!(closed.ends_with("To remove an imported day, use `/delete`."));
    }

    #[test]
    fn the_result_reports_races_sprouts_and_a_failed_log_write() {
        let mut sprout = series("Seedling");
        sprout.state = SeriesState::Sprout;
        let fresh = entries([4]);
        let done = Imported {
            days: 1,
            raced: 2,
            stamp: NOW,
            sprout: Sprout::Still {
                archived: 1,
                threshold: 3,
            },
            note: Some("I couldn't write to the log channel <#9>.".to_owned()),
        };
        let text = done_text(&sprout, &fresh, &done, None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "📥 Imported 1 day into **Seedling**: Day 4.");
        assert_eq!(
            lines[1],
            "2 days were archived by someone else while this prompt was open, and were left \
             as they are."
        );
        assert_eq!(
            lines[3],
            "🌱 **Seedling** is still a sprout: 1 of 3 days archived. Until then only its \
             creator can see it in the gallery (server admins see it in the admin panel and in \
             chat commands)."
        );
        assert_eq!(lines[4], "⚠️ I couldn't write to the log channel <#9>.");
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn a_promoted_sprout_says_who_can_see_it_now() {
        let public = series("Sketches");
        assert_eq!(sprout_line(&public, Sprout::NotOne), None);
        assert_eq!(
            sprout_line(&public, Sprout::Promoted).unwrap(),
            "🌿 **Sketches** is out of its sprout stage: everyone in this server can now see \
             it in the gallery."
        );
        let mut gated = series("Sketches");
        gated.privacy = Privacy::RoleGated;
        gated.privacy_role_id = Some("42".into());
        assert!(
            sprout_line(&gated, Sprout::Promoted)
                .unwrap()
                .contains("members with <@&42> can now see it")
        );
        let mut private = series("Sketches");
        private.privacy = Privacy::CreatorOnly;
        assert!(
            sprout_line(&private, Sprout::Promoted)
                .unwrap()
                .ends_with("so other members still can't see it.")
        );
    }

    #[test]
    fn the_undo_outcome_says_what_was_removed_and_what_stayed() {
        let name = "**Daily Johan**";
        let clean = Removal {
            removed: 940,
            ..Removal::default()
        };
        assert_eq!(
            undone_text(name, &clean, false, None),
            "↩️ Import undone: 940 days removed from **Daily Johan**."
        );
        let partial = Removal {
            removed: 938,
            changed: 2,
        };
        assert_eq!(
            undone_text(name, &partial, true, Some("note")),
            "↩️ Import undone: 938 days removed from **Daily Johan**.\n\
             2 days were left in place: they were archived again or changed since the import.\n\
             🌱 **Daily Johan** is back in its sprout stage.\n\
             ⚠️ note"
        );
        let none = Removal {
            removed: 0,
            changed: 1,
        };
        assert_eq!(
            undone_text(name, &none, false, None),
            "🍂 Nothing was removed from **Daily Johan**: none of the imported days is still \
             as the import left it.\n\
             1 day was left in place: it was archived again or changed since the import."
        );
        // Every imported day was deleted by someone else in the meantime.
        assert_eq!(
            undone_text(name, &Removal::default(), false, None),
            "🍂 Nothing was removed from **Daily Johan**: none of the imported days is still \
             as the import left it."
        );
    }

    #[test]
    fn what_is_said_about_leaf_migrate_is_the_same_before_and_after() {
        // Both lines rest on one fact: leaf-migrate adds the files of days
        // imported here, so neither tells the admin to cancel or undo first.
        for line in [MIGRATE_FILLS, MIGRATE_AFTER] {
            assert!(line.contains("run `leaf-migrate` with this file"), "{line}");
            assert!(line.contains("from the original messages"), "{line}");
            assert!(
                !line.contains("Cancel") && !line.contains("skips"),
                "{line}"
            );
        }
        let done = done_text(&series("Daily Johan"), &entries([1]), &imported(1), None);
        assert!(done.contains(MIGRATE_AFTER), "{done}");
    }

    #[test]
    fn log_lines_name_the_actor_and_hide_series_members_cannot_see() {
        let actor = serenity::UserId::new(42);
        let public = series("Daily_Johan");
        assert_eq!(
            imported_log_line(&public, 940, 13, actor),
            "📥 **Daily\\_Johan** · 940 days imported by <@42>, without media · 13 skipped"
        );
        assert_eq!(
            imported_log_line(&public, 1, 0, actor),
            "📥 **Daily\\_Johan** · 1 day imported by <@42>, without media"
        );
        assert_eq!(
            undone_log_line(&public, 940, actor),
            "↩️ **Daily\\_Johan** · import of 940 days undone by <@42>"
        );
        assert_eq!(
            sprouted_log_line(&public),
            "🌿 **Daily\\_Johan** · out of its sprout stage"
        );

        let mut private = series("Secret Diary");
        private.privacy = Privacy::CreatorOnly;
        let mut sprout = series("Seedling");
        sprout.state = SeriesState::Sprout;
        for hidden in [&private, &sprout] {
            let line = imported_log_line(hidden, 3, 0, actor);
            assert_eq!(
                line,
                "📥 A private series · 3 days imported by <@42>, without media"
            );
            assert!(undone_log_line(hidden, 3, actor).starts_with("↩️ A private series · "));
        }
    }

    // -- /export ---------------------------------------------------------------

    fn row(day: i64) -> ExportRow {
        ExportRow {
            day,
            message_id: (5_000 + day).to_string(),
            channel_id: "200".into(),
            caption: "a caption the format has no room for".into(),
            posted_at: START + day * 86_400,
            media_keys: vec![format!("g/1/s/7/d/{day}/a1")],
        }
    }

    #[test]
    fn exported_days_keep_the_transfer_shape() {
        assert_eq!(
            transfer_post(row(12), "100"),
            TransferPost {
                day: 12,
                message_id: "5012".into(),
                channel_id: "200".into(),
                user_id: "100".into(),
                timestamp: START + 12 * 86_400,
                media: vec!["g/1/s/7/d/12/a1".into()],
            }
        );
        // What /export writes, /import reads back as usable entries.
        let posts: Vec<TransferPost> = [row(1), row(2)]
            .into_iter()
            .map(|row| transfer_post(row, "100"))
            .collect();
        let bytes = transfer::serialize(&posts).unwrap();
        let review = review(transfer::parse(&bytes).unwrap(), &[], NOW);
        assert_eq!(review.fresh, posts);
        assert!(review.invalid.is_empty());
    }

    #[test]
    fn the_export_reply_says_the_file_is_an_index_not_a_backup() {
        let series = series("Daily Johan");
        let rows: Vec<ExportRow> = (1..=953).map(row).collect();
        let text = export_text(&series, &rows).unwrap();
        assert!(
            text.starts_with("🍃 **Daily Johan**: 953 archived days, Day 1 to Day 953.\n"),
            "{text}"
        );
        assert!(text.contains("This file is an index, not a backup."));
        assert!(text.contains("no images, videos or captions"));
        assert!(text.contains("`leaf.db`"));
        assert!(text.contains("Save the file now"));
        assert!(text.chars().count() <= MESSAGE_MAX_CHARS);

        let one = export_text(&series, &[row(5)]).unwrap();
        assert!(one.starts_with("🍃 **Daily Johan**: 1 archived day, Day 5.\n"));
        assert_eq!(export_text(&series, &[]), None);
    }

    #[test]
    fn export_files_are_named_safely_and_dated() {
        let named = |name: &str| export_filename(&series(name), "2026-10-02");
        assert_eq!(
            named("Daily Johan"),
            "leaf-export-daily-johan-2026-10-02.json"
        );
        assert_eq!(
            named("Daily / Johan 🌿"),
            "leaf-export-daily-johan-2026-10-02.json"
        );
        assert_eq!(named("--Été 2024--"), "leaf-export-t-2024-2026-10-02.json");
        // Nothing ASCII to keep: the series id tells the files apart.
        assert_eq!(named("Ежедневник"), "leaf-export-series-7-2026-10-02.json");
        assert_eq!(named("🌿🌿"), "leaf-export-series-7-2026-10-02.json");

        assert_eq!(slug("a".repeat(80).as_str()).unwrap().len(), SLUG_MAX_CHARS);
        let cut = slug(&format!("{} tail", "a".repeat(SLUG_MAX_CHARS - 1))).unwrap();
        assert_eq!(cut, "a".repeat(SLUG_MAX_CHARS - 1));
        for name in ["Daily / Johan 🌿", "x.y/z\\w", "..", "CON"] {
            let file = named(name);
            assert!(
                file.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'),
                "{file}"
            );
        }
    }

    // -- registration ----------------------------------------------------------

    #[test]
    fn the_commands_register_for_server_admins_only() {
        for command in [export(), import()] {
            assert!(command.guild_only, "{}", command.name);
            assert_eq!(
                command.install_context,
                Some(vec![serenity::InstallationContext::Guild])
            );
            assert_eq!(
                command.default_member_permissions,
                serenity::Permissions::MANAGE_GUILD
            );
            assert_eq!(
                command.required_permissions,
                serenity::Permissions::MANAGE_GUILD
            );
            // Discord refuses a description over 100 characters, and with
            // it the whole registration.
            let description = command.description.unwrap();
            assert!(description.chars().count() <= 100, "{description}");
            for parameter in &command.parameters {
                let description = parameter.description.clone().unwrap();
                assert!(description.chars().count() <= 100, "{description}");
            }
        }
        // The file comes first: required options must precede optional ones.
        let import = import();
        let options: Vec<(&str, bool)> = import
            .parameters
            .iter()
            .map(|p| (p.name.as_str(), p.required))
            .collect();
        assert_eq!(options, vec![("file", true), ("series", false)]);
    }

    #[test]
    fn component_ids_fit_discords_limit() {
        for action in ["yes", "no", "undo", "open"] {
            let id = scoped_id(ID_KIND, &["1234567890123456789", action]);
            assert!(id.ends_with(&format!(":import:1234567890123456789:{action}")));
            assert!(id.len() <= 100, "{id}");
        }
    }
}
