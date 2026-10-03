//! Chat-side reading and deleting: `/search`, `/random`, `/status`,
//! `/delete`, and the Remove Archive Entry context menu.
//!
//! Privacy goes through `policy::can_view` on every path (see
//! `series_lookup`): a series the invoker may not see is never named, and
//! an answer is public only for a public, active series. Database lookups
//! come first, so a miss can still be answered privately; storage is only
//! touched after the defer, so a slow fetch cannot outrun Discord's three
//! seconds.

use std::time::Duration;

use leaf_core::db::PostRepo;
use leaf_core::domain::{GuildSettings, MediaAttachment, Post, Privacy, Series, SeriesState};
use leaf_core::series_ops;
use poise::serenity_prelude as serenity;
use serenity::futures::StreamExt as _;

use crate::commands::series_lookup::{
    Answer, App, Asker, CHOICES_MAX, OPTION_LABEL_MAX_CHARS, PROMPT_TIMEOUT, Scope, Via,
    autocomplete_owned_series, autocomplete_viewable_series, bold, clip, not_theirs_text,
    open_gallery_button, rerun_hint, scoped_id, select_one, settle, taken_down_text,
};
use crate::components::{self, FlightKey, INFLIGHT};
use crate::{Context, Data, Error, checks};

/// Longest caption a day card shows; the full text is one tap away.
const CAPTION_MAX_CHARS: usize = 500;
/// Most caption lines a day card shows.
const CAPTION_MAX_LINES: usize = 8;
/// How long a card waits for its thumbnail before going without.
const THUMB_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// File name a card's thumbnail is attached under.
const THUMB_NAME: &str = "thumb.webp";
/// leaf green, on every card.
pub(crate) const EMBED_COLOUR: u32 = 0x006F_BF73;
/// Room for the missing-day list in `/status` (a description holds 4096).
const MISSING_LIST_MAX_CHARS: usize = 2_000;
/// Room for the no-media list in `/status`.
const PLACEHOLDER_LIST_MAX_CHARS: usize = 800;
/// The reaction leaf uses when a series has no emoji of its own.
const FALLBACK_REACTION: &str = "🍃";
/// The warning leaf used to put on a post archived twice.
const LEGACY_WARNING_REACTION: &str = "⚠";

/// How to archive, for a creator whose series is still empty.
const ARCHIVE_HINT: &str = "To archive a post, long-press it (right-click on desktop), then \
     Apps, then Archive to Series.";
/// Remove Archive Entry on a message nothing was archived from.
const NOT_ARCHIVED: &str = "🍂 That message isn't archived in any series.";
/// Remove Archive Entry by someone who may not remove that entry.
const NOT_ALLOWED: &str = "🍂 Only the series creator, a server admin, or the person who \
     posted the message can remove it from the archive.";
/// Remove Archive Entry by the poster, on a series (not their own) that an
/// admin took down. The series is hidden from them, so it is not named.
const TAKEN_DOWN_HIDDEN: &str = "🔒 This message is archived in a series that a server admin \
     revoked, so the entry is read-only. Ask an admin if you'd like it removed.";
/// What a series is called to someone who may not view it.
const HIDDEN_SERIES: &str = "a private series";
/// The question above the entry select.
const ENTRY_PICKER: &str = "🍃 This message is archived more than once. Which entry should I \
     delete?";
/// An entry select whose pick is no longer valid.
const ENTRY_GONE: &str = "🍂 That entry isn't available any more, so nothing happened.";

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Look up one archived day of a series.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn search(
    ctx: Context<'_>,
    #[description = "Day number"]
    #[min = 1]
    #[max = 999_999]
    day: i64,
    #[description = "Series (you can leave it out when there's only one)"]
    #[autocomplete = "autocomplete_viewable_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let Some(mut begun) = begin(ctx, series.as_deref(), Scope::Viewable).await? else {
        return Ok(());
    };
    show_day(&mut begun.via, &begun.asker, &begun.series, day).await
}

/// A random archived day, one with a picture when the series has any.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn random(
    ctx: Context<'_>,
    #[description = "Series (you can leave it out when there's only one)"]
    #[autocomplete = "autocomplete_viewable_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let Some(mut begun) = begin(ctx, series.as_deref(), Scope::Viewable).await? else {
        return Ok(());
    };
    let picked = begun
        .via
        .data()
        .posts
        .random_day_with_media(begun.series.id)
        .await?;
    if let Some(day) = picked {
        show_day(&mut begun.via, &begun.asker, &begun.series, day).await
    } else {
        let text = empty_series_text(&begun.series, &begun.asker);
        begun.via.send(Answer::private(text)).await?;
        Ok(())
    }
}

/// Which days of a series are archived and which are missing.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn status(
    ctx: Context<'_>,
    #[description = "Series (you can leave it out when there's only one)"]
    #[autocomplete = "autocomplete_viewable_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let Some(mut begun) = begin(ctx, series.as_deref(), Scope::Viewable).await? else {
        return Ok(());
    };
    let series = &begun.series;
    let posts = &begun.via.data().posts;
    let days = posts.all_days(series.id).await?;
    if days.is_empty() {
        let text = empty_series_text(series, &begun.asker);
        begun.via.send(Answer::private(text)).await?;
        return Ok(());
    }
    let placeholders = posts.placeholder_days(series.id).await?;
    let latest = posts.latest_posted_at(series.id).await?;
    let embed = serenity::CreateEmbed::new()
        .title(format!(
            "{} {} — archive status",
            series_emoji(series),
            series_ops::display_name(&series.name)
        ))
        .description(status_description(
            series.start_day,
            &days,
            &placeholders,
            latest,
        ))
        .colour(EMBED_COLOUR);
    let answer = Answer {
        embed: Some(embed),
        ephemeral: true,
        ..Answer::default()
    }
    .button(open_gallery_button(Some((series.id, None))));
    begun.via.send(answer).await?;
    Ok(())
}

/// Delete an archived day from your series, stored files included.
#[poise::command(slash_command, guild_only, install_context = "Guild")]
pub async fn delete(
    ctx: Context<'_>,
    #[description = "Day number to delete"]
    #[min = 1]
    #[max = 999_999]
    day: i64,
    #[description = "Series (you can leave it out when you have only one)"]
    #[autocomplete = "autocomplete_owned_series"]
    series: Option<String>,
) -> Result<(), Error> {
    let Some(mut begun) = begin(ctx, series.as_deref(), Scope::Deletable).await? else {
        return Ok(());
    };
    let request = DeleteRequest {
        settings: &begun.settings,
        asker: &begun.asker,
        series: &begun.series,
        day,
        by_author: false,
    };
    confirm_delete(&mut begun.via, &request).await
}

/// Delete the archive entry made from a message. The series creator, a
/// server admin, or the person who posted the message can.
#[poise::command(
    context_menu_command = "Remove Archive Entry",
    guild_only,
    install_context = "Guild"
)]
pub async fn delete_menu(ctx: Context<'_>, msg: serenity::Message) -> Result<(), Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(());
    };
    let poise::Context::Application(app) = ctx else {
        return Ok(());
    };
    let data = ctx.data();
    let asker = Asker::of(&ctx).await;
    let by_author = msg.author.id == ctx.author().id;

    let mut entries = Vec::new();
    for (series_id, day) in data.posts.find_all_by_message(&msg.id.to_string()).await? {
        if let Some(series) = data.series.get(series_id).await?
            && series.guild_id == settings.guild_id
        {
            entries.push((series, day));
        }
    }

    let mut via = Via::command(app);
    let allowed = removable(&entries, &asker, by_author);
    match allowed.as_slice() {
        [] => {
            via.send(Answer::private(entry_refusal(&entries, &asker, by_author)))
                .await?;
            Ok(())
        }
        [(series, day)] => {
            let request = DeleteRequest {
                settings: &settings,
                asker: &asker,
                series,
                day: *day,
                by_author,
            };
            confirm_delete(&mut via, &request).await
        }
        several => pick_entry(via, &settings, &asker, several, by_author).await,
    }
}

/// What every command here has once it may go ahead.
pub(crate) struct Begun<'a> {
    /// The server's settings (set up).
    pub(crate) settings: GuildSettings,
    /// The invoker.
    pub(crate) asker: Asker,
    /// The series the command acts on.
    pub(crate) series: Series,
    /// Where to answer.
    pub(crate) via: Via<'a>,
}

/// The common start: the setup gate, the invoker, and the series within
/// `scope`. `None` when the invoker has been answered already.
pub(crate) async fn begin<'a>(
    ctx: Context<'a>,
    input: Option<&str>,
    scope: Scope,
) -> Result<Option<Begun<'a>>, Error> {
    let Some(settings) = checks::setup_settings(&ctx).await? else {
        return Ok(None);
    };
    let poise::Context::Application(app) = ctx else {
        return Ok(None);
    };
    let asker = Asker::of(&ctx).await;
    let Some((series, via)) = settle(app, &settings.guild_id, input, scope, &asker).await? else {
        return Ok(None);
    };
    Ok(Some(Begun {
        settings,
        asker,
        series,
        via,
    }))
}

// ---------------------------------------------------------------------------
// Day cards
// ---------------------------------------------------------------------------

/// Whether answers about `series` must be private: anything but a public,
/// active series.
pub(crate) fn private_series(series: &Series) -> bool {
    series.privacy != Privacy::Public || series.state != SeriesState::Active
}

/// The series' emoji, or the leaf when it has none.
pub(crate) fn series_emoji(series: &Series) -> &str {
    let emoji = series.emoji.trim();
    if emoji.is_empty() {
        FALLBACK_REACTION
    } else {
        emoji
    }
}

/// A link that opens the message in Discord.
pub(crate) fn jump_link(guild_id: &str, channel_id: &str, message_id: &str) -> String {
    format!("https://discord.com/channels/{guild_id}/{channel_id}/{message_id}")
}

/// "s" unless `n` is one.
pub(crate) const fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A count as `i64`, for [`plural`] and arithmetic on day counts.
fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Shows one archived day: privately "no such day" when it is missing,
/// otherwise the card, public for a public series.
async fn show_day(
    via: &mut Via<'_>,
    asker: &Asker,
    series: &Series,
    day: i64,
) -> Result<(), Error> {
    let data = via.data();
    let Some((post, media)) = data.posts.get(series.id, day).await? else {
        let near = data.posts.neighbor_days(series.id, day).await?;
        via.send(Answer::private(day_miss_text(series, day, near, asker)))
            .await?;
        return Ok(());
    };
    let private = private_series(series);
    via.defer(private).await?;
    let (embed, image) = day_card(data, series, &post, &media, true).await;
    let answer = Answer {
        embed: Some(embed),
        image,
        ephemeral: private,
        ..Answer::default()
    }
    .button(open_gallery_button(Some((series.id, Some(day)))));
    via.send(answer).await?;
    Ok(())
}

/// The day's card and the thumbnail it shows (fetched from storage, so
/// only call this after deferring). `named` is whether the invoker may be
/// told which series it is (see [`may_name`]).
async fn day_card(
    data: &Data,
    series: &Series,
    post: &Post,
    media: &[MediaAttachment],
    named: bool,
) -> (serenity::CreateEmbed, Option<serenity::CreateAttachment>) {
    let shown = media.iter().find(|m| m.thumb_key.is_some());
    let bytes = match shown.and_then(|m| m.thumb_key.as_deref()) {
        Some(key) => fetch_image(data, key).await,
        None => None,
    };
    let preview = match (shown, &bytes) {
        (Some(_), Some(_)) => Preview::Shown,
        (Some(_), None) => Preview::Unavailable,
        (None, _) if media.iter().any(|m| !m.media_missing) => Preview::NoThumbnail,
        (None, _) => Preview::NothingStored,
    };
    let facts = CardFacts {
        files: media.iter().filter(|m| !m.media_missing).count(),
        video: shown.is_some_and(|m| m.content_type.starts_with("video/")),
        preview,
    };
    let jump = jump_link(&series.guild_id, &post.channel_id, &post.message_id);
    let mut embed = serenity::CreateEmbed::new()
        .title(card_title(series, post.day, named))
        .description(day_description(&post.caption, post.posted_at, &jump, facts))
        .colour(EMBED_COLOUR);
    let image = bytes.map(|bytes| serenity::CreateAttachment::bytes(bytes, THUMB_NAME));
    if image.is_some() {
        embed = embed.image(format!("attachment://{THUMB_NAME}"));
    }
    (embed, image)
}

/// A day card's title. The name is escaped so it shows as typed. A series
/// the invoker may not view is not named, and its emoji is not shown.
fn card_title(series: &Series, day: i64, named: bool) -> String {
    if named {
        format!(
            "{} Day {day} — {}",
            series_emoji(series),
            series_ops::display_name(&series.name)
        )
    } else {
        format!("{FALLBACK_REACTION} Day {day} — {HIDDEN_SERIES}")
    }
}

/// A stored image, or `None` (logged) when storage fails or is slow.
async fn fetch_image(data: &Data, key: &str) -> Option<Vec<u8>> {
    match tokio::time::timeout(THUMB_FETCH_TIMEOUT, data.media.get_bytes(key)).await {
        Ok(Ok(bytes)) => Some(bytes),
        Ok(Err(e)) => {
            tracing::warn!(key, error = %e, "thumbnail fetch failed for a card");
            None
        }
        Err(_) => {
            tracing::warn!(key, "thumbnail fetch timed out for a card");
            None
        }
    }
}

/// The thumbnail of a day's first file that has one, if storage has it.
pub(crate) async fn day_thumbnail(data: &Data, series_id: i64, day: i64) -> Option<Vec<u8>> {
    let media = match data.posts.get(series_id, day).await {
        Ok(Some((_, media))) => media,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(series = series_id, day, error = %e, "could not load a day's media");
            return None;
        }
    };
    let key = media.into_iter().find_map(|m| m.thumb_key)?;
    fetch_image(data, &key).await
}

/// How a card's picture turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preview {
    /// The thumbnail is attached and shown.
    Shown,
    /// There is a thumbnail, but storage did not hand it over just now.
    Unavailable,
    /// Files are stored, but none has a thumbnail.
    NoThumbnail,
    /// No file is stored for the day (imported days, lost sources).
    NothingStored,
}

/// What a card says about the day's files.
#[derive(Debug, Clone, Copy)]
struct CardFacts {
    /// Files stored for the day (placeholder rows are not counted: the
    /// gallery has nothing to show for them either).
    files: usize,
    /// The shown file is a video (its poster frame is the picture).
    video: bool,
    /// How the picture turned out.
    preview: Preview,
}

/// The card's text: the caption (clipped), a labelled date with the link to
/// the original post, and a line on what the picture is.
fn day_description(caption: &str, posted_at: i64, jump: &str, facts: CardFacts) -> String {
    let mut lines = vec![format!(
        "Posted <t:{posted_at}:D> · [Jump to the original post]({jump})"
    )];
    let others = if facts.files > 1 {
        format!(" This day has {} files.", facts.files)
    } else {
        String::new()
    };
    match facts.preview {
        Preview::Shown => {
            if facts.video {
                lines.push("🎬 A video: play it in the gallery or in the original post.".into());
            }
            if facts.files > 1 {
                lines.push(format!(
                    "🖼️ Showing 1 of {} files. The gallery has them all.",
                    facts.files
                ));
            }
        }
        Preview::Unavailable => lines.push(format!("Preview unavailable right now.{others}")),
        Preview::NoThumbnail => lines.push(format!("No preview is stored for this day.{others}")),
        Preview::NothingStored => {
            lines.push("No media is stored in the archive for this day.".into());
        }
    }
    let caption = clip_caption(caption);
    if caption.is_empty() {
        lines.join("\n")
    } else {
        format!("{caption}\n\n{}", lines.join("\n"))
    }
}

/// A caption kept short enough for a phone: at most
/// [`CAPTION_MAX_LINES`] lines and [`CAPTION_MAX_CHARS`] characters. A cut
/// closes what it left open, innermost first: a code block would swallow
/// the rest of the card, and a spoiler would show its text in plain view.
fn clip_caption(caption: &str) -> String {
    let caption = caption.trim();
    let mut lines = caption.lines();
    let kept: Vec<&str> = lines.by_ref().take(CAPTION_MAX_LINES).collect();
    let more_lines = lines.next().is_some();
    let joined = kept.join("\n");
    let mut out = clip(&joined, CAPTION_MAX_CHARS);
    let cut = more_lines || out != joined;
    if more_lines && out == joined {
        out.push('…');
    }
    if cut {
        if out.matches("```").count() % 2 == 1 {
            out.push_str("\n```");
        }
        if ends_in_spoiler(&out) {
            out.push_str("||");
        }
    }
    out
}

/// Whether `text` (with its code blocks closed) ends inside a spoiler: an
/// odd number of `||` outside the code blocks, where the marker is only
/// text.
fn ends_in_spoiler(text: &str) -> bool {
    let markers: usize = text
        .split("```")
        .step_by(2)
        .map(|outside| outside.matches("||").count())
        .sum();
    markers % 2 == 1
}

/// The reply for a day that is not archived: the nearest days that are.
fn day_miss_text(
    series: &Series,
    day: i64,
    (previous, next): (Option<i64>, Option<i64>),
    asker: &Asker,
) -> String {
    let nearest = match (previous, next) {
        (Some(p), Some(n)) => format!("The nearest archived days are Day {p} and Day {n}."),
        (Some(p), None) => format!("The latest archived day is Day {p}."),
        (None, Some(n)) => format!("The first archived day is Day {n}."),
        (None, None) => return empty_series_text(series, asker),
    };
    format!("🍂 {} has no Day {day}. {nearest}", bold(&series.name))
}

/// The reply for a series with nothing archived. Its creator also learns
/// how to archive; anyone else just learns it is empty.
pub(crate) fn empty_series_text(series: &Series, asker: &Asker) -> String {
    let name = bold(&series.name);
    if asker.owns(series) {
        format!("🌱 {name} has nothing archived yet. {ARCHIVE_HINT}")
    } else {
        format!("🌱 {name} has nothing archived yet.")
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// `/status` for a series with at least one archived day: the count and
/// span, the latest post, the missing days and the days with no stored
/// media, each as compressed ranges.
fn status_description(
    start_day: i64,
    days: &[i64],
    placeholders: &[i64],
    latest: Option<i64>,
) -> String {
    let (Some(&first), Some(&last)) = (days.first(), days.last()) else {
        return String::new();
    };
    let from = start_day.min(first);
    let archived = count(days.len());
    let mut blocks = Vec::new();

    let summary = if from == last {
        format!(
            "**{archived}** day{} archived: Day {last}.",
            plural(archived)
        )
    } else {
        format!(
            "**{archived}** day{} archived between Day {from} and Day {last}.",
            plural(archived)
        )
    };
    blocks.push(match latest {
        Some(at) => format!("{summary}\nLatest post <t:{at}:R>."),
        None => summary,
    });

    let gaps = missing_ranges(from, days);
    let missing = span_days(&gaps);
    if missing > 0 {
        blocks.push(format!(
            "**Missing ({missing}):** {}",
            ranges_text(&gaps, MISSING_LIST_MAX_CHARS)
        ));
    } else if from < last {
        blocks.push(format!(
            "Nothing is missing between Day {from} and Day {last}. 🌿"
        ));
    }
    if !placeholders.is_empty() {
        blocks.push(format!(
            "**No media stored ({}):** {}",
            placeholders.len(),
            ranges_text(&runs(placeholders), PLACEHOLDER_LIST_MAX_CHARS)
        ));
    }
    blocks.join("\n\n")
}

/// The unarchived stretches of day numbers from `from` up to the last of
/// `days` (ascending, unique), as inclusive ranges.
fn missing_ranges(from: i64, days: &[i64]) -> Vec<(i64, i64)> {
    let mut gaps = Vec::new();
    let mut expected = from;
    for &day in days {
        if day > expected {
            gaps.push((expected, day.saturating_sub(1)));
        }
        expected = expected.max(day.saturating_add(1));
    }
    gaps
}

/// Ascending day numbers grouped into runs of consecutive days.
fn runs(days: &[i64]) -> Vec<(i64, i64)> {
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for &day in days {
        match runs.last_mut() {
            Some((_, end)) if day == end.saturating_add(1) => *end = day,
            Some((_, end)) if day == *end => {}
            _ => runs.push((day, day)),
        }
    }
    runs
}

/// How many days inclusive ranges cover.
fn span_days(ranges: &[(i64, i64)]) -> i64 {
    ranges.iter().fold(0_i64, |total, &(a, b)| {
        total.saturating_add(b.saturating_sub(a).saturating_add(1))
    })
}

/// "Day 3, Days 17–20, Day 101", cut to about `max_chars` with "and N more
/// days" for what did not fit.
fn ranges_text(ranges: &[(i64, i64)], max_chars: usize) -> String {
    let mut text = String::new();
    let mut shown = 0_i64;
    for &(a, b) in ranges {
        let piece = if a == b {
            format!("Day {a}")
        } else {
            format!("Days {a}–{b}")
        };
        if !text.is_empty() {
            if text.len() + 2 + piece.len() > max_chars {
                break;
            }
            text.push_str(", ");
        }
        text.push_str(&piece);
        shown = shown.saturating_add(span_days(&[(a, b)]));
    }
    let rest = span_days(ranges).saturating_sub(shown);
    if rest > 0 {
        format!("{text} and {rest} more day{}", plural(rest))
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// Deleting
// ---------------------------------------------------------------------------

/// One delete the invoker asked for.
struct DeleteRequest<'r> {
    settings: &'r GuildSettings,
    asker: &'r Asker,
    series: &'r Series,
    day: i64,
    /// The invoker posted the archived message, which lets them remove its
    /// entry from a series that is not theirs (see [`may_remove`]).
    by_author: bool,
}

/// How a confirmed delete ended.
enum Outcome {
    /// The day is gone. Carries the series as it was re-read, how many of
    /// the day's stored files were removed and how many were kept (another
    /// archived day still uses them), and a note for an admin when the log
    /// line could not be written.
    Deleted {
        series: Box<Series>,
        removed: usize,
        kept: usize,
        note: Option<String>,
    },
    /// Nothing was deleted, for the reason given.
    Refused(String),
}

/// Whether the invoker may delete an entry of `series`: an admin always;
/// its creator, or the person who posted the archived message
/// (`by_author`), unless an admin took the series down. A taken-down series
/// is read-only for everyone but admins, so having posted the message is no
/// way around the revoke, least of all for the series' own creator.
fn may_remove(asker: &Asker, series: &Series, by_author: bool) -> bool {
    asker.can_delete(series) || (by_author && series.state != SeriesState::Revoked)
}

/// Why the invoker may not delete from `series` (see [`may_remove`]). Only
/// a series they know about is named.
fn removal_refusal(asker: &Asker, series: &Series, by_author: bool) -> String {
    if asker.taken_down_own(series) {
        taken_down_text(series)
    } else if by_author && series.state == SeriesState::Revoked {
        TAKEN_DOWN_HIDDEN.to_owned()
    } else if may_name(series, asker) {
        not_theirs_text(series)
    } else {
        NOT_ALLOWED.to_owned()
    }
}

/// Whether `asker` may be told the name of `series`: they can view it, or
/// it is their own. The poster of an archived message can get as far as a
/// delete prompt on a series that is hidden from them (Remove Archive
/// Entry), and a hidden series is never named.
fn may_name(series: &Series, asker: &Asker) -> bool {
    asker.can_view(series) || asker.owns(series)
}

/// `series` as `asker` is told about it: its name in bold, or "a private
/// series" (see [`may_name`]).
fn subject(series: &Series, asker: &Asker) -> String {
    if may_name(series, asker) {
        bold(&series.name)
    } else {
        HIDDEN_SERIES.to_owned()
    }
}

/// Shows the day with a Delete / Keep prompt, and deletes it on Delete.
/// The prompt waits [`PROMPT_TIMEOUT`]; the outcome replaces it.
async fn confirm_delete(via: &mut Via<'_>, request: &DeleteRequest<'_>) -> Result<(), Error> {
    let (series, day) = (request.series, request.day);
    let data = via.data();
    let named = may_name(series, request.asker);
    let name = subject(series, request.asker);
    let Some((post, media)) = data.posts.get(series.id, day).await? else {
        let text = if named {
            let near = data.posts.neighbor_days(series.id, day).await?;
            day_miss_text(series, day, near, request.asker)
        } else {
            // Found through its message a moment ago, so it was just
            // deleted; the days around it are not theirs to learn.
            already_deleted_text(&name, day)
        };
        via.send(Answer::private(text)).await?;
        return Ok(());
    };
    via.defer(true).await?;
    let (embed, image) = day_card(data, series, &post, &media, named).await;
    let stored = media.iter().filter(|m| !m.media_missing).count();

    let app = via.app();
    let interaction = app.interaction.id.to_string();
    let yes = scoped_id("del", &[&interaction, "yes"]);
    let no = scoped_id("del", &[&interaction, "no"]);
    // Listening starts before the prompt is shown, so no press is missed.
    let _listening = components::SESSIONS.listen(&yes);
    let presses = serenity::ComponentInteractionCollector::new(&app.serenity_context.shard)
        .author_id(app.interaction.user.id)
        .custom_ids(vec![yes.clone(), no.clone()])
        .timeout(PROMPT_TIMEOUT)
        .stream();
    let mut presses = std::pin::pin!(presses);
    let buttons = vec![
        serenity::CreateButton::new(yes.clone())
            .style(serenity::ButtonStyle::Danger)
            .label(format!("Delete Day {day}")),
        serenity::CreateButton::new(no)
            .style(serenity::ButtonStyle::Secondary)
            .label("Keep it"),
    ];
    via.send(Answer {
        content: delete_prompt_text(&name, day, stored),
        embed: Some(embed),
        image,
        components: vec![serenity::CreateActionRow::Buttons(buttons)],
        ephemeral: true,
    })
    .await?;

    let Some(press) = presses.next().await else {
        let text = format!(
            "⏳ This prompt expired, so Day {day} of {name} is still archived. {}",
            rerun_hint(app)
        );
        via.expire(text).await;
        return Ok(());
    };
    let http = &app.serenity_context.http;
    if press.data.custom_id != yes {
        let kept = format!("Kept Day {day} of {name}. Nothing changed.");
        press.create_response(http, plain_update(kept)).await?;
        return Ok(());
    }
    let working = format!("🗑️ Deleting Day {day} of {name}…");
    press.create_response(http, plain_update(working)).await?;

    let outcome = delete_confirmed(app, request, &post).await;
    let text = match &outcome {
        Outcome::Deleted {
            series,
            removed,
            kept,
            note,
        } => deleted_text(
            &subject(series, request.asker),
            day,
            (*removed, *kept),
            note.as_deref(),
        ),
        Outcome::Refused(text) => text.clone(),
    };
    let edit = serenity::EditInteractionResponse::new()
        .content(text)
        .embeds(Vec::new())
        .components(Vec::new())
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    // The delete already happened: a prompt the invoker dismissed in the
    // meantime is no reason to report a failure or to skip the clean-up.
    if let Err(e) = press.edit_response(http, edit).await {
        tracing::debug!(error = %e, "could not show a delete's outcome");
    }
    if let Outcome::Deleted { series, .. } = &outcome {
        unmark(http, &data.posts, series, &post).await;
    }
    Ok(())
}

/// Replaces a prompt with a line of text: no card, no file, no buttons.
fn plain_update(text: String) -> serenity::CreateInteractionResponse {
    serenity::CreateInteractionResponse::UpdateMessage(
        serenity::CreateInteractionResponseMessage::new()
            .content(text)
            .embeds(Vec::new())
            .components(Vec::new())
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
}

/// Deletes the day the prompt showed (`shown`), after checking again that
/// it may: minutes can pass between the prompt and the press.
async fn delete_confirmed(app: App<'_>, request: &DeleteRequest<'_>, shown: &Post) -> Outcome {
    match try_delete(app, request, shown).await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(
                series = request.series.id,
                day = request.day,
                error = format!("{e:#}"),
                "deleting a day failed"
            );
            Outcome::Refused(format!(
                "🍂 Something went wrong on my end, so Day {} of {} is still archived. It's been \
                 logged. Try again in a moment.",
                request.day,
                subject(request.series, request.asker)
            ))
        }
    }
}

async fn try_delete(
    app: App<'_>,
    request: &DeleteRequest<'_>,
    shown: &Post,
) -> Result<Outcome, Error> {
    let data = app.data;
    let day = request.day;
    let asker = request.asker;
    let Some(series) = data.series.get(request.series.id).await? else {
        return Ok(Outcome::Refused(format!(
            "🍂 Day {day} of {} no longer exists, so there was nothing to delete.",
            subject(request.series, asker)
        )));
    };
    if !may_remove(asker, &series, request.by_author) {
        let text = removal_refusal(asker, &series, request.by_author);
        return Ok(Outcome::Refused(text));
    }
    let name = subject(&series, asker);
    // An archive or a renumber of this entry may be mid-write (its files
    // are stored before its row): deleting beside it could release files
    // the entry is about to point at.
    let keys = [
        FlightKey::Message(series.id, shown.message_id.clone()),
        FlightKey::Day(series.id, day),
    ];
    let Ok(_flight) = INFLIGHT.try_begin(&keys) else {
        return Ok(Outcome::Refused(components::BUSY.to_owned()));
    };
    let media = match data.posts.get(series.id, day).await? {
        None => return Ok(Outcome::Refused(already_deleted_text(&name, day))),
        // Renumbered or replaced since the prompt: not what was confirmed.
        Some((now, _)) if now.message_id != shown.message_id => {
            return Ok(Outcome::Refused(format!(
                "🍂 Day {day} of {name} holds a different post now, so I left it alone. {}",
                rerun_hint(app)
            )));
        }
        Some((_, media)) => media,
    };
    // `delete` hands back only the keys no other entry still holds.
    let freed = match data.posts.delete(series.id, day).await {
        Ok(freed) => freed,
        // Someone else deleted it a moment ago (an Undo, another prompt).
        Err(_) if !data.posts.exists(series.id, day).await? => {
            return Ok(Outcome::Refused(already_deleted_text(&name, day)));
        }
        Err(e) => return Err(e.into()),
    };
    data.media.delete_keys(&freed).await;
    let (removed, kept) = freed_files(&media, &freed);
    let line = removed_log_line(&series, day, app.interaction.user.id);
    let note = log_quiet(
        &app.serenity_context.http,
        request.settings,
        asker.is_admin,
        &line,
    )
    .await;
    Ok(Outcome::Deleted {
        series: Box::new(series),
        removed,
        kept,
        note,
    })
}

/// Of a deleted day's stored files, how many left storage and how many
/// stayed, as `(removed, kept)`. `freed` is what `PostRepo::delete` handed
/// back: a file whose key another archived day still holds is kept.
fn freed_files(media: &[MediaAttachment], freed: &[String]) -> (usize, usize) {
    let stored: Vec<&String> = media
        .iter()
        .filter_map(|m| m.original_key.as_ref())
        .collect();
    let removed = stored.iter().filter(|key| freed.contains(key)).count();
    (removed, stored.len().saturating_sub(removed))
}

/// A day someone else deleted first. `name` is the series as the invoker
/// is told about it (see [`subject`]), here and in the texts below.
fn already_deleted_text(name: &str, day: i64) -> String {
    format!("🍂 Day {day} of {name} was already deleted, so there was nothing to do.")
}

/// The question on the delete prompt.
fn delete_prompt_text(name: &str, day: i64, stored: usize) -> String {
    let files = match stored {
        0 => String::new(),
        1 => " Its stored file is deleted too.".to_owned(),
        n => format!(" Its {n} stored files are deleted too."),
    };
    format!(
        "Delete Day {day} of {name} from the archive?{files} The post itself stays in the \
         channel. This can't be undone."
    )
}

/// The line that replaces the prompt once the day is gone: how many stored
/// files went with it, and how many stayed (see [`freed_files`]).
fn deleted_text(
    name: &str,
    day: i64,
    (removed, kept): (usize, usize),
    note: Option<&str>,
) -> String {
    let files = match removed {
        0 => String::new(),
        1 => " and its stored file".to_owned(),
        n => format!(" and its {n} stored files"),
    };
    let stayed = match kept {
        0 => String::new(),
        1 => " 1 stored file was kept: another archived day still uses it.".to_owned(),
        n => format!(" {n} stored files were kept: another archived day still uses them."),
    };
    let mut text = format!("🗑️ Deleted Day {day} of {name}{files}.{stayed}");
    if let Some(note) = note {
        text.push('\n');
        text.push_str(note);
    }
    text
}

/// The entries of one message that the invoker may delete (see
/// [`may_remove`]).
fn removable<'e>(
    entries: &'e [(Series, i64)],
    asker: &Asker,
    by_author: bool,
) -> Vec<&'e (Series, i64)> {
    entries
        .iter()
        .filter(|(series, _)| may_remove(asker, series, by_author))
        .collect()
}

/// Why Remove Archive Entry cannot go ahead on a message: none of its
/// `entries` is the invoker's to delete (see [`may_remove`]). Only their
/// own taken-down series is named.
fn entry_refusal(entries: &[(Series, i64)], asker: &Asker, by_author: bool) -> String {
    if entries.is_empty() {
        return NOT_ARCHIVED.to_owned();
    }
    let taken_down = |(series, _): &(Series, i64)| series.state == SeriesState::Revoked;
    if let Some((series, _)) = entries.iter().find(|(s, _)| asker.taken_down_own(s)) {
        taken_down_text(series)
    } else if by_author && entries.iter().any(taken_down) {
        TAKEN_DOWN_HIDDEN.to_owned()
    } else {
        NOT_ALLOWED.to_owned()
    }
}

/// Asks which of a message's entries to delete, then confirms that one.
async fn pick_entry(
    via: Via<'_>,
    settings: &GuildSettings,
    asker: &Asker,
    entries: &[&(Series, i64)],
    by_author: bool,
) -> Result<(), Error> {
    let options = entries
        .iter()
        .take(CHOICES_MAX)
        .map(|(series, day)| {
            serenity::CreateSelectMenuOption::new(
                entry_label(series, *day, may_name(series, asker)),
                format!("{}:{day}", series.id),
            )
        })
        .collect();
    let picked = select_one(
        via,
        "entry",
        entry_picker_text(entries.len()),
        options,
        "Choose an entry",
    )
    .await?;
    let Some((value, mut via)) = picked else {
        return Ok(());
    };
    let chosen = parse_entry(&value).and_then(|(id, day)| {
        entries
            .iter()
            .find(|(series, entry_day)| series.id == id && *entry_day == day)
    });
    let Some((series, day)) = chosen else {
        via.send(Answer::private(ENTRY_GONE)).await?;
        return Ok(());
    };
    let request = DeleteRequest {
        settings,
        asker,
        series,
        day: *day,
        by_author,
    };
    confirm_delete(&mut via, &request).await
}

/// An entry's label in the select. Labels show text as it is, so the name
/// is not escaped; a series the invoker may not view is not named.
fn entry_label(series: &Series, day: i64, named: bool) -> String {
    let name = if named { &series.name } else { HIDDEN_SERIES };
    clip(&format!("Day {day} of {name}"), OPTION_LABEL_MAX_CHARS)
}

/// The question above the entry select. A select holds 25 options; past
/// that it says how to reach the rest.
fn entry_picker_text(total: usize) -> String {
    if total > CHOICES_MAX {
        format!(
            "{ENTRY_PICKER}\nShowing {CHOICES_MAX} of {total}. Delete one of these, then use \
             Remove Archive Entry on the message again for the rest."
        )
    } else {
        ENTRY_PICKER.to_owned()
    }
}

/// Reads an entry option's value, `<series id>:<day>`.
fn parse_entry(value: &str) -> Option<(i64, i64)> {
    let (series, day) = value.split_once(':')?;
    Some((series.parse().ok()?, day.parse().ok()?))
}

/// Every reaction leaf may have left on an archived post: the series'
/// emoji, its first character (what leaf reacted with before whole emoji
/// were supported), the leaf, and the warning duplicate attempts used to
/// add. Text that cannot be a reaction is left out.
fn marks(emoji: &str) -> Vec<String> {
    let whole = emoji.trim();
    let legacy = whole
        .chars()
        .next()
        .filter(|c| !c.is_ascii())
        .map_or_else(|| FALLBACK_REACTION.to_owned(), String::from);
    let mut marks: Vec<String> = Vec::new();
    for mark in [
        whole.to_owned(),
        legacy,
        FALLBACK_REACTION.to_owned(),
        LEGACY_WARNING_REACTION.to_owned(),
    ] {
        if !mark.is_ascii() && !marks.contains(&mark) {
            marks.push(mark);
        }
    }
    marks
}

/// Takes leaf's reactions off the post once nothing is archived from it any
/// more, so it no longer looks archived. Best effort: a reaction that is
/// not there is refused, which is expected.
async fn unmark(http: &serenity::Http, posts: &PostRepo, series: &Series, post: &Post) {
    match posts.find_all_by_message(&post.message_id).await {
        Ok(rest) if rest.is_empty() => {}
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "could not check a deleted post's other entries");
            return;
        }
    }
    let (Some(channel), Some(message)) = (snowflake(&post.channel_id), snowflake(&post.message_id))
    else {
        return;
    };
    let (channel, message) = (
        serenity::ChannelId::new(channel),
        serenity::MessageId::new(message),
    );
    for mark in marks(&series.emoji) {
        let reaction = serenity::ReactionType::Unicode(mark);
        if let Err(e) = http.delete_reaction_me(channel, message, &reaction).await {
            tracing::debug!(%channel, error = %e, "could not remove an archive reaction");
        }
    }
}

/// A non-zero snowflake (serenity's id types refuse zero).
fn snowflake(raw: &str) -> Option<u64> {
    raw.parse::<u64>().ok().filter(|id| *id != 0)
}

/// The log-channel line for a deleted day. A series members cannot see
/// (not public, or not active) is not named: the log channel has its own
/// audience. Same wording as the archive command's lines.
fn removed_log_line(series: &Series, day: i64, actor: serenity::UserId) -> String {
    let subject = if private_series(series) {
        "A private series".to_owned()
    } else {
        bold(&series.name)
    };
    format!("🗑️ {subject} · Day {day} removed by <@{actor}>")
}

/// Writes `line` to the server's log channel (see
/// [`components::log_line`]). Returns a note for an admin when the write
/// failed.
async fn log_quiet(
    http: &serenity::Http,
    settings: &GuildSettings,
    is_admin: bool,
    line: &str,
) -> Option<String> {
    components::log_line(http, settings, line)
        .await
        .filter(|_| is_admin)
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

    fn series(name: &str, creator: &str) -> Series {
        Series {
            id: 1,
            guild_id: "g".into(),
            creator_id: creator.into(),
            name: name.into(),
            description: String::new(),
            channels: vec![],
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

    fn asker(id: &str, is_admin: bool) -> Asker {
        Asker {
            user_id: id.into(),
            role_ids: vec![],
            is_admin,
        }
    }

    const fn facts(files: usize, video: bool, preview: Preview) -> CardFacts {
        CardFacts {
            files,
            video,
            preview,
        }
    }

    #[test]
    fn only_public_active_series_answer_in_public() {
        let mut s = series("A", "u");
        assert!(!private_series(&s));
        s.state = SeriesState::Sprout;
        assert!(private_series(&s));
        s.state = SeriesState::Active;
        s.privacy = Privacy::RoleGated;
        assert!(private_series(&s));
    }

    #[test]
    fn long_captions_are_clipped_for_phones() {
        assert_eq!(clip_caption("  short  "), "short");
        let long = "x".repeat(900);
        let clipped = clip_caption(&long);
        assert_eq!(clipped.chars().count(), CAPTION_MAX_CHARS);
        assert!(clipped.ends_with('…'));
        let tall = (1..=12)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(clip_caption(&tall), "1\n2\n3\n4\n5\n6\n7\n8…");
        // A cut inside a code block closes it.
        let code = format!("```\n{}", "y\n".repeat(20));
        let clipped = clip_caption(&code);
        assert!(clipped.ends_with("\n```"), "{clipped}");
        // An uncut caption is left as written.
        assert_eq!(clip_caption("```open"), "```open");
        assert_eq!(clip_caption("||open"), "||open");
        // Multi-byte text is cut on character boundaries.
        let wide = "é".repeat(600);
        assert_eq!(clip_caption(&wide).chars().count(), CAPTION_MAX_CHARS);
    }

    #[test]
    fn a_cut_never_uncovers_a_spoiler() {
        // Cut inside a spoiler: it is closed again, so the text stays hidden.
        let secret = format!("||{}||", "s".repeat(900));
        let clipped = clip_caption(&secret);
        assert!(clipped.starts_with("||s"), "{clipped}");
        assert!(clipped.ends_with("s…||"), "{clipped}");
        assert_eq!(clipped.matches("||").count(), 2);
        let tall = format!("||{}||", "line\n".repeat(12));
        assert!(clip_caption(&tall).ends_with("line…||"));
        // A spoiler that closed before the cut is left alone.
        let closed = format!("||hidden|| {}", "x".repeat(900));
        assert!(clip_caption(&closed).ends_with("x…"));
        // Pipes inside a code block are text, not a spoiler.
        let piped = format!("```\na || b\n```\n{}", "x".repeat(900));
        assert!(clip_caption(&piped).ends_with("x…"));
        // A code block cut inside a spoiler: both closed, innermost first.
        let both = format!("||```\n{}", "y\n".repeat(20));
        assert!(clip_caption(&both).ends_with("y…\n```||"));
    }

    #[test]
    fn a_card_title_names_only_what_the_invoker_may_view() {
        let mut s = series("daily_art", "creator");
        s.emoji = "🎨".into();
        assert_eq!(card_title(&s, 7, true), r"🎨 Day 7 — daily\_art");
        assert_eq!(card_title(&s, 7, false), "🍃 Day 7 — a private series");
    }

    #[test]
    fn a_card_says_what_its_picture_is() {
        let jump = "https://discord.com/channels/g/c/m";
        let text = day_description("hi", 1_700_000_000, jump, facts(1, false, Preview::Shown));
        assert_eq!(
            text,
            "hi\n\nPosted <t:1700000000:D> · [Jump to the original post](https://discord.com/channels/g/c/m)"
        );
        let text = day_description("", 1, jump, facts(3, true, Preview::Shown));
        assert!(text.starts_with("Posted <t:1:D>"));
        assert!(text.contains("🎬 A video"));
        assert!(text.contains("Showing 1 of 3 files"));
        let text = day_description("", 1, jump, facts(2, false, Preview::Unavailable));
        assert!(text.ends_with("Preview unavailable right now. This day has 2 files."));
        let text = day_description("", 1, jump, facts(0, false, Preview::NothingStored));
        assert!(text.ends_with("No media is stored in the archive for this day."));
        let text = day_description("", 1, jump, facts(1, false, Preview::NoThumbnail));
        assert!(text.ends_with("No preview is stored for this day."));
    }

    #[test]
    fn a_day_miss_points_at_the_nearest_days() {
        let s = series("Daily *art*", "creator");
        let viewer = asker("viewer", false);
        assert_eq!(
            day_miss_text(&s, 42, (Some(41), Some(45)), &viewer),
            r"🍂 **Daily \*art\*** has no Day 42. The nearest archived days are Day 41 and Day 45."
        );
        assert!(
            day_miss_text(&s, 99, (Some(30), None), &viewer)
                .ends_with("latest archived day is Day 30.")
        );
        assert!(
            day_miss_text(&s, 1, (None, Some(5)), &viewer)
                .ends_with("first archived day is Day 5.")
        );
        assert_eq!(
            day_miss_text(&s, 1, (None, None), &viewer),
            empty_series_text(&s, &viewer)
        );
    }

    #[test]
    fn only_the_creator_is_taught_to_archive_into_an_empty_series() {
        let s = series("A", "creator");
        assert!(empty_series_text(&s, &asker("creator", false)).contains("Archive to Series"));
        assert!(!empty_series_text(&s, &asker("someone", false)).contains("Archive to Series"));
    }

    #[test]
    fn missing_days_become_ranges() {
        assert_eq!(missing_ranges(1, &[1, 2, 3]), vec![]);
        assert_eq!(
            missing_ranges(1, &[3, 4, 7, 10]),
            vec![(1, 2), (5, 6), (8, 9)]
        );
        assert_eq!(missing_ranges(5, &[5, 7]), vec![(6, 6)]);
        assert_eq!(runs(&[2, 3, 4, 9, 11, 12]), vec![(2, 4), (9, 9), (11, 12)]);
        assert_eq!(span_days(&[(1, 2), (5, 5)]), 3);
    }

    #[test]
    fn range_lists_are_capped_with_a_count_of_the_rest() {
        assert_eq!(ranges_text(&[(3, 3), (17, 20)], 100), "Day 3, Days 17–20");
        let many: Vec<(i64, i64)> = (0..200).map(|i| (i * 10, i * 10)).collect();
        let text = ranges_text(&many, 60);
        assert!(text.len() < 100, "{text}");
        assert!(text.starts_with("Day 0, Day 10"));
        assert!(text.ends_with("more days"), "{text}");
        assert!(ranges_text(&[(1, 1), (5, 6)], 6).ends_with(" and 2 more days"));
        assert!(ranges_text(&[(1, 1), (5, 5)], 6).ends_with(" and 1 more day"));
    }

    #[test]
    fn status_summarises_the_whole_series() {
        let text = status_description(1, &[1, 2, 3, 7, 8], &[2], Some(1_700_000_000));
        assert_eq!(
            text,
            "**5** days archived between Day 1 and Day 8.\nLatest post <t:1700000000:R>.\n\n\
             **Missing (3):** Days 4–6\n\n**No media stored (1):** Day 2"
        );
        let text = status_description(1, &[1, 2], &[], None);
        assert_eq!(
            text,
            "**2** days archived between Day 1 and Day 2.\n\nNothing is missing between Day 1 \
             and Day 2. 🌿"
        );
        // A series that starts before its first archived day.
        let text = status_description(1, &[4], &[], None);
        assert!(
            text.contains("**1** day archived between Day 1 and Day 4."),
            "{text}"
        );
        assert!(text.contains("**Missing (3):** Days 1–3"));
        assert_eq!(
            status_description(5, &[5], &[], None),
            "**1** day archived: Day 5."
        );
    }

    #[test]
    fn the_prompt_and_outcome_name_the_day_and_files() {
        let name = "**A**";
        assert_eq!(
            delete_prompt_text(name, 42, 2),
            "Delete Day 42 of **A** from the archive? Its 2 stored files are deleted too. The \
             post itself stays in the channel. This can't be undone."
        );
        assert!(delete_prompt_text(name, 1, 1).contains("Its stored file is deleted too."));
        assert!(!delete_prompt_text(name, 1, 0).contains("stored"));
        assert_eq!(
            deleted_text(name, 42, (2, 0), None),
            "🗑️ Deleted Day 42 of **A** and its 2 stored files."
        );
        assert_eq!(
            deleted_text(name, 42, (0, 0), Some("note")),
            "🗑️ Deleted Day 42 of **A**.\nnote"
        );
        // Files another archived day still uses are not claimed as deleted.
        assert_eq!(
            deleted_text(name, 42, (1, 2), None),
            "🗑️ Deleted Day 42 of **A** and its stored file. 2 stored files were kept: another \
             archived day still uses them."
        );
        assert_eq!(
            deleted_text(name, 42, (0, 1), None),
            "🗑️ Deleted Day 42 of **A**. 1 stored file was kept: another archived day still \
             uses it."
        );
    }

    #[test]
    fn only_files_that_left_storage_count_as_removed() {
        let file = |key: Option<&str>| MediaAttachment {
            id: 1,
            series_id: 1,
            day: 1,
            attachment_id: "a".into(),
            channel_id: "c".into(),
            message_id: "m".into(),
            content_type: "image/png".into(),
            original_key: key.map(str::to_owned),
            thumb_key: key.map(|k| format!("{k}.thumb")),
            media_missing: key.is_none(),
        };
        let media = [file(Some("one")), file(Some("two")), file(None)];
        let freed = ["one".to_owned(), "one.thumb".to_owned()];
        assert_eq!(freed_files(&media, &freed), (1, 1));
        assert_eq!(freed_files(&media, &[]), (0, 2));
        assert_eq!(freed_files(&[], &freed), (0, 0));
    }

    #[test]
    fn a_series_hidden_from_the_poster_is_not_named() {
        let mut hidden = series("Secret Diary", "creator");
        hidden.privacy = Privacy::CreatorOnly;
        let (poster, creator) = (asker("poster", false), asker("creator", false));
        // The poster may remove their post, but never learns the name.
        assert!(may_remove(&poster, &hidden, true));
        assert!(!may_name(&hidden, &poster));
        assert_eq!(subject(&hidden, &poster), "a private series");
        assert_eq!(entry_label(&hidden, 9, false), "Day 9 of a private series");
        let prompt = delete_prompt_text(&subject(&hidden, &poster), 9, 1);
        assert!(prompt.starts_with("Delete Day 9 of a private series from the archive?"));
        assert!(!prompt.contains("Secret"));
        // Its creator and an admin do; so does anyone for a public series.
        assert_eq!(subject(&hidden, &creator), "**Secret Diary**");
        assert_eq!(subject(&hidden, &asker("boss", true)), "**Secret Diary**");
        assert_eq!(
            entry_label(&series("daily_art", "creator"), 9, true),
            "Day 9 of daily_art"
        );
        assert_eq!(subject(&series("Open", "creator"), &poster), "**Open**");
        // A creator may be told about their own taken-down series.
        hidden.state = SeriesState::Revoked;
        assert!(may_name(&hidden, &creator));
    }

    #[test]
    fn the_author_may_remove_their_post_unless_the_series_was_taken_down() {
        let mine = series("Mine", "creator");
        let mut gone = series("Gone", "creator");
        gone.state = SeriesState::Revoked;
        let entries = vec![(mine, 4), (gone, 9)];
        let only_gone = vec![entries[1].clone()];
        // The poster, who made neither series: only the live one. A
        // taken-down series is read-only, and is not named to them.
        let poster = asker("poster", false);
        let allowed = removable(&entries, &poster, true);
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].1, 4);
        assert_eq!(entry_refusal(&only_gone, &poster, true), TAKEN_DOWN_HIDDEN);
        // The creator: only the live series, whether or not they posted the
        // message. Having posted it is no way around the revoke.
        let creator = asker("creator", false);
        for by_author in [false, true] {
            let allowed = removable(&entries, &creator, by_author);
            assert_eq!(allowed.len(), 1, "by_author: {by_author}");
            assert_eq!(allowed[0].1, 4);
            assert!(!may_remove(&creator, &entries[1].0, by_author));
            // They are told why.
            assert!(
                entry_refusal(&only_gone, &creator, by_author)
                    .starts_with("🔒 **Gone** was revoked")
            );
        }
        // An admin: both. A bystander: none, with the plain refusal.
        assert_eq!(removable(&entries, &asker("boss", true), false).len(), 2);
        let bystander = asker("someone", false);
        assert!(removable(&entries, &bystander, false).is_empty());
        assert_eq!(entry_refusal(&entries, &bystander, false), NOT_ALLOWED);
        assert_eq!(entry_refusal(&only_gone, &bystander, false), NOT_ALLOWED);
        assert_eq!(entry_refusal(&[], &bystander, false), NOT_ARCHIVED);
    }

    #[test]
    fn a_refused_delete_names_only_what_the_invoker_knows() {
        let mut s = series("Daily", "creator");
        let (creator, other) = (asker("creator", false), asker("someone", false));
        // Visible, not theirs: `/delete` says whose it is.
        assert_eq!(removal_refusal(&other, &s, false), not_theirs_text(&s));
        // Hidden from them: the name stays out of it.
        s.privacy = Privacy::CreatorOnly;
        assert_eq!(removal_refusal(&other, &s, false), NOT_ALLOWED);
        // Taken down after the prompt was shown.
        s.state = SeriesState::Revoked;
        assert_eq!(removal_refusal(&creator, &s, true), taken_down_text(&s));
        assert_eq!(removal_refusal(&other, &s, true), TAKEN_DOWN_HIDDEN);
    }

    #[test]
    fn a_long_entry_list_says_how_to_reach_the_rest() {
        assert_eq!(entry_picker_text(2), ENTRY_PICKER);
        assert_eq!(entry_picker_text(CHOICES_MAX), ENTRY_PICKER);
        let text = entry_picker_text(30);
        assert!(text.starts_with(ENTRY_PICKER));
        assert!(text.contains("Showing 25 of 30"), "{text}");
    }

    #[test]
    fn entry_values_round_trip() {
        assert_eq!(parse_entry("12:345"), Some((12, 345)));
        assert_eq!(parse_entry("12"), None);
        assert_eq!(parse_entry("a:1"), None);
    }

    #[test]
    fn every_mark_leaf_may_have_left_is_taken_off() {
        assert_eq!(marks("🍃"), vec!["🍃", "⚠"]);
        // Joined emoji: the whole sequence and its first character, which
        // is what older versions reacted with.
        assert_eq!(marks("🧑‍🎨"), vec!["🧑‍🎨", "🧑", "🍃", "⚠"]);
        // Plain text was never a reaction; the leaf stood in for it.
        assert_eq!(marks("x"), vec!["🍃", "⚠"]);
        assert_eq!(marks(""), vec!["🍃", "⚠"]);
    }

    #[test]
    fn log_lines_hide_series_members_cannot_see() {
        let actor = serenity::UserId::new(7);
        let mut s = series("Daily", "u");
        assert_eq!(
            removed_log_line(&s, 3, actor),
            "🗑️ **Daily** · Day 3 removed by <@7>"
        );
        s.privacy = Privacy::CreatorOnly;
        assert_eq!(
            removed_log_line(&s, 3, actor),
            "🗑️ A private series · Day 3 removed by <@7>"
        );
    }

    #[test]
    fn snowflakes_refuse_zero_and_text() {
        assert_eq!(snowflake("42"), Some(42));
        assert_eq!(snowflake("0"), None);
        assert_eq!(snowflake("abc"), None);
    }
}
